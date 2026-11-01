use crate::ir::*;
use crate::shared_text::fmt_float;
use std::cell::{Cell as StdCell, RefCell};
use std::cmp::Ordering;
use std::rc::Rc;

use super::Slot;

pub(super) enum Val {
    Empty,
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    Path(Rc<str>),
    ParseError,
    Agg(Box<Agg>),
    Ref(Ptr),
    Closure(Rc<Clos>),
    Fn(FuncId),
    Cell(Rc<RefCell<Val>>),
    Shared(Rc<Val>),
    Bytes(Rc<RefCell<Vec<u8>>>),
    Addr(usize),
    /// 取得した `Lock` の持ち手（`LockGuard`）。最後の写しの破棄で `Lock` を解放する。
    Guard(Rc<GuardTok>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Tag {
    Tuple,
    List,
    Struct(usize),
    Variant(usize, usize),
}

pub(super) struct Agg {
    pub(super) tag: Tag,
    pub(super) f: Vec<Val>,
    /// 破棄を記録しない（複写・記録を済ませたもの）。
    pub(super) silent: bool,
}

#[derive(Clone)]
pub(super) struct Ptr {
    pub(super) slot: Slot,
    pub(super) path: Vec<u32>,
}

pub(super) struct Clos {
    pub(super) func: FuncId,
    pub(super) env: Slot,
}

/// `LockGuard` の実体。
///
/// 未対応: 現在の言語には `LockGuard` の中身を読み書きする操作がない（`Lock::acquire` の戻り値を
/// 持つだけで、`LockGuard` にメソッドはない）。そのため `Lock` の中身（`LockSt.value`）へは届かない。
/// 持ち手を `Cell` の操作（`SyncSharedGet` など）や破棄グルーに渡しても、`Val::Guard` は
/// 「該当なし」として扱われて panic しない。中身の操作を足すときは、この実体から `LockSt.value` を引く。
///
/// 破棄は `Lock` の解放の知らせになり、スケジューラが次の取得者へ渡す。
/// 異常停止の巻き戻しの途中（`UNWINDING`）での破棄は、その `Lock` を壊れた状態にする（ADR 0026 決定 4）。
pub(super) struct GuardTok {
    pub(super) lock: usize,
}

impl Drop for GuardTok {
    fn drop(&mut self) {
        let unwinding = UNWINDING.try_with(|c| c.get()).unwrap_or(false);
        let _ = LOCK_EVENTS.try_with(|e| e.borrow_mut().push((self.lock, unwinding)));
    }
}

thread_local! {
    /// 異常停止の巻き戻し（後始末の基本ブロック）を実行している間だけ真。
    pub(super) static UNWINDING: StdCell<bool> = const { StdCell::new(false) };
    /// `LockGuard` の破棄の知らせ（`Lock` の番号と、巻き戻しの最中だったか）。
    pub(super) static LOCK_EVENTS: RefCell<Vec<(usize, bool)>> = const { RefCell::new(Vec::new()) };

    pub(super) static STACK_LIMIT: StdCell<(usize, usize)> = const { StdCell::new((0, usize::MAX)) };
    pub(super) static DEEP: StdCell<bool> = const { StdCell::new(false) };
    pub(super) static DROP_LOG: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    pub(super) static TYPES: RefCell<Rc<Vec<TypeDef>>> = RefCell::new(Rc::new(vec![]));
}

#[inline(never)]
pub(super) fn stack_addr() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

fn deep() -> bool {
    let (base, budget) = STACK_LIMIT.with(|c| c.get());
    if base.abs_diff(stack_addr()) > budget {
        DEEP.with(|d| d.set(true));
        true
    } else {
        false
    }
}

pub(super) fn types() -> Rc<Vec<TypeDef>> {
    TYPES.with(|t| t.borrow().clone())
}

// ---------------------------------------------------------------------------
// 場所の読み書き

fn walk<'a>(mut v: &'a Val, path: &[u32]) -> Option<&'a Val> {
    for &i in path {
        match v {
            Val::Agg(a) => v = a.f.get(i as usize)?,
            _ => return None,
        }
    }
    Some(v)
}

fn walk_mut<'a>(mut v: &'a mut Val, path: &[u32]) -> Option<&'a mut Val> {
    for &i in path {
        match v {
            Val::Agg(a) => v = a.f.get_mut(i as usize)?,
            _ => return None,
        }
    }
    Some(v)
}

pub(super) fn with_val<T>(slot: &Slot, path: &[u32], f: impl FnOnce(&Val) -> T) -> T {
    match slot.try_borrow() {
        Ok(b) => match walk(&b, path) {
            Some(v) => f(v),
            None => f(&Val::Empty),
        },
        Err(_) => f(&Val::Empty),
    }
}

pub(super) fn with_val_mut<T>(
    slot: &Slot,
    path: &[u32],
    f: impl FnOnce(&mut Val) -> T,
) -> Option<T> {
    let mut b = slot.try_borrow_mut().ok()?;
    let v = walk_mut(&mut b, path)?;
    Some(f(v))
}

impl Ptr {
    pub(super) fn with<T>(&self, f: impl FnOnce(&Val) -> T) -> T {
        with_val(&self.slot, &self.path, f)
    }
}

pub(super) fn new_slot(v: Val) -> Slot {
    Rc::new(RefCell::new(v))
}

/// 値の位置から参照を 1 段たどる。
pub(super) fn ref_at(slot: &Slot, path: &[u32]) -> Option<Ptr> {
    with_val(slot, path, |v| match v {
        Val::Ref(p) => Some(p.clone()),
        _ => None,
    })
}

pub(super) fn follow_all(slot: &mut Slot, path: &mut Vec<u32>) {
    while let Some(p) = ref_at(slot, path) {
        *slot = p.slot;
        *path = p.path;
    }
}

pub(super) fn locate(locals: &[Slot], pl: &Place) -> (Slot, Vec<u32>) {
    let mut s = locals[pl.base].clone();
    let mut p: Vec<u32> = vec![];
    for pr in &pl.proj {
        match pr {
            Proj::Field(i) => {
                follow_all(&mut s, &mut p);
                p.push(*i as u32);
            }
            Proj::Downcast(_) => follow_all(&mut s, &mut p),
            Proj::Deref => {
                if let Some(q) = ref_at(&s, &p) {
                    s = q.slot;
                    p = q.path;
                }
            }
        }
    }
    (s, p)
}

// ---------------------------------------------------------------------------
// 複写・破棄

pub(super) fn shadow(v: &Val) -> Val {
    match v {
        Val::Empty => Val::Empty,
        Val::Unit => Val::Unit,
        Val::Int(n) => Val::Int(*n),
        Val::Addr(a) => Val::Addr(*a),
        Val::Float(x) => Val::Float(*x),
        Val::Bool(b) => Val::Bool(*b),
        Val::Str(s) => Val::Str(s.clone()),
        Val::Path(s) => Val::Path(s.clone()),
        Val::ParseError => Val::ParseError,
        Val::Agg(a) => Val::Agg(Box::new(Agg {
            tag: a.tag,
            f: a.f.iter().map(shadow).collect(),
            silent: true,
        })),
        Val::Ref(p) => Val::Ref(p.clone()),
        Val::Closure(c) => Val::Closure(c.clone()),
        Val::Fn(f) => Val::Fn(*f),
        Val::Cell(c) => Val::Cell(c.clone()),
        Val::Shared(s) => Val::Shared(s.clone()),
        Val::Bytes(b) => Val::Bytes(b.clone()),
        Val::Guard(g) => Val::Guard(g.clone()),
    }
}

/// `clone`。破棄される値が増える（`Cell` は独立した複製、`Shared` は同じものを指す）。
pub(super) fn deep_clone(v: &Val) -> Val {
    match v {
        Val::Agg(a) => Val::Agg(Box::new(Agg {
            tag: a.tag,
            f: a.f.iter().map(deep_clone).collect(),
            silent: false,
        })),
        Val::Cell(c) => Val::Cell(Rc::new(RefCell::new(deep_clone(&c.borrow())))),
        Val::Bytes(b) => Val::Bytes(Rc::new(RefCell::new(b.borrow().clone()))),
        Val::Ref(p) => p.with(deep_clone),
        other => shadow(other),
    }
}

fn log_drop(a: &Agg) {
    if a.silent || !matches!(a.tag, Tag::Struct(_)) {
        return;
    }
    if a.f.iter().any(|v| matches!(v, Val::Empty)) {
        return;
    }
    let on = DROP_LOG.try_with(|l| l.borrow().is_some()).unwrap_or(false);
    if !on {
        return;
    }
    let ts = types();
    let text = show_agg(&ts, a);
    let _ = DROP_LOG.try_with(|l| {
        if let Some(v) = l.borrow_mut().as_mut() {
            v.push(text);
        }
    });
}

/// 深く入れ子になった値を再帰せずに破棄する。順序は、値そのもの、次にフィールド・要素を宣言の順。
fn drain(mut stack: Vec<Val>) {
    stack.reverse();
    while let Some(v) = stack.pop() {
        match v {
            Val::Agg(mut a) => {
                log_drop(&a);
                a.silent = true;
                let mut f = std::mem::take(&mut a.f);
                f.reverse();
                stack.extend(f);
            }
            Val::Cell(rc) => {
                if let Ok(inner) = Rc::try_unwrap(rc) {
                    stack.push(inner.into_inner());
                }
            }
            Val::Shared(rc) => {
                if let Ok(inner) = Rc::try_unwrap(rc) {
                    stack.push(inner);
                }
            }
            Val::Closure(rc) => {
                if let Ok(c) = Rc::try_unwrap(rc) {
                    if let Ok(env) = Rc::try_unwrap(c.env) {
                        stack.push(env.into_inner());
                    }
                }
            }
            _ => {}
        }
    }
}

impl Drop for Agg {
    fn drop(&mut self) {
        log_drop(self);
        self.silent = true;
        if !self.f.is_empty() {
            drain(std::mem::take(&mut self.f));
        }
    }
}

// ---------------------------------------------------------------------------
// 表示・比較

fn show_agg(ts: &[TypeDef], a: &Agg) -> String {
    match a.tag {
        Tag::Tuple => {
            let xs: Vec<String> = a.f.iter().map(|v| show_val(ts, v, true)).collect();
            format!("({})", xs.join(", "))
        }
        Tag::List => {
            let xs: Vec<String> = a.f.iter().map(|v| show_val(ts, v, true)).collect();
            format!("[{}]", xs.join(", "))
        }
        Tag::Struct(t) => {
            let def = &ts[t];
            let names: Vec<&str> = match &def.kind {
                TypeKind::Struct { fields } => fields.iter().map(|(n, _)| n.as_str()).collect(),
                _ => vec![],
            };
            let xs: Vec<String> =
                a.f.iter()
                    .enumerate()
                    .map(|(i, v)| {
                        format!(
                            "{}: {}",
                            names.get(i).copied().unwrap_or("?"),
                            show_val(ts, v, true)
                        )
                    })
                    .collect();
            format!("{} {{ {} }}", def.name, xs.join(", "))
        }
        Tag::Variant(t, i) => {
            let def = &ts[t];
            let vname = match &def.kind {
                TypeKind::Enum { variants } => variants[i].name.as_str(),
                _ => "?",
            };
            let head = if def.name == "Option" || def.name == "Result" {
                vname.to_string()
            } else {
                format!("{}.{}", def.name, vname)
            };
            if a.f.is_empty() {
                head
            } else {
                let xs: Vec<String> = a.f.iter().map(|v| show_val(ts, v, true)).collect();
                format!("{head}({})", xs.join(", "))
            }
        }
    }
}

pub(super) fn show_val(ts: &[TypeDef], v: &Val, quote: bool) -> String {
    if deep() {
        return String::new();
    }
    match v {
        Val::Empty | Val::Unit => "()".into(),
        Val::Int(n) => n.to_string(),
        Val::Addr(a) => format!("Addr({a})"),
        Val::Float(f) => fmt_float(*f),
        Val::Bool(b) => b.to_string(),
        Val::Str(s) | Val::Path(s) => {
            if quote {
                format!("{:?}", &**s)
            } else {
                s.to_string()
            }
        }
        Val::ParseError => "ParseError".into(),
        Val::Agg(a) => show_agg(ts, a),
        Val::Ref(p) => p.with(|x| show_val(ts, x, quote)),
        Val::Closure(_) | Val::Fn(_) => "<関数>".into(),
        Val::Cell(c) => format!("Cell({})", show_val(ts, &c.borrow(), true)),
        Val::Shared(s) => format!("Shared({})", show_val(ts, s, true)),
        Val::Bytes(b) => format!("Bytes({:?})", *b.borrow()),
        Val::Guard(_) => "LockGuard".into(),
    }
}

fn val_eq(a: &Val, b: &Val) -> bool {
    if deep() {
        return false;
    }
    match (a, b) {
        (Val::Ref(p), _) => p.with(|x| val_eq(x, b)),
        (_, Val::Ref(q)) => q.with(|y| val_eq(a, y)),
        (Val::Unit, Val::Unit) | (Val::ParseError, Val::ParseError) => true,
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Addr(x), Val::Addr(y)) => x == y,
        (Val::Float(x), Val::Float(y)) => x == y,
        (Val::Bool(x), Val::Bool(y)) => x == y,
        (Val::Str(x), Val::Str(y)) | (Val::Path(x), Val::Path(y)) => x == y,
        (Val::Agg(x), Val::Agg(y)) => {
            x.tag == y.tag
                && x.f.len() == y.f.len()
                && x.f.iter().zip(&y.f).all(|(p, q)| val_eq(p, q))
        }
        (Val::Cell(x), Val::Cell(y)) => Rc::ptr_eq(x, y) || val_eq(&x.borrow(), &y.borrow()),
        (Val::Shared(x), Val::Shared(y)) => Rc::ptr_eq(x, y) || val_eq(x, y),
        (Val::Bytes(x), Val::Bytes(y)) => *x.borrow() == *y.borrow(),
        _ => false,
    }
}

pub(super) fn cmp_val(a: &Val, b: &Val) -> Option<Ordering> {
    if deep() {
        return None;
    }
    match (a, b) {
        (Val::Ref(p), _) => p.with(|x| cmp_val(x, b)),
        (_, Val::Ref(q)) => q.with(|y| cmp_val(a, y)),
        (Val::Int(x), Val::Int(y)) => Some(x.cmp(y)),
        (Val::Addr(x), Val::Addr(y)) => Some(x.cmp(y)),
        (Val::Float(x), Val::Float(y)) => x.partial_cmp(y),
        (Val::Bool(x), Val::Bool(y)) => Some(x.cmp(y)),
        (Val::Str(x), Val::Str(y)) => Some(x.cmp(y)),
        (Val::Bytes(x), Val::Bytes(y)) => Some(x.borrow().cmp(&*y.borrow())),
        (Val::Agg(x), Val::Agg(y)) if x.tag == y.tag && matches!(x.tag, Tag::Tuple | Tag::List) => {
            for (p, q) in x.f.iter().zip(&y.f) {
                match cmp_val(p, q)? {
                    Ordering::Equal => {}
                    o => return Some(o),
                }
            }
            Some(x.f.len().cmp(&y.f.len()))
        }
        _ => None,
    }
}

pub(super) fn compare(op: BinOp, a: &Val, b: &Val) -> bool {
    match op {
        BinOp::Eq => val_eq(a, b),
        BinOp::Ne => !val_eq(a, b),
        _ => {
            let o = cmp_val(a, b);
            match op {
                BinOp::Lt => o == Some(Ordering::Less),
                BinOp::Le => matches!(o, Some(Ordering::Less | Ordering::Equal)),
                BinOp::Gt => o == Some(Ordering::Greater),
                _ => matches!(o, Some(Ordering::Greater | Ordering::Equal)),
            }
        }
    }
}

/// 参照を最後までたどった値の写し。
pub(super) fn deref_val(v: Val) -> Val {
    let mut v = v;
    while let Val::Ref(p) = &v {
        let n = p.with(shadow);
        v = n;
    }
    v
}

pub(super) fn list_of(items: Vec<Val>) -> Val {
    Val::Agg(Box::new(Agg {
        tag: Tag::List,
        f: items,
        silent: false,
    }))
}

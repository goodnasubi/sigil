//! SIR の評価器（ADR 0015 の決定 2・5）。`lower` が作った `Program` を、値・出力・破棄の順・停止を観測として返して実行する。
//!
//! 値の持ち方:
//! - 局所はすべて `Slot`（`Rc<RefCell<Val>>`）。組・構造体・列挙型の腕・一覧は `Agg`（フィールドの並び）。
//! - 参照は `Ptr`（スロットと、その中の位置の列）。借用の間は元の値が動かないことを検査器が保証する。
//! - 移動は「読んで元を `Empty` にする」。`Drop` は `Empty` に対して何もしない。破棄は型でなく値の形から行う。
//! - 構造体の破棄は、自分、次にフィールドを宣言の順（`Agg` の `Drop` と `drain`）。移動した部分を持つ構造体は、自分を記録しない（Rust と同じ）。
//! - 複写（`Copy`）は破棄を記録しない写し（`silent`）。`clone` だけが破棄される値を増やす。

use crate::ast::{File, Item, Type};
use crate::exec::{Arg, Failure, Options, Outcome, RunError, Stop, StopKind};
use crate::ir::*;
use crate::shared_args::arg_name;
use crate::shared_path as spath;
use crate::shared_text::{self as stext, fmt_float};
use std::cell::{Cell as StdCell, RefCell};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;

type Slot = Rc<RefCell<Val>>;
type E = Box<Stop>;

enum Val {
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tag {
    Tuple,
    List,
    Struct(usize),
    Variant(usize, usize),
}

struct Agg {
    tag: Tag,
    f: Vec<Val>,
    /// 破棄を記録しない（複写・記録を済ませたもの）。
    silent: bool,
}

#[derive(Clone)]
struct Ptr {
    slot: Slot,
    path: Vec<u32>,
}

struct Clos {
    func: FuncId,
    env: Slot,
}

thread_local! {
    static STACK_LIMIT: StdCell<(usize, usize)> = const { StdCell::new((0, usize::MAX)) };
    static DEEP: StdCell<bool> = const { StdCell::new(false) };
    static DROP_LOG: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    static TYPES: RefCell<Rc<Vec<TypeDef>>> = RefCell::new(Rc::new(vec![]));
}

#[inline(never)]
fn stack_addr() -> usize {
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

fn types() -> Rc<Vec<TypeDef>> {
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

fn with_val<T>(slot: &Slot, path: &[u32], f: impl FnOnce(&Val) -> T) -> T {
    match slot.try_borrow() {
        Ok(b) => match walk(&b, path) {
            Some(v) => f(v),
            None => f(&Val::Empty),
        },
        Err(_) => f(&Val::Empty),
    }
}

fn with_val_mut<T>(slot: &Slot, path: &[u32], f: impl FnOnce(&mut Val) -> T) -> Option<T> {
    let mut b = slot.try_borrow_mut().ok()?;
    let v = walk_mut(&mut b, path)?;
    Some(f(v))
}

impl Ptr {
    fn with<T>(&self, f: impl FnOnce(&Val) -> T) -> T {
        with_val(&self.slot, &self.path, f)
    }
}

fn new_slot(v: Val) -> Slot {
    Rc::new(RefCell::new(v))
}

/// 値の位置から参照を 1 段たどる。
fn ref_at(slot: &Slot, path: &[u32]) -> Option<Ptr> {
    with_val(slot, path, |v| match v {
        Val::Ref(p) => Some(p.clone()),
        _ => None,
    })
}

fn follow_all(slot: &mut Slot, path: &mut Vec<u32>) {
    while let Some(p) = ref_at(slot, path) {
        *slot = p.slot;
        *path = p.path;
    }
}

fn locate(locals: &[Slot], pl: &Place) -> (Slot, Vec<u32>) {
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

fn shadow(v: &Val) -> Val {
    match v {
        Val::Empty => Val::Empty,
        Val::Unit => Val::Unit,
        Val::Int(n) => Val::Int(*n),
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
    }
}

/// `clone`。破棄される値が増える（`Cell` は独立した複製、`Shared` は同じものを指す）。
fn deep_clone(v: &Val) -> Val {
    match v {
        Val::Agg(a) => Val::Agg(Box::new(Agg {
            tag: a.tag,
            f: a.f.iter().map(deep_clone).collect(),
            silent: false,
        })),
        Val::Cell(c) => Val::Cell(Rc::new(RefCell::new(deep_clone(&c.borrow())))),
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

fn show_val(ts: &[TypeDef], v: &Val, quote: bool) -> String {
    if deep() {
        return String::new();
    }
    match v {
        Val::Empty | Val::Unit => "()".into(),
        Val::Int(n) => n.to_string(),
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
        _ => false,
    }
}

fn cmp_val(a: &Val, b: &Val) -> Option<Ordering> {
    if deep() {
        return None;
    }
    match (a, b) {
        (Val::Ref(p), _) => p.with(|x| cmp_val(x, b)),
        (_, Val::Ref(q)) => q.with(|y| cmp_val(a, y)),
        (Val::Int(x), Val::Int(y)) => Some(x.cmp(y)),
        (Val::Float(x), Val::Float(y)) => x.partial_cmp(y),
        (Val::Bool(x), Val::Bool(y)) => Some(x.cmp(y)),
        (Val::Str(x), Val::Str(y)) => Some(x.cmp(y)),
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

fn compare(op: BinOp, a: &Val, b: &Val) -> bool {
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
fn deref_val(v: Val) -> Val {
    let mut v = v;
    while let Val::Ref(p) = &v {
        let n = p.with(shadow);
        v = n;
    }
    v
}

// ---------------------------------------------------------------------------
// 評価器

struct M<'p> {
    p: &'p Program,
    strs: Vec<Rc<str>>,
    out: String,
    capture: bool,
    fs: Option<BTreeMap<String, String>>,
    canon: Vec<usize>,
    opt: usize,
    res: usize,
    io: usize,
}

fn list_of(items: Vec<Val>) -> Val {
    Val::Agg(Box::new(Agg {
        tag: Tag::List,
        f: items,
        silent: false,
    }))
}

impl<'p> M<'p> {
    fn variant(&self, t: usize, i: usize, f: Vec<Val>) -> Val {
        Val::Agg(Box::new(Agg {
            tag: Tag::Variant(t, i),
            f,
            silent: false,
        }))
    }

    fn some(&self, v: Val) -> Val {
        self.variant(self.opt, OPTION_SOME, vec![v])
    }

    fn none(&self) -> Val {
        self.variant(self.opt, OPTION_NONE, vec![])
    }

    fn ok(&self, v: Val) -> Val {
        self.variant(self.res, RESULT_OK, vec![v])
    }

    fn err(&self, v: Val) -> Val {
        self.variant(self.res, RESULT_ERR, vec![v])
    }

    fn io_error(&self, kind: std::io::ErrorKind, path: Rc<str>) -> Val {
        self.variant(self.io, spath::io_arm(kind), vec![Val::Path(path)])
    }

    fn stop_at(&self, id: StopId, kind: Option<StopKind>, detail: Option<&str>) -> E {
        let s = &self.p.stop_sites[id];
        Box::new(Stop {
            kind: kind.unwrap_or(s.kind),
            func: s.func.clone(),
            line: s.line,
            during: s.during.clone(),
            detail: detail.map_or_else(|| s.detail.clone(), str::to_string),
        })
    }

    fn after_deep(&self, loc: StopId) -> Result<(), E> {
        if DEEP.with(|d| d.replace(false)) {
            Err(self.stop_at(
                loc,
                Some(StopKind::StackExhausted),
                Some("値の入れ子が深すぎる"),
            ))
        } else {
            Ok(())
        }
    }

    fn konst(&self, c: &Const) -> Val {
        match c {
            Const::Unit => Val::Unit,
            Const::Int(n) => Val::Int(*n),
            Const::Float(x) => Val::Float(*x),
            Const::Bool(b) => Val::Bool(*b),
            Const::Str(i) => Val::Str(self.strs[*i].clone()),
            Const::Fn(f) => Val::Fn(*f),
        }
    }

    fn read(&self, locals: &[Slot], op: &Operand) -> Val {
        match op {
            Operand::Const(c) => self.konst(c),
            Operand::Copy(pl) => {
                let (s, p) = locate(locals, pl);
                with_val(&s, &p, shadow)
            }
            Operand::Move(pl) => {
                let (s, p) = locate(locals, pl);
                with_val_mut(&s, &p, |v| match v {
                    Val::Ref(_) | Val::Fn(_) => shadow(v),
                    _ => std::mem::replace(v, Val::Empty),
                })
                .unwrap_or(Val::Empty)
            }
        }
    }

    /// 参照をたどった値（スカラーの読み）。
    fn scalar(&self, locals: &[Slot], op: &Operand) -> Val {
        deref_val(self.read(locals, op))
    }

    /// 所有する引数（参照が来たら指す先の写し）。
    fn owned(&self, locals: &[Slot], op: &Operand) -> Val {
        match self.read(locals, op) {
            v @ Val::Ref(_) => deref_val(v),
            v => v,
        }
    }

    fn read_int(&self, locals: &[Slot], op: &Operand) -> i64 {
        match self.scalar(locals, op) {
            Val::Int(n) => n,
            _ => 0,
        }
    }

    fn read_bool(&self, locals: &[Slot], op: &Operand) -> bool {
        matches!(self.scalar(locals, op), Val::Bool(true))
    }

    /// 参照をすべてたどった先の場所。
    fn view(&self, locals: &[Slot], op: &Operand) -> (Slot, Vec<u32>) {
        match op {
            Operand::Const(c) => (new_slot(self.konst(c)), vec![]),
            Operand::Copy(pl) | Operand::Move(pl) => {
                let (mut s, mut p) = locate(locals, pl);
                follow_all(&mut s, &mut p);
                (s, p)
            }
        }
    }

    fn write(&self, locals: &[Slot], pl: &Place, v: Val) {
        let (s, p) = locate(locals, pl);
        // 古い値は借用を外してから破棄する。
        let old = with_val_mut(&s, &p, |slot| std::mem::replace(slot, v));
        drop(old);
    }

    fn exec(&mut self, locals: &[Slot], s: &Stmt) {
        match s {
            Stmt::Assign(pl, rv) => {
                let v = self.rvalue(locals, rv);
                self.write(locals, pl, v);
            }
            Stmt::Drop(pl) => {
                let (s, p) = locate(locals, pl);
                let old = with_val_mut(&s, &p, |v| match v {
                    Val::Ref(_) | Val::Fn(_) => Val::Empty,
                    _ => std::mem::replace(v, Val::Empty),
                });
                drop(old);
            }
        }
    }

    fn canon_tag(&self, k: &AggKind) -> Tag {
        match k {
            AggKind::Tuple => Tag::Tuple,
            AggKind::Struct(t) => Tag::Struct(self.canon[*t]),
            AggKind::Variant(t, i) => Tag::Variant(self.canon[*t], *i),
        }
    }

    fn rvalue(&mut self, locals: &[Slot], rv: &Rvalue) -> Val {
        match rv {
            Rvalue::Use(op) => self.read(locals, op),
            Rvalue::Unary(op, a) => match (op, self.scalar(locals, a)) {
                (UnOp::Not, Val::Bool(b)) => Val::Bool(!b),
                (UnOp::NegFloat, Val::Float(x)) => Val::Float(-x),
                _ => Val::Unit,
            },
            Rvalue::Binary(op, a, b) => {
                let x = self.scalar(locals, a);
                let y = self.scalar(locals, b);
                match (op, &x, &y) {
                    (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, ..) => {
                        Val::Bool(compare(*op, &x, &y))
                    }
                    (_, Val::Float(p), Val::Float(q)) => Val::Float(match op {
                        BinOp::Add => p + q,
                        BinOp::Sub => p - q,
                        BinOp::Mul => p * q,
                        BinOp::Div => p / q,
                        _ => p % q,
                    }),
                    (BinOp::Add, Val::Str(p), Val::Str(q)) => {
                        Val::Str(format!("{p}{q}").as_str().into())
                    }
                    _ => Val::Unit,
                }
            }
            Rvalue::Aggregate(k, ops) => {
                let f: Vec<Val> = ops.iter().map(|o| self.read(locals, o)).collect();
                Val::Agg(Box::new(Agg {
                    tag: self.canon_tag(k),
                    f,
                    silent: false,
                }))
            }
            Rvalue::Ref(pl) | Rvalue::RefMut(pl) => {
                let (s, p) = locate(locals, pl);
                if let Some(q) = ref_at(&s, &p) {
                    return Val::Ref(q);
                }
                Val::Ref(Ptr { slot: s, path: p })
            }
            Rvalue::Clone(op, _) => {
                let (s, p) = self.view(locals, op);
                with_val(&s, &p, deep_clone)
            }
            Rvalue::Discriminant(op) => {
                let (s, p) = self.view(locals, op);
                Val::Int(with_val(&s, &p, |v| match v {
                    Val::Agg(a) => match a.tag {
                        Tag::Variant(_, i) => i as i64,
                        _ => 0,
                    },
                    _ => 0,
                }))
            }
            Rvalue::MakeClosure(fid, ops) => {
                let f: Vec<Val> = ops.iter().map(|o| self.read(locals, o)).collect();
                let env = new_slot(Val::Agg(Box::new(Agg {
                    tag: Tag::Tuple,
                    f,
                    silent: false,
                })));
                Val::Closure(Rc::new(Clos { func: *fid, env }))
            }
        }
    }

    fn call_val(&mut self, f: &Val, args: Vec<Val>, loc: StopId) -> Result<Val, E> {
        match f {
            Val::Closure(c) => {
                let mut a = vec![Val::Ref(Ptr {
                    slot: c.env.clone(),
                    path: vec![],
                })];
                a.extend(args);
                self.run_func(c.func, a, loc)
            }
            Val::Fn(g) => self.run_func(*g, args, loc),
            Val::Ref(p) => {
                let t = p.with(shadow);
                self.call_val(&t, args, loc)
            }
            _ => Ok(Val::Unit),
        }
    }

    fn run_func(&mut self, fid: FuncId, args: Vec<Val>, loc: StopId) -> Result<Val, E> {
        let (base, budget) = STACK_LIMIT.with(|c| c.get());
        if base.abs_diff(stack_addr()) > budget {
            return Err(self.stop_at(loc, Some(StopKind::StackExhausted), Some("")));
        }
        let p = self.p;
        let f = &p.funcs[fid];
        let locals: Vec<Slot> = (0..f.locals.len()).map(|_| new_slot(Val::Empty)).collect();
        for (a, &pi) in args.into_iter().zip(&f.params) {
            *locals[pi].borrow_mut() = a;
        }
        let mut bb = 0usize;
        let mut pending: Option<E> = None;
        loop {
            let blk = &f.blocks[bb];
            for s in &blk.stmts {
                self.exec(&locals, s);
            }
            match &blk.term {
                Term::Goto(b) => bb = *b,
                Term::Branch(c, t, e) => bb = if self.read_bool(&locals, c) { *t } else { *e },
                Term::Switch(c, arms, d) => {
                    let n = self.read_int(&locals, c);
                    bb = arms.iter().find(|(k, _)| *k == n).map_or(*d, |(_, b)| *b);
                }
                Term::Call {
                    func,
                    args,
                    dest,
                    next,
                    cleanup,
                    stop,
                } => {
                    let r = match func {
                        Callee::Func(g) => {
                            let argv: Vec<Val> =
                                args.iter().map(|a| self.read(&locals, a)).collect();
                            self.run_func(*g, argv, *stop)
                        }
                        Callee::Indirect(op) => {
                            let (s, pth) = self.view(&locals, op);
                            let fv = with_val(&s, &pth, shadow);
                            let argv: Vec<Val> =
                                args.iter().map(|a| self.read(&locals, a)).collect();
                            self.call_val(&fv, argv, *stop)
                        }
                        Callee::Builtin(b) => self.builtin(&locals, *b, args, *stop),
                    };
                    match r {
                        Ok(v) => {
                            self.write(&locals, dest, v);
                            bb = *next;
                        }
                        Err(e) => {
                            pending = Some(e);
                            bb = *cleanup;
                        }
                    }
                }
                Term::CheckedOp {
                    op,
                    a,
                    b,
                    dest,
                    next,
                    stop,
                    cleanup,
                } => {
                    let x = self.read_int(&locals, a);
                    let y = self.read_int(&locals, b);
                    let r = match op {
                        ArithOp::Add => x.checked_add(y),
                        ArithOp::Sub => x.checked_sub(y),
                        ArithOp::Mul => x.checked_mul(y),
                        ArithOp::Div => x.checked_div(y),
                        ArithOp::Rem => x.checked_rem(y),
                    };
                    match r {
                        Some(n) => {
                            self.write(&locals, dest, Val::Int(n));
                            bb = *next;
                        }
                        None => {
                            pending = Some(self.stop_at(*stop, None, None));
                            bb = *cleanup;
                        }
                    }
                }
                Term::Assert {
                    cond,
                    stop,
                    next,
                    cleanup,
                } => {
                    if self.read_bool(&locals, cond) {
                        bb = *next;
                    } else {
                        pending = Some(self.stop_at(*stop, None, None));
                        bb = *cleanup;
                    }
                }
                Term::Return => {
                    let v = std::mem::replace(&mut *locals[0].borrow_mut(), Val::Empty);
                    return Ok(v);
                }
                Term::Unreachable => unreachable!("到達しない基本ブロックに来た"),
                Term::Resume => {
                    return Err(pending
                        .take()
                        .unwrap_or_else(|| self.stop_at(loc, None, None)));
                }
            }
        }
    }

    // --- 標準ライブラリ ---

    fn list_len(&self, s: &Slot, p: &[u32]) -> usize {
        with_val(s, p, |v| match v {
            Val::Agg(a) => a.f.len(),
            _ => 0,
        })
    }

    fn elem_ptr(&self, s: &Slot, p: &[u32], i: usize) -> Ptr {
        let mut q = p.to_vec();
        q.push(i as u32);
        Ptr {
            slot: s.clone(),
            path: q,
        }
    }

    fn builtin(
        &mut self,
        locals: &[Slot],
        b: Builtin,
        args: &[Operand],
        loc: StopId,
    ) -> Result<Val, E> {
        use Builtin::*;
        Ok(match b {
            ListNew => list_of(vec![]),
            ListLen => {
                let (s, p) = self.view(locals, &args[0]);
                Val::Int(self.list_len(&s, &p) as i64)
            }
            ListIsEmpty => {
                let (s, p) = self.view(locals, &args[0]);
                Val::Bool(self.list_len(&s, &p) == 0)
            }
            ListGet | ListGetMut => {
                let (s, p) = self.view(locals, &args[0]);
                let i = self.read_int(locals, &args[1]);
                let n = self.list_len(&s, &p);
                if i >= 0 && (i as usize) < n {
                    self.some(Val::Ref(self.elem_ptr(&s, &p, i as usize)))
                } else {
                    self.none()
                }
            }
            ListPush => {
                let (s, p) = self.view(locals, &args[0]);
                let x = self.read(locals, &args[1]);
                with_val_mut(&s, &p, |v| {
                    if let Val::Agg(a) = v {
                        a.f.push(x);
                    }
                });
                Val::Unit
            }
            ListSumInt => {
                let (s, p) = self.view(locals, &args[0]);
                let xs: Vec<i64> = with_val(&s, &p, |v| match v {
                    Val::Agg(a) => a.f.iter().map(int_of).collect(),
                    _ => vec![],
                });
                let mut it = xs.into_iter();
                let Some(mut acc) = it.next() else {
                    return Ok(Val::Int(0));
                };
                for x in it {
                    match acc.checked_add(x) {
                        Some(n) => acc = n,
                        None => return Err(self.stop_at(loc, Some(StopKind::Overflow), Some(""))),
                    }
                }
                Val::Int(acc)
            }
            ListSumFloat => {
                let (s, p) = self.view(locals, &args[0]);
                let xs: Vec<f64> = with_val(&s, &p, |v| match v {
                    Val::Agg(a) => {
                        a.f.iter()
                            .map(|x| match deref_val(shadow(x)) {
                                Val::Float(f) => f,
                                _ => 0.0,
                            })
                            .collect()
                    }
                    _ => vec![],
                });
                let mut it = xs.into_iter();
                let Some(mut acc) = it.next() else {
                    return Ok(Val::Float(0.0));
                };
                for x in it {
                    acc += x;
                }
                Val::Float(acc)
            }
            ListMin | ListMax => {
                let (s, p) = self.view(locals, &args[0]);
                let n = self.list_len(&s, &p);
                if n == 0 {
                    let name = if b == ListMin { "min" } else { "max" };
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some(&format!("List.{name} は self.len() > 0 を要求する")),
                    ));
                }
                let best = with_val(&s, &p, |v| {
                    let Val::Agg(a) = v else { return 0 };
                    let mut best = 0;
                    for (i, x) in a.f.iter().enumerate().skip(1) {
                        let o = cmp_val(x, &a.f[best]);
                        let better = if b == ListMin {
                            o == Some(Ordering::Less)
                        } else {
                            o == Some(Ordering::Greater)
                        };
                        if better {
                            best = i;
                        }
                    }
                    best
                });
                self.after_deep(loc)?;
                Val::Ref(self.elem_ptr(&s, &p, best))
            }
            ListMap => {
                let (s, p) = self.view(locals, &args[0]);
                let f = self.owned(locals, &args[1]);
                let n = self.list_len(&s, &p);
                let mut out = Vec::with_capacity(n);
                for i in 0..n {
                    let x = with_val(&s, &self.elem_ptr(&s, &p, i).path, shadow);
                    out.push(self.call_val(&f, vec![x], loc)?);
                }
                list_of(out)
            }
            ListFind => {
                let (s, p) = self.view(locals, &args[0]);
                let f = self.owned(locals, &args[1]);
                let n = self.list_len(&s, &p);
                for i in 0..n {
                    let r = Val::Ref(self.elem_ptr(&s, &p, i));
                    if let Val::Bool(true) = self.call_val(&f, vec![r], loc)? {
                        return Ok(self.some(Val::Ref(self.elem_ptr(&s, &p, i))));
                    }
                }
                self.none()
            }
            ListMaxBy | ListMinBy => {
                let (s, p) = self.view(locals, &args[0]);
                let f = self.owned(locals, &args[1]);
                let n = self.list_len(&s, &p);
                let mut best: Option<(Val, usize)> = None;
                for i in 0..n {
                    let r = Val::Ref(self.elem_ptr(&s, &p, i));
                    let k = self.call_val(&f, vec![r], loc)?;
                    let better = match &best {
                        None => true,
                        Some((bk, _)) => {
                            let o = cmp_val(&k, bk);
                            self.after_deep(loc)?;
                            if b == ListMaxBy {
                                o == Some(Ordering::Greater)
                            } else {
                                o == Some(Ordering::Less)
                            }
                        }
                    };
                    if better {
                        best = Some((k, i));
                    }
                }
                match best {
                    Some((_, i)) => self.some(Val::Ref(self.elem_ptr(&s, &p, i))),
                    None => self.none(),
                }
            }
            OptIsSome | OptIsNone | ResIsOk | ResIsErr => {
                let (s, p) = self.view(locals, &args[0]);
                let idx = with_val(&s, &p, |v| match v {
                    Val::Agg(a) => match a.tag {
                        Tag::Variant(_, i) => i,
                        _ => 9,
                    },
                    _ => 9,
                });
                Val::Bool(match b {
                    OptIsSome => idx == OPTION_SOME,
                    OptIsNone => idx == OPTION_NONE,
                    ResIsOk => idx == RESULT_OK,
                    _ => idx == RESULT_ERR,
                })
            }
            OptUnwrapOr | ResUnwrapOr => {
                let v = self.owned(locals, &args[0]);
                let d = self.owned(locals, &args[1]);
                match v {
                    Val::Agg(mut a) if matches!(a.tag, Tag::Variant(_, 0)) => {
                        a.f.pop().unwrap_or(Val::Unit)
                    }
                    _ => d,
                }
            }
            OptMap => {
                let v = self.owned(locals, &args[0]);
                let f = self.owned(locals, &args[1]);
                match v {
                    Val::Agg(mut a) if matches!(a.tag, Tag::Variant(_, OPTION_SOME)) => {
                        let x = a.f.pop().unwrap_or(Val::Unit);
                        let r = self.call_val(&f, vec![x], loc)?;
                        self.some(r)
                    }
                    _ => self.none(),
                }
            }
            OptOkOr => {
                let v = self.owned(locals, &args[0]);
                let e = self.owned(locals, &args[1]);
                match v {
                    Val::Agg(mut a) if matches!(a.tag, Tag::Variant(_, OPTION_SOME)) => {
                        let x = a.f.pop().unwrap_or(Val::Unit);
                        self.ok(x)
                    }
                    _ => self.err(e),
                }
            }
            ResOk => {
                let v = self.owned(locals, &args[0]);
                match v {
                    Val::Agg(mut a) if matches!(a.tag, Tag::Variant(_, RESULT_OK)) => {
                        let x = a.f.pop().unwrap_or(Val::Unit);
                        self.some(x)
                    }
                    _ => self.none(),
                }
            }
            ResMap => {
                let v = self.owned(locals, &args[0]);
                let f = self.owned(locals, &args[1]);
                match v {
                    Val::Agg(mut a) if matches!(a.tag, Tag::Variant(_, RESULT_OK)) => {
                        let x = a.f.pop().unwrap_or(Val::Unit);
                        let r = self.call_val(&f, vec![x], loc)?;
                        self.ok(r)
                    }
                    Val::Agg(mut a) => {
                        let e = a.f.pop().unwrap_or(Val::Unit);
                        self.err(e)
                    }
                    _ => Val::Unit,
                }
            }
            ResMapErr => {
                let v = self.owned(locals, &args[0]);
                let f = self.owned(locals, &args[1]);
                match v {
                    Val::Agg(mut a) if matches!(a.tag, Tag::Variant(_, RESULT_OK)) => {
                        let x = a.f.pop().unwrap_or(Val::Unit);
                        self.ok(x)
                    }
                    Val::Agg(mut a) => {
                        let e = a.f.pop().unwrap_or(Val::Unit);
                        let r = self.call_val(&f, vec![e], loc)?;
                        self.err(r)
                    }
                    _ => Val::Unit,
                }
            }
            StrLen | StrIsEmpty | StrTrim | StrLines | StrParseInt => {
                let (s, p) = self.view(locals, &args[0]);
                let text: Rc<str> = with_val(&s, &p, |v| match v {
                    Val::Str(x) => x.clone(),
                    _ => "".into(),
                });
                match b {
                    StrLen => Val::Int(stext::char_len(&text)),
                    StrIsEmpty => Val::Bool(text.is_empty()),
                    StrTrim => Val::Str(stext::trim(&text).into()),
                    StrLines => list_of(
                        stext::lines(&text)
                            .into_iter()
                            .map(|l| Val::Str(l.into()))
                            .collect(),
                    ),
                    _ => match stext::parse_int(&text) {
                        Some(n) => self.ok(Val::Int(n)),
                        None => self.err(Val::ParseError),
                    },
                }
            }
            PathNew => {
                let t = self.text_of(locals, &args[0]);
                Val::Path(t)
            }
            PathIsEmpty => {
                let t = self.text_of(locals, &args[0]);
                Val::Bool(t.is_empty())
            }
            PathJoin => {
                let t = self.text_of(locals, &args[0]);
                let o = self.text_of(locals, &args[1]);
                Val::Path(spath::join(&t, &o).into())
            }
            PathFileName => {
                let t = self.text_of(locals, &args[0]);
                match spath::file_name(&t) {
                    Some(n) => self.some(Val::Str(n.into())),
                    None => self.none(),
                }
            }
            PathParent => {
                let t = self.text_of(locals, &args[0]);
                match spath::parent(&t) {
                    Some(n) => self.some(Val::Path(n.into())),
                    None => self.none(),
                }
            }
            CellNew => Val::Cell(Rc::new(RefCell::new(self.owned(locals, &args[0])))),
            SharedNew => Val::Shared(Rc::new(self.owned(locals, &args[0]))),
            SharedGet => {
                let (s, p) = self.view(locals, &args[0]);
                with_val(&s, &p, |v| match v {
                    Val::Shared(rc) => match &**rc {
                        Val::Cell(c) => shadow(&c.borrow()),
                        other => shadow(other),
                    },
                    Val::Cell(c) => shadow(&c.borrow()),
                    _ => Val::Unit,
                })
            }
            SharedSet => {
                let (s, p) = self.view(locals, &args[0]);
                let new = self.owned(locals, &args[1]);
                let cell = with_val(&s, &p, |v| match v {
                    Val::Shared(rc) => match &**rc {
                        Val::Cell(c) => Some(c.clone()),
                        _ => None,
                    },
                    Val::Cell(c) => Some(c.clone()),
                    _ => None,
                });
                if let Some(c) = cell {
                    // 古い値は借用を外してから破棄する。
                    let old = std::mem::replace(&mut *c.borrow_mut(), new);
                    drop(old);
                }
                Val::Unit
            }
            IntMinValue => Val::Int(i64::MIN),
            ToFloat => Val::Float(self.read_int(locals, &args[0]) as f64),
            ToInt => {
                let x = match self.scalar(locals, &args[0]) {
                    Val::Float(x) => x,
                    _ => f64::NAN,
                };
                let lo = -9_223_372_036_854_775_808.0_f64;
                let hi = 9_223_372_036_854_775_808.0_f64;
                if x.is_finite() && x.trunc() >= lo && x.trunc() < hi {
                    self.some(Val::Int(x.trunc() as i64))
                } else {
                    self.none()
                }
            }
            ConsoleLog => {
                let (s, p) = self.view(locals, &args[0]);
                let ts = types();
                let text = with_val(&s, &p, |v| show_val(&ts, v, false));
                self.after_deep(loc)?;
                if self.capture {
                    self.out.push_str(&text);
                    self.out.push('\n');
                } else {
                    use std::io::Write;
                    let _ = writeln!(std::io::stdout().lock(), "{text}");
                }
                Val::Unit
            }
            FsRead => {
                let path = self.text_of(locals, &args[0]);
                match &self.fs {
                    Some(files) => match files.get(&*path) {
                        Some(text) => self.ok(Val::Str(text.as_str().into())),
                        None => self.err(self.io_error(std::io::ErrorKind::NotFound, path)),
                    },
                    None => match std::fs::read_to_string(&*path) {
                        Ok(text) => self.ok(Val::Str(text.into())),
                        Err(e) => self.err(self.io_error(e.kind(), path)),
                    },
                }
            }
            FsWrite => {
                let path = self.text_of(locals, &args[0]);
                let text = self.text_of(locals, &args[1]);
                match &mut self.fs {
                    Some(files) => {
                        files.insert(path.to_string(), text.to_string());
                        self.ok(Val::Unit)
                    }
                    None => match std::fs::write(&*path, text.as_bytes()) {
                        Ok(()) => self.ok(Val::Unit),
                        Err(e) => self.err(self.io_error(e.kind(), path)),
                    },
                }
            }
            Compare(op) => {
                let (s1, p1) = self.view(locals, &args[0]);
                let (s2, p2) = self.view(locals, &args[1]);
                let r = with_val(&s1, &p1, |x| with_val(&s2, &p2, |y| compare(op, x, y)));
                self.after_deep(loc)?;
                Val::Bool(r)
            }
        })
    }

    /// 文字列・パスの中身（参照はたどる）。
    fn text_of(&self, locals: &[Slot], op: &Operand) -> Rc<str> {
        let (s, p) = self.view(locals, op);
        with_val(&s, &p, |v| match v {
            Val::Str(x) | Val::Path(x) => x.clone(),
            _ => "".into(),
        })
    }
}

fn int_of(v: &Val) -> i64 {
    match v {
        Val::Int(n) => *n,
        Val::Ref(p) => p.with(int_of),
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// 入口

fn to_val(a: &Arg) -> Val {
    match a {
        Arg::Int(n) => Val::Int(*n),
        Arg::Float(x) => Val::Float(*x),
        Arg::Bool(b) => Val::Bool(*b),
        Arg::Str(s) => Val::Str(s.as_str().into()),
        Arg::List(xs) => list_of(xs.iter().map(to_val).collect()),
    }
}

/// 入口の引数を宣言の型に合わせて値にする（`runtime/core` の `coerce` と同じ規則）。
fn coerce(a: &Arg, ty: &Type) -> Result<Val, String> {
    let ty = match ty {
        Type::Ref { inner, .. } => inner,
        t => t,
    };
    let Type::Path { path, args } = ty else {
        return Ok(to_val(a));
    };
    let name = path.last().map(String::as_str).unwrap_or("");
    let mismatch = || Err(format!("{name} が要るが、{}が渡された", arg_name(a)));
    match (name, a) {
        ("Int", Arg::Int(n)) => Ok(Val::Int(*n)),
        ("Float", Arg::Float(x)) => Ok(Val::Float(*x)),
        ("Float", Arg::Int(n)) => Ok(Val::Float(*n as f64)),
        ("Bool", Arg::Bool(b)) => Ok(Val::Bool(*b)),
        ("String", Arg::Str(x)) => Ok(Val::Str(x.as_str().into())),
        ("Path", Arg::Str(x)) => Ok(Val::Path(x.as_str().into())),
        ("List", Arg::List(xs)) => {
            let elem = args.first();
            let mut items = Vec::with_capacity(xs.len());
            for x in xs {
                items.push(match elem {
                    Some(t) => coerce(x, t)?,
                    None => to_val(x),
                });
            }
            Ok(list_of(items))
        }
        ("Int" | "Float" | "Bool" | "String" | "Path" | "List", _) => mismatch(),
        _ => Ok(to_val(a)),
    }
}

/// ソースを検査して、入口の関数から SIR にする（ネイティブ出力の入口。`sigil build`）。
pub fn lower_sources(srcs: &[&str], func: &str) -> Result<Program, RunError> {
    lower_sources_with(srcs, func, &[])
}

/// `lower_sources` に `--trust-base` のモジュール名を渡す版。
pub fn lower_sources_with(
    srcs: &[&str],
    func: &str,
    trust_base: &[String],
) -> Result<Program, RunError> {
    let file = crate::exec::merge_sources(srcs).map_err(|f| f.error)?;
    let opts = crate::effects::Options {
        trust_base: trust_base.to_vec(),
        ..Default::default()
    };
    let typed = crate::check::typed_with(&file, &opts).map_err(RunError::Check)?;
    let found = file
        .items
        .iter()
        .any(|i| matches!(i, Item::Fn(d) if d.name == func));
    if !found {
        return Err(RunError::Entry(format!("関数 {func} がない")));
    }
    crate::lower::lower(&typed, func).map_err(|e| RunError::Unsupported(e.0))
}

pub fn run(src: &str, func: &str, args: &[Arg]) -> Result<Outcome, Failure> {
    run_sources_with(&[src], func, args, &Options::captured())
}

pub fn run_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    run_sources_with(&[src], func, args, opts)
}

pub fn run_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Result<Outcome, Failure> {
    run_sources_with(srcs, func, args, &Options::captured())
}

pub fn run_sources_with(
    srcs: &[&str],
    func: &str,
    args: &[Arg],
    opts: &Options,
) -> Result<Outcome, Failure> {
    std::thread::scope(|s| {
        let spawned = std::thread::Builder::new()
            .stack_size(opts.stack_bytes)
            .spawn_scoped(s, || execute(srcs, func, args, opts));
        match spawned {
            Ok(h) => h.join().unwrap_or_else(|_| {
                Err(crate::exec::fail(RunError::Entry(
                    "評価器の内部エラー".into(),
                )))
            }),
            Err(e) => Err(crate::exec::fail(RunError::Entry(format!(
                "スレッドを作れない: {e}"
            )))),
        }
    })
}

fn execute(srcs: &[&str], func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    use crate::exec::fail;
    let file: File = crate::exec::merge_sources(srcs)?;
    let typed = crate::check::typed_with(&file, &crate::exec::check_options(opts))
        .map_err(|e| fail(RunError::Check(e)))?;
    let decl = file.items.iter().find_map(|i| match i {
        Item::Fn(d) if d.name == func => Some(d),
        _ => None,
    });
    let Some(decl) = decl else {
        return Err(fail(RunError::Entry(format!("関数 {func} がない"))));
    };
    if decl.params.len() != args.len() {
        return Err(fail(RunError::Entry(format!(
            "{func} は引数を {} 個とる（{} 個渡された）",
            decl.params.len(),
            args.len()
        ))));
    }
    let mut argv = Vec::with_capacity(args.len());
    for (p, a) in decl.params.iter().zip(args) {
        match coerce(a, &p.ty) {
            Ok(v) => argv.push(if matches!(p.ty, Type::Ref { .. }) {
                Val::Ref(Ptr {
                    slot: new_slot(v),
                    path: vec![],
                })
            } else {
                v
            }),
            Err(m) => {
                return Err(fail(RunError::Entry(format!(
                    "引数 {} の型が合わない: {m}",
                    p.name
                ))))
            }
        }
    }
    let program = match crate::lower::lower(&typed, func) {
        Ok(p) => p,
        Err(e) => return Err(fail(RunError::Unsupported(e.0))),
    };
    drop(typed);

    let (base, budget) = (stack_addr(), opts.stack_bytes / 4 * 3);
    STACK_LIMIT.with(|c| c.set((base, budget)));
    DEEP.with(|d| d.set(false));
    DROP_LOG.with(|l| *l.borrow_mut() = opts.trace_drops.then(Vec::new));
    TYPES.with(|t| *t.borrow_mut() = Rc::new(program.types.clone()));

    let mut canon = Vec::with_capacity(program.types.len());
    for (i, t) in program.types.iter().enumerate() {
        canon.push(
            program
                .types
                .iter()
                .position(|u| u.name == t.name)
                .unwrap_or(i),
        );
    }
    let find = |n: &str| program.types.iter().position(|t| t.name == n).unwrap_or(0);
    let mut m = M {
        p: &program,
        strs: program.strings.iter().map(|s| s.as_str().into()).collect(),
        out: String::new(),
        capture: opts.capture_console,
        fs: opts.fs_files.clone(),
        opt: find("Option"),
        res: find("Result"),
        io: find("IoError"),
        canon,
    };
    let result = m.run_func(program.entry, argv, 0);
    let output = std::mem::take(&mut m.out);
    let res = match result {
        Ok(v) => {
            let ts = types();
            let value = show_val(&ts, &v, true);
            drop(v);
            if DEEP.with(|d| d.replace(false)) {
                Err((
                    RunError::Stop(Box::new(Stop {
                        kind: StopKind::StackExhausted,
                        func: func.to_string(),
                        line: program.exit_line,
                        during: None,
                        detail: "値の入れ子が深すぎる".into(),
                    })),
                    output,
                ))
            } else {
                Ok((value, output))
            }
        }
        Err(s) => Err((RunError::Stop(s), output)),
    };
    drop(m);
    let drops = DROP_LOG.with(|l| l.borrow_mut().take()).unwrap_or_default();
    match res {
        Ok((value, output)) => Ok(Outcome {
            value,
            output,
            drops,
        }),
        Err((error, output)) => Err(Failure {
            error,
            output,
            drops: drops.into_boxed_slice(),
        }),
    }
}

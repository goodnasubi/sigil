use crate::exec::StopKind;
use crate::ir::*;
use crate::shared_path as spath;
use crate::shared_text::{self as stext};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use super::value::{
    cmp_val, compare, deref_val, list_of, shadow, show_val, types, with_val, with_val_mut, Agg,
    Ptr, Tag, Val,
};
use super::{Chunk, Origin, Slot, E, M};

/// 実行時の `fs.read` が読むファイルの大きさの上限（バイト）。`lsp::MAX_DEP_BYTES` に揃えた暫定の定数。
/// ADR 0020 6-3 で `src/limits.rs` ができたら、そちらへ移す。
const MAX_FS_READ_BYTES: u64 = 64 * 1024 * 1024;

/// 実ファイルを読む `fs.read` の本体。`O_NONBLOCK` で開き（書き手のいない FIFO で `open` が止まらないように）、
/// 開いたファイルが通常ファイルか確かめ、上限を超えるものは読まずに（または 1 バイト余分に読んで）拒否する。
fn read_file_bounded(path: &str) -> std::io::Result<String> {
    use std::io::{Error, ErrorKind, Read};
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        #[cfg(all(
            any(target_os = "linux", target_os = "android"),
            any(target_arch = "mips", target_arch = "mips64")
        ))]
        const O_NONBLOCK: i32 = 0x80;
        #[cfg(all(
            any(target_os = "linux", target_os = "android"),
            any(target_arch = "sparc", target_arch = "sparc64")
        ))]
        const O_NONBLOCK: i32 = 0x4000;
        #[cfg(all(
            any(target_os = "linux", target_os = "android"),
            not(any(
                target_arch = "mips",
                target_arch = "mips64",
                target_arch = "sparc",
                target_arch = "sparc64"
            ))
        ))]
        const O_NONBLOCK: i32 = 0o4000;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        const O_NONBLOCK: i32 = 0x4;
        o.custom_flags(O_NONBLOCK);
    }
    let mut h = o.open(path)?;
    let meta = h.metadata()?;
    if !meta.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "通常のファイルではない（FIFO・デバイスなどは読まない）",
        ));
    }
    let too_big = || Error::new(ErrorKind::InvalidData, "ファイルが大きすぎる");
    if meta.len() > MAX_FS_READ_BYTES {
        return Err(too_big());
    }
    let mut bytes = vec![];
    (&mut h)
        .take(MAX_FS_READ_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FS_READ_BYTES {
        return Err(too_big());
    }
    String::from_utf8(bytes).map_err(|e| Error::new(ErrorKind::InvalidData, e))
}

impl<'p> M<'p> {
    pub(super) fn call_val(&mut self, f: &Val, args: Vec<Val>, loc: StopId) -> Result<Val, E> {
        match self.resolve_callee(f, args) {
            Some((g, a)) => self.run_func(g, a, loc),
            None => Ok(Val::Unit),
        }
    }

    fn variant(&self, t: usize, i: usize, f: Vec<Val>) -> Val {
        Val::Agg(Box::new(Agg {
            tag: Tag::Variant(t, i),
            f,
            silent: false,
        }))
    }

    pub(super) fn some(&self, v: Val) -> Val {
        self.variant(self.opt, OPTION_SOME, vec![v])
    }

    pub(super) fn none(&self) -> Val {
        self.variant(self.opt, OPTION_NONE, vec![])
    }

    pub(super) fn ok(&self, v: Val) -> Val {
        self.variant(self.res, RESULT_OK, vec![v])
    }

    pub(super) fn err(&self, v: Val) -> Val {
        self.variant(self.res, RESULT_ERR, vec![v])
    }

    fn io_error(&self, kind: std::io::ErrorKind, path: Rc<str>) -> Val {
        self.variant(self.io, spath::io_arm(kind), vec![Val::Path(path)])
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

    pub(super) fn builtin(
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
            CtEq => {
                let (s1, p1) = self.view(locals, &args[0]);
                let (s2, p2) = self.view(locals, &args[1]);
                let eq = with_val(&s1, &p1, |v1| {
                    with_val(&s2, &p2, |v2| match (v1, v2) {
                        (Val::Bytes(b1), Val::Bytes(b2)) => {
                            let (b1, b2) = (b1.borrow(), b2.borrow());
                            if b1.len() != b2.len() {
                                false
                            } else {
                                let mut diff = 0u8;
                                for (&x, &y) in b1.iter().zip(b2.iter()) {
                                    diff |= x ^ y;
                                }
                                diff == 0
                            }
                        }
                        _ => false,
                    })
                });
                Val::Bool(eq)
            }
            StrAsBytes => {
                let text = self.text_of(locals, &args[0]);
                Val::Bytes(Rc::new(RefCell::new(text.as_bytes().to_vec())))
            }
            IntRotl => {
                let val = self.read_int(locals, &args[0]);
                let n = self.read_int(locals, &args[1]);
                let width = self.read_int(locals, &args[2]);
                if width == 32 {
                    let u = (val as u32).rotate_left((n % 32) as u32);
                    Val::Int(u as i64)
                } else {
                    let u = (val as u64).rotate_left((n % 64) as u32);
                    Val::Int(u as i64)
                }
            }
            IntRotr => {
                let val = self.read_int(locals, &args[0]);
                let n = self.read_int(locals, &args[1]);
                let width = self.read_int(locals, &args[2]);
                if width == 32 {
                    let u = (val as u32).rotate_right((n % 32) as u32);
                    Val::Int(u as i64)
                } else {
                    let u = (val as u64).rotate_right((n % 64) as u32);
                    Val::Int(u as i64)
                }
            }
            IntRotr32 => {
                let val = self.read_int(locals, &args[0]);
                let n = self.read_int(locals, &args[1]);
                let u = (val as u32).rotate_right((n % 32) as u32);
                Val::Int(u as i64)
            }
            IntAshr => {
                let val = self.read_int(locals, &args[0]);
                let n = self.read_int(locals, &args[1]);
                Val::Int(val >> n)
            }
            FloatToBits => match self.scalar(locals, &args[0]) {
                Val::Float(x) => Val::Int(x.to_bits() as i64),
                // 型検査と lowering が `Float` を保証する。違う値が来るのは処理系の不具合なので止める。
                _ => unreachable!("Float.to_bits の引数が Float でない"),
            },
            FloatFromBits => match self.scalar(locals, &args[0]) {
                Val::Int(n) => Val::Float(f64::from_bits(n as u64)),
                _ => unreachable!("Float.from_bits の引数が Int でない"),
            },
            BytesNew => Val::Bytes(Rc::new(RefCell::new(Vec::new()))),
            BytesWithCapacity => {
                let cap = self.read_int(locals, &args[0]).max(0) as usize;
                Val::Bytes(Rc::new(RefCell::new(Vec::with_capacity(cap))))
            }
            BytesFromList => {
                let (s, p) = self.view(locals, &args[0]);
                let res = with_val(&s, &p, |v| match v {
                    Val::Agg(a) if a.tag == Tag::List => {
                        let mut bytes = Vec::with_capacity(a.f.len());
                        for item in &a.f {
                            let n = int_of(item);
                            if (0..=255).contains(&n) {
                                bytes.push(n as u8);
                            } else {
                                return Err("0..=255 の範囲外".to_string());
                            }
                        }
                        Ok(bytes)
                    }
                    _ => Err("リストでない".to_string()),
                });
                match res {
                    Ok(b) => self.ok(Val::Bytes(Rc::new(RefCell::new(b)))),
                    Err(e) => self.err(Val::Str(e.into())),
                }
            }
            BytesLen => {
                let (s, p) = self.view(locals, &args[0]);
                let len = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) => b.borrow().len() as i64,
                    _ => 0,
                });
                Val::Int(len)
            }
            BytesIsEmpty => {
                let (s, p) = self.view(locals, &args[0]);
                let is_empty = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) => b.borrow().is_empty(),
                    _ => true,
                });
                Val::Bool(is_empty)
            }
            BytesGet => {
                let (s, p) = self.view(locals, &args[0]);
                let idx = self.read_int(locals, &args[1]);
                let opt = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) if idx >= 0 => {
                        b.borrow().get(idx as usize).copied().map(|x| x as i64)
                    }
                    _ => None,
                });
                match opt {
                    Some(n) => self.some(Val::Int(n)),
                    None => self.none(),
                }
            }
            BytesSet => {
                let (s, p) = self.view(locals, &args[0]);
                let idx = self.read_int(locals, &args[1]);
                let val = self.read_int(locals, &args[2]);
                let res = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) if idx >= 0 && (0..=255).contains(&val) => {
                        let mut b = b.borrow_mut();
                        if let Some(slot) = b.get_mut(idx as usize) {
                            *slot = val as u8;
                            Ok(())
                        } else {
                            Err("インデックス範囲外".to_string())
                        }
                    }
                    _ => Err("不正な引数".to_string()),
                });
                match res {
                    Ok(()) => self.ok(Val::Unit),
                    Err(e) => self.err(Val::Str(e.into())),
                }
            }
            BytesPush => {
                let (s, p) = self.view(locals, &args[0]);
                let val = self.read_int(locals, &args[1]);
                with_val(&s, &p, |v| {
                    if let Val::Bytes(b) = v {
                        b.borrow_mut().push(val as u8);
                    }
                });
                Val::Unit
            }
            BytesSlice => {
                let (s, p) = self.view(locals, &args[0]);
                let start = self.read_int(locals, &args[1]);
                let end = self.read_int(locals, &args[2]);
                let res = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) if start >= 0 && start <= end => {
                        let b = b.borrow();
                        if end as usize <= b.len() {
                            Ok(b[start as usize..end as usize].to_vec())
                        } else {
                            Err("範囲外".to_string())
                        }
                    }
                    _ => Err("範囲不正".to_string()),
                });
                match res {
                    Ok(v) => self.ok(Val::Bytes(Rc::new(RefCell::new(v)))),
                    Err(e) => self.err(Val::Str(e.into())),
                }
            }
            BytesConcat => {
                let (s1, p1) = self.view(locals, &args[0]);
                let (s2, p2) = self.view(locals, &args[1]);
                let out = with_val(&s1, &p1, |v1| {
                    with_val(&s2, &p2, |v2| match (v1, v2) {
                        (Val::Bytes(b1), Val::Bytes(b2)) => {
                            let mut v = b1.borrow().clone();
                            v.extend_from_slice(&b2.borrow());
                            v
                        }
                        _ => Vec::new(),
                    })
                });
                Val::Bytes(Rc::new(RefCell::new(out)))
            }
            BytesExtend => {
                let (s1, p1) = self.view(locals, &args[0]);
                let (s2, p2) = self.view(locals, &args[1]);
                with_val(&s1, &p1, |v1| {
                    with_val(&s2, &p2, |v2| {
                        if let (Val::Bytes(b1), Val::Bytes(b2)) = (v1, v2) {
                            b1.borrow_mut().extend_from_slice(&b2.borrow());
                        }
                    })
                });
                Val::Unit
            }
            BytesToStringUtf8 => {
                let (s, p) = self.view(locals, &args[0]);
                let res = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) => match String::from_utf8(b.borrow().clone()) {
                        Ok(s) => Ok(s),
                        Err(e) => Err(e.to_string()),
                    },
                    _ => Err("Bytes でない".to_string()),
                });
                match res {
                    Ok(s) => self.ok(Val::Str(s.into())),
                    Err(e) => self.err(Val::Str(e.into())),
                }
            }
            BytesPushU16Be => {
                let (s, p) = self.view(locals, &args[0]);
                let val = self.read_int(locals, &args[1]) as u16;
                with_val(&s, &p, |v| {
                    if let Val::Bytes(b) = v {
                        b.borrow_mut().extend_from_slice(&val.to_be_bytes());
                    }
                });
                Val::Unit
            }
            BytesPushU32Be => {
                let (s, p) = self.view(locals, &args[0]);
                let val = self.read_int(locals, &args[1]) as u32;
                with_val(&s, &p, |v| {
                    if let Val::Bytes(b) = v {
                        b.borrow_mut().extend_from_slice(&val.to_be_bytes());
                    }
                });
                Val::Unit
            }
            BytesPushU64Be => {
                let (s, p) = self.view(locals, &args[0]);
                let val = self.read_int(locals, &args[1]) as u64;
                with_val(&s, &p, |v| {
                    if let Val::Bytes(b) = v {
                        b.borrow_mut().extend_from_slice(&val.to_be_bytes());
                    }
                });
                Val::Unit
            }
            BytesPushU32Le => {
                let (s, p) = self.view(locals, &args[0]);
                let val = self.read_int(locals, &args[1]) as u32;
                with_val(&s, &p, |v| {
                    if let Val::Bytes(b) = v {
                        b.borrow_mut().extend_from_slice(&val.to_le_bytes());
                    }
                });
                Val::Unit
            }
            BytesReadU32Be => {
                let (s, p) = self.view(locals, &args[0]);
                let off = self.read_int(locals, &args[1]);
                let opt = with_val(&s, &p, |v| match v {
                    Val::Bytes(b) if off >= 0 => {
                        let b = b.borrow();
                        let o = off as usize;
                        if o + 4 <= b.len() {
                            let bytes: [u8; 4] = b[o..o + 4].try_into().unwrap();
                            Some(u32::from_be_bytes(bytes) as i64)
                        } else {
                            None
                        }
                    }
                    _ => None,
                });
                match opt {
                    Some(n) => self.some(Val::Int(n)),
                    None => self.none(),
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
            EnvGet => {
                let name = self.text_of(locals, &args[0]);
                // ネイティブの getenv と同じ名前の制限。差し替え表でも外部環境へフォールバックしない。
                let value = if name.is_empty() || name.contains(['\0', '=']) {
                    None
                } else {
                    match &self.env {
                        Some(vars) => vars.get(&*name).cloned(),
                        None => std::env::var_os(&*name).map(|s| s.to_string_lossy().into_owned()),
                    }
                };
                match value {
                    Some(s) => self.some(Val::Str(s.into())),
                    None => self.none(),
                }
            }
            SyncSharedBorrow => {
                let (s, p) = self.view(locals, &args[0]);
                let slot = with_val(&s, &p, |v| super::sched::shared_cell(v).cloned());
                match slot {
                    Some(slot) => Val::Ref(Ptr { slot, path: vec![] }),
                    None => {
                        return Err(self.stop_at(
                            loc,
                            Some(StopKind::StdRequiresViolated),
                            Some("SyncShared でない値への borrow"),
                        ))
                    }
                }
            }
            FsRead => {
                let path = self.text_of(locals, &args[0]);
                match &self.fs {
                    Some(files) => match files.get(&*path) {
                        Some(text) => self.ok(Val::Str(text.as_str().into())),
                        None => self.err(self.io_error(std::io::ErrorKind::NotFound, path)),
                    },
                    None => match read_file_bounded(&path) {
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
            RawAddrNull => Val::Addr(0),
            RawAddrToInt => {
                let v = self.scalar(locals, &args[0]);
                match v {
                    Val::Addr(a) => Val::Int(a as i64),
                    _ => Val::Int(0),
                }
            }
            RawIntToAddr => {
                let v = self.read_int(locals, &args[0]);
                if v < 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.int_to_addr: v >= 0"),
                    ));
                }
                Val::Addr(v as usize)
            }
            RawOffset => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let n = self.read_int(locals, &args[1]);
                let new_a = if n >= 0 {
                    a.checked_add(n as usize).unwrap_or(a)
                } else {
                    a.checked_sub((-n) as usize).unwrap_or(a)
                };
                Val::Addr(new_a)
            }
            RawLoadU8 => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                Val::Int(i64::from(self.peek_u8(a, loc)?))
            }
            RawStoreU8 => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let v = self.read_int(locals, &args[1]);
                if !(0..=255).contains(&v) {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.store_u8: 0 <= v <= 255"),
                    ));
                }
                self.check_raw_access(a, 1, loc)?;
                unsafe { *(a as *mut u8) = v as u8 };
                Val::Unit
            }
            RawLoadI64 => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                if a % 8 != 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.load_i64: 8バイト境界違反"),
                    ));
                }
                self.check_raw_access(a, 8, loc)?;
                let w = unsafe { *(a as *const i64) };
                Val::Int(w)
            }
            RawStoreI64 => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let v = self.read_int(locals, &args[1]);
                if a % 8 != 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.store_i64: 8バイト境界違反"),
                    ));
                }
                self.check_raw_access(a, 8, loc)?;
                unsafe { *(a as *mut i64) = v };
                Val::Unit
            }
            RawLoadAddr => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                if a % 8 != 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.load_addr: 8バイト境界違反"),
                    ));
                }
                self.check_raw_access(a, 8, loc)?;
                let w = unsafe { *(a as *const usize) };
                Val::Addr(w)
            }
            RawStoreAddr => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let v = match self.scalar(locals, &args[1]) {
                    Val::Addr(v) => v,
                    _ => 0,
                };
                if a % 8 != 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.store_addr: 8バイト境界違反"),
                    ));
                }
                self.check_raw_access(a, 8, loc)?;
                unsafe { *(a as *mut usize) = v };
                Val::Unit
            }
            RawCopy => {
                let dst = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let src = match self.scalar(locals, &args[1]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let n = self.read_int(locals, &args[2]);
                if n < 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.copy: n >= 0"),
                    ));
                }
                let nu = n as usize;
                if nu > 0 {
                    self.check_raw_access(dst, nu, loc)?;
                    self.check_raw_access(src, nu, loc)?;
                    unsafe {
                        std::ptr::copy(src as *const u8, dst as *mut u8, nu);
                    }
                }
                Val::Unit
            }
            RawSet => {
                let dst = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let byte = self.read_int(locals, &args[1]);
                let n = self.read_int(locals, &args[2]);
                if n < 0 || !(0..=255).contains(&byte) {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.set: n >= 0 && 0 <= byte <= 255"),
                    ));
                }
                let nu = n as usize;
                if nu > 0 {
                    self.check_raw_access(dst, nu, loc)?;
                    unsafe {
                        std::ptr::write_bytes(dst as *mut u8, byte as u8, nu);
                    }
                }
                Val::Unit
            }
            RawAlloc => {
                let size = self.read_int(locals, &args[0]);
                let align = self.read_int(locals, &args[1]);
                if size <= 0 || align <= 0 || !(align as usize).is_power_of_two() {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.alloc: size > 0 && align は 2の累乗"),
                    ));
                }
                // 上限は Sigil 製の確保器と同じ（ADR 0025 補足 A-4: align <= 4096、size <= 2^46）。
                if align > 4096 || size > (1i64 << 46) {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.alloc: align <= 4096 && size <= 2^46"),
                    ));
                }
                let su = size as usize;
                let au = align as usize;
                let layout = std::alloc::Layout::from_size_align(su, au).map_err(|_| {
                    self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.alloc: 不正なレイアウト"),
                    )
                })?;
                let ptr = unsafe { std::alloc::alloc(layout) };
                if ptr.is_null() {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::Overflow),
                        Some("raw.alloc: メモリ枯渇"),
                    ));
                }
                let addr = ptr as usize;
                self.chunks.insert(
                    addr,
                    Chunk {
                        size: su,
                        align: au,
                        origin: Origin::Alloc,
                    },
                );
                Val::Addr(addr)
            }
            RawFree => {
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let size = self.read_int(locals, &args[1]);
                let align = self.read_int(locals, &args[2]);
                if a == 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.free: ヌル番地解放"),
                    ));
                }
                let Some(&chunk) = self.chunks.get(&a) else {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.free: 二重解放または未確保の番地"),
                    ));
                };
                if chunk.origin != Origin::Alloc {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.free: raw.alloc 由来でない塊（mmap 由来）は解放できない"),
                    ));
                }
                let (stored_sz, stored_al) = (chunk.size, chunk.align);
                if stored_sz != size as usize || stored_al != align as usize {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some("raw.free: 確保時と異なるサイズまたはアライメント"),
                    ));
                }
                self.chunks.remove(&a);
                let layout =
                    unsafe { std::alloc::Layout::from_size_align_unchecked(stored_sz, stored_al) };
                unsafe { std::alloc::dealloc(a as *mut u8, layout) };
                Val::Unit
            }
            RawSyscall => {
                let n = self.read_int(locals, &args[0]);
                let a0 = self.read_int(locals, &args[1]);
                let a1 = self.read_int(locals, &args[2]);
                let a2 = self.read_int(locals, &args[3]);
                let a3 = self.read_int(locals, &args[4]);
                let a4 = self.read_int(locals, &args[5]);
                let a5 = self.read_int(locals, &args[6]);
                self.check_syscall(n, [a0, a1, a2, a3, a4, a5], loc)?;
                let ret = issue_syscall(n, [a0, a1, a2, a3, a4, a5]);
                if n == SYS_MMAP && !(-4095..0).contains(&ret) {
                    let len = (a1 as usize).div_ceil(PAGE) * PAGE;
                    self.chunks.insert(
                        ret as usize,
                        Chunk {
                            size: len,
                            align: PAGE,
                            origin: Origin::Mmap,
                        },
                    );
                } else if n == SYS_MUNMAP && ret == 0 {
                    self.chunks.remove(&(a0 as usize));
                }
                Val::Int(ret)
            }
            // 原子的な操作（ADR 0030 決定 5）。評価器はタスクを 1 つのスレッドで動かすので、通常の読み書きと
            // 同じ意味で足りるが、ネイティブとの食い違いを避けるため `AtomicI64`（逐次一貫）で行う。
            // 検査は `load_i64`・`store_i64` と同じ（8 バイト境界・生きた領域）で、状態を書き換える前に行う。
            RawAtomicLoadI64 | RawAtomicStoreI64 | RawAtomicAddI64 | RawAtomicSwapI64
            | RawAtomicCasI64 => {
                use std::sync::atomic::{AtomicI64, Ordering::SeqCst};
                let name = match b {
                    RawAtomicLoadI64 => "raw.atomic_load_i64",
                    RawAtomicStoreI64 => "raw.atomic_store_i64",
                    RawAtomicAddI64 => "raw.atomic_add_i64",
                    RawAtomicSwapI64 => "raw.atomic_swap_i64",
                    _ => "raw.atomic_cas_i64",
                };
                let a = match self.scalar(locals, &args[0]) {
                    Val::Addr(a) => a,
                    _ => 0,
                };
                let x = if matches!(b, RawAtomicLoadI64) {
                    0
                } else {
                    self.read_int(locals, &args[1])
                };
                let y = if matches!(b, RawAtomicCasI64) {
                    self.read_int(locals, &args[2])
                } else {
                    0
                };
                if a % 8 != 0 {
                    return Err(self.stop_at(
                        loc,
                        Some(StopKind::StdRequiresViolated),
                        Some(&format!("{name}: 8バイト境界違反")),
                    ));
                }
                self.check_raw_access(a, 8, loc)?;
                // SAFETY: 8 バイト境界で、生きた領域の中であることを検査した。
                let cell = unsafe { AtomicI64::from_ptr(a as *mut i64) };
                match b {
                    RawAtomicLoadI64 => Val::Int(cell.load(SeqCst)),
                    RawAtomicStoreI64 => {
                        cell.store(x, SeqCst);
                        Val::Unit
                    }
                    RawAtomicAddI64 => Val::Int(cell.fetch_add(x, SeqCst)),
                    RawAtomicSwapI64 => Val::Int(cell.swap(x, SeqCst)),
                    _ => Val::Int(match cell.compare_exchange(x, y, SeqCst, SeqCst) {
                        Ok(v) | Err(v) => v,
                    }),
                }
            }
            RawAtomicFence => {
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                Val::Unit
            }
            RootsScope | RootReserve | RootStart | IoWaitFd | TaskScope | TaskAllOk | TaskRace
            | TaskTimeout | TaskYield | TaskSleep | TaskChannel | ScopeSpawn | TaskJoin
            | ChanSend | ChanRecv | SyncSharedNew | SyncSharedGet | LockNew | LockAcquire
            | LockGuardGet | LockGuardGetMut | AtomicNew | AtomicLoad | AtomicStore => {
                unreachable!("タスク関連の組み込み操作はスケジューラ側で特別に処理される")
            }
        })
    }

    /// 番地 `a` の 1 バイトを、生きた確保領域の中であることを確かめてから読む（`raw.load_u8` と、許可表の引数検証が使う）。
    fn peek_u8(&self, a: usize, loc: StopId) -> Result<u8, E> {
        self.check_raw_access(a, 1, loc)?;
        Ok(unsafe { *(a as *const u8) })
    }

    /// 生きた確保領域内へのアクセスであることを検証する。
    fn check_raw_access(&self, addr: usize, len: usize, loc: StopId) -> Result<(), E> {
        if addr == 0 {
            return Err(self.stop_at(
                loc,
                Some(StopKind::StdRequiresViolated),
                Some("ヌル番地へのアクセス"),
            ));
        }
        let end = addr.checked_add(len).ok_or_else(|| {
            self.stop_at(loc, Some(StopKind::Overflow), Some("番地オーバーフロー"))
        })?;
        if let Some((&start, c)) = self.chunks.range(..=addr).next_back() {
            if end <= start.saturating_add(c.size) {
                return Ok(());
            }
        }
        Err(self.stop_at(
            loc,
            Some(StopKind::StdRequiresViolated),
            Some("未確保または範囲外の番地アクセス"),
        ))
    }

    fn syscall_stop(&self, loc: StopId, msg: &str) -> E {
        self.stop_at(loc, Some(StopKind::StdRequiresViolated), Some(msg))
    }

    /// `raw.syscall` の許可表と引数検証（ADR 0025 補足 A-3）。状態の書き換えより前に行う。
    fn check_syscall(&self, n: i64, a: [i64; 6], loc: StopId) -> Result<(), E> {
        let addr_of = |v: i64, what: &str| -> Result<usize, E> {
            usize::try_from(v)
                .map_err(|_| self.syscall_stop(loc, &format!("raw.syscall: {what} が負の番地")))
        };
        match n {
            SYS_READ | SYS_WRITE => {
                let name = if n == SYS_READ { "read" } else { "write" };
                if a[2] < 0 {
                    return Err(self.syscall_stop(loc, &format!("raw.syscall {name}: count >= 0")));
                }
                if a[2] > 0 {
                    let buf = addr_of(a[1], "buf")?;
                    self.check_raw_access(buf, a[2] as usize, loc)?;
                }
            }
            SYS_CLOSE => {}
            // ソケット系（ADR 0031 決定 3-4・判断点 D の案 B、PR 6-3e）。`std.os.net` が使う形だけを通す。
            // 番地を渡す引数は、生きた塊の範囲に収まることを確かめる。カーネルが書く長さは、検査した大きさを超えない。
            SYS_SOCKET => {
                if a[0] != AF_INET
                    || a[1] != SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC
                    || a[2] != 0
                {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall socket: AF_INET・SOCK_STREAM|SOCK_NONBLOCK|SOCK_CLOEXEC・protocol 0 のみ許可",
                    ));
                }
            }
            SYS_SETSOCKOPT => {
                if a[1] != SOL_SOCKET || a[2] != SO_REUSEADDR || a[4] != 4 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall setsockopt: SOL_SOCKET・SO_REUSEADDR・長さ 4 のみ許可",
                    ));
                }
                self.check_raw_access(addr_of(a[3], "optval")?, 4, loc)?;
            }
            SYS_BIND => {
                if a[2] != SOCKADDR_IN_SIZE as i64 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall bind: addrlen は 16（sockaddr_in）のみ許可",
                    ));
                }
                let sa = addr_of(a[1], "addr")?;
                self.check_raw_access(sa, SOCKADDR_IN_SIZE, loc)?;
                // 家族（先頭の 2 バイト。機械の並び）。
                let family =
                    u16::from_le_bytes([self.peek_u8(sa, loc)?, self.peek_u8(sa + 1, loc)?]);
                if i64::from(family) != AF_INET {
                    return Err(self.syscall_stop(loc, "raw.syscall bind: AF_INET のみ許可"));
                }
            }
            SYS_LISTEN => {}
            SYS_GETSOCKNAME => {
                self.check_raw_access(addr_of(a[1], "addr")?, SOCKADDR_IN_SIZE, loc)?;
                let lp = addr_of(a[2], "addrlen")?;
                self.check_raw_access(lp, 4, loc)?;
                let mut lb = [0u8; 4];
                for (i, b) in lb.iter_mut().enumerate() {
                    *b = self.peek_u8(lp + i, loc)?;
                }
                if u32::from_le_bytes(lb) as usize != SOCKADDR_IN_SIZE {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall getsockname: *addrlen は 16（検査した領域の大きさ）のみ許可",
                    ));
                }
            }
            SYS_ACCEPT4 => {
                if a[1] != 0 || a[2] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall accept4: 相手のアドレスは受け取らない（addr・addrlen は 0 のみ許可）",
                    ));
                }
                if a[3] != SOCK_NONBLOCK | SOCK_CLOEXEC {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall accept4: flags は SOCK_NONBLOCK|SOCK_CLOEXEC のみ許可",
                    ));
                }
            }
            SYS_RECVFROM | SYS_SENDTO => {
                let name = if n == SYS_RECVFROM {
                    "recvfrom"
                } else {
                    "sendto"
                };
                let flags = if n == SYS_RECVFROM { 0 } else { MSG_NOSIGNAL };
                if a[2] < 0 {
                    return Err(self.syscall_stop(loc, &format!("raw.syscall {name}: len >= 0")));
                }
                if a[3] != flags || a[4] != 0 || a[5] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        &format!(
                            "raw.syscall {name}: flags は {flags} のみ、相手のアドレスは渡さない（0 のみ許可）"
                        ),
                    ));
                }
                if a[2] > 0 {
                    let buf = addr_of(a[1], "buf")?;
                    self.check_raw_access(buf, a[2] as usize, loc)?;
                }
            }
            SYS_SHUTDOWN => {
                if a[1] != SHUT_WR {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall shutdown: how は SHUT_WR のみ許可")
                    );
                }
            }
            SYS_EXIT_GROUP => {
                return Err(self.syscall_stop(
                    loc,
                    &format!("raw.syscall: exit_group({}) の模擬停止", a[0]),
                ));
            }
            SYS_OPENAT | SYS_UNLINKAT => {
                self.check_path(addr_of(a[1], "path")?, loc)?;
            }
            SYS_NEWFSTATAT => {
                self.check_path(addr_of(a[1], "path")?, loc)?;
                self.check_raw_access(addr_of(a[2], "statbuf")?, STAT_SIZE, loc)?;
            }
            SYS_MMAP => {
                if a[0] != 0 {
                    return Err(self.syscall_stop(loc, "raw.syscall mmap: addr は 0 のみ許可"));
                }
                if a[1] <= 0 || a[1] > (1i64 << 46) {
                    return Err(self.syscall_stop(loc, "raw.syscall mmap: 0 < len <= 2^46"));
                }
                if a[2] != PROT_READ | PROT_WRITE {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall mmap: prot は PROT_READ|PROT_WRITE のみ許可（PROT_EXEC 禁止）",
                    ));
                }
                if a[3] != MAP_PRIVATE | MAP_ANONYMOUS {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall mmap: flags は MAP_PRIVATE|MAP_ANONYMOUS のみ許可（MAP_FIXED 等は拒否）",
                    ));
                }
                if a[4] != -1 || a[5] != 0 {
                    return Err(self.syscall_stop(loc, "raw.syscall mmap: fd = -1 かつ offset = 0"));
                }
            }
            SYS_MUNMAP => {
                let addr = addr_of(a[0], "addr")?;
                let ok = match self.chunks.get(&addr) {
                    Some(c) if c.origin == Origin::Mmap => {
                        a[1] > 0 && (a[1] as usize).div_ceil(PAGE).checked_mul(PAGE) == Some(c.size)
                    }
                    Some(_) => {
                        return Err(self.syscall_stop(
                            loc,
                            "raw.syscall munmap: mmap 由来でない塊（raw.alloc 由来）は解放できない",
                        ));
                    }
                    None => false,
                };
                if !ok {
                    return Err(self
                        .syscall_stop(loc, "raw.syscall munmap: mmap で得た範囲と完全一致しない"));
                }
            }
            _ => {
                return Err(self.syscall_stop(
                    loc,
                    &format!("raw.syscall: 許可表にないシステムコール番号 {n}"),
                ));
            }
        }
        Ok(())
    }

    /// NUL 終端のパスで、終端までが 1 つの生きた塊の範囲内であること。
    fn check_path(&self, addr: usize, loc: StopId) -> Result<(), E> {
        self.check_raw_access(addr, 1, loc)?;
        let (&start, c) = self.chunks.range(..=addr).next_back().expect("検証済み");
        let limit = start.saturating_add(c.size);
        let mut p = addr;
        while p < limit {
            if unsafe { *(p as *const u8) } == 0 {
                return Ok(());
            }
            p += 1;
        }
        Err(self.syscall_stop(
            loc,
            "raw.syscall: パスが NUL 終端されていない（範囲内に終端がない）",
        ))
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

/// システムコールを発行する（`raw.syscall` の実体。引数の検証は呼び出し側が済ませる）。Linux x86-64 以外では -1。
fn issue_syscall(n: i64, a: [i64; 6]) -> i64 {
    let ret: i64;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    unsafe {
        std::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a[0],
            in("rsi") a[1],
            in("rdx") a[2],
            in("r10") a[3],
            in("r8") a[4],
            in("r9") a[5],
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (n, a);
        ret = -1;
    }
    ret
}

/// fd を閉じる（`close(2)`。失敗は捨てる）。fd を所有する値（`TcpListener`・`TcpStream`）の破棄からだけ呼ぶ
/// （`value::close_owned_fd`。ADR 0031 決定 3-1）。`close` は許可表にある番号で、番地を取らない。
pub(super) fn close_fd(fd: i64) {
    let _ = issue_syscall(SYS_CLOSE, [fd, 0, 0, 0, 0, 0]);
}

const PAGE: usize = 4096;
const STAT_SIZE: usize = 144;
const SYS_READ: i64 = 0;
const SYS_WRITE: i64 = 1;
const SYS_CLOSE: i64 = 3;
const SYS_MMAP: i64 = 9;
const SYS_MUNMAP: i64 = 11;
const SYS_EXIT_GROUP: i64 = 231;
const SYS_OPENAT: i64 = 257;
const SYS_NEWFSTATAT: i64 = 262;
const SYS_UNLINKAT: i64 = 263;
const SYS_SOCKET: i64 = 41;
const SYS_SENDTO: i64 = 44;
const SYS_RECVFROM: i64 = 45;
const SYS_SHUTDOWN: i64 = 48;
const SYS_BIND: i64 = 49;
const SYS_LISTEN: i64 = 50;
const SYS_GETSOCKNAME: i64 = 51;
const SYS_SETSOCKOPT: i64 = 54;
const SYS_ACCEPT4: i64 = 288;
const SOCKADDR_IN_SIZE: usize = 16;
const AF_INET: i64 = 2;
const SOCK_STREAM: i64 = 1;
const SOCK_NONBLOCK: i64 = 0o4000;
const SOCK_CLOEXEC: i64 = 0o2000000;
const SOL_SOCKET: i64 = 1;
const SO_REUSEADDR: i64 = 2;
const MSG_NOSIGNAL: i64 = 0x4000;
const SHUT_WR: i64 = 1;
const PROT_READ: i64 = 1;
const PROT_WRITE: i64 = 2;
const MAP_PRIVATE: i64 = 0x2;
const MAP_ANONYMOUS: i64 = 0x20;

fn int_of(v: &Val) -> i64 {
    match v {
        Val::Int(n) => *n,
        Val::Ref(p) => p.with(int_of),
        _ => 0,
    }
}

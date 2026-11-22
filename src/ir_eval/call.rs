use crate::exec::StopKind;
use crate::ir::*;
use crate::shared_path as spath;
use crate::shared_text::{self as stext};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeMap;
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

    fn binary_error(&self, error: super::binary_io::Error, max: i64, path: Rc<str>) -> Val {
        use super::binary_io::Error;
        let (arm, fields) = match error {
            Error::InvalidLimit => (0, vec![Val::Int(max)]),
            Error::InvalidPath => (1, vec![]),
            Error::TooLarge => (2, vec![Val::Int(max)]),
            Error::UnsupportedFile => (3, vec![]),
            Error::Io(e) => (4, vec![self.io_error(e.kind(), path)]),
        };
        self.variant(self.binary_io, arm, fields)
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
            StrLen | StrByteLen | StrIsEmpty | StrTrim | StrLines | StrParseInt => {
                let (s, p) = self.view(locals, &args[0]);
                let text: Rc<str> = with_val(&s, &p, |v| match v {
                    Val::Str(x) => x.clone(),
                    _ => "".into(),
                });
                match b {
                    StrLen => Val::Int(stext::char_len(&text)),
                    StrByteLen => Val::Int(text.len() as i64),
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
            FsReadBytes => {
                use super::binary_io as bio;
                let path = self.text_of(locals, &args[0]);
                let max = self.read_int(locals, &args[1]);
                let shadowed = self.fs_shadow.path_of(&path);
                let result = bio::validate(&path, max).and_then(|cap| match &self.fs_bytes {
                    _ if shadowed.is_some() => {
                        bio::read(&shadowed.as_ref().unwrap().to_string_lossy(), cap)
                    }
                    Some(files) => {
                        let p_clean = path.trim_start_matches('/');
                        if self.fs_bytes_deleted.contains(&*path)
                            || self.fs_bytes_deleted.contains(p_clean)
                            || self.fs_bytes_deleted.contains(&format!("/{}", p_clean))
                        {
                            Err(bio::Error::Io(std::io::ErrorKind::NotFound.into()))
                        } else {
                            match self
                                .fs_bytes_written
                                .get(&*path)
                                .or_else(|| self.fs_bytes_written.get(p_clean))
                                .or_else(|| self.fs_bytes_written.get(&format!("/{}", p_clean)))
                                .or_else(|| files.get(&*path))
                                .or_else(|| files.get(p_clean))
                                .or_else(|| files.get(&format!("/{}", p_clean)))
                            {
                                Some(bytes) if bytes.len() <= cap => Ok(bytes.clone()),
                                Some(_) => Err(bio::Error::TooLarge),
                                None => Err(bio::Error::Io(std::io::ErrorKind::NotFound.into())),
                            }
                        }
                    }
                    None => bio::read(&path, cap),
                });
                match result {
                    Ok(bytes) => self.ok(Val::Bytes(Rc::new(RefCell::new(bytes)))),
                    Err(e) => self.err(self.binary_error(e, max, path)),
                }
            }
            FsWriteBytes => {
                use super::binary_io as bio;
                let path = self.text_of(locals, &args[0]);
                let max = self.read_int(locals, &args[2]);
                let (slot, field) = self.view(locals, &args[1]);
                let shadowed = self.fs_shadow.path_of(&path);
                let result = bio::validate(&path, max).and_then(|cap| {
                    with_val(&slot, &field, |v| {
                        let Val::Bytes(bytes) = v else {
                            unreachable!("checked Bytes argument")
                        };
                        let bytes = bytes.borrow();
                        if bytes.len() > cap {
                            return Err(bio::Error::TooLarge);
                        }
                        if let Some(sp) = &shadowed {
                            return bio::write(&sp.to_string_lossy(), &bytes);
                        }
                        match &self.fs_bytes {
                            Some(_) => {
                                let p_clean = path.trim_start_matches('/');
                                self.fs_bytes_deleted.remove(&*path);
                                self.fs_bytes_deleted.remove(p_clean);
                                self.fs_bytes_deleted.remove(&format!("/{}", p_clean));
                                self.record_fs_parent_dirs(&path);
                                self.fs_bytes_written
                                    .insert(path.to_string(), bytes.clone());
                                Ok(())
                            }
                            None => bio::write(&path, &bytes),
                        }
                    })
                });
                match result {
                    Ok(()) => self.ok(Val::Unit),
                    Err(e) => self.err(self.binary_error(e, max, path)),
                }
            }
            FsRead => {
                let path = self.text_of(locals, &args[0]);
                if let Some(sp) = self.fs_shadow.path_of(&path) {
                    return match read_file_bounded(&sp.to_string_lossy()) {
                        Ok(text) => Ok(self.ok(Val::Str(text.into()))),
                        Err(e) => Ok(self.err(self.io_error(e.kind(), path))),
                    };
                }
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
                if let Some(sp) = self.fs_shadow.path_of(&path) {
                    return match std::fs::write(&sp, text.as_bytes()) {
                        Ok(()) => Ok(self.ok(Val::Unit)),
                        Err(e) => Ok(self.err(self.io_error(e.kind(), path))),
                    };
                }
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
                let ret = if let (SYS_CLOCK_GETTIME, Some(injected)) = (n, self.wall_clock) {
                    match injected {
                        Ok(sec) => {
                            let tp = a1 as usize as *mut i64;
                            unsafe {
                                tp.write(sec);
                                tp.add(1).write(0);
                            }
                            0
                        }
                        Err(err) => {
                            let code = err.abs();
                            if code == 0 {
                                -1
                            } else {
                                -code
                            }
                        }
                    }
                } else if (self.fs.is_some() || self.fs_bytes.is_some())
                    && matches!(
                        n,
                        SYS_OPENAT
                            | SYS_MKDIRAT
                            | SYS_NEWFSTATAT
                            | SYS_UNLINKAT
                            | SYS_READLINKAT
                            | SYS_GETDENTS64
                            | SYS_FLOCK
                            | SYS_FSYNC
                            | SYS_FDATASYNC
                            | SYS_RENAMEAT
                            | SYS_FSTAT
                            | SYS_FCHMOD
                    )
                {
                    self.eval_fs_double_syscall(n, [a0, a1, a2, a3, a4, a5])
                } else {
                    issue_syscall(n, [a0, a1, a2, a3, a4, a5])
                };
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
            | LockGuardGet | LockGuardGetMut | CapacityNew | CapacityTryAcquire | AtomicNew
            | AtomicLoad | AtomicStore => {
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
            SYS_READ | SYS_WRITE | SYS_PREAD64 | SYS_PWRITE64 => {
                let name = match n {
                    SYS_READ => "read",
                    SYS_WRITE => "write",
                    SYS_PREAD64 => "pread64",
                    _ => "pwrite64",
                };
                if matches!(n, SYS_PREAD64 | SYS_PWRITE64) && a[3] < 0 {
                    return Err(self.syscall_stop(loc, &format!("raw.syscall {name}: offset >= 0")));
                }
                if a[2] < 0 {
                    return Err(self.syscall_stop(loc, &format!("raw.syscall {name}: count >= 0")));
                }
                if a[2] > 0 {
                    let buf = addr_of(a[1], "buf")?;
                    self.check_raw_access(buf, a[2] as usize, loc)?;
                }
            }
            SYS_CLOSE => {}
            // ADR 0033 K: 非同期ハンドラは持たず SIGTERM / SIGINT のみ。
            SYS_RT_SIGPROCMASK | SYS_SIGNALFD4 => {
                if a[3] != 8 && n == SYS_RT_SIGPROCMASK {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall rt_sigprocmask: sigsetsize は 8 のみ許可",
                    ));
                }
                if n == SYS_RT_SIGPROCMASK {
                    if !matches!(a[0], 0 | 1) || a[4] != 0 || a[5] != 0 {
                        return Err(self.syscall_stop(loc, "raw.syscall rt_sigprocmask: SIG_BLOCK / SIG_UNBLOCK のみ許可（未使用引数は 0）"));
                    }
                    if a[2] != 0 {
                        self.check_raw_access(addr_of(a[2], "oldset")?, 8, loc)?;
                    }
                } else if a[0] != -1 || a[2] != 8 || a[3] != 526336 || a[4] != 0 || a[5] != 0 {
                    return Err(self.syscall_stop(loc, "raw.syscall signalfd4: 新規 fd・sigsetsize 8・NONBLOCK|CLOEXEC のみ許可（未使用引数は 0）"));
                }
                let set = addr_of(a[1], "set")?;
                self.check_raw_access(set, 8, loc)?;
                let mut bytes = [0u8; 8];
                for (i, byte) in bytes.iter_mut().enumerate() {
                    *byte = self.peek_u8(set + i, loc)?;
                }
                let mask = u64::from_le_bytes(bytes);
                if (n == SYS_SIGNALFD4 || a[0] == 0) && mask != SHUTDOWN_MASK
                    || mask & !SHUTDOWN_MASK != 0
                {
                    return Err(self.syscall_stop(loc, "raw.syscall signal: SIGTERM / SIGINT のみ許可（block と signalfd は両方を指定）"));
                }
            }
            // ソケット系（ADR 0031 決定 3-4・判断点 D の案 B、PR 6-3e）。`std.os.net` が使う形だけを通す。
            // 番地を渡す引数は、生きた塊の範囲に収まることを確かめる。カーネルが書く長さは、検査した大きさを超えない。
            SYS_SOCKET => {
                // TCP（AF_INET/AF_INET6 の SOCK_STREAM）と、UDP（AF_INET の SOCK_DGRAM。ADR 0057 決定 4）の組だけを通す。
                let tcp = (a[0] == AF_INET || a[0] == AF_INET6)
                    && a[1] == SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC;
                let udp = a[0] == AF_INET && a[1] == SOCK_DGRAM | SOCK_NONBLOCK | SOCK_CLOEXEC;
                if !(tcp || udp) || a[2] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall socket: AF_INET/AF_INET6・SOCK_STREAM|SOCK_NONBLOCK|SOCK_CLOEXEC、または AF_INET・SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC（protocol 0）のみ許可",
                    ));
                }
            }
            SYS_SETSOCKOPT => {
                let is_reuseaddr = a[1] == SOL_SOCKET && a[2] == SO_REUSEADDR && a[4] == 4;
                let is_v6only = a[1] == IPPROTO_IPV6 && a[2] == IPV6_V6ONLY && a[4] == 4;
                if !is_reuseaddr && !is_v6only {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall setsockopt: SOL_SOCKET/SO_REUSEADDR または IPPROTO_IPV6/IPV6_V6ONLY（長さ 4）のみ許可",
                    ));
                }
                self.check_raw_access(addr_of(a[3], "optval")?, 4, loc)?;
            }
            SYS_BIND | SYS_CONNECT => {
                let sa = addr_of(a[1], "addr")?;
                if a[2] == SOCKADDR_IN_SIZE as i64 {
                    self.check_raw_access(sa, SOCKADDR_IN_SIZE, loc)?;
                    // 家族（先頭の 2 バイト。機械の並び）。
                    let family =
                        u16::from_le_bytes([self.peek_u8(sa, loc)?, self.peek_u8(sa + 1, loc)?]);
                    if i64::from(family) != AF_INET {
                        return Err(
                            self.syscall_stop(loc, "raw.syscall bind/connect: AF_INET のみ許可")
                        );
                    }
                } else if a[2] == SOCKADDR_IN6_SIZE as i64 {
                    self.check_raw_access(sa, SOCKADDR_IN6_SIZE, loc)?;
                    // 家族（先頭の 2 バイト。機械の並び）。
                    let family =
                        u16::from_le_bytes([self.peek_u8(sa, loc)?, self.peek_u8(sa + 1, loc)?]);
                    if i64::from(family) != AF_INET6 {
                        return Err(
                            self.syscall_stop(loc, "raw.syscall bind/connect: AF_INET6 のみ許可")
                        );
                    }
                } else {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall bind/connect: addrlen は 16（sockaddr_in）または 28（sockaddr_in6）のみ許可",
                    ));
                }
            }
            SYS_LISTEN => {}
            SYS_GETSOCKNAME => {
                let lp = addr_of(a[2], "addrlen")?;
                self.check_raw_access(lp, 4, loc)?;
                let mut lb = [0u8; 4];
                for (i, b) in lb.iter_mut().enumerate() {
                    *b = self.peek_u8(lp + i, loc)?;
                }
                let len = u32::from_le_bytes(lb) as usize;
                if len != SOCKADDR_IN_SIZE && len != SOCKADDR_IN6_SIZE {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall getsockname: *addrlen は 16 または 28（検査した領域の大きさ）のみ許可",
                    ));
                }
                self.check_raw_access(addr_of(a[1], "addr")?, len, loc)?;
            }
            SYS_GETSOCKOPT => {
                if a[1] != SOL_SOCKET || a[2] != SO_ERROR {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall getsockopt: SOL_SOCKET・SO_ERROR のみ許可",
                    ));
                }
                let opt = addr_of(a[3], "optval")?;
                let len = addr_of(a[4], "optlen")?;
                self.check_raw_access(opt, 4, loc)?;
                self.check_raw_access(len, 4, loc)?;
                let mut bytes = [0u8; 4];
                for (i, byte) in bytes.iter_mut().enumerate() {
                    *byte = self.peek_u8(len + i, loc)?;
                }
                if u32::from_le_bytes(bytes) != 4 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall getsockopt: *optlen は 4 のみ許可")
                    );
                }
            }
            SYS_GETRANDOM => {
                if !(1..=65536).contains(&a[1]) || a[2] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall getrandom: 長さは 1..65536・flags は 0 のみ許可",
                    ));
                }
                self.check_raw_access(addr_of(a[0], "buf")?, a[1] as usize, loc)?;
            }
            SYS_CLOCK_GETTIME => {
                if a[0] != CLOCK_REALTIME || a[2] != 0 || a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall clock_gettime: CLOCK_REALTIME (0) のみ許可（未使用引数は 0）",
                    ));
                }
                self.check_raw_access(addr_of(a[1], "tp")?, 16, loc)?;
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
            // TCP の形（相手のアドレスを渡さない: addr・addrlen が 0）と、UDP の形（ADR 0057 決定 4: 相手のアドレスを渡す）を
            // 区別する。包括的には許さず、UDP の形は 16 バイトの sockaddr_in だけを通す。
            SYS_RECVFROM | SYS_SENDTO => {
                let name = if n == SYS_RECVFROM {
                    "recvfrom"
                } else {
                    "sendto"
                };
                if a[2] < 0 {
                    return Err(self.syscall_stop(loc, &format!("raw.syscall {name}: len >= 0")));
                }
                if a[4] == 0 {
                    // TCP の形。
                    let flags = if n == SYS_RECVFROM { 0 } else { MSG_NOSIGNAL };
                    if a[3] != flags || a[5] != 0 {
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
                } else if n == SYS_SENDTO {
                    // UDP の送信: MSG_NOSIGNAL、0〜65507 バイト、長さ 16 の AF_INET 宛先。
                    if a[3] != MSG_NOSIGNAL || a[5] != SOCKADDR_IN_SIZE as i64 {
                        return Err(self.syscall_stop(
                            loc,
                            "raw.syscall sendto: UDP は flags が MSG_NOSIGNAL、宛先の長さが 16 のみ許可",
                        ));
                    }
                    if a[2] > UDP_MAX_PAYLOAD {
                        return Err(self.syscall_stop(
                            loc,
                            "raw.syscall sendto: UDP のペイロードは 65507 バイト以下",
                        ));
                    }
                    if a[2] > 0 {
                        let buf = addr_of(a[1], "buf")?;
                        self.check_raw_access(buf, a[2] as usize, loc)?;
                    }
                    let dest = addr_of(a[4], "dest_addr")?;
                    self.check_raw_access(dest, SOCKADDR_IN_SIZE, loc)?;
                    let family = u16::from_le_bytes([
                        self.peek_u8(dest, loc)?,
                        self.peek_u8(dest + 1, loc)?,
                    ]);
                    if i64::from(family) != AF_INET {
                        return Err(
                            self.syscall_stop(loc, "raw.syscall sendto: 宛先は AF_INET のみ許可")
                        );
                    }
                } else {
                    // UDP の受信: MSG_TRUNC、1〜65507 バイトの受信バッファ、16 バイトの送信元領域、
                    // 16 に初期化した 4 バイトの長さ領域。三領域の生存・境界・非重複を syscall の前に確かめる。
                    if a[3] != MSG_TRUNC {
                        return Err(self.syscall_stop(
                            loc,
                            "raw.syscall recvfrom: UDP は flags が MSG_TRUNC のみ許可",
                        ));
                    }
                    if !(1..=UDP_MAX_PAYLOAD).contains(&a[2]) {
                        return Err(self.syscall_stop(
                            loc,
                            "raw.syscall recvfrom: UDP の受信バッファは 1 以上 65507 以下",
                        ));
                    }
                    let buf = addr_of(a[1], "buf")?;
                    let src = addr_of(a[4], "src_addr")?;
                    let alen = addr_of(a[5], "addrlen")?;
                    let (buf_len, src_len, alen_len) = (a[2] as usize, SOCKADDR_IN_SIZE, 4usize);
                    self.check_raw_access(buf, buf_len, loc)?;
                    self.check_raw_access(src, src_len, loc)?;
                    self.check_raw_access(alen, alen_len, loc)?;
                    let regions = [(buf, buf_len), (src, src_len), (alen, alen_len)];
                    for (i, &(s1, l1)) in regions.iter().enumerate() {
                        for &(s2, l2) in &regions[i + 1..] {
                            if s1 < s2 + l2 && s2 < s1 + l1 {
                                return Err(self.syscall_stop(
                                    loc,
                                    "raw.syscall recvfrom: 受信バッファ・送信元領域・長さ領域は重ならない",
                                ));
                            }
                        }
                    }
                    let mut lb = [0u8; 4];
                    for (i, b) in lb.iter_mut().enumerate() {
                        *b = self.peek_u8(alen + i, loc)?;
                    }
                    if u32::from_le_bytes(lb) as usize != SOCKADDR_IN_SIZE {
                        return Err(self.syscall_stop(
                            loc,
                            "raw.syscall recvfrom: *addrlen は 16 に初期化したものだけ許可",
                        ));
                    }
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
            SYS_READLINKAT => {
                if a[4] != 0 || a[5] != 0 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall readlinkat: 未使用の引数は 0 のみ許可")
                    );
                }
                self.check_path(addr_of(a[1], "path")?, loc)?;
                if a[3] <= 0 || a[3] > 65536 {
                    return Err(self
                        .syscall_stop(loc, "raw.syscall readlinkat: bufsiz は 1 以上 65536 以下"));
                }
                let buf = addr_of(a[2], "buf")?;
                self.check_raw_access(buf, a[3] as usize, loc)?;
            }
            // 同期（ADR 0049 決定 4.2）。fd だけを取り、番地を取らない。未使用の引数は 0 のみ許可。
            SYS_FSYNC | SYS_FDATASYNC => {
                if a[1] != 0 || a[2] != 0 || a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall fsync/fdatasync: 未使用の引数は 0 のみ許可",
                    ));
                }
            }
            // 原子的な名前変更（ADR 0049 決定 4.3）。olddirfd・newdirfd とも AT_FDCWD のみ許可し、両方のパスを検査する。
            SYS_RENAMEAT => {
                if a[0] != -100 || a[2] != -100 || a[4] != 0 || a[5] != 0 {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall renameat: olddirfd・newdirfd は AT_FDCWD のみ許可（未使用の引数は 0）",
                    ));
                }
                self.check_path(addr_of(a[1], "oldpath")?, loc)?;
                self.check_path(addr_of(a[3], "newpath")?, loc)?;
            }
            SYS_MKDIRAT => {
                if a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall mkdirat: 未使用の引数は 0 のみ許可")
                    );
                }
                self.check_path(addr_of(a[1], "path")?, loc)?;
            }
            SYS_GETDENTS64 => {
                if a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall getdents64: 未使用の引数は 0 のみ許可")
                    );
                }
                if a[2] < 0 || a[2] > 65536 {
                    return Err(self
                        .syscall_stop(loc, "raw.syscall getdents64: count は 0 以上 65536 以下"));
                }
                if a[2] > 0 {
                    let dirp = addr_of(a[1], "dirp")?;
                    self.check_raw_access(dirp, a[2] as usize, loc)?;
                }
            }
            SYS_FLOCK => {
                if a[2] != 0 || a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall flock: 未使用の引数は 0 のみ許可")
                    );
                }
                let allowed_ops = LOCK_SH | LOCK_EX | LOCK_NB | LOCK_UN;
                if a[1] & !allowed_ops != 0 || (a[1] & (LOCK_SH | LOCK_EX | LOCK_UN) == 0) {
                    return Err(self.syscall_stop(
                        loc,
                        "raw.syscall flock: operation は LOCK_SH/LOCK_EX/LOCK_NB/LOCK_UN の組み合わせのみ許可",
                    ));
                }
            }
            SYS_FSTAT => {
                if a[2] != 0 || a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall fstat: 未使用の引数は 0 のみ許可")
                    );
                }
                self.check_raw_access(addr_of(a[1], "statbuf")?, STAT_SIZE, loc)?;
            }
            SYS_FCHMOD => {
                if a[2] != 0 || a[3] != 0 || a[4] != 0 || a[5] != 0 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall fchmod: 未使用の引数は 0 のみ許可")
                    );
                }
                if a[1] < 0 || a[1] > 0o777 {
                    return Err(
                        self.syscall_stop(loc, "raw.syscall fchmod: mode は 0 以上 0o777 以下")
                    );
                }
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

    /// NUL 終端アドレスから String を読む。
    fn path_string(&self, addr: usize) -> String {
        let mut p = addr;
        let mut bytes = Vec::new();
        while unsafe { *(p as *const u8) } != 0 {
            bytes.push(unsafe { *(p as *const u8) });
            p += 1;
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn is_mock_file(&self, p: &str) -> bool {
        if let Some(sp) = self.fs_shadow.path_of(p) {
            return sp.is_file();
        }
        let p_clean = p.trim_start_matches('/');
        let in_fs = self.fs.as_ref().is_some_and(|files| {
            files.contains_key(p)
                || files.contains_key(p_clean)
                || files.contains_key(&format!("/{}", p_clean))
        });
        let in_written = self.fs_bytes_written.contains_key(p)
            || self.fs_bytes_written.contains_key(p_clean)
            || self.fs_bytes_written.contains_key(&format!("/{}", p_clean));
        let in_bytes = self.fs_bytes.as_ref().is_some_and(|files| {
            files.contains_key(p)
                || files.contains_key(p_clean)
                || files.contains_key(&format!("/{}", p_clean))
        });
        let is_deleted = self.fs_bytes_deleted.contains(p)
            || self.fs_bytes_deleted.contains(p_clean)
            || self.fs_bytes_deleted.contains(&format!("/{}", p_clean));
        in_fs || (in_written && !is_deleted) || (in_bytes && !is_deleted)
    }

    pub(super) fn record_fs_parent_dirs(&mut self, path: &str) {
        let clean = path.trim_matches('/');
        let mut cur = clean;
        while let Some(parent) = std::path::Path::new(cur).parent() {
            let p_str = parent.to_str().unwrap_or("");
            if p_str.is_empty() {
                break;
            }
            self.fs_dirs.insert(p_str.to_string());
            self.fs_dirs.insert(format!("/{}", p_str));
            cur = p_str;
        }
    }

    fn is_mock_dir(&self, p: &str) -> bool {
        let p_clean = p.trim_matches('/');
        if p_clean.is_empty() {
            return true;
        }
        if self.fs_dirs.contains(p_clean) || self.fs_dirs.contains(&format!("/{}", p_clean)) {
            return true;
        }
        let prefix = format!("{}/", p_clean);
        let in_fs = self.fs.as_ref().is_some_and(|files| {
            files
                .keys()
                .any(|k| k.trim_matches('/').starts_with(&prefix))
        });
        let in_bytes = self.fs_bytes.as_ref().is_some_and(|files| {
            files.keys().any(|k| {
                let k_clean = k.trim_matches('/');
                k_clean.starts_with(&prefix)
                    && !self.fs_bytes_deleted.contains(k)
                    && !self.fs_bytes_deleted.contains(k_clean)
            })
        });
        let in_written = self.fs_bytes_written.keys().any(|k| {
            let k_clean = k.trim_matches('/');
            k_clean.starts_with(&prefix)
                && !self.fs_bytes_deleted.contains(k)
                && !self.fs_bytes_deleted.contains(k_clean)
        });
        let in_dirs = self.fs_dirs.iter().any(|d| {
            let d_clean = d.trim_matches('/');
            d_clean != p_clean && d_clean.starts_with(&prefix)
        });
        in_fs || in_bytes || in_written || in_dirs
    }

    /// テスト差し替え（fs_files / fs_bytes）下の疑似システムコール処理（ADR 0049 決定 5）。
    /// 未対応操作は fail-closed（-95: EOPNOTSUPP）で拒否する。
    fn eval_fs_double_syscall(&mut self, n: i64, a: [i64; 6]) -> i64 {
        match n {
            // ADR 0049 決定 5: 差し替えの表は永続媒体を持たないので、同期を成功と偽らない。
            // 影の実ファイル／実ディレクトリ（ハンドルで開いたもの。決定 5.1）の fd にだけ、本物の fsync／fdatasync を発行する。
            // それ以外の fd は fail-closed（EOPNOTSUPP → IoError.Other）。
            SYS_FSYNC | SYS_FDATASYNC => {
                if self.is_shadow_fd(a[0]) {
                    issue_syscall(n, [a[0], 0, 0, 0, 0, 0])
                } else {
                    -95
                }
            }
            // ADR 0049 決定 7: 影の実ファイルの fd にだけ本物の fstat／fchmod を発行する。
            // それ以外の fd は fail-closed（EOPNOTSUPP）。
            SYS_FSTAT | SYS_FCHMOD => {
                if self.is_shadow_fd(a[0]) {
                    issue_syscall(n, a)
                } else {
                    -95
                }
            }
            // 差し替えのパスの rename は、表と実ファイルシステムのどちらの原子性も保証できないので明示的に拒否する
            // （実ファイルシステムのパスを仮のパスとして誤って rename しないために、本物の renameat も発行しない）。
            SYS_RENAMEAT => -95, // EOPNOTSUPP（IoError.Other）
            SYS_UNLINKAT => {
                let p = match self.double_path(a[0], a[1] as usize) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let p_clean = p.trim_matches('/');
                if a[2] == 512 {
                    if self.is_mock_file(&p) {
                        -20 // ENOTDIR: ファイルを rmdir しようとした
                    } else if !self.is_mock_dir(&p) {
                        -2 // ENOENT: ディレクトリが存在しない
                    } else {
                        let prefix = format!("{}/", p_clean);
                        // 影の実ファイル（表に無い、ハンドルで作ったものを含む）も子として数える。
                        let mut has_children = self.fs_shadow.has_file_under(&prefix);
                        if let Some(files) = &self.fs {
                            has_children |= files
                                .keys()
                                .any(|k| k.trim_matches('/').starts_with(&prefix));
                        }
                        if let Some(files) = &self.fs_bytes {
                            has_children |= files.keys().any(|k| {
                                let k_clean = k.trim_matches('/');
                                k_clean.starts_with(&prefix)
                                    && !self.fs_bytes_deleted.contains(k)
                                    && !self.fs_bytes_deleted.contains(k_clean)
                            });
                        }
                        has_children |= self.fs_bytes_written.keys().any(|k| {
                            let k_clean = k.trim_matches('/');
                            k_clean.starts_with(&prefix)
                                && !self.fs_bytes_deleted.contains(k)
                                && !self.fs_bytes_deleted.contains(k_clean)
                        });
                        has_children |= self.fs_dirs.iter().any(|d| {
                            let d_clean = d.trim_matches('/');
                            d_clean != p_clean && d_clean.starts_with(&prefix)
                        });
                        if has_children {
                            -39 // ENOTEMPTY
                        } else {
                            // ハンドルのために影へ置いた実ディレクトリも、実ディレクトリとして消す。
                            // 子（ファイルでもディレクトリでも）が残っていれば ENOTEMPTY、消えていれば何もしない。
                            if let Some(root) = self.fs_shadow.root() {
                                let comps = super::shadow::split(&p).unwrap_or_default();
                                if !comps.is_empty() {
                                    match std::fs::remove_dir(root.join(comps.join("/"))) {
                                        Ok(()) => {}
                                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                                        Err(e) => {
                                            return -(e.raw_os_error().unwrap_or(5) as i64);
                                        }
                                    }
                                }
                            }
                            self.fs_dirs.remove(p_clean);
                            self.fs_dirs.remove(&format!("/{}", p_clean));
                            0
                        }
                    }
                } else if let Some(sp) = self.fs_shadow.path_of(&p) {
                    // 影の実ファイル（ハンドルで開いたもの）は、実ファイルとして消す。影の登録は残し、以後は実ファイルが ENOENT を返す。
                    // 表（fs／fs_bytes）の同名の登録も消して、ディレクトリの空判定（rmdir）が影の有無と食い違わないようにする。
                    match std::fs::remove_file(&sp) {
                        Ok(()) => {
                            if let Some(files) = &mut self.fs {
                                files.remove(&p);
                                files.remove(p_clean);
                                files.remove(&format!("/{}", p_clean));
                            }
                            self.fs_bytes_written.remove(&p);
                            self.fs_bytes_written.remove(p_clean);
                            self.fs_bytes_written.remove(&format!("/{}", p_clean));
                            self.fs_bytes_deleted.insert(p.clone());
                            self.fs_bytes_deleted.insert(p_clean.to_string());
                            self.fs_bytes_deleted.insert(format!("/{}", p_clean));
                            0
                        }
                        Err(e) => -(e.raw_os_error().unwrap_or(5) as i64),
                    }
                } else {
                    let mut removed = false;
                    if let Some(files) = &mut self.fs {
                        removed |= files.remove(&p).is_some()
                            || files.remove(p_clean).is_some()
                            || files.remove(&format!("/{}", p_clean)).is_some();
                    }
                    let had_written = self.fs_bytes_written.remove(&p).is_some()
                        || self.fs_bytes_written.remove(p_clean).is_some()
                        || self
                            .fs_bytes_written
                            .remove(&format!("/{}", p_clean))
                            .is_some();
                    let in_initial_bytes = self.fs_bytes.as_ref().is_some_and(|files| {
                        files.contains_key(&p)
                            || files.contains_key(p_clean)
                            || files.contains_key(&format!("/{}", p_clean))
                    });
                    let already_deleted = self.fs_bytes_deleted.contains(&p)
                        || self.fs_bytes_deleted.contains(p_clean)
                        || self.fs_bytes_deleted.contains(&format!("/{}", p_clean));
                    let had_bytes = (had_written || in_initial_bytes) && !already_deleted;
                    if had_bytes {
                        self.fs_bytes_deleted.insert(p.clone());
                        self.fs_bytes_deleted.insert(p_clean.to_string());
                        self.fs_bytes_deleted.insert(format!("/{}", p_clean));
                    }
                    removed |= had_bytes;
                    if removed {
                        0
                    } else if self.fs.is_some() || self.fs_bytes.is_some() {
                        -2 // ENOENT: 差し替え下でファイルが存在しない
                    } else {
                        // どちらも設定されていない場合は fail-closed
                        -95 // EOPNOTSUPP
                    }
                }
            }
            SYS_MKDIRAT => {
                let p = match self.double_path(a[0], a[1] as usize) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                let p_clean = p.trim_matches('/');
                if self.is_mock_file(&p) || self.is_mock_dir(&p) {
                    -17 // EEXIST: 既にファイルまたはディレクトリが存在する
                } else {
                    if let Some(parent) = std::path::Path::new(p_clean).parent() {
                        let parent_str = parent.to_str().unwrap_or("");
                        if !parent_str.is_empty() && !self.is_mock_dir(parent_str) {
                            return -2; // ENOENT: 親ディレクトリが存在しない
                        }
                    }
                    self.fs_dirs.insert(p_clean.to_string());
                    self.fs_dirs.insert(format!("/{}", p_clean));
                    0
                }
            }
            SYS_NEWFSTATAT => {
                let p = match self.double_path(a[0], a[1] as usize) {
                    Ok(p) => p,
                    Err(e) => return e,
                };
                if let Some(sp) = self.fs_shadow.path_of(&p) {
                    return self.shadow_syscall_path(SYS_NEWFSTATAT, &sp, [a[2], a[3]]);
                }
                let p_clean = p.trim_start_matches('/');
                let statbuf = a[2] as usize as *mut u8;
                let mut found_len = None;
                if let Some(files) = &self.fs {
                    if let Some(s) = files
                        .get(&p)
                        .or_else(|| files.get(p_clean))
                        .or_else(|| files.get(&format!("/{}", p_clean)))
                    {
                        found_len = Some(s.len() as i64);
                    }
                }
                let is_deleted = self.fs_bytes_deleted.contains(&p)
                    || self.fs_bytes_deleted.contains(p_clean)
                    || self.fs_bytes_deleted.contains(&format!("/{}", p_clean));
                if found_len.is_none() && !is_deleted {
                    if let Some(b) = self
                        .fs_bytes_written
                        .get(&p)
                        .or_else(|| self.fs_bytes_written.get(p_clean))
                        .or_else(|| self.fs_bytes_written.get(&format!("/{}", p_clean)))
                    {
                        found_len = Some(b.len() as i64);
                    } else if let Some(files) = &self.fs_bytes {
                        if let Some(b) = files
                            .get(&p)
                            .or_else(|| files.get(p_clean))
                            .or_else(|| files.get(&format!("/{}", p_clean)))
                        {
                            found_len = Some(b.len() as i64);
                        }
                    }
                }

                if let Some(len) = found_len {
                    unsafe {
                        std::ptr::write_bytes(statbuf, 0, STAT_SIZE);
                        *(statbuf.add(24) as *mut i64) = 32768 | 0o644;
                        *(statbuf.add(48) as *mut i64) = len;
                    }
                    0
                } else if self.is_mock_dir(&p) {
                    unsafe {
                        std::ptr::write_bytes(statbuf, 0, STAT_SIZE);
                        *(statbuf.add(24) as *mut i64) = 16384 | 0o755;
                        *(statbuf.add(48) as *mut i64) = 4096;
                    }
                    0
                } else {
                    -2 // ENOENT
                }
            }
            // ADR 0049 決定 5: 差し替え下でシミュレーション不可能な操作（OFD ロック、生 fd 露出、生ディレクトリエントリ）は
            // 暗黙の偽装を行わず fail-closed（-95: EOPNOTSUPP）で明示的に拒否する。
            SYS_OPENAT => self.double_openat(a),
            SYS_READLINKAT => self.double_readlinkat(a),
            SYS_FLOCK => self.double_flock(a),
            SYS_GETDENTS64 => -95,
            _ => -95,
        }
    }

    /// 差し替え下のパス引数を、仮のルートからの絶対パスにする。絶対パスと `AT_FDCWD` 基準はそのまま。
    /// ディレクトリ fd 基準の相対パスは、fd が影の根の下を指すときだけ、その仮のパスに解決する（それ以外は fail-closed の -95）。
    fn double_path(&mut self, dirfd: i64, addr: usize) -> Result<String, i64> {
        let path = self.path_string(addr);
        if path.starts_with('/') || dirfd == -100 {
            return Ok(path);
        }
        let comps = self.shadow_dir_comps(dirfd)?;
        let Some(add) = super::shadow::split(&path) else {
            return Err(-95);
        };
        let mut all = comps;
        all.extend(add);
        Ok(format!("/{}", all.join("/")))
    }

    /// 影の根の下のディレクトリ fd が指す、仮のパスの構成要素。
    fn shadow_dir_comps(&mut self, dirfd: i64) -> Result<Vec<String>, i64> {
        let Ok(root) = self.fs_shadow.ensure_root() else {
            return Err(-5);
        };
        let Some(dir) = self.shadow_fd_path(dirfd) else {
            return Err(-95);
        };
        let Ok(rel) = dir.strip_prefix(&root) else {
            return Err(-95); // 影の根の下でない fd は差し替えの外
        };
        rel.components()
            .map(|c| c.as_os_str().to_str().map(String::from))
            .collect::<Option<Vec<_>>>()
            .ok_or(-95)
    }

    /// `/proc/self/fd/<fd>` が指す実際のパス（影の根の下か調べるのに使う）。
    fn shadow_fd_path(&self, fd: i64) -> Option<std::path::PathBuf> {
        std::fs::read_link(format!("/proc/self/fd/{fd}")).ok()
    }

    /// 影の実ファイルに対してパス引数のシステムコールを発行する（`rest` はパスの後ろの引数 2 つ）。
    fn shadow_syscall_path(&self, n: i64, real: &std::path::Path, rest: [i64; 2]) -> i64 {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c) = std::ffi::CString::new(real.as_os_str().as_bytes()) else {
            return -22;
        };
        // newfstatat(AT_FDCWD, path, statbuf, flags)
        issue_syscall(n, [-100, c.as_ptr() as i64, rest[0], rest[1], 0, 0])
    }

    /// 表の内容から、仮のパスの実ファイルを影の根に置く（既に置いてあれば何もしない）。
    fn shadow_materialize_file(
        &mut self,
        comps: &[String],
        root: &std::path::Path,
    ) -> std::io::Result<()> {
        let key = comps.join("/");
        if self.fs_shadow.is_marked(&key) {
            return Ok(());
        }
        let virt = format!("/{key}");
        let p_clean = key.as_str();
        let alt = [virt.as_str(), p_clean];
        let from_text = self
            .fs
            .as_ref()
            .and_then(|f| alt.iter().find_map(|k| f.get(*k)))
            .map(|t| t.as_bytes().to_vec());
        let deleted = alt.iter().any(|k| self.fs_bytes_deleted.contains(*k));
        let from_bytes = if deleted {
            None
        } else {
            alt.iter()
                .find_map(|k| self.fs_bytes_written.get(*k))
                .cloned()
                .or_else(|| {
                    self.fs_bytes
                        .as_ref()
                        .and_then(|f| alt.iter().find_map(|k| f.get(*k)))
                        .cloned()
                })
        };
        let Some(content) = from_text.or(from_bytes) else {
            return Ok(());
        };
        let real = root.join(&key);
        if let Some(parent) = real.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&real, content)?;
        self.fs_shadow.mark(key);
        Ok(())
    }

    /// 差し替え下の `openat`（ADR 0049 決定 5.1）。仮のパスを影の根の下の実パスに写して、本物の `openat` を発行する。
    /// ディレクトリ fd を基準にした相対パスは、fd が影の根の下を指すときだけ通す（それ以外は fail-closed）。
    fn double_openat(&mut self, a: [i64; 6]) -> i64 {
        const O_DIRECTORY: i64 = 65536;
        let path = self.path_string(a[1] as usize);
        let Some(add) = super::shadow::split(&path) else {
            return -95; // `..` や NUL。影の根の外へ出す可能性のあるパスは解決しない
        };
        let Ok(root) = self.fs_shadow.ensure_root() else {
            return -5; // EIO: 影の根を作れない
        };
        // 絶対パスと AT_FDCWD 基準は、影の根（仮のルート）から始める。
        let mut comps: Vec<String> = if path.starts_with('/') || a[0] == -100 {
            vec![]
        } else {
            match self.shadow_dir_comps(a[0]) {
                Ok(v) => v,
                Err(e) => return e,
            }
        };
        comps.extend(add);
        // 仮の表にあるファイルとディレクトリを、開く前に影へ置く（存在しないものは作らない。O_CREAT は本物の openat が作る）。
        for i in 1..=comps.len() {
            let key = comps[..i].join("/");
            if self.is_mock_file(&key) {
                if let Err(e) = self.shadow_materialize_file(&comps[..i], &root) {
                    return -(e.raw_os_error().unwrap_or(5) as i64);
                }
            } else if self.is_mock_dir(&key) {
                if let Err(e) = std::fs::create_dir_all(root.join(&key)) {
                    return -(e.raw_os_error().unwrap_or(5) as i64);
                }
            }
        }
        let mut real = root.join(comps.join("/"));
        // 末尾の `/` と `/.` は「ディレクトリでなければならない」という制約。正規化で捨てずに、実パスに `/` として残し、
        // 通常ファイルに対しては本物の `openat` が ENOTDIR を返すようにする（ネイティブと同じ結果）。
        let dir_required = super::shadow::requires_dir(&path);
        if dir_required {
            let mut os = real.into_os_string();
            os.push("/");
            real = std::path::PathBuf::from(os);
        }
        let fd = self.shadow_syscall_path(SYS_OPENAT, &real, [a[2], a[3]]);
        if fd >= 0 && a[2] & O_DIRECTORY == 0 && !dir_required {
            self.fs_shadow.mark(comps.join("/"));
        }
        fd
    }

    /// fd が影の根の下の実ファイル／実ディレクトリを指すか。
    fn is_shadow_fd(&self, fd: i64) -> bool {
        match (self.fs_shadow.root(), self.shadow_fd_path(fd)) {
            (Some(root), Some(p)) => p.starts_with(root),
            _ => false,
        }
    }

    /// 差し替え下の `readlinkat`。影の根の下のディレクトリ fd を基準にした相対パスに限り、影の実パスに対して本物の `readlinkat` を発行する。
    /// それ以外の fd や `..` を含むパスは fail-closed（-95: EOPNOTSUPP）。
    fn double_readlinkat(&mut self, a: [i64; 6]) -> i64 {
        let path = self.path_string(a[1] as usize);
        if path.starts_with('/') || a[0] == -100 || super::shadow::requires_dir(&path) {
            return -95;
        }
        let Some(add) = super::shadow::split(&path) else {
            return -95;
        };
        if add.is_empty() {
            return -95;
        }
        let Ok(root) = self.fs_shadow.ensure_root() else {
            return -5;
        };
        let mut comps = match self.shadow_dir_comps(a[0]) {
            Ok(v) => v,
            Err(e) => return e,
        };
        comps.extend(add);
        for i in 1..=comps.len() {
            let key = comps[..i].join("/");
            if self.is_mock_file(&key) {
                if let Err(e) = self.shadow_materialize_file(&comps[..i], &root) {
                    return -(e.raw_os_error().unwrap_or(5) as i64);
                }
            } else if self.is_mock_dir(&key) {
                if let Err(e) = std::fs::create_dir_all(root.join(&key)) {
                    return -(e.raw_os_error().unwrap_or(5) as i64);
                }
            }
        }
        let real = root.join(comps.join("/"));
        self.shadow_syscall_path(SYS_READLINKAT, &real, [a[2], a[3]])
    }

    /// 差し替え下の `flock`。影の実ファイルの fd にだけ本物の `flock` を発行し、それ以外は fail-closed（ロックを偽装しない）。
    fn double_flock(&self, a: [i64; 6]) -> i64 {
        if self.is_shadow_fd(a[0]) {
            issue_syscall(SYS_FLOCK, [a[0], a[1], 0, 0, 0, 0])
        } else {
            -95
        }
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

/// fd を閉じる（`close(2)`。失敗は捨てる）。fd を所有する値（`TcpListener`・`TcpStream`・`File`・`Dir`）の破棄からだけ呼ぶ
/// （`value::close_owned_fd`。ADR 0031 決定 3-1、ADR 0049 決定 1-1）。`close` は許可表にある番号で、番地を取らない。
pub(super) fn close_fd(fd: i64) {
    if fd >= 0 {
        let _ = issue_syscall(SYS_CLOSE, [fd, 0, 0, 0, 0, 0]);
    }
}

thread_local! {
    static ACTIVE_CHUNKS: std::cell::Cell<*mut BTreeMap<usize, Chunk>> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
}

pub(super) struct ChunksScope {
    prev: *mut BTreeMap<usize, Chunk>,
}

impl ChunksScope {
    pub(super) fn enter(chunks: &mut BTreeMap<usize, Chunk>) -> Self {
        let prev = ACTIVE_CHUNKS.with(|c| {
            let old = c.get();
            c.set(chunks as *mut _);
            old
        });
        Self { prev }
    }
}

impl Drop for ChunksScope {
    fn drop(&mut self) {
        ACTIVE_CHUNKS.with(|c| c.set(self.prev));
    }
}

/// Dir ハンドルの破棄で列挙バッファを解放する（value::close_owned_fd、ADR 0049 決定 1-1）。
pub(super) fn free_dir_buf(addr: usize) {
    if addr == 0 {
        return;
    }
    ACTIVE_CHUNKS.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            unsafe {
                if let Some(chunk) = (*ptr).remove(&addr) {
                    if chunk.origin == Origin::Alloc {
                        let layout =
                            std::alloc::Layout::from_size_align_unchecked(chunk.size, chunk.align);
                        std::alloc::dealloc(addr as *mut u8, layout);
                    }
                }
            }
        }
    });
}

const PAGE: usize = 4096;
const STAT_SIZE: usize = 144;
const SYS_READ: i64 = 0;
const SYS_WRITE: i64 = 1;
const SYS_CLOSE: i64 = 3;
const SYS_FSTAT: i64 = 5;
const SYS_FCHMOD: i64 = 91;
const SYS_MMAP: i64 = 9;
const SYS_MUNMAP: i64 = 11;
const SYS_RT_SIGPROCMASK: i64 = 14;
const SYS_PREAD64: i64 = 17;
const SYS_PWRITE64: i64 = 18;
const SYS_SIGNALFD4: i64 = 289;
const SHUTDOWN_MASK: u64 = (1 << (15 - 1)) | (1 << (2 - 1));
const SYS_EXIT_GROUP: i64 = 231;
const SYS_FLOCK: i64 = 73;
const SYS_FSYNC: i64 = 74;
const SYS_FDATASYNC: i64 = 75;
const SYS_RENAMEAT: i64 = 264;
const SYS_GETDENTS64: i64 = 217;
const SYS_OPENAT: i64 = 257;
const SYS_MKDIRAT: i64 = 258;
const SYS_NEWFSTATAT: i64 = 262;
const SYS_UNLINKAT: i64 = 263;
const SYS_READLINKAT: i64 = 267;
const LOCK_SH: i64 = 1;
const LOCK_EX: i64 = 2;
const LOCK_NB: i64 = 4;
const LOCK_UN: i64 = 8;
const SYS_SOCKET: i64 = 41;
const SYS_CONNECT: i64 = 42;
const SYS_GETSOCKOPT: i64 = 55;
const SYS_GETRANDOM: i64 = 318;
const SYS_CLOCK_GETTIME: i64 = 228;
const CLOCK_REALTIME: i64 = 0;
const SYS_SENDTO: i64 = 44;
const SYS_RECVFROM: i64 = 45;
const SYS_SHUTDOWN: i64 = 48;
const SYS_BIND: i64 = 49;
const SYS_LISTEN: i64 = 50;
const SYS_GETSOCKNAME: i64 = 51;
const SYS_SETSOCKOPT: i64 = 54;
const SYS_ACCEPT4: i64 = 288;
const SOCKADDR_IN_SIZE: usize = 16;
const SOCKADDR_IN6_SIZE: usize = 28;
const AF_INET: i64 = 2;
const AF_INET6: i64 = 10;
const SOCK_STREAM: i64 = 1;
const SOCK_DGRAM: i64 = 2;
const SOCK_NONBLOCK: i64 = 0o4000;
const SOCK_CLOEXEC: i64 = 0o2000000;
const SOL_SOCKET: i64 = 1;
const SO_REUSEADDR: i64 = 2;
const SO_ERROR: i64 = 4;
const IPPROTO_IPV6: i64 = 41;
const IPV6_V6ONLY: i64 = 26;
const MSG_NOSIGNAL: i64 = 0x4000;
const MSG_TRUNC: i64 = 0x20;
/// UDP のペイロードの上限（IPv4 の最小 IP ヘッダと UDP ヘッダを引いた値。ADR 0057 決定 1）。
const UDP_MAX_PAYLOAD: i64 = 65507;
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

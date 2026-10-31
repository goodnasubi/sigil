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
use super::{Slot, E, M};

impl<'p> M<'p> {
    pub(super) fn call_val(&mut self, f: &Val, args: Vec<Val>, loc: StopId) -> Result<Val, E> {
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

use std::collections::HashMap;

use super::{
    analyze, block_returns, contains_ref, contains_ref_outside_fn, show, AccKind, Ck, Local, Prop,
    Ty, R,
};
use crate::ast::*;
use crate::effects::Options;
use crate::error::{ErrorKind, Span};
use crate::parser::parse;

/// クロージャが外側の値をとらえる方法（ADR 0012 の決定 1 の 5。ADR 0006 の条件 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapMode {
    /// 値を移す（`Copyable` でない値を `move` のクロージャが持つ）。
    Move,
    /// 値を複製して持つ（`Copyable` な値を `move` のクロージャが持つ）。
    Copy,
    /// `&` で借りる（呼び出しの引数の位置のクロージャが、読むだけの値）。
    Ref,
    /// `&mut` で借りる（呼び出しの引数の位置のクロージャが、書き換える値）。
    RefMut,
}

/// クロージャがとらえた値 1 つ。
#[derive(Debug, Clone, PartialEq)]
pub struct Capture {
    pub name: String,
    /// とらえた変数の型（表示用）。
    pub ty: String,
    pub mode: CapMode,
    pub(super) props: Vec<(&'static str, Option<bool>)>,
}

impl Capture {
    /// とらえ方を含めた性質（借りてとらえるなら `&T`・`&mut T` が持つ性質）。決まらなければ `None`。
    pub fn has(&self, prop: &str) -> Option<bool> {
        self.props
            .iter()
            .find(|(n, _)| *n == prop)
            .and_then(|p| p.1)
    }
}

/// クロージャ 1 つの、とらえた値の一覧（ADR 0006 の条件 3: クロージャの型から取り出せる情報）。
#[derive(Debug, Clone, PartialEq)]
pub struct ClosureInfo {
    /// クロージャを含む関数の名前。
    pub func: String,
    pub line: u32,
    /// 呼び出しの引数の位置にない（`let` に入れる・返すなど）。`move` が要る。
    pub escaping: bool,
    pub is_move: bool,
    /// `let` で束縛した名前。
    pub bound: Option<String>,
    pub captures: Vec<Capture>,
}

impl ClosureInfo {
    /// とらえた値のすべてが性質 `prop` を持つか（条件 1 の性質を、とらえた値の一覧に対して調べる）。
    /// 持たないものが 1 つでもあれば `Some(false)`、全部持てば `Some(true)`、決まらなければ `None`。
    pub fn satisfies(&self, prop: &str) -> Option<bool> {
        let mut unknown = false;
        for c in &self.captures {
            match c.has(prop) {
                Some(false) => return Some(false),
                None => unknown = true,
                Some(true) => {}
            }
        }
        if unknown {
            None
        } else {
            Some(true)
        }
    }
}

/// ソースを検査して、クロージャごとのとらえた値の一覧を返す。
pub fn closure_captures(src: &str) -> R<Vec<ClosureInfo>> {
    analyze(&parse(src)?, &[], &[], &Options::default()).map(|ck| ck.closures)
}

/// 関数の本体の検査中の、クロージャの枠。
pub(super) struct Frame {
    /// クロージャの引数のスコープの位置（これより前のスコープの変数が外側）。
    pub(super) base: usize,
    pub(super) caps: Vec<Cap>,
}

/// 外側の変数のとらえ方の記録（変数ごと）。
pub(super) struct Cap {
    pub(super) name: String,
    pub(super) write: bool,
    /// とらえた場所（経路と、書き換えか）。
    pub(super) paths: Vec<(String, bool)>,
}

/// 検査し終えたクロージャ（型が決まる前の記録）。
pub(super) struct RawClosure {
    pub(super) span: Span,
    pub(super) escaping: bool,
    pub(super) is_move: bool,
    pub(super) bound: Option<String>,
    pub(super) caps: Vec<(String, Ty, CapMode)>,
}

impl Ck {
    pub(super) fn closure(
        &mut self,
        is_move: bool,
        params: &[String],
        body: &ClosureBody,
        hint: Option<&Ty>,
    ) -> R<Ty> {
        // 呼び出しの引数の位置のクロージャは、その呼び出しの間だけ生きる。それ以外（`let` に入れる・
        // 返す・入れ物に入れる）は呼び出しより長く生きうるので `move` が要る（位置だけで決める）。
        let escaping = !std::mem::take(&mut self.closure_arg);
        if escaping && !is_move && self.track {
            return self.err(
                ErrorKind::ClosureNeedsMove,
                "呼び出しの引数の位置にないクロージャは、外側の値を持ち出しうるので `move |..| ...` と書く（とらえた値は移すか複製して持つ。何もとらえなくても、位置で決まる）",
            );
        }
        let (hps, hret) = match hint.map(|h| self.shallow(h)) {
            Some(Ty::Fn(ps, r)) => (Some(ps), Some(*r)),
            _ => (None, None),
        };
        if let Some(ps) = &hps {
            if ps.len() != params.len() {
                return self.err(
                    ErrorKind::ArgCount,
                    format!(
                        "クロージャの引数は {} 個だが、渡す先は {} 個の引数の関数を求めている",
                        params.len(),
                        ps.len()
                    ),
                );
            }
        }
        let defer = std::mem::take(&mut self.defer);
        self.eff_stack.push(vec![]);
        self.scopes.push(HashMap::new());
        // 本体の中の流れは捨てる。外側への影響は、本体を調べ終えてから、とらえ方に従って足す。
        let outer_flow = self.flow.clone();
        let log_mark = self.log.len();
        let outer_base = std::mem::replace(&mut self.closure_base, self.scopes.len() - 1);
        self.frames.push(Frame {
            base: self.scopes.len() - 1,
            caps: vec![],
        });
        let mut pts = vec![];
        for (i, p) in params.iter().enumerate() {
            self.shadow_check(p)?;
            self.forget(p);
            let t = match &hps {
                Some(ps) => ps[i].clone(),
                None => self.fresh_known(format!("クロージャの引数 `{p}`")),
            };
            self.scopes.last_mut().expect("scope").insert(
                p.clone(),
                Local {
                    ty: t.clone(),
                    mutable: false,
                    param: true,
                },
            );
            pts.push(t);
        }
        let saved = self.ret.clone();
        let r = match hret {
            Some(r) => r,
            None => self.fresh(),
        };
        self.ret = r.clone();
        let sp = self.span;
        self.closure_depth += 1;
        let res: R<()> = match body {
            ClosureBody::Expr(e) => match self.expr(e) {
                Ok(t) => {
                    self.span = sp;
                    if contains_ref(&self.zonk(&t)) {
                        self.return_roots(e)?;
                    }
                    self.consume(e, &t)?;
                    if self.coerce(&r, &t, e) {
                        Ok(())
                    } else {
                        self.err(
                            ErrorKind::TypeMismatch,
                            format!(
                                "クロージャの戻り値は `{}` でなければならないが、`{}` である",
                                self.sh(&r),
                                self.sh(&t)
                            ),
                        )
                    }
                }
                Err(e) => Err(e),
            },
            ClosureBody::Block(b) => self.block(b).and_then(|()| {
                self.span = sp;
                if !block_returns(b) && !self.try_unify(&r, &Ty::Unit) {
                    return self.err(
                        ErrorKind::MissingReturn,
                        format!(
                            "クロージャは `{}` を返すので、すべての道が `return` で終わらなければならない",
                            self.sh(&r)
                        ),
                    );
                }
                Ok(())
            }),
        };
        self.closure_depth -= 1;
        self.ret = saved;
        self.scopes.pop();
        self.sync_borrowed();
        self.flow = outer_flow;
        self.log.truncate(log_mark);
        self.res.clear();
        self.closure_base = outer_base;
        let frame = self.frames.pop();
        let effs = self.eff_stack.pop().unwrap_or_default();
        if defer {
            self.last_closure = effs;
        } else {
            for a in effs {
                self.add_eff(a);
            }
        }
        res?;
        self.span = sp;
        self.apply_captures(frame, escaping, is_move)?;
        Ok(Ty::Fn(pts, Box::new(r)))
    }

    /// クロージャがとらえた外側の値のとらえ方を決めて、外側の流れに足す（ADR 0012 の決定 1 の 5）。
    /// - 引数の位置で `move` のないもの: 読むだけなら `&`、書き換えるなら `&mut` で借りる（呼び出しの間）。
    /// - `move` のもの、脱出するもの: 値を移す（`Copyable` なら複製）。参照を含む値はとらえられない。
    fn apply_captures(&mut self, frame: Option<Frame>, escaping: bool, is_move: bool) -> R<()> {
        let Some(frame) = frame else { return Ok(()) };
        if !self.track || self.refine {
            return Ok(());
        }
        let mut done = vec![];
        for c in &frame.caps {
            let Some(l) = self.lookup(&c.name).cloned() else {
                continue;
            };
            let ty = self.zonk(&l.ty);
            let has_ref = contains_ref_outside_fn(&ty);
            let by_value = escaping || is_move;
            let mode = if by_value && !has_ref {
                match self.holds(&ty, Prop::Copyable, 0) {
                    Some(true) => CapMode::Copy,
                    // 型が決まらないものは、移動とみなす（`consume` と同じ保守的な扱い）
                    _ => CapMode::Move,
                }
            } else if by_value && escaping {
                return self.err(
                    ErrorKind::ClosureCaptureRef,
                    format!(
                        "`{}` は参照（`{}`）を含むので、呼び出しより長く生きるクロージャはとらえられない（参照の指す先がクロージャより先に消えうる。値を `.clone()` で複製して持たせる）",
                        c.name,
                        show(&ty)
                    ),
                );
            } else if c.write {
                CapMode::RefMut
            } else if by_value && matches!(ty, Ty::Ref(false, _)) {
                CapMode::Copy
            } else {
                CapMode::Ref
            };
            done.push((c, ty, mode));
        }
        let mut caps = vec![];
        for (c, ty, mode) in done {
            match mode {
                CapMode::Move if self.fn_borrowed.contains(&c.name) => {
                    // 借りている関数の値は、とらえても移動しない。同じ呼び出しで借りていれば重なりになる
                    self.log_acc(&c.name, AccKind::Move);
                }
                CapMode::Move => {
                    if self.holds(&ty, Prop::Copyable, 0).is_none() {
                        // 型が決まってから、使用を調べ直す
                        if !self.is_outer(&c.name) {
                            let at = self.span;
                            self.flow
                                .maybe
                                .entry(c.name.clone())
                                .or_insert((at, ty.clone()));
                        }
                    } else {
                        self.record_move(&c.name, &c.name)?;
                    }
                }
                CapMode::Copy => {
                    for (p, _) in &c.paths {
                        self.touch(p, false, "クロージャのとらえ");
                        self.log_acc(p, AccKind::Read);
                    }
                }
                CapMode::Ref | CapMode::RefMut => {
                    for (p, w) in &c.paths {
                        self.touch(p, *w, "クロージャのとらえ");
                        self.log_acc(p, if *w { AccKind::Mut } else { AccKind::Shared });
                    }
                }
            }
            caps.push((c.name.clone(), ty, mode));
        }
        self.cur_closures.push(RawClosure {
            span: self.span,
            escaping,
            is_move,
            bound: None,
            caps,
        });
        Ok(())
    }
}

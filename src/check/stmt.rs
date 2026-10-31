use std::collections::HashMap;

use crate::ast::*;
use crate::borrow::TEMP;
use crate::error::{Error, ErrorKind};

use super::types::{show, Ty};
use super::util::{
    contains_ref_outside_fn, pat_names, place_path, place_root, strip_paren, strip_refs,
};
use super::{merge, Atom, Ck, Flow, Local, Pos, R};

impl Ck {
    pub(super) fn expect(&mut self, expected: &Ty, e: &Expr, what: &str) -> R<()> {
        self.expect_ty(expected, e, what).map(|_| ())
    }

    pub(super) fn expect_ty(&mut self, expected: &Ty, e: &Expr, what: &str) -> R<Ty> {
        let t = self.expr(e)?;
        if !self.coerce(expected, &t, e) {
            return self.err(
                self.mismatch_kind(expected, &t),
                format!(
                    "{what}は `{}` でなければならないが、`{}` である{}",
                    self.sh(expected),
                    self.sh(&t),
                    self.check_hint(expected, &t)
                ),
            );
        }
        Ok(t)
    }

    pub(super) fn check_hint(&self, expected: &Ty, actual: &Ty) -> &'static str {
        let (ex, ac) = (self.zonk(expected), self.zonk(actual));
        if self
            .refinement_base(&ex)
            .is_some_and(|b| self.zonk(&b) == ac)
        {
            "（基の型から篩型への変換は、リテラルの定数か `T.check` に限る）"
        } else if self.refine_mut_mismatch(&ex, &ac) {
            "（`&mut` で篩型を基の型として渡すと、篩型の条件を壊す値を書き込めてしまう。`&` なら渡せる）"
        } else if matches!(ex, Ty::Int) && ac == Ty::Float
            || matches!(ex, Ty::Float) && ac == Ty::Int
        {
            "（`Int` と `Float` の暗黙の変換はない）"
        } else {
            ""
        }
    }

    pub(super) fn stmt(&mut self, s: &Stmt) -> R<()> {
        self.span = s.span;
        match &s.kind {
            StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
            } => {
                let closure = matches!(strip_paren(value), Expr::Closure { .. });
                self.defer = closure;
                let vt = self.expr(value);
                self.defer = false;
                let vt = vt?;
                let let_srcs = std::mem::take(&mut self.res);
                let vt_moved = vt.clone();
                self.fn_value_form(value, &vt)?;
                let mut value_closure_effs: Option<Vec<Atom>> = None;
                if closure {
                    if let Pattern::Bind(n) = pattern {
                        let effs = std::mem::take(&mut self.last_closure);
                        value_closure_effs = Some(effs.clone());
                        self.local_eff.insert(n.clone(), effs);
                        if let Some(c) = self.cur_closures.last_mut() {
                            c.bound = Some(n.clone());
                        }
                    } else {
                        // 束縛されないクロージャは呼べない。効果は起こらない。
                        self.last_closure.clear();
                    }
                } else if matches!(self.zonk(&vt), Ty::Fn(..))
                    && matches!(strip_paren(value), Expr::Name(_))
                {
                    // 別名は元の関数の効果を引き継ぐ（引数の別名なら `of(f)` のまま）。
                    if let Pattern::Bind(n) = pattern {
                        let effs = self.fn_effects(value);
                        self.local_eff.insert(n.clone(), effs);
                    }
                }
                self.span = s.span;
                let fin = match ty {
                    None => vt,
                    Some(t) => {
                        let d = self.resolve(t, Pos::Placed)?;
                        let i = self.annot_n;
                        self.annot_n += 1;
                        if self.skip == Some(i) {
                            self.skipped = Some(vt.clone());
                            vt
                        } else {
                            if self.skip.is_none() {
                                self.annots.push((s.span, d.clone()));
                            }
                            if !self.coerce(&d, &vt, value) {
                                return self.err(
                                    self.mismatch_kind(&d, &vt),
                                    format!(
                                        "型注釈は `{}` だが、式の型は `{}` である{}",
                                        show(&d),
                                        self.sh(&vt),
                                        self.check_hint(&d, &vt)
                                    ),
                                );
                            }
                            d
                        }
                    }
                };
                if self.skip.is_none() {
                    self.rec_lets.push((s as *const Stmt as usize, fin.clone()));
                }
                // 置ける位置の型注釈に入れる値の検査（効果の順序・とらえ方。
                // ADR 0022 決定 3）。注釈の要否の再検査（skip）では行わない。
                if self.skip.is_none() {
                    if let Ty::Fn(_, _, Some(d)) = self.zonk(&fin) {
                        self.check_fn_insert(&d, value, &vt_moved, value_closure_effs.as_deref())?;
                    }
                }
                if *mutable && contains_ref_outside_fn(&self.zonk(&fin)) {
                    return self.err(
                        ErrorKind::LetMutRef,
                        "型が参照を含む変数（`&T`・`Option<&T>` など）は `let mut` で宣言できない（参照を別の参照で束縛し直す操作はない）",
                    );
                }
                if *mutable && matches!(self.zonk(&fin), Ty::Fn(..)) {
                    return self.err(
                        ErrorKind::FnValueReassign,
                        "関数の型の変数は `let mut` で宣言できない（関数の値の再代入では効果を追えない。`let` で束縛し直す）",
                    );
                }
                if !matches!(pattern, Pattern::Wild) {
                    self.consume(value, &vt_moved)?;
                }
                self.bind(pattern, &fin, *mutable)?;
                if matches!(self.zonk(&vt_moved), Ty::Fn(..)) && self.borrowed_fn(value) {
                    if let Pattern::Bind(n) = pattern {
                        self.fn_borrowed.insert(n.clone());
                    }
                }
                let mut names = vec![];
                pat_names(pattern, &mut names);
                if *mutable && self.has_var(&fin) {
                    for n in &names {
                        if let Some(l) = self.lookup(n) {
                            let t = l.ty.clone();
                            self.let_pending.push((n.clone(), t, s.span));
                        }
                    }
                }
                if self.track
                    && let_srcs.iter().any(|(p, _)| p == TEMP)
                    && names.iter().any(|n| {
                        self.lookup(n)
                            .is_some_and(|l| contains_ref_outside_fn(&self.zonk(&l.ty)))
                    })
                {
                    self.span = s.span;
                    return self.err(
                        ErrorKind::TempBorrow,
                        "一時的な値への参照は、この文の終わりで使えなくなるので、変数に持てない（一時的な値を先に `let` の変数に入れてから借りる。例: `let l = make(); let r = l.get(0);`）",
                    );
                }
                self.adopt(&names, &let_srcs, false)
            }
            StmtKind::Assign { target, value } => {
                let root = place_root(target).map(str::to_string);
                self.skip_use = place_path(target).is_some();
                let tt = self.expr(target);
                self.skip_use = false;
                let tt = tt?;
                self.span = s.span;
                let Some(root) = root else {
                    return self.err(
                        ErrorKind::AssignToImmutable,
                        "代入できるのは変数とそのフィールドだけ",
                    );
                };
                if matches!(self.zonk(&tt), Ty::Fn(..)) {
                    return self.err(
                        ErrorKind::FnValueReassign,
                        "関数の値は再代入できない（引数への代入も同じ。`let` で束縛し直す）",
                    );
                }
                self.assignable_place(&root, target, true)?;
                self.span = s.span;
                let vt = self.expr(value)?;
                self.span = s.span;
                if !self.coerce(&tt, &vt, value) {
                    return self.err(
                        ErrorKind::TypeMismatch,
                        format!(
                            "代入先は `{}` だが、右辺は `{}` である{}",
                            self.sh(&tt),
                            self.sh(&vt),
                            self.check_hint(&tt, &vt)
                        ),
                    );
                }
                self.consume(value, &vt)?;
                if let Some((r, p)) = place_path(target) {
                    self.touch(&p, true, "代入");
                    self.reinit(&r, &p)?;
                }
                Ok(())
            }
            StmtKind::Expr(e) => self.expr(e).map(|_| ()),
            StmtKind::Return(v) => {
                let ret = self.ret.clone();
                match v {
                    None => {
                        self.ens_check()?;
                        self.flow.dead = true;
                        if !self.try_unify(&ret, &Ty::Unit) {
                            return self.err(
                                ErrorKind::TypeMismatch,
                                format!(
                                    "`{}` を返す関数で、値のない `return;` は書けない",
                                    self.sh(&ret)
                                ),
                            );
                        }
                        Ok(())
                    }
                    Some(e) if contains_ref_outside_fn(&self.zonk(&ret)) => {
                        // 参照を返す関数（ADR 0012 の決定 1）。`return 場所;` は場所への参照を返す
                        // ので、型は参照を外して比べる。どの場所を返せるかは所有権の検査で調べる。
                        let t = self.expr(e)?;
                        self.return_roots(e)?;
                        let (want, got) =
                            (strip_refs(&self.zonk(&ret)), strip_refs(&self.zonk(&t)));
                        if !self.coerce(&want, &got, e) {
                            return self.err(
                                ErrorKind::TypeMismatch,
                                format!(
                                    "戻り値は `{}` でなければならないが、`{}` である",
                                    self.sh(&ret),
                                    self.sh(&t)
                                ),
                            );
                        }
                        self.ens_check()?;
                        self.flow.dead = true;
                        Ok(())
                    }
                    Some(e) => {
                        // 戻すクロージャの効果は取り分けて、入れる検査に使う
                        let closure = matches!(strip_paren(e), Expr::Closure { .. });
                        if closure {
                            self.defer = true;
                        }
                        let t = self.expect_ty(&ret, e, "戻り値");
                        let ce = if closure {
                            self.defer = false;
                            std::mem::take(&mut self.last_closure)
                        } else {
                            vec![]
                        };
                        let t = t?;
                        // 置ける位置の戻り値に入れる値の検査（ADR 0022 決定 3）
                        if let Ty::Fn(_, _, Some(d)) = self.zonk(&ret) {
                            self.fn_value_form(e, &t)?;
                            self.check_fn_insert(
                                &d,
                                e,
                                &t,
                                if closure { Some(&ce) } else { None },
                            )?;
                        }
                        self.consume(e, &t)?;
                        self.ens_check()?;
                        self.flow.dead = true;
                        Ok(())
                    }
                }
            }
            StmtKind::If(i) => self.if_stmt(i),
            StmtKind::While { cond, body } => {
                // ループは、本体の終わりの状態が次の周の頭の状態に合流する。外側の場所を本体で
                // 移動して再代入しないまま周の終わりに届くなら、次の周でその移動が二重になる。
                let head = self.flow.moved.clone();
                let head_loans = self.flow.loans.clone();
                let head_used = std::mem::take(&mut self.used);
                self.expect(&Ty::Bool, cond, "`while` の条件")?;
                let after_cond = self.flow.clone();
                self.block(body)?;
                if !self.flow.dead && self.track {
                    let hit = self
                        .flow
                        .moved
                        .iter()
                        .find(|(k, _)| {
                            !head.contains_key(*k)
                                && self.lookup(k.split('.').next().unwrap_or(k)).is_some()
                        })
                        .map(|(k, at)| (k.clone(), *at));
                    if let Some((k, at)) = hit {
                        return Err(Error::new(
                            ErrorKind::MovedInLoop,
                            at,
                            format!(
                                "`{k}` は {} 行目でループの本体の中で移動していて、周の終わりまでに再代入されない。次の周でまた使われるので移動済みになる（本体の中で再代入するか、ループの外で移動する）",
                                at.line
                            ),
                        ));
                    }
                }
                // 周の中で使えなくなった貸しの変数が、周の中で使われていれば、次の周で使えない
                if !self.flow.dead && self.track {
                    let hit = self
                        .flow
                        .loans
                        .iter()
                        .find(|l| {
                            l.dead.is_some()
                                && self.used.contains(&l.holder)
                                && head_loans.iter().any(|h| {
                                    h.holder == l.holder
                                        && h.path == l.path
                                        && h.mutable == l.mutable
                                        && h.dead.is_none()
                                })
                        })
                        .cloned();
                    if let Some(l) = hit {
                        let at = l.dead.map_or(self.span, |d| d.0);
                        return Err(Error::new(
                            ErrorKind::BorrowConflict,
                            at,
                            format!(
                                "`{}` は `{}` を借りているが、ループの本体のこの行で `{}` が変更・移動されている。`{}` は次の周でも使われるので、使えなくなっている（借りた値の使用をループの中で完結させるか、変更の前に使い終える）",
                                l.holder, l.path, l.path, l.holder
                            ),
                        ));
                    }
                }
                self.used.extend(head_used);
                // 0 周で抜ける経路と、本体の終わりから抜ける経路の合流
                let end = std::mem::take(&mut self.flow);
                self.flow = merge(after_cond, end);
                Ok(())
            }
            StmtKind::Match {
                scrutinee, arms, ..
            } => {
                let (st, by_ref) = self.scrutinee(scrutinee)?;
                let m_srcs = std::mem::take(&mut self.res);
                let st = self.peel(&st);
                let sp = s.span;
                let mut pats = vec![];
                self.match_moves(scrutinee, &st, by_ref, arms.iter().map(|a| &a.pattern))?;
                let start = self.flow.clone();
                let mut joined: Option<Flow> = None;
                for a in arms {
                    self.span = sp;
                    self.flow = start.clone();
                    self.scopes.push(HashMap::new());
                    self.by_ref = by_ref;
                    let r = self.pat(&a.pattern, &st);
                    self.by_ref = false;
                    r?;
                    let mut names = vec![];
                    pat_names(&a.pattern, &mut names);
                    self.adopt(&names, &m_srcs, true)?;
                    self.block(&a.body)?;
                    self.pop_scope();
                    pats.push(&a.pattern);
                    let f = std::mem::take(&mut self.flow);
                    joined = Some(match joined {
                        None => f,
                        Some(j) => merge(j, f),
                    });
                }
                self.flow = joined.unwrap_or(start);
                self.span = sp;
                self.exhaustive(&pats)
            }
        }
    }

    /// `ensures` が参照する、値で受け取った引数が、ここまでの道で移動されていないこと。
    /// 関数から戻る点（`return`・`?`・本体の終わり）で調べる。
    pub(super) fn ens_check(&self) -> R<()> {
        if self.closure_base > 0 || self.ens_used.is_empty() || !self.track {
            return Ok(());
        }
        for (k, at) in &self.flow.moved {
            let root = k.split('.').next().unwrap_or(k);
            if self.ens_used.contains(root) {
                return Err(Error::new(
                    ErrorKind::EnsuresMoved,
                    *at,
                    format!(
                        "`{root}` は `ensures` が参照する引数だが、{} 行目で移動していて、関数から戻る道に届く（`ensures` は戻るときに引数の値を読むので、引数は移動しない。移さずに使うか `.clone()` する）",
                        at.line
                    ),
                ));
            }
        }
        Ok(())
    }

    fn if_stmt(&mut self, i: &IfStmt) -> R<()> {
        self.expect(&Ty::Bool, &i.cond, "`if` の条件")?;
        let start = self.flow.clone();
        self.block(&i.then)?;
        let then_end = std::mem::replace(&mut self.flow, start);
        match i.otherwise.as_deref() {
            None => {}
            Some(Else::If(n)) => self.if_stmt(n)?,
            Some(Else::Block(b)) => self.block(b)?,
        }
        let else_end = std::mem::take(&mut self.flow);
        self.flow = merge(then_end, else_end);
        Ok(())
    }

    /// 新しく宣言する変数の名前の、古い移動の記録を捨てる。
    pub(super) fn forget(&mut self, n: &str) {
        self.flow
            .moved
            .retain(|k, _| k.split('.').next().unwrap_or(k) != n);
        self.flow
            .maybe
            .retain(|k, _| k.split('.').next().unwrap_or(k) != n);
        self.flow.loans.retain(|l| l.holder != n);
    }

    pub(super) fn shadow_check(&self, n: &str) -> R<()> {
        if self.lookup(n).is_some() {
            return self.err(
                ErrorKind::ShadowedName,
                format!(
                    "名前 `{n}` は、同じ関数の外側ですでに使われている（名前の隠蔽は禁止。別の名前にする）"
                ),
            );
        }
        Ok(())
    }

    pub(super) fn declare(&mut self, n: &str, ty: Ty, mutable: bool) -> R<()> {
        self.shadow_check(n)?;
        self.forget(n);
        // 新しい束縛は、別名と分かるまで借用の関数値ではない
        self.fn_borrowed.remove(n);
        self.scopes.last_mut().expect("scope").insert(
            n.to_string(),
            Local {
                ty,
                mutable,
                param: false,
            },
        );
        Ok(())
    }

    /// `let` の分解。名前・`_`・組だけ（場合分けは `match`）。
    fn bind(&mut self, p: &Pattern, t: &Ty, mutable: bool) -> R<()> {
        match p {
            Pattern::Wild => Ok(()),
            Pattern::Bind(n) => self.declare(n, t.clone(), mutable),
            Pattern::Tuple(ps) => {
                let ts = match self.shallow(t) {
                    Ty::Tuple(ts) => ts,
                    Ty::Var(_) => {
                        let ts: Vec<Ty> = ps.iter().map(|_| self.fresh()).collect();
                        let _ = self.unify(t, &Ty::Tuple(ts.clone()));
                        ts
                    }
                    other => {
                        return self.err(
                            ErrorKind::DestructureMismatch,
                            format!(
                                "組として分解しているが、式の型は `{}` である",
                                self.sh(&other)
                            ),
                        )
                    }
                };
                if ps.len() != ts.len() {
                    return self.err(
                        ErrorKind::DestructureMismatch,
                        format!(
                            "{} 個に分解しているが、組の要素は {} 個ある",
                            ps.len(),
                            ts.len()
                        ),
                    );
                }
                for (p, t) in ps.iter().zip(&ts) {
                    self.bind(p, t, mutable)?;
                }
                Ok(())
            }
            _ => self.err(
                ErrorKind::PatternMismatch,
                "`let` で分解できるのは名前・`_`・組だけ（場合分けは `match` で書く）",
            ),
        }
    }

    /// 代入先（または `&mut` で借りる場所）の根の変数が書き換えられるか。
    pub(super) fn assignable_place(&self, root: &str, target: &Expr, assign: bool) -> R<()> {
        let Some(v) = self.lookup(root) else {
            return self.err(
                ErrorKind::UnknownName,
                format!("名前 `{root}` が見つからない"),
            );
        };
        let direct = matches!(target, Expr::Name(_) | Expr::SelfValue);
        match &v.ty {
            Ty::Ref(true, _) => Ok(()),
            Ty::Ref(false, _) => {
                if assign {
                    self.err(
                        ErrorKind::AssignThroughSharedRef,
                        format!(
                            "`{root}` は `{}` なので、{}書き換えられない（`&mut` で受け取る）",
                            show(&v.ty),
                            if direct { "" } else { "フィールドも" }
                        ),
                    )
                } else {
                    self.err(
                        ErrorKind::BorrowMutOfImmutable,
                        format!(
                            "`{root}` は `{}` なので、`&mut` では借りられない",
                            show(&v.ty)
                        ),
                    )
                }
            }
            _ if v.mutable => Ok(()),
            _ => {
                let why = if v.param {
                    "値で受け取った引数は書き換えられない（`&mut` で受け取る）"
                } else {
                    "`mut` のない `let` の変数は書き換えられない"
                };
                self.err(
                    if assign {
                        ErrorKind::AssignToImmutable
                    } else {
                        ErrorKind::BorrowMutOfImmutable
                    },
                    format!("`{root}`: {why}"),
                )
            }
        }
    }
}

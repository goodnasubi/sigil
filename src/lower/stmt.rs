use crate::ast as a;
use crate::ir::{self, Place, Rvalue, Term};

use super::{bail, is_ref, needs_drop, strip, Arm, L, R};

impl<'a> L<'a> {
    // ---- 文 ----

    pub(super) fn block(&mut self, b: &'a a::Block) -> R<()> {
        let mark = self.enter_scope();
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.exit_scope(mark);
        Ok(())
    }

    fn stmt(&mut self, s: &'a a::Stmt) -> R<()> {
        self.fb.line = s.span.line;
        let mark = self.fb.reg.len();
        match &s.kind {
            a::StmtKind::Let { pattern, value, .. } => {
                let op = self.expr(value)?;
                let ty = match self.t.let_ty.get(&(s as *const a::Stmt as usize)) {
                    Some(ct) => {
                        let m = self.fb.mono.clone();
                        self.conv(ct, &m)?
                    }
                    None => self.ety(value)?,
                };
                match pattern {
                    a::Pattern::Bind(n) => {
                        let l = self.new_local(n, ty.clone());
                        self.assign(Place::local(l), Rvalue::Use(op));
                        if needs_drop(&ty) {
                            self.register(l, false);
                        }
                        self.push_bind(n, Place::local(l), ty);
                    }
                    a::Pattern::Wild => {
                        let t = self.tmp(ty.clone());
                        self.assign(Place::local(t), Rvalue::Use(op));
                        if needs_drop(&ty) {
                            self.register(t, true);
                        }
                    }
                    p => {
                        let t = self.tmp(ty.clone());
                        self.assign(Place::local(t), Rvalue::Use(op));
                        if needs_drop(&ty) {
                            self.register(t, true);
                        }
                        self.pat_bind(p, &Place::local(t), &ty, None)?;
                    }
                }
                self.flush_temps(mark);
            }
            a::StmtKind::Assign { target, value } => {
                let op = self.expr(value)?;
                let Some((mut p, ty)) = self.place_expr(target)? else {
                    return bail("代入先が場所でない");
                };
                if is_ref(&ty) {
                    p = p.deref();
                }
                self.assign(p, Rvalue::Use(op));
                self.flush_temps(mark);
            }
            a::StmtKind::Expr(e) => {
                self.expr(e)?;
                self.flush_temps(mark);
            }
            a::StmtKind::Return(e) => {
                if let Some(e) = e {
                    let op = self.expr(e)?;
                    self.assign(Place::local(0), Rvalue::Use(op));
                }
                self.early_return();
            }
            a::StmtKind::If(i) => {
                let line = s.span.line;
                self.if_stmt(i, line)?;
            }
            a::StmtKind::While { cond, body } => {
                let head = self.new_block();
                let bb = self.new_block();
                let exit = self.new_block();
                self.goto(head);
                self.fb.cur = head;
                self.fb.line = s.span.line;
                let op = self.expr(cond)?;
                self.flush_temps(mark);
                self.term(Term::Branch(op, bb, exit));
                self.fb.cur = bb;
                self.block(body)?;
                self.goto(head);
                self.fb.cur = exit;
            }
            a::StmtKind::Match {
                scrutinee, arms, ..
            } => {
                let arms: Vec<(&a::Pattern, Arm)> =
                    arms.iter().map(|x| (&x.pattern, Arm::B(&x.body))).collect();
                self.match_(scrutinee, arms, None)?;
                self.flush_temps(mark);
            }
        }
        Ok(())
    }

    /// `return`・失敗する `?`: 本体の局所を逆順に破棄して終わりの基本ブロックへ。
    pub(super) fn early_return(&mut self) {
        let base = self.fb.body_base;
        self.drop_from(base);
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.dead();
    }

    fn if_stmt(&mut self, i: &'a a::IfStmt, line: u32) -> R<()> {
        let mark = self.fb.reg.len();
        let op = self.expr(&i.cond)?;
        self.flush_temps(mark);
        let tb = self.new_block();
        let eb = self.new_block();
        let join = self.new_block();
        self.term(Term::Branch(op, tb, eb));
        self.fb.cur = tb;
        self.block(&i.then)?;
        self.goto(join);
        self.fb.cur = eb;
        match i.otherwise.as_deref() {
            None => {}
            Some(a::Else::Block(b)) => self.block(b)?,
            Some(a::Else::If(j)) => {
                self.fb.line = line;
                self.if_stmt(j, line)?;
            }
        }
        self.goto(join);
        self.fb.cur = join;
        Ok(())
    }

    // ---- match とパターン ----

    fn root_ref(&self, e: &a::Expr) -> Option<bool> {
        match e {
            a::Expr::Paren(i) => self.root_ref(i),
            a::Expr::Field { expr, .. } => self.root_ref(expr),
            a::Expr::Name(n) => match self.lookup(n)?.ty {
                ir::Ty::MutRef(_) => Some(true),
                ir::Ty::Ref(_) => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn match_(
        &mut self,
        scrut: &'a a::Expr,
        arms: Vec<(&'a a::Pattern, Arm<'a>)>,
        res: Option<usize>,
    ) -> R<()> {
        let s = strip(scrut);
        let (place, sty, by_ref) = if let a::Expr::Unary {
            op: op @ (a::UnOp::Ref | a::UnOp::RefMut),
            expr: inner,
        } = s
        {
            let (p, ty) = self.expr_place(inner)?;
            (p, ty, Some(*op == a::UnOp::RefMut))
        } else if let Some((p, ty)) = self.place_expr(s)? {
            let br = match &ty {
                ir::Ty::MutRef(_) => Some(true),
                ir::Ty::Ref(_) => Some(false),
                _ => self.root_ref(s),
            };
            (p, ty, br)
        } else {
            let (p, ty) = self.expr_place(s)?;
            let br = match &ty {
                ir::Ty::MutRef(_) => Some(true),
                ir::Ty::Ref(_) => Some(false),
                _ => None,
            };
            (p, ty, br)
        };
        let join = self.new_block();
        for (pat, arm) in arms {
            let fail = self.new_block();
            self.pat_test(pat, &place, &sty, fail)?;
            let mark = self.enter_scope();
            self.pat_bind(pat, &place, &sty, by_ref)?;
            match arm {
                Arm::B(b) => self.block(b)?,
                Arm::E(e) => {
                    let m2 = self.fb.reg.len();
                    let op = self.expr(e)?;
                    if let Some(r) = res {
                        self.assign(Place::local(r), Rvalue::Use(op));
                    }
                    self.flush_temps(m2);
                }
            }
            self.exit_scope(mark);
            self.goto(join);
            self.fb.cur = fail;
        }
        // どの腕にも合わなかった場合（腕がすべて並んでいれば来ない）。
        self.term(Term::Unreachable);
        self.fb.cur = join;
        Ok(())
    }
}

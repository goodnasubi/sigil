use crate::ast as a;
use crate::ir::{self, Operand, Place, Rvalue, Term};

use super::{bail, is_ref, needs_drop, Bind, Fb, L, R};

fn pat_names(p: &a::Pattern, out: &mut Vec<String>) {
    match p {
        a::Pattern::Bind(n) => out.push(n.clone()),
        a::Pattern::Variant { args, .. } => args.iter().for_each(|x| pat_names(x, out)),
        a::Pattern::Tuple(ps) => ps.iter().for_each(|x| pat_names(x, out)),
        _ => {}
    }
}

// ---- 自由変数（クロージャの捕捉） ----

fn free_expr(e: &a::Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    use a::Expr::*;
    match e {
        Name(n) => {
            if !bound.contains(n) && !out.contains(n) {
                out.push(n.clone());
            }
        }
        Old(i) | Paren(i) | Try(i) => free_expr(i, bound, out),
        Tuple(es) => es.iter().for_each(|x| free_expr(x, bound, out)),
        Unary { expr, .. } => free_expr(expr, bound, out),
        Binary { lhs, rhs, .. } => {
            free_expr(lhs, bound, out);
            free_expr(rhs, bound, out);
        }
        Field { expr, .. } => free_expr(expr, bound, out),
        Call { callee, args } => {
            free_expr(callee, bound, out);
            args.iter().for_each(|x| free_expr(x, bound, out));
        }
        StructLit { fields, .. } => fields.iter().for_each(|(_, x)| free_expr(x, bound, out)),
        Match {
            scrutinee, arms, ..
        } => {
            free_expr(scrutinee, bound, out);
            for arm in arms {
                let n = bound.len();
                pat_names(&arm.pattern, bound);
                free_expr(&arm.body, bound, out);
                bound.truncate(n);
            }
        }
        Closure { params, body, .. } => {
            let n = bound.len();
            bound.extend(params.iter().cloned());
            match body {
                a::ClosureBody::Expr(x) => free_expr(x, bound, out),
                a::ClosureBody::Block(b) => free_block(b, bound, out),
            }
            bound.truncate(n);
        }
        _ => {}
    }
}

fn free_block(b: &a::Block, bound: &mut Vec<String>, out: &mut Vec<String>) {
    let n = bound.len();
    for s in &b.stmts {
        free_stmt(s, bound, out);
    }
    bound.truncate(n);
}

fn free_if(i: &a::IfStmt, bound: &mut Vec<String>, out: &mut Vec<String>) {
    free_expr(&i.cond, bound, out);
    free_block(&i.then, bound, out);
    match i.otherwise.as_deref() {
        Some(a::Else::If(j)) => free_if(j, bound, out),
        Some(a::Else::Block(b)) => free_block(b, bound, out),
        None => {}
    }
}

fn free_stmt(s: &a::Stmt, bound: &mut Vec<String>, out: &mut Vec<String>) {
    match &s.kind {
        a::StmtKind::Let { pattern, value, .. } => {
            free_expr(value, bound, out);
            pat_names(pattern, bound);
        }
        a::StmtKind::Assign { target, value } => {
            free_expr(target, bound, out);
            free_expr(value, bound, out);
        }
        a::StmtKind::Expr(e) => free_expr(e, bound, out),
        a::StmtKind::Return(e) => {
            if let Some(e) = e {
                free_expr(e, bound, out)
            }
        }
        a::StmtKind::If(i) => free_if(i, bound, out),
        a::StmtKind::While { cond, body } => {
            free_expr(cond, bound, out);
            free_block(body, bound, out);
        }
        a::StmtKind::Match {
            scrutinee, arms, ..
        } => {
            free_expr(scrutinee, bound, out);
            for arm in arms {
                let n = bound.len();
                pat_names(&arm.pattern, bound);
                free_block(&arm.body, bound, out);
                bound.truncate(n);
            }
        }
    }
}

impl<'a> L<'a> {
    // ---- クロージャ ----

    pub(super) fn closure(
        &mut self,
        e: &'a a::Expr,
        is_move: bool,
        params: &'a [String],
        body: &'a a::ClosureBody,
    ) -> R<Operand> {
        let fty = self.ety(e)?;
        let ir::Ty::FnRef(pts, rty) = fty.clone() else {
            return bail("クロージャの型");
        };
        let mut bound: Vec<String> = params.to_vec();
        let mut out = vec![];
        match body {
            a::ClosureBody::Expr(x) => free_expr(x, &mut bound, &mut out),
            a::ClosureBody::Block(b) => free_block(b, &mut bound, &mut out),
        }
        let caps: Vec<Bind> = out.iter().filter_map(|n| self.lookup(n)).collect();
        // 捕捉の演算子と、環境の中での型
        let mut ops = vec![];
        let mut cap_tys = vec![];
        let mut by_ref = vec![];
        for c in &caps {
            if is_move {
                ops.push(self.mv(&c.place, &c.ty));
                cap_tys.push(c.ty.clone());
                by_ref.push(false);
            } else if is_ref(&c.ty) {
                ops.push(Operand::Copy(c.place.clone()));
                cap_tys.push(c.ty.clone());
                by_ref.push(false);
            } else {
                let rt = ir::Ty::Ref(Box::new(c.ty.clone()));
                ops.push(self.fresh(Rvalue::Ref(c.place.clone()), rt.clone()));
                cap_tys.push(rt);
                by_ref.push(true);
            }
        }
        let fid = self.funcs.len();
        self.funcs.push(None);
        let nfb = Fb::new(
            &self.fb.name.clone(),
            (*rty).clone(),
            self.fb.mono.clone(),
            self.fb.line,
        );
        let mut nfb = nfb;
        nfb.during = self.fb.during.clone();
        let outer = std::mem::replace(&mut self.fb, nfb);
        let r = self.closure_body(params, &pts, &caps, &cap_tys, &by_ref, body);
        let cfb = std::mem::replace(&mut self.fb, outer);
        r?;
        let f = self.finish(cfb);
        self.funcs[fid] = Some(f);
        Ok(self.fresh(Rvalue::MakeClosure(fid, ops), fty))
    }

    fn closure_body(
        &mut self,
        params: &'a [String],
        pts: &[ir::Ty],
        caps: &[Bind],
        cap_tys: &[ir::Ty],
        by_ref: &[bool],
        body: &'a a::ClosureBody,
    ) -> R<()> {
        self.start();
        let env_ty = if cap_tys.is_empty() {
            ir::Ty::Unit
        } else {
            ir::Ty::Tuple(cap_tys.to_vec())
        };
        let env = self.new_local("_env", ir::Ty::Ref(Box::new(env_ty)));
        self.fb.params.push(env);
        let mut cscope = vec![];
        for (i, c) in caps.iter().enumerate() {
            let mut p = Place::local(env).deref().field(i);
            if by_ref[i] {
                p = p.deref();
            }
            cscope.push(Bind {
                name: c.name.clone(),
                place: p,
                ty: c.ty.clone(),
            });
        }
        let mut pscope = vec![];
        for (i, n) in params.iter().enumerate() {
            let ty = pts.get(i).cloned().unwrap_or(ir::Ty::Unit);
            let l = self.new_local(n, ty.clone());
            self.fb.params.push(l);
            if needs_drop(&ty) {
                self.register(l, false);
            }
            pscope.push(Bind {
                name: n.clone(),
                place: Place::local(l),
                ty,
            });
        }
        self.fb.scopes.push(cscope);
        self.fb.scopes.push(pscope);
        self.fb.body_base = self.fb.reg.len();
        match body {
            a::ClosureBody::Expr(x) => {
                let mark = self.fb.reg.len();
                let op = self.expr(x)?;
                self.assign(Place::local(0), Rvalue::Use(op));
                self.flush_temps(mark);
            }
            a::ClosureBody::Block(b) => self.block(b)?,
        }
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.fb.cur = epi;
        self.drop_from(0);
        self.term(Term::Return);
        Ok(())
    }
}

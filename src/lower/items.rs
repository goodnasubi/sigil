use crate::ast as a;
use crate::exec::{collect_olds, Clause, StopKind};
use crate::ir::{self, AggKind, Place, Rvalue, Term};
use std::collections::HashMap;

use super::{addr, bail, needs_drop, Bind, Fb, Job, L, R};

impl<'a> L<'a> {
    pub(super) fn lower_fn(&mut self, job: Job<'a>) -> R<ir::Func> {
        let decl = job.decl;
        let ret_ty = match &decl.ret {
            Some(t) => self.conv_ast(t, &job.mono)?,
            None => ir::Ty::Unit,
        };
        // 実体名（ADR 0022 決定 1）。トレイトの実装の関数は登録側で付け替えた名前を使う。
        let ename = self
            .names
            .get(&(decl as *const a::FnDecl as usize))
            .cloned()
            .unwrap_or_else(|| decl.name.clone());
        let fb = Fb::new(&ename, ret_ty, job.mono, decl.span.line);
        let saved = std::mem::replace(&mut self.fb, fb);
        let r = self.fn_body(decl);
        let fb = std::mem::replace(&mut self.fb, saved);
        r?;
        Ok(self.finish(fb))
    }

    fn fn_body(&mut self, decl: &'a a::FnDecl) -> R<()> {
        self.start();
        let mono = self.fb.mono.clone();
        let mut pscope = vec![];
        for p in &decl.params {
            let ty = self.conv_ast(&p.ty, &mono)?;
            let l = self.new_local(&p.name, ty.clone());
            self.fb.params.push(l);
            if p.name == "self" {
                // 本体の中の `self`（`Expr::SelfValue`）の場所。メソッドの受け手は先頭の引数。
                self.fb.self_local = Some(l);
            }
            if needs_drop(&ty) {
                self.register(l, false);
            }
            pscope.push(Bind {
                name: p.name.clone(),
                place: Place::local(l),
                ty,
            });
        }
        self.fb.scopes.push(pscope);
        if let Some(r) = &decl.requires {
            self.fb.line = decl.clause_lines[1];
            self.fb.during = Some((decl.name.clone(), Clause::Requires));
            let mark = self.fb.reg.len();
            let op = self.expr(r)?;
            self.flush_temps(mark);
            self.fb.during = None;
            let site = self.site(StopKind::RequiresViolated, "");
            self.assert(op, site);
        }
        let mut olds_exprs = vec![];
        if let Some(en) = &decl.ensures {
            collect_olds(en, &mut olds_exprs);
        }
        if !olds_exprs.is_empty() {
            self.fb.line = decl.clause_lines[2];
            self.fb.during = Some((decl.name.clone(), Clause::Ensures));
            for inner in olds_exprs {
                let mark = self.fb.reg.len();
                let op = self.expr(inner)?;
                let ty = self.ety(inner)?;
                let l = self.new_local("_old", ty.clone());
                self.assign(Place::local(l), Rvalue::Use(op));
                self.flush_temps(mark);
                if needs_drop(&ty) {
                    self.register(l, false);
                }
                self.fb.olds.insert(addr(inner), l);
            }
            self.fb.during = None;
        }
        self.fb.body_base = self.fb.reg.len();
        self.fb.line = decl.body.open_line;
        self.block(&decl.body)?;
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.fb.cur = epi;
        if let Some(en) = &decl.ensures {
            self.fb.line = decl.clause_lines[2];
            self.fb.during = Some((decl.name.clone(), Clause::Ensures));
            self.register(0, false);
            let mark = self.fb.reg.len();
            let op = self.expr(en)?;
            self.flush_temps(mark);
            self.fb.during = None;
            let site = self.site(StopKind::EnsuresViolated, "");
            self.assert(op, site);
            self.fb.reg.pop();
            self.fb.reg_blocks.pop();
        }
        self.drop_from(0);
        self.term(Term::Return);
        Ok(())
    }

    // ---- 篩型の check ----

    pub(super) fn check_fn(&mut self, tname: &str) -> R<usize> {
        if let Some(&f) = self.checks.get(tname) {
            return Ok(f);
        }
        let Some(&td) = self.refined.get(tname) else {
            return bail(format!("篩型 {tname} がない"));
        };
        let base = self.conv_ast(&td.base, &HashMap::new())?;
        let opt = self.adt("Option", vec![base.clone()])?;
        let fid = self.funcs.len();
        self.funcs.push(None);
        self.checks.insert(tname.to_string(), fid);
        let nfb = Fb::new(tname, ir::Ty::Adt(opt), HashMap::new(), td.span.line);
        let outer = std::mem::replace(&mut self.fb, nfb);
        let r = self.check_body(td, &base, opt);
        let cfb = std::mem::replace(&mut self.fb, outer);
        r?;
        let f = self.finish(cfb);
        self.funcs[fid] = Some(f);
        Ok(fid)
    }

    fn check_body(&mut self, td: &'a a::TypeDecl, base: &ir::Ty, opt: usize) -> R<()> {
        self.start();
        let p = self.new_local("self", base.clone());
        self.fb.params.push(p);
        if needs_drop(base) {
            self.register(p, false);
        }
        self.fb.self_local = Some(p);
        self.fb.scopes.push(vec![]);
        self.fb.body_base = self.fb.reg.len();
        let mark = self.fb.reg.len();
        let op = self.expr(&td.cond)?;
        self.flush_temps(mark);
        let yes = self.new_block();
        let no = self.new_block();
        self.term(Term::Branch(op, yes, no));
        let epi = self.fb.epilogue;
        self.fb.cur = yes;
        let mvd = self.mv(&Place::local(p), base);
        self.assign(
            Place::local(0),
            Rvalue::Aggregate(AggKind::Variant(opt, ir::OPTION_SOME), vec![mvd]),
        );
        self.goto(epi);
        self.fb.cur = no;
        self.assign(
            Place::local(0),
            Rvalue::Aggregate(AggKind::Variant(opt, ir::OPTION_NONE), vec![]),
        );
        self.goto(epi);
        self.fb.cur = epi;
        self.drop_from(0);
        self.term(Term::Return);
        Ok(())
    }
}

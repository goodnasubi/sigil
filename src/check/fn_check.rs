use std::collections::HashMap;

use crate::ast::*;
use crate::error::{Error, ErrorKind};

use super::types::{show, Sig, Ty, TypeDef};
use super::util::{block_returns, is_fn_or_fn_list};
use super::{fn_key, Atom, Ck, EffTerm, FnReport, Local, R};

impl Ck {
    pub(super) fn check_refinement(&mut self, t: &TypeDecl) -> R<()> {
        self.span = t.span;
        let Some(TypeDef::Alias(base)) = self.types.get(&t.name).cloned() else {
            return Ok(());
        };
        self.begin();
        self.scopes = vec![HashMap::from([(
            "self".to_string(),
            Local {
                ty: base,
                mutable: false,
                param: true,
            },
        )])];
        self.refine = true;
        let r = self.expect(&Ty::Bool, &t.cond, "篩型の条件");
        self.refine = false;
        r?;
        self.finish()?;
        self.flush_typed();
        Ok(())
    }

    pub(super) fn check_fn(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.env = sig.generics.iter().cloned().collect();
        self.cur_trait_bounds = sig.trait_bounds.clone();
        self.tparams = sig.generics.iter().map(|(n, _)| n.clone()).collect();
        let r = self.check_fn_in(f, sig);
        self.env.clear();
        self.cur_trait_bounds.clear();
        self.tparams.clear();
        self.skip = None;
        r
    }

    fn check_fn_in(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        if f.unchecked.is_some() && !self.base_module {
            return Err(Error::new(
                ErrorKind::UncheckedOutsideTrustBase,
                f.span,
                format!(
                    "関数 `{}` に `unchecked` の節があるが、このモジュールは `trust base` ではない（`unchecked` は `trust base` のモジュールの関数にだけ書ける）",
                    f.name
                ),
            ));
        }
        self.raw_fn = f.unchecked.is_some();
        self.raw_used.clear();
        self.raw_delegated = false;
        let r = self.check_fn_body(f, sig);
        self.raw_fn = false;
        r
    }

    fn check_fn_body(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.skip = None;
        self.annots.clear();
        self.cur_fn = f.name.clone();
        self.body_once(f, sig)?;
        self.flush_typed();
        let mut fin = std::mem::take(&mut self.fin_closures);
        self.closures.append(&mut fin);
        self.span = f.span;
        self.record_report(f, sig);
        self.match_effects(f, sig)?;
        // 型注釈の要否: 注釈を 1 つ外して検査し直し、同じ型に決まれば冗長
        let annots = self.annots.clone();
        for (i, (span, declared)) in annots.iter().enumerate() {
            self.skip = Some(i);
            let again = self.body_once(f, sig);
            if again.is_ok() && self.skipped.as_ref() == Some(declared) {
                return Err(Error::new(
                    ErrorKind::RedundantAnnotation,
                    *span,
                    format!(
                        "型注釈 `{}` を外しても同じ型に決まるので、型注釈を取り除く",
                        show(declared)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 関数の記録（使った原始操作と、宣言だけの効果）を残す。
    fn record_report(&mut self, f: &FnDecl, sig: &Sig) {
        let mut declared: Vec<String> = vec![];
        for t in &sig.effects {
            match t {
                EffTerm::Cat(n) => declared.extend(self.table.atoms(n)),
                EffTerm::Of(i) => declared.push(format!("of({})", sig.params[*i].0)),
            }
        }
        let mut inferred: Vec<String> = vec![];
        for a in &self.inferred {
            match a {
                Atom::Cat(n) => inferred.extend(self.table.atoms(n)),
                Atom::Of(x) => inferred.push(format!("of({x})")),
            }
        }
        let mut not_inferred: Vec<String> = vec![];
        for e in declared {
            if !inferred.contains(&e) && !not_inferred.contains(&e) {
                not_inferred.push(e);
            }
        }
        let raw_ops = self
            .raw_used
            .iter()
            .map(|k| (k.clone(), self.raw_ops[k].premise.clone()))
            .collect();
        self.reports.insert(
            fn_key(f),
            FnReport {
                raw_ops,
                delegated: self.raw_delegated,
                not_inferred,
            },
        );
    }

    fn body_once(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.span = f.span;
        self.begin();
        let mut scope = HashMap::new();
        for (n, t) in &sig.params {
            scope.insert(
                n.clone(),
                Local {
                    ty: t.clone(),
                    mutable: false,
                    param: true,
                },
            );
        }
        self.scopes = vec![scope];
        self.fn_params = sig
            .params
            .iter()
            .filter(|(_, t)| is_fn_or_fn_list(t))
            .map(|(n, _)| n.clone())
            .collect();
        self.fn_borrowed = self.fn_params.iter().cloned().collect();
        self.ret = sig.ret.clone();
        // 契約の式は効果の推論に数えない（捨てる枠で検査する）
        self.eff_stack.push(vec![]);
        self.track = false;
        self.contract = true;
        let mut contract_res: R<()> = Ok(());
        if let Some(r) = &f.requires {
            contract_res = self.expect(&Ty::Bool, r, "`requires`").map(|_| ());
        }
        if let (Ok(()), Some(e)) = (&contract_res, &f.ensures) {
            self.result = Some(sig.ret.clone());
            self.in_ensures = true;
            contract_res = self.expect(&Ty::Bool, e, "`ensures`").map(|_| ());
            self.in_ensures = false;
            self.result = None;
        }
        self.contract = false;
        self.track = true;
        contract_res?;
        self.eff_stack.pop();
        self.block(&f.body)?;
        if !self.flow.dead {
            self.ens_check()?;
        }
        self.inferred = self.eff_stack[0].clone();
        self.span = f.span;
        if sig.ret != Ty::Unit && !block_returns(&f.body) {
            return self.err(
                ErrorKind::MissingReturn,
                format!(
                    "関数 `{}` は `{}` を返すので、すべての道が `return` で終わらなければならない",
                    f.name,
                    show(&sig.ret)
                ),
            );
        }
        self.finish()
    }
}

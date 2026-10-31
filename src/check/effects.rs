use super::types::PROPS;
use super::{
    contains_ref, show, strip_paren, Atom, CapMode, Capture, Ck, ClosureInfo, EffTerm, Flow, Prop,
    Sig, Ty, R,
};
use crate::ast::*;
use crate::error::{Error, ErrorKind};

impl Ck {
    /// `effects` 節の名前を調べる（ADR 0010 決定 12）。
    pub(super) fn effect_terms(
        &self,
        effects: &[Effect],
        params: &[(String, Ty)],
    ) -> R<Vec<EffTerm>> {
        let mut out = vec![];
        for e in effects {
            match e {
                Effect::Category(path) => {
                    let name = path.join(".");
                    if !self.table.valid(&name) {
                        let hint = if name == "browser" {
                            "（`browser` は実行対象の語で、効果ではない。DOM は `dom`、保存領域は `browser.storage`）"
                        } else {
                            ""
                        };
                        return self.err(
                            ErrorKind::UnknownEffect,
                            format!("効果 `{name}` は、効果のカテゴリの表にない{hint}"),
                        );
                    }
                    out.push(EffTerm::Cat(name));
                }
                Effect::Of(x) => {
                    match params
                        .iter()
                        .position(|(n, t)| n == x && matches!(t, Ty::Fn(..)))
                    {
                        Some(i) => out.push(EffTerm::Of(i)),
                        None => {
                            return self.err(
                                ErrorKind::OfNotParam,
                                format!("`of({x})` の `{x}` は、関数の型の引数でなければならない"),
                            )
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    // ---------- 篩型・関数の検査 ----------

    /// 関数 1 つ（または篩型 1 つ）ごとに推論の状態を作り直す。
    pub(super) fn begin(&mut self) {
        self.subst.clear();
        self.pending.clear();
        self.trait_pending.clear();
        self.must_know.clear();
        self.skipped = None;
        self.rec_ty.clear();
        self.rec_calls.clear();
        self.rec_moves.clear();
        self.rec_lets.clear();
        self.call_stack.clear();
        self.annot_n = 0;
        self.scopes.clear();
        self.refine = false;
        self.result = None;
        self.ret = Ty::Unit;
        self.eff_stack = vec![vec![]];
        self.fn_params.clear();
        self.fn_borrowed.clear();
        self.local_eff.clear();
        self.defer = false;
        self.flow = Flow::default();
        self.track = true;
        self.closure_base = 0;
        self.skip_use = false;
        self.pat_moves = false;
        self.res.clear();
        self.log.clear();
        self.used.clear();
        self.sig_checks.clear();
        self.moved_pending.clear();
        self.frames.clear();
        self.closure_arg = false;
        self.cur_closures.clear();
        self.fin_closures.clear();
        self.contract = false;
        self.in_ensures = false;
        self.in_old = false;
        self.ens_used.clear();
        self.let_pending.clear();
    }

    pub(super) fn add_eff(&mut self, a: Atom) {
        let top = self.eff_stack.last_mut().expect("effect frame");
        if !top.contains(&a) {
            top.push(a);
        }
    }

    pub(super) fn has_var(&self, t: &Ty) -> bool {
        match self.shallow(t) {
            Ty::Var(_) => true,
            Ty::Tuple(ts) => ts.iter().any(|t| self.has_var(t)),
            Ty::Ref(_, i) => self.has_var(&i),
            Ty::Named(_, a) => a.iter().any(|t| self.has_var(t)),
            Ty::Fn(ps, r) => ps.iter().any(|t| self.has_var(t)) || self.has_var(&r),
            _ => false,
        }
    }

    /// 本体の終わり: 繰り延べた性質の制約を調べ、決まっていない型変数があれば `CannotInfer`。
    pub(super) fn finish(&mut self) -> R<()> {
        let pend = std::mem::take(&mut self.pending);
        let mut unknown = None;
        for p in &pend {
            match self.holds(&p.ty, p.prop, 0) {
                Some(true) => {}
                Some(false) => {
                    return Err(Error::new(
                        p.kind.clone(),
                        p.span,
                        p.text.replace("{}", &self.sh(&p.ty)),
                    ))
                }
                None => unknown = Some(p.span),
            }
        }
        let tpend = std::mem::take(&mut self.trait_pending);
        for p in &tpend {
            match self.trait_satisfied(&p.trait_name, &p.ty, 0) {
                Some(true) => {}
                Some(false) => {
                    return Err(Error::new(
                        ErrorKind::ConstraintUnsatisfied,
                        p.span,
                        p.text.replace("{}", &self.sh(&p.ty)),
                    ))
                }
                None => unknown = Some(p.span),
            }
        }
        let mk = std::mem::take(&mut self.must_know);
        for (t, what, span) in &mk {
            if self.has_var(t) {
                return Err(Error::new(
                    ErrorKind::CannotInfer,
                    *span,
                    format!(
                        "{what}の型が決まらない（使い方で型が決まるようにするか、型注釈で示す）"
                    ),
                ));
            }
        }
        if let Some(span) = unknown {
            return Err(Error::new(
                ErrorKind::CannotInfer,
                span,
                "型が決まらないので、性質を調べられない",
            ));
        }
        // 型が決まっていない値の移動の後の使用: `Copyable` と分からなければ移動として扱う
        for (t, e) in std::mem::take(&mut self.moved_pending) {
            if self.holds(&self.zonk(&t), Prop::Copyable, 0) != Some(true) {
                return Err(e);
            }
        }
        for (ps, ret, what, span) in std::mem::take(&mut self.sig_checks) {
            if let Some(m) = self.subst_problem(&ps, &ret) {
                return Err(Error::new(
                    ErrorKind::RefSubstitution,
                    span,
                    format!("{what}の型引数に参照を入れた結果が、参照の位置の規則に合わない: {m}"),
                ));
            }
        }
        for (n, t, span) in std::mem::take(&mut self.let_pending) {
            let z = self.zonk(&t);
            if contains_ref(&z) {
                return Err(Error::new(
                    ErrorKind::LetMutRef,
                    span,
                    format!("`{n}` は、型が決まると `{}` で参照を含むので `let mut` で宣言できない（型が参照を含む変数は `let` で宣言する）", show(&z)),
                ));
            }
            if matches!(z, Ty::Fn(..)) {
                return Err(Error::new(
                    ErrorKind::FnValueReassign,
                    span,
                    format!("`{n}` は、型が決まると関数の型になるので `let mut` で宣言できない（`let` で束縛し直す）"),
                ));
            }
        }
        if let Some(t) = self.skipped.take() {
            self.skipped = Some(self.zonk(&t));
        }
        for raw in std::mem::take(&mut self.cur_closures) {
            let mut captures = vec![];
            for (name, ty, mode) in raw.caps {
                let z = self.zonk(&ty);
                // 型が決まらなかったので移動とみなしたものが、決まってみると `Copyable` だった
                let mode = if mode == CapMode::Move
                    && !matches!(z, Ty::Fn(..))
                    && self.holds(&z, Prop::Copyable, 0) == Some(true)
                {
                    CapMode::Copy
                } else {
                    mode
                };
                let seen = match mode {
                    CapMode::Ref => Ty::Ref(false, Box::new(z.clone())),
                    CapMode::RefMut => Ty::Ref(true, Box::new(z.clone())),
                    _ => z.clone(),
                };
                let props = PROPS
                    .iter()
                    .map(|(n, p)| (*n, self.holds(&seen, *p, 0)))
                    .collect();
                captures.push(Capture {
                    name,
                    ty: show(&z),
                    mode,
                    props,
                });
            }
            self.fin_closures.push(ClosureInfo {
                func: self.cur_fn.clone(),
                line: raw.span.line,
                escaping: raw.escaping,
                is_move: raw.is_move,
                bound: raw.bound,
                captures,
            });
        }
        Ok(())
    }

    /// 宣言した効果と、本体から推論した効果の照合（ADR 0004 の規則 3・ADR 0006 の条件 11）。
    pub(super) fn match_effects(&self, f: &FnDecl, sig: &Sig) -> R<()> {
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
        for t in &self.targets {
            for e in declared.iter().chain(&inferred) {
                if !e.starts_with("of(") && !self.table.allowed_on(e, t) {
                    return self.err(
                        ErrorKind::EffectTarget,
                        format!(
                            "効果 `{e}` は、実行対象 `{t}` では使えない（モジュールの `targets` に `{t}` がある）"
                        ),
                    );
                }
            }
        }
        if let Some(e) = inferred.iter().find(|e| !declared.contains(e)) {
            return self.err(
                ErrorKind::EffectMissing,
                format!(
                    "関数 `{}` の本体は効果 `{e}` を起こすが、`effects` 節に書かれていない（足す）",
                    f.name
                ),
            );
        }
        // 宣言に委ねる原始操作を呼ぶ関数は、宣言が推論より多くてよい（ADR 0016 の決定 4）
        if self.raw_delegated {
            return Ok(());
        }
        if let Some(e) = declared.iter().find(|e| !inferred.contains(e)) {
            return self.err(
                ErrorKind::EffectExtra,
                format!(
                    "関数 `{}` の `effects` 節に効果 `{e}` があるが、本体は起こさない（取り除く）",
                    f.name
                ),
            );
        }
        Ok(())
    }

    /// 引数として渡された関数の効果。
    pub(super) fn fn_effects(&self, a: &Expr) -> Vec<Atom> {
        match strip_paren(a) {
            Expr::Name(n) => {
                if self.lookup(n).is_some() {
                    if self.fn_params.contains(n) {
                        vec![Atom::Of(n.clone())]
                    } else {
                        self.local_eff.get(n).cloned().unwrap_or_default()
                    }
                } else {
                    self.fns.get(n).map_or_else(Vec::new, |sig| {
                        sig.effects
                            .iter()
                            .filter_map(|t| match t {
                                EffTerm::Cat(c) => Some(Atom::Cat(c.clone())),
                                // TODO: 関数の型を入れ子にできるようにしたら、
                                // ここで `of(..)` を実引数の効果へ展開する必要がある。
                                EffTerm::Of(_) => None,
                            })
                            .collect()
                    })
                }
            }
            _ => vec![],
        }
    }
}

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::ErrorKind;

use super::types::{head_of, match_pattern, show, substitute, Sig, Ty, TypeDef};
use super::util::{conjuncts, rename_names};
use super::{effects_subset, Ck, Eff, Pos, ResKind, TraitDef, TraitImpl, TraitPending, R};

impl Ck {
    /// トレイトの宣言を登録する（ADR 0022 決定 2）。シグネチャは `Self` を型引数として持つ。
    pub(super) fn register_trait(&mut self, t: &TraitDecl) -> R<()> {
        self.span = t.span;
        if self.traits.contains_key(&t.name) {
            return self.err(
                ErrorKind::DuplicateDefinition,
                format!("トレイト `{}` が二重に宣言されている", t.name),
            );
        }
        let self_ty = Type::Path {
            path: vec!["Self".to_string()],
            args: vec![],
        };
        let mut fns = vec![];
        let mut seen = HashSet::new();
        for f in &t.fns {
            self.span = f.span;
            if !seen.insert(f.name.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("トレイト `{}` に関数 `{}` が二重にある", t.name, f.name),
                );
            }
            // `Self` を型引数として解決する（`Param("Self")` になる）。
            self.tparams = vec!["Self".to_string()];
            let sig = self.signature_in(f, Some((&t.name, &self_ty)), vec![], vec![]);
            self.tparams.clear();
            fns.push((f.clone(), sig?));
        }
        self.traits.insert(t.name.clone(), TraitDef { fns });
        Ok(())
    }

    /// `impl トレイト for 型` の登録（ADR 0022 決定 2 の一貫性の規則）。
    /// `own` のときだけ、置き場所と適合を検査する（取り込んだ実装は、書いたモジュールで検査済み）。
    pub(super) fn register_impl_for(&mut self, i: &ImplForDecl, own: bool) -> R<()> {
        self.span = i.span;
        let (generics, trait_bounds) = self.generics(&i.generics, true)?;
        self.tparams = generics.iter().map(|(n, _)| n.clone()).collect();
        let ity = self.resolve(&i.ty, Pos::Other);
        self.tparams.clear();
        let ity = ity?;
        let Some((head, _)) = head_of(&ity) else {
            return self.err(
                ErrorKind::TraitImplMismatch,
                "型の頭のない実装は書けない（`impl<T> トレイト for T` の形は書けない。型の構築子を持つ型に実装する）",
            );
        };
        // 取り込んだ実装は、トレイト自体を取り込んでいなくても登録する（呼べるかどうかは見える範囲で決まる）。
        let tdef = self.traits.get(&i.trait_name).cloned();
        if own && tdef.is_none() {
            return self.err(
                ErrorKind::UnknownType,
                format!("トレイト `{}` が見つからない", i.trait_name),
            );
        }
        let fp = (
            i.span.line,
            i.span.col,
            i.fns.len(),
            i.fns.first().map(|f| f.name.clone()).unwrap_or_default(),
            i.fns.first().map_or(0, |f| f.span.line),
        );
        if let Some((eidx, _)) = self
            .trait_impls
            .iter()
            .enumerate()
            .find(|(_, x)| x.trait_name == i.trait_name && x.head == head)
        {
            // 同じ実装が複数の `use` の経路で届いた場合は、二重ではなく同じものとして扱う。
            if self.trait_impls[eidx].fp == fp {
                self.impl_index
                    .insert(i as *const ImplForDecl as usize, eidx);
                return Ok(());
            }
            return self.err(
                ErrorKind::TraitImplDuplicate,
                format!(
                    "トレイト `{}` の型 `{head}` への実装はすでに 1 つある（同じ（トレイト, 型の頭）の組の実装は 1 つだけ）",
                    i.trait_name
                ),
            );
        }
        let mut fns = vec![];
        for f in &i.fns {
            self.span = f.span;
            let sig = self.signature(f, Some((&head, &i.ty)), &generics, &trait_bounds)?;
            fns.push((f.clone(), sig));
        }
        if own {
            // 置ける場所: トレイトを宣言したモジュールか、型を宣言したモジュールだけ。
            if !self.own_traits.contains(&i.trait_name) && !self.own_types.contains(&head) {
                self.span = i.span;
                return self.err(
                    ErrorKind::TraitImplPlacement,
                    format!(
                        "`impl {} for {head}` は、トレイト `{}` を宣言したモジュールか、型 `{head}` を宣言したモジュールにだけ書ける",
                        i.trait_name, i.trait_name
                    ),
                );
            }
            self.check_impl_conformance(
                i,
                tdef.as_ref().expect("自分の実装のトレイトは登録済み"),
                &ity,
                &fns,
            )?;
        }
        let idx = self.trait_impls.len();
        self.impl_index
            .insert(i as *const ImplForDecl as usize, idx);
        self.trait_impls.push(TraitImpl {
            trait_name: i.trait_name.clone(),
            head,
            ty: ity,
            generics,
            trait_bounds,
            fns,
            fp,
        });
        Ok(())
    }

    /// 実装がトレイトの宣言に合うか（ADR 0022 決定 2 の 4 節の規則と、固有のメソッドとの名前の重なり）。
    fn check_impl_conformance(
        &mut self,
        i: &ImplForDecl,
        tdef: &TraitDef,
        ity: &Ty,
        fns: &[(FnDecl, Sig)],
    ) -> R<()> {
        let head = head_of(ity).map(|(h, _)| h).unwrap_or_default();
        // 固有のメソッド・フィールドと、トレイトの関数の名前は重ねられない。
        for (f, _) in fns {
            self.span = f.span;
            if self.methods.contains_key(&(head.clone(), f.name.clone())) {
                return self.err(
                    ErrorKind::TraitImplMismatch,
                    format!(
                        "型 `{head}` には固有のメソッド `{}` があり、トレイト `{}` の関数と同じ名前は付けられない",
                        f.name, i.trait_name
                    ),
                );
            }
            if let Some(TypeDef::Struct { fields, .. }) = self.types.get(&head) {
                if fields.iter().any(|(n, _)| *n == f.name) {
                    return self.err(
                        ErrorKind::TraitImplMismatch,
                        format!(
                            "型 `{head}` にはフィールド `{}` があり、トレイト `{}` の関数と同じ名前は付けられない",
                            f.name, i.trait_name
                        ),
                    );
                }
            }
        }
        let subst: HashMap<String, Ty> = HashMap::from([("Self".to_string(), ity.clone())]);
        for (tf, tsig) in &tdef.fns {
            let Some((imf, isig)) = fns.iter().find(|(f, _)| f.name == tf.name) else {
                self.span = i.span;
                return self.err(
                    ErrorKind::TraitImplMismatch,
                    format!(
                        "`impl {} for {}` に関数 `{}` がない（トレイトの関数はすべて実装する）",
                        i.trait_name,
                        show(ity),
                        tf.name
                    ),
                );
            };
            self.check_fn_conformance(i, tf, tsig, imf, isig, &subst)?;
        }
        for (f, _) in fns {
            if !tdef.fns.iter().any(|(tf, _)| tf.name == f.name) {
                self.span = f.span;
                return self.err(
                    ErrorKind::TraitImplMismatch,
                    format!(
                        "`impl {} for {}` の関数 `{}` は、トレイト `{}` にない（余分な関数は書けない）",
                        i.trait_name,
                        show(ity),
                        f.name,
                        i.trait_name
                    ),
                );
            }
        }
        Ok(())
    }

    /// トレイトの関数 1 つと実装の関数の対応（シグネチャと 4 節。ADR 0022 決定 2）。
    fn check_fn_conformance(
        &mut self,
        i: &ImplForDecl,
        tf: &FnDecl,
        tsig: &Sig,
        imf: &FnDecl,
        isig: &Sig,
        subst: &HashMap<String, Ty>,
    ) -> R<()> {
        self.span = imf.span;
        let bad = |ck: &Ck, what: String| {
            ck.err::<()>(
                ErrorKind::TraitImplMismatch,
                format!(
                    "`impl {} for {}` の関数 `{}`: {what}",
                    i.trait_name,
                    show(&subst["Self"]),
                    tf.name
                ),
            )
        };
        if !imf.generics.is_empty() {
            return bad(
                self,
                "トレイトの関数に型引数はないので、実装の関数にも型引数は書けない".into(),
            );
        }
        if tsig.has_self != isig.has_self {
            return bad(self, "受け手（`self`）の有無がトレイトの宣言と違う".into());
        }
        if tsig.params.len() != isig.params.len() {
            return bad(
                self,
                format!(
                    "引数の数が違う（トレイトは {} 個、実装は {} 個）",
                    tsig.params.len(),
                    isig.params.len()
                ),
            );
        }
        for (k, ((_, tt), (_, it))) in tsig.params.iter().zip(isig.params.iter()).enumerate() {
            let want = substitute(tt, subst);
            if want != *it {
                return bad(
                    self,
                    format!(
                        "第 {} 引数の型が違う（トレイトは `{}`、実装は `{}`。`Self` と型引数を置き換えて同じでなければならない）",
                        k + 1,
                        show(&want),
                        show(it)
                    ),
                );
            }
        }
        let want_ret = substitute(&tsig.ret, subst);
        if want_ret != isig.ret {
            return bad(
                self,
                format!(
                    "戻り値の型が違う（トレイトは `{}`、実装は `{}`）",
                    show(&want_ret),
                    show(&isig.ret)
                ),
            );
        }
        // 引数の名前の対応（位置で対応させる）。
        let ren: HashMap<String, String> = tsig
            .params
            .iter()
            .zip(isig.params.iter())
            .map(|((a, _), (b, _))| (a.clone(), b.clone()))
            .collect();
        // requires: `Self` を置き換えたうえでの α 同値でなければならない。
        match (&tf.requires, &imf.requires) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                if rename_names(a, &ren) != *b {
                    return bad(
                        self,
                        "`requires` がトレイトの宣言と違う（同じものを書き直す。引数の名前の違いだけは許される）"
                            .into(),
                    );
                }
            }
            _ => {
                return bad(
                    self,
                    "`requires` がトレイトの宣言と違う（同じものを書き直す）".into(),
                )
            }
        }
        // ensures: トレイトの条件を消すことはできない（足すのはよい）。
        if let Some(te) = &tf.ensures {
            let want = rename_names(te, &ren);
            let ok = match &imf.ensures {
                Some(ie) => {
                    let mut cs = vec![];
                    conjuncts(ie, &mut cs);
                    cs.contains(&want)
                }
                None => false,
            };
            if !ok {
                return bad(
                    self,
                    "`ensures` からトレイトの条件が消えている（条件を足すのはよいが、消すことはできない）"
                        .into(),
                );
            }
        }
        // effects: トレイト以下でなければならない（効果の順序 ⊑。ADR 0022 決定 2）。
        if !effects_subset(&self.table, &isig.effects, &tsig.effects) {
            return bad(
                self,
                "効果がトレイトの宣言を超えている（実装の効果はトレイト以下でなければならない）"
                    .into(),
            );
        }
        Ok(())
    }

    /// 型引数のトレイトの制約を、型が決まってから調べるよう繰り延べる。
    pub(super) fn require_traits(
        &mut self,
        m: &HashMap<String, Ty>,
        tb: &[(String, String)],
        what: &str,
    ) {
        for (p, tr) in tb {
            if let Some(t) = m.get(p) {
                self.trait_pending.push(TraitPending {
                    ty: t.clone(),
                    trait_name: tr.clone(),
                    text: format!(
                        "{what}の型引数 `{p}` はトレイト `{tr}` を実装しなければならないが、`{{}}` は実装していない"
                    ),
                    span: self.span,
                });
            }
        }
    }

    /// 型 `ty` がトレイト `trait_name` を実装するか（`Some(true/false)`。決まらなければ `None`）。
    pub(super) fn trait_satisfied(&self, trait_name: &str, ty: &Ty, depth: u32) -> Option<bool> {
        if depth > 16 {
            return Some(false);
        }
        match self.shallow(ty) {
            Ty::Var(_) => None,
            Ty::Param(p) => Some(
                self.cur_trait_bounds
                    .iter()
                    .any(|(n, t)| *n == p && t == trait_name),
            ),
            t => {
                let z = self.zonk(&t);
                let Some((head, _)) = head_of(&z) else {
                    return Some(false);
                };
                // 実装がなければ「満たさない」（決まらない、ではない）。
                let Some(imp) = self
                    .trait_impls
                    .iter()
                    .find(|x| x.trait_name == trait_name && x.head == head)
                else {
                    return Some(false);
                };
                let Some(m) = match_pattern(self, &imp.ty, &z) else {
                    return Some(false);
                };
                for (g, props) in &imp.generics {
                    let Some(at) = m.get(g) else {
                        return Some(false);
                    };
                    for p in props {
                        match self.holds(at, *p, 0) {
                            Some(true) => {}
                            other => return other,
                        }
                    }
                }
                for (g, tr) in &imp.trait_bounds {
                    let Some(at) = m.get(g) else {
                        return Some(false);
                    };
                    match self.trait_satisfied(tr, at, depth + 1) {
                        Some(true) => {}
                        other => return other,
                    }
                }
                Some(true)
            }
        }
    }

    /// 具体的な型の受け手に対する、トレイトの実装のメソッドのシグネチャ（ADR 0022 決定 2）。
    /// 見えるトレイトの候補が 1 つならそれを返す。実装はあるが見えるトレイトがないときと、
    /// 見えるトレイトが複数あるときはエラーにする。
    pub(super) fn trait_method_sig(&mut self, rt: &Ty, name: &str) -> R<Option<(Sig, String)>> {
        let z = self.zonk(rt);
        let Some((head, _)) = head_of(&z) else {
            return Ok(None);
        };
        let mut cands: Vec<(Sig, String)> = vec![];
        let mut hidden: Vec<String> = vec![];
        for imp in &self.trait_impls {
            if imp.head != head {
                continue;
            }
            let Some((_, sig)) = imp.fns.iter().find(|(f, _)| f.name == name) else {
                continue;
            };
            if self.visible_traits.contains(&imp.trait_name) {
                cands.push((sig.clone(), imp.trait_name.clone()));
            } else if !hidden.contains(&imp.trait_name) {
                hidden.push(imp.trait_name.clone());
            }
        }
        match cands.len() {
            1 => Ok(Some(cands.pop().expect("1 つ"))),
            0 => {
                if hidden.is_empty() {
                    Ok(None)
                } else {
                    self.err(
                        ErrorKind::NoSuchMember,
                        format!(
                            "型 `{head}` のメソッド `{name}` はトレイト `{}` の実装にあるが、このモジュールではそのトレイトが見えていない（`use` で取り込む）",
                            hidden.join("`・`")
                        ),
                    )
                }
            }
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "メソッド `{name}` は見えている複数のトレイト（{list}）の実装にあり、どれを呼ぶか決まらない（`トレイト.{name}(受け手, ...)` と書いて選ぶ）"
                    ),
                )
            }
        }
    }

    /// 型に属する関数としての、トレイトの実装の関数（受け手のないもの。ADR 0022 決定 2）。
    pub(super) fn trait_typefn_sig(&mut self, tn: &str, name: &str) -> R<Option<(Sig, String)>> {
        let mut cands: Vec<(Sig, String)> = vec![];
        let mut hidden: Vec<String> = vec![];
        for imp in &self.trait_impls {
            if imp.head != tn {
                continue;
            }
            let Some((_, sig)) = imp.fns.iter().find(|(f, s)| f.name == name && !s.has_self) else {
                continue;
            };
            if self.visible_traits.contains(&imp.trait_name) {
                cands.push((sig.clone(), imp.trait_name.clone()));
            } else if !hidden.contains(&imp.trait_name) {
                hidden.push(imp.trait_name.clone());
            }
        }
        match cands.len() {
            1 => Ok(Some(cands.pop().expect("1 つ"))),
            0 => {
                if hidden.is_empty() {
                    Ok(None)
                } else {
                    self.err(
                        ErrorKind::NoSuchMember,
                        format!(
                            "型 `{tn}` の関数 `{name}` はトレイト `{}` の実装にあるが、このモジュールではそのトレイトが見えていない（`use` で取り込む）",
                            hidden.join("`・`")
                        ),
                    )
                }
            }
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "関数 `{name}` は見えている複数のトレイト（{list}）の実装にあり、どれを呼ぶか決まらない"
                    ),
                )
            }
        }
    }

    /// 型引数 `p` の制約のトレイトから、受け手のある関数 `name` のシグネチャ（`Self` を `p` に置き換えたもの）。
    pub(super) fn trait_param_method_sig(
        &mut self,
        p: &str,
        name: &str,
    ) -> R<Option<(Sig, String)>> {
        let bounds: Vec<String> = self
            .cur_trait_bounds
            .iter()
            .filter(|(n, _)| n == p)
            .map(|(_, t)| t.clone())
            .collect();
        let mut cands: Vec<(Sig, String)> = vec![];
        for tr in &bounds {
            let Some(tdef) = self.traits.get(tr) else {
                continue;
            };
            if let Some((_, sig)) = tdef.fns.iter().find(|(f, s)| f.name == name && s.has_self) {
                let subst = HashMap::from([("Self".to_string(), Ty::Param(p.to_string()))]);
                let mut s = sig.clone();
                s.params = s
                    .params
                    .iter()
                    .map(|(n, t)| (n.clone(), substitute(t, &subst)))
                    .collect();
                s.ret = substitute(&s.ret, &subst);
                s.generics = vec![];
                s.trait_bounds = vec![];
                cands.push((s, tr.clone()));
            }
        }
        match cands.len() {
            1 => Ok(Some(cands.pop().expect("1 つ"))),
            0 => Ok(None),
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "メソッド `{name}` は制約の複数のトレイト（{list}）にあり、どれを呼ぶか決まらない（`トレイト.{name}(受け手, ...)` と書いて選ぶ）"
                    ),
                )
            }
        }
    }

    /// `T.f(...)`（型引数の、受け手のないトレイトの関数。ADR 0022 決定 2）。
    pub(super) fn trait_param_typefn(
        &mut self,
        callee: &Expr,
        p: &str,
        name: &str,
        args: &[Expr],
    ) -> R<Ty> {
        let bounds: Vec<String> = self
            .cur_trait_bounds
            .iter()
            .filter(|(n, _)| n == p)
            .map(|(_, t)| t.clone())
            .collect();
        let mut cands: Vec<(Sig, String)> = vec![];
        for tr in &bounds {
            let Some(tdef) = self.traits.get(tr) else {
                continue;
            };
            if let Some((_, sig)) = tdef.fns.iter().find(|(f, s)| f.name == name && !s.has_self) {
                cands.push((sig.clone(), tr.clone()));
            }
        }
        let what = format!("`{p}.{name}`");
        let (sig, tr) = match cands.len() {
            1 => cands.pop().expect("1 つ"),
            0 => {
                return self.err(
                    ErrorKind::NoSuchMember,
                    format!(
                        "型引数 `{p}` の制約にあるトレイトに、受け手のない関数 `{name}` がない（`T.{name}(...)` で呼べるのは、制約のトレイトの関数だけ）"
                    ),
                )
            }
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                return self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "関数 `{name}` は制約の複数のトレイト（{list}）にあり、どれを呼ぶか決まらない"
                    ),
                );
            }
        };
        let subst = HashMap::from([("Self".to_string(), Ty::Param(p.to_string()))]);
        let ft = Ty::Fn(
            sig.params
                .iter()
                .map(|(_, t)| substitute(t, &subst))
                .collect(),
            Box::new(substitute(&sig.ret, &subst)),
            None,
        );
        self.set_resolved(
            ResKind::TraitTypeFn,
            Some(&tr),
            name,
            vec![(p.to_string(), Ty::Param(p.to_string()))],
        );
        self.rec_expr(callee, &ft);
        let eff = Eff {
            terms: sig.effects.clone(),
            ..Eff::default()
        };
        self.apply(ft, args, &what, eff)
    }

    /// `トレイト.m(x, ...)`（受け手を最初の引数に渡してトレイトを選ぶ形。ADR 0022 決定 2）。
    pub(super) fn trait_qualified_call(
        &mut self,
        callee: &Expr,
        tr: &str,
        name: &str,
        args: &[Expr],
    ) -> R<Ty> {
        let Some(tdef) = self.traits.get(tr).cloned() else {
            return self.err(
                ErrorKind::UnknownType,
                format!("トレイト `{tr}` が見つからない"),
            );
        };
        let Some((_, tsig)) = tdef.fns.iter().find(|(f, _)| f.name == name) else {
            return self.err(
                ErrorKind::NoSuchMember,
                format!("トレイト `{tr}` に関数 `{name}` がない"),
            );
        };
        if !tsig.has_self {
            return self.err(
                ErrorKind::NotCallable,
                format!(
                    "`{tr}.{name}` は受け手のない関数なので、型に対して `{name}(...)` の形では呼べない（受け手のある関数をトレイトで選ぶ形）"
                ),
            );
        }
        if args.is_empty() {
            return self.err(
                ErrorKind::ArgCount,
                format!("`{tr}.{name}` は受け手を最初の引数に渡す形（`{tr}.{name}(受け手, ...)`）で呼ぶ"),
            );
        }
        let (recv, rest) = args.split_first().expect("1 つ以上");
        let mark = self.log.len();
        let rt0 = self.expr(recv)?;
        let recv_res = std::mem::take(&mut self.res);
        let rt = self.peel(&rt0);
        let (sig, kind) = match &rt {
            Ty::Param(p) => {
                if !self.cur_trait_bounds.iter().any(|(n, t)| n == p && t == tr) {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!(
                            "型引数 `{p}` の制約にトレイト `{tr}` がないので、`{tr}.{name}(...)` の形では呼べない"
                        ),
                    );
                }
                let subst = HashMap::from([("Self".to_string(), Ty::Param(p.clone()))]);
                let mut s = tsig.clone();
                s.params = s
                    .params
                    .iter()
                    .map(|(n, t)| (n.clone(), substitute(t, &subst)))
                    .collect();
                s.ret = substitute(&s.ret, &subst);
                s.generics = vec![];
                s.trait_bounds = vec![];
                (s, ResKind::TraitMethod)
            }
            _ => {
                let z = self.zonk(&rt);
                let Some((head, _)) = head_of(&z) else {
                    return self.err(
                        ErrorKind::TypeMismatch,
                        format!("`{tr}.{name}` の受け手の型が決まらない"),
                    );
                };
                let Some(imp) = self
                    .trait_impls
                    .iter()
                    .find(|x| x.trait_name == tr && x.head == head)
                else {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!("型 `{head}` はトレイト `{tr}` を実装していない"),
                    );
                };
                let Some((_, s)) = imp.fns.iter().find(|(f, _)| f.name == name) else {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!("型 `{head}` のトレイト `{tr}` の実装に関数 `{name}` がない"),
                    );
                };
                (s.clone(), ResKind::TraitMethod)
            }
        };
        let what = format!("メソッド `{tr}.{name}`");
        self.method_tail(
            callee, recv, &rt0, &rt, recv_res, mark, &sig, kind, tr, name, rest, &what,
        )
    }
}

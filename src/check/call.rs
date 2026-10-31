use std::collections::HashMap;

use crate::ast::*;
use crate::error::ErrorKind;

use super::types::{show, substitute, Prop, Sig, Ty, TypeDef};
use super::util::{place_path, place_root, strip_paren};
use super::{AccKind, Atom, Ck, Eff, EffTerm, ResKind, Src, PRELUDE_VALUES, R};

impl Ck {
    /// 置き換えた後のシグネチャの検査を、型が決まってからに回す。
    fn sig_check(&mut self, ps: &[Ty], ret: &Ty, what: &str) {
        if self.track && !self.refine {
            self.sig_checks
                .push((ps.to_vec(), ret.clone(), what.to_string(), self.span));
        }
    }

    /// 関数の型 `ft` の値を `args` で呼ぶ。
    pub(super) fn apply(&mut self, ft: Ty, args: &[Expr], what: &str, eff: Eff) -> R<Ty> {
        match self.shallow(&ft) {
            Ty::Fn(ps, r) => {
                let mark = self.log.len();
                let srcs = self.args(&ps, args, what, &eff)?;
                self.check_overlap(mark)?;
                self.sig_check(&ps, &r, what);
                self.set_res(&r, srcs);
                Ok(*r)
            }
            Ty::Var(_) => self.err(
                ErrorKind::CannotInfer,
                format!("{what}の型が決まっていないので、呼べるか分からない"),
            ),
            other => self.err(
                ErrorKind::NotCallable,
                format!("{what}は `{}` で、呼び出せない", self.sh(&other)),
            ),
        }
    }

    pub(super) fn call(&mut self, callee: &Expr, args: &[Expr]) -> R<Ty> {
        match callee {
            Expr::Name(n) => {
                if self.refine {
                    return match self.fns.get(n).cloned() {
                        Some(sig) if sig.effects_none => {
                            let ft = self.fn_type(&sig, &format!("関数 `{n}`"))?;
                            let targs = std::mem::take(&mut self.last_inst);
                            self.set_resolved(ResKind::Fn, None, n, targs);
                            self.rec_expr(callee, &ft);
                            self.apply(ft, args, &format!("関数 `{n}`"), Eff::default())
                        }
                        Some(_) => self.err(
                            ErrorKind::RefinementEffect,
                            format!(
                                "篩型の条件で呼べるのは `effects none` の関数だけ（`{n}` は違う）"
                            ),
                        ),
                        None => {
                            self.err(ErrorKind::UnknownName, format!("関数 `{n}` が見つからない"))
                        }
                    };
                }
                let known = self.lookup(n).is_some()
                    || self.fns.contains_key(n)
                    || PRELUDE_VALUES.iter().any(|(v, _)| v == n);
                if !known {
                    return self.err(
                        ErrorKind::UnknownName,
                        format!(
                            "関数 `{n}` が見つからない（この版の標準ライブラリにある関数は ADR 0013 の決定 2 の一覧に限る）"
                        ),
                    );
                }
                self.last_inst.clear();
                let ft = self.name(n)?;
                let targs = std::mem::take(&mut self.last_inst);
                let kind = if self.lookup(n).is_some() {
                    ResKind::Local
                } else {
                    ResKind::Fn
                };
                self.set_resolved(kind, None, n, targs.clone());
                self.rec_expr(callee, &ft);
                let eff = if self.lookup(n).is_some() {
                    if self.fn_params.contains(&n.to_string()) {
                        Eff {
                            extra: vec![Atom::Of(n.clone())],
                            ..Eff::default()
                        }
                    } else {
                        Eff {
                            extra: self.local_eff.get(n).cloned().unwrap_or_default(),
                            ..Eff::default()
                        }
                    }
                } else {
                    self.fns.get(n).map_or_else(Eff::default, |sig| Eff {
                        terms: sig.effects.clone(),
                        ..Eff::default()
                    })
                };
                let cspan = self.span;
                let r = self.apply(ft, args, &format!("`{n}`"), eff)?;
                if kind == ResKind::Fn {
                    // 多相再帰の検査は、引数と対応づいて型引数が決まった後に行う。
                    self.span = cspan;
                    let m: HashMap<String, Ty> = targs.iter().cloned().collect();
                    self.poly_check(&m, n)?;
                }
                Ok(r)
            }
            Expr::Field { expr, name } => {
                // 名前空間の関数: `fs.read(p)`・`console.log(x)`。
                if let Expr::Name(ns) = &**expr {
                    let key = format!("{ns}.{name}");
                    if self.lookup(ns).is_none() && self.fns.contains_key(&key) {
                        if let Some(info) = self.raw_ops.get(&key) {
                            let delegated = info.delegated;
                            self.raw_allowed(&key, false)?;
                            self.raw_used.insert(key.clone());
                            self.raw_delegated |= delegated;
                        }
                        if self.core && self.os_items.fns.contains(&key) {
                            return self.os_in_core(&key);
                        }
                        let sig = self.fns[&key].clone();
                        let what = format!("`{key}`");
                        let ft = self.fn_type(&sig, &what)?;
                        let targs = std::mem::take(&mut self.last_inst);
                        self.set_resolved(ResKind::Namespaced, None, &key, targs);
                        self.rec_expr(callee, &ft);
                        let eff = Eff {
                            terms: sig.effects.clone(),
                            ..Eff::default()
                        };
                        return self.apply(ft, args, &what, eff);
                    }
                }
                // 型引数に属する関数: `T.f(...)`（`T` はいまの関数の型引数で、制約のトレイトの
                // 受け手のない関数を呼ぶ。ADR 0022 決定 2）。
                if let Expr::Name(p) = &**expr {
                    if self.lookup(p).is_none()
                        && self.tparams.iter().any(|x| x == p)
                        && !self.types.contains_key(p)
                    {
                        return self.trait_param_typefn(callee, p, name, args);
                    }
                }
                if let Some(tn) = self.type_path(expr) {
                    // `トレイト.m(x, ...)` の形（曖昧なメソッドをトレイトで選ぶ。ADR 0022 決定 2）。
                    if self.traits.contains_key(&tn) {
                        return self.trait_qualified_call(callee, &tn, name, args);
                    }
                    if self.core && self.os_item_type_fn(&tn, name) {
                        return self.os_in_core(&format!("{tn}.{name}"));
                    }
                    match self.types.get(&tn).cloned() {
                        Some(TypeDef::Enum { variants, .. })
                            if variants.iter().any(|(v, _)| v == name) =>
                        {
                            self.last_inst.clear();
                            let ft = self.variant_value(&tn, name)?;
                            let targs = std::mem::take(&mut self.last_inst);
                            self.set_resolved(ResKind::Variant, Some(&tn), name, targs);
                            self.rec_expr(callee, &ft);
                            return self.apply(ft, args, &format!("`{tn}.{name}`"), Eff::default());
                        }
                        Some(TypeDef::Alias(base)) if name == "check" => {
                            let ft = self.check_fn_type(&tn, &base);
                            self.set_resolved(ResKind::Check, Some(&tn), name, vec![]);
                            self.rec_expr(callee, &ft);
                            return self.apply(ft, args, &format!("`{tn}.check`"), Eff::default());
                        }
                        Some(TypeDef::Opaque(_))
                            if !self.methods.contains_key(&(tn.clone(), name.clone())) =>
                        {
                            return self.err(
                                ErrorKind::NoSuchMember,
                                format!("標準の型 `{tn}` に `{name}` という関数・値がない"),
                            );
                        }
                        _ => {}
                    }
                    let inherent = self.methods.get(&(tn.clone(), name.clone())).cloned();
                    let (sig, kind, owner) = match inherent {
                        Some(s) => (s, ResKind::TypeFn, tn.clone()),
                        None => match self.trait_typefn_sig(&tn, name)? {
                            Some((s, tr)) => (s, ResKind::TraitTypeFn, tr),
                            None => {
                                return self.err(
                                    ErrorKind::NoSuchMember,
                                    format!("`{tn}` に `{name}` という関数・値がない"),
                                )
                            }
                        },
                    };
                    if sig.has_self {
                        return self.err(
                            ErrorKind::NotCallable,
                            format!("`{tn}.{name}` は受け手のあるメソッドなので、値に対して `x.{name}(...)` と呼ぶ"),
                        );
                    }
                    let what = format!("`{tn}.{name}`");
                    let ft = self.fn_type(&sig, &what)?;
                    let targs = std::mem::take(&mut self.last_inst);
                    let m: HashMap<String, Ty> = targs.iter().cloned().collect();
                    self.poly_check(&m, name)?;
                    self.set_resolved(kind, Some(&owner), name, targs);
                    self.rec_expr(callee, &ft);
                    let eff = Eff {
                        terms: sig.effects.clone(),
                        ..Eff::default()
                    };
                    return self.apply(ft, args, &what, eff);
                }
                // メソッド呼び出し x.m(...)
                let mark = self.log.len();
                let rt0 = self.expr(expr)?;
                let recv_res = std::mem::take(&mut self.res);
                let rt = self.peel(&rt0);
                let clone_sig = if name == "clone" {
                    Some(self.clone_sig(&rt)?)
                } else {
                    None
                };
                // 型引数の受け手は、制約のトレイトの関数としてだけ呼べる（ADR 0022 決定 2）。
                // 組み込みの `clone` はこの限りでない（従来どおり型引数でも使える）。
                let mut preset: Option<(Sig, ResKind, String)> = None;
                if let Ty::Param(p) = &rt {
                    if clone_sig.is_none() {
                        match self.trait_param_method_sig(p, name)? {
                            Some((s, tr)) => preset = Some((s, ResKind::TraitMethod, tr)),
                            None => {
                                return self.err(
                                    ErrorKind::NoSuchMember,
                                    format!("型引数 `{p}` にはメソッド `{name}` がない（性質の制約で使えるのは演算子だけ。トレイトの関数は、型引数の制約にそのトレイトを書いたときだけ呼べる）"),
                                )
                            }
                        }
                    }
                }
                let (tn, _) = match &rt {
                    _ if clone_sig.is_some() => (self.sh(&rt), vec![]),
                    Ty::Named(tn, a) => (tn.clone(), a.clone()),
                    Ty::Str => ("String".to_string(), vec![]),
                    Ty::Var(_) => {
                        return self.err(
                            ErrorKind::CannotInfer,
                            format!(
                                "メソッド `{name}` を呼ぶ式の型が決まっていない（先に型を決める）"
                            ),
                        )
                    }
                    Ty::Param(p) => (p.clone(), vec![]),
                    other => {
                        return self.unsupported(&format!(
                            "標準ライブラリの型 `{}` のメソッド `{name}`",
                            show(&self.zonk(other))
                        ))
                    }
                };
                if self.core
                    && clone_sig.is_none()
                    && preset.is_none()
                    && self.os_item_type_fn(&tn, name)
                {
                    return self.os_in_core(&format!("{tn}.{name}"));
                }
                let (sig, res_kind, res_owner) = match preset {
                    Some(x) => x,
                    None => match clone_sig
                        .or_else(|| self.methods.get(&(tn.clone(), name.clone())).cloned())
                    {
                        Some(s) => (s, ResKind::Method, tn.clone()),
                        None => match self.trait_method_sig(&rt, name)? {
                            Some((s, tr)) => (s, ResKind::TraitMethod, tr),
                            None => {
                                return self.err(
                                    ErrorKind::NoSuchMember,
                                    format!("型 `{tn}` にメソッド `{name}` がない"),
                                )
                            }
                        },
                    },
                };
                if !sig.has_self {
                    return self.err(
                        ErrorKind::NotCallable,
                        format!("`{name}` は受け手のない関数なので、`{tn}.{name}(...)` と呼ぶ"),
                    );
                }
                let what = format!("メソッド `{tn}.{name}`");
                self.method_tail(
                    callee, expr, &rt0, &rt, recv_res, mark, &sig, res_kind, &res_owner, name,
                    args, &what,
                )
            }
            _ => self.err(
                ErrorKind::NotCallable,
                "呼び出せるのは関数・メソッド・値の構築子だけ",
            ),
        }
    }

    /// メソッド呼び出しの末尾（シグネチャが決まった後。固有・トレイト共通）。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn method_tail(
        &mut self,
        callee: &Expr,
        recv: &Expr,
        rt0: &Ty,
        rt: &Ty,
        recv_res: Vec<Src>,
        mark: usize,
        sig: &Sig,
        kind: ResKind,
        owner: &str,
        name: &str,
        args: &[Expr],
        what: &str,
    ) -> R<Ty> {
        let m = self.instantiate(&sig.generics, what)?;
        self.require_traits(&m, &sig.trait_bounds, what);
        let targs = std::mem::take(&mut self.last_inst);
        self.poly_check(&m, name)?;
        self.set_resolved(kind, Some(owner), name, targs);
        let selfty = self.peel(&substitute(&sig.params[0].1, &m));
        if !self.try_unify(&selfty, rt) {
            return self.err(
                ErrorKind::TypeMismatch,
                format!(
                    "{what}の受け手は `{}` だが、`{}` である",
                    self.sh(&selfty),
                    self.sh(rt)
                ),
            );
        }
        if let Ty::Ref(true, _) = sig.params[0].1 {
            let Some(root) = place_root(recv).map(str::to_string) else {
                return self.err(
                    ErrorKind::BorrowOfNonPlace,
                    format!("`{name}` は `&mut` の受け手を取るので、変数に対して呼ぶ"),
                );
            };
            self.assignable_place(&root, recv, false)?;
        }
        let mut srcs: Vec<Src> = vec![];
        if let Ty::Ref(m, _) = sig.params[0].1 {
            // 借りる受け手
            let kind = if m { AccKind::Mut } else { AccKind::Shared };
            let place = place_path(strip_paren(recv)).filter(|(r, _)| self.lookup(r).is_some());
            if let Some((_, path)) = &place {
                self.log.truncate(mark);
                self.log_acc(path, kind);
                if m {
                    self.touch(
                        path,
                        true,
                        &format!("`&mut` を受け取るメソッド `{name}` の呼び出し"),
                    );
                }
            }
            srcs = self.borrow_srcs(recv, m, &recv_res, rt0);
        } else {
            // 値で受け取る受け手（`opt.map(..)` など）は移動する
            srcs.extend(recv_res);
            self.consume(recv, rt0)?;
        }
        let ps: Vec<Ty> = sig.params[1..]
            .iter()
            .map(|(_, t)| substitute(t, &m))
            .collect();
        let eff = Eff {
            terms: sig.effects.clone(),
            off: 1,
            ..Eff::default()
        };
        let more = self.args(&ps, args, what, &eff)?;
        srcs.extend(more);
        self.check_overlap(mark)?;
        let ret = substitute(&sig.ret, &m);
        self.rec_expr(callee, &Ty::Fn(ps.clone(), Box::new(ret.clone())));
        let mut all: Vec<Ty> = vec![substitute(&sig.params[0].1, &m)];
        all.extend(ps.iter().cloned());
        self.sig_check(&all, &ret, what);
        self.set_res(&ret, srcs);
        Ok(ret)
    }

    /// 組み込みの `clone(self: &T) -> T` のシグネチャ（ADR 0013 の決定 2）。受け手の型ごとに作る。
    fn clone_sig(&self, rt: &Ty) -> R<Sig> {
        let shown = self.sh(rt);
        match self.shallow(rt) {
            Ty::Var(_) => {
                return self.err(
                    ErrorKind::CannotInfer,
                    "`clone` を呼ぶ式の型が決まっていない（先に型を決める）",
                )
            }
            Ty::Fn(..) => {
                return self.err(
                    ErrorKind::NoSuchMember,
                    "関数の型には `clone` がない（関数の値は引数に渡しても移動しないので、別名を作らずに元の名前を使う）",
                )
            }
            _ => {}
        }
        if self.holds(rt, Prop::Copyable, 0) == Some(true) {
            return self.err(
                ErrorKind::CloneOfCopyable,
                format!("`{shown}` は `Copyable` なので `clone` は要らない（値として読む位置で複製される）"),
            );
        }
        Ok(Sig {
            generics: vec![],
            trait_bounds: vec![],
            params: vec![("self".into(), Ty::Ref(false, Box::new(rt.clone())))],
            ret: rt.clone(),
            effects_none: true,
            effects: vec![],
            has_self: true,
        })
    }

    fn args(&mut self, params: &[Ty], args: &[Expr], what: &str, eff: &Eff) -> R<Vec<Src>> {
        if params.len() != args.len() {
            return self.err(
                ErrorKind::ArgCount,
                format!(
                    "{what}の引数は {} 個だが、{} 個渡している",
                    params.len(),
                    args.len()
                ),
            );
        }
        let sp = self.span;
        let mut gained: Vec<Atom> = vec![];
        let mut srcs: Vec<Src> = vec![];
        for (i, (p, a)) in params.iter().zip(args).enumerate() {
            let closure = matches!(strip_paren(a), Expr::Closure { .. });
            self.defer = closure;
            self.closure_arg = closure;
            self.ref_ok = matches!(
                a,
                Expr::Unary {
                    op: UnOp::Ref | UnOp::RefMut,
                    ..
                }
            ) && matches!(self.shallow(p), Ty::Ref(..) | Ty::Var(_));
            let t = self.expr_hint(a, Some(p));
            self.defer = false;
            self.closure_arg = false;
            self.ref_ok = false;
            let t = t?;
            if !closure {
                srcs.append(&mut self.res);
            }
            self.res.clear();
            self.span = sp;
            self.fn_value_form(a, &t)?;
            // 値で受け取る引数は移動する（参照の引数は `&x` と書くので場所ではない）
            // 関数の型の引数へ渡す関数の値は、呼び出しの間だけの共有借用（ADR 0013 の決定 3）
            let fn_arg = matches!(self.shallow(p), Ty::Fn(..))
                && matches!(self.zonk(&t), Ty::Fn(..))
                && matches!(strip_paren(a), Expr::Name(n) if self.lookup(n).is_some());
            if fn_arg {
                if let Expr::Name(n) = strip_paren(a) {
                    self.log_acc(n, AccKind::Shared);
                }
            } else if !matches!(self.shallow(p), Ty::Ref(..)) {
                self.consume(a, &t)?;
            }
            let of_this = eff
                .terms
                .iter()
                .any(|t| matches!(t, EffTerm::Of(j) if *j == i + eff.off));
            if closure {
                let ce = std::mem::take(&mut self.last_closure);
                if of_this {
                    gained.extend(ce);
                }
            } else if of_this {
                gained.extend(self.fn_effects(a));
            }
            if !self.coerce(p, &t, a) {
                let hint = if matches!(self.shallow(p), Ty::Ref(..))
                    && !matches!(self.shallow(&t), Ty::Ref(..))
                {
                    "（借用は呼び出しの位置で `&x`・`&mut x` と書く）"
                } else {
                    self.check_hint(p, &t)
                };
                return self.err(
                    self.mismatch_kind(p, &t),
                    format!(
                        "{what}の第 {} 引数は `{}` でなければならないが、`{}` である{hint}",
                        i + 1,
                        self.sh(p),
                        self.sh(&t)
                    ),
                );
            }
        }
        for t in &eff.terms {
            if let EffTerm::Cat(c) = t {
                self.add_eff(Atom::Cat(c.clone()));
            }
        }
        for a in eff.extra.iter().cloned().chain(gained) {
            self.add_eff(a);
        }
        Ok(srcs)
    }
}

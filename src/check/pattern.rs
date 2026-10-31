use std::collections::HashMap;

use super::{
    contains_ref, place_path, place_root, strip_paren, substitute, Ck, Prop, Ty, TypeDef,
    PRELUDE_VALUES, R,
};
use crate::ast::*;
use crate::error::ErrorKind;
use crate::exhaustive::{missing, useful, Ctor, P};

impl Ck {
    // ---------- パターンと網羅性 ----------

    /// パターンの値の名前（`Some`・`Shape.Circle` など）から、列挙型の名前と値の名前。
    fn variant_of(&self, path: &[String]) -> R<(String, String)> {
        match path {
            [v] => match PRELUDE_VALUES.iter().find(|(n, _)| n == v) {
                Some((n, t)) => Ok((t.to_string(), n.to_string())),
                None => self.err(
                    ErrorKind::UnknownName,
                    format!("値 `{v}` が見つからない（列挙型の値は `型.値` と書く）"),
                ),
            },
            [e, v] => match self.types.get(e) {
                Some(TypeDef::Enum { variants, .. }) => {
                    if variants.iter().any(|(n, _)| n == v) {
                        Ok((e.clone(), v.clone()))
                    } else {
                        self.err(
                            ErrorKind::NoSuchMember,
                            format!("列挙型 `{e}` に値 `{v}` がない"),
                        )
                    }
                }
                _ => self.err(
                    ErrorKind::UnknownType,
                    format!("列挙型 `{e}` が見つからない"),
                ),
            },
            _ => self.err(
                ErrorKind::UnknownName,
                format!("値 `{}` が見つからない", path.join(".")),
            ),
        }
    }

    fn pat_mismatch<T>(&self, p: &str, t: &Ty) -> R<T> {
        self.err(
            ErrorKind::PatternMismatch,
            format!("{p}のパターンだが、調べる式の型は `{}` である", self.sh(t)),
        )
    }

    /// `match` の対象が場所で、腕が `Copyable` でない部分を値で束縛するなら、対象はこの `match` で
    /// 移動する（どの腕に入るかによらない）。対象が参照（の型・`&場所`）なら束縛が参照になり移動しない。
    pub(super) fn match_moves<'a>(
        &mut self,
        scrutinee: &Expr,
        st: &Ty,
        by_ref: bool,
        pats: impl Iterator<Item = &'a Pattern>,
    ) -> R<()> {
        if by_ref || !self.track || self.refine {
            return Ok(());
        }
        let Some((root, path)) = place_path(strip_paren(scrutinee)) else {
            return Ok(());
        };
        if self.lookup(&root).is_none() {
            return Ok(());
        }
        // 腕の束縛の型を調べるために、パターンを捨てるスコープで一度合わせる
        self.pat_moves = false;
        let sp = self.span;
        for p in pats {
            self.scopes.push(HashMap::new());
            let r = self.pat(p, st);
            self.scopes.pop();
            self.sync_borrowed();
            r?;
        }
        self.span = sp;
        if std::mem::take(&mut self.pat_moves) {
            self.record_move(&root, &path)?;
        }
        Ok(())
    }

    /// パターンを型 `t` に合わせ、束縛する名前を現在のスコープに宣言する。
    /// `match` の対象の型と、腕の束縛が参照になるか（ADR 0012 の決定 1 の 3）。
    /// `match &x` は所有する場所だけに書ける。参照の名前（`Option<&T>` の変数を含む）と、
    /// それを通して届く場所は `match x` と書く。`&mut` は対象にできない。
    pub(super) fn scrutinee(&mut self, e: &Expr) -> R<(Ty, bool)> {
        if let Expr::Unary { op, expr } = e {
            if matches!(op, UnOp::Ref | UnOp::RefMut) {
                if *op == UnOp::RefMut {
                    return self.err(
                        ErrorKind::MatchBorrowForm,
                        "`match` の対象に `&mut` は書けない（`&` で借りるか、`match x` と書く）",
                    );
                }
                let Some(root) = place_root(expr).map(str::to_string) else {
                    // 一時的な値の借用（文の終わりまで）
                    let t = self.expr(expr)?;
                    let inner_res = std::mem::take(&mut self.res);
                    self.res = self.borrow_srcs(expr, false, &inner_res, &t);
                    return Ok((t, true));
                };
                let t = self.expr(expr)?;
                let inner_res = std::mem::take(&mut self.res);
                if self
                    .lookup(&root)
                    .is_some_and(|l| contains_ref(&self.zonk(&l.ty)))
                {
                    return self.err(
                        ErrorKind::MatchBorrowForm,
                        format!(
                            "`{root}` は参照（または参照を含む値）なので、`&` を外して `match` の対象に直接書く（`match {root}`）。腕の束縛は参照になる"
                        ),
                    );
                }
                self.res = self.borrow_srcs(expr, false, &inner_res, &t);
                return Ok((t, true));
            }
        }
        let t = self.expr(e)?;
        // 参照を通して届く場所（`r`・`r.f`）の束縛も参照になる（ADR 0011 の決定 4）
        let via_ref = place_path(strip_paren(e)).is_some_and(|(r, _)| {
            self.lookup(&r)
                .is_some_and(|l| matches!(self.shallow(&l.ty), Ty::Ref(..)))
        });
        let by_ref = via_ref || contains_ref(&self.zonk(&t));
        Ok((t, by_ref))
    }

    pub(super) fn pat(&mut self, p: &Pattern, t: &Ty) -> R<()> {
        // 中身が `&mut T` なら、束縛は `&mut T` のまま持つ（`get_mut` の結果の `Some(a)` で書き換える）
        let exclusive = matches!(self.shallow(t), Ty::Ref(true, _));
        let t = self.peel(t);
        match p {
            Pattern::Wild => Ok(()),
            Pattern::Bind(n) => {
                let t = if self.by_ref {
                    Ty::Ref(exclusive, Box::new(t))
                } else {
                    if self.holds(&t, Prop::Copyable, 0) == Some(false) {
                        self.pat_moves = true;
                    }
                    t
                };
                self.declare(n, t, false)
            }
            Pattern::Int(s) | Pattern::NegInt(s) => {
                let text = if matches!(p, Pattern::NegInt(_)) {
                    format!("-{s}")
                } else {
                    s.clone()
                };
                if text.parse::<i64>().is_err() {
                    return self.err(
                        ErrorKind::IntLiteralRange,
                        format!("整数のリテラル `{text}` が `Int` の範囲を超えている"),
                    );
                }
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Int) {
                    return self.pat_mismatch("整数", &t);
                }
                Ok(())
            }
            Pattern::Str(_) => {
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Str) {
                    return self.pat_mismatch("文字列", &t);
                }
                Ok(())
            }
            Pattern::Bool(_) => {
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Bool) {
                    return self.pat_mismatch("`Bool`", &t);
                }
                Ok(())
            }
            Pattern::Tuple(ps) => {
                let ts = match self.shallow(&t) {
                    Ty::Tuple(ts) => ts,
                    Ty::Var(_) => {
                        let ts: Vec<Ty> = ps.iter().map(|_| self.fresh()).collect();
                        let _ = self.unify(&t, &Ty::Tuple(ts.clone()));
                        ts
                    }
                    other => return self.pat_mismatch("組", &other),
                };
                if ps.len() != ts.len() {
                    return self.err(
                        ErrorKind::PatternMismatch,
                        format!(
                            "{} 個の組のパターンだが、組の要素は {} 個ある",
                            ps.len(),
                            ts.len()
                        ),
                    );
                }
                for (p, t) in ps.iter().zip(&ts) {
                    self.pat(p, t)?;
                }
                Ok(())
            }
            Pattern::Variant { path, args } => {
                let (en, vn) = self.variant_of(path)?;
                let (ty, m) = self.fresh_named(&en);
                if !self.try_unify(&t, &ty) {
                    return self.pat_mismatch(&format!("`{en}`"), &t);
                }
                let Some(TypeDef::Enum { variants, .. }) = self.types.get(&en).cloned() else {
                    return Ok(());
                };
                let (_, fs) = variants.iter().find(|(n, _)| *n == vn).expect("variant");
                if fs.len() != args.len() {
                    return self.err(
                        ErrorKind::PatternMismatch,
                        format!(
                            "値 `{en}.{vn}` の中身は {} 個だが、パターンは {} 個ある",
                            fs.len(),
                            args.len()
                        ),
                    );
                }
                for (a, f) in args.iter().zip(fs) {
                    self.pat(a, &substitute(f, &m))?;
                }
                Ok(())
            }
        }
    }

    fn to_p(&self, p: &Pattern) -> P {
        match p {
            Pattern::Wild | Pattern::Bind(_) => P::Wild,
            Pattern::Int(s) => P::Ctor(Ctor::Lit(s.clone()), vec![]),
            Pattern::NegInt(s) => P::Ctor(Ctor::Lit(format!("-{s}")), vec![]),
            Pattern::Str(s) => P::Ctor(Ctor::Lit(format!("{s:?}")), vec![]),
            Pattern::Bool(b) => P::Ctor(Ctor::Bool(*b), vec![]),
            Pattern::Tuple(ps) => P::Ctor(
                Ctor::Tuple(ps.len()),
                ps.iter().map(|p| self.to_p(p)).collect(),
            ),
            Pattern::Variant { path, args } => match self.variant_of(path) {
                Ok((ty, name)) => P::Ctor(
                    Ctor::Variant { ty, name },
                    args.iter().map(|p| self.to_p(p)).collect(),
                ),
                Err(_) => P::Wild,
            },
        }
    }

    fn enum_shapes(&self, ty: &str) -> Vec<(String, usize)> {
        match self.types.get(ty) {
            Some(TypeDef::Enum { variants, .. }) => variants
                .iter()
                .map(|(n, fs)| (n.clone(), fs.len()))
                .collect(),
            _ => vec![],
        }
    }

    /// 前の腕がすべて覆う値にしか当たらない腕はエラー（ADR 0011 の決定 9 の追記）。
    fn unreachable_arms(&self, pats: &[&Pattern]) -> R<()> {
        let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![self.to_p(p)]).collect();
        let enums = |ty: &str| self.enum_shapes(ty);
        for i in 1..rows.len() {
            if !useful(&rows[..i], &rows[i], &enums) {
                return self.err(
                    ErrorKind::UnreachableArm,
                    format!(
                        "`match` の第 {} 腕は、前の腕がすでに覆っているので到達できない（取り除く）",
                        i + 1
                    ),
                );
            }
        }
        Ok(())
    }

    /// `match` の腕がすべての値を覆うか（ADR 0011 の決定 9）。
    pub(super) fn exhaustive(&self, pats: &[&Pattern]) -> R<()> {
        self.unreachable_arms(pats)?;
        let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![self.to_p(p)]).collect();
        let enums = |ty: &str| self.enum_shapes(ty);
        match missing(&rows, 1, &enums) {
            None => Ok(()),
            Some(w) => self.err(
                ErrorKind::NotExhaustive,
                format!(
                    "`match` が値をすべて覆っていない: `{}` の場合がない（腕を足すか、`_` を最後に置く）",
                    w.first().cloned().unwrap_or_default()
                ),
            ),
        }
    }
}

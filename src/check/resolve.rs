use std::collections::HashSet;

use super::{Ck, Local, Pos, Ty, TypeDef, R};
use crate::ast::*;
use crate::error::ErrorKind;

impl Ck {
    pub(super) fn resolve(&self, t: &Type, pos: Pos) -> R<Ty> {
        match t {
            Type::Path { path, args } => {
                if path.len() != 1 {
                    return self.err(
                        ErrorKind::UnknownType,
                        format!("型 `{}` が見つからない", path.join(".")),
                    );
                }
                let n = path[0].as_str();
                if self.core
                    && !self.tparams.iter().any(|p| p == n)
                    && self.os_items.types.contains(n)
                {
                    return self.os_in_core(n);
                }
                let arity = if self.tparams.iter().any(|p| p == n) {
                    0
                } else {
                    match n {
                        "Int" | "Float" | "Bool" | "String" | "Unit" => 0,
                        _ => match self.types.get(n) {
                            Some(TypeDef::Alias(_)) => 0,
                            Some(TypeDef::Struct { params, .. } | TypeDef::Enum { params, .. }) => {
                                params.len()
                            }
                            Some(TypeDef::Opaque(k)) => *k,
                            None => {
                                return self.err(
                                    ErrorKind::UnknownType,
                                    format!("型 `{n}` が見つからない（この版の標準ライブラリにある型は ADR 0013 の決定 2 の一覧に限る）"),
                                )
                            }
                        },
                    }
                };
                if args.len() != arity {
                    return self.err(
                        ErrorKind::TypeArgCount,
                        format!(
                            "型 `{n}` の型引数は {arity} 個だが、{} 個書かれている",
                            args.len()
                        ),
                    );
                }
                if self.tparams.iter().any(|p| p == n) {
                    return Ok(Ty::Param(n.to_string()));
                }
                Ok(match n {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "Bool" => Ty::Bool,
                    "String" => Ty::Str,
                    "Unit" => Ty::Unit,
                    _ => {
                        let apos = if pos == Pos::Ret && n == "Option" {
                            Pos::RetOpt
                        } else {
                            Pos::Other
                        };
                        Ty::Named(
                            n.to_string(),
                            args.iter()
                                .map(|a| self.resolve(a, apos))
                                .collect::<R<_>>()?,
                        )
                    }
                })
            }
            Type::Ref { mutable, inner } => {
                if pos == Pos::Other || pos == Pos::Placed {
                    return self.err(
                        ErrorKind::RefTypePosition,
                        "参照 `&T`・`&mut T` を書けるのは、引数の型の一番外、関数の型の引数の一番外、戻り値の型の一番外と一番外の `Option<…>` の中だけ（構造体のフィールド・列挙子の値・型引数・`let` の注釈には書けない。ADR 0012 の決定 1）",
                    );
                }
                Ok(Ty::Ref(
                    *mutable,
                    Box::new(self.resolve(inner, Pos::Other)?),
                ))
            }
            Type::Tuple(ts) => Ok(Ty::Tuple(
                ts.iter()
                    .map(|t| self.resolve(t, Pos::Other))
                    .collect::<R<_>>()?,
            )),
            Type::Fn {
                params,
                ret,
                effects,
            } => {
                // 置ける位置（フィールド・列挙型の値の中身・戻り値・`let` の型注釈）では
                // 効果節が必須で、宣言の効果を型に持つ。引数の位置では効果節は書けない。
                // それ以外の位置（型引数・組の中・関数の型の引数など）には置けない
                // （ADR 0022 決定 3）。
                let slot = match pos {
                    Pos::Param => {
                        if effects.is_some() {
                            return self.err(
                                ErrorKind::FnTypePosition,
                                "引数の位置の関数の型には効果節は書けない（渡された関数の効果は、呼び出し側の宣言の `of(...)` で表す。ADR 0022 決定 3）",
                            );
                        }
                        None
                    }
                    Pos::Placed | Pos::Ret => {
                        let Some(effects) = effects else {
                            return self.err(
                                ErrorKind::FnTypePosition,
                                "フィールド・列挙型の値の中身・戻り値・`let` の型注釈に置く関数の型には、効果節（`effects ...`、効果なしなら `effects none`）が必須である（ADR 0022 決定 3）",
                            );
                        };
                        Some(self.fn_type_terms(effects)?)
                    }
                    _ => {
                        return self.err(
                            ErrorKind::FnTypePosition,
                            "関数の型 `fn(...)` は、関数・メソッドの引数の型の一番外と、効果節つきでフィールド・列挙型の値の中身・戻り値・`let` の型注釈にだけ書ける（型引数・組・Option・List の中や、関数の型の引数には書けない。ADR 0022 決定 3）",
                        );
                    }
                };
                let ps = params
                    .iter()
                    .map(|t| self.resolve(t, Pos::FnParam))
                    .collect::<R<_>>()?;
                let r = match ret {
                    None => Ty::Unit,
                    Some(r) => self.resolve(r, Pos::Other)?,
                };
                Ok(Ty::Fn(ps, Box::new(r), slot))
            }
        }
    }

    pub(super) fn contains_bad_base(&self, t: &Ty) -> bool {
        match t {
            Ty::Ref(..) | Ty::Fn(..) => true,
            Ty::Named(n, a) => {
                matches!(self.types.get(n), Some(TypeDef::Alias(_)))
                    || a.iter().any(|t| self.contains_bad_base(t))
            }
            Ty::Tuple(ts) => ts.iter().any(|t| self.contains_bad_base(t)),
            _ => false,
        }
    }

    /// `from` の定義が（組・構造体・列挙型・篩型をたどって）`target` を直接含むか。標準の入れ物
    /// （`List` など）は中身を別の場所に持つので、含むとは数えない。
    /// 有限の値を作り始められる型の名前（輪を通らない腕・フィールドの組があるもの）。
    /// 空集合から、増えなくなるまで広げる。
    pub(super) fn well_founded(&self) -> HashSet<String> {
        fn ok(t: &Ty, set: &HashSet<String>, ck: &Ck) -> bool {
            match t {
                Ty::Named(n, _) => match ck.types.get(n) {
                    Some(TypeDef::Opaque(_)) | None => true,
                    _ => set.contains(n),
                },
                Ty::Tuple(ts) => ts.iter().all(|t| ok(t, set, ck)),
                _ => true,
            }
        }
        let mut set: HashSet<String> = HashSet::new();
        loop {
            let mut grew = false;
            for (n, d) in &self.types {
                if set.contains(n) {
                    continue;
                }
                let good = match d {
                    TypeDef::Alias(b) => ok(b, &set, self),
                    TypeDef::Struct { fields, .. } => fields.iter().all(|(_, t)| ok(t, &set, self)),
                    TypeDef::Enum { variants, .. } => variants
                        .iter()
                        .any(|(_, ts)| ts.iter().all(|t| ok(t, &set, self))),
                    TypeDef::Opaque(_) => false,
                };
                if good {
                    set.insert(n.clone());
                    grew = true;
                }
            }
            if !grew {
                return set;
            }
        }
    }

    pub(super) fn reaches(&self, from: &str, target: &str, seen: &mut HashSet<String>) -> bool {
        let tys: Vec<Ty> = match self.types.get(from) {
            Some(TypeDef::Alias(b)) => vec![b.clone()],
            Some(TypeDef::Struct { fields, .. }) => fields.iter().map(|(_, t)| t.clone()).collect(),
            Some(TypeDef::Enum { variants, .. }) => variants
                .iter()
                .flat_map(|(_, ts)| ts.iter().cloned())
                .collect(),
            _ => return false,
        };
        fn names(ck: &Ck, t: &Ty, out: &mut Vec<String>) {
            match t {
                Ty::Named(n, a) => {
                    if !matches!(ck.types.get(n), Some(TypeDef::Opaque(_))) {
                        out.push(n.clone());
                        a.iter().for_each(|t| names(ck, t, out));
                    }
                }
                Ty::Tuple(ts) => ts.iter().for_each(|t| names(ck, t, out)),
                _ => {}
            }
        }
        let mut ns = vec![];
        tys.iter().for_each(|t| names(self, t, &mut ns));
        for n in ns {
            if n == target {
                return true;
            }
            if seen.insert(n.clone()) && self.reaches(&n, target, seen) {
                return true;
            }
        }
        false
    }

    pub(super) fn lookup(&self, n: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }

    /// 参照を外して、型変数は解決できるところまで解決した型。
    pub(super) fn peel(&self, t: &Ty) -> Ty {
        match self.shallow(t) {
            Ty::Ref(_, i) => self.peel(&i),
            t => t,
        }
    }

    /// `T.名前`（`T` が型）のとき、その型の名前。
    pub(super) fn type_path(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Name(n)
                if self.lookup(n).is_none() && (self.types.contains_key(n) || n == "Int") =>
            {
                Some(n.clone())
            }
            _ => None,
        }
    }
}

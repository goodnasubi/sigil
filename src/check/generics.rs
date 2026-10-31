use std::collections::HashMap;

use super::types::PROPS;
use super::{args_named, Ck, Prop, Ty, TypeDef, R};
use crate::ast::*;
use crate::error::ErrorKind;

impl Ck {
    /// 総称の型を具体化する: 型引数ごとに新しい型変数を作り、制約を繰り延べの調べに加える。
    pub(super) fn instantiate(
        &mut self,
        generics: &[(String, Vec<Prop>)],
        what: &str,
    ) -> R<HashMap<String, Ty>> {
        let mut m = HashMap::new();
        for (n, props) in generics {
            let v = self.fresh_known(format!("{what}の型引数 `{n}`"));
            for p in props {
                self.require(
                    &v,
                    *p,
                    ErrorKind::ConstraintUnsatisfied,
                    format!(
                        "{what}の型引数 `{n}` は `{}` でなければならないが、`{{}}` は持たない",
                        p.name()
                    ),
                )?;
            }
            m.insert(n.clone(), v);
        }
        self.last_inst = generics
            .iter()
            .map(|(n, _)| (n.clone(), m[n].clone()))
            .collect();
        Ok(m)
    }

    /// 型引数に新しい型変数を入れた、構造体・列挙型の型。
    pub(super) fn fresh_named(&mut self, name: &str) -> (Ty, HashMap<String, Ty>) {
        let params = match self.types.get(name) {
            Some(TypeDef::Struct { params, .. } | TypeDef::Enum { params, .. }) => params.clone(),
            _ => vec![],
        };
        let mut m = HashMap::new();
        let mut args = vec![];
        for p in params {
            let v = self.fresh_known(format!("`{name}` の型引数 `{p}`"));
            args.push(v.clone());
            m.insert(p, v);
        }
        self.last_inst = args_named(self.types.get(name), &m);
        (Ty::Named(name.to_string(), args), m)
    }

    // ---------- 宣言の収集 ----------

    /// 総称の宣言の型引数と制約。`bounded` が偽なら制約を書けない（構造体・列挙型）。
    /// 戻り値は（性質の制約, トレイトの制約）。制約に書けるのは性質と、この検査で見えるトレイト
    /// （ADR 0022 決定 2。性質の名前は予約名なので、トレイトの名前と重ならない）。
    #[allow(clippy::type_complexity)]
    pub(super) fn generics(
        &self,
        gs: &[Generic],
        bounded: bool,
    ) -> R<(Vec<(String, Vec<Prop>)>, Vec<(String, String)>)> {
        let mut out: Vec<(String, Vec<Prop>)> = vec![];
        let mut tbounds: Vec<(String, String)> = vec![];
        for g in gs {
            if out.iter().any(|(n, _)| *n == g.name) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{}` が二重にある", g.name),
                );
            }
            if self.types.contains_key(&g.name) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{}` が同じ名前の型を隠している", g.name),
                );
            }
            if !bounded && !g.bounds.is_empty() {
                return self.err(
                    ErrorKind::TypeParamBound,
                    format!(
                        "構造体・列挙型の型引数 `{}` には制約を書けない（制約は関数と `impl` の型引数にだけ書ける）",
                        g.name
                    ),
                );
            }
            let mut props = vec![];
            for b in &g.bounds {
                let name = b.join(".");
                if let Some((_, p)) = PROPS.iter().find(|(n, _)| *n == name) {
                    if !props.contains(p) {
                        props.push(*p);
                    }
                } else if b.len() == 1 && self.trait_names.contains(&name) {
                    let tb = (g.name.clone(), name);
                    if !tbounds.contains(&tb) {
                        tbounds.push(tb);
                    }
                } else {
                    return self.err(
                        ErrorKind::UnknownProperty,
                        format!(
                            "性質 `{name}` はない（使えるのは {} と、このモジュールで見えるトレイト）",
                            PROPS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join("・")
                        ),
                    );
                }
            }
            out.push((g.name.clone(), props));
        }
        Ok((out, tbounds))
    }
}

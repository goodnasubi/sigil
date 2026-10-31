use std::collections::HashSet;

use crate::ast::*;
use crate::error::{Error, ErrorKind};
use crate::parser::parse;

use super::types::{show, Prop, Sig, Ty, TypeDef};
use super::util::{contains_ref, contains_ref_outside_fn};
use super::{Ck, Pos, RawInfo, PRELUDE_TYPES, R};

impl Ck {
    pub(super) fn prelude(&mut self) {
        let t = || Ty::Param("T".into());
        let e = || Ty::Param("E".into());
        self.types.insert(
            "Option".into(),
            TypeDef::Enum {
                params: vec!["T".into()],
                variants: vec![("Some".into(), vec![t()]), ("None".into(), vec![])],
            },
        );
        self.types.insert(
            "Result".into(),
            TypeDef::Enum {
                params: vec!["T".into(), "E".into()],
                variants: vec![("Ok".into(), vec![t()]), ("Err".into(), vec![e()])],
            },
        );
        for (n, k, layer) in crate::stdlib::OPAQUE_TYPES {
            self.types.insert((*n).into(), TypeDef::Opaque(*k));
            if *layer == crate::stdlib::Layer::Os {
                self.os_items.types.insert((*n).into());
            }
        }
    }

    /// `impl` の型の名前（`impl<T> List<T>` なら `List`）。標準ライブラリの表の鍵に使う。
    fn impl_type_name(&self, i: &ImplDecl) -> String {
        match &i.ty {
            Type::Path { path, .. } => path.join("."),
            _ => String::new(),
        }
    }

    /// 型 `tn` の `name`（型に付く関数またはメソッド）が OS 層か。型そのものが OS 層でもそう扱う。
    pub(super) fn os_item_type_fn(&self, tn: &str, name: &str) -> bool {
        self.os_items.types.contains(tn)
            || self
                .os_items
                .methods
                .contains(&(tn.to_string(), name.to_string()))
    }

    /// 核のモジュールで OS 層の項目を名指ししたときのエラー（ADR 0014 の決定 4）。
    pub(super) fn os_in_core<T>(&self, name: &str) -> R<T> {
        self.err(
            ErrorKind::OsItemInCore,
            format!("`{name}` は標準ライブラリの OS 層（std.os）にあり、`targets` に `bare` を持つモジュールからは使えない（このモジュールを OS なしで使わないなら `targets` から `bare` を外す）"),
        )
    }

    pub(super) fn register_impl(&mut self, i: &ImplDecl, std: bool) -> R<()> {
        self.span = i.span;
        let (outer, outer_tb) = self.generics(&i.generics, true)?;
        self.tparams = outer.iter().map(|(n, _)| n.clone()).collect();
        let ity = self.resolve(&i.ty, Pos::Other);
        self.tparams.clear();
        let tn = match ity? {
            Ty::Named(tn, _) => tn,
            Ty::Str if std => "String".to_string(),
            Ty::Int if std => "Int".to_string(),
            _ => {
                return self.err(
                    ErrorKind::TypeMismatch,
                    "`impl` は構造体・列挙型・篩型に付ける",
                )
            }
        };
        if !std && PRELUDE_TYPES.contains(&tn.as_str()) {
            return self.err(
                ErrorKind::TypeMismatch,
                format!("`impl` は型を宣言したモジュールの中にだけ書ける（標準の型 `{tn}` には付けられない）"),
            );
        }
        for f in &i.fns {
            self.span = f.span;
            let key = (tn.clone(), f.name.clone());
            if self.methods.contains_key(&key) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("`{tn}` のメソッド `{}` が二重に宣言されている", f.name),
                );
            }
            if let Some(TypeDef::Struct { fields, .. }) = self.types.get(&tn) {
                if fields.iter().any(|(n, _)| *n == f.name) {
                    return self.err(
                        ErrorKind::MemberConflict,
                        format!(
                            "`{tn}` のフィールドとメソッドに同じ名前 `{}` は付けられない",
                            f.name
                        ),
                    );
                }
            }
            let sig = self.signature(f, Some((&tn, &i.ty)), &outer, &outer_tb)?;
            self.methods.insert(key, sig);
        }
        Ok(())
    }

    /// 関数のシグネチャ。`recv` は `impl` の中のとき（型の名前と、`impl` の型）。`outer` は `impl` の型引数。
    pub(super) fn signature(
        &mut self,
        f: &FnDecl,
        recv: Option<(&str, &Type)>,
        outer: &[(String, Vec<Prop>)],
        outer_tb: &[(String, String)],
    ) -> R<Sig> {
        let (own, own_tb) = self.generics(&f.generics, true)?;
        let mut generics = outer.to_vec();
        for (n, ps) in own {
            if generics.iter().any(|(m, _)| *m == n) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{n}` が `impl` の型引数と重なっている"),
                );
            }
            generics.push((n, ps));
        }
        let mut trait_bounds = outer_tb.to_vec();
        for tb in own_tb {
            if trait_bounds.iter().any(|(m, _)| *m == tb.0) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{}` が `impl` の型引数と重なっている", tb.0),
                );
            }
            trait_bounds.push(tb);
        }
        self.tparams = generics.iter().map(|(n, _)| n.clone()).collect();
        let r = self.signature_in(f, recv, generics, trait_bounds);
        self.tparams.clear();
        r
    }

    pub(super) fn signature_in(
        &mut self,
        f: &FnDecl,
        recv: Option<(&str, &Type)>,
        generics: Vec<(String, Vec<Prop>)>,
        trait_bounds: Vec<(String, String)>,
    ) -> R<Sig> {
        let mut params = vec![];
        let mut seen = HashSet::new();
        for p in &f.params {
            if !seen.insert(p.name.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("引数 `{}` が二重にある", p.name),
                );
            }
            params.push((p.name.clone(), self.resolve(&p.ty, Pos::Param)?));
        }
        let has_self = params.first().is_some_and(|(n, _)| n == "self");
        if has_self {
            let Some((tn, ity)) = recv else {
                return self.err(
                    ErrorKind::TypeMismatch,
                    "`self` を受け手に書けるのは `impl` の中のメソッドだけ",
                );
            };
            let want = self.resolve(ity, Pos::Other)?;
            let inner = match &params[0].1 {
                Ty::Ref(_, i) => (**i).clone(),
                t => t.clone(),
            };
            if inner != want {
                return self.err(
                    ErrorKind::TypeMismatch,
                    format!(
                        "受け手 `self` の型は `{w}`・`&{w}`・`&mut {w}` のどれかでなければならない（`{}` は違う。型 `{tn}`）",
                        show(&params[0].1),
                        w = show(&want),
                    ),
                );
            }
        }
        let ret = match &f.ret {
            None => Ty::Unit,
            Some(t) => self.resolve(t, Pos::Ret)?,
        };
        if contains_ref(&ret) {
            let n = params
                .iter()
                .filter(|(_, t)| contains_ref_outside_fn(t))
                .count();
            if n != 1 {
                return self.err(
                    ErrorKind::RefReturnBorrowCount,
                    format!(
                        "参照を返せるのは、借用の引数（型のどこかに参照を含む引数。受け手を含む）がちょうど 1 つの関数だけだが、{n} 個ある（ADR 0003 の規則 3）"
                    ),
                );
            }
        }
        let effects = self.effect_terms(&f.effects, &params)?;
        Ok(Sig {
            generics,
            trait_bounds,
            params,
            ret,
            effects_none: f.effects.is_empty(),
            effects,
            has_self,
        })
    }

    /// 標準ライブラリの最小部分（`stdlib.rs` の表）を関数とメソッドの表に入れる。
    pub(super) fn register_std(&mut self) -> R<()> {
        let saved = self.span;
        // 表のシグネチャは OS 層の型を名指しする。核の検査（`OsItemInCore`）は利用者のコードにだけかける。
        let core = std::mem::replace(&mut self.core, false);
        for (layer, sig, effects) in crate::stdlib::FREE_FNS {
            let (name, s) = self.std_sig(&crate::stdlib::free_source(*layer, sig, effects))?;
            self.fns.insert(name, s);
        }
        for (layer, ns, sig, effects) in crate::stdlib::NAMESPACED_FNS {
            let (name, s) = self.std_sig(&crate::stdlib::free_source(*layer, sig, effects))?;
            let key = format!("{ns}.{name}");
            if *layer == crate::stdlib::Layer::Os {
                self.os_items.fns.insert(key.clone());
            }
            self.fns.insert(key, s);
        }
        for (layer, head, fns) in crate::stdlib::IMPLS {
            let file = self.std_parse(&crate::stdlib::impl_source(*layer, head, fns))?;
            if let Some(Item::Impl(i)) = file.items.first() {
                self.register_impl(i, true)?;
                if *layer == crate::stdlib::Layer::Os {
                    let tn = self.impl_type_name(i);
                    for f in &i.fns {
                        self.os_items.methods.insert((tn.clone(), f.name.clone()));
                    }
                }
            }
        }
        self.core = core;
        self.span = saved;
        Ok(())
    }

    /// 原始操作（`raw.名前`）を関数の表に入れる（ADR 0016 の決定 1）。呼べるかどうかは呼ぶ場所で決める。
    pub(super) fn register_raw(&mut self) -> R<()> {
        let saved = self.span;
        let core = std::mem::replace(&mut self.core, false);
        for d in self.raw_defs.clone() {
            if d.premise.trim().is_empty() {
                return self.err(
                    ErrorKind::Unsupported,
                    format!(
                        "原始操作 `{}` の前提の文が空（空にできない。ADR 0016 の決定 1）",
                        d.signature
                    ),
                );
            }
            let src = crate::stdlib::raw_source(d.layer, &d.signature, &d.effects, &d.requires);
            let (name, s) = self.std_sig(&src)?;
            let key = format!("raw.{name}");
            if self.fns.contains_key(&key) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("原始操作 `{key}` が二重にある"),
                );
            }
            if d.layer == crate::stdlib::Layer::Os {
                self.os_items.fns.insert(key.clone());
            }
            self.raw_ops.insert(
                key.clone(),
                RawInfo {
                    premise: d.premise.clone(),
                    delegated: d.effects == crate::stdlib::DELEGATED,
                },
            );
            self.fns.insert(key, s);
        }
        self.core = core;
        self.span = saved;
        Ok(())
    }

    /// 原始操作 `key` をいまの場所で使えるか（ADR 0016 の決定 2）。使えなければ `RawOutsideUnchecked`。
    pub(super) fn raw_allowed(&self, key: &str, as_value: bool) -> R<()> {
        let why = if as_value {
            "原始操作は関数の値として使えない（`unchecked` の関数の本体で直接呼ぶ）"
        } else if self.refine || self.contract {
            "契約の式（`requires`・`ensures`・篩型の条件）の中では原始操作を呼べない"
        } else if self.closure_depth > 0 {
            "クロージャの本体の中では原始操作を呼べない（クロージャは関数の外へ持ち出されうる）"
        } else if !self.raw_fn {
            "原始操作は `unchecked` の節を持つ関数の本体でだけ呼べる"
        } else {
            return Ok(());
        };
        self.err(ErrorKind::RawOutsideUnchecked, format!("`{key}`: {why}"))
    }

    fn std_parse(&self, src: &str) -> R<File> {
        crate::parser::parse_std(src).map_err(|e| {
            Error::new(
                ErrorKind::Unsupported,
                self.span,
                format!(
                    "標準ライブラリの表が読めない（処理系の誤り）: {}\n{src}",
                    e.message
                ),
            )
        })
    }

    fn std_sig(&mut self, src: &str) -> R<(String, Sig)> {
        let file = self.std_parse(src)?;
        let Some(Item::Fn(f)) = file.items.first() else {
            return self.err(ErrorKind::Unsupported, "標準ライブラリの表が読めない");
        };
        let sig = self.signature(f, None, &[], &[])?;
        Ok((f.name.clone(), sig))
    }

    /// 仮の関数（標準ライブラリが入るまでのテスト用）を関数の表に入れる。
    pub(super) fn register_primitives(&mut self) -> R<()> {
        for p in self.prims.clone() {
            let src = format!(
                "module p.p\n  targets server\n  trust none\n\nfn {}\n  intent \"p\"\n  requires none\n  ensures none\n  effects none\n{{\n}}\n",
                p.signature
            );
            let file = parse(&src)?;
            let Some(Item::Fn(f)) = file.items.first() else {
                continue;
            };
            if self.fns.contains_key(&f.name) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("仮の関数 `{}` が二重にある", f.name),
                );
            }
            let mut sig = self.signature(f, None, &[], &[])?;
            let fx: Vec<Effect> = p
                .effects
                .iter()
                .map(|e| Effect::Category(e.split('.').map(str::to_string).collect()))
                .collect();
            sig.effects = self.effect_terms(&fx, &sig.params)?;
            sig.effects_none = fx.is_empty();
            self.fns.insert(f.name.clone(), sig);
        }
        Ok(())
    }
}

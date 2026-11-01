use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::ErrorKind;

use super::types::{contains_param, show, Prop, Ty, TypeDef};
use super::util::collect_calls_block;
use super::{std_heads, Ck, Pos, PRELUDE_TYPES, PRELUDE_VALUES, R};

impl Ck {
    pub(super) fn run(&mut self, file: &File) -> R<()> {
        // 0. モジュールの宣言（ADR 0016 の決定 3・6 の補足。名前の予約が許可より先）
        self.span = file.module.span;
        let mname = file.module.name.join(".");
        if file
            .module
            .name
            .first()
            .is_some_and(|h| std_heads().contains(&h.as_str()))
        {
            return self.err(
                ErrorKind::ReservedModuleName,
                format!("モジュール名 `{mname}` は標準ライブラリの名前（`std`・`raw`・`fs`・`console` で始まる名前は利用者は使えない）"),
            );
        }
        // 公開のシグネチャが非公開の型を名指ししない（ADR 0021 の決定 4）
        if let Some((span, msg)) = crate::project::private_in_public(file) {
            self.span = span;
            return self.err(ErrorKind::PrivateInPublic, msg);
        }
        let externs = self.externs.clone();
        let all: Vec<&Item> = externs.iter().chain(file.items.iter()).collect();
        if self.base_module && !self.allowed_base.contains(&mname) {
            return self.err(
                ErrorKind::TrustBaseNotAllowed,
                format!("モジュール `{mname}` は `trust base` を宣言しているが、許可がない（検査・実行・組み立ての呼び出しに `--trust-base {mname}` を付ける。許可は人間が決める）"),
            );
        }
        // 1. 型の名前と型引数
        let mut names: HashSet<String> = HashSet::new();
        let mut shells: Vec<(String, TypeDef)> = vec![];
        for item in &all {
            let (name, span, generics, shell) = match item {
                Item::Type(t) => (Some(&t.name), t.span, &[][..], TypeDef::Alias(Ty::Unit)),
                Item::Struct(s) => (
                    Some(&s.name),
                    s.span,
                    &s.generics[..],
                    TypeDef::Struct {
                        params: vec![],
                        fields: vec![],
                    },
                ),
                Item::Enum(e) => (
                    Some(&e.name),
                    e.span,
                    &e.generics[..],
                    TypeDef::Enum {
                        params: vec![],
                        variants: vec![],
                    },
                ),
                Item::Fn(f) => (None, f.span, &f.generics[..], TypeDef::Alias(Ty::Unit)),
                Item::Impl(i) => (None, i.span, &i.generics[..], TypeDef::Alias(Ty::Unit)),
                // トレイトは上の 0' で止まるのでここには来ない。型の同一性としては名前を持つ（ADR 0022 の決定 1）。
                Item::Trait(t) => (Some(&t.name), t.span, &[][..], TypeDef::Alias(Ty::Unit)),
                Item::ImplFor(i) => (None, i.span, &i.generics[..], TypeDef::Alias(Ty::Unit)),
                Item::Global(g) => (None, g.span, &[][..], TypeDef::Alias(Ty::Unit)),
                Item::Properties(p) => (None, p.span, &[][..], TypeDef::Alias(Ty::Unit)),
            };
            self.span = span;
            let Some(n) = name else { continue };
            if PRELUDE_TYPES.contains(&n.as_str()) || PRELUDE_VALUES.iter().any(|(v, _)| v == n) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("`{n}` は標準の前置きの名前なので、型の名前に使えない"),
                );
            }
            if !names.insert(n.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型 `{n}` が二重に宣言されている"),
                );
            }
            let (gs, _) = self.generics(generics, false)?;
            let params: Vec<String> = gs.into_iter().map(|(n, _)| n).collect();
            let shell = match shell {
                TypeDef::Struct { fields, .. } => TypeDef::Struct { params, fields },
                TypeDef::Enum { variants, .. } => TypeDef::Enum { params, variants },
                other => other,
            };
            shells.push((n.clone(), shell));
        }
        // 型の名前を先に仮登録（互いに参照できるように）
        for (n, d) in shells {
            self.types.insert(n, d);
        }
        // 2. 型の中身
        for item in &all {
            match item {
                Item::Type(t) => {
                    self.span = t.span;
                    let base = self.resolve(&t.base, Pos::Base)?;
                    if self.contains_bad_base(&base) {
                        return self.err(
                            ErrorKind::RefinementBase,
                            format!(
                                "篩型 `{}` の基の型 `{}` は、篩型・参照・関数の型を含まない型でなければならない",
                                t.name,
                                show(&base)
                            ),
                        );
                    }
                    self.types.insert(t.name.clone(), TypeDef::Alias(base));
                }
                Item::Struct(s) => {
                    self.span = s.span;
                    self.tparams = s.generics.iter().map(|g| g.name.clone()).collect();
                    let mut fields = vec![];
                    let mut seen = HashSet::new();
                    for f in &s.fields {
                        if !seen.insert(f.name.clone()) {
                            return self.err(
                                ErrorKind::DuplicateDefinition,
                                format!("フィールド `{}` が二重にある", f.name),
                            );
                        }
                        fields.push((f.name.clone(), self.resolve(&f.ty, Pos::Placed)?));
                    }
                    self.tparams.clear();
                    let params = s.generics.iter().map(|g| g.name.clone()).collect();
                    self.types
                        .insert(s.name.clone(), TypeDef::Struct { params, fields });
                }
                Item::Enum(e) => {
                    self.span = e.span;
                    self.tparams = e.generics.iter().map(|g| g.name.clone()).collect();
                    let mut variants = vec![];
                    let mut seen = HashSet::new();
                    for v in &e.variants {
                        if !seen.insert(v.name.clone()) {
                            return self.err(
                                ErrorKind::DuplicateDefinition,
                                format!("値 `{}` が二重にある", v.name),
                            );
                        }
                        let mut ts = vec![];
                        for t in &v.fields {
                            ts.push(self.resolve(t, Pos::Placed)?);
                        }
                        variants.push((v.name.clone(), ts));
                    }
                    self.tparams.clear();
                    let params = e.generics.iter().map(|g| g.name.clone()).collect();
                    self.types
                        .insert(e.name.clone(), TypeDef::Enum { params, variants });
                }
                _ => {}
            }
        }
        // 3. 再帰する型（ADR 0013 補遺。輪を通らない腕のある列挙型だけ書ける）
        let founded = self.well_founded();
        for item in &file.items {
            let (name, span) = match item {
                Item::Struct(s) => (&s.name, s.span),
                Item::Enum(e) => (&e.name, e.span),
                _ => continue,
            };
            self.span = span;
            if !founded.contains(name) && self.reaches(name, name, &mut HashSet::new()) {
                return self.err(
                    ErrorKind::RecursiveType,
                    format!(
                        "型 `{name}` が自分自身を含んでいて、値を作り始められない（再帰する型は、輪を通らない腕を持つ列挙型を経由して書く）"
                    ),
                );
            }
        }
        // 4. 関数とメソッドのシグネチャ
        self.register_std()?;
        self.register_raw()?;
        self.register_primitives()?;
        for item in &all {
            match item {
                Item::Fn(f) => {
                    self.span = f.span;
                    if self.fns.contains_key(&f.name) {
                        return self.err(
                            ErrorKind::DuplicateDefinition,
                            format!("関数 `{}` が二重に宣言されている", f.name),
                        );
                    }
                    let sig = self.signature(f, None, &[], &[])?;
                    self.fns.insert(f.name.clone(), sig);
                }
                Item::Global(g) => {
                    self.span = g.span;
                    if !self.base_module {
                        return self.err(
                            ErrorKind::GlobalOutsideTrustBase,
                            format!(
                                "大域変数 `{}` が宣言されているが、このモジュールは `trust base` ではない（大域状態は `trust base` のモジュールでのみ宣言できる。ADR 0025 の決定 2）",
                                g.name
                            ),
                        );
                    }
                    if self.globals.contains_key(&g.name) {
                        return self.err(
                            ErrorKind::DuplicateDefinition,
                            format!("大域変数 `{}` が二重に宣言されている", g.name),
                        );
                    }
                    let ty = self.resolve(&g.ty, Pos::Placed)?;
                    self.globals.insert(g.name.clone(), (ty, g.is_mut));
                }
                Item::Properties(p) => {
                    self.span = p.span;
                    if !self.base_module {
                        return self.err(
                            ErrorKind::PropertiesOutsideTrustBase,
                            format!(
                                "性質の手付け `properties {}` が宣言されているが、このモジュールは `trust base` ではない（性質の手付けは `trust base` のモジュールでのみ宣言できる。ADR 0026 の決定 2・ADR 0016 の決定 7）",
                                p.type_name
                            ),
                        );
                    }
                    // 対象の型はこのモジュールで宣言された型であること
                    if !names.contains(&p.type_name)
                        && !PRELUDE_TYPES.contains(&p.type_name.as_str())
                    {
                        return self.err(
                            ErrorKind::UnknownType,
                            format!(
                                "性質の手付けの対象の型 `{}` がこのモジュールで見つからない（手付けは対象の型を宣言したモジュールの中でのみ書ける。ADR 0026 の決定 2）",
                                p.type_name
                            ),
                        );
                    }
                    for prop_name in &p.properties {
                        let prop = match prop_name.as_str() {
                            "Sendable" => Prop::Sendable,
                            "Shareable" => Prop::Shareable,
                            other => {
                                return self.err(
                                    ErrorKind::UnknownProperty,
                                    format!("手付けできる性質は `Sendable` と `Shareable` だけである（見つかったのは `{other}`。ADR 0026 の決定 2）"),
                                );
                            }
                        };
                        self.manual_properties.push((p.type_name.clone(), prop));
                    }
                }
                Item::Impl(i) => self.register_impl(i, false)?,
                _ => {}
            }
        }
        // 4.5. トレイトと、その実装（ADR 0022 決定 2）。取り込んだ実装は登録だけをし、
        // 置き場所・適合の検査は、それを書いたモジュールの検査で済んでいるものとして扱う。
        for item in &all {
            if let Item::Trait(t) = item {
                self.register_trait(t)?;
            }
        }
        let n_externs = self.externs.len();
        for (idx, item) in all.iter().enumerate() {
            if let Item::ImplFor(i) = item {
                // 同じ実装が複数の `use` の経路で届いても、登録は 1 回だけ（二重の実装ではない）。
                if self
                    .impl_index
                    .contains_key(&(i as *const ImplForDecl as usize))
                {
                    continue;
                }
                self.register_impl_for(i, idx >= n_externs)?;
            }
        }
        self.build_scc(file);
        // 5. 篩型の条件と関数の本体
        for item in &file.items {
            match item {
                Item::Type(t) => self.check_refinement(t)?,
                Item::Global(g) => {
                    self.span = g.span;
                    let (expected_ty, _) = self.globals[&g.name].clone();
                    let init_ty = self.expr(&g.init)?;
                    if !self.coerce(&expected_ty, &init_ty, &g.init) {
                        return self.err(
                            ErrorKind::TypeMismatch,
                            format!(
                                "大域変数 `{}` の初期化式の型は `{}` でなければならないが、`{}` である",
                                g.name,
                                self.sh(&expected_ty),
                                self.sh(&init_ty)
                            ),
                        );
                    }
                }
                Item::Fn(f) => {
                    let sig = self.fns[&f.name].clone();
                    self.check_fn(f, &sig)?;
                }
                Item::Impl(i) => {
                    self.tparams = i.generics.iter().map(|g| g.name.clone()).collect();
                    let r = self.resolve(&i.ty, Pos::Other);
                    self.tparams.clear();
                    let Ok(Ty::Named(tn, _)) = r else { continue };
                    for f in &i.fns {
                        let sig = self.methods[&(tn.clone(), f.name.clone())].clone();
                        self.check_fn(f, &sig)?;
                    }
                }
                Item::ImplFor(i) => {
                    let Some(&idx) = self.impl_index.get(&(i as *const ImplForDecl as usize))
                    else {
                        continue;
                    };
                    let imp = self.trait_impls[idx].clone();
                    for f in &i.fns {
                        let sig = imp
                            .fns
                            .iter()
                            .find(|(d, _)| d.name == f.name)
                            .map(|(_, s)| s.clone())
                            .expect("登録済みの実装の関数");
                        self.check_fn(f, &sig)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// 多相再帰の拒否（ADR 0022 決定 2）: 循環する総称の関数どうしの呼び出しで、呼ぶ側の型引数を
    /// 真に含む型を型引数に渡していたら、その呼び出しの位置でエラーにする。
    pub(super) fn poly_check(&mut self, m: &HashMap<String, Ty>, callee: &str) -> R<()> {
        let (Some(&a), Some(&b)) = (self.scc.get(&self.cur_fn), self.scc.get(callee)) else {
            return Ok(());
        };
        if a != b {
            return Ok(());
        }
        for t in m.values() {
            let z = self.zonk(t);
            if self
                .tparams
                .iter()
                .any(|p| contains_param(&z, p) && z != Ty::Param(p.clone()))
            {
                return self.err(
                    ErrorKind::PolymorphicRecursion,
                    format!(
                        "関数 `{}` から `{callee}` を呼ぶときに、型引数を真に含む型 `{}` を渡している（多相再帰は書けない。単相化が止まらなくなる）",
                        self.cur_fn,
                        show(&z)
                    ),
                );
            }
        }
        Ok(())
    }

    /// 循環する総称の関数の強連結成分を作る（名前で引く。メソッドとトレイトの関数も名前でまとめる）。
    fn build_scc(&mut self, file: &File) {
        let mut generic: HashSet<&str> = HashSet::new();
        let mut bodies: Vec<(&str, &Block, bool)> = vec![];
        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    let g = !f.generics.is_empty();
                    if g {
                        generic.insert(f.name.as_str());
                    }
                    bodies.push((f.name.as_str(), &f.body, g));
                }
                Item::Impl(i) => {
                    let g = !i.generics.is_empty();
                    for f in &i.fns {
                        let gf = g || !f.generics.is_empty();
                        if gf {
                            generic.insert(f.name.as_str());
                        }
                        bodies.push((f.name.as_str(), &f.body, gf));
                    }
                }
                Item::ImplFor(i) => {
                    let g = !i.generics.is_empty();
                    for f in &i.fns {
                        let gf = g || !f.generics.is_empty();
                        if gf {
                            generic.insert(f.name.as_str());
                        }
                        bodies.push((f.name.as_str(), &f.body, gf));
                    }
                }
                _ => {}
            }
        }
        let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
        for (name, body, g) in &bodies {
            if !g {
                continue;
            }
            let mut called = vec![];
            collect_calls_block(body, &mut called);
            for c in called {
                if generic.contains(c.as_str()) {
                    edges.entry(name.to_string()).or_default().insert(c);
                }
            }
        }
        // 到達可能性で強連結成分に分ける（総称の関数の数は小さい）。
        let nodes: Vec<&str> = generic.iter().copied().collect();
        let reach = |from: &str, edges: &HashMap<String, HashSet<String>>| -> HashSet<String> {
            let mut seen = HashSet::new();
            let mut work: Vec<String> = edges
                .get(from)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            while let Some(n) = work.pop() {
                if seen.insert(n.clone()) {
                    if let Some(s) = edges.get(&n) {
                        work.extend(s.iter().cloned());
                    }
                }
            }
            seen
        };
        let reaches: HashMap<&str, HashSet<String>> =
            nodes.iter().map(|n| (*n, reach(n, &edges))).collect();
        let mut comp: HashMap<String, usize> = HashMap::new();
        let mut next = 0usize;
        for &n in &nodes {
            if !reaches[n].contains(n) {
                continue; // 循環していない
            }
            if comp.contains_key(n) {
                continue;
            }
            for &m in &nodes {
                if reaches[n].contains(m) && reaches[m].contains(n) {
                    comp.insert(m.to_string(), next);
                }
            }
            next += 1;
        }
        self.scc = comp;
    }
}

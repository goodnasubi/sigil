use crate::ast::*;
use crate::error::{Error, ErrorKind, Span};
use crate::lexer::Tok;

use super::{describe, FnKind, Parser, R, TARGETS};

impl Parser {
    pub(super) fn file(&mut self) -> R<File> {
        let module = self.module_decl()?;
        let uses = self.use_decls()?;
        let mut items = Vec::new();
        loop {
            match self.tok() {
                Tok::Eof => break,
                Tok::Kw("use") => {
                    return Err(self.err(
                        ErrorKind::UseMisplaced,
                        "`use` は `module` の宣言の直後にまとめて書く（宣言の後には書けない）",
                    ));
                }
                Tok::Kw("pub") => {
                    let pub_span = self.span();
                    self.bump();
                    let item = match self.tok() {
                        Tok::Kw("type") => Item::Type(self.type_decl()?),
                        Tok::Kw("struct") => Item::Struct(self.struct_decl()?),
                        Tok::Kw("enum") => Item::Enum(self.enum_decl()?),
                        Tok::Kw("fn") => Item::Fn(self.fn_decl(FnKind::Free)?),
                        Tok::Kw("trait") => Item::Trait(self.trait_decl()?),
                        _ => {
                            return Err(Error::new(
                                ErrorKind::PubTarget,
                                pub_span,
                                "`pub` は `type`・`struct`・`enum`・`fn`・`trait` の宣言にだけ付けられる（`impl` には付けず、固有のメソッドはメソッドごとに付ける）",
                            ))
                        }
                    };
                    items.push(match item {
                        Item::Type(mut t) => {
                            t.is_pub = true;
                            Item::Type(t)
                        }
                        Item::Struct(mut s) => {
                            s.is_pub = true;
                            Item::Struct(s)
                        }
                        Item::Enum(mut e) => {
                            e.is_pub = true;
                            Item::Enum(e)
                        }
                        Item::Fn(mut f) => {
                            f.is_pub = true;
                            Item::Fn(f)
                        }
                        Item::Trait(mut t) => {
                            t.is_pub = true;
                            Item::Trait(t)
                        }
                        Item::Impl(i) => Item::Impl(i),
                        Item::ImplFor(i) => Item::ImplFor(i),
                        Item::Global(g) => Item::Global(g),
                        Item::Properties(p) => Item::Properties(p),
                    });
                }
                Tok::Kw("module") => {
                    return Err(self.err(
                        ErrorKind::ModuleDuplicate,
                        "`module` の宣言はファイルに 1 つだけ書ける",
                    ));
                }
                Tok::Kw("type") => items.push(Item::Type(self.type_decl()?)),
                Tok::Kw("struct") => items.push(Item::Struct(self.struct_decl()?)),
                Tok::Kw("enum") => items.push(Item::Enum(self.enum_decl()?)),
                Tok::Kw("fn") => items.push(Item::Fn(self.fn_decl(FnKind::Free)?)),
                Tok::Kw("trait") => items.push(Item::Trait(self.trait_decl()?)),
                Tok::Kw("impl") => {
                    let item = self.impl_decl()?;
                    items.push(item);
                }
                Tok::Kw("global") => items.push(Item::Global(self.global_decl()?)),
                Tok::Kw("properties") => items.push(Item::Properties(self.properties_decl()?)),
                Tok::Kw("let") => {
                    return Err(self.err(
                        ErrorKind::GlobalValue,
                        "大域の値（`let`）は宣言できない（P0 は大域の可変な値を持たない。ADR 0006 の決定 3 の条件 12）",
                    ));
                }
                _ => {
                    return Err(self
                        .unexpected("`type`・`struct`・`enum`・`fn`・`trait`・`impl` のいずれか"))
                }
            }
        }
        Ok(File {
            module,
            uses,
            items,
        })
    }

    /// `use a.b.name;` を 1 行 1 名で、文字の順に（重複・順の違反はエラー。ADR 0021 の決定 3）。
    fn use_decls(&mut self) -> R<Vec<UseDecl>> {
        let mut uses: Vec<UseDecl> = Vec::new();
        while self.at_kw("use") {
            let span = self.span();
            self.bump();
            let mut path = Vec::new();
            loop {
                let (s, sp) = self.ident_raw("`use` の名前")?;
                // 最後の部分は型の名前（CamelCase）でもよい。手前はモジュール名の部分。
                if self.at_punct(".") {
                    Self::check_snake(&s, sp, "モジュール名の部分")?;
                }
                path.push(s);
                if !self.eat_punct(".") {
                    break;
                }
            }
            if path.len() < 2 {
                return Err(Error::new(
                    ErrorKind::UseForm,
                    span,
                    "`use` は モジュール名.名前 の形で 1 名ずつ書く（例: `use app.http.server.handle;`）",
                ));
            }
            if self.at_punct("{") || self.at_punct("*") {
                return Err(self.err(
                    ErrorKind::UseForm,
                    "`use` は 1 名ずつで、まとめ書き・ワイルドカード・別名は持たない（ADR 0021 の決定 3）",
                ));
            }
            self.expect_punct(";")?;
            if let Some(prev) = uses.last() {
                if prev.path == path {
                    return Err(Error::new(
                        ErrorKind::UseDuplicate,
                        span,
                        format!("`use {}` が重複している", path.join(".")),
                    ));
                }
                if prev.path > path {
                    return Err(Error::new(
                        ErrorKind::UseUnsorted,
                        span,
                        format!(
                            "`use` は文字の順に並べる（`{}` は `{}` より前）",
                            path.join("."),
                            prev.path.join(".")
                        ),
                    ));
                }
            }
            uses.push(UseDecl { path, span });
        }
        Ok(uses)
    }

    fn module_decl(&mut self) -> R<ModuleDecl> {
        let span = self.span();
        if !self.at_kw("module") {
            return Err(self.err(
                ErrorKind::ModuleMissing,
                format!(
                    "ファイルの先頭（コメントの後）に `module` の宣言が要る（見つかったのは {}）",
                    describe(self.tok())
                ),
            ));
        }
        self.bump();
        let mut name = Vec::new();
        loop {
            let (s, sp) = self.ident_raw("モジュール名")?;
            Self::check_snake(&s, sp, "モジュール名の部分")?;
            name.push(s);
            if !self.eat_punct(".") {
                break;
            }
        }
        let targets_line = self.span().line;
        self.module_clause("targets")?;
        let mut targets: Vec<(String, Span)> = Vec::new();
        loop {
            let (s, sp) = self.ident_raw("出力先")?;
            if !TARGETS.contains(&s.as_str()) {
                return Err(Error::new(
                    ErrorKind::UnknownTarget,
                    sp,
                    format!(
                        "出力先 `{s}` は P0 で書けない（書けるのは {}）",
                        TARGETS.join("・")
                    ),
                ));
            }
            if let Some((prev, _)) = targets.last() {
                if prev == &s {
                    return Err(Error::new(
                        ErrorKind::TargetsDuplicate,
                        sp,
                        format!("`targets` に `{s}` が重複している"),
                    ));
                }
                if prev.as_str() > s.as_str() {
                    return Err(Error::new(
                        ErrorKind::TargetsUnsorted,
                        sp,
                        format!("`targets` は文字の順に並べる（`{s}` は `{prev}` より前）"),
                    ));
                }
            }
            targets.push((s, sp));
            if !self.eat_punct(",") {
                break;
            }
            if !matches!(self.tok(), Tok::Ident(_)) {
                return Err(self.err(
                    ErrorKind::UnknownTarget,
                    "`targets` は 1 行の並びで、末尾に `,` は付けない",
                ));
            }
        }
        let trust_line = self.span().line;
        self.module_clause("trust")?;
        let trust = match self.tok() {
            Tok::Kw("none") => Trust::None,
            Tok::Ident(s) if s == "base" => Trust::Base,
            _ => return Err(self.unexpected("`none` または `base`")),
        };
        self.bump();
        Ok(ModuleDecl {
            name,
            targets: targets.into_iter().map(|(s, _)| s).collect(),
            trust,
            span,
            targets_line,
            trust_line,
        })
    }

    /// `module` の宣言の節（`targets` → `trust` の順）を 1 つ読む。
    fn module_clause(&mut self, want: &'static str) -> R<()> {
        if self.eat_kw(want) {
            return Ok(());
        }
        if self.at_kw("targets") || self.at_kw("trust") {
            return Err(self.err(
                ErrorKind::ModuleClauseOrder,
                "`module` の宣言の節は `targets` → `trust` の順に書く",
            ));
        }
        Err(self.err(
            ErrorKind::ModuleClauseMissing,
            format!("`module` の宣言に `{want}` の節がない（2 節ともすべて必須）"),
        ))
    }

    pub(super) fn generics(&mut self) -> R<Vec<Generic>> {
        if !self.eat_punct("<") {
            return Ok(Vec::new());
        }
        let gs = self.comma_sep(">", |p| {
            let name = p.camel_def("型引数名")?;
            let mut bounds = Vec::new();
            if p.eat_punct(":") {
                bounds.push(p.path("制約")?);
                while p.eat_punct("+") {
                    bounds.push(p.path("制約")?);
                }
            }
            Ok(Generic { name, bounds })
        })?;
        if gs.is_empty() {
            return Err(self.unexpected("型引数"));
        }
        Ok(gs)
    }

    pub(super) fn path(&mut self, what: &str) -> R<Vec<String>> {
        let mut v = vec![self.name_ref(what)?];
        while self.eat_punct(".") {
            v.push(self.name_ref(what)?);
        }
        Ok(v)
    }

    fn type_decl(&mut self) -> R<TypeDecl> {
        let span = self.span();
        self.bump();
        let name = self.camel_def("型の名前")?;
        self.expect_punct("=")?;
        let base = self.ty()?;
        if !self.at_kw("where") {
            return Err(self.err(
                ErrorKind::TypeAliasWithoutWhere,
                "`type` は篩型にだけ使い、`where` 条件が必須（別名だけの `type A = B;` は持たない）",
            ));
        }
        self.bump();
        let saved = self.allow_self;
        self.allow_self = true;
        let cond = self.expr();
        self.allow_self = saved;
        let cond = cond?;
        self.expect_punct(";")?;
        Ok(TypeDecl {
            is_pub: false,
            name,
            base,
            cond,
            span,
        })
    }

    pub(super) fn global_decl(&mut self) -> R<GlobalDecl> {
        let span = self.span();
        self.bump(); // "global"
        let is_mut = if self.tok() == &Tok::Kw("mut") {
            self.bump();
            true
        } else {
            false
        };
        let (name, nspan) = self.ident_raw("大域変数の名前")?;
        Self::check_upper_snake(&name, nspan, "大域変数の名前")?;
        self.check_not_reserved(&name, nspan, "大域変数の名前")?;
        self.expect_punct(":")?;
        let ty = self.ty()?;
        self.expect_punct("=")?;
        let init = self.expr()?;
        Ok(GlobalDecl {
            is_mut,
            name,
            ty,
            init,
            span,
        })
    }

    pub(super) fn properties_decl(&mut self) -> R<PropertiesDecl> {
        let span = self.span();
        self.bump(); // "properties"
        let (type_name, nspan) = self.ident_raw("型の名前")?;
        Self::check_camel(&type_name, nspan, "型の名前")?;
        self.expect_punct(":")?;
        let mut properties = Vec::new();
        loop {
            let (prop_name, pspan) = self.ident_raw("性質の名前")?;
            Self::check_camel(&prop_name, pspan, "性質の名前")?;
            if !matches!(prop_name.as_str(), "Sendable" | "Shareable") {
                return Err(Error::new(
                    ErrorKind::UnknownProperty,
                    pspan,
                    format!(
                        "手付けできる性質は `Sendable` と `Shareable` だけである（見つかったのは `{prop_name}`。ADR 0026 の決定 2）"
                    ),
                ));
            }
            if !properties.contains(&prop_name) {
                properties.push(prop_name);
            }
            if !self.eat_punct(",") {
                break;
            }
        }
        let premise_span = self.span();
        let premise = match self.tok().clone() {
            Tok::Str(s) => {
                self.bump();
                if s.is_empty() {
                    return Err(Error::new(
                        ErrorKind::EmptyPremise,
                        premise_span,
                        "性質の手付けには空でない前提の文が要る（ADR 0026 の決定 2・ADR 0016 の決定 7）",
                    ));
                }
                s
            }
            _ => {
                return Err(self.err(
                    ErrorKind::MissingClause,
                    format!(
                        "`properties` 項目には前提の文（文字列）が要る。見つかったのは {}",
                        describe(self.tok())
                    ),
                ));
            }
        };
        Ok(PropertiesDecl {
            type_name,
            properties,
            premise,
            span,
        })
    }

    fn struct_decl(&mut self) -> R<StructDecl> {
        let span = self.span();
        self.bump();
        let name = self.camel_def("構造体の名前")?;
        let generics = self.generics()?;
        self.expect_punct("{")?;
        let fields = self.comma_sep("}", |p| {
            let line = p.span().line;
            let name = p.snake_plain("フィールド名")?;
            p.expect_punct(":")?;
            let ty = p.ty()?;
            Ok(Field { name, ty, line })
        })?;
        let end_line = self.toks[self.pos - 1].span.line;
        Ok(StructDecl {
            is_pub: false,
            name,
            generics,
            fields,
            span,
            end_line,
        })
    }

    fn enum_decl(&mut self) -> R<EnumDecl> {
        let span = self.span();
        self.bump();
        let name = self.camel_def("列挙型の名前")?;
        let generics = self.generics()?;
        self.expect_punct("{")?;
        let open = self.span();
        let variants = self.comma_sep("}", |p| {
            let line = p.span().line;
            let name = p.camel_def("列挙型の値の名前")?;
            let fields = if p.eat_punct("(") {
                let ts = p.comma_sep(")", |p| p.ty())?;
                if ts.is_empty() {
                    return Err(p.unexpected("型"));
                }
                ts
            } else {
                Vec::new()
            };
            Ok(Variant { name, fields, line })
        })?;
        if variants.is_empty() {
            return Err(Error::new(
                ErrorKind::EmptyEnum,
                open,
                "列挙型は値を 1 つ以上持つ",
            ));
        }
        let end_line = self.toks[self.pos - 1].span.line;
        Ok(EnumDecl {
            is_pub: false,
            name,
            generics,
            variants,
            span,
            end_line,
        })
    }

    /// `trait 名前 { fn ...; }`（ADR 0022 の決定 2）。型引数は持たない。
    /// トレイトの関数は 4 節を持ち、本体なしで `;` で終わる（`pub` は関数ごとに書かない）。
    fn trait_decl(&mut self) -> R<TraitDecl> {
        let span = self.span();
        self.bump();
        let name = self.camel_def("トレイトの名前")?;
        self.expect_punct("{")?;
        let mut fns = Vec::new();
        while !self.at_punct("}") {
            if !self.at_kw("fn") {
                return Err(self.unexpected("`fn` または `}`"));
            }
            fns.push(self.fn_decl(FnKind::Trait)?);
        }
        let end_line = self.span().line;
        self.bump();
        Ok(TraitDecl {
            is_pub: false,
            name,
            fns,
            span,
            end_line,
        })
    }

    /// `impl 型 { ... }` と `impl トレイト for 型 { ... }`（ADR 0022 の決定 2）。
    /// 最初の型を読んでから `for` の有無で分ける。
    fn impl_decl(&mut self) -> R<Item> {
        let span = self.span();
        self.bump();
        let generics = self.generics()?;
        let first = self.ty()?;
        if self.at_kw("for") {
            // `impl トレイト for 型`。トレイトは修飾しない名前 1 つ（型引数は持たない）。
            let Type::Path { path, args } = &first else {
                return Err(Error::new(
                    ErrorKind::Unexpected,
                    span,
                    "`impl ... for ...` のトレイトは名前で書く（`impl Handler for Health { ... }`）",
                ));
            };
            if path.len() != 1 || !args.is_empty() {
                return Err(Error::new(
                    ErrorKind::Unexpected,
                    span,
                    "`impl` のトレイトは修飾しない名前 1 つで書き、型引数は付けない（ADR 0022 の決定 2）",
                ));
            }
            let trait_name = path[0].clone();
            self.bump();
            let ty = self.ty()?;
            if !matches!(ty, Type::Path { .. }) {
                return Err(Error::new(
                    ErrorKind::Unexpected,
                    span,
                    "`impl ... for` の対象は型の名前（`impl Handler for Health { ... }`）",
                ));
            }
            let fns = self.impl_fns()?;
            let end_line = self.span().line;
            self.bump();
            return Ok(Item::ImplFor(ImplForDecl {
                generics,
                trait_name,
                ty,
                fns,
                span,
                end_line,
            }));
        }
        let ty = first;
        if !matches!(ty, Type::Path { .. }) {
            return Err(Error::new(
                ErrorKind::Unexpected,
                span,
                "`impl` の対象は型の名前（`impl Account { ... }`）",
            ));
        }
        let fns = self.impl_fns()?;
        let end_line = self.span().line;
        self.bump();
        Ok(Item::Impl(ImplDecl {
            generics,
            ty,
            fns,
            span,
            end_line,
        }))
    }

    /// `impl` の波括弧の中の関数（メソッドごとに `pub` を付けられる）。
    fn impl_fns(&mut self) -> R<Vec<FnDecl>> {
        self.expect_punct("{")?;
        let mut fns = Vec::new();
        while !self.at_punct("}") {
            let is_pub = self.eat_kw("pub");
            if !self.at_kw("fn") {
                return Err(self.unexpected("`fn` または `}`"));
            }
            let mut f = self.fn_decl(FnKind::Impl)?;
            f.is_pub = is_pub;
            fns.push(f);
        }
        Ok(fns)
    }

    fn fn_decl(&mut self, kind: FnKind) -> R<FnDecl> {
        let span = self.span();
        self.bump();
        let name_span = self.span();
        let name = self.snake_def("関数名")?;
        if kind == FnKind::Impl && name == "clone" && !self.allow_reserved {
            return Err(Error::new(
                ErrorKind::ReservedNameDefined,
                name_span,
                "`clone` は標準のメソッド名なので、`impl` の中で定義できない",
            ));
        }
        if name == "check" {
            return Err(Error::new(
                ErrorKind::ReservedNameDefined,
                name_span,
                "`check` は篩型の関数名として予約されており、関数として定義できない",
            ));
        }
        let generics = self.generics()?;
        if kind == FnKind::Trait && !generics.is_empty() {
            return Err(Error::new(
                ErrorKind::NotInP0,
                span,
                "トレイトの関数に型引数は書けない（ADR 0022 の決定 2 の「持たないもの」）",
            ));
        }
        self.expect_punct("(")?;
        let mut first_param = true;
        let saved_self = self.allow_self;
        // トレイトの関数のシグネチャの中だけ、型の名前 `Self` を書ける。
        let saved_trait = self.in_trait;
        if kind == FnKind::Trait {
            self.in_trait = true;
        }
        let params = self.comma_sep(")", |p| {
            let name = if kind != FnKind::Free && first_param && p.at_kw("self") {
                p.bump();
                p.allow_self = true;
                "self".to_string()
            } else {
                p.snake_def("引数名")?
            };
            first_param = false;
            p.expect_punct(":")?;
            let ty = p.ty()?;
            Ok(Param { name, ty })
        });
        let params = match params {
            Ok(v) => v,
            Err(e) => {
                self.allow_self = saved_self;
                self.in_trait = saved_trait;
                return Err(e);
            }
        };
        let ret = if self.eat_punct("->") {
            match self.ty() {
                Ok(t) => Some(t),
                Err(e) => {
                    self.allow_self = saved_self;
                    self.in_trait = saved_trait;
                    return Err(e);
                }
            }
        } else {
            None
        };
        self.in_trait = saved_trait;

        let mut clause_lines = [0u32; clause::COUNT];
        clause_lines[clause::INTENT] = self.span().line;
        self.expect_clause("intent")?;
        let intent_span = self.span();
        let intent = match self.tok().clone() {
            Tok::Str(s) => {
                self.bump();
                s
            }
            _ => return Err(self.unexpected("`intent` の説明（文字列リテラル）")),
        };
        if intent.is_empty() {
            return Err(Error::new(
                ErrorKind::EmptyIntent,
                intent_span,
                "`intent` は空にできない",
            ));
        }

        clause_lines[clause::REQUIRES] = self.span().line;
        self.expect_clause("requires")?;
        let requires = if self.eat_kw("none") {
            None
        } else {
            Some(self.expr()?)
        };

        clause_lines[clause::ENSURES] = self.span().line;
        self.expect_clause("ensures")?;
        let ensures = if self.eat_kw("none") {
            None
        } else {
            let (o, r) = (self.allow_old, self.allow_result);
            self.allow_old = true;
            self.allow_result = true;
            let e = self.expr();
            self.allow_old = o;
            self.allow_result = r;
            Some(e?)
        };

        clause_lines[clause::EFFECTS] = self.span().line;
        self.expect_clause("effects")?;
        let mut effects = Vec::new();
        if !self.eat_kw("none") {
            loop {
                effects.push(self.effect()?);
                if !(self.eat_punct(",") && !self.at_punct("{")) {
                    break;
                }
            }
        }

        if kind == FnKind::Trait && self.at_kw("unchecked") {
            return Err(self.err(
                ErrorKind::NotInP0,
                "`unchecked` は本体のある関数にだけ書ける（トレイトの関数は本体を持たない）",
            ));
        }
        let mut unchecked = None;
        if self.at_kw("unchecked") {
            clause_lines[clause::UNCHECKED] = self.span().line;
            self.bump();
            let span = self.span();
            match self.tok().clone() {
                Tok::Str(s) => {
                    self.bump();
                    if s.is_empty() {
                        return Err(Error::new(
                            ErrorKind::EmptyUnchecked,
                            span,
                            "`unchecked` の前提の文は空にできない",
                        ));
                    }
                    unchecked = Some(s);
                }
                _ => return Err(self.unexpected("`unchecked` の前提（文字列リテラル）")),
            }
        }

        let body = if kind == FnKind::Trait {
            // トレイトの関数は本体を持たず、`effects` 節の後の `;` で終わる。
            // `body` は空とし、開き・閉じの行は `;` の行を指す（`ast::TraitDecl` の注記）。
            let semi = self.span();
            self.expect_punct(";").map(|_| Block {
                stmts: Vec::new(),
                open_line: semi.line,
                end_line: semi.line,
            })
        } else {
            self.block()
        };
        self.allow_self = saved_self;
        let body = body?;
        Ok(FnDecl {
            is_pub: false,
            name,
            generics,
            params,
            ret,
            intent,
            requires,
            ensures,
            effects,
            unchecked,
            body,
            span,
            clause_lines,
        })
    }

    fn expect_clause(&mut self, want: &'static str) -> R<()> {
        if self.eat_kw(want) {
            return Ok(());
        }
        for c in ["intent", "requires", "ensures", "effects", "unchecked"] {
            if self.at_kw(c) {
                return Err(self.err(
                    ErrorKind::ClauseOrder,
                    format!(
                        "契約節は `intent` → `requires` → `ensures` → `effects`（→ 省ける `unchecked`）の順に書く（`{want}` のところに `{c}` がある）"
                    ),
                ));
            }
        }
        Err(self.err(
            ErrorKind::MissingClause,
            format!(
                "関数に契約節 `{want}` がない（4 節はすべて必須。空の節は `none`）。見つかったのは {}",
                describe(self.tok())
            ),
        ))
    }
}

//! 構文解析（ADR 0009 の「文法の要約」）。
//!
//! 再帰下降。最初のエラーで打ち切る。名前の大小文字の規約、予約名の定義、
//! `old`・`result`・`self` の位置、`module` の宣言の検査など、文法の後の
//! 検査（ADR 0009）も構文解析の中で行い、違反は構文エラーにする。
//!
//! 整形器が「正規形の違反」として扱うもの（末尾のカンマ、不要な括弧、
//! 効果の並べ方、`|x| { return e; }` など）は、構文としては受理する。

use crate::ast::*;
use crate::error::{Error, ErrorKind, Span};
use crate::lexer::{lex, Comment, Tok, Token};

type R<T> = Result<T, Error>;

/// 利用者が関数・変数・引数・型として定義できない名前（ADR 0009 の決定 4）。
/// `check` は関数名としてだけ予約するので、ここには含めない。
pub const RESERVED_NAMES: &[&str] = &[
    "console",
    "fs",
    "net",
    "db",
    "state",
    "clock",
    "process",
    "foreign",
    "task",
    "browser",
    "Int",
    "Float",
    "Bool",
    "String",
    "Unit",
    "List",
    "Option",
    "Result",
    "Some",
    "None",
    "Ok",
    "Err",
    "Copyable",
    "Equatable",
    "Comparable",
    "Numeric",
    "Sendable",
    "Shareable",
    "Displayable",
    "Path",
    "IoError",
    "ParseError",
    "Shared",
    "Cell",
    "Set",
    "Map",
    "to_float",
];

/// 出力先（ADR 0008 の決定 6。P0 で書けるもの）。
pub const TARGETS: &[&str] = &["browser", "server"];

#[derive(Debug, Clone)]
pub struct Parsed {
    pub file: File,
    pub comments: Vec<Comment>,
}

pub fn parse(src: &str) -> Result<File, Error> {
    parse_with_comments(src).map(|p| p.file)
}

pub fn parse_with_comments(src: &str) -> Result<Parsed, Error> {
    parse_inner(src, false)
}

/// 標準ライブラリの表（`src/stdlib.rs`）を読む。標準の名前を定義するので、予約名の検査をしない。
pub fn parse_std(src: &str) -> Result<File, Error> {
    parse_inner(src, true).map(|p| p.file)
}

fn parse_inner(src: &str, allow_reserved: bool) -> Result<Parsed, Error> {
    let lexed = lex(src)?;
    let mut p = Parser {
        allow_reserved,
        toks: lexed.tokens,
        pos: 0,
        no_struct: false,
        allow_old: false,
        allow_result: false,
        allow_self: false,
        in_old: false,
        depth: 0,
    };
    let file = p.file()?;
    Ok(Parsed {
        file,
        comments: lexed.comments,
    })
}

fn is_snake(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || !b[0].is_ascii_lowercase() {
        return false;
    }
    let mut prev_us = false;
    for &c in b {
        if c == b'_' {
            if prev_us {
                return false;
            }
            prev_us = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            prev_us = false;
        } else {
            return false;
        }
    }
    !prev_us
}

fn is_camel(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_uppercase() && b.iter().all(|c| c.is_ascii_alphanumeric())
}

fn is_upper_start(s: &str) -> bool {
    s.as_bytes().first().is_some_and(|c| c.is_ascii_uppercase())
}

struct Parser {
    /// 予約名の定義を許す（標準ライブラリの表を読むときだけ）。
    allow_reserved: bool,
    toks: Vec<Token>,
    pos: usize,
    /// 条件・`match` の対象の中では構造体の値を括弧なしで書けない。
    no_struct: bool,
    allow_old: bool,
    allow_result: bool,
    allow_self: bool,
    in_old: bool,
    depth: usize,
}

/// 式・型・パターン・ブロックの入れ子の深さの上限。
/// `.`・`(...)`・`?` の連鎖、二項演算の連鎖、`else if` の連鎖も 1 つにつき 1 段と数える
/// （構文木の深さを抑え、木をたどる再帰と `Drop` のスタックあふれを防ぐ）。
const MAX_DEPTH: usize = 256;

fn describe(t: &Tok) -> String {
    match t {
        Tok::Ident(s) => format!("識別子 `{s}`"),
        Tok::Underscore => "`_`".to_string(),
        Tok::Kw(k) => format!("予約語 `{k}`"),
        Tok::Int(s) | Tok::Float(s) => format!("数値 `{s}`"),
        Tok::Str(_) => "文字列".to_string(),
        Tok::Punct(p) => format!("`{p}`"),
        Tok::Eof => "ファイルの終わり".to_string(),
    }
}

impl Parser {
    // ---- 基本操作 ----

    fn tok(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn bump(&mut self) {
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
    }

    fn at_punct(&self, p: &str) -> bool {
        matches!(self.tok(), Tok::Punct(q) if *q == p)
    }

    fn at_kw(&self, k: &str) -> bool {
        matches!(self.tok(), Tok::Kw(q) if *q == k)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.at_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        if self.at_kw(k) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn ty(&mut self) -> R<Type> {
        self.enter()?;
        let r = self.ty_inner();
        self.depth -= 1;
        r
    }

    fn block(&mut self) -> R<Block> {
        self.enter()?;
        let r = self.block_inner();
        self.depth -= 1;
        r
    }

    fn pattern(&mut self) -> R<Pattern> {
        self.enter()?;
        let r = self.pattern_inner();
        self.depth -= 1;
        r
    }

    fn expr(&mut self) -> R<Expr> {
        self.enter()?;
        let r = self.expr_inner();
        self.depth -= 1;
        r
    }

    fn prefix(&mut self) -> R<Expr> {
        self.enter()?;
        let r = self.prefix_inner();
        self.depth -= 1;
        r
    }

    /// 入れ子の深さを 1 つ進める。上限を超えたら構文エラー（スタックあふれ防止）。
    fn enter(&mut self) -> R<()> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err(
                ErrorKind::TooDeep,
                format!("入れ子が深すぎる（上限は {MAX_DEPTH}）"),
            ));
        }
        self.depth += 1;
        Ok(())
    }

    fn unexpected(&self, expected: &str) -> Error {
        Error::new(
            ErrorKind::Unexpected,
            self.span(),
            format!(
                "{expected}が要る（見つかったのは {}）",
                describe(self.tok())
            ),
        )
    }

    fn expect_punct(&mut self, p: &str) -> R<()> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{p}`")))
        }
    }

    fn err(&self, kind: ErrorKind, msg: impl Into<String>) -> Error {
        Error::new(kind, self.span(), msg)
    }

    /// `open` を消費した後に呼ぶ。末尾のカンマは受理する（整形器が正規形にする）。
    fn comma_sep<T>(&mut self, close: &str, mut f: impl FnMut(&mut Self) -> R<T>) -> R<Vec<T>> {
        let mut items = Vec::new();
        while !self.at_punct(close) {
            items.push(f(self)?);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(close)?;
        Ok(items)
    }

    // ---- 名前 ----

    fn ident_raw(&mut self, what: &str) -> R<(String, Span)> {
        let span = self.span();
        match self.tok().clone() {
            Tok::Ident(s) => {
                self.bump();
                Ok((s, span))
            }
            Tok::Kw(k) => Err(Error::new(
                ErrorKind::ReservedWordAsName,
                span,
                format!("予約語 `{k}` は{what}に使えない"),
            )),
            _ => Err(self.unexpected(what)),
        }
    }

    fn check_snake(s: &str, span: Span, what: &str) -> R<()> {
        if is_snake(s) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::BadNameForm,
                span,
                format!("{what} `{s}` は小文字の `_` 区切り（例: `read_port`）で書く"),
            ))
        }
    }

    fn check_camel(s: &str, span: Span, what: &str) -> R<()> {
        if is_camel(s) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::BadNameForm,
                span,
                format!("{what} `{s}` は大文字始まりの連結形（`_` なし。例: `ConfigError`）で書く"),
            ))
        }
    }

    fn check_not_reserved(&self, s: &str, span: Span, what: &str) -> R<()> {
        if !self.allow_reserved && RESERVED_NAMES.contains(&s) {
            Err(Error::new(
                ErrorKind::ReservedNameDefined,
                span,
                format!("`{s}` は予約名なので{what}として定義できない"),
            ))
        } else {
            Ok(())
        }
    }

    /// 小文字の名前の定義（関数・変数・引数・束縛）。
    fn snake_def(&mut self, what: &str) -> R<String> {
        let (s, span) = self.ident_raw(what)?;
        Self::check_snake(&s, span, what)?;
        self.check_not_reserved(&s, span, what)?;
        Ok(s)
    }

    /// 小文字の名前（フィールドなど。予約名の制限なし）。
    fn snake_plain(&mut self, what: &str) -> R<String> {
        let (s, span) = self.ident_raw(what)?;
        Self::check_snake(&s, span, what)?;
        Ok(s)
    }

    /// 大文字始まりの名前の定義（型・構造体・列挙型・値・型引数）。
    fn camel_def(&mut self, what: &str) -> R<String> {
        let (s, span) = self.ident_raw(what)?;
        Self::check_camel(&s, span, what)?;
        self.check_not_reserved(&s, span, what)?;
        Ok(s)
    }

    /// 参照の位置の名前。大文字始まりなら連結形、小文字始まりなら `_` 区切り。
    fn name_ref(&mut self, what: &str) -> R<String> {
        let (s, span) = self.ident_raw(what)?;
        if is_upper_start(&s) {
            Self::check_camel(&s, span, what)?;
        } else {
            Self::check_snake(&s, span, what)?;
        }
        Ok(s)
    }

    /// 大文字始まりの名前の参照（パターンの列挙型の値）。
    fn camel_ref(&mut self, what: &str) -> R<String> {
        let (s, span) = self.ident_raw(what)?;
        Self::check_camel(&s, span, what)?;
        Ok(s)
    }

    // ---- ファイルと宣言 ----

    fn file(&mut self) -> R<File> {
        let module = self.module_decl()?;
        let mut items = Vec::new();
        loop {
            match self.tok() {
                Tok::Eof => break,
                Tok::Kw("module") => {
                    return Err(self.err(
                        ErrorKind::ModuleDuplicate,
                        "`module` の宣言はファイルに 1 つだけ書ける",
                    ));
                }
                Tok::Kw("type") => items.push(Item::Type(self.type_decl()?)),
                Tok::Kw("struct") => items.push(Item::Struct(self.struct_decl()?)),
                Tok::Kw("enum") => items.push(Item::Enum(self.enum_decl()?)),
                Tok::Kw("fn") => items.push(Item::Fn(self.fn_decl(false)?)),
                Tok::Kw("impl") => items.push(Item::Impl(self.impl_decl()?)),
                Tok::Kw("let") => {
                    return Err(self.err(
                        ErrorKind::GlobalValue,
                        "大域の値（`let`）は宣言できない（P0 は大域の可変な値を持たない。ADR 0006 の決定 3 の条件 12）",
                    ));
                }
                _ => {
                    return Err(self.unexpected("`type`・`struct`・`enum`・`fn`・`impl` のいずれか"))
                }
            }
        }
        Ok(File { module, items })
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
                    format!("出力先 `{s}` は P0 で書けない（書けるのは browser・server）"),
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

    fn generics(&mut self) -> R<Vec<Generic>> {
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

    fn path(&mut self, what: &str) -> R<Vec<String>> {
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
            name,
            base,
            cond,
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
            name,
            generics,
            variants,
            span,
            end_line,
        })
    }

    fn impl_decl(&mut self) -> R<ImplDecl> {
        let span = self.span();
        self.bump();
        let generics = self.generics()?;
        let ty = self.ty()?;
        if !matches!(ty, Type::Path { .. }) {
            return Err(Error::new(
                ErrorKind::Unexpected,
                span,
                "`impl` の対象は型の名前（`impl Account { ... }`）",
            ));
        }
        self.expect_punct("{")?;
        let mut fns = Vec::new();
        while !self.at_punct("}") {
            if !self.at_kw("fn") {
                return Err(self.unexpected("`fn` または `}`"));
            }
            fns.push(self.fn_decl(true)?);
        }
        let end_line = self.span().line;
        self.bump();
        Ok(ImplDecl {
            generics,
            ty,
            fns,
            span,
            end_line,
        })
    }

    fn fn_decl(&mut self, in_impl: bool) -> R<FnDecl> {
        let span = self.span();
        self.bump();
        let name_span = self.span();
        let name = self.snake_def("関数名")?;
        if in_impl && name == "clone" && !self.allow_reserved {
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
        self.expect_punct("(")?;
        let mut first_param = true;
        let saved_self = self.allow_self;
        let params = self.comma_sep(")", |p| {
            let name = if in_impl && first_param && p.at_kw("self") {
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
                return Err(e);
            }
        };
        let ret = if self.eat_punct("->") {
            match self.ty() {
                Ok(t) => Some(t),
                Err(e) => {
                    self.allow_self = saved_self;
                    return Err(e);
                }
            }
        } else {
            None
        };

        let mut clause_lines = [0u32; 4];
        clause_lines[0] = self.span().line;
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

        clause_lines[1] = self.span().line;
        self.expect_clause("requires")?;
        let requires = if self.eat_kw("none") {
            None
        } else {
            Some(self.expr()?)
        };

        clause_lines[2] = self.span().line;
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

        clause_lines[3] = self.span().line;
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

        let body = self.block();
        self.allow_self = saved_self;
        let body = body?;
        Ok(FnDecl {
            name,
            generics,
            params,
            ret,
            intent,
            requires,
            ensures,
            effects,
            body,
            span,
            clause_lines,
        })
    }

    fn expect_clause(&mut self, want: &'static str) -> R<()> {
        if self.eat_kw(want) {
            return Ok(());
        }
        for c in ["intent", "requires", "ensures", "effects"] {
            if self.at_kw(c) {
                return Err(self.err(
                    ErrorKind::ClauseOrder,
                    format!(
                        "契約節は `intent` → `requires` → `ensures` → `effects` の順に書く（`{want}` のところに `{c}` がある）"
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

    fn effect(&mut self) -> R<Effect> {
        if self.eat_kw("of") {
            self.expect_punct("(")?;
            let name = self.snake_plain("引数名")?;
            self.expect_punct(")")?;
            return Ok(Effect::Of(name));
        }
        let mut v = vec![self.snake_plain("効果のカテゴリ")?];
        while self.eat_punct(".") {
            v.push(self.snake_plain("効果のカテゴリ")?);
        }
        Ok(Effect::Category(v))
    }

    // ---- 型 ----

    fn ty_inner(&mut self) -> R<Type> {
        match self.tok().clone() {
            Tok::Punct("&") => {
                self.bump();
                let mutable = self.eat_kw("mut");
                if self.at_punct("&") || self.at_punct("&&") {
                    return Err(self.err(
                        ErrorKind::RefOfRef,
                        "参照の参照 `& &T` は書けない（ADR 0003）",
                    ));
                }
                Ok(Type::Ref {
                    mutable,
                    inner: Box::new(self.ty()?),
                })
            }
            Tok::Punct("&&") => Err(self.err(
                ErrorKind::RefOfRef,
                "参照の参照 `&&T` は書けない（ADR 0003）",
            )),
            Tok::Kw("fn") => {
                self.bump();
                self.expect_punct("(")?;
                let params = self.comma_sep(")", |p| p.ty())?;
                let ret = if self.eat_punct("->") {
                    Some(Box::new(self.ty()?))
                } else {
                    None
                };
                Ok(Type::Fn { params, ret })
            }
            Tok::Punct("(") => {
                let sp = self.span();
                self.bump();
                let ts = self.comma_sep(")", |p| p.ty())?;
                if ts.len() < 2 {
                    return Err(Error::new(
                        ErrorKind::BadTuple,
                        sp,
                        "組の型は要素 2 つ以上（空の組と要素 1 つの組は P0 にない）",
                    ));
                }
                Ok(Type::Tuple(ts))
            }
            Tok::Ident(_) => {
                let path = self.path("型の名前")?;
                let mut args = Vec::new();
                if self.eat_punct("<") {
                    args = self.comma_sep(">", |p| p.ty())?;
                    if args.is_empty() {
                        return Err(self.unexpected("型引数"));
                    }
                }
                Ok(Type::Path { path, args })
            }
            _ => Err(self.unexpected("型")),
        }
    }

    // ---- 文 ----

    fn block_inner(&mut self) -> R<Block> {
        let open_line = self.span().line;
        self.expect_punct("{")?;
        let saved = self.no_struct;
        self.no_struct = false;
        let mut stmts = Vec::new();
        while !self.at_punct("}") {
            if matches!(self.tok(), Tok::Eof) {
                return Err(self.unexpected("`}`"));
            }
            stmts.push(self.stmt()?);
        }
        let end_line = self.span().line;
        self.bump();
        self.no_struct = saved;
        Ok(Block {
            stmts,
            open_line,
            end_line,
        })
    }

    fn cond_expr(&mut self) -> R<Expr> {
        let saved = self.no_struct;
        self.no_struct = true;
        let e = self.expr();
        self.no_struct = saved;
        e
    }

    fn stmt(&mut self) -> R<Stmt> {
        let span = self.span();
        let kind = match self.tok().clone() {
            Tok::Kw("let") => {
                self.bump();
                let mutable = self.eat_kw("mut");
                if mutable && self.at_punct("(") {
                    return Err(self.err(
                        ErrorKind::NotInP0,
                        "`let mut (a, b)` は書けない（分解した名前に `mut` は付けられない。ADR 0011 の決定 7）",
                    ));
                }
                let pattern = self.let_pattern()?;
                let ty = if self.eat_punct(":") {
                    Some(self.ty()?)
                } else {
                    None
                };
                self.expect_punct("=")?;
                let value = self.expr()?;
                self.expect_punct(";")?;
                StmtKind::Let {
                    mutable,
                    pattern,
                    ty,
                    value,
                }
            }
            Tok::Kw("return") => {
                self.bump();
                let value = if self.at_punct(";") {
                    None
                } else {
                    Some(self.expr()?)
                };
                self.expect_punct(";")?;
                StmtKind::Return(value)
            }
            Tok::Kw("if") => StmtKind::If(self.if_stmt()?),
            Tok::Kw("while") => {
                self.bump();
                let cond = self.cond_expr()?;
                let body = self.block()?;
                StmtKind::While { cond, body }
            }
            Tok::Kw("match") => self.stmt_match()?,
            Tok::Punct("{") => {
                return Err(self.err(
                    ErrorKind::StandaloneBlock,
                    "単独のブロック `{ ... }` は文として書けない",
                ));
            }
            _ => {
                let e = self.expr()?;
                if self.at_punct("=") {
                    if !is_place(&e) {
                        return Err(self.err(
                            ErrorKind::InvalidAssignTarget,
                            "代入の左辺は変数かフィールド（`a.b`）だけ",
                        ));
                    }
                    self.bump();
                    let value = self.expr()?;
                    self.expect_punct(";")?;
                    StmtKind::Assign { target: e, value }
                } else if self.at_punct(";") {
                    if !is_call_stmt(&e) {
                        return Err(Error::new(
                            ErrorKind::ExprStmtNotCall,
                            span,
                            "式文にできるのは呼び出し（末尾の `?` を含む）だけ",
                        ));
                    }
                    self.bump();
                    StmtKind::Expr(e)
                } else {
                    return Err(self.unexpected("`;` または `=`"));
                }
            }
        };
        Ok(Stmt { kind, span })
    }

    /// `else if` の連鎖は 1 つにつき入れ子の深さを 1 段進める。
    fn if_stmt(&mut self) -> R<IfStmt> {
        self.enter()?;
        let line = self.span().line;
        self.bump();
        let cond = self.cond_expr()?;
        let then = self.block()?;
        let otherwise = if self.eat_kw("else") {
            if self.at_kw("if") {
                Some(Box::new(Else::If(self.if_stmt()?)))
            } else {
                Some(Box::new(Else::Block(self.block()?)))
            }
        } else {
            None
        };
        self.depth -= 1;
        Ok(IfStmt {
            line,
            cond,
            then,
            otherwise,
        })
    }

    fn stmt_match(&mut self) -> R<StmtKind> {
        let open_span = self.span();
        self.bump();
        let scrutinee = self.cond_expr()?;
        self.expect_punct("{")?;
        let mut arms = Vec::new();
        while !self.at_punct("}") {
            let line = self.span().line;
            let pattern = self.pattern()?;
            self.expect_punct("=>")?;
            if !self.at_punct("{") {
                return Err(self.err(
                    ErrorKind::StmtMatchArmNotBlock,
                    "文の位置の `match` の腕は `パターン => {` で始まるブロックにする（値を選ぶ `match` は `return match ...;` か `let y = match ...;` と書く）",
                ));
            }
            let body = self.block()?;
            if self.at_punct(",") {
                return Err(self.err(
                    ErrorKind::StmtMatchArmComma,
                    "文の `match` の腕の後に `,` は付けない",
                ));
            }
            arms.push(StmtArm {
                pattern,
                body,
                line,
            });
        }
        let end_line = self.span().line;
        self.bump();
        if arms.is_empty() {
            return Err(Error::new(
                ErrorKind::EmptyMatch,
                open_span,
                "`match` は腕を 1 つ以上持つ",
            ));
        }
        Ok(StmtKind::Match {
            scrutinee,
            arms,
            end_line,
        })
    }

    // ---- パターン ----

    /// `let` の分解のパターン: 束縛・`_`・組の入れ子だけ（ADR 0011 の決定 7）。
    fn let_pattern(&mut self) -> R<Pattern> {
        match self.tok().clone() {
            Tok::Underscore => {
                self.bump();
                Ok(Pattern::Wild)
            }
            Tok::Punct("(") => {
                let sp = self.span();
                self.bump();
                let ps = self.comma_sep(")", |p| p.let_pattern())?;
                if ps.len() < 2 {
                    return Err(Error::new(
                        ErrorKind::BadTuple,
                        sp,
                        "組の分解は要素 2 つ以上",
                    ));
                }
                Ok(Pattern::Tuple(ps))
            }
            _ => Ok(Pattern::Bind(self.snake_def("変数名")?)),
        }
    }

    fn pattern_inner(&mut self) -> R<Pattern> {
        match self.tok().clone() {
            Tok::Underscore => {
                self.bump();
                Ok(Pattern::Wild)
            }
            Tok::Int(s) => {
                self.bump();
                Ok(Pattern::Int(s))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Pattern::Str(s))
            }
            Tok::Float(_) => Err(self.err(ErrorKind::NotInP0, "小数はパターンに書けない")),
            Tok::Kw("true") => {
                self.bump();
                Ok(Pattern::Bool(true))
            }
            Tok::Kw("false") => {
                self.bump();
                Ok(Pattern::Bool(false))
            }
            Tok::Punct("-") => {
                self.bump();
                match self.tok().clone() {
                    Tok::Int(s) => {
                        self.bump();
                        Ok(Pattern::NegInt(s))
                    }
                    _ => Err(self.unexpected("整数")),
                }
            }
            Tok::Punct("(") => {
                let sp = self.span();
                self.bump();
                let ps = self.comma_sep(")", |p| p.pattern())?;
                if ps.len() < 2 {
                    return Err(Error::new(
                        ErrorKind::BadTuple,
                        sp,
                        "組のパターンは要素 2 つ以上",
                    ));
                }
                Ok(Pattern::Tuple(ps))
            }
            Tok::Ident(s) if is_upper_start(&s) => {
                let mut path = vec![self.camel_ref("列挙型の値")?];
                if self.eat_punct(".") {
                    path.push(self.camel_ref("列挙型の値")?);
                }
                let mut args = Vec::new();
                if self.eat_punct("(") {
                    args = self.comma_sep(")", |p| p.pattern())?;
                    if args.is_empty() {
                        return Err(self.unexpected("パターン"));
                    }
                }
                Ok(Pattern::Variant { path, args })
            }
            Tok::Ident(_) => Ok(Pattern::Bind(self.snake_def("束縛名")?)),
            _ => Err(self.unexpected("パターン")),
        }
    }

    // ---- 式 ----

    fn expr_inner(&mut self) -> R<Expr> {
        if self.at_punct("|") || self.at_punct("||") || self.at_kw("move") {
            self.closure()
        } else {
            self.or_expr()
        }
    }

    fn closure(&mut self) -> R<Expr> {
        let is_move = self.eat_kw("move");
        let mut params = Vec::new();
        if !self.eat_punct("||") {
            self.expect_punct("|")?;
            while !self.at_punct("|") {
                params.push(self.snake_def("引数名")?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct("|")?;
        }
        let body = if self.at_punct("{") {
            ClosureBody::Block(self.block()?)
        } else {
            ClosureBody::Expr(Box::new(self.expr()?))
        };
        Ok(Expr::Closure {
            is_move,
            params,
            body,
        })
    }

    fn or_expr(&mut self) -> R<Expr> {
        let mut lhs = self.cmp_expr()?;
        let (op, tok, other) = if self.at_punct("&&") {
            (BinOp::And, "&&", "||")
        } else if self.at_punct("||") {
            (BinOp::Or, "||", "&&")
        } else {
            return Ok(lhs);
        };
        let mut links = 0;
        while self.at_punct(tok) {
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.cmp_expr()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        self.depth -= links;
        if self.at_punct(other) {
            return Err(self.err(
                ErrorKind::MixedAndOr,
                "`&&` と `||` を括弧なしで混ぜられない（`a || (b && c)` のように括弧を付ける）",
            ));
        }
        Ok(lhs)
    }

    fn cmp_op(&self) -> Option<BinOp> {
        match self.tok() {
            Tok::Punct("==") => Some(BinOp::Eq),
            Tok::Punct("!=") => Some(BinOp::Ne),
            Tok::Punct("<") => Some(BinOp::Lt),
            Tok::Punct("<=") => Some(BinOp::Le),
            Tok::Punct(">") => Some(BinOp::Gt),
            Tok::Punct(">=") => Some(BinOp::Ge),
            _ => None,
        }
    }

    fn cmp_expr(&mut self) -> R<Expr> {
        let lhs = self.add_expr()?;
        let Some(op) = self.cmp_op() else {
            return Ok(lhs);
        };
        self.bump();
        let rhs = self.add_expr()?;
        if self.cmp_op().is_some() {
            return Err(self.err(
                ErrorKind::ComparisonChain,
                "比較は連鎖できない（`a < b && b < c` のように `&&` でつなぐ）",
            ));
        }
        Ok(Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn add_expr(&mut self) -> R<Expr> {
        let mut lhs = self.mul_expr()?;
        let mut links = 0;
        loop {
            let op = match self.tok() {
                Tok::Punct("+") => BinOp::Add,
                Tok::Punct("-") => BinOp::Sub,
                _ => {
                    self.depth -= links;
                    return Ok(lhs);
                }
            };
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.mul_expr()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn mul_expr(&mut self) -> R<Expr> {
        let mut lhs = self.prefix()?;
        let mut links = 0;
        loop {
            let op = match self.tok() {
                Tok::Punct("*") => BinOp::Mul,
                Tok::Punct("/") => BinOp::Div,
                Tok::Punct("%") => BinOp::Rem,
                _ => {
                    self.depth -= links;
                    return Ok(lhs);
                }
            };
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.prefix()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn prefix_inner(&mut self) -> R<Expr> {
        let op = match self.tok() {
            Tok::Punct("!") => UnOp::Not,
            Tok::Punct("-") => UnOp::Neg,
            Tok::Punct("&") => {
                self.bump();
                let op = if self.eat_kw("mut") {
                    UnOp::RefMut
                } else {
                    UnOp::Ref
                };
                if self.at_punct("&") || self.at_punct("&&") {
                    return Err(self.err(
                        ErrorKind::RefOfRef,
                        "参照の参照 `& &x` は書けない（ADR 0003。必要なら `&(&x)` と書く）",
                    ));
                }
                let expr = Box::new(self.prefix()?);
                return Ok(Expr::Unary { op, expr });
            }
            Tok::Punct("&&") => {
                return Err(self.err(
                    ErrorKind::RefOfRef,
                    "前置の `&&`（参照の参照）は書けない（ADR 0003）",
                ));
            }
            _ => return self.postfix(),
        };
        self.bump();
        let expr = Box::new(self.prefix()?);
        Ok(Expr::Unary { op, expr })
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        let mut links = 0;
        loop {
            if self.at_punct(".") || self.at_punct("(") || self.at_punct("?") {
                self.enter()?;
                links += 1;
            }
            if self.eat_punct(".") {
                let name = self.name_ref("フィールド名・メソッド名")?;
                e = Expr::Field {
                    expr: Box::new(e),
                    name,
                };
            } else if self.eat_punct("(") {
                let saved = self.no_struct;
                self.no_struct = false;
                let args = self.comma_sep(")", |p| p.expr());
                self.no_struct = saved;
                e = Expr::Call {
                    callee: Box::new(e),
                    args: args?,
                };
            } else if self.eat_punct("?") {
                e = Expr::Try(Box::new(e));
            } else if self.at_punct("[") {
                return Err(self.err(
                    ErrorKind::NotInP0,
                    "添字 `a[i]` は持たない（`[` `]` は将来のための予約）",
                ));
            } else {
                break;
            }
        }
        self.depth -= links;
        if self.at_punct("{") {
            if let Some(path) = as_type_path(&e) {
                if !self.no_struct {
                    return self.struct_lit(path);
                }
                if matches!(
                    self.toks.get(self.pos + 1).map(|t| &t.tok),
                    Some(Tok::Ident(_))
                ) && matches!(
                    self.toks.get(self.pos + 2).map(|t| &t.tok),
                    Some(Tok::Punct(":"))
                ) {
                    return Err(self.err(
                        ErrorKind::Unexpected,
                        "条件や `match` の対象に構造体の値を直接書けない。括弧で囲む（例: `if x == (Foo { a: 1 }) {`）",
                    ));
                }
            }
        }
        Ok(e)
    }

    fn struct_lit(&mut self, path: Vec<String>) -> R<Expr> {
        let open = self.span();
        self.bump();
        let saved = self.no_struct;
        self.no_struct = false;
        let fields = self.comma_sep("}", |p| {
            let name = p.snake_plain("フィールド名")?;
            p.expect_punct(":")?;
            let v = p.expr()?;
            Ok((name, v))
        });
        self.no_struct = saved;
        let fields = fields?;
        if fields.is_empty() {
            return Err(Error::new(
                ErrorKind::EmptyStructLiteral,
                open,
                "構造体の値はフィールドを 1 つ以上書く（文法の要約）",
            ));
        }
        Ok(Expr::StructLit { path, fields })
    }

    fn primary(&mut self) -> R<Expr> {
        match self.tok().clone() {
            Tok::Kw("if") => Err(self.err(
                ErrorKind::Unexpected,
                "`if` は文であり式ではない。値を選ぶ分岐は `match` で書く（例: `let y = match c { true => a, false => b };`）",
            )),
            Tok::Int(s) => {
                self.bump();
                Ok(Expr::Int(s))
            }
            Tok::Float(s) => {
                self.bump();
                Ok(Expr::Float(s))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(Expr::Str(s))
            }
            Tok::Kw("true") => {
                self.bump();
                Ok(Expr::Bool(true))
            }
            Tok::Kw("false") => {
                self.bump();
                Ok(Expr::Bool(false))
            }
            Tok::Kw("result") => {
                if !self.allow_result {
                    let msg = if self.in_old {
                        "`old(...)` の中に `result` は書けない"
                    } else {
                        "`result` は `ensures` の中だけで書ける"
                    };
                    return Err(self.err(ErrorKind::ResultOutsideEnsures, msg));
                }
                self.bump();
                Ok(Expr::Result)
            }
            Tok::Kw("self") => {
                if !self.allow_self {
                    return Err(self.err(
                        ErrorKind::SelfOutsideRefinement,
                        "`self` は篩型の条件の中だけで書ける",
                    ));
                }
                self.bump();
                Ok(Expr::SelfValue)
            }
            Tok::Kw("old") => {
                if !self.allow_old {
                    let msg = if self.in_old {
                        "`old(...)` の中に `old` は書けない"
                    } else {
                        "`old` は `ensures` の中だけで書ける"
                    };
                    return Err(self.err(ErrorKind::OldOutsideEnsures, msg));
                }
                self.bump();
                self.expect_punct("(")?;
                let saved = (self.allow_old, self.allow_result, self.in_old, self.no_struct);
                self.allow_old = false;
                self.allow_result = false;
                self.in_old = true;
                self.no_struct = false;
                let e = self.expr();
                (self.allow_old, self.allow_result, self.in_old, self.no_struct) = saved;
                let e = e?;
                self.expect_punct(")")?;
                Ok(Expr::Old(Box::new(e)))
            }
            Tok::Kw("match") => self.match_expr(),
            Tok::Ident(_) => Ok(Expr::Name(self.name_ref("名前")?)),
            Tok::Punct("(") => {
                let sp = self.span();
                self.bump();
                let saved = self.no_struct;
                self.no_struct = false;
                let r = self.paren_rest(sp);
                self.no_struct = saved;
                r
            }
            _ => Err(self.unexpected("式")),
        }
    }

    /// `(` の後。括弧または組。
    fn paren_rest(&mut self, sp: Span) -> R<Expr> {
        if self.at_punct(")") {
            return Err(Error::new(
                ErrorKind::BadTuple,
                sp,
                "空の組 `()` は P0 にない",
            ));
        }
        let first = self.expr()?;
        if self.eat_punct(")") {
            return Ok(Expr::Paren(Box::new(first)));
        }
        if !self.at_punct(",") {
            return Err(self.unexpected("`)` または `,`"));
        }
        let mut items = vec![first];
        while self.eat_punct(",") {
            if self.at_punct(")") {
                break;
            }
            items.push(self.expr()?);
        }
        self.expect_punct(")")?;
        if items.len() < 2 {
            return Err(Error::new(
                ErrorKind::BadTuple,
                sp,
                "要素 1 つの組は P0 にない",
            ));
        }
        Ok(Expr::Tuple(items))
    }

    fn match_expr(&mut self) -> R<Expr> {
        let open_span = self.span();
        self.bump();
        let scrutinee = self.cond_expr()?;
        self.expect_punct("{")?;
        let saved = self.no_struct;
        self.no_struct = false;
        let arms = self.value_arms();
        self.no_struct = saved;
        let (arms, end_line) = arms?;
        if arms.is_empty() {
            return Err(Error::new(
                ErrorKind::EmptyMatch,
                open_span,
                "`match` は腕を 1 つ以上持つ",
            ));
        }
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            end_line,
        })
    }

    fn value_arms(&mut self) -> R<(Vec<ExprArm>, u32)> {
        let mut arms = Vec::new();
        while !self.at_punct("}") {
            let line = self.span().line;
            let pattern = self.pattern()?;
            self.expect_punct("=>")?;
            if self.at_punct("{") {
                return Err(self.err(
                    ErrorKind::ValueMatchArmBlock,
                    "値を選ぶ `match` の腕にブロックは書けない（`パターン => 式,`）。副作用の文を実行するなら、文の位置に `match` を書く",
                ));
            }
            let body = self.expr()?;
            arms.push(ExprArm {
                pattern,
                body,
                line,
            });
            if !self.eat_punct(",") {
                break;
            }
        }
        let end_line = self.span().line;
        self.expect_punct("}")?;
        Ok((arms, end_line))
    }
}

/// 代入の左辺: 変数、またはフィールドの連なり。
fn is_place(e: &Expr) -> bool {
    let mut cur = e;
    loop {
        match cur {
            Expr::Name(n) => return !is_upper_start(n),
            Expr::SelfValue => return true,
            Expr::Field { expr, name } if !is_upper_start(name) => cur = expr,
            _ => return false,
        }
    }
}

fn is_call_stmt(e: &Expr) -> bool {
    let mut cur = e;
    loop {
        match cur {
            Expr::Call { .. } => return true,
            Expr::Try(inner) => cur = inner,
            _ => return false,
        }
    }
}

/// 名前と `.` の連なりで、最後の部分が大文字始まりなら、構造体の値のパス。
fn as_type_path(e: &Expr) -> Option<Vec<String>> {
    let mut v = Vec::new();
    let mut cur = e;
    loop {
        match cur {
            Expr::Name(n) => {
                v.push(n.clone());
                break;
            }
            Expr::Field { expr, name } => {
                v.push(name.clone());
                cur = expr;
            }
            _ => return None,
        }
    }
    v.reverse();
    if v.last().is_some_and(|s| is_upper_start(s)) {
        Some(v)
    } else {
        None
    }
}

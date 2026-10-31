//! 構文解析（ADR 0009 の「文法の要約」）。
//!
//! 再帰下降。最初のエラーで打ち切る。名前の大小文字の規約、予約名の定義、
//! `old`・`result`・`self` の位置、`module` の宣言の検査など、文法の後の
//! 検査（ADR 0009）も構文解析の中で行い、違反は構文エラーにする。
//!
//! 整形器が「正規形の違反」として扱うもの（末尾のカンマ、不要な括弧、
//! 効果の並べ方、`|x| { return e; }` など）は、構文としては受理する。

mod expr;
mod items;
mod pattern;
mod stmt;
mod types;

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
    // トレイトの宣言の中でだけ書ける型の名前（ADR 0022 の決定 2）。
    "Self",
    "Path",
    "IoError",
    "ParseError",
    "Shared",
    "Cell",
    "Addr",
    "Set",
    "Map",
    "to_float",
    "raw",
];

/// 出力先（ADR 0008 の決定 6。P0 で書けるもの）。
pub const TARGETS: &[&str] = &["bare", "browser", "server"];

#[derive(Debug, Clone)]
pub struct Parsed {
    pub file: File,
    pub comments: Vec<Comment>,
}

/// `fn_decl` が読む関数の置き場所。本体の有無と、受け手 `self`・型の名前 `Self` の扱いが違う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FnKind {
    /// トップレベルの関数。
    Free,
    /// `impl` の中のメソッド（固有・トレイトの実装とも）。
    Impl,
    /// トレイトの宣言の中の関数（本体なし。`Self` を書ける）。
    Trait,
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
        in_trait: false,
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
    /// トレイトの関数のシグネチャの中（型の名前 `Self` を書ける唯一の場所。ADR 0022 の決定 2）。
    in_trait: bool,
    depth: usize,
}

/// 式・型・パターン・ブロックの入れ子の深さの上限。
/// `.`・`(...)`・`?` の連鎖、二項演算の連鎖、`else if` の連鎖も 1 つにつき 1 段と数える
/// （構文木の深さを抑え、木をたどる再帰と `Drop` のスタックあふれを防ぐ）。
const MAX_DEPTH: usize = 128;

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
        match self.tok() {
            Tok::Punct(q) if *q == p => true,
            Tok::Punct(">>") if p == ">" => true,
            _ => false,
        }
    }

    fn at_kw(&self, k: &str) -> bool {
        matches!(self.tok(), Tok::Kw(q) if *q == k)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        match self.tok() {
            Tok::Punct(q) if *q == p => {
                self.bump();
                true
            }
            Tok::Punct(">>") if p == ">" => {
                // `>>` を 1 つ目の `>` として消費し、2 つ目の `>` に置き換える
                self.toks[self.pos].tok = Tok::Punct(">");
                true
            }
            _ => false,
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
}

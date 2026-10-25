//! 字句解析（ADR 0009 の決定 1〜6）。
//!
//! 空白と改行は区切りとしてだけ働く。コメントは字句の列とは別に集める
//! （整形器が使う）。最初の字句エラーで打ち切る。

use crate::error::{Error, ErrorKind, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// 識別子（予約語でないもの）。先頭が大文字か小文字かは文字列から分かる。
    Ident(String),
    /// `_` だけの語（パターンの「何でもよい」）。
    Underscore,
    /// 予約語。
    Kw(&'static str),
    /// 10 進の整数リテラル（綴りのまま）。
    Int(String),
    /// 小数リテラル（綴りのまま）。
    Float(String),
    /// 文字列リテラル（エスケープを解いた値）。
    Str(String),
    /// 記号。
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub span: Span,
    /// `//` の後ろの本文（そのまま）。
    pub text: String,
    /// 同じ行の前にコードがあるか（行末コメント）。
    pub trailing: bool,
}

#[derive(Debug, Clone)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub comments: Vec<Comment>,
}

/// 識別子として使えない構文の語（ADR 0009 の決定 4）。
pub const KEYWORDS: &[&str] = &[
    "module", "targets", "trust", "fn", "type", "struct", "enum", "intent", "requires", "ensures",
    "effects", "none", "of", "old", "result", "where", "self", "let", "mut", "return", "if",
    "else", "while", "match", "move", "true", "false", "impl",
];

/// 将来のために予約する語。P0 では使えば字句エラー。
pub const FUTURE_RESERVED: &[&str] = &[
    "trait", "for", "in", "loop", "break", "continue", "use", "pub", "const", "as", "async",
    "await",
];

fn keyword(s: &str) -> Option<&'static str> {
    KEYWORDS.iter().find(|k| **k == s).copied()
}

pub fn lex(src: &str) -> Result<Lexed, Error> {
    Lexer::new(src).run()
}

struct Lexer {
    chars: Vec<char>,
    i: usize,
    line: u32,
    col: u32,
    tokens: Vec<Token>,
    comments: Vec<Comment>,
    last_token_line: u32,
}

fn err(kind: ErrorKind, span: Span, msg: impl Into<String>) -> Error {
    Error::new(kind, span, msg)
}

impl Lexer {
    fn new(src: &str) -> Self {
        Lexer {
            chars: src.chars().collect(),
            i: 0,
            line: 1,
            col: 1,
            tokens: Vec::new(),
            comments: Vec::new(),
            last_token_line: 0,
        }
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn peek_at(&self, n: usize) -> Option<char> {
        self.chars.get(self.i + n).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.chars.get(self.i).copied()?;
        self.i += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(c)
    }

    fn push(&mut self, tok: Tok, span: Span) {
        self.last_token_line = span.line;
        self.tokens.push(Token { tok, span });
    }

    /// タブ・CR は文字列やコメントの中でも字句エラー。
    fn check_raw(&self, c: char, span: Span) -> Result<(), Error> {
        match c {
            '\t' => Err(err(
                ErrorKind::Tab,
                span,
                "タブは書けない（字下げは空白 2 つ）",
            )),
            '\r' => Err(err(
                ErrorKind::CarriageReturn,
                span,
                "改行は LF だけ（CR は書けない）",
            )),
            _ => Ok(()),
        }
    }

    fn run(mut self) -> Result<Lexed, Error> {
        if self.peek() == Some('\u{feff}') {
            return Err(err(ErrorKind::Bom, self.span(), "BOM は書けない"));
        }
        while let Some(c) = self.peek() {
            let span = self.span();
            match c {
                ' ' | '\n' => {
                    self.bump();
                }
                '\t' | '\r' => {
                    self.check_raw(c, span)?;
                }
                '/' if self.peek_at(1) == Some('/') => self.comment(span)?,
                '/' if self.peek_at(1) == Some('*') => {
                    return Err(err(
                        ErrorKind::BlockComment,
                        span,
                        "ブロックコメント `/* */` はない（コメントは `//` だけ）",
                    ));
                }
                '"' => self.string(span)?,
                '0'..='9' => self.number(span)?,
                'A'..='Z' | 'a'..='z' | '_' => self.word(span)?,
                '\'' => {
                    return Err(err(
                        ErrorKind::CharLiteral,
                        span,
                        "文字リテラル `'a'` は P0 にない（ライフタイム注釈もない）",
                    ));
                }
                c if !c.is_ascii() => {
                    return Err(err(
                        ErrorKind::NonAsciiOutsideLiteral,
                        span,
                        format!(
                            "ASCII 以外の文字 `{c}` は文字列リテラルとコメントの中にだけ書ける"
                        ),
                    ));
                }
                _ => self.punct(span)?,
            }
        }
        let span = self.span();
        self.push(Tok::Eof, span);
        Ok(Lexed {
            tokens: self.tokens,
            comments: self.comments,
        })
    }

    fn comment(&mut self, span: Span) -> Result<(), Error> {
        self.bump();
        self.bump();
        let mut text = String::new();
        while let Some(c) = self.peek() {
            if c == '\n' {
                break;
            }
            self.check_raw(c, self.span())?;
            text.push(c);
            self.bump();
        }
        let trailing = self.last_token_line == span.line;
        self.comments.push(Comment {
            span,
            text,
            trailing,
        });
        Ok(())
    }

    fn string(&mut self, span: Span) -> Result<(), Error> {
        self.bump();
        let mut out = String::new();
        loop {
            let here = self.span();
            let Some(c) = self.peek() else {
                return Err(err(
                    ErrorKind::UnterminatedString,
                    span,
                    "文字列が閉じていない",
                ));
            };
            match c {
                '"' => {
                    self.bump();
                    break;
                }
                '\n' => {
                    return Err(err(
                        ErrorKind::NewlineInString,
                        here,
                        "文字列に改行を直接含められない（1 行の文字列だけ。`\\n` を使う）",
                    ));
                }
                '\\' => {
                    self.bump();
                    out.push(self.escape(here)?);
                }
                _ => {
                    self.check_raw(c, here)?;
                    out.push(c);
                    self.bump();
                }
            }
        }
        self.push(Tok::Str(out), span);
        Ok(())
    }

    fn escape(&mut self, at: Span) -> Result<char, Error> {
        let Some(c) = self.bump() else {
            return Err(err(
                ErrorKind::UnterminatedString,
                at,
                "文字列が閉じていない",
            ));
        };
        match c {
            '\\' => Ok('\\'),
            '"' => Ok('"'),
            'n' => Ok('\n'),
            't' => Ok('\t'),
            'r' => Ok('\r'),
            'u' => {
                let bad = |m: &str| err(ErrorKind::BadEscape, at, m.to_string());
                if self.bump() != Some('{') {
                    return Err(bad("`\\u` の後は `{16進}` を書く"));
                }
                let mut hex = String::new();
                loop {
                    match self.bump() {
                        Some('}') => break,
                        Some(h) if h.is_ascii_hexdigit() => hex.push(h),
                        _ => return Err(bad("`\\u{...}` の中は 16 進数だけ")),
                    }
                }
                if hex.is_empty() || hex.len() > 6 {
                    return Err(bad("`\\u{...}` は 1〜6 桁の 16 進数"));
                }
                let n = u32::from_str_radix(&hex, 16).map_err(|_| bad("`\\u{...}` の値が不正"))?;
                char::from_u32(n).ok_or_else(|| bad("`\\u{...}` が Unicode のスカラ値でない"))
            }
            other => Err(err(
                ErrorKind::BadEscape,
                at,
                format!("未定義のエスケープ `\\{other}`（使えるのは \\\\ \\\" \\n \\t \\r \\u{{16進}} だけ）"),
            )),
        }
    }

    fn digits(&mut self) -> String {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        s
    }

    fn number(&mut self, span: Span) -> Result<(), Error> {
        let int = self.digits();
        if int.len() > 1 && int.starts_with('0') {
            return Err(err(
                ErrorKind::LeadingZero,
                span,
                format!("数値の先頭に 0 は書けない（`{int}`）"),
            ));
        }
        let mut text = int;
        let mut is_float = false;
        if self.peek() == Some('.') {
            if self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
                self.bump();
                let frac = self.digits();
                text.push('.');
                text.push_str(&frac);
                is_float = true;
            } else {
                return Err(err(
                    ErrorKind::BadNumber,
                    span,
                    format!("小数点の後に数字が要る（`{text}.` は書けない）"),
                ));
            }
        }
        if self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(err(
                ErrorKind::BadNumber,
                span,
                format!(
                    "`{text}` の直後に文字がある。16 進・2 進・指数・桁区切り `_`・サフィックスは持たない（10 進だけ）"
                ),
            ));
        }
        self.push(
            if is_float {
                Tok::Float(text)
            } else {
                Tok::Int(text)
            },
            span,
        );
        Ok(())
    }

    fn word(&mut self, span: Span) -> Result<(), Error> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                s.push(c);
                self.bump();
            } else {
                break;
            }
        }
        if s == "_" {
            self.push(Tok::Underscore, span);
        } else if s.starts_with('_') {
            return Err(err(
                ErrorKind::UnderscoreName,
                span,
                format!("`_` で始まる名前 `{s}` は書けない"),
            ));
        } else if FUTURE_RESERVED.contains(&s.as_str()) {
            let note = if s == "async" || s == "await" {
                "Sigil に async/await の構文はない（中断は効果で表す。ADR 0007）"
            } else {
                "将来のために予約されており、P0 では使えない"
            };
            return Err(err(
                ErrorKind::FutureReservedWord,
                span,
                format!("予約語 `{s}`: {note}"),
            ));
        } else if let Some(k) = keyword(&s) {
            self.push(Tok::Kw(k), span);
        } else {
            self.push(Tok::Ident(s), span);
        }
        Ok(())
    }

    fn punct(&mut self, span: Span) -> Result<(), Error> {
        let c = self.peek().unwrap_or('\0');
        let n = self.peek_at(1);
        let two: Option<&'static str> = match (c, n) {
            ('&', Some('&')) => Some("&&"),
            ('|', Some('|')) => Some("||"),
            ('=', Some('=')) => Some("=="),
            ('!', Some('=')) => Some("!="),
            ('<', Some('=')) => Some("<="),
            ('>', Some('=')) => Some(">="),
            ('-', Some('>')) => Some("->"),
            ('=', Some('>')) => Some("=>"),
            _ => None,
        };
        if let Some(p) = two {
            self.bump();
            self.bump();
            self.push(Tok::Punct(p), span);
            return Ok(());
        }
        match (c, n) {
            ('+' | '-' | '*' | '/' | '%', Some('=')) => {
                return Err(err(
                    ErrorKind::CompoundAssign,
                    span,
                    format!("複合代入 `{c}=` は持たない（`x = x {c} 1` と書く）"),
                ));
            }
            ('.', Some('.')) => {
                return Err(err(ErrorKind::RangeOperator, span, "範囲 `..` は持たない"));
            }
            (':', Some(':')) => {
                return Err(err(
                    ErrorKind::PathSeparator,
                    span,
                    "パスの区切りは `.`（`::` は持たない）",
                ));
            }
            _ => {}
        }
        let one: Option<&'static str> = match c {
            '{' => Some("{"),
            '}' => Some("}"),
            '(' => Some("("),
            ')' => Some(")"),
            '[' => Some("["),
            ']' => Some("]"),
            ',' => Some(","),
            ';' => Some(";"),
            ':' => Some(":"),
            '.' => Some("."),
            '?' => Some("?"),
            '+' => Some("+"),
            '-' => Some("-"),
            '*' => Some("*"),
            '/' => Some("/"),
            '%' => Some("%"),
            '!' => Some("!"),
            '&' => Some("&"),
            '|' => Some("|"),
            '=' => Some("="),
            '<' => Some("<"),
            '>' => Some(">"),
            _ => None,
        };
        match one {
            Some(p) => {
                self.bump();
                self.push(Tok::Punct(p), span);
                Ok(())
            }
            None => Err(err(
                ErrorKind::UnexpectedChar,
                span,
                format!("使えない文字 `{c}`"),
            )),
        }
    }
}

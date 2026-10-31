use crate::ast::*;
use crate::error::{Error, ErrorKind};
use crate::lexer::Tok;

use super::{is_upper_start, Parser, R};

impl Parser {
    // ---- パターン ----

    /// `let` の分解のパターン: 束縛・`_`・組の入れ子だけ（ADR 0011 の決定 7）。
    pub(super) fn let_pattern(&mut self) -> R<Pattern> {
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

    pub(super) fn pattern_inner(&mut self) -> R<Pattern> {
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
}

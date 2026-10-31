use crate::ast::*;
use crate::error::{Error, ErrorKind};
use crate::lexer::Tok;

use super::{Parser, R};

impl Parser {
    pub(super) fn effect(&mut self) -> R<Effect> {
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

    pub(super) fn ty_inner(&mut self) -> R<Type> {
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
                let start = self.span();
                let path = self.path("型の名前")?;
                let mut args = Vec::new();
                if self.eat_punct("<") {
                    args = self.comma_sep(">", |p| p.ty())?;
                    if args.is_empty() {
                        return Err(self.unexpected("型引数"));
                    }
                }
                if path[0] == "Self" {
                    // `Self` はトレイトの宣言の中でのみ書ける型の名前（ADR 0022 の決定 2）。
                    // `impl` の中では書かず、実装する型を書く。
                    if path.len() != 1 || !args.is_empty() {
                        return Err(Error::new(
                            ErrorKind::Unexpected,
                            start,
                            "`Self` は単独で書く（`Self.名前` や `Self<...>` は書けない）",
                        ));
                    }
                    if !self.in_trait {
                        return Err(Error::new(
                            ErrorKind::SelfOutsideTrait,
                            start,
                            "`Self` はトレイトの宣言の中でのみ書ける型の名前（`impl` の中では実装する型を書く。ADR 0022 の決定 2）",
                        ));
                    }
                }
                Ok(Type::Path { path, args })
            }
            _ => Err(self.unexpected("型")),
        }
    }
}

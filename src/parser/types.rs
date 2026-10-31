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

    /// 関数の型の効果 1 つ（ADR 0022 の決定 3）。カテゴリだけが書ける
    /// （関数の型には引数の名前がないので `of(...)` は書けない）。
    pub(super) fn fn_type_effect(&mut self) -> R<Effect> {
        if self.at_kw("of") {
            return Err(self.err(
                ErrorKind::OfInFnType,
                "関数の型の効果に `of(...)` は書けない（関数の型には引数の名前がない。ADR 0022 の決定 3）",
            ));
        }
        let mut v = vec![self.snake_plain("効果のカテゴリ")?];
        while self.eat_punct(".") {
            v.push(self.snake_plain("効果のカテゴリ")?);
        }
        Ok(Effect::Category(v))
    }

    /// 関数の型の戻り値の後に続く効果節（ADR 0022 の決定 3）。書かれていなければ `None`、
    /// `effects none` は空の `Some`。位置による必須・禁止の検査は型検査（PR 2-4d の 2/3）で行う。
    pub(super) fn fn_type_effects(&mut self) -> R<Option<Vec<Effect>>> {
        if !self.eat_kw("effects") {
            return Ok(None);
        }
        if self.eat_kw("none") {
            return Ok(Some(Vec::new()));
        }
        let mut v = vec![self.fn_type_effect()?];
        loop {
            if !self.at_punct(",") {
                break;
            }
            // カンマが効果の続きか、外側の並び（フィールド・引数・組の要素）の区切りかは、
            // 次がカテゴリとして読めて、その後に `:`（名前つきの並びの区切り）が来ないことで決める。
            // 読めなければカンマを外側に返す（位置を戻す）。`of` のときだけは構文エラーのままにする。
            let save = self.pos;
            self.bump();
            if self.at_kw("of") {
                return Err(self.err(
                    ErrorKind::OfInFnType,
                    "関数の型の効果に `of(...)` は書けない（関数の型には引数の名前がない。ADR 0022 の決定 3）",
                ));
            }
            match self.fn_type_effect() {
                Ok(e) if !self.at_punct(":") => v.push(e),
                _ => {
                    self.pos = save;
                    break;
                }
            }
        }
        Ok(Some(v))
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
                let effects = self.fn_type_effects()?;
                Ok(Type::Fn {
                    params,
                    ret,
                    effects,
                })
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

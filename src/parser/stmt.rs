use crate::ast::*;
use crate::error::{Error, ErrorKind};
use crate::lexer::Tok;

use super::expr::{is_call_stmt, is_place};
use super::{Parser, R};

impl Parser {
    // ---- 文 ----

    pub(super) fn block_inner(&mut self) -> R<Block> {
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

    pub(super) fn cond_expr(&mut self) -> R<Expr> {
        let saved = self.no_struct;
        self.no_struct = true;
        let e = self.expr();
        self.no_struct = saved;
        e
    }

    pub(super) fn stmt(&mut self) -> R<Stmt> {
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
}

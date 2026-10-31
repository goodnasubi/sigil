use crate::ast::*;
use crate::error::{Error, ErrorKind, Span};
use crate::lexer::Tok;

use super::{is_upper_start, Parser, R};

impl Parser {
    // ---- 式 ----

    pub(super) fn expr_inner(&mut self) -> R<Expr> {
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
        let mut lhs = self.bit_or_expr()?;
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
            let rhs = self.bit_or_expr()?;
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

    fn bit_or_expr(&mut self) -> R<Expr> {
        let mut lhs = self.bit_xor_expr()?;
        let mut links = 0;
        while self.at_punct("|") {
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.bit_xor_expr()?;
            lhs = Expr::Binary {
                op: BinOp::BitOr,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        self.depth -= links;
        Ok(lhs)
    }

    fn bit_xor_expr(&mut self) -> R<Expr> {
        let mut lhs = self.bit_and_expr()?;
        let mut links = 0;
        while self.at_punct("^") {
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.bit_and_expr()?;
            lhs = Expr::Binary {
                op: BinOp::BitXor,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        self.depth -= links;
        Ok(lhs)
    }

    fn bit_and_expr(&mut self) -> R<Expr> {
        let mut lhs = self.cmp_expr()?;
        let mut links = 0;
        while self.at_punct("&") {
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.cmp_expr()?;
            lhs = Expr::Binary {
                op: BinOp::BitAnd,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        self.depth -= links;
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
        let lhs = self.shift_expr()?;
        let Some(op) = self.cmp_op() else {
            return Ok(lhs);
        };
        self.bump();
        let rhs = self.shift_expr()?;
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

    fn shift_expr(&mut self) -> R<Expr> {
        let mut lhs = self.add_expr()?;
        let mut links = 0;
        loop {
            let op = match self.tok() {
                Tok::Punct("<<") => BinOp::Shl,
                Tok::Punct(">>") => BinOp::Shr,
                _ => {
                    self.depth -= links;
                    return Ok(lhs);
                }
            };
            self.enter()?;
            links += 1;
            self.bump();
            let rhs = self.add_expr()?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
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

    pub(super) fn prefix_inner(&mut self) -> R<Expr> {
        let op = match self.tok() {
            Tok::Punct("!") => UnOp::Not,
            Tok::Punct("-") => UnOp::Neg,
            Tok::Punct("~") => UnOp::BitNot,
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
pub(super) fn is_place(e: &Expr) -> bool {
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

pub(super) fn is_call_stmt(e: &Expr) -> bool {
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

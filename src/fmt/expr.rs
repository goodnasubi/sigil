use crate::ast::*;

use super::escape_str;
use super::{
    returns_only, Printer, ADD, AND, BITAND, BITOR, BITXOR, CMP, LOWEST, MUL, OR, POSTFIX, PREFIX,
    PRIMARY, SHIFT,
};

fn strip(mut e: &Expr) -> &Expr {
    while let Expr::Paren(inner) = e {
        e = inner;
    }
    e
}

fn level(e: &Expr) -> u8 {
    match e {
        Expr::Closure { .. } => LOWEST,
        Expr::Binary { op, .. } => binop_level(*op),
        Expr::Unary { .. } => PREFIX,
        Expr::Field { .. } | Expr::Call { .. } | Expr::Try(_) | Expr::StructLit { .. } => POSTFIX,
        _ => PRIMARY,
    }
}

fn binop_level(op: BinOp) -> u8 {
    match op {
        BinOp::Or => OR,
        BinOp::And => AND,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => CMP,
        BinOp::BitOr => BITOR,
        BinOp::BitXor => BITXOR,
        BinOp::BitAnd => BITAND,
        BinOp::Shl | BinOp::Shr => SHIFT,
        BinOp::Add | BinOp::Sub => ADD,
        BinOp::Mul | BinOp::Div | BinOp::Rem => MUL,
    }
}

fn binop_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Shl => "<<",
        BinOp::Shr => ">>",
        BinOp::BitAnd => "&",
        BinOp::BitXor => "^",
        BinOp::BitOr => "|",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn is_logic(op: BinOp) -> bool {
    matches!(op, BinOp::And | BinOp::Or)
}

/// 小数の正規形: 小数部の末尾の 0 は、`0` 1 桁の場合を除いて書かない（ADR 0009 の決定 5）。
fn float_str(s: &str) -> String {
    match s.split_once('.') {
        Some((int, frac)) => {
            let t = frac.trim_end_matches('0');
            format!("{int}.{}", if t.is_empty() { "0" } else { t })
        }
        None => s.to_string(),
    }
}

impl<'a> Printer<'a> {
    pub(super) fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Wild => self.w("_"),
            Pattern::Bind(n) => self.w(n),
            Pattern::Int(s) => self.w(s),
            Pattern::NegInt(s) => self.w(&format!("-{s}")),
            Pattern::Str(s) => self.w(&escape_str(s)),
            Pattern::Bool(b) => self.w(if *b { "true" } else { "false" }),
            Pattern::Variant { path, args } => {
                self.w(&path.join("."));
                if !args.is_empty() {
                    self.w("(");
                    self.patterns(args);
                    self.w(")");
                }
            }
            Pattern::Tuple(ps) => {
                self.w("(");
                self.patterns(ps);
                self.w(")");
            }
        }
    }

    pub(super) fn patterns(&mut self, ps: &[Pattern]) {
        for (i, p) in ps.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.pattern(p);
        }
    }

    // ---- 文 ----

    /// `{` から `}` まで。`{` の前の空白は呼び出し側が置く。
    pub(super) fn block(&mut self, b: &Block, indent: usize, drop_last_return: bool) {
        let n = b.stmts.len() - usize::from(drop_last_return);
        if n == 0 && !self.pending_before(b.end_line) {
            self.w("{}");
            return;
        }
        self.w("{");
        let mut first = true;
        for s in &b.stmts[..n] {
            self.item_start(s.span.line, indent + 1, &mut first);
            self.stmt(s, indent + 1);
        }
        self.comments_before(b.end_line, indent + 1, true, &mut first);
        self.nl(indent, b.end_line);
        self.w("}");
    }

    fn stmt(&mut self, s: &Stmt, indent: usize) {
        match &s.kind {
            StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
            } => {
                self.w(if *mutable { "let mut " } else { "let " });
                self.pattern(pattern);
                if let Some(t) = ty {
                    self.w(": ");
                    self.ty(t);
                }
                self.w(" = ");
                self.expr(value, 0, false, indent);
                self.w(";");
            }
            StmtKind::Assign { target, value } => {
                self.lead = true;
                self.expr(target, 0, false, indent);
                self.w(" = ");
                self.expr(value, 0, false, indent);
                self.w(";");
            }
            StmtKind::Expr(e) => {
                self.lead = true;
                self.expr(e, 0, false, indent);
                self.w(";");
            }
            StmtKind::Return(None) => self.w("return;"),
            StmtKind::Return(Some(e)) => {
                self.w("return ");
                self.expr(e, 0, false, indent);
                self.w(";");
            }
            StmtKind::If(i) => self.if_stmt(i, indent),
            StmtKind::While { cond, body } => {
                self.w("while ");
                self.expr(cond, 0, true, indent);
                self.w(" ");
                self.block(body, indent, false);
            }
            StmtKind::Match {
                scrutinee,
                arms,
                end_line,
            } => self.stmt_match(scrutinee, arms, *end_line, indent),
        }
    }

    fn if_stmt(&mut self, i: &IfStmt, indent: usize) {
        let mut cur = i;
        loop {
            self.w("if ");
            self.expr(&cur.cond, 0, true, indent);
            self.w(" ");
            self.block(&cur.then, indent, false);
            match cur.otherwise.as_deref() {
                None => break,
                Some(Else::Block(b)) => {
                    self.w(" else ");
                    self.block(b, indent, false);
                    break;
                }
                Some(Else::If(next)) => {
                    self.w(" else ");
                    cur = next;
                }
            }
        }
    }

    fn stmt_match(&mut self, scrutinee: &Expr, arms: &[StmtArm], end_line: u32, indent: usize) {
        // すべての腕が `return 式;` だけなら `return match ...;`（ADR 0009 の決定 7）。
        // 腕が、同じ条件を満たす入れ子の `match` だけのときも、その `match` は式として書く（整形の結果が不動点になる）。
        let all_return = arms.iter().all(|a| returns_only(&a.body));
        self.w(if all_return { "return " } else { "" });
        self.match_body(scrutinee, arms, end_line, indent, all_return);
        self.w(if all_return { ";" } else { "" });
    }

    fn match_body(
        &mut self,
        scrutinee: &Expr,
        arms: &[StmtArm],
        end_line: u32,
        indent: usize,
        as_expr: bool,
    ) {
        self.w("match ");
        self.expr(scrutinee, 0, true, indent);
        self.w(" {");
        for a in arms {
            self.plain_start(a.line, indent + 1);
            self.pattern(&a.pattern);
            self.w(" => ");
            if as_expr {
                match &a.body.stmts[0].kind {
                    StmtKind::Return(Some(e)) => self.expr(e, 0, false, indent + 1),
                    StmtKind::Match {
                        scrutinee,
                        arms,
                        end_line,
                    } => self.match_body(scrutinee, arms, *end_line, indent + 1, true),
                    _ => {}
                }
                self.w(",");
            } else {
                self.block(&a.body, indent + 1, false);
            }
        }
        let mut first = false;
        self.comments_before(end_line, indent + 1, false, &mut first);
        self.nl(indent, end_line);
        self.w("}");
    }

    // ---- 式 ----

    /// 括弧は必要なものだけ書く。`min` は文脈が求める最低の段、`ns` は条件・`match` の
    /// 対象の中（構造体の値に括弧が要る）。
    pub(super) fn expr(&mut self, e: &Expr, min: u8, ns: bool, indent: usize) {
        self.expr_ctx(e, min, ns, false, indent);
    }

    pub(super) fn expr_ctx(&mut self, e: &Expr, min: u8, ns: bool, force: bool, indent: usize) {
        let e = strip(e);
        let lead = std::mem::take(&mut self.lead);
        let wrap = force
            || level(e) < min
            || (ns && matches!(e, Expr::StructLit { .. }))
            || (lead && matches!(e, Expr::Match { .. }))
            || (self.full && level(e) < PRIMARY);
        if wrap {
            self.w("(");
            self.expr_inner(e, false, indent);
            self.w(")");
        } else {
            // 最初に出力する子が文の先頭に残る式だけ、先頭であることを引き継ぐ。
            self.lead = lead
                && matches!(
                    e,
                    Expr::Field { .. } | Expr::Call { .. } | Expr::Try(_) | Expr::Binary { .. }
                );
            self.expr_inner(e, ns, indent);
            self.lead = false;
        }
    }

    fn args(&mut self, es: &[Expr], indent: usize) {
        for (i, a) in es.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.expr(a, 0, false, indent);
        }
    }

    fn expr_inner(&mut self, e: &Expr, ns: bool, indent: usize) {
        match e {
            Expr::Int(s) => self.w(s),
            Expr::Float(s) => self.w(&float_str(s)),
            Expr::Str(s) => self.w(&escape_str(s)),
            Expr::Bool(b) => self.w(if *b { "true" } else { "false" }),
            Expr::Name(n) => self.w(n),
            Expr::Result => self.w("result"),
            Expr::SelfValue => self.w("self"),
            Expr::Old(x) => {
                self.w("old(");
                self.expr(x, 0, false, indent);
                self.w(")");
            }
            Expr::Paren(x) => self.expr_ctx(x, 0, ns, false, indent),
            Expr::Tuple(items) => {
                self.w("(");
                self.args(items, indent);
                self.w(")");
            }
            Expr::Unary { op, expr } => {
                self.w(match op {
                    UnOp::Not => "!",
                    UnOp::Neg => "-",
                    UnOp::Ref => "&",
                    UnOp::RefMut => "&mut ",
                    UnOp::BitNot => "~",
                });
                // `&` が続くと `&&` になるので、参照の参照は括弧を残す。
                let force = matches!(op, UnOp::Ref | UnOp::RefMut)
                    && matches!(
                        strip(expr),
                        Expr::Unary {
                            op: UnOp::Ref | UnOp::RefMut,
                            ..
                        }
                    );
                self.expr_ctx(expr, PREFIX, ns, force, indent);
            }
            Expr::Binary { op, lhs, rhs } => {
                let p = binop_level(*op);
                let (lmin, rmin) = if p == CMP { (ADD, ADD) } else { (p, p + 1) };
                // `&&` と `||` は括弧なしで混ぜられない。
                let mixed = |x: &Expr| matches!(strip(x), Expr::Binary { op: o, .. } if is_logic(*o) && is_logic(*op) && o != op);
                self.expr_ctx(lhs, lmin, ns, mixed(lhs), indent);
                self.w(&format!(" {} ", binop_str(*op)));
                self.expr_ctx(rhs, rmin, ns, mixed(rhs), indent);
            }
            Expr::Field { expr, name } => {
                // `1.abs` は字句エラーなので整数の基には括弧が要る。
                let force = matches!(strip(expr), Expr::Int(_) | Expr::StructLit { .. });
                self.expr_ctx(expr, POSTFIX, ns, force, indent);
                self.w(".");
                self.w(name);
            }
            Expr::Call { callee, args } => {
                let force = matches!(strip(callee), Expr::StructLit { .. });
                self.expr_ctx(callee, POSTFIX, ns, force, indent);
                self.w("(");
                self.args(args, indent);
                self.w(")");
            }
            Expr::Try(x) => {
                let force = matches!(strip(x), Expr::StructLit { .. });
                self.expr_ctx(x, POSTFIX, ns, force, indent);
                self.w("?");
            }
            Expr::StructLit { path, fields } => {
                self.w(&path.join("."));
                self.w(" { ");
                for (i, (n, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.w(&format!("{n}: "));
                    self.expr(v, 0, false, indent);
                }
                self.w(" }");
            }
            Expr::Match {
                scrutinee,
                arms,
                end_line,
            } => {
                self.w("match ");
                self.expr(scrutinee, 0, true, indent);
                self.w(" {");
                for a in arms {
                    self.plain_start(a.line, indent + 1);
                    self.pattern(&a.pattern);
                    self.w(" => ");
                    self.expr(&a.body, 0, false, indent + 1);
                    self.w(",");
                }
                let mut first = false;
                self.comments_before(*end_line, indent + 1, false, &mut first);
                self.nl(indent, *end_line);
                self.w("}");
            }
            Expr::Closure {
                is_move,
                params,
                body,
            } => {
                if *is_move {
                    self.w("move ");
                }
                self.w("|");
                self.w(&params.join(", "));
                self.w("| ");
                match body {
                    ClosureBody::Expr(x) => self.expr(x, 0, ns, indent),
                    // `|x| { return e; }` は `|x| e`（ADR 0008 の決定 2）。
                    ClosureBody::Block(b) => match b.stmts.as_slice() {
                        [Stmt {
                            kind: StmtKind::Return(Some(x)),
                            ..
                        }] => self.expr(x, 0, ns, indent),
                        _ => self.block(b, indent, false),
                    },
                }
            }
        }
    }
}

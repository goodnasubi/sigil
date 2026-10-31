use std::collections::HashMap;

use super::Ty;
use crate::ast::*;

/// 式の中の名前を、対応表で置き換える（α 同値の比較用。ADR 0022 決定 2）。
/// クロージャの引数と `match` の腕の束縛が隠す名前は置き換えない。
pub(super) fn rename_names(e: &Expr, map: &HashMap<String, String>) -> Expr {
    rename_names_in(e, map, &mut vec![])
}

/// `rename_names` の、隠している名前を引き継ぐ版。
fn rename_names_in(e: &Expr, map: &HashMap<String, String>, bound: &mut Vec<String>) -> Expr {
    fn go(e: &Expr, map: &HashMap<String, String>, bound: &mut Vec<String>) -> Expr {
        match e {
            Expr::Name(n) => {
                if bound.iter().any(|b| b == n) {
                    e.clone()
                } else if let Some(m) = map.get(n) {
                    Expr::Name(m.clone())
                } else {
                    e.clone()
                }
            }
            Expr::Old(x) => Expr::Old(Box::new(go(x, map, bound))),
            Expr::Paren(x) => Expr::Paren(Box::new(go(x, map, bound))),
            Expr::Try(x) => Expr::Try(Box::new(go(x, map, bound))),
            Expr::Tuple(xs) => Expr::Tuple(xs.iter().map(|x| go(x, map, bound)).collect()),
            Expr::Unary { op, expr: x } => Expr::Unary {
                op: *op,
                expr: Box::new(go(x, map, bound)),
            },
            Expr::Binary { op, lhs, rhs } => Expr::Binary {
                op: *op,
                lhs: Box::new(go(lhs, map, bound)),
                rhs: Box::new(go(rhs, map, bound)),
            },
            Expr::Field { expr: x, name } => Expr::Field {
                expr: Box::new(go(x, map, bound)),
                name: name.clone(),
            },
            Expr::Call { callee, args } => Expr::Call {
                callee: Box::new(go(callee, map, bound)),
                args: args.iter().map(|a| go(a, map, bound)).collect(),
            },
            Expr::StructLit { path, fields } => Expr::StructLit {
                path: path.clone(),
                fields: fields
                    .iter()
                    .map(|(n, x)| (n.clone(), go(x, map, bound)))
                    .collect(),
            },
            Expr::Match {
                scrutinee,
                arms,
                end_line,
            } => Expr::Match {
                scrutinee: Box::new(go(scrutinee, map, bound)),
                arms: arms
                    .iter()
                    .map(|a| {
                        let mut ns = vec![];
                        pat_names(&a.pattern, &mut ns);
                        let at = bound.len();
                        bound.extend(ns);
                        let body = go(&a.body, map, bound);
                        bound.truncate(at);
                        ExprArm {
                            pattern: a.pattern.clone(),
                            body,
                            line: a.line,
                        }
                    })
                    .collect(),
                end_line: *end_line,
            },
            Expr::Closure {
                is_move,
                params,
                body,
            } => {
                let at = bound.len();
                bound.extend(params.iter().cloned());
                let body = match body {
                    ClosureBody::Expr(x) => ClosureBody::Expr(Box::new(go(x, map, bound))),
                    ClosureBody::Block(b) => ClosureBody::Block(rename_block(b, map, bound)),
                };
                bound.truncate(at);
                Expr::Closure {
                    is_move: *is_move,
                    params: params.clone(),
                    body,
                }
            }
            other => other.clone(),
        }
    }
    go(e, map, bound)
}

/// `rename_names` のブロック版（`let` の束縛も隠す）。
fn rename_block(b: &Block, map: &HashMap<String, String>, bound: &mut Vec<String>) -> Block {
    // ブロックの中の式は `rename_names` に入り直す（束縛の共有のため、ここでは文の形だけをたどる）。
    fn stmt(s: &Stmt, map: &HashMap<String, String>, bound: &mut Vec<String>) -> Stmt {
        let kind = match &s.kind {
            StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
            } => {
                let value = rename_names_in(value, map, bound);
                let mut ns = vec![];
                pat_names(pattern, &mut ns);
                bound.extend(ns);
                StmtKind::Let {
                    mutable: *mutable,
                    pattern: pattern.clone(),
                    ty: ty.clone(),
                    value,
                }
            }
            StmtKind::Assign { target, value } => StmtKind::Assign {
                target: rename_names_in(target, map, bound),
                value: rename_names_in(value, map, bound),
            },
            StmtKind::Expr(e) => StmtKind::Expr(rename_names_in(e, map, bound)),
            StmtKind::Return(e) => {
                StmtKind::Return(e.as_ref().map(|e| rename_names_in(e, map, bound)))
            }
            StmtKind::If(i) => StmtKind::If(rename_if(i, map, bound)),
            StmtKind::While { cond, body } => StmtKind::While {
                cond: rename_names_in(cond, map, bound),
                body: rename_block(body, map, bound),
            },
            StmtKind::Match {
                scrutinee,
                arms,
                end_line,
            } => StmtKind::Match {
                scrutinee: rename_names_in(scrutinee, map, bound),
                arms: arms
                    .iter()
                    .map(|a| {
                        let mut ns = vec![];
                        pat_names(&a.pattern, &mut ns);
                        let at = bound.len();
                        bound.extend(ns);
                        let body = rename_block(&a.body, map, bound);
                        bound.truncate(at);
                        StmtArm {
                            pattern: a.pattern.clone(),
                            body,
                            line: a.line,
                        }
                    })
                    .collect(),
                end_line: *end_line,
            },
        };
        Stmt { kind, span: s.span }
    }
    fn rename_if(i: &IfStmt, map: &HashMap<String, String>, bound: &mut Vec<String>) -> IfStmt {
        IfStmt {
            line: i.line,
            cond: rename_names_in(&i.cond, map, bound),
            then: rename_block(&i.then, map, bound),
            otherwise: match i.otherwise.as_deref() {
                Some(Else::If(n)) => Some(Box::new(Else::If(rename_if(n, map, bound)))),
                Some(Else::Block(b)) => Some(Box::new(Else::Block(rename_block(b, map, bound)))),
                None => None,
            },
        }
    }
    let at = bound.len();
    let stmts = b.stmts.iter().map(|s| stmt(s, map, bound)).collect();
    bound.truncate(at);
    Block {
        stmts,
        open_line: b.open_line,
        end_line: b.end_line,
    }
}

/// 式を最上位の `&&` で割り、条件の並びにする（括弧は剥がす）。
pub(super) fn conjuncts(e: &Expr, out: &mut Vec<Expr>) {
    match e {
        Expr::Paren(x) => conjuncts(x, out),
        Expr::Binary {
            op: BinOp::And,
            lhs,
            rhs,
        } => {
            conjuncts(lhs, out);
            conjuncts(rhs, out);
        }
        other => out.push(other.clone()),
    }
}

/// 本体の中の呼び出しの、呼ばれる側の名前（関数名・メソッド名）を集める（多相再帰の検査用）。
pub(super) fn collect_calls_block(b: &Block, out: &mut Vec<String>) {
    for s in &b.stmts {
        collect_calls_stmt(s, out);
    }
}

fn collect_calls_stmt(s: &Stmt, out: &mut Vec<String>) {
    match &s.kind {
        StmtKind::Let { value, .. } => collect_calls_expr(value, out),
        StmtKind::Assign { target, value } => {
            collect_calls_expr(target, out);
            collect_calls_expr(value, out);
        }
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => collect_calls_expr(e, out),
        StmtKind::Return(None) => {}
        StmtKind::If(i) => collect_calls_if(i, out),
        StmtKind::While { cond, body } => {
            collect_calls_expr(cond, out);
            collect_calls_block(body, out);
        }
        StmtKind::Match {
            scrutinee, arms, ..
        } => {
            collect_calls_expr(scrutinee, out);
            for a in arms {
                collect_calls_block(&a.body, out);
            }
        }
    }
}

fn collect_calls_if(i: &IfStmt, out: &mut Vec<String>) {
    collect_calls_expr(&i.cond, out);
    collect_calls_block(&i.then, out);
    match i.otherwise.as_deref() {
        Some(Else::If(n)) => collect_calls_if(n, out),
        Some(Else::Block(b)) => collect_calls_block(b, out),
        None => {}
    }
}

fn collect_calls_expr(e: &Expr, out: &mut Vec<String>) {
    match e {
        Expr::Call { callee, args } => {
            match &**callee {
                Expr::Name(n) => out.push(n.clone()),
                Expr::Field { name, .. } => out.push(name.clone()),
                other => collect_calls_expr(other, out),
            }
            collect_calls_expr(callee, out);
            for a in args {
                collect_calls_expr(a, out);
            }
        }
        Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => collect_calls_expr(x, out),
        Expr::Tuple(xs) => xs.iter().for_each(|x| collect_calls_expr(x, out)),
        Expr::Unary { expr: x, .. } | Expr::Field { expr: x, .. } => collect_calls_expr(x, out),
        Expr::Binary { lhs, rhs, .. } => {
            collect_calls_expr(lhs, out);
            collect_calls_expr(rhs, out);
        }
        Expr::StructLit { fields, .. } => {
            fields.iter().for_each(|(_, x)| collect_calls_expr(x, out))
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_calls_expr(scrutinee, out);
            for a in arms {
                collect_calls_expr(&a.body, out);
            }
        }
        Expr::Closure { body, .. } => match body {
            ClosureBody::Expr(x) => collect_calls_expr(x, out),
            ClosureBody::Block(b) => collect_calls_block(b, out),
        },
        _ => {}
    }
}

pub(super) fn strip_paren(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(i) => strip_paren(i),
        e => e,
    }
}

/// 型から参照を外した形。
pub(super) fn strip_refs(t: &Ty) -> Ty {
    match t {
        Ty::Ref(_, i) => strip_refs(i),
        Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(strip_refs).collect()),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(strip_refs).collect()),
        Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(strip_refs).collect(), Box::new(strip_refs(r))),
        t => t.clone(),
    }
}

/// 型のどこかに参照を含むか。
pub(super) fn contains_ref(t: &Ty) -> bool {
    match t {
        Ty::Ref(..) => true,
        Ty::Named(_, a) => a.iter().any(contains_ref),
        Ty::Tuple(ts) => ts.iter().any(contains_ref),
        Ty::Fn(ps, r) => ps.iter().any(contains_ref) || contains_ref(r),
        _ => false,
    }
}

/// 関数の型の中を数えずに、型のどこかに参照を含むか（借用の引数の判定）。
pub(super) fn contains_ref_outside_fn(t: &Ty) -> bool {
    match t {
        Ty::Ref(..) => true,
        Ty::Named(_, a) => a.iter().any(contains_ref_outside_fn),
        Ty::Tuple(ts) => ts.iter().any(contains_ref_outside_fn),
        _ => false,
    }
}

/// 場所の式（変数とそのフィールドの連なり）の根の変数と、`a.b.c` の形の経路。
pub(super) fn place_path(e: &Expr) -> Option<(String, String)> {
    match e {
        Expr::Name(n) => Some((n.clone(), n.clone())),
        Expr::SelfValue => Some(("self".into(), "self".into())),
        Expr::Field { expr, name } => {
            let (r, p) = place_path(expr)?;
            Some((r, format!("{p}.{name}")))
        }
        Expr::Paren(i) => place_path(i),
        _ => None,
    }
}

pub(super) fn place_root(e: &Expr) -> Option<&str> {
    match e {
        Expr::Name(n) => Some(n),
        Expr::SelfValue => Some("self"),
        Expr::Field { expr, .. } => place_root(expr),
        Expr::Paren(i) => place_root(i),
        _ => None,
    }
}

/// パターンが束縛する名前。
pub(super) fn pat_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Bind(n) => out.push(n.clone()),
        Pattern::Variant { args, .. } => args.iter().for_each(|a| pat_names(a, out)),
        Pattern::Tuple(ps) => ps.iter().for_each(|a| pat_names(a, out)),
        _ => {}
    }
}

/// 基の型のリテラルの定数か（`-` を前置した数も含む）。
pub(super) fn is_literal(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => true,
        Expr::Paren(i) => is_literal(i),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
        } => matches!(**expr, Expr::Int(_) | Expr::Float(_)),
        _ => false,
    }
}

pub(super) fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
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

pub(super) fn block_returns(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::If(i) => if_returns(i),
        StmtKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|a| block_returns(&a.body))
        }
        _ => false,
    })
}

fn if_returns(i: &IfStmt) -> bool {
    block_returns(&i.then)
        && match i.otherwise.as_deref() {
            None => false,
            Some(Else::If(n)) => if_returns(n),
            Some(Else::Block(b)) => block_returns(b),
        }
}

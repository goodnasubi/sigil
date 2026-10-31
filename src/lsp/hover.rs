//! hover（ADR 0019 の決定 5）。名前の解決は構文だけで行う。返すのは `sigil spec` の塊（`SpecItem::text`）そのもの。

use crate::ast::*;
use crate::lexer::{lex, Tok, Token};
use crate::spec::{Body, Spec};

/// 局所の名前が隠す行の範囲（両端を含む）。
type Scope = (String, u32, u32);

fn pat_names(p: &Pattern, out: &mut Vec<String>) {
    match p {
        Pattern::Bind(n) => out.push(n.clone()),
        Pattern::Variant { args, .. } | Pattern::Tuple(args) => {
            args.iter().for_each(|a| pat_names(a, out));
        }
        _ => {}
    }
}

fn bind(p: &Pattern, from: u32, to: u32, out: &mut Vec<Scope>) {
    let mut ns = vec![];
    pat_names(p, &mut ns);
    out.extend(ns.into_iter().map(|n| (n, from, to)));
}

fn block(b: &Block, all: (u32, u32), out: &mut Vec<Scope>) {
    for s in &b.stmts {
        match &s.kind {
            StmtKind::Let { pattern, value, .. } => {
                bind(pattern, s.span.line, b.end_line, out);
                expr(value, all, out);
            }
            StmtKind::Assign { target, value } => {
                expr(target, all, out);
                expr(value, all, out);
            }
            StmtKind::Expr(e) | StmtKind::Return(Some(e)) => expr(e, all, out),
            StmtKind::Return(None) => {}
            StmtKind::If(i) => if_stmt(i, all, out),
            StmtKind::While { cond, body } => {
                expr(cond, all, out);
                block(body, all, out);
            }
            StmtKind::Match {
                scrutinee,
                arms,
                end_line,
            } => {
                expr(scrutinee, all, out);
                for a in arms {
                    bind(&a.pattern, a.line, *end_line, out);
                    block(&a.body, all, out);
                }
            }
        }
    }
}

fn if_stmt(i: &IfStmt, all: (u32, u32), out: &mut Vec<Scope>) {
    expr(&i.cond, all, out);
    block(&i.then, all, out);
    match i.otherwise.as_deref() {
        Some(Else::If(n)) => if_stmt(n, all, out),
        Some(Else::Block(b)) => block(b, all, out),
        None => {}
    }
}

/// クロージャの引数は、位置が分からないので関数の全体を隠す（隠しすぎる側に倒す）。
fn expr(e: &Expr, all: (u32, u32), out: &mut Vec<Scope>) {
    match e {
        Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => expr(x, all, out),
        Expr::Unary { expr: x, .. } | Expr::Field { expr: x, .. } => expr(x, all, out),
        Expr::Tuple(xs) => xs.iter().for_each(|x| expr(x, all, out)),
        Expr::Binary { lhs, rhs, .. } => {
            expr(lhs, all, out);
            expr(rhs, all, out);
        }
        Expr::Call { callee, args } => {
            expr(callee, all, out);
            args.iter().for_each(|x| expr(x, all, out));
        }
        Expr::StructLit { fields, .. } => fields.iter().for_each(|(_, x)| expr(x, all, out)),
        Expr::Match {
            scrutinee,
            arms,
            end_line,
        } => {
            expr(scrutinee, all, out);
            for (i, a) in arms.iter().enumerate() {
                let to = arms.get(i + 1).map_or(*end_line, |n| n.line);
                bind(&a.pattern, a.line, to, out);
                expr(&a.body, all, out);
            }
        }
        Expr::Closure { params, body, .. } => {
            out.extend(params.iter().map(|p| (p.clone(), all.0, all.1)));
            match body {
                ClosureBody::Expr(x) => expr(x, all, out),
                ClosureBody::Block(b) => block(b, all, out),
            }
        }
        _ => {}
    }
}

fn fns(file: &File) -> Vec<&FnDecl> {
    let mut v = vec![];
    for it in &file.items {
        match it {
            Item::Fn(f) => v.push(f),
            Item::Impl(i) => v.extend(i.fns.iter()),
            Item::ImplFor(i) => v.extend(i.fns.iter()),
            Item::Trait(t) => v.extend(t.fns.iter()),
            _ => {}
        }
    }
    v
}

fn shadowed(f: &FnDecl, name: &str, line: u32) -> bool {
    let all = (f.span.line, f.body.end_line);
    let mut sc: Vec<Scope> = f
        .params
        .iter()
        .map(|p| (p.name.clone(), all.0, all.1))
        .collect();
    block(&f.body, all, &mut sc);
    sc.iter()
        .any(|(n, a, b)| n == name && *a <= line && line <= *b)
}

fn ident(t: Option<&Token>) -> Option<&str> {
    match t.map(|t| &t.tok) {
        Some(Tok::Ident(n)) => Some(n),
        _ => None,
    }
}

fn is_punct(t: Option<&Token>, p: &str) -> bool {
    matches!(t.map(|t| &t.tok), Some(Tok::Punct(q)) if *q == p)
}

/// `(line, col)`（1 始まり、列は文字数）の位置の hover の文字列。返さないときは `None`。
/// `own` はこの文書の仕様、`all` はこの文書から `use` で辿れるモジュールすべての仕様。
pub fn hover(text: &str, line: u32, col: u32, own: &Spec, all: &[Spec]) -> Option<String> {
    let file = crate::parse(text).ok()?;
    let toks = lex(text).ok()?.tokens;
    let under = toks.iter().position(|t| {
        let n = match &t.tok {
            Tok::Ident(n) => n.chars().count() as u32,
            _ => 0,
        };
        t.span.line == line && t.span.col <= col && col < t.span.col + n
    });
    let cursor_tok = under.map(|i| &toks[i]);
    let item_at = |span: crate::error::Span| own.items.iter().find(|i| i.span == span);
    let all_items = || all.iter().flat_map(|s| s.items.iter());
    // 宣言: `fn` から本体の `{` の前まで。
    for f in fns(&file) {
        let open = f.body.open_line;
        // 本体の `{` は、開き行の最初の `{`。その位置とそれより右（1 行の本体）は宣言ではない。
        let brace = toks
            .iter()
            .find(|t| t.span.line == open && matches!(&t.tok, Tok::Punct("{")))
            .map_or(u32::MAX, |t| t.span.col);
        let right = line == open && col >= brace;
        if f.span.line <= line && line <= open && !right {
            return item_at(f.span).map(|i| i.text());
        }
    }
    // 型・構造体・列挙型の宣言の名前。
    if let Some(Tok::Ident(n)) = cursor_tok.map(|t| &t.tok) {
        let decl = own
            .items
            .iter()
            .find(|i| i.span.line == line && i.name == *n && !matches!(i.body, Body::Fn { .. }));
        if let Some(i) = decl {
            return Some(i.text());
        }
    }
    // 呼び出しの箇所。
    let i = under?;
    let name = ident(toks.get(i))?;
    if !is_punct(toks.get(i + 1), "(") {
        return None;
    }
    let encl = fns(&file)
        .into_iter()
        .find(|f| f.span.line <= line && line <= f.body.end_line)?;
    let want = if i >= 2 && is_punct(toks.get(i - 1), ".") {
        let recv = ident(toks.get(i - 2))?;
        // 値の上のメソッド呼び出し（`x.m(...)`）は返さない。受け手が型の名前（大文字）で、局所でないときだけ。
        if !recv.starts_with(|c: char| c.is_uppercase()) || shadowed(encl, recv, line) {
            return None;
        }
        format!("{recv}.{name}")
    } else {
        if shadowed(encl, name, line) {
            return None;
        }
        name.to_string()
    };
    if let Some(i) = all_items().find(|i| i.name == want && matches!(i.body, Body::Fn { .. })) {
        return Some(i.text());
    }
    // `トレイト.m(x)` の修飾形（ADR 0022 決定 2）: 受け手がトレイトの名前なら、
    // そのトレイトの仕様（関数の仕様を含む）を示す。
    if let Some((tr, _)) = want.split_once('.') {
        return all_items()
            .find(|i| {
                i.name == tr
                    && matches!(&i.body, Body::Trait { fns } if fns.iter().any(|(n, _)| n == name))
            })
            .map(|i| i.text());
    }
    None
}

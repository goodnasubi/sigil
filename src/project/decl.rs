//! 型の名前の収集、シグネチャの検査、名前の使用状況の判定。

use std::collections::HashSet;

use crate::ast::*;
use crate::error::Span;

pub(super) fn item_name(i: &Item) -> Option<&str> {
    match i {
        Item::Type(t) => Some(&t.name),
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::Fn(f) => Some(&f.name),
        Item::Trait(t) => Some(&t.name),
        Item::Impl(_) | Item::ImplFor(_) => None,
    }
}

pub(super) fn find_decl<'a>(f: &'a File, name: &str) -> Option<&'a Item> {
    f.items.iter().find(|i| item_name(i) == Some(name))
}

pub(super) fn decl_is_pub(i: &Item) -> bool {
    match i {
        Item::Type(t) => t.is_pub,
        Item::Struct(s) => s.is_pub,
        Item::Enum(e) => e.is_pub,
        Item::Fn(f) => f.is_pub,
        Item::Trait(t) => t.is_pub,
        Item::Impl(_) | Item::ImplFor(_) => false,
    }
}

// ---- 型の名前の収集 ----

pub(super) fn type_heads(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::Path { path, args } => {
            out.push(path[0].clone());
            args.iter().for_each(|a| type_heads(a, out));
        }
        Type::Ref { inner, .. } => type_heads(inner, out),
        Type::Fn { params, ret, .. } => {
            params.iter().for_each(|p| type_heads(p, out));
            if let Some(r) = ret {
                type_heads(r, out);
            }
        }
        Type::Tuple(ts) => ts.iter().for_each(|t| type_heads(t, out)),
    }
}

pub(super) fn fn_sig_heads(f: &FnDecl, outer: &[Generic], out: &mut Vec<String>) {
    let mut hs = vec![];
    f.params.iter().for_each(|p| type_heads(&p.ty, &mut hs));
    if let Some(r) = &f.ret {
        type_heads(r, &mut hs);
    }
    hs.retain(|h| !f.generics.iter().any(|g| g.name == *h) && !outer.iter().any(|g| g.name == *h));
    out.extend(hs);
}

/// 宣言のシグネチャ（本体を除く）に現れる型の名前。型引数の名前は除く。
pub(super) fn decl_type_heads(i: &Item, out: &mut Vec<String>) {
    let mut hs = vec![];
    match i {
        Item::Fn(f) => fn_sig_heads(f, &[], &mut hs),
        Item::Struct(s) => {
            s.fields.iter().for_each(|f| type_heads(&f.ty, &mut hs));
            hs.retain(|h| !s.generics.iter().any(|g| g.name == *h));
        }
        Item::Enum(e) => {
            e.variants
                .iter()
                .for_each(|v| v.fields.iter().for_each(|t| type_heads(t, &mut hs)));
            hs.retain(|h| !e.generics.iter().any(|g| g.name == *h));
        }
        Item::Type(t) => type_heads(&t.base, &mut hs),
        Item::Impl(im) => {
            for f in &im.fns {
                fn_sig_heads(f, &im.generics, &mut hs);
            }
        }
        Item::Trait(t) => {
            for f in &t.fns {
                fn_sig_heads(f, &[], &mut hs);
            }
            // トレイトの関数に出る `Self` は宣言の名前ではないので除く。
            hs.retain(|h| h != "Self");
        }
        Item::ImplFor(im) => {
            hs.push(im.trait_name.clone());
            type_heads(&im.ty, &mut hs);
            for f in &im.fns {
                fn_sig_heads(f, &im.generics, &mut hs);
            }
        }
    }
    out.extend(hs);
}

/// 公開の宣言のシグネチャが、同じモジュールの非公開の型を名指ししていれば、その位置と説明
/// （ADR 0021 の決定 4。仕様に出るシグネチャが、取り込めない型を指さないようにする）。
pub fn private_in_public(file: &File) -> Option<(Span, String)> {
    let private: HashSet<&str> = file
        .items
        .iter()
        .filter(|i| !matches!(i, Item::Fn(_) | Item::Impl(_)) && !decl_is_pub(i))
        .filter_map(item_name)
        .collect();
    if private.is_empty() {
        return None;
    }
    let hit = |what: String, span: Span, heads: Vec<String>| {
        heads.iter().find(|h| private.contains(h.as_str())).map(|h| {
            (
                span,
                format!("公開の{what}のシグネチャが、非公開の型 `{h}` を名指ししている（`{h}` にも `pub` を付けるか、公開をやめる）"),
            )
        })
    };
    for i in &file.items {
        let r = match i {
            Item::Fn(f) if f.is_pub => {
                let mut hs = vec![];
                fn_sig_heads(f, &[], &mut hs);
                hit(format!("関数 `{}`", f.name), f.span, hs)
            }
            Item::Struct(s) if s.is_pub => {
                let mut hs = vec![];
                decl_type_heads(i, &mut hs);
                hit(format!("構造体 `{}`", s.name), s.span, hs)
            }
            Item::Enum(e) if e.is_pub => {
                let mut hs = vec![];
                decl_type_heads(i, &mut hs);
                hit(format!("列挙型 `{}`", e.name), e.span, hs)
            }
            Item::Type(t) if t.is_pub => {
                let mut hs = vec![];
                decl_type_heads(i, &mut hs);
                hit(format!("型 `{}`", t.name), t.span, hs)
            }
            Item::Impl(im) => {
                let head_private =
                    matches!(&im.ty, Type::Path { path, .. } if private.contains(path[0].as_str()));
                if head_private {
                    None
                } else {
                    im.fns.iter().filter(|f| f.is_pub).find_map(|f| {
                        let mut hs = vec![];
                        fn_sig_heads(f, &im.generics, &mut hs);
                        hit(format!("メソッド `{}`", f.name), f.span, hs)
                    })
                }
            }
            _ => None,
        };
        if r.is_some() {
            return r;
        }
    }
    None
}

// ---- 名前の使用の収集（未使用の `use` の検出。字面で見る。同じ名前の局所変数があれば使用に数える）----

/// 呼び出しの形（`受け手.名前(...)`・`型.名前(...)`）で現れる名前の集合。
/// トレイトの取り込みが「使われている」かの判定に使う（メソッドの呼び出しはトレイトの名前を書かない）。
pub(super) fn field_call_names(file: &File) -> HashSet<String> {
    fn ex(e: &Expr, out: &mut HashSet<String>) {
        match e {
            Expr::Call { callee, args } => {
                if let Expr::Field { name, .. } = callee.as_ref() {
                    out.insert(name.clone());
                }
                ex(callee, out);
                args.iter().for_each(|a| ex(a, out));
            }
            Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => ex(x, out),
            Expr::Tuple(xs) => xs.iter().for_each(|x| ex(x, out)),
            Expr::Unary { expr: x, .. } => ex(x, out),
            Expr::Binary { lhs, rhs, .. } => {
                ex(lhs, out);
                ex(rhs, out);
            }
            Expr::Field { expr: x, .. } => ex(x, out),
            Expr::StructLit { fields, .. } => fields.iter().for_each(|(_, x)| ex(x, out)),
            Expr::Match {
                scrutinee, arms, ..
            } => {
                ex(scrutinee, out);
                for a in arms {
                    ex(&a.body, out);
                }
            }
            Expr::Closure { body, .. } => match body {
                ClosureBody::Expr(x) => ex(x, out),
                ClosureBody::Block(b) => bl(b, out),
            },
            _ => {}
        }
    }
    fn bl(b: &Block, out: &mut HashSet<String>) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { value, .. } => ex(value, out),
                StmtKind::Assign { target, value } => {
                    ex(target, out);
                    ex(value, out);
                }
                StmtKind::Expr(e) | StmtKind::Return(Some(e)) => ex(e, out),
                StmtKind::Return(None) => {}
                StmtKind::If(i) => {
                    ex(&i.cond, out);
                    bl(&i.then, out);
                    match i.otherwise.as_deref() {
                        Some(Else::If(n)) => {
                            ex(&n.cond, out);
                            bl(&n.then, out);
                        }
                        Some(Else::Block(b2)) => bl(b2, out),
                        None => {}
                    }
                }
                StmtKind::While { cond, body } => {
                    ex(cond, out);
                    bl(body, out);
                }
                StmtKind::Match {
                    scrutinee, arms, ..
                } => {
                    ex(scrutinee, out);
                    for a in arms {
                        bl(&a.body, out);
                    }
                }
            }
        }
    }
    let mut out = HashSet::new();
    for i in &file.items {
        let fns: Vec<&FnDecl> = match i {
            Item::Fn(f) => vec![f],
            Item::Impl(im) => im.fns.iter().collect(),
            Item::ImplFor(im) => im.fns.iter().collect(),
            _ => vec![],
        };
        for f in fns {
            if let Some(e) = &f.requires {
                ex(e, &mut out);
            }
            if let Some(e) = &f.ensures {
                ex(e, &mut out);
            }
            bl(&f.body, &mut out);
        }
    }
    out
}

pub(super) fn used_names(file: &File) -> HashSet<String> {
    let mut u = HashSet::new();
    for i in &file.items {
        match i {
            Item::Type(t) => {
                ty(&t.base, &mut u);
                expr(&t.cond, &mut u);
            }
            Item::Struct(s) => s.fields.iter().for_each(|f| ty(&f.ty, &mut u)),
            Item::Enum(e) => e
                .variants
                .iter()
                .for_each(|v| v.fields.iter().for_each(|t| ty(t, &mut u))),
            Item::Fn(f) => func(f, &mut u),
            Item::Impl(im) => {
                ty(&im.ty, &mut u);
                im.fns.iter().for_each(|f| func(f, &mut u));
            }
            Item::Trait(t) => t.fns.iter().for_each(|f| func(f, &mut u)),
            Item::ImplFor(im) => {
                u.insert(im.trait_name.clone());
                ty(&im.ty, &mut u);
                im.fns.iter().for_each(|f| func(f, &mut u));
            }
        }
    }
    u
}

fn ty(t: &Type, u: &mut HashSet<String>) {
    let mut hs = vec![];
    type_heads(t, &mut hs);
    u.extend(hs);
}

fn func(f: &FnDecl, u: &mut HashSet<String>) {
    f.params.iter().for_each(|p| ty(&p.ty, u));
    if let Some(r) = &f.ret {
        ty(r, u);
    }
    if let Some(e) = &f.requires {
        expr(e, u);
    }
    if let Some(e) = &f.ensures {
        expr(e, u);
    }
    block(&f.body, u);
}

fn block(b: &Block, u: &mut HashSet<String>) {
    b.stmts.iter().for_each(|s| stmt(s, u));
}

fn stmt(s: &Stmt, u: &mut HashSet<String>) {
    match &s.kind {
        StmtKind::Let {
            pattern,
            ty: t,
            value,
            ..
        } => {
            pat(pattern, u);
            if let Some(t) = t {
                ty(t, u);
            }
            expr(value, u);
        }
        StmtKind::Assign { target, value } => {
            expr(target, u);
            expr(value, u);
        }
        StmtKind::Expr(e) => expr(e, u),
        StmtKind::Return(e) => {
            if let Some(e) = e {
                expr(e, u);
            }
        }
        StmtKind::If(i) => if_stmt(i, u),
        StmtKind::While { cond, body } => {
            expr(cond, u);
            block(body, u);
        }
        StmtKind::Match {
            scrutinee, arms, ..
        } => {
            expr(scrutinee, u);
            for a in arms {
                pat(&a.pattern, u);
                block(&a.body, u);
            }
        }
    }
}

fn if_stmt(i: &IfStmt, u: &mut HashSet<String>) {
    expr(&i.cond, u);
    block(&i.then, u);
    match i.otherwise.as_deref() {
        Some(Else::If(n)) => if_stmt(n, u),
        Some(Else::Block(b)) => block(b, u),
        None => {}
    }
}

fn pat(p: &Pattern, u: &mut HashSet<String>) {
    match p {
        Pattern::Variant { path, args } => {
            u.insert(path[0].clone());
            args.iter().for_each(|a| pat(a, u));
        }
        Pattern::Tuple(ps) => ps.iter().for_each(|p| pat(p, u)),
        _ => {}
    }
}

fn expr(e: &Expr, u: &mut HashSet<String>) {
    match e {
        Expr::Name(n) => {
            u.insert(n.clone());
        }
        Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => expr(x, u),
        Expr::Tuple(xs) => xs.iter().for_each(|x| expr(x, u)),
        Expr::Unary { expr: x, .. } => expr(x, u),
        Expr::Binary { lhs, rhs, .. } => {
            expr(lhs, u);
            expr(rhs, u);
        }
        Expr::Field { expr: x, .. } => expr(x, u),
        Expr::Call { callee, args } => {
            expr(callee, u);
            args.iter().for_each(|a| expr(a, u));
        }
        Expr::StructLit { path, fields } => {
            u.insert(path[0].clone());
            fields.iter().for_each(|(_, x)| expr(x, u));
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr(scrutinee, u);
            for a in arms {
                pat(&a.pattern, u);
                expr(&a.body, u);
            }
        }
        Expr::Closure { body, .. } => match body {
            ClosureBody::Expr(x) => expr(x, u),
            ClosureBody::Block(b) => block(b, u),
        },
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Result
        | Expr::SelfValue => {}
    }
}

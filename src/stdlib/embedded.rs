//! 処理系に埋め込んだ Sigil 製の標準モジュール（ADR 0025 補足 A-6、PR 3-4h）。
//!
//! 埋め込みモジュールは、利用者のプログラムと同じ検査・lowering にかけて 1 つのプログラムに結ぶ。
//! `std.*` の `trust base` は処理系のソースに入っていることを根拠に許可する（`trust-base.list` を経ない）。
//!
//! - 経路は暗黙のみ。利用者の `module std.*` / `use std.*` は従来どおり拒否する（ADR 0016 決定 3、ADR 0021）。
//!   結ぶ側は `effects::Options::embedded_roots` で名指しする（3-4i が `raw.alloc` の差し替えで使う）。
//! - 名前は `std.` で始まるものに限る。結合後の名前も `std.` 始まりになり、検査器はそれを出所の印にする
//!   （利用者のモジュールは `std` で始まれず、`.` を含む名前は結合でしか生まれない）。
//! - 本番のレジストリ `EMBEDDED` には確保器 `std.os.alloc` がある（3-4i）。`raw.alloc`/`raw.free` を使い、
//!   入口の `targets` が `server` だけのプログラムに暗黙に結ぶ（補足 B-3・B-4。`implicit_roots`）。

use crate::ast::{ClosureBody, Else, Expr, File, FnDecl, IfStmt, Item, Stmt, StmtKind};

/// Sigil 製の確保器のモジュール名（ADR 0025 決定 3、補足 A-2・B-1）。
pub const ALLOC_MODULE: &str = "std.os.alloc";
/// 確保器の入口の関数（結合後の名前）。lowering は入口から辿れなくても SIR に入れ、ネイティブは
/// `sigil_rt_raw_alloc` / `sigil_rt_raw_free` / `sigil_rt_sigil_live_blocks` のシムから呼ぶ（補足 B-2・B-5）。
pub const ALLOC_FN: &str = "std.os.alloc.alloc";
pub const FREE_FN: &str = "std.os.alloc.free";
pub const LIVE_FN: &str = "std.os.alloc.live_blocks";

/// `Float` の余り（fmod）の Sigil 版のモジュール名と関数名（結合後の名前。ADR 0025 補足 C-3）。
/// 言語に `Float` の `%` はない（ADR 0011 の決定。検査器が拒否する）ので、プログラムには暗黙に結ばない。
/// 後続の PR で Sigil 製の標準ライブラリ（`Float` の表示・解析）が `use` して使う。
pub const FLOAT_MODULE: &str = "std.core.float";
pub const FMOD_FN: &str = "std.core.float.fmod";

/// 埋め込みモジュールの一覧（モジュール名, Sigil のソース）。
pub const EMBEDDED: &[(&str, &str)] = &[
    (ALLOC_MODULE, include_str!("os_alloc.sigil")),
    (FLOAT_MODULE, include_str!("core_float.sigil")),
];

/// 入口のファイルが暗黙に結ぶ埋め込みモジュール（補足 B-3・B-4）。
///
/// 原始操作は信頼基盤の入口のモジュールでしか呼べない（入口でない `trust base` は結合で拒否される）ので、
/// 入口のファイルだけを見ればよい。名前で見る（`raw` を変数名にした誤検出は確保器を余分に結ぶだけで害はない）。
/// 試験用の差し込み（`Options::raw_ops`）で `raw.alloc`/`raw.free` を置き換えたときは、本物の原始操作では
/// ないので結ばない。
pub fn implicit_roots(entry: &File, opts: &crate::effects::Options) -> Vec<&'static str> {
    let replaced = opts.raw_ops.iter().any(|r| {
        let name = r.signature.split('(').next().unwrap_or("").trim();
        name == "alloc" || name == "free"
    });
    if replaced {
        return vec![];
    }
    let server_only =
        entry.module.targets.iter().all(|t| t == "server") && !entry.module.targets.is_empty();
    if server_only && entry.items.iter().any(item_uses_raw_alloc) {
        vec![ALLOC_MODULE]
    } else {
        vec![]
    }
}

fn item_uses_raw_alloc(it: &Item) -> bool {
    match it {
        Item::Fn(f) => fn_uses(f),
        Item::Impl(i) => i.fns.iter().any(fn_uses),
        Item::ImplFor(i) => i.fns.iter().any(fn_uses),
        Item::Trait(t) => t.fns.iter().any(fn_uses),
        _ => false,
    }
}

fn fn_uses(f: &FnDecl) -> bool {
    f.body.stmts.iter().any(stmt_uses)
}

fn stmt_uses(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Let { value, .. } => expr_uses(value),
        StmtKind::Assign { target, value } => expr_uses(target) || expr_uses(value),
        StmtKind::Expr(e) => expr_uses(e),
        StmtKind::Return(e) => e.as_ref().is_some_and(expr_uses),
        StmtKind::If(i) => if_uses(i),
        StmtKind::While { cond, body } => expr_uses(cond) || body.stmts.iter().any(stmt_uses),
        StmtKind::Match {
            scrutinee, arms, ..
        } => expr_uses(scrutinee) || arms.iter().any(|a| a.body.stmts.iter().any(stmt_uses)),
    }
}

fn if_uses(i: &IfStmt) -> bool {
    expr_uses(&i.cond)
        || i.then.stmts.iter().any(stmt_uses)
        || match i.otherwise.as_deref() {
            Some(Else::If(n)) => if_uses(n),
            Some(Else::Block(b)) => b.stmts.iter().any(stmt_uses),
            None => false,
        }
}

fn expr_uses(e: &Expr) -> bool {
    match e {
        Expr::Field { expr, name } => {
            (matches!(&**expr, Expr::Name(n) if n == "raw") && (name == "alloc" || name == "free"))
                || expr_uses(expr)
        }
        Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => expr_uses(x),
        Expr::Unary { expr, .. } => expr_uses(expr),
        Expr::Tuple(xs) => xs.iter().any(expr_uses),
        Expr::Binary { lhs, rhs, .. } => expr_uses(lhs) || expr_uses(rhs),
        Expr::Call { callee, args } => expr_uses(callee) || args.iter().any(expr_uses),
        Expr::StructLit { fields, .. } => fields.iter().any(|(_, x)| expr_uses(x)),
        Expr::Match {
            scrutinee, arms, ..
        } => expr_uses(scrutinee) || arms.iter().any(|a| expr_uses(&a.body)),
        Expr::Closure { body, .. } => match body {
            ClosureBody::Expr(x) => expr_uses(x),
            ClosureBody::Block(b) => b.stmts.iter().any(stmt_uses),
        },
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Name(_)
        | Expr::Result
        | Expr::SelfValue => false,
    }
}

/// 埋め込みモジュールとして扱える名前か（先頭が `std` で、あとに 1 区間以上続く）。
pub fn is_embedded_name(name: &str) -> bool {
    name.strip_prefix("std.")
        .is_some_and(|rest| !rest.is_empty())
}

/// 名前から埋め込みモジュールのソースを引く。`extra` は試験用の追加（`Options::embedded_extra`）。
/// `std.` で始まらない名前は、一覧にあっても引けない（出所の印を守るため）。
pub fn lookup<'a>(name: &str, extra: &'a [(String, String)]) -> Option<&'a str> {
    if !is_embedded_name(name) {
        return None;
    }
    if let Some((_, s)) = extra.iter().find(|(n, _)| n == name) {
        return Some(s.as_str());
    }
    EMBEDDED.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_must_start_with_std_dot() {
        assert!(is_embedded_name("std.os.alloc"));
        assert!(!is_embedded_name("std"));
        assert!(!is_embedded_name("std."));
        assert!(!is_embedded_name("stdx.a"));
        assert!(!is_embedded_name("m.std.a"));
    }

    #[test]
    fn lookup_rejects_non_std_names_even_if_registered() {
        let extra = vec![("m.a".to_string(), "x".to_string())];
        assert_eq!(lookup("m.a", &extra), None);
        let extra = vec![("std.a".to_string(), "x".to_string())];
        assert_eq!(lookup("std.a", &extra), Some("x"));
        assert_eq!(lookup("std.b", &extra), None);
    }

    #[test]
    fn production_registry_names_are_embedded_names() {
        for (n, _) in EMBEDDED {
            assert!(is_embedded_name(n));
        }
    }
}

//! 仕様の抽出（`sigil spec`。ADR 0008 の決定 7）。
//!
//! モジュールの宣言・型の宣言・関数のシグネチャと 4 節を日本語の定型文で出す。関数の本体は
//! 含めない。条件の式（`old(...)`・`result` を含む）は訳さず、正規形の表記で示す。出力は構文
//! だけから決まる（型の情報は使わない）。

use crate::ast::*;
use crate::error::Error;
use crate::fmt::{expr_lines, is_unit, normalize_effects, type_text};
use crate::parser::parse;

/// ソースから仕様のテキストを作る。構文エラーならそのエラー。
pub fn spec(src: &str) -> Result<String, Error> {
    let file = parse(src)?;
    let mut blocks: Vec<String> = vec![module_spec(&file.module, unchecked_count(&file.items))];
    for item in &file.items {
        blocks.push(match item {
            Item::Type(t) => type_spec(t),
            Item::Struct(s) => struct_spec(s),
            Item::Enum(e) => enum_spec(e),
            Item::Fn(f) => fn_spec(f, "関数", &f.name, &f.generics),
            Item::Impl(i) => i
                .fns
                .iter()
                .map(|f| {
                    let name = format!("{}.{}", type_text(&i.ty), f.name);
                    // `impl<T> 型<T>` の型引数と、メソッド自身の型引数をあわせて示す。
                    let gs: Vec<Generic> = i
                        .generics
                        .iter()
                        .chain(f.generics.iter())
                        .cloned()
                        .collect();
                    fn_spec(f, "メソッド", &name, &gs)
                })
                .collect::<Vec<_>>()
                .join("\n"),
        });
    }
    Ok(blocks.join("\n"))
}

/// `unchecked` の節を持つ関数（`impl` のメソッドを含む）の数。
fn unchecked_count(items: &[Item]) -> usize {
    items
        .iter()
        .map(|i| match i {
            Item::Fn(f) => usize::from(f.unchecked.is_some()),
            Item::Impl(i) => i.fns.iter().filter(|f| f.unchecked.is_some()).count(),
            _ => 0,
        })
        .sum()
}

fn module_spec(m: &ModuleDecl, unchecked: usize) -> String {
    let targets: Vec<&str> = m
        .targets
        .iter()
        .map(|t| match t.as_str() {
            "bare" => "実行環境なし",
            "browser" => "ブラウザ",
            "server" => "サーバー",
            other => other,
        })
        .collect();
    format!(
        "モジュール {}\n  出力先: {}\n  信頼基盤: {}\n",
        m.name.join("."),
        targets.join(", "),
        if m.trust == Trust::Base {
            format!("はい（検査を外す関数 {unchecked} 個）")
        } else {
            "いいえ".to_string()
        }
    )
}

fn generics(gs: &[Generic]) -> String {
    if gs.is_empty() {
        return String::new();
    }
    let v: Vec<String> = gs
        .iter()
        .map(|g| {
            if g.bounds.is_empty() {
                g.name.clone()
            } else {
                let mut bs: Vec<String> = g.bounds.iter().map(|b| b.join(".")).collect();
                bs.sort();
                bs.dedup();
                format!("{}: {}", g.name, bs.join(" + "))
            }
        })
        .collect();
    format!("<{}>", v.join(", "))
}

/// 条件の式。複数行になる式は、2 行目以降を字下げして続ける。
fn cond(e: &Option<Expr>) -> String {
    match e {
        None => "なし".to_string(),
        Some(e) => expr_lines(e).join("\n    "),
    }
}

fn type_spec(t: &TypeDecl) -> String {
    format!(
        "型 {}\n  基にする型: {}\n  条件: {}\n",
        t.name,
        type_text(&t.base),
        expr_lines(&t.cond).join("\n    ")
    )
}

fn struct_spec(s: &StructDecl) -> String {
    let fields: Vec<String> = s
        .fields
        .iter()
        .map(|f| format!("{}（{}）", f.name, type_text(&f.ty)))
        .collect();
    format!(
        "構造体 {}{}\n  フィールド: {}\n",
        s.name,
        generics(&s.generics),
        fields.join("、")
    )
}

fn enum_spec(e: &EnumDecl) -> String {
    let vs: Vec<String> = e
        .variants
        .iter()
        .map(|v| {
            if v.fields.is_empty() {
                v.name.clone()
            } else {
                let ts: Vec<String> = v.fields.iter().map(type_text).collect();
                format!("{}（{}）", v.name, ts.join(", "))
            }
        })
        .collect();
    format!(
        "列挙型 {}{}\n  バリアント: {}\n",
        e.name,
        generics(&e.generics),
        vs.join("、")
    )
}

/// 引数の受け渡し方の訳（ADR 0003 の規則 7。型を括弧で付記し、移動の注記は省く）。
fn param_spec(p: &Param) -> String {
    match &p.ty {
        Type::Ref {
            mutable: true,
            inner,
        } => format!("{} を書き換える（{}）", p.name, type_text(inner)),
        Type::Ref {
            mutable: false,
            inner,
        } => format!("{} を読む（{}）", p.name, type_text(inner)),
        // 関数の値の受け渡しは呼び出しの間の借用なので、受け取る（所有する）とは訳さない（ADR 0013）
        t @ Type::Fn { .. } => format!("{} を呼ぶ（{}）", p.name, type_text(t)),
        t => format!("{} を受け取る（{}）", p.name, type_text(t)),
    }
}

/// 効果のカテゴリの訳（ADR 0004 の表）。
fn effect_ja(name: &str) -> String {
    match name {
        "console" => "コンソールに出力する",
        "fs" => "ファイルを読み書きする",
        "fs.read" => "ファイルを読む",
        "fs.write" => "ファイルに書く",
        "net" => "ネットワークに接続し、接続を待ち受ける",
        "net.client" => "ネットワークに接続する",
        "net.server" => "接続を待ち受ける",
        "db" => "データベースを読み書きする",
        "clock" => "現在時刻を読む・待つ",
        "random" => "乱数を得る",
        "env" => "環境変数・起動引数を読む",
        "process" => "他のプロセスを起動する・自分を終了する",
        "dom" => "画面（DOM）を読む・書き換える",
        "browser.storage" => "ブラウザの保存領域を読み書きする",
        "state" => "共有所有の型を通して値を書き換える",
        "foreign" => "外部から渡された関数を呼ぶ",
        "task" => "タスクを起動する・中断しうる・値を共有する",
        "task.spawn" => "並行に動くタスクを起動する",
        "task.suspend" => "中断しうる",
        "task.share" => "他のタスクと共有する値を読み書きする",
        other => return other.to_string(),
    }
    .to_string()
}

fn effects_spec(f: &FnDecl) -> String {
    let names = normalize_effects(&f.effects, &f.params);
    if names.is_empty() {
        return "なし".to_string();
    }
    let v: Vec<String> = names
        .iter()
        .map(
            |n| match n.strip_prefix("of(").and_then(|r| r.strip_suffix(')')) {
                Some(arg) => format!("{arg} の効果"),
                None => effect_ja(n),
            },
        )
        .collect();
    v.join("、")
}

fn intent_text(s: &str) -> String {
    if s.chars().any(crate::fmt::needs_u_escape) {
        let e = crate::fmt::escape_str(s);
        e[1..e.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn fn_spec(f: &FnDecl, label: &str, name: &str, gs: &[Generic]) -> String {
    let params = if f.params.is_empty() {
        "なし".to_string()
    } else {
        f.params
            .iter()
            .map(param_spec)
            .collect::<Vec<_>>()
            .join("、")
    };
    // `-> Unit` は正規形では書かない（ADR 0011 の決定 2）ので、`Unit` も「なし」と示す。
    let ret = f
        .ret
        .as_ref()
        .filter(|r| !is_unit(r))
        .map_or("なし".to_string(), type_text);
    let unchecked = f.unchecked.as_ref().map_or(String::new(), |u| {
        format!("  検査を外す: {}\n", intent_text(u))
    });
    format!(
        "{} {}{}\n  意図: {}\n  引数: {}\n  戻り値: {}\n  事前条件: {}\n  事後条件: {}\n  効果: {}\n{}",
        label,
        name,
        generics(gs),
        intent_text(&f.intent),
        params,
        ret,
        cond(&f.requires),
        cond(&f.ensures),
        effects_spec(f),
        unchecked
    )
}

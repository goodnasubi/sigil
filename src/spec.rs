//! 仕様の抽出（`sigil spec`。ADR 0008 の決定 7）。
//!
//! モジュールの宣言・型の宣言・関数のシグネチャと 4 節を日本語の定型文で出す。関数の本体は
//! 含めない。条件の式（`old(...)`・`result` を含む）は訳さず、正規形の表記で示す。出力は構文
//! だけから決まる（型の情報は使わない）。

use crate::ast::*;
use crate::error::Error;
use crate::fmt::{expr_lines, normalize_effects, type_text};
use crate::parser::parse;

/// ソースから仕様のテキストを作る。構文エラーならそのエラー。
pub fn spec(src: &str) -> Result<String, Error> {
    let file = parse(src)?;
    let mut blocks: Vec<String> = vec![module_spec(&file.module)];
    for item in &file.items {
        blocks.push(match item {
            Item::Type(t) => type_spec(t),
            Item::Struct(s) => struct_spec(s),
            Item::Enum(e) => enum_spec(e),
            Item::Fn(f) => fn_spec(f),
        });
    }
    Ok(blocks.join("\n"))
}

fn module_spec(m: &ModuleDecl) -> String {
    let targets: Vec<&str> = m
        .targets
        .iter()
        .map(|t| match t.as_str() {
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
            "はい"
        } else {
            "いいえ"
        }
    )
}

fn generics(gs: &[Generic]) -> String {
    if gs.is_empty() {
        return String::new();
    }
    let v: Vec<String> = gs
        .iter()
        .map(|g| match &g.bound {
            Some(b) => format!("{}: {}", g.name, b.join(".")),
            None => g.name.clone(),
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

fn fn_spec(f: &FnDecl) -> String {
    let params = if f.params.is_empty() {
        "なし".to_string()
    } else {
        f.params
            .iter()
            .map(param_spec)
            .collect::<Vec<_>>()
            .join("、")
    };
    let ret = f.ret.as_ref().map_or("なし".to_string(), type_text);
    format!(
        "関数 {}{}\n  意図: {}\n  引数: {}\n  戻り値: {}\n  事前条件: {}\n  事後条件: {}\n  効果: {}\n",
        f.name,
        generics(&f.generics),
        intent_text(&f.intent),
        params,
        ret,
        cond(&f.requires),
        cond(&f.ensures),
        effects_spec(f)
    )
}

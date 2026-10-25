//! ADR 0014 の PR1: 出力先 `bare` と効果の表の `bare` の列。

use sigil::effects::EffectTable;
use sigil::{check::check, parse, ErrorKind};

const GOOD_FN: &str = "fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return x;\n}\n";

fn module(targets: &str) -> String {
    format!("module m.a\n  targets {targets}\n  trust none\n\n{GOOD_FN}")
}

fn parse_kind(src: &str) -> sigil::Error {
    match parse(src) {
        Ok(_) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => e,
    }
}

fn core_fn(effects: &str, body: &str) -> String {
    format!(
        "module m.a\n  targets bare\n  trust none\n\nfn g() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

// ---- 構文・整形・仕様 ----

#[test]
fn targets_bare_parses() {
    for t in ["bare", "bare, server", "bare, browser, server"] {
        let src = module(t);
        parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert!(sigil::fmt::check(&src).unwrap().is_none(), "{t}");
    }
}

#[test]
fn targets_bare_unsorted() {
    let e = parse_kind(&module("browser, bare"));
    assert_eq!(e.kind, ErrorKind::TargetsUnsorted);
}

#[test]
fn targets_bare_duplicate() {
    let e = parse_kind(&module("bare, bare"));
    assert_eq!(e.kind, ErrorKind::TargetsDuplicate);
}

#[test]
fn unknown_target_lists_bare() {
    let e = parse_kind(&module("metal"));
    assert_eq!(e.kind, ErrorKind::UnknownTarget);
    assert!(e.message.contains("bare・browser・server"), "{}", e.message);
}

#[test]
fn spec_shows_bare() {
    let src = std::fs::read_to_string("examples/01-average/c-brace.sigil").unwrap();
    let out = sigil::spec::spec(&src).unwrap();
    assert!(
        out.contains("  出力先: 実行環境なし, ブラウザ, サーバー\n"),
        "{out}"
    );
}

// ---- 効果の表 ----

#[test]
fn bare_column_is_state_only() {
    let t = EffectTable::standard();
    let cats = [
        "console",
        "fs.read",
        "fs.write",
        "net.client",
        "net.server",
        "db",
        "clock",
        "random",
        "env",
        "process",
        "dom",
        "browser.storage",
        "state",
        "foreign",
        "task.spawn",
        "task.suspend",
        "task.share",
    ];
    for c in cats {
        assert_eq!(t.allowed_on(c, "bare"), c == "state", "{c}");
    }
}

#[test]
fn bare_rejects_declared_console() {
    let e = check(&core_fn("console", "  return 0;")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::EffectTarget);
}

#[test]
fn bare_rejects_task_effects() {
    let e = check(&core_fn("task.suspend", "  return 0;")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::EffectTarget);
}

#[test]
fn bare_allows_state() {
    let src = core_fn(
        "state",
        "  let s = Shared.new(Cell.new(1));\n  s.set(2);\n  return s.get();",
    );
    check(&src).unwrap_or_else(|e| panic!("{e}\n{src}"));
}

#[test]
fn hosted_targets_still_allow_console() {
    // `server` と `browser` の列は変わらない。
    let t = EffectTable::standard();
    assert!(t.allowed_on("console", "server") && t.allowed_on("console", "browser"));
    assert!(t.allowed_on("fs.read", "server") && !t.allowed_on("fs.read", "browser"));
}

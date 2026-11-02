//! ADR 0025 補足 A-1 / PR 3-4e: `sigil spec` の大域の宣言と `触る大域:` 行。
//!
//! 触る大域は関数の本体が直接触るものだけを数える（推移的な呼び出し先は含めない。ADR に指定なし）。

use sigil::spec::{extract, spec};

fn ufn(name: &str, body: &str) -> String {
    format!(
        "fn {name}() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"前提\"\n{{\n{body}\n}}\n\n"
    )
}

const HEAD: &str = "module m.t\n  targets server\n  trust base\n\n";

const DECLS: &str = "global LIMIT: Int = 8\n\nglobal mut HEAP: Addr = raw.addr_null()\n\nglobal mut COUNT: Int = 0\n\n";

fn src() -> String {
    format!(
        "{HEAD}{DECLS}{}{}{}{}{}",
        ufn("none_touched", "  return 1;"),
        ufn("read_only", "  return LIMIT + COUNT;"),
        ufn("writer", "  COUNT = 1;\n  return 0;"),
        ufn(
            "mixed",
            "  COUNT = COUNT + 1;\n  HEAP = raw.addr_null();\n  return LIMIT;"
        ),
        ufn("caller", "  return writer();"),
    )
}

#[test]
fn module_lists_globals_in_source_order() {
    let out = spec(&src()).unwrap();
    let want = "  大域: LIMIT: Int = 8\n  大域: mut HEAP: Addr = raw.addr_null()\n  大域: mut COUNT: Int = 0\n";
    assert!(out.contains(want), "{out}");
    // 信頼基盤の行の下、最初の関数より前に並ぶ。
    let head = out.find("信頼基盤:").unwrap();
    assert!(head < out.find("大域:").unwrap());
    assert!(out.find("大域:").unwrap() < out.find("関数 none_touched").unwrap());
}

#[test]
fn touched_globals_line_sorted_dedup_with_access_kind() {
    let out = spec(&src()).unwrap();
    assert!(
        out.contains("  触る大域: COUNT（読む）, LIMIT（読む）\n"),
        "{out}"
    );
    assert!(out.contains("  触る大域: COUNT（読み書き）\n"), "{out}");
    assert!(
        out.contains("  触る大域: COUNT（読み書き）, HEAP（読み書き）, LIMIT（読む）\n"),
        "{out}"
    );
}

#[test]
fn untouched_function_has_no_line() {
    let out = spec(&src()).unwrap();
    let block = out
        .split("関数 ")
        .find(|b| b.starts_with("none_touched"))
        .unwrap();
    assert!(!block.contains("触る大域"), "{block}");
}

#[test]
fn transitive_calls_are_not_counted() {
    let out = spec(&src()).unwrap();
    let block = out
        .split("関数 ")
        .find(|b| b.starts_with("caller"))
        .unwrap();
    assert!(!block.contains("触る大域"), "{block}");
}

#[test]
fn module_without_globals_is_unchanged() {
    let s = format!("{HEAD}{}", ufn("f", "  return 1;"));
    let out = spec(&s).unwrap();
    assert!(!out.contains("大域"), "{out}");
}

#[test]
fn json_has_globals_and_touched_globals() {
    let sp = extract(&src()).unwrap();
    let j = sp.to_json().to_string();
    assert!(j.contains("\"globals\""), "{j}");
    assert!(j.contains("\"touched_globals\""), "{j}");
    assert!(j.contains("\"mutable\":true"), "{j}");
}

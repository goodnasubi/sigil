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

// ---- 書き込みの経路ごとの記録（代入以外も「読み書き」になる） ----

const S_DEFS: &str = "struct S {\n  v: Int,\n}\n\nimpl S {\n  fn bump(self: &mut S)\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {\n    self.v = self.v + 1;\n  }\n}\n\nglobal mut G: S = S { v: 0 }\n\nglobal mut COUNT: Int = 0\n\nglobal LIMIT: Int = 8\n\n";

fn bump_fn() -> String {
    "fn add(n: &mut Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  n = n + 1;\n}\n\n".to_string()
}

fn block_of<'a>(out: &'a str, name: &str) -> &'a str {
    out.split("関数 ")
        .find(|b| b.starts_with(name))
        .unwrap_or_else(|| panic!("{name}\n{out}"))
}

fn touched(body: &str) -> String {
    let s = format!("{HEAD}{S_DEFS}{}{}", bump_fn(), ufn("f", body));
    let out = spec(&s).unwrap_or_else(|e| panic!("{e:?}\n{s}"));
    block_of(&out, "f")
        .lines()
        .find(|l| l.contains("触る大域"))
        .unwrap_or("")
        .to_string()
}

#[test]
fn mut_borrow_counts_as_write() {
    assert_eq!(
        touched("  add(&mut COUNT);\n  return 0;"),
        "  触る大域: COUNT（読み書き）"
    );
}

#[test]
fn mut_self_method_counts_as_write() {
    assert_eq!(
        touched("  G.bump();\n  return 0;"),
        "  触る大域: G（読み書き）"
    );
}

#[test]
fn closure_write_is_recorded() {
    assert_eq!(
        touched("  let g = move || { COUNT = 1; };\n  g();\n  return 0;"),
        "  触る大域: COUNT（読み書き）"
    );
}

#[test]
fn touches_inside_if_and_while_are_recorded() {
    assert_eq!(
        touched("  if LIMIT > 1 {\n    COUNT = 2;\n  }\n  return 0;"),
        "  触る大域: COUNT（読み書き）, LIMIT（読む）"
    );
    assert_eq!(
        touched("  let mut i = 0;\n  while i < LIMIT {\n    i = i + COUNT + 1;\n  }\n  return i;"),
        "  触る大域: COUNT（読む）, LIMIT（読む）"
    );
    assert_eq!(
        touched(
            "  let mut i = 0;\n  while i < 3 {\n    COUNT = i;\n    i = i + 1;\n  }\n  return 0;"
        ),
        "  触る大域: COUNT（読み書き）"
    );
}

#[test]
fn json_write_flag() {
    let sp = extract(&src()).unwrap();
    let j = sp.to_json();
    let items = j.get("items").unwrap().as_array().unwrap();
    let flags = |name: &str| -> Vec<(String, bool)> {
        let f = items
            .iter()
            .find(|i| i.get("name").unwrap().as_str() == Some(name))
            .unwrap();
        f.get("touched_globals")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|g| {
                (
                    g.get("name").unwrap().as_str().unwrap().to_string(),
                    g.get("write").unwrap().to_json() == "true",
                )
            })
            .collect()
    };
    assert_eq!(
        flags("mixed"),
        vec![
            ("COUNT".to_string(), true),
            ("HEAP".to_string(), true),
            ("LIMIT".to_string(), false)
        ]
    );
    assert_eq!(
        flags("read_only"),
        vec![("COUNT".to_string(), false), ("LIMIT".to_string(), false)]
    );
    let m = j.get("module").unwrap();
    let g = m.get("globals").unwrap().as_array().unwrap();
    assert_eq!(g.len(), 3);
    assert_eq!(g[1].get("mutable").unwrap().to_json(), "true");
    assert_eq!(g[0].get("mutable").unwrap().to_json(), "false");
}

#[test]
fn json_without_globals_has_no_new_fields() {
    let s = format!("{HEAD}{}", ufn("f", "  return 1;"));
    let j = extract(&s).unwrap().to_json();
    assert!(j.get("module").unwrap().get("globals").is_none());
    let items = j.get("items").unwrap().as_array().unwrap();
    assert!(items[0].get("touched_globals").is_none());
}

#[test]
fn check_failure_shows_placeholder_for_touched_globals() {
    // 型エラーで検査が失敗すると、他の行と同様に表示できない旨を出す。
    let s = format!(
        "{HEAD}{DECLS}{}",
        ufn("broken", "  COUNT = true;\n  return 0;")
    );
    let out = spec(&s).unwrap();
    assert!(
        out.contains("  触る大域: （検査のエラーのため表示できない）\n"),
        "{out}"
    );
    assert!(
        out.contains("  使う原始操作: （検査のエラーのため表示できない）\n"),
        "{out}"
    );
}

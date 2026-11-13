//! Issue #416: ネイティブで、文字列の書き出し（リテラル）を比較（`==`・`!=`・`<`…）の片側か両側に直接書くと、
//! 書き出し 1 つにつき塊 2 つを漏らしていた。比較の演算子は読むだけなので、書き出しから作った一時の文字列は、比較の後に
//! 破棄されなければならない。
//!
//! 式の形ごとに、結果が正しいことと、終わりの `sigil_blocks=` が 0 であることを確かめる。繰り返しても増えない（毎回破棄される）
//! ことも確かめる。評価器でも同じ結果になる（突き合わせ）。

mod common;

use common::native::{exec_env, scratch, try_build};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

fn blocks(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {stderr}"));
    line.split_whitespace()
        .find(|w| w.starts_with("sigil_blocks="))
        .unwrap_or_else(|| panic!("sigil_blocks= がない: {line}"))
        .to_string()
}

/// `body` を本体にした `t(n: Int) -> Int` をネイティブで `n` 回の引数で走らせ、(値, 終わりの塊の数) を返す。
fn run_native(body: &str, n: i64) -> (String, String) {
    let src = format!("{HEAD}{}", func("t(n: Int) -> Int", body));
    let dir = scratch("strcmp416");
    let exe = dir.join("prog");
    try_build(&[&src], "t", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec_env(&exe, &[n.to_string()], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}\n{src}", got.stderr);
    (got.stdout.trim().to_string(), blocks(&got.stderr))
}

/// 評価器の値（突き合わせ）。
fn run_eval(body: &str, n: i64) -> String {
    let src = format!("{HEAD}{}", func("t(n: Int) -> Int", body));
    common::run(&src, "t", &[sigil::exec::Arg::Int(n)])
        .unwrap_or_else(|e| panic!("{:?}\n{src}", e.error))
        .value
}

/// 形ごとの本体と期待する値。`m` は変数の文字列 "GET"。
const CASES: &[(&str, &str, &str)] = &[
    ("eq_literal_right", "let m = \"GET\";\nif m == \"GET\" { return 1; }\nreturn 0;", "1"),
    ("eq_literal_left", "let m = \"GET\";\nif \"GET\" == m { return 1; }\nreturn 0;", "1"),
    ("ne_literal_right", "let m = \"GET\";\nif m != \"GET\" { return 1; }\nreturn 0;", "0"),
    ("lt_literal_right", "let m = \"ABC\";\nif m < \"GET\" { return 1; }\nreturn 0;", "1"),
    ("both_literals", "let e = \"x\" == \"x\";\nif e { return 1; }\nreturn 0;", "1"),
    ("both_literals_ne", "let e = \"x\" != \"y\";\nif e { return 1; }\nreturn 0;", "1"),
    (
        "literal_in_field_compare",
        "let a = (\"GET\", 1);\nlet (s, k) = a;\nif s == \"GET\" && k == 1 { return 1; }\nreturn 0;",
        "1",
    ),
    (
        "literal_and_or",
        "let m = \"GET\";\nif m == \"POST\" || m == \"GET\" { return 1; }\nreturn 0;",
        "1",
    ),
    // 文字列のパターンの照合も、書き出しと変数の `==` に下ろされる（同じ漏れだった）。
    (
        "match_string_pattern",
        "let m = \"GET\";\nmatch m {\n  \"POST\" => { return 2; }\n  \"GET\" => { return 1; }\n  _ => { return 0; }\n}",
        "1",
    ),
    (
        "match_string_pattern_in_option",
        "let o = Some(\"GET\");\nmatch o {\n  Some(\"GET\") => { return 1; }\n  _ => { return 0; }\n}",
        "1",
    ),
    (
        "chain_of_literal_compares",
        "let m = \"DELETE\";\nif m == \"GET\" { return 1; }\nif m == \"HEAD\" { return 2; }\nif m == \"DELETE\" { return 3; }\nreturn 0;",
        "3",
    ),
];

#[test]
fn string_literal_in_a_comparison_leaves_no_blocks() {
    for (name, body, want) in CASES {
        let (value, live) = run_native(body, 0);
        assert_eq!(value, *want, "{name}: 値");
        assert_eq!(live, "sigil_blocks=0", "{name}: 塊が漏れた");
        assert_eq!(run_eval(body, 0), *want, "{name}: 評価器の値");
    }
}

#[test]
fn repeated_literal_comparisons_do_not_accumulate() {
    // 繰り返す数によらず、終わりの塊は 0 のまま（毎回破棄される）。
    let body = "let m = \"GET\";\nlet mut i = 0;\nlet mut hits = 0;\nwhile i < n {\n    if m == \"GET\" { hits = hits + 1; }\n    if \"POST\" != m { hits = hits + 1; }\n    i = i + 1;\n}\nreturn hits;";
    let (v, live) = run_native(body, 1000);
    assert_eq!(v, "2000");
    assert_eq!(live, "sigil_blocks=0");
    let (v, live) = run_native(body, 10000);
    assert_eq!(v, "20000");
    assert_eq!(live, "sigil_blocks=0");
}

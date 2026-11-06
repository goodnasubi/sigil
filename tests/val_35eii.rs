//! ADR 0025 補足 G の 3-5e-ii（複写・影・破棄・塊の確保を型ごとの生成関数へ移す）の試験。
//!
//! 確かめること: `clone` が独立した塊を作ること、300 万段の連鎖を捨てても溢れないこと、作業の列（破棄の反復）が
//! 4096 項を超える広い値を捨てられること、繰り返しの回数（1000 と 100000）によらず `sigil_blocks=` が 0 で一致すること。
//! 二重解放の停止（G-4）は、わざと壊した版で手元で確かめる（ここには置かない）。

mod common;

use common::native::{exec, exec_env, native_drops, scratch, try_build};
use sigil::exec::Arg;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";
const PROBE: &str = "struct Probe {\n  id: Int,\n}\n\nfn mk(n: Int) -> Probe\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Probe { id: n };\n}\n\n";
const CHAIN: &str = "enum L {\n  Nil,\n  Cons(Int, L),\n}\n\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

/// 標準エラーの `live:` の行から、`sigil_blocks=` の語を取る（完全一致の語として）。
fn sigil_blocks_token(stderr: &str) -> String {
    let line = stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {stderr}"));
    line.split_whitespace()
        .find(|w| w.starts_with("sigil_blocks="))
        .unwrap_or_else(|| panic!("sigil_blocks= がない: {line}"))
        .to_string()
}

#[test]
fn a_clone_makes_an_independent_block_that_drops_on_its_own() {
    // `clone` は新しい塊を作る。片方を書き換えても、もう片方には届かず、両方が別々に破棄される（順は宣言の逆）。
    let src = format!(
        "{HEAD}{PROBE}{}",
        func(
            "t() -> Int",
            "let a = mk(1);\nlet mut b = a.clone();\nb.id = 2;\nreturn 0;"
        )
    );
    let (got, drops) = native_drops(&[&src], "t", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(drops, ["Probe { id: 2 }", "Probe { id: 1 }"]);
}

#[test]
fn a_wide_value_drops_every_element_and_leaves_no_blocks() {
    // 10000 個の構造体を持つ一覧を捨てる。子は 4096 項を超えて作業の列を伸ばす（補足 G-2）。
    // 子は宣言の順（先頭の要素から）に破棄される。
    let src = format!(
        "{HEAD}{PROBE}{}",
        func(
            "t() -> Int",
            "let mut l = List.new();\nlet mut i = 0;\nwhile i < 10000 {\n    l.push(mk(i));\n    i = i + 1;\n}\nreturn l.len();"
        )
    );
    let dir = scratch("val35eii-wide");
    let exe = dir.join("prog");
    try_build(&[&src], "t", &exe, true).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let drops: Vec<&str> = got
        .stderr
        .lines()
        .filter_map(|l| l.strip_prefix("drop: "))
        .collect();
    assert_eq!(drops.len(), 10000, "破棄の数");
    assert_eq!(drops[0], "Probe { id: 0 }");
    assert_eq!(drops[9999], "Probe { id: 9999 }");
    assert_eq!(sigil_blocks_token(&got.stderr), "sigil_blocks=0");
}

#[test]
fn a_very_deep_value_is_built_and_dropped_natively_without_overflow() {
    // 300 万段の連鎖を作って捨てる（評価器の `very_deep_value_is_dropped_without_overflowing_the_host_stack` と同じ入力）。
    let src = format!(
        "{HEAD}{CHAIN}{}",
        func(
            "t() -> Int",
            "let mut l = L.Nil;\nlet mut i = 0;\nwhile i < 3000000 {\n    l = L.Cons(i, l);\n    i = i + 1;\n}\nreturn 1;"
        )
    );
    let dir = scratch("val35eii-deep");
    let exe = dir.join("prog");
    try_build(&[&src], "t", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec(&exe, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout.trim(), "1");
}

#[test]
fn repeated_construction_keeps_sigil_blocks_exact_at_1000_and_100000() {
    // 組・構造体・列挙型・クロージャを毎回作って捨てる。回数によらず、終わりの生存数は同じで 0 になる。
    let body = "let mut i = 0;\nlet mut total = 0;\nwhile i < n {\n    let p = (i, mk(i));\n    let (x, y) = p;\n    let l = L.Cons(i, L.Cons(1, L.Nil));\n    let c = move |z| z + i;\n    total = total + c(1) + x + y.id;\n    i = i + 1;\n}\nreturn total;";
    let src = format!("{HEAD}{PROBE}{CHAIN}{}", func("t(n: Int) -> Int", body));
    let dir = scratch("val35eii-loops");
    let exe = dir.join("prog");
    try_build(&[&src], "t", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let small = exec_env(&exe, &["1000".to_string()], &[("SIGIL_RT_LIVE", "1")]);
    let large = exec_env(&exe, &["100000".to_string()], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(small.code, Some(0), "{}", small.stderr);
    assert_eq!(large.code, Some(0), "{}", large.stderr);
    let (a, b) = (
        sigil_blocks_token(&small.stderr),
        sigil_blocks_token(&large.stderr),
    );
    assert_eq!(a, "sigil_blocks=0", "1000 回の後");
    assert_eq!(b, "sigil_blocks=0", "100000 回の後");
}

/// 3-5e-iii（補足 G-7）: `SIGIL_RT_LIVE` の行は `live: sigil_blocks=<数>` だけで、`strs=`・`blocks=` を含まない。
#[test]
fn live_line_has_only_sigil_blocks() {
    let src = format!("{HEAD}{}", func("t() -> Int", "return 1;"));
    let dir = scratch("val35eiii-live");
    let exe = dir.join("prog");
    try_build(&[&src], "t", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let lines: Vec<&str> = got
        .stderr
        .lines()
        .filter(|l| l.starts_with("live: "))
        .collect();
    assert_eq!(lines, ["live: sigil_blocks=0"], "{}", got.stderr);
}

/// 3-5e-iii: Rust のランタイムから、旧書式の `strs=`・`blocks=` の数え（`sigil_rt_live_strs`・`sigil_rt_live_blocks`）が消えている。
#[test]
fn old_live_counters_are_gone_from_the_rust_runtime() {
    let rt = include_str!("../runtime/core/src/lib.rs");
    for name in ["sigil_rt_live_strs", "sigil_rt_live_blocks"] {
        assert!(
            !rt.contains(name),
            "{name} が Rust のランタイムに残っている"
        );
    }
    assert!(
        !rt.contains("live: strs="),
        "旧書式 `live: strs=` が残っている"
    );
}

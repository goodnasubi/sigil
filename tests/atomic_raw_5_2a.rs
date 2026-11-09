//! ADR 0030 決定 5 / PR 5-2a: 原子的な操作の原始操作（`raw.atomic_*`）。
//!
//! - 評価器とネイティブの突き合わせ（`common::run_with` が両方を走らせて比べる）で、戻り値の規則を固定する:
//!   `add`・`swap`・`cas` は操作の**前**の値を返し、`add` は 2 の補数で巻き戻る。
//! - 評価器は、8 バイト境界違反・生きていない領域・ヌル番地で停止する（`load_i64`・`store_i64` と同じ検査）。
mod common;
use common::run_with;
use sigil::exec::Options;

fn src(body: &str) -> String {
    format!(
        "module m.t\n  targets server\n  trust base\n\nfn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"原子的な操作の試験。領域は自分で確保して解放する\"\n{{\n{body}\n}}\n"
    )
}

fn opts() -> Options {
    Options {
        trust_base: vec!["m.t".to_string()],
        ..Options::captured()
    }
}

/// 評価器とネイティブで同じ値になることを確かめて、その値を返す。
fn value(body: &str) -> String {
    let s = src(body);
    run_with(&s, "t", &[], &opts())
        .unwrap_or_else(|e| panic!("{e:?}\n{s}"))
        .value
}

#[test]
fn load_and_store() {
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 41);\n  let a = raw.atomic_load_i64(p);\n  raw.atomic_store_i64(p, -7);\n  let b = raw.atomic_load_i64(p);\n  raw.free(p, 16, 8);\n  return a * 1000 + b;",
    );
    assert_eq!(v, "40993"); // 41 * 1000 + (-7)
}

#[test]
fn add_returns_the_previous_value() {
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 5);\n  let before = raw.atomic_add_i64(p, 3);\n  let after = raw.atomic_load_i64(p);\n  let neg = raw.atomic_add_i64(p, -20);\n  let last = raw.atomic_load_i64(p);\n  raw.free(p, 16, 8);\n  return before * 1000000 + after * 10000 + neg * 100 + last;",
    );
    // before = 5, after = 8, neg = 8（-20 を足す前）, last = -12
    assert_eq!(v, (5_000_000 + 80_000 + 800 - 12).to_string());
}

#[test]
fn add_wraps_around_in_twos_complement() {
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 9223372036854775807);\n  let before = raw.atomic_add_i64(p, 1);\n  let after = raw.atomic_load_i64(p);\n  raw.free(p, 16, 8);\n  if before == 9223372036854775807 && after == -9223372036854775807 - 1 {\n    return 1;\n  }\n  return 0;",
    );
    assert_eq!(v, "1");
}

#[test]
fn swap_returns_the_previous_value() {
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 10);\n  let prev = raw.atomic_swap_i64(p, 99);\n  let now = raw.atomic_load_i64(p);\n  raw.free(p, 16, 8);\n  return prev * 1000 + now;",
    );
    assert_eq!(v, "10099");
}

#[test]
fn cas_succeeds_only_when_the_value_matches() {
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 10);\n  let ok = raw.atomic_cas_i64(p, 10, 20);\n  let after_ok = raw.atomic_load_i64(p);\n  let fail = raw.atomic_cas_i64(p, 99, 1);\n  let after_fail = raw.atomic_load_i64(p);\n  raw.free(p, 16, 8);\n  return ok * 1000000 + after_ok * 10000 + fail * 100 + after_fail;",
    );
    // ok = 10（前の値が expected と等しい = 成功）, after_ok = 20, fail = 20（前の値。expected と違う = 失敗）, after_fail = 20
    assert_eq!(v, (10_000_000 + 200_000 + 2_000 + 20).to_string());
}

#[test]
fn fence_has_no_visible_effect_on_a_single_thread() {
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 3);\n  raw.atomic_fence();\n  let a = raw.atomic_load_i64(p);\n  raw.atomic_fence();\n  raw.free(p, 16, 8);\n  return a;",
    );
    assert_eq!(v, "3");
}

#[test]
fn a_counter_loop_gives_the_same_result_on_both() {
    // 原子的な加算で 1000 まで数える。評価器とネイティブの両方で同じ最終値になる。
    let v = value(
        "  let p = raw.alloc(16, 8);\n  raw.atomic_store_i64(p, 0);\n  let mut i = 0;\n  while i < 1000 {\n    raw.atomic_add_i64(p, 1);\n    i = i + 1;\n  }\n  let n = raw.atomic_load_i64(p);\n  raw.free(p, 16, 8);\n  return n;",
    );
    assert_eq!(v, "1000");
}

fn stops(body: &str, needle: &str) {
    let s = src(body);
    match sigil::ir_eval::run_with(&s, "t", &[], &opts()) {
        Err(e) => {
            let m = format!("{e:?}");
            assert!(m.contains(needle), "期待 {needle} / 実際 {m}");
        }
        Ok(o) => panic!("停止するはずが成功: {}", o.value),
    }
}

#[test]
fn evaluator_stops_on_misaligned_dead_and_null_addresses() {
    // 8 バイト境界違反（全ての操作）
    for call in [
        "raw.atomic_load_i64(q)",
        "raw.atomic_store_i64(q, 1)",
        "raw.atomic_add_i64(q, 1)",
        "raw.atomic_swap_i64(q, 1)",
        "raw.atomic_cas_i64(q, 0, 1)",
    ] {
        stops(
            &format!(
                "  let p = raw.alloc(32, 8);\n  let q = raw.offset(p, 4);\n  let r = {call};\n  raw.free(p, 32, 8);\n  return 0;"
            ),
            "8バイト境界違反",
        );
    }
    // ヌル番地
    stops(
        "  let r = raw.atomic_load_i64(raw.addr_null());\n  return r;",
        "ヌル番地",
    );
    // 解放済みの領域
    stops(
        "  let p = raw.alloc(16, 8);\n  raw.free(p, 16, 8);\n  let r = raw.atomic_add_i64(p, 1);\n  return r;",
        "範囲外",
    );
}

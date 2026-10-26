//! 破棄の順序が仕様（ADR 0006 の条件 9）どおりであることを固定するテスト。
//!
//! インタプリタは破棄の順序の基準ではない（ADR 0017）。移動の追跡が粗いため、値は移動先でなく
//! 元の場所の位置で破棄される既知の制限があり、ここでは扱わない。順序の基準は SIR の評価器と
//! ネイティブの実行ファイルで、二つの一致も確かめる（ADR 0015 の決定 8、PR6）。
//!
//! 加えて、スタックの枯渇による停止のときの破棄の順序を固定する。枠の数は実装で違うので、
//! 実行ファイルの順序は形で確かめる。

mod common;

use sigil::interp::{Arg, Options, RunError, StopKind};
use sigil::ir_eval::run_with;

/// SIR の評価器とネイティブの実行ファイルで `t` を走らせ、破棄された Probe の id を順に返す。
/// 2 つの順序が一致していることも確かめる。
fn sir_ids(src: &str) -> Vec<i64> {
    let sir = sigil::ir_eval::run_with(src, "t", &[], &traced(None)).unwrap();
    let (got, native) = common::native::native_drops(&[src], "t", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        sir.drops, native,
        "SIR の評価器と実行ファイルの破棄の順序\n{src}"
    );
    probe_ids(&sir.drops)
}

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\nstruct Probe {\n  id: Int,\n}\n\nstruct Two {\n  a: Probe,\n  b: Probe,\n}\n\nfn mk(n: Int) -> Probe\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Probe { id: n };\n}\n\n";

fn f(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

fn traced(stack_bytes: Option<usize>) -> Options {
    let mut o = Options {
        trace_drops: true,
        ..Options::default()
    };
    if let Some(n) = stack_bytes {
        o.stack_bytes = n;
    }
    o
}

/// `t` の本体を SIR の評価器とネイティブで実行して、仕様の順序の順序（二つで一致）を返す。
fn spec_ids_of(body: &str) -> Vec<i64> {
    sir_ids(&format!("{HEAD}{}", f("t() -> Int", body)))
}

fn probe_ids(d: &[String]) -> Vec<i64> {
    d.iter()
        .filter_map(|s| s.strip_prefix("Probe { id: "))
        // SIR の評価器は、枯渇の直前の最も深い枠の値を表示できない（本文が空になる）ので、数として読めないものは除く。
        .filter_map(|s| s.trim_end_matches(" }").trim().parse().ok())
        .collect()
}

#[test]
fn partial_move_of_a_field_drops_with_the_whole_value() {
    // 仕様の順序: y (9)、x (1)、t の残り (2)。
    let src = format!(
        "{HEAD}{}",
        f(
            "t() -> Int",
            "let t = Two { a: mk(1), b: mk(2) };\nlet y = mk(9);\nlet x = t.a;\nreturn 0;"
        )
    );
    assert_eq!(sir_ids(&src), [1, 9, 2]);
}

#[test]
fn a_match_binding_that_moves_drops_with_the_scrutinee() {
    // 仕様の順序: 7 (腕の局所)、1 (束縛 p。腕の終わり)、5。
    assert_eq!(spec_ids_of("let o = Some(mk(1));\nlet z = mk(5);\nmatch o {\n    Some(p) => {\n        let q = mk(7);\n    }\n    None => {}\n}\nreturn 0;"), [7, 1, 5]);
}

#[test]
fn a_consuming_method_receiver_drops_with_the_original_place() {
    // 仕様の順序: map の中で p (1) が先に破棄され、そのあと z (5)。
    assert_eq!(
        spec_ids_of("let o = Some(mk(1));\nlet z = mk(5);\nlet m = o.map(|p| p.id);\nreturn 0;"),
        [1, 5]
    );
    // 仕様の順序: 既定値 3 は呼び出しの中で、m (1)、z (5) の順。
    assert_eq!(
        spec_ids_of("let o = Some(mk(1));\nlet z = mk(5);\nlet m = o.unwrap_or(mk(3));\nreturn 0;"),
        [3, 1, 5]
    );
}

#[test]
fn a_captured_value_drops_at_its_original_place_not_with_the_closure() {
    // 仕様の順序: w (6)、クロージャ c の環境の p (1)、z (5)。
    assert_eq!(
        spec_ids_of(
            "let p = mk(1);\nlet z = mk(5);\nlet c = move || p.id;\nlet w = mk(6);\nreturn c();"
        ),
        [6, 1, 5]
    );
}

#[test]
fn a_closure_is_treated_as_copyable_so_a_move_of_it_does_not_empty_the_source() {
    // 仕様の順序: k (7)、d の環境の p (1)、w (6)。
    // `let d = c;` で c は空にならず、環境の p は元の場所 (最初の宣言) の位置まで残る。
    assert_eq!(spec_ids_of("let p = mk(1);\nlet c = move || p.id;\nlet w = mk(6);\nlet d = c;\nlet k = mk(7);\nreturn d();"), [7, 1, 6]);
}

#[test]
fn stack_exhaustion_drops_from_the_innermost_frame_outward() {
    let src = format!(
        "{HEAD}{}{}",
        f(
            "rec(n: Int) -> Int",
            "let p = mk(n);\nlet q = mk(0 - n);\nreturn rec(n + 1);"
        ),
        f("t() -> Int", "let a = mk(100000000);\nreturn rec(1);")
    );
    let o = traced(Some(4 * 1024 * 1024));
    let e = run_with(&src, "t", &[], &o).unwrap_err();
    let RunError::Stop(s) = &e.error else {
        panic!("{}", e.error)
    };
    assert_eq!(s.kind, StopKind::StackExhausted);
    assert_frames_unwind(&probe_ids(&e.drops));
    // 実行ファイルも、同じ形で破棄する（枠の数は違ってよい）。
    let (got, native) = common::native::native_drops(&[&src], "t", &[] as &[Arg]);
    assert_eq!(got.code, Some(70), "{}", got.stderr);
    assert_frames_unwind(&probe_ids(&native));
}

/// 各枠では q (-n) が p (n) より先、枠は深い方 (n が大きい) から浅い方へ、最後に呼び出し元の a。
fn assert_frames_unwind(ids: &[i64]) {
    let (last, frames) = ids.split_last().unwrap();
    assert_eq!(*last, 100_000_000);
    assert!(
        frames.len() >= 2 && frames.len() % 2 == 0,
        "{}",
        frames.len()
    );
    // 最も深い枠は表示できないことがあるので、深さは先頭の q (-n) から読む。
    let depth = -frames[0];
    assert_eq!(frames.len() as i64, depth * 2);
    let want: Vec<i64> = (1..=depth).rev().flat_map(|n| [-n, n]).collect();
    assert_eq!(frames, want.as_slice());
}

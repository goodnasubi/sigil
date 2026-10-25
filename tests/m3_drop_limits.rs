//! ADR 0006 の M3 の記録に書いた「既知の制限」を、現在の挙動のまま固定するテスト。
//!
//! 移動の追跡が粗い場所では、値は移動先でなく元の場所と同じ位置（共有している最後の持ち主が
//! 消えるとき）で破棄される。各テストの「仕様の想定」は条件 9 が本来求める順序で、M5 の出力で
//! 満たす。ここの `assert_eq!` は現状の順序であり、仕様の想定に近づけたらこのテストを直す
//! （直すのを忘れないための印）。
//!
//! 加えて、スタックの枯渇による停止のときの破棄の順序（条件 9 を満たすもの）を固定する。

use sigil::interp::{run_with, Options, RunError, StopKind};

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

/// `t` の本体を実行して、破棄された Probe の id を順に返す。
fn ids_of(body: &str) -> Vec<i64> {
    let src = format!("{HEAD}{}", f("t() -> Int", body));
    let o = run_with(&src, "t", &[], &traced(None)).unwrap_or_else(|e| panic!("{e}\n{src}"));
    probe_ids(&o.drops)
}

fn probe_ids(d: &[String]) -> Vec<i64> {
    d.iter()
        .filter_map(|s| s.strip_prefix("Probe { id: "))
        .map(|s| s.trim_end_matches(" }").parse().unwrap())
        .collect()
}

#[test]
fn partial_move_of_a_field_drops_with_the_whole_value() {
    // 仕様の想定: x (1)、続いて t の残り (2)。現状: 1 は t と共有しているので t の破棄の中で出る。
    let src = format!(
        "{HEAD}{}",
        f(
            "t() -> Int",
            "let t = Two { a: mk(1), b: mk(2) };\nlet y = mk(9);\nlet x = t.a;\nreturn 0;"
        )
    );
    let o = run_with(&src, "t", &[], &traced(None)).unwrap();
    assert_eq!(
        o.drops.as_slice(),
        [
            "Probe { id: 9 }",
            "Two { a: Probe { id: 1 }, b: Probe { id: 2 } }",
            "Probe { id: 1 }",
            "Probe { id: 2 }",
        ]
    );
}

#[test]
fn a_match_binding_that_moves_drops_with_the_scrutinee() {
    // 仕様の想定: 7 (腕の局所)、1 (束縛 p。腕の終わり)、5。現状: p の値は o の破棄まで残る。
    let v = ids_of(
        "let o = Some(mk(1));\nlet z = mk(5);\nmatch o {\n    Some(p) => {\n        let q = mk(7);\n    }\n    None => {}\n}\nreturn 0;",
    );
    assert_eq!(v, [7, 5, 1]);
}

#[test]
fn a_consuming_method_receiver_drops_with_the_original_place() {
    // 仕様の想定: map の中で p (1) が先に破棄され、そのあと z (5)。現状: 1 は o の位置まで残る。
    let v = ids_of("let o = Some(mk(1));\nlet z = mk(5);\nlet m = o.map(|p| p.id);\nreturn 0;");
    assert_eq!(v, [5, 1]);
    // 仕様の想定: 既定値 3 は呼び出しの中で、m (1)、z (5) の順。現状: 1 は o と共有で最後。
    let v = ids_of("let o = Some(mk(1));\nlet z = mk(5);\nlet m = o.unwrap_or(mk(3));\nreturn 0;");
    assert_eq!(v, [3, 5, 1]);
}

#[test]
fn a_captured_value_drops_at_its_original_place_not_with_the_closure() {
    // 仕様の想定: w (6)、クロージャ c の環境の p (1)、z (5)。現状: p は元の場所 (最初の宣言) の位置。
    let v = ids_of(
        "let p = mk(1);\nlet z = mk(5);\nlet c = move || p.id;\nlet w = mk(6);\nreturn c();",
    );
    assert_eq!(v, [6, 5, 1]);
}

#[test]
fn a_closure_is_treated_as_copyable_so_a_move_of_it_does_not_empty_the_source() {
    // 仕様の想定: k (7)、d の環境の p (1)、w (6)。現状: クロージャは Copyable として扱うので
    // `let d = c;` で c は空にならず、環境の p は元の場所 (最初の宣言) の位置まで残る。
    let v = ids_of(
        "let p = mk(1);\nlet c = move || p.id;\nlet w = mk(6);\nlet d = c;\nlet k = mk(7);\nreturn d();",
    );
    assert_eq!(v, [7, 6, 1]);
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
    let ids = probe_ids(&e.drops);
    // 各枠では q (-n) が p (n) より先、枠は深い方 (n が大きい) から浅い方へ、最後に呼び出し元の a。
    let (last, frames) = ids.split_last().unwrap();
    assert_eq!(*last, 100_000_000);
    assert!(
        frames.len() >= 2 && frames.len() % 2 == 0,
        "{}",
        frames.len()
    );
    let depth = (frames.len() / 2) as i64;
    let want: Vec<i64> = (1..=depth).rev().flat_map(|n| [-n, n]).collect();
    assert_eq!(frames, want.as_slice());
}

//! ADR 0026 PR 4-2g: 並行処理のサンプル `examples/06-concurrency` の検証。
//!
//! 構文・正規形・検査・仕様の出力・評価器での実行結果を固定する。
//! 評価器の実行は、実行順の種を変えても結果が変わらないことも確かめる。

use sigil::check::check;
use sigil::exec::{Arg, Options};
use sigil::fmt::format;
use sigil::ir_eval::{run, run_with};
use sigil::spec::spec;

const PATH: &str = "examples/06-concurrency/concurrency";

fn src() -> String {
    std::fs::read_to_string(format!("{PATH}.sigil")).unwrap()
}

fn value(f: &str, args: &[i64]) -> String {
    let args: Vec<Arg> = args.iter().map(|&n| Arg::Int(n)).collect();
    run(&src(), f, &args)
        .unwrap_or_else(|e| panic!("{f}: {:?}", e.error))
        .value
}

#[test]
fn example_parses_checks_and_is_canonical() {
    let s = src();
    sigil::parse(&s).unwrap();
    if let Err(e) = check(&s) {
        panic!("{e}");
    }
    assert_eq!(format(&s).unwrap(), s);
}

#[test]
fn spec_matches_expected_output() {
    let want = std::fs::read_to_string(format!("{PATH}.spec")).unwrap();
    assert_eq!(spec(&src()).unwrap(), want);
}

#[test]
fn scope_spawn_and_join() {
    assert_eq!(value("sum_of_doubles", &[3, 4]), "14");
}

#[test]
fn channel_relay() {
    assert_eq!(value("relay", &[10]), "33");
}

#[test]
fn timeout_cancels_on_the_virtual_clock() {
    assert_eq!(value("first_within", &[50, 7]), "None");
    assert_eq!(value("first_within", &[500, 7]), "Some(7)");
}

#[test]
fn lock_acquire_with_an_argument() {
    // `&Lock<Int>` の引数には、評価器の入口では整数を渡す（初期値になる）。
    assert_eq!(value("bump", &[7]), "1");
}

#[test]
fn atomic_roundtrip() {
    assert_eq!(value("publish", &[9]), "9");
}

#[test]
fn results_do_not_depend_on_the_schedule_seed() {
    for seed in 1..=8 {
        let opts = Options {
            schedule_seed: Some(seed),
            ..Options::default()
        };
        for (f, args, want) in [
            ("sum_of_doubles", vec![3, 4], "14"),
            ("relay", vec![10], "33"),
            ("first_within", vec![50, 7], "None"),
        ] {
            let a: Vec<Arg> = args.iter().map(|&n| Arg::Int(n)).collect();
            let o = run_with(&src(), f, &a, &opts).unwrap_or_else(|e| panic!("{:?}", e.error));
            assert_eq!(o.value, want, "{f} seed={seed}");
        }
    }
}

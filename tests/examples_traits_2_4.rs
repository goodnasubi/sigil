//! ADR 0022（PR 2-4）のサンプル `examples/08-traits` の検証。
//!
//! 構文・正規形・検査・仕様の出力・評価器での実行結果を固定する。

use sigil::check::check;
use sigil::exec::Arg;
use sigil::fmt::format;
use sigil::ir_eval::run;
use sigil::spec::spec;

const PATH: &str = "examples/08-traits/traits";

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
fn trait_methods_dispatch_per_type() {
    assert_eq!(value("square_area", &[5]), "25");
    assert_eq!(value("rectangle_area", &[3, 4]), "12");
    assert_eq!(value("rectangle_label", &[3, 4]), "\"rectangle\"");
}

#[test]
fn generic_bound_calls_trait_method() {
    assert_eq!(value("square_doubled_area", &[5]), "50");
}

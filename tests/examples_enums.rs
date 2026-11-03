//! サンプル `examples/09-enums` の検証。
//!
//! 構文・正規形・検査・仕様の出力・評価器での実行結果を固定する。

use sigil::check::check;
use sigil::exec::Arg;
use sigil::fmt::format;
use sigil::ir_eval::run;
use sigil::spec::spec;

const PATH: &str = "examples/09-enums/enums";

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
fn variant_payloads_are_extracted() {
    assert_eq!(value("received_id", &[42]), "42");
    assert_eq!(value("paid_code_and_amount", &[7, 1200]), "101200");
}

#[test]
fn transitions_follow_the_lifecycle() {
    assert_eq!(value("shipped_code", &[7, 1200, 555]), "2");
    assert_eq!(value("shipped_cancel_code", &[7, 1200, 555]), "2");
    assert_eq!(value("paid_cancel_finished", &[7, 1200]), "true");
}

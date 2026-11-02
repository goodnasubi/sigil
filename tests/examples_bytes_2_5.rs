//! ADR 0023（PR 2-5）のサンプル `examples/07-bytes` の検証。
//!
//! 構文・正規形・検査・仕様の出力・評価器での実行結果を固定する。

use sigil::check::check;
use sigil::exec::Arg;
use sigil::fmt::format;
use sigil::ir_eval::run;
use sigil::spec::spec;

const PATH: &str = "examples/07-bytes/bytes";

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
fn packet_round_trip() {
    assert_eq!(value("sample_len", &[9, 1, 2]), "7");
    assert_eq!(value("sample_tag", &[9, 1, 2]), "9");
    assert_eq!(value("sample_declared_len", &[9, 1, 2]), "2");
}

#[test]
fn checksum_mixes_all_bytes() {
    assert_eq!(value("sample_checksum", &[9, 1, 2]), "301989889");
}

#[test]
fn scramble_masks_and_rotates() {
    assert_eq!(value("scramble", &[4660]), "2147483668");
}

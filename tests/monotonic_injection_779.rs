mod common;

use sigil::codegen::Opts;
use sigil::exec::{Arg, Options};
use sigil::ir_eval;

const SOURCE: &str = r#"
module test.monotonic_injection
  targets server
  trust none

fn read_clock() -> Option<Int>
  intent "判定に使う単調時刻を読む"
  requires none
  ensures none
  effects clock
{
  return clock.monotonic_nanos();
}

fn boundaries(start: Int) -> (Int, Bool)
  intent "空のバケットの補充数と検査期限切れを返す"
  requires none
  ensures none
  effects clock
{
  match read_clock() {
    Some(now) => {
      let elapsed = now - start;
      let tokens = elapsed / 5000000;
      let expired = elapsed >= 10000000;
      return (tokens, expired);
    }
    None => {
      return (0, true);
    }
  }
}

fn read_both() -> (Option<Int>, Option<Int>)
  intent "単調時計と壁時計を独立に読む"
  requires none
  ensures none
  effects clock
{
  return (read_clock(), clock.unix_seconds());
}
"#;

#[test]
fn refill_and_deadline_boundaries_in_eval_and_native() {
    sigil::verify::verify_sources(&[SOURCE], "boundaries").unwrap();
    // 秒をまたぐ値で、timespec の秒とナノ秒の両方が判定に反映されることを確かめる。
    let start = 1_995_000_123;
    let args = [Arg::Int(start)];
    for (elapsed, expected) in [
        (4_999_999, "(0, false)"),
        (5_000_000, "(1, false)"),
        (5_000_001, "(1, false)"),
        (9_999_999, "(1, false)"),
        (10_000_000, "(2, true)"),
        (10_000_001, "(2, true)"),
    ] {
        let now = start + elapsed;
        let opts = Options {
            monotonic_clock: Some(Ok(now)),
            ..Options::captured()
        };
        let got = ir_eval::run_with(SOURCE, "boundaries", &args, &opts).unwrap();
        assert_eq!(got.value, expected, "評価器: 経過 {elapsed} ns");

        let got = run_native_with_clock(now, &args);
        assert_eq!(got.code, Some(0), "ネイティブ: {}", got.stderr);
        assert_eq!(
            got.stdout,
            format!("{expected}\n"),
            "ネイティブ: 経過 {elapsed} ns"
        );
    }
}

fn run_native_with_clock(now: i64, args: &[Arg]) -> common::native::Ran {
    // ネイティブでは試験用の埋め込みモジュールで単調時計の関数だけを差し替える。
    let (wall_clock, _) = include_str!("../src/stdlib/os_clock.sigil")
        .split_once("pub fn monotonic_nanos()")
        .unwrap();
    let clock = format!(
        r#"{wall_clock}
pub fn monotonic_nanos() -> Option<Int>
  intent "テスト用の固定単調時刻を返す"
  requires none
  ensures none
  effects none
{{
  return Some({now});
}}
"#
    );
    let opts = sigil::effects::Options {
        embedded_extra: vec![("std.os.clock".to_owned(), clock)],
        ..Default::default()
    };
    let program = ir_eval::lower_sources_opts(&[SOURCE], "boundaries", &opts).unwrap();
    let dir = common::native::scratch("monotonic-injection");
    let exe = dir.join("prog");
    sigil::native::build_program(&program, &Opts::default(), &exe).unwrap();
    let got = common::native::exec(&exe, args);
    std::fs::remove_dir_all(dir).unwrap();
    got
}

#[test]
fn injected_clocks_are_independent_and_handle_failure() {
    let mut opts = Options {
        wall_clock: Some(Ok(1_700_000_000)),
        ..Options::captured()
    };
    for (injected, expected) in [
        (Ok(0), "(Some(0), Some(1700000000))"),
        (
            Ok(i64::MAX),
            "(Some(9223372036854775807), Some(1700000000))",
        ),
        (Ok(-1), "(None, Some(1700000000))"),
        (Err(1), "(None, Some(1700000000))"),
    ] {
        opts.monotonic_clock = Some(injected);
        let got = ir_eval::run_with(SOURCE, "read_both", &[], &opts).unwrap();
        assert_eq!(got.value, expected, "{injected:?}");
    }

    opts.monotonic_clock = Some(Ok(1_000_000_001));
    opts.wall_clock = Some(Err(1));
    let got = ir_eval::run_with(SOURCE, "read_both", &[], &opts).unwrap();
    assert_eq!(got.value, "(Some(1000000001), None)");
}

mod common;

use sigil::exec::Options;

const SOURCE: &str = r#"
module test.monotonic_clock
  targets server
  trust none

fn read_clock() -> Option<Int>
  intent "公開 API からナノ秒単位の単調時刻を読む"
  requires none
  ensures none
  effects clock
{
  return clock.monotonic_nanos();
}

fn advances() -> Bool
  intent "単調時刻が逆行せず、繰り返すうちに進むことを確認する"
  requires none
  ensures none
  effects clock
{
  let first = read_clock().unwrap_or(-1);
  if first < 0 {
    return false;
  }
  let mut previous = first;
  let mut i = 0;
  while i < 64 {
    let now = read_clock().unwrap_or(-1);
    if now < previous {
      return false;
    }
    previous = now;
    i = i + 1;
  }
  return previous > first;
}
"#;

#[test]
fn trust_none_clock_advances_in_eval_and_native() {
    assert_eq!(common::run(SOURCE, "advances", &[]).unwrap().value, "true");
}

#[test]
fn wall_clock_injection_does_not_affect_monotonic_clock() {
    let mut opts = Options::captured();
    opts.wall_clock = Some(Err(1));
    let outcome = sigil::ir_eval::run_with(SOURCE, "advances", &[], &opts).unwrap();
    assert_eq!(outcome.value, "true");
}

#[test]
fn monotonic_clock_requires_clock_effect() {
    let source = SOURCE.replace("effects clock", "effects none");
    let error = sigil::check::check(&source).unwrap_err();
    assert_eq!(error.kind, sigil::ErrorKind::EffectMissing);
}

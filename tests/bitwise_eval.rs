#[path = "common/mod.rs"]
mod common;
use common::run;
use sigil::exec::{RunError, StopKind};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn fnsrc(body: &str) -> String {
    format!("{HEAD}{body}")
}

#[test]
fn test_bitwise_eval() {
    let src = fnsrc(
        r#"
fn test_f() -> Int
  intent "test bitwise execution"
  requires none
  ensures none
  effects none
{
  let a = 0b1100;
  let b = 0b1010;
  let and_val = a & b; // 0b1000 = 8
  let or_val = a | b;  // 0b1110 = 14
  let xor_val = a ^ b; // 0b0110 = 6
  let not_val = ~0;    // -1
  let shl_val = 1 << 4; // 16
  let shr_val = 16 >> 2; // 4
  return and_val + or_val + xor_val + not_val + shl_val + shr_val;
}
"#,
    );
    let out = run(&src, "test_f", &[]).expect("run failed");
    // 8 + 14 + 6 + (-1) + 16 + 4 = 47
    assert_eq!(out.value, "47");
}

#[test]
fn test_shift_out_of_range_stop() {
    let src = fnsrc(
        r#"
fn test_f() -> Int
  intent "shift out of range stops"
  requires none
  ensures none
  effects none
{
  let x = 1 << 64;
  return x;
}
"#,
    );
    let err = run(&src, "test_f", &[]).unwrap_err();
    match err.error {
        RunError::Stop(s) => assert_eq!(s.kind, StopKind::Overflow),
        other => panic!("予期しないエラー: {other:?}"),
    }
}

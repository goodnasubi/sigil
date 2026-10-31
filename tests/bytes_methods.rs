#[path = "common/mod.rs"]
mod common;
use common::run;
use sigil::check::check;
use sigil::parse;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn fnsrc(body: &str) -> String {
    format!("{HEAD}{body}")
}

#[test]
fn test_bytes_methods_and_helpers() {
    let src = fnsrc(
        r#"
fn test_bytes(s: &String) -> Int
  intent "test bytes methods"
  requires none
  ensures none
  effects none
{
  let b = Bytes.new();
  let l = b.len();
  let b2 = s.as_bytes();
  let eq = ct_eq(&b, &b2);
  let n = 10;
  let rot = n.rotr32(2);
  let rot64 = n.rotr(2, 64);
  let sh = n.ashr(1);
  return l + rot + rot64 + sh;
}
"#,
    );
    let parsed = parse(&src);
    assert!(parsed.is_ok(), "パース成功するはず: {:?}", parsed.err());
    let checked = check(&src);
    assert!(checked.is_ok(), "型検査成功するはず: {:?}", checked.err());
}

#[test]
fn test_bytes_execution() {
    let src = fnsrc(
        r#"
fn test_exec() -> Int
  intent "exec"
  requires none
  ensures none
  effects none
{
  let mut b = Bytes.new();
  b.push(1);
  b.push(2);
  b.push_u16_be(0x1234);
  let l = b.len();
  let b1 = b.get(0).unwrap_or(0);
  let b2 = b.get(1).unwrap_or(0);
  let u16_val = b.read_u32_be(0).unwrap_or(0);
  let mut bonus = 0;
  if u16_val == 0x01021234 {
    bonus = 10;
  }
  return l + b1 + b2 + bonus;
}
"#,
    );
    let out = run(&src, "test_exec", &[]).expect("run failed");
    // l = 4, b1 = 1, b2 = 2 => 7, u16_val (read_u32_be) == 0x01021234 => +10 = 17
    assert_eq!(out.value, "17");
}

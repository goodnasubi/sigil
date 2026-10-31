use sigil::check::check;
use sigil::{parse, ErrorKind};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn fnsrc(body: &str) -> String {
    format!("{HEAD}{body}")
}

#[test]
fn test_bitwise_syntax_and_precedence() {
    let src = fnsrc(
        r#"
fn test_ops(a: Int, b: Int, c: Int) -> Int
  intent "test bitwise operators"
  requires none
  ensures none
  effects none
{
  let x = a & b | c ^ ~a;
  let s = (a << 2) >> 1;
  let hex = 0x1f_2a + 0b1010;
  return x + s + hex;
}
"#,
    );
    let parsed = parse(&src);
    assert!(parsed.is_ok(), "パース成功するはず: {:?}", parsed.err());
    let checked = check(&src);
    assert!(checked.is_ok(), "型検査成功するはず: {:?}", checked.err());
}

#[test]
fn test_bitwise_type_error_on_non_int() {
    let src = fnsrc(
        r#"
fn test_err(a: Bool, b: Int) -> Int
  intent "err"
  requires none
  ensures none
  effects none
{
  return a & b;
}
"#,
    );
    let checked = check(&src);
    assert!(checked.is_err());
    assert_eq!(checked.unwrap_err().kind, ErrorKind::OperandTypes);
}

#[test]
fn test_bitnot_type_error_on_bool() {
    let src = fnsrc(
        r#"
fn test_err(a: Bool) -> Bool
  intent "err"
  requires none
  ensures none
  effects none
{
  return ~a;
}
"#,
    );
    let checked = check(&src);
    assert!(checked.is_err());
    assert_eq!(checked.unwrap_err().kind, ErrorKind::OperandTypes);
}

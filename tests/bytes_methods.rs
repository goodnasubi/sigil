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

use sigil::check::{check, check_with};
use sigil::effects::Options;
use sigil::error::ErrorKind;
use sigil::fmt::format;
use sigil::parser::parse;

#[test]
fn global_syntax_and_fmt() {
    let src = r#"module m.test
  targets server
  trust base

global ALLOCATOR_STATE: Int = 42

global mut COUNTER: Int = 0

fn test() -> Int
  intent "test"
  requires none
  ensures none
  effects none
  unchecked "ok"
{
  COUNTER = COUNTER + 1;
  return ALLOCATOR_STATE + COUNTER;
}
"#;

    let parsed = parse(src).expect("parse succeeded");
    assert_eq!(parsed.items.len(), 3);

    let formatted = format(src).expect("format succeeded");
    assert_eq!(format(&formatted).unwrap(), formatted);
}

#[test]
fn global_outside_trust_base_is_rejected() {
    let src = r#"module m.test
  targets server
  trust none

global ALLOCATOR_STATE: Int = 42

fn test() -> Int
  intent "test"
  requires none
  ensures none
  effects none
{
  return ALLOCATOR_STATE;
}
"#;

    let err = check(src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::GlobalOutsideTrustBase);
}

#[test]
fn global_bad_name_form_is_rejected() {
    let src = r#"module m.test
  targets server
  trust base

global allocator_state: Int = 42
"#;

    let err = parse(src).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadNameForm);
}

#[test]
fn global_mut_access_outside_unchecked_is_rejected() {
    let src = r#"module m.test
  targets server
  trust base

global mut COUNTER: Int = 0

fn test() -> Int
  intent "test"
  requires none
  ensures none
  effects none
{
  return COUNTER;
}
"#;

    let opts = Options {
        trust_base: vec!["m.test".into()],
        ..Default::default()
    };
    let err = check_with(src, &opts).unwrap_err();
    assert_eq!(err.kind, ErrorKind::GlobalMutOutsideUnchecked);
}

#[test]
fn global_mut_assign_outside_unchecked_is_rejected() {
    let src = r#"module m.test
  targets server
  trust base

global mut COUNTER: Int = 0

fn test()
  intent "test"
  requires none
  ensures none
  effects none
{
  COUNTER = 1;
  return;
}
"#;

    let opts = Options {
        trust_base: vec!["m.test".into()],
        ..Default::default()
    };
    let err = check_with(src, &opts).unwrap_err();
    assert_eq!(err.kind, ErrorKind::GlobalMutOutsideUnchecked);
}

#[test]
fn global_immutable_assign_is_rejected() {
    let src = r#"module m.test
  targets server
  trust base

global COUNTER: Int = 0

fn test()
  intent "test"
  requires none
  ensures none
  effects none
  unchecked "ok"
{
  COUNTER = 1;
  return;
}
"#;

    let opts = Options {
        trust_base: vec!["m.test".into()],
        ..Default::default()
    };
    let err = check_with(src, &opts).unwrap_err();
    assert_eq!(err.kind, ErrorKind::AssignToImmutable);
}

#[test]
fn global_valid_usage_in_unchecked() {
    let src = r#"module m.test
  targets server
  trust base

global ALLOCATOR_STATE: Int = 42

global mut COUNTER: Int = 0

fn test() -> Int
  intent "test"
  requires none
  ensures none
  effects none
  unchecked "safe"
{
  COUNTER = COUNTER + 1;
  return ALLOCATOR_STATE + COUNTER;
}
"#;

    let opts = Options {
        trust_base: vec!["m.test".into()],
        ..Default::default()
    };
    check_with(src, &opts).expect("check succeeded");
}

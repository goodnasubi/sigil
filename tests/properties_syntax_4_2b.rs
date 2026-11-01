use sigil::check::check_with;
use sigil::effects::Options;
use sigil::error::ErrorKind;
use sigil::fmt::format;
use sigil::spec::{extract_with, spec_with};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";
const TRUST_HEAD: &str = "module m.a\n  targets server\n  trust base\n\n";

const CL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

fn func(sig: &str, body: &str) -> String {
    format!("fn {sig}\n{CL}{{\n{body}\n}}\n")
}

fn opts_with_trust() -> Options {
    Options {
        trust_base: vec!["m.a".to_string()],
        ..Default::default()
    }
}

#[test]
fn properties_syntax_and_fmt() {
    let src = format!(
        "{TRUST_HEAD}struct MyHandle {{\n  ptr: Int,\n}}\n\nproperties MyHandle: Shareable, Sendable \"ポインタはアトミック管理されている\"\n"
    );
    let expected_fmt = format!(
        "{TRUST_HEAD}struct MyHandle {{\n  ptr: Int,\n}}\n\nproperties MyHandle: Sendable, Shareable \"ポインタはアトミック管理されている\"\n"
    );

    let formatted = format(&src).expect("formatting should succeed");
    assert_eq!(formatted, expected_fmt);

    // 冪等性
    let formatted_again = format(&formatted).expect("formatting should be idempotent");
    assert_eq!(formatted_again, formatted);
}

#[test]
fn properties_spec_text_and_json() {
    let src = format!(
        "{TRUST_HEAD}struct MyHandle {{\n  ptr: Int,\n}}\n\nproperties MyHandle: Sendable, Shareable \"信頼された前提\"\n"
    );
    let opts = opts_with_trust();
    let text = spec_with(&src, &opts).unwrap();
    assert!(text.contains("性質の手付け MyHandle"));
    assert!(text.contains("性質: Sendable, Shareable"));
    assert!(text.contains("前提: 信頼された前提"));

    let s = extract_with(&src, &opts).unwrap();
    let json = s.to_json().to_string();
    assert!(json.contains("\"kind\":\"properties\"") || json.contains("\"kind\": \"properties\""));
    assert!(json.contains("\"name\":\"MyHandle\"") || json.contains("\"name\": \"MyHandle\""));
    assert!(json.contains("信頼された前提"));
}

#[test]
fn properties_outside_trust_base_rejected() {
    let src = format!(
        "{HEAD}struct MyHandle {{\n  ptr: Int,\n}}\n\nproperties MyHandle: Sendable \"前提文\"\n"
    );
    let err = check_with(&src, &Options::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::PropertiesOutsideTrustBase);
}

#[test]
fn properties_empty_premise_rejected() {
    let src = format!(
        "{TRUST_HEAD}struct MyHandle {{\n  ptr: Int,\n}}\n\nproperties MyHandle: Sendable \"\"\n"
    );
    let err = check_with(&src, &opts_with_trust()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::EmptyPremise);
}

#[test]
fn properties_unknown_type_rejected() {
    let src = format!("{TRUST_HEAD}properties NonExistent: Sendable \"前提\"\n");
    let err = check_with(&src, &opts_with_trust()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownType);
}

#[test]
fn properties_unknown_property_rejected() {
    let src = format!(
        "{TRUST_HEAD}struct MyHandle {{\n  ptr: Int,\n}}\n\nproperties MyHandle: FooBar \"前提\"\n"
    );
    let err = check_with(&src, &opts_with_trust()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::UnknownProperty);
}

#[test]
fn properties_allows_generic_types_with_addr() {
    // String, List, Map が Sendable / Shareable として扱われること
    let prog = format!(
        "{HEAD}{}{}{}",
        func("test_sendable<T: Sendable>(x: T) -> T", "  return x;"),
        func("test_shareable<T: Shareable>(x: T) -> T", "  return x;"),
        func(
            "run(s: String, list: List<Int>) -> Int",
            "  let s1 = test_sendable(s);\n  let s2 = test_shareable(s1);\n  let l1 = test_sendable(list);\n  let l2 = test_shareable(l1);\n  return 0;"
        )
    );
    check_with(&prog, &Options::default())
        .expect("String and List should satisfy Sendable and Shareable bounds");
}

#[test]
fn properties_generic_type_respects_type_argument_properties() {
    // List<LockGuard<Int>> は LockGuard が Sendable ではないため Sendable 境界を満たせないこと
    let prog = format!(
        "{HEAD}{}{}",
        func("test_sendable<T: Sendable>(x: T) -> T", "  return x;"),
        func(
            "run(bad_list: List<LockGuard<Int>>) -> Int",
            "  let x = test_sendable(bad_list);\n  return 0;"
        )
    );
    let err = check_with(&prog, &Options::default()).unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConstraintUnsatisfied);
}

use sigil::fmt::{check, format};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn fnsrc(effects: &str, body: &str) -> String {
    format!(
        "{HEAD}fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}}}\n"
    )
}

fn body(stmts: &str) -> String {
    fnsrc("none", stmts)
}

fn fmt(src: &str) -> String {
    match format(src) {
        Ok(s) => s,
        Err(e) => panic!("{e}\n{src}"),
    }
}

/// 整形結果が `want` に一致し、冪等で、`want` 自体は違反なしであること。
fn expect(src: &str, want: &str) {
    let got = fmt(src);
    assert_eq!(got, want, "入力:\n{src}");
    assert_eq!(fmt(&got), got, "冪等でない");
    assert!(check(&got).unwrap().is_none());
    assert!(check(src).unwrap().is_some(), "違反として検出されない");
}

fn unchanged(src: &str) {
    assert_eq!(fmt(src), src);
    assert!(check(src).unwrap().is_none());
}

#[test]
fn examples_are_canonical() {
    for p in [
        "examples/01-average/c-brace.sigil",
        "examples/02-read-port/c-brace.sigil",
        "examples/03-ownership/ownership.sigil",
        "examples/04-effects/effects.sigil",
        "examples/05-control/control.sigil",
    ] {
        let s = std::fs::read_to_string(p).unwrap();
        assert!(check(&s).unwrap().is_none(), "{p} が正規形でない");
        assert_eq!(fmt(&s), s, "{p}");
    }
}

#[test]
fn trailing_commas() {
    expect(&body("  return f(1, 2,);\n"), &body("  return f(1, 2);\n"));
    expect(
        &format!("{HEAD}struct P {{ x: Int, y: Int, }}\n"),
        &format!("{HEAD}struct P {{\n  x: Int,\n  y: Int,\n}}\n"),
    );
    expect(
        &fnsrc("fs.read,", "  return x;\n"),
        &fnsrc("fs.read", "  return x;\n"),
    );
}

#[test]
fn redundant_parens() {
    expect(&body("  return (x);\n"), &body("  return x;\n"));
    expect(
        &body("  return (x + 1) + 2;\n"),
        &body("  return x + 1 + 2;\n"),
    );
    expect(&body("  return x * (y);\n"), &body("  return x * y;\n"));
}

#[test]
fn needed_parens_stay() {
    unchanged(&body("  return (x + 1) * 2;\n"));
    unchanged(&body("  return x - (y - z);\n"));
    unchanged(&body("  return (a || b) && c;\n"));
    unchanged(&body("  return (1).abs();\n"));
    unchanged(&body("  return (&(&x)).len();\n"));
}

#[test]
fn comment_spacing() {
    expect(
        &body("  //x\n  return x;\n"),
        &body("  // x\n  return x;\n"),
    );
    expect(&body("  return x; // c\n"), &body("  return x;  // c\n"));
    expect(
        &body("  return x;   //c   \n"),
        &body("  return x;  // c\n"),
    );
    unchanged(&body("  //\n  return x;\n"));
}

#[test]
fn return_unit_removed() {
    let src = format!(
        "{HEAD}fn g()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let a = 1;\n  return;\n}}\n"
    );
    let want = src.replace("  return;\n", "");
    expect(&src, &want);
}

#[test]
fn closure_single_return() {
    expect(
        &body("  let c = |y| { return y + 1; };\n  return x;\n"),
        &body("  let c = |y| y + 1;\n  return x;\n"),
    );
}

#[test]
fn stmt_match_single_return() {
    expect(
        &body("  match x {\n    1 => { return 1; }\n    _ => { return 2; }\n  }\n"),
        &body("  return match x {\n    1 => 1,\n    _ => 2,\n  };\n"),
    );
}

#[test]
fn value_match_final_comma() {
    expect(
        &body("  let y = match x {\n    1 => 2,\n    _ => 3\n  };\n  return y;\n"),
        &body("  let y = match x {\n    1 => 2,\n    _ => 3,\n  };\n  return y;\n"),
    );
}

#[test]
fn effects_order_and_fold() {
    expect(
        &fnsrc("fs.write, console, fs.read", "  return x;\n"),
        &fnsrc("console, fs", "  return x;\n"),
    );
    expect(
        &fnsrc("console, console", "  return x;\n"),
        &fnsrc("console", "  return x;\n"),
    );
    expect(
        &fnsrc("fs, fs.read", "  return x;\n"),
        &fnsrc("fs", "  return x;\n"),
    );
    unchanged(&fnsrc("console, fs.read", "  return x;\n"));
}

#[test]
fn effects_of_first() {
    let src = format!(
        "{HEAD}fn g(f: fn(Int) -> Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects console, of(f)\n{{\n  return 1;\n}}\n"
    );
    let want = src.replace("console, of(f)", "of(f), console");
    expect(&src, &want);
}

#[test]
fn literals() {
    expect(&body("  return 1.50;\n"), &body("  return 1.5;\n"));
    unchanged(&body("  return 1.0;\n"));
    expect(
        &body("  return \"a\\u{9}b\";\n"),
        &body("  return \"a\\tb\";\n"),
    );
}

#[test]
fn layout() {
    expect(&body("    return x;\n"), &body("  return x;\n"));
    expect(&body("  return x;  \n"), &body("  return x;\n"));
    let s = body("  return x;\n");
    expect(s.trim_end_matches('\n'), &s);
}

#[test]
fn blank_lines() {
    unchanged(&body("  let a = 1;\n\n  return a;\n"));
    expect(
        &body("  let a = 1;\n\n\n  return a;\n"),
        &body("  let a = 1;\n\n  return a;\n"),
    );
}

#[test]
fn struct_lit_in_condition_keeps_parens() {
    let src = body("  if (P { x: 1 }) == p {\n    return 1;\n  }\n  return 2;\n");
    unchanged(&src);
}

#[test]
fn idempotent_on_messy_input() {
    let src = std::fs::read_to_string("tests/data/messy.sigil").unwrap();
    let once = fmt(&src);
    assert_ne!(once, src);
    assert_eq!(fmt(&once), once);
}

#[test]
fn check_reports_line() {
    let src = body("  //x\n  return x;\n");
    let d = check(&src).unwrap().unwrap();
    assert_eq!(d.found, "  //x");
    assert_eq!(d.expected, "  // x");
    assert_eq!(d.line, src.lines().position(|l| l == "  //x").unwrap() + 1);
}

#[test]
fn parse_error_is_error() {
    assert!(format("fn").is_err());
    assert!(check("fn").is_err());
}

use sigil::{parse, ErrorKind, Phase};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn fnsrc(body: &str) -> String {
    format!("{HEAD}{body}")
}

fn kind(src: &str) -> ErrorKind {
    match parse(src) {
        Ok(_) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => e.kind,
    }
}

fn ok(src: &str) {
    if let Err(e) = parse(src) {
        panic!("{e}\n{src}");
    }
}

const GOOD_FN: &str = "fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return x;\n}\n";

#[test]
fn minimal_ok() {
    ok(&fnsrc(GOOD_FN));
}

// ---- 字句エラー (ADR0009) ----
#[test]
fn lex_tab() {
    assert_eq!(kind(&format!("{}\t", fnsrc(GOOD_FN))), ErrorKind::Tab);
}

#[test]
fn lex_cr() {
    assert_eq!(kind(&fnsrc(GOOD_FN).replace('\n', "\r\n")), ErrorKind::CarriageReturn);
}

#[test]
fn lex_block_comment() {
    assert_eq!(kind(&fnsrc("/* x */\n")), ErrorKind::BlockComment);
}

#[test]
fn lex_future_reserved() {
    let s = fnsrc(&GOOD_FN.replace("return x;", "let async = 1;\n  return x;"));
    assert_eq!(kind(&s), ErrorKind::FutureReservedWord);
}

#[test]
fn lex_bad_escape() {
    let s = fnsrc(&GOOD_FN.replace("\"x\"", "\"a\\qb\""));
    assert_eq!(kind(&s), ErrorKind::BadEscape);
}

#[test]
fn lex_newline_in_string() {
    let s = fnsrc(&GOOD_FN.replace("\"x\"", "\"a\nb\""));
    assert!(matches!(kind(&s), ErrorKind::NewlineInString | ErrorKind::UnterminatedString));
}

#[test]
fn lex_leading_zero() {
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", "return 007;"))), ErrorKind::LeadingZero);
}

#[test]
fn lex_hex_and_exponent() {
    assert!(matches!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", "return 0x10;"))),
        ErrorKind::BadNumber | ErrorKind::LeadingZero
    ));
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", "return 1e3;"))), ErrorKind::BadNumber);
}

#[test]
fn lex_non_ascii_ident() {
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", "let é = 1;\n  return x;"))), ErrorKind::NonAsciiOutsideLiteral);
}

#[test]
fn lex_compound_assign_and_range() {
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", "x += 1;\n  return x;"))), ErrorKind::CompoundAssign);
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", "return x..x;"))), ErrorKind::RangeOperator);
}

#[test]
fn error_phase_and_position() {
    let e = parse(&fnsrc("/* x */\n")).unwrap_err();
    assert_eq!(e.kind.phase(), Phase::Lex);
    assert_eq!(e.span.line, 5);
    let e = parse("fn").unwrap_err();
    assert_eq!(e.kind.phase(), Phase::Syntax);
}

// ---- module (ADR0008) ----
#[test]
fn module_missing() {
    assert_eq!(kind(GOOD_FN), ErrorKind::ModuleMissing);
}

#[test]
fn module_duplicate() {
    assert_eq!(kind(&format!("{HEAD}{HEAD}{GOOD_FN}")), ErrorKind::ModuleDuplicate);
}

#[test]
fn module_clause_missing() {
    assert_eq!(kind(&format!("module m.a\n  targets server\n\n{GOOD_FN}")), ErrorKind::ModuleClauseMissing);
}

#[test]
fn module_clause_order() {
    assert_eq!(kind(&format!("module m.a\n  trust none\n  targets server\n\n{GOOD_FN}")), ErrorKind::ModuleClauseOrder);
}

#[test]
fn module_targets() {
    assert_eq!(kind(&format!("module m.a\n  targets server, server\n  trust none\n\n{GOOD_FN}")), ErrorKind::TargetsDuplicate);
    assert_eq!(kind(&format!("module m.a\n  targets server, browser\n  trust none\n\n{GOOD_FN}")), ErrorKind::TargetsUnsorted);
    assert_eq!(kind(&format!("module m.a\n  targets mars\n  trust none\n\n{GOOD_FN}")), ErrorKind::UnknownTarget);
    ok(&format!("module m.a\n  targets browser, server\n  trust none\n\n{GOOD_FN}"));
}

// ---- 契約節 (ADR0002/0008) ----
#[test]
fn clause_missing() {
    let s = fnsrc(&GOOD_FN.replace("  effects none\n", ""));
    assert_eq!(kind(&s), ErrorKind::MissingClause);
}

#[test]
fn clause_order() {
    let s = fnsrc(&GOOD_FN.replace("  requires none\n  ensures none\n", "  ensures none\n  requires none\n"));
    assert_eq!(kind(&s), ErrorKind::ClauseOrder);
}

#[test]
fn intent_empty() {
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("\"x\"", "\"\""))), ErrorKind::EmptyIntent);
}

#[test]
fn old_outside_ensures() {
    let s = fnsrc(&GOOD_FN.replace("requires none", "requires old(x) > 0"));
    assert_eq!(kind(&s), ErrorKind::OldOutsideEnsures);
    let s = fnsrc(&GOOD_FN.replace("return x;", "return old(x);"));
    assert_eq!(kind(&s), ErrorKind::OldOutsideEnsures);
    ok(&fnsrc(&GOOD_FN.replace("ensures none", "ensures result == old(x)")));
}

#[test]
fn result_outside_ensures() {
    let s = fnsrc(&GOOD_FN.replace("requires none", "requires result > 0"));
    assert_eq!(kind(&s), ErrorKind::ResultOutsideEnsures);
}

// ---- 名前 ----
#[test]
fn name_forms() {
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn myFn("))), ErrorKind::BadNameForm);
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn _f("))), ErrorKind::UnderscoreName);
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn match("))), ErrorKind::ReservedWordAsName);
    assert_eq!(kind(&fnsrc(&format!("type my_type = Int where self > 0;\n{GOOD_FN}"))), ErrorKind::BadNameForm);
}

// ---- 式 ----
#[test]
fn expr_errors() {
    let r = |e: &str| fnsrc(&GOOD_FN.replace("return x;", &format!("return {e};")));
    assert_eq!(kind(&r("1 < 2 < 3")), ErrorKind::ComparisonChain);
    assert_eq!(kind(&r("true && false || true")), ErrorKind::MixedAndOr);
    ok(&r("(true && false) || true"));
}

#[test]
fn stmt_errors() {
    let b = |s: &str| fnsrc(&GOOD_FN.replace("return x;", &format!("{s}\n  return x;")));
    assert_eq!(kind(&b("x + 1;")), ErrorKind::ExprStmtNotCall);
    assert_eq!(kind(&b("{ let y = 1; }")), ErrorKind::StandaloneBlock);
}

#[test]
fn match_arm_forms() {
    let stmt_ok = "match x {\n    0 => { return 1; }\n    _ => { return 2; }\n  }";
    ok(&fnsrc(&GOOD_FN.replace("return x;", stmt_ok)));
    let stmt_comma = "match x {\n    0 => { return 1; },\n    _ => { return 2; }\n  }";
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", stmt_comma))), ErrorKind::StmtMatchArmComma);
    let stmt_expr = "match x {\n    0 => 1,\n    _ => 2,\n  }";
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", stmt_expr))), ErrorKind::StmtMatchArmNotBlock);
    let val_ok = "return match x {\n    0 => 1,\n    _ => 2,\n  };";
    ok(&fnsrc(&GOOD_FN.replace("return x;", val_ok)));
    let val_block = "return match x {\n    0 => { 1 },\n    _ => 2,\n  };";
    assert_eq!(kind(&fnsrc(&GOOD_FN.replace("return x;", val_block))), ErrorKind::ValueMatchArmBlock);
}

#[test]
fn shift_right_is_two_gt() {
    ok(&fnsrc(&GOOD_FN.replace("-> Int", "-> List<List<Int>>").replace("return x;", "return empty();")));
}

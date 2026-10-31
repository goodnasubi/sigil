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
    assert_eq!(
        kind(&fnsrc(GOOD_FN).replace('\n', "\r\n")),
        ErrorKind::CarriageReturn
    );
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
    assert_eq!(kind(&s), ErrorKind::NewlineInString);
}

#[test]
fn lex_leading_zero() {
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", "return 007;"))),
        ErrorKind::LeadingZero
    );
}

#[test]
fn lex_hex_and_exponent() {
    ok(&fnsrc(&GOOD_FN.replace("return x;", "return 0x10;")));
    ok(&fnsrc(&GOOD_FN.replace("return x;", "return 0b1010;")));
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", "return 1e3;"))),
        ErrorKind::BadNumber
    );
}

#[test]
fn lex_non_ascii_ident() {
    assert_eq!(
        kind(&fnsrc(
            &GOOD_FN.replace("return x;", "let é = 1;\n  return x;")
        )),
        ErrorKind::NonAsciiOutsideLiteral
    );
}

#[test]
fn lex_compound_assign_and_range() {
    assert_eq!(
        kind(&fnsrc(
            &GOOD_FN.replace("return x;", "x += 1;\n  return x;")
        )),
        ErrorKind::CompoundAssign
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", "return x..x;"))),
        ErrorKind::RangeOperator
    );
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
    assert_eq!(
        kind(&format!("{HEAD}{HEAD}{GOOD_FN}")),
        ErrorKind::ModuleDuplicate
    );
}

#[test]
fn module_clause_missing() {
    assert_eq!(
        kind(&format!("module m.a\n  targets server\n\n{GOOD_FN}")),
        ErrorKind::ModuleClauseMissing
    );
}

#[test]
fn module_clause_order() {
    assert_eq!(
        kind(&format!(
            "module m.a\n  trust none\n  targets server\n\n{GOOD_FN}"
        )),
        ErrorKind::ModuleClauseOrder
    );
}

#[test]
fn module_targets() {
    assert_eq!(
        kind(&format!(
            "module m.a\n  targets server, server\n  trust none\n\n{GOOD_FN}"
        )),
        ErrorKind::TargetsDuplicate
    );
    assert_eq!(
        kind(&format!(
            "module m.a\n  targets server, browser\n  trust none\n\n{GOOD_FN}"
        )),
        ErrorKind::TargetsUnsorted
    );
    assert_eq!(
        kind(&format!(
            "module m.a\n  targets mars\n  trust none\n\n{GOOD_FN}"
        )),
        ErrorKind::UnknownTarget
    );
    ok(&format!(
        "module m.a\n  targets browser, server\n  trust none\n\n{GOOD_FN}"
    ));
}

// ---- 契約節 (ADR0002/0008) ----
#[test]
fn clause_missing() {
    let s = fnsrc(&GOOD_FN.replace("  effects none\n", ""));
    assert_eq!(kind(&s), ErrorKind::MissingClause);
}

#[test]
fn clause_order() {
    let s = fnsrc(&GOOD_FN.replace(
        "  requires none\n  ensures none\n",
        "  ensures none\n  requires none\n",
    ));
    assert_eq!(kind(&s), ErrorKind::ClauseOrder);
}

#[test]
fn intent_empty() {
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("\"x\"", "\"\""))),
        ErrorKind::EmptyIntent
    );
}

#[test]
fn old_outside_ensures() {
    let s = fnsrc(&GOOD_FN.replace("requires none", "requires old(x) > 0"));
    assert_eq!(kind(&s), ErrorKind::OldOutsideEnsures);
    let s = fnsrc(&GOOD_FN.replace("return x;", "return old(x);"));
    assert_eq!(kind(&s), ErrorKind::OldOutsideEnsures);
    ok(&fnsrc(
        &GOOD_FN.replace("ensures none", "ensures result == old(x)"),
    ));
}

#[test]
fn result_outside_ensures() {
    let s = fnsrc(&GOOD_FN.replace("requires none", "requires result > 0"));
    assert_eq!(kind(&s), ErrorKind::ResultOutsideEnsures);
}

// ---- 名前 ----
#[test]
fn name_forms() {
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn myFn("))),
        ErrorKind::BadNameForm
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn _f("))),
        ErrorKind::UnderscoreName
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn match("))),
        ErrorKind::ReservedWordAsName
    );
    assert_eq!(
        kind(&fnsrc(&format!(
            "type my_type = Int where self > 0;\n{GOOD_FN}"
        ))),
        ErrorKind::BadNameForm
    );
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
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", stmt_comma))),
        ErrorKind::StmtMatchArmComma
    );
    let stmt_expr = "match x {\n    0 => 1,\n    _ => 2,\n  }";
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", stmt_expr))),
        ErrorKind::StmtMatchArmNotBlock
    );
    let val_ok = "return match x {\n    0 => 1,\n    _ => 2,\n  };";
    ok(&fnsrc(&GOOD_FN.replace("return x;", val_ok)));
    let val_block = "return match x {\n    0 => { 1 },\n    _ => 2,\n  };";
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", val_block))),
        ErrorKind::ValueMatchArmBlock
    );
}

#[test]
fn shift_right_is_two_gt() {
    ok(&fnsrc(
        &GOOD_FN
            .replace("-> Int", "-> List<List<Int>>")
            .replace("return x;", "return empty();"),
    ));
}

// ---- 追加: 深さ上限 ----
fn ret(e: &str) -> String {
    fnsrc(&GOOD_FN.replace("return x;", &format!("return {e};")))
}

#[test]
fn depth_limit() {
    let n = 5000;
    assert_eq!(
        kind(&ret(&format!("{}x{}", "(".repeat(n), ")".repeat(n)))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("{}x", "-".repeat(n)))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("{}x", "!".repeat(n)))),
        ErrorKind::TooDeep
    );
    let blocks = format!("{}return x;{}", "if true {\n".repeat(n), "}\n".repeat(n));
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", &blocks))),
        ErrorKind::TooDeep
    );
    let ty = format!("{}Int{}", "List<".repeat(n), ">".repeat(n));
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("-> Int", &format!("-> {ty}")))),
        ErrorKind::TooDeep
    );
    ok(&ret(&format!("{}x{}", "(".repeat(50), ")".repeat(50))));
}

// ---- 追加: 代入の左辺 ----
#[test]
fn assign_targets() {
    let b = |s: &str| fnsrc(&GOOD_FN.replace("return x;", &format!("{s}\n  return x;")));
    assert_eq!(kind(&b("Foo.bar = 1;")), ErrorKind::InvalidAssignTarget);
    assert_eq!(kind(&b("x.B = 1;")), ErrorKind::InvalidAssignTarget);
    assert_eq!(kind(&b("f(x) = 1;")), ErrorKind::InvalidAssignTarget);
    ok(&b("x.a.b = 1;"));
}

// ---- 追加: targets の末尾カンマ ----
#[test]
fn targets_trailing_comma() {
    let s = format!("module m.a\n  targets browser,\n  trust none\n\n{GOOD_FN}");
    assert_eq!(kind(&s), ErrorKind::UnknownTarget);
}

// ---- 追加: 案内メッセージ ----
#[test]
fn guidance_messages() {
    let s = fnsrc(&GOOD_FN.replace(
        "return x;",
        "if x == Foo { a: 1 } {\n    return x;\n  }\n  return x;",
    ));
    let e = parse(&s).unwrap_err();
    assert!(e.message.contains("括弧で囲む"), "{e}");
    let s = fnsrc(&GOOD_FN.replace(
        "return x;",
        "let y = if true { 1 } else { 2 };\n  return x;",
    ));
    let e = parse(&s).unwrap_err();
    assert!(e.message.contains("match"), "{e}");
    ok(&fnsrc(&GOOD_FN.replace(
        "return x;",
        "if x == Color.Red {\n    return 1;\n  }\n  return x;",
    )));
}

// ---- 追加: 各エラー種別 ----
#[test]
fn lex_kinds() {
    assert_eq!(kind(&format!("\u{feff}{}", fnsrc(GOOD_FN))), ErrorKind::Bom);
    assert_eq!(kind(&ret("x @ x")), ErrorKind::UnexpectedChar);
    assert_eq!(kind(&ret("'a'")), ErrorKind::CharLiteral);
    assert_eq!(kind(&ret("a::b")), ErrorKind::PathSeparator);
}

#[test]
fn reserved_names_defined() {
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn console("))),
        ErrorKind::ReservedNameDefined
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("x: Int", "state: Int"))),
        ErrorKind::ReservedNameDefined
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn check("))),
        ErrorKind::ReservedNameDefined
    );
}

#[test]
fn ref_of_ref() {
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("x: Int", "x: &&Int"))),
        ErrorKind::RefOfRef
    );
    assert_eq!(kind(&ret("&&x")), ErrorKind::RefOfRef);
}

#[test]
fn self_outside_refinement() {
    assert_eq!(kind(&ret("self")), ErrorKind::SelfOutsideRefinement);
    ok(&fnsrc(&format!(
        "type Pos = Int where self > 0;\n{GOOD_FN}"
    )));
}

#[test]
fn empty_forms() {
    assert_eq!(
        kind(&fnsrc(&format!("enum E {{}}\n{GOOD_FN}"))),
        ErrorKind::EmptyEnum
    );
    assert_eq!(
        kind(&fnsrc(
            &GOOD_FN.replace("return x;", "match x {\n  }\n  return x;")
        )),
        ErrorKind::EmptyMatch
    );
    assert_eq!(kind(&ret("Foo {}")), ErrorKind::EmptyStructLiteral);
    assert_eq!(
        kind(&fnsrc(&format!("type Pos = Int;\n{GOOD_FN}"))),
        ErrorKind::TypeAliasWithoutWhere
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("-> Int", "-> (Int)"))),
        ErrorKind::BadTuple
    );
}

// ---- 追加: 正の側 ----
#[test]
fn positive_forms() {
    let b = |s: &str| ok(&fnsrc(&GOOD_FN.replace("return x;", s)));
    b("while x > 0 {\n    x = x - 1;\n  }\n  return x;");
    b("if x > 0 {\n    return 1;\n  } else if x < 0 {\n    return 2;\n  } else {\n    return 3;\n  }");
    b("f(x)?;\n  return x;");
    b("let g = move || x;\n  let h = |a, b| a + b;\n  return x;");
    b("match c {\n    Color.Red => { return 1; }\n    Shape.Circle(r) => { return r; }\n    _ => { return 2; }\n  }");
    ok(&fnsrc(&GOOD_FN.replace(
        "ensures none",
        "ensures result == -1 || result == 1",
    )));
}

// ---- 追加: 位置 ----
#[test]
fn spans() {
    // HEAD は 4 行。fn は 5 行目。
    let e = parse(&fnsrc(&GOOD_FN.replace("intent \"x\"", "intent \"\""))).unwrap_err();
    assert_eq!(e.span.line, 6);
    let e = parse(&fnsrc(&GOOD_FN.replace("return x;", "return 007;"))).unwrap_err();
    assert_eq!((e.span.line, e.span.col), (11, 10));
    let e = parse(&fnsrc(&GOOD_FN.replace("return x;", "\t"))).unwrap_err();
    assert_eq!((e.span.line, e.span.col), (11, 3));
    let e = parse("module m.a\n  targets mars\n  trust none\n").unwrap_err();
    assert_eq!((e.span.line, e.span.col), (2, 11));
}

#[test]
fn tuples_positive() {
    ok(&fnsrc(
        &GOOD_FN
            .replace("-> Int", "-> (Int, Int)")
            .replace("return x;", "return (x, x);"),
    ));
    assert_eq!(kind(&ret("()")), ErrorKind::BadTuple);
}

#[test]
fn depth_limit_chains() {
    // else-if、後置の連鎖、二項演算の連鎖も深さに数える（AST の Drop と再帰を抑える）。
    let n = 5000;
    let mut s = String::from("if a { return x; }");
    for _ in 0..n {
        s.push_str(" else if a { return x; }");
    }
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("return x;", &s))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("x{}", ".a".repeat(n)))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("x{}", "(1)".repeat(n)))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("x{}", "?".repeat(n)))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("x{}", " + 1".repeat(n)))),
        ErrorKind::TooDeep
    );
    assert_eq!(
        kind(&ret(&format!("x{}", " || y".repeat(n)))),
        ErrorKind::TooDeep
    );
    ok(&ret(&format!("x{}", ".a".repeat(50))));
}

#[test]
fn ref_of_ref_nested() {
    assert_eq!(kind(&ret("& &x")), ErrorKind::RefOfRef);
    let t = fnsrc(&GOOD_FN.replace("x: Int", "x: & &Int"));
    assert_eq!(kind(&t), ErrorKind::RefOfRef);
}

// ---- M1: ADR 0011 の構文の拡張 ----

#[test]
fn impl_syntax() {
    let m = "  fn m(self: &mut A, n: Int)\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {\n    self.n = n;\n  }\n";
    ok(&fnsrc(&format!("impl A {{\n{m}}}\n")));
    ok(&fnsrc(&format!(
        "impl<T: Equatable + Sendable> A<T> {{\n{m}}}\n"
    )));
    // `self` は `impl` の中の最初の引数だけ
    assert!(parse(&fnsrc("fn f(n: Int, self: A)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\n")).is_err());
    assert!(parse(&fnsrc("fn f(self: A)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\n")).is_err());
}

#[test]
fn reserved_property_names() {
    for n in [
        "Unit",
        "Copyable",
        "Equatable",
        "Comparable",
        "Numeric",
        "Sendable",
        "Shareable",
    ] {
        assert_eq!(
            kind(&fnsrc(&format!("struct {n} {{\n  x: Int,\n}}\n"))),
            ErrorKind::ReservedNameDefined,
            "{n}"
        );
    }
}

#[test]
fn let_syntax() {
    let f = |b: &str| fnsrc(&GOOD_FN.replace("  return x;", b));
    ok(&f("  let a: Int = 1;\n  return x;"));
    ok(&f("  let (a, b) = (1, 2);\n  return x;"));
    ok(&f("  let (a, (_, b)): (Int, (Int, Int)) = t;\n  return x;"));
    assert_eq!(
        kind(&f("  let mut (a, b) = (1, 2);\n  return x;")),
        ErrorKind::NotInP0
    );
    // 空の組・要素 1 つの組の分解は書けない
    assert!(parse(&f("  let () = t;\n  return x;")).is_err());
    assert!(parse(&f("  let (a) = t;\n  return x;")).is_err());
}

// ---- unchecked の節（ADR 0016 の決定 2） ----

const UNCHECKED_FN: &str = "fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"前提\"\n{\n  return x;\n}\n";

#[test]
fn unchecked_clause_parses_after_effects() {
    ok(&fnsrc(UNCHECKED_FN));
}

#[test]
fn unchecked_clause_must_be_last() {
    let s = fnsrc(&GOOD_FN.replace(
        "  requires none\n",
        "  unchecked \"前提\"\n  requires none\n",
    ));
    assert_eq!(kind(&s), ErrorKind::ClauseOrder);
}

#[test]
fn unchecked_clause_empty_rejected() {
    let s = fnsrc(&UNCHECKED_FN.replace("\"前提\"", "\"\""));
    assert_eq!(kind(&s), ErrorKind::EmptyUnchecked);
}

#[test]
fn unchecked_clause_needs_a_string() {
    let s = fnsrc(&UNCHECKED_FN.replace("\"前提\"", "none"));
    assert_eq!(kind(&s), ErrorKind::Unexpected);
}

#[test]
fn raw_is_reserved_name() {
    assert_eq!(
        kind(&fnsrc(
            &GOOD_FN.replace("return x;", "let raw = 1;\n  return x;")
        )),
        ErrorKind::ReservedNameDefined
    );
    assert_eq!(
        kind(&fnsrc(&GOOD_FN.replace("fn f(", "fn raw("))),
        ErrorKind::ReservedNameDefined
    );
}

#[test]
fn unchecked_is_a_keyword() {
    let s = fnsrc(&GOOD_FN.replace("return x;", "let unchecked = 1;\n  return x;"));
    assert!(parse(&s).is_err());
}

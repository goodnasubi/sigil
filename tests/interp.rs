//! M3 の最初の PR のテスト: インタープリタの土台と、4 種類の異常停止（契約違反・Int オーバーフロー・
//! 0 除算・スタック枯渇。ADR 0013 の決定 1）。

use sigil::interp::{run, run_with, Arg, Clause, Options, RunError, Stop, StopKind};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(sig: &str, requires: &str, ensures: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires {requires}\n  ensures {ensures}\n  effects none\n{{\n{body}\n}}\n"
    )
}

fn program(items: &str) -> String {
    format!("{HEAD}{items}")
}

fn value(items: &str, f: &str, args: &[Arg]) -> String {
    match run(&program(items), f, args) {
        Ok(o) => o.value,
        Err(e) => panic!("成功するはずが失敗した: {e}"),
    }
}

fn stop(items: &str, f: &str, args: &[Arg]) -> Stop {
    match run(&program(items), f, args) {
        Ok(o) => panic!("異常停止するはずが成功した: {}", o.value),
        Err(e) => match e.error {
            RunError::Stop(s) => *s,
            other => panic!("異常停止でない失敗: {other}"),
        },
    }
}

fn example(path: &str) -> String {
    std::fs::read_to_string(format!("{}/examples/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

// ---- 契約違反 ----

#[test]
fn requires_violation_blames_the_caller() {
    let s = stop(
        &func("f(x: Int) -> Int", "x > 0", "none", "return x;"),
        "f",
        &[Arg::Int(0)],
    );
    assert_eq!(s.kind, StopKind::RequiresViolated);
    assert_eq!(s.func, "f");
    assert!(s.during.is_none());
}

#[test]
fn ensures_violation_blames_the_implementation() {
    let s = stop(
        &func("f(x: Int) -> Int", "none", "result == x + 1", "return x;"),
        "f",
        &[Arg::Int(1)],
    );
    assert_eq!(s.kind, StopKind::EnsuresViolated);
    assert_eq!(s.func, "f");
}

#[test]
fn ensures_sees_old_values() {
    let items = "struct Account {\n  balance: Int,\n}\n\n".to_string()
        + &func(
            "deposit(a: &mut Account, n: Int)",
            "n > 0",
            "a.balance == old(a.balance) + n",
            "a.balance = a.balance + n;",
        )
        + &func(
            "bad(a: &mut Account, n: Int)",
            "n > 0",
            "a.balance == old(a.balance) + n",
            "a.balance = a.balance + n + 1;",
        )
        + &func(
            "good() -> Int",
            "none",
            "result == 150",
            "let mut a = Account { balance: 100 };\ndeposit(&mut a, 50);\nreturn a.balance;",
        )
        + &func(
            "wrong() -> Int",
            "none",
            "none",
            "let mut a = Account { balance: 100 };\nbad(&mut a, 50);\nreturn a.balance;",
        );
    assert_eq!(value(&items, "good", &[]), "150");
    let s = stop(&items, "wrong", &[]);
    assert_eq!(s.kind, StopKind::EnsuresViolated);
    assert_eq!(s.func, "bad");
}

#[test]
fn requires_violated_inside_a_callee_is_reported_for_the_callee() {
    let items = func("g(x: Int) -> Int", "x > 0", "none", "return x;")
        + &func("f() -> Int", "none", "none", "return g(-1);");
    let s = stop(&items, "f", &[]);
    assert_eq!(s.kind, StopKind::RequiresViolated);
    assert_eq!(s.func, "g");
}

// ---- Int オーバーフロー ----

#[test]
fn overflow_stops_for_add_sub_mul_neg() {
    for body in [
        "return x + 1;",
        "return x * 2;",
        "return -x - 2;",
        "return 0 - x - 2;",
    ] {
        let s = stop(
            &func("f(x: Int) -> Int", "none", "none", body),
            "f",
            &[Arg::Int(i64::MAX)],
        );
        assert_eq!(s.kind, StopKind::Overflow, "{body}");
    }
    let s = stop(
        &func("f(x: Int) -> Int", "none", "none", "return -x;"),
        "f",
        &[Arg::Int(i64::MIN)],
    );
    assert_eq!(s.kind, StopKind::Overflow);
}

#[test]
fn min_div_minus_one_is_overflow() {
    let s = stop(
        &func("f(x: Int, y: Int) -> Int", "none", "none", "return x / y;"),
        "f",
        &[Arg::Int(i64::MIN), Arg::Int(-1)],
    );
    assert_eq!(s.kind, StopKind::Overflow);
}

#[test]
fn overflow_inside_ensures_is_an_abnormal_stop_during_the_clause() {
    let s = stop(
        &func("f(x: Int) -> Int", "none", "result == x + 1", "return x;"),
        "f",
        &[Arg::Int(i64::MAX)],
    );
    assert_eq!(s.kind, StopKind::Overflow);
    assert_eq!(s.during, Some(("f".to_string(), Clause::Ensures)));
}

#[test]
fn overflow_inside_requires_is_an_abnormal_stop_during_the_clause() {
    let s = stop(
        &func("f(x: Int) -> Int", "x + 1 > 0", "none", "return x;"),
        "f",
        &[Arg::Int(i64::MAX)],
    );
    assert_eq!(s.kind, StopKind::Overflow);
    assert_eq!(s.during, Some(("f".to_string(), Clause::Requires)));
}

#[test]
fn arithmetic_within_range_is_exact() {
    let f = func("f(x: Int) -> Int", "none", "none", "return x + 1;");
    assert_eq!(
        value(&f, "f", &[Arg::Int(i64::MAX - 1)]),
        i64::MAX.to_string()
    );
}

// ---- 0 除算 ----

#[test]
fn division_and_remainder_by_zero_stop() {
    for body in ["return x / y;", "return x % y;"] {
        let s = stop(
            &func("f(x: Int, y: Int) -> Int", "none", "none", body),
            "f",
            &[Arg::Int(1), Arg::Int(0)],
        );
        assert_eq!(s.kind, StopKind::DivisionByZero, "{body}");
    }
}

#[test]
fn integer_division_truncates_toward_zero() {
    let f = func("f(x: Int, y: Int) -> Int", "none", "none", "return x / y;");
    assert_eq!(value(&f, "f", &[Arg::Int(-7), Arg::Int(2)]), "-3");
    let r = func("f(x: Int, y: Int) -> Int", "none", "none", "return x % y;");
    assert_eq!(value(&r, "f", &[Arg::Int(-7), Arg::Int(2)]), "-1");
}

#[test]
fn float_division_by_zero_is_not_a_stop() {
    let f = func(
        "f(x: Float, y: Float) -> Float",
        "none",
        "none",
        "return x / y;",
    );
    assert_eq!(value(&f, "f", &[Arg::Float(1.0), Arg::Float(0.0)]), "inf");
    assert_eq!(value(&f, "f", &[Arg::Float(-1.0), Arg::Float(0.0)]), "-inf");
    assert_eq!(value(&f, "f", &[Arg::Float(0.0), Arg::Float(0.0)]), "NaN");
}

// ---- スタック枯渇 ----

const RECURSE: &str = "fn down(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return down(n + 1) + 1;\n}\n";

#[test]
fn unbounded_recursion_is_stack_exhaustion() {
    let opts = Options {
        stack_bytes: 1024 * 1024,
        ..Options::default()
    };
    let e = run_with(&program(RECURSE), "down", &[Arg::Int(0)], &opts).unwrap_err();
    match e.error {
        RunError::Stop(s) => {
            assert_eq!(s.kind, StopKind::StackExhausted);
            assert_eq!(s.func, "down");
        }
        other => panic!("{other}"),
    }
}

#[test]
fn deep_but_bounded_recursion_succeeds_with_the_default_stack() {
    let items = func(
        "sum(n: Int) -> Int",
        "n >= 0",
        "none",
        "if n == 0 {\n    return 0;\n  }\n  return sum(n - 1) + n;",
    );
    assert_eq!(value(&items, "sum", &[Arg::Int(1_000)]), "500500");
}

// ---- 異常停止の終了コード（全種類で同じ値） ----

#[test]
fn every_kind_of_abnormal_stop_exits_with_the_same_code() {
    let bin = env!("CARGO_BIN_EXE_sigil");
    let dir = std::env::temp_dir().join(format!("sigil-interp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let items = func("req(x: Int) -> Int", "x > 0", "none", "return x;")
        + &func("ens(x: Int) -> Int", "none", "result > x", "return x;")
        + &func("ovf(x: Int) -> Int", "none", "none", "return x + 1;")
        + &func("dz(x: Int) -> Int", "none", "none", "return 1 / x;")
        + RECURSE;
    let file = dir.join("p.sigil");
    std::fs::write(&file, program(&items)).unwrap();
    let cases: [(&str, &str); 5] = [
        ("req", "0"),
        ("ens", "0"),
        ("ovf", "9223372036854775807"),
        ("dz", "0"),
        ("down", "0"),
    ];
    for (f, a) in cases {
        let out = std::process::Command::new(bin)
            .args(["run", file.to_str().unwrap(), f, a])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(70), "{f}");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("異常停止"),
            "{f}"
        );
    }
    let ok = std::process::Command::new(bin)
        .args(["run", file.to_str().unwrap(), "req", "3"])
        .output()
        .unwrap();
    assert_eq!(ok.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&ok.stdout), "3\n");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 例 ----

#[test]
fn example_average() {
    let src = example("01-average/c-brace.sigil");
    let o = run(
        &src,
        "average",
        &[Arg::List(vec![
            Arg::Float(1.0),
            Arg::Float(2.0),
            Arg::Float(4.5),
        ])],
    )
    .unwrap();
    assert_eq!(o.value, "2.5");
    let e = run(&src, "average", &[Arg::List(vec![])]).unwrap_err();
    match e.error {
        RunError::Stop(s) => assert_eq!(s.kind, StopKind::RequiresViolated),
        other => panic!("{other}"),
    }
}

#[test]
fn example_control() {
    let src = example("05-control/control.sigil");
    let v = |f: &str, a: &[i64]| {
        let args: Vec<Arg> = a.iter().map(|n| Arg::Int(*n)).collect();
        run(&src, f, &args).unwrap().value
    };
    assert_eq!(v("sum_range", &[1, 10]), "55");
    assert_eq!(v("clamp", &[99, 1, 5]), "5");
    assert_eq!(v("clamp", &[-3, 1, 5]), "1");
    assert_eq!(v("sign", &[-4]), "-1");
    assert_eq!(v("sign", &[0]), "1");
    assert_eq!(v("add_to", &[3, 4]), "7");
}

#[test]
fn example_ownership_main_prints_through_console() {
    let src = example("03-ownership/ownership.sigil");
    let o = run(&src, "main", &[]).unwrap();
    assert_eq!(o.output, "150\n150\n");
}

#[test]
fn example_effects_pure_functions() {
    let src = example("04-effects/effects.sigil");
    assert_eq!(
        run(&src, "add_tax", &[Arg::Int(200), Arg::Int(10)])
            .unwrap()
            .value,
        "220"
    );
    let l = Arg::List(vec![Arg::Int(100), Arg::Int(200)]);
    assert_eq!(run(&src, "with_tax", &[l]).unwrap().value, "[110, 220]");
}

// ---- 範囲外の機能と入口の誤り ----

#[test]
fn unsupported_and_entry_errors_are_not_abnormal_stops() {
    let src = example("02-read-port/c-brace.sigil");
    let e = run(&src, "read_port", &[Arg::Str("p".into())]).unwrap_err();
    assert!(matches!(e.error, RunError::Unsupported(_)), "{}", e.error);
    let e = run(
        &program(&func("f() -> Int", "none", "none", "return 1;")),
        "nope",
        &[],
    )
    .unwrap_err();
    assert!(matches!(e.error, RunError::Entry(_)));
    let e = run(
        &program(&func("f() -> Int", "none", "none", "return true;")),
        "f",
        &[],
    )
    .unwrap_err();
    assert!(matches!(e.error, RunError::Check(_)));
}

// ---- 標準ライブラリ（ADR 0013 の決定 2） ----

fn expr(ty: &str, e: &str) -> String {
    value(
        &func(
            &format!("f() -> {ty}"),
            "none",
            "none",
            &format!("return {e};"),
        ),
        "f",
        &[],
    )
}

fn get(i: i64) -> String {
    let f = func(
        "f(xs: &List<Int>, i: Int) -> Option<&Int>",
        "none",
        "none",
        "return xs.get(i);",
    );
    let xs = Arg::List(vec![Arg::Int(1), Arg::Int(2)]);
    value(&f, "f", &[xs, Arg::Int(i)])
}

#[test]
fn stdlib_semantics() {
    assert_eq!(
        expr("Result<Int, ParseError>", "\"12\".parse_int()"),
        "Ok(12)"
    );
    assert_eq!(
        expr("Result<Int, ParseError>", "\"+12\".parse_int()"),
        "Err(ParseError)"
    );
    assert_eq!(
        expr("Result<Int, ParseError>", "\"-0\".parse_int()"),
        "Ok(0)"
    );
    assert_eq!(
        expr(
            "Result<Int, ParseError>",
            "\"9223372036854775808\".parse_int()"
        ),
        "Err(ParseError)"
    );
    assert_eq!(
        expr("Result<Int, ParseError>", "\"\".parse_int()"),
        "Err(ParseError)"
    );
    assert_eq!(expr("Option<Int>", "to_int(-2.9)"), "Some(-2)");
    assert_eq!(expr("Option<Int>", "to_int(1.0 / 0.0)"), "None");
    assert_eq!(expr("Option<Int>", "to_int(0.0 / 0.0)"), "None");
    assert_eq!(expr("Float", "-0.0"), "-0.0");
    assert_eq!(
        expr("List<String>", "\"a\\r\\nb\\n\".lines()"),
        "[\"a\", \"b\"]"
    );
    assert_eq!(get(2), "None");
    assert_eq!(get(-1), "None");
    assert_eq!(get(1), "Some(2)");
}

#[test]
fn refinement_check_is_none_out_of_range() {
    let items = "type Port = Int where 1 <= self && self <= 65535;\n\n".to_string()
        + &func(
            "f(n: Int) -> Option<Port>",
            "none",
            "none",
            "return Port.check(n);",
        );
    assert_eq!(value(&items, "f", &[Arg::Int(8080)]), "Some(8080)");
    assert_eq!(value(&items, "f", &[Arg::Int(0)]), "None");
    assert_eq!(value(&items, "f", &[Arg::Int(65536)]), "None");
}

// ---- 入口の引数の型合わせ ----

#[test]
fn int_argument_is_converted_for_a_float_parameter() {
    let f = func(
        "k(a: Float, b: Float) -> Float",
        "none",
        "none",
        "return a / b;",
    );
    assert_eq!(value(&f, "k", &[Arg::Int(1), Arg::Int(0)]), "inf");
    assert_eq!(value(&f, "k", &[Arg::Int(1), Arg::Float(2.0)]), "0.5");
    let l = func(
        "s(xs: &List<Float>) -> Float",
        "none",
        "none",
        "return xs.sum();",
    );
    assert_eq!(value(&l, "s", &[Arg::List(vec![])]), "0.0");
    assert_eq!(
        value(&l, "s", &[Arg::List(vec![Arg::Int(1), Arg::Float(2.5)])]),
        "3.5"
    );
}

#[test]
fn mismatched_argument_types_are_entry_errors() {
    let f = func("k(a: Int, b: Bool) -> Int", "none", "none", "return a;");
    for args in [
        [Arg::Float(1.0), Arg::Bool(true)],
        [Arg::Int(1), Arg::Int(1)],
    ] {
        let e = run(&program(&f), "k", &args).unwrap_err();
        assert!(matches!(e.error, RunError::Entry(_)), "{}", e.error);
    }
}

#[test]
fn out_of_range_integer_literal_is_an_entry_error_on_the_command_line() {
    let bin = env!("CARGO_BIN_EXE_sigil");
    let dir = std::env::temp_dir().join(format!("sigil-interp-lit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("p.sigil");
    let f = func("k(a: Float) -> Float", "none", "none", "return a;");
    std::fs::write(&file, program(&f)).unwrap();
    let run_cli = |a: &str| {
        std::process::Command::new(bin)
            .args(["run", file.to_str().unwrap(), "k", a])
            .output()
            .unwrap()
    };
    let out = run_cli("99999999999999999999");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("入口の指定が誤り"));
    assert_eq!(run_cli("3").status.code(), Some(0));
    assert_eq!(run_cli("1e30").status.code(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- 呼び出しを伴わない深い値 ----

const CHAIN: &str = "enum L {\n  Nil,\n  Cons(Int, L),\n}\n\n";

fn chain_fn(n: i64, tail: &str) -> String {
    CHAIN.to_string()
        + &func(
            "build() -> Int",
            "none",
            "none",
            &format!(
                "let mut l = L.Nil;\nlet mut i = 0;\nwhile i < {n} {{\n    l = L.Cons(i, l);\n    i = i + 1;\n}}\n{tail}"
            ),
        )
}

#[test]
fn very_deep_value_is_dropped_without_overflowing_the_host_stack() {
    let items = chain_fn(3_000_000, "return 1;");
    assert_eq!(value(&items, "build", &[]), "1");
}

#[test]
fn comparing_very_deep_values_stops_instead_of_overflowing_the_host_stack() {
    let items = chain_fn(
        100_000,
        "let mut m = L.Nil;\nlet mut j = 0;\nwhile j < 100000 {\n    m = L.Cons(j, m);\n    j = j + 1;\n}\nif l == m {\n    return 1;\n}\nreturn 0;",
    );
    let opts = Options {
        stack_bytes: 1024 * 1024,
        ..Options::default()
    };
    let e = run_with(&program(&items), "build", &[], &opts).unwrap_err();
    match e.error {
        RunError::Stop(s) => assert_eq!(s.kind, StopKind::StackExhausted),
        other => panic!("{other}"),
    }
}

#[test]
fn stack_exhaustion_while_showing_the_result_keeps_the_position() {
    let items = chain_fn(100_000, "return l;");
    let items = items.replace("build() -> Int", "build() -> L");
    let opts = Options {
        stack_bytes: 1024 * 1024,
        ..Options::default()
    };
    let e = run_with(&program(&items), "build", &[], &opts).unwrap_err();
    match e.error {
        RunError::Stop(s) => {
            assert_eq!(s.kind, StopKind::StackExhausted);
            assert_eq!(s.func, "build", "関数名が空にならない");
            assert!(s.line > 0, "行が 0 にならない");
        }
        other => panic!("{other}"),
    }
}

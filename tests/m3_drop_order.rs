//! ADR 0006 の条件 9: 破棄の順序を、破棄を記録する実行（`Options::trace_drops`）で固定する。
//! 構造体の値が破棄されるたびに、その時点の表示が `drops` に残る。
//!
//! 規則（ADR 0006 の M3 の記録）:
//! - 各スコープは、宣言の逆順に値を破棄する。内側のスコープ、内側の呼び出しの枠から外側へ進む。
//! - 値の中は、値そのもの、次にフィールド・要素を宣言の順に破棄する。
//! - `?` による早期の戻りと、すべての異常停止は、ふつうの戻りと同じ順序で破棄してから戻る・停止する。

use sigil::interp::{run_with, Arg, Options, RunError, StopKind};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

const DEFS: &str = "struct Probe {\n  id: Int,\n}\n\nstruct Two {\n  a: Probe,\n  b: Probe,\n}\n\nfn mk(n: Int) -> Probe\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Probe { id: n };\n}\n\nfn take(p: Probe)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\n\nfn none_of() -> Option<Int>\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return None;\n}\n\n";

fn func(sig: &str, requires: &str, ensures: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires {requires}\n  ensures {ensures}\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

fn f(sig: &str, body: &str) -> String {
    func(sig, "none", "none", body)
}

fn opts() -> Options {
    Options {
        trace_drops: true,
        ..Options::default()
    }
}

/// 実行して、成功なら戻り値の表示と破棄の記録を返す。
fn ok(items: &str, entry: &str, args: &[Arg]) -> (String, Vec<String>) {
    let src = format!("{HEAD}{DEFS}{items}");
    match run_with(&src, entry, args, &opts()) {
        Ok(o) => (o.value, o.drops),
        Err(e) => panic!("{}\n{src}", e.error),
    }
}

/// 実行して、異常停止なら停止の種類と破棄の記録を返す。
fn stopped(items: &str, entry: &str, args: &[Arg]) -> (StopKind, Vec<String>) {
    let src = format!("{HEAD}{DEFS}{items}");
    match run_with(&src, entry, args, &opts()) {
        Ok(o) => panic!("停止しなかった: {}\n{src}", o.value),
        Err(e) => match e.error {
            RunError::Stop(s) => (s.kind, e.drops.into_vec()),
            other => panic!("{other}\n{src}"),
        },
    }
}

fn ids(d: &[String]) -> Vec<i64> {
    d.iter()
        .map(|s| {
            s.trim_start_matches("Probe { id: ")
                .trim_end_matches(" }")
                .parse()
                .unwrap_or_else(|_| panic!("{s}"))
        })
        .collect()
}

#[test]
fn locals_drop_in_reverse_declaration_order() {
    let (v, d) = ok(
        &f(
            "t() -> Int",
            "let a = mk(1);\nlet b = mk(2);\nlet c = mk(3);\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(v, "0");
    assert_eq!(ids(&d), [3, 2, 1]);
}

#[test]
fn inner_scopes_drop_before_outer_ones() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let a = mk(1);\nif true {\n  let b = mk(2);\n  let c = mk(3);\n}\nlet d = mk(4);\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(ids(&d), [3, 2, 4, 1]);
}

#[test]
fn a_moved_value_drops_in_the_callee_not_at_the_end_of_the_caller() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let a = mk(1);\nlet b = mk(2);\ntake(a);\nlet c = mk(3);\nreturn 0;",
        ),
        "t",
        &[],
    );
    // a は take の中で破棄される。残りは c, b の順。
    assert_eq!(ids(&d), [1, 3, 2]);
}

#[test]
fn parameters_drop_in_reverse_order_when_the_function_returns() {
    let src = format!(
        "{}{}",
        f(
            "two(a: Probe, b: Probe) -> Int",
            "let c = mk(3);\nreturn 0;"
        ),
        f(
            "t() -> Int",
            "let x = mk(0);\nlet r = two(mk(1), mk(2));\nreturn r;"
        )
    );
    let (_, d) = ok(&src, "t", &[]);
    // two の枠: 局所 c、引数 b、引数 a。そのあと t の x。
    assert_eq!(ids(&d), [3, 2, 1, 0]);
}

#[test]
fn a_returned_value_moves_out_and_drops_with_its_new_owner() {
    let src = format!(
        "{}{}",
        f(
            "make() -> Probe",
            "let tmp = mk(1);\nlet r = mk(2);\nreturn r;"
        ),
        f(
            "t() -> Int",
            "let a = mk(0);\nlet got = make();\nlet z = mk(9);\nreturn 0;"
        )
    );
    let (_, d) = ok(&src, "t", &[]);
    // make の中: tmp(1) だけが破棄される。呼び出し元は z(9)、got(2)、a(0) の順。
    assert_eq!(ids(&d), [1, 9, 2, 0]);
}

#[test]
fn assignment_drops_the_old_value_at_the_assignment() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let mut a = mk(1);\nlet b = mk(2);\na = mk(3);\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(ids(&d), [1, 2, 3]);
}

#[test]
fn a_temporary_drops_at_the_end_of_its_statement() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let a = mk(1);\nmk(5);\nlet b = mk(2);\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(ids(&d), [5, 2, 1]);
}

#[test]
fn a_value_drops_itself_first_then_its_fields_in_order() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let t = Two { a: mk(1), b: mk(2) };\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(
        d,
        [
            "Two { a: Probe { id: 1 }, b: Probe { id: 2 } }",
            "Probe { id: 1 }",
            "Probe { id: 2 }"
        ]
    );
}

#[test]
fn list_elements_drop_in_order() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let mut xs = List.new();\nxs.push(mk(1));\nxs.push(mk(2));\nxs.push(mk(3));\nlet after = mk(4);\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(ids(&d), [4, 1, 2, 3]);
}

#[test]
fn clone_makes_two_values_that_drop_separately() {
    let (_, d) = ok(
        &f(
            "t() -> Int",
            "let a = mk(1);\nlet b = a.clone();\nreturn 0;",
        ),
        "t",
        &[],
    );
    assert_eq!(ids(&d), [1, 1]);
}

// ---- `?` による早期の戻り ----

#[test]
fn question_mark_drops_like_a_normal_return() {
    let src = format!(
        "{}{}",
        f(
            "inner() -> Option<Int>",
            "let a = mk(1);\nlet b = mk(2);\nlet n = none_of()?;\nlet c = mk(3);\nreturn Some(n);"
        ),
        f(
            "outer() -> Option<Int>",
            "let x = mk(0);\nlet n = inner()?;\nlet y = mk(9);\nreturn Some(n);"
        )
    );
    let (v, d) = ok(&src, "outer", &[]);
    assert_eq!(v, "None");
    // inner: b, a。outer の `?` も戻る: x。c と y は作られない。
    assert_eq!(ids(&d), [2, 1, 0]);
}

#[test]
fn question_mark_order_equals_the_order_of_a_normal_return() {
    let body = |tail: &str| {
        f(
            "t() -> Option<Int>",
            &format!("let a = mk(1);\nlet b = mk(2);\n{tail}"),
        )
    };
    let (_, early) = ok(&body("let n = none_of()?;\nreturn Some(n);"), "t", &[]);
    let (_, normal) = ok(&body("return None;"), "t", &[]);
    assert_eq!(early, normal);
    assert_eq!(ids(&early), [2, 1]);
}

// ---- 異常停止 ----

#[test]
fn requires_violation_drops_the_arguments_then_the_callers_values() {
    let src = format!(
        "{}{}",
        func(
            "g(a: Probe, b: Probe) -> Int",
            "a.id > 100",
            "none",
            "let never = mk(7);\nreturn 0;"
        ),
        f(
            "t() -> Int",
            "let x = mk(0);\nlet r = g(mk(1), mk(2));\nlet y = mk(9);\nreturn r;"
        )
    );
    let (k, d) = stopped(&src, "t", &[]);
    assert_eq!(k, StopKind::RequiresViolated);
    // g は入口で停止: 引数 b, a。そのあと t の x。
    assert_eq!(ids(&d), [2, 1, 0]);
}

#[test]
fn ensures_violation_drops_locals_then_the_result_then_arguments_then_the_caller() {
    let src = format!(
        "{}{}",
        func(
            "g(a: Probe) -> Probe",
            "none",
            "false",
            "let tmp = mk(5);\nreturn mk(6);"
        ),
        f("t() -> Int", "let x = mk(0);\nlet r = g(mk(1));\nreturn 0;")
    );
    let (k, d) = stopped(&src, "t", &[]);
    assert_eq!(k, StopKind::EnsuresViolated);
    // 戻る直前に局所 tmp(5) が破棄され、ensures が偽で停止する。戻り値(6)、引数(1)、呼び出し元の x(0) の順。
    assert_eq!(ids(&d), [5, 6, 1, 0]);
}

#[test]
fn overflow_and_division_by_zero_drop_in_the_same_order_as_a_contract_violation() {
    let src = f(
        "t(n: Int) -> Int",
        "let a = mk(1);\nlet b = mk(2);\nlet c = n + 1;\nreturn c;",
    );
    let (k, d) = stopped(&src, "t", &[Arg::Int(i64::MAX)]);
    assert_eq!(k, StopKind::Overflow);
    assert_eq!(ids(&d), [2, 1]);

    let src = f(
        "t(n: Int) -> Int",
        "let a = mk(1);\nlet b = mk(2);\nlet c = 10 / n;\nreturn c;",
    );
    let (k, d) = stopped(&src, "t", &[Arg::Int(0)]);
    assert_eq!(k, StopKind::DivisionByZero);
    assert_eq!(ids(&d), [2, 1]);
}

#[test]
fn a_stop_deep_in_calls_drops_from_the_innermost_frame_outward() {
    let src = format!(
        "{}{}{}",
        f(
            "c(p: Probe, n: Int) -> Int",
            "let l3 = mk(3);\nreturn 10 / n;"
        ),
        f(
            "b(p: Probe, n: Int) -> Int",
            "let l2 = mk(2);\nreturn c(mk(30), n);"
        ),
        f("a(n: Int) -> Int", "let l1 = mk(1);\nreturn b(mk(20), n);")
    );
    let (k, d) = stopped(&src, "a", &[Arg::Int(0)]);
    assert_eq!(k, StopKind::DivisionByZero);
    // c: l3, p(30)。b: l2, p(20)。a: l1。
    assert_eq!(ids(&d), [3, 30, 2, 20, 1]);
}

#[test]
fn a_stop_in_a_contract_expression_drops_like_other_stops() {
    let src = format!(
        "{}{}",
        func(
            "g(a: Probe, n: Int) -> Int",
            "none",
            "result + 1 > 0",
            "let l = mk(5);\nreturn n;"
        ),
        f(
            "t() -> Int",
            "let x = mk(0);\nreturn g(mk(1), 9223372036854775807);"
        )
    );
    let (k, d) = stopped(&src, "t", &[]);
    assert_eq!(k, StopKind::Overflow);
    assert_eq!(ids(&d), [5, 1, 0]);
}

#[test]
fn the_returned_value_is_dropped_after_it_is_shown() {
    let src = f("t() -> Probe", "let a = mk(1);\nlet r = mk(2);\nreturn r;");
    let (v, d) = ok(&src, "t", &[]);
    assert_eq!(v, "Probe { id: 2 }");
    assert_eq!(ids(&d), [1, 2]);
}

#[test]
fn without_tracing_nothing_is_recorded() {
    let src = format!(
        "{HEAD}{DEFS}{}",
        f("t() -> Int", "let a = mk(1);\nreturn 0;")
    );
    let o = run_with(&src, "t", &[], &Options::default()).unwrap();
    assert!(o.drops.is_empty());
}

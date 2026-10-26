//! ADR 0013 の「M3 の完了条件に加えるもの」のうち、標準ライブラリの各関数の意味（決定 2）と
//! 関数の値を引数に渡すこと（決定 3）の実行時のテスト。

mod common;
use common::run;
use sigil::exec::Arg;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func_fx(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

fn func(sig: &str, body: &str) -> String {
    func_fx(sig, "none", body)
}

/// 式を 1 つ評価して表示を返す。
fn ev(ty: &str, e: &str) -> String {
    ev_with("", ty, e)
}

fn ev_with(defs: &str, ty: &str, e: &str) -> String {
    let src = format!(
        "{HEAD}{defs}{}",
        func(&format!("f() -> {ty}"), &format!("return {e};"))
    );
    out(&src, "f", &[])
}

fn out(src: &str, f: &str, args: &[Arg]) -> String {
    match run(src, f, args) {
        Ok(o) => o.value,
        Err(e) => panic!("{e}\n{src}"),
    }
}

fn ints(xs: &[i64]) -> Arg {
    Arg::List(xs.iter().map(|n| Arg::Int(*n)).collect())
}

fn strs(xs: &[&str]) -> Arg {
    Arg::List(xs.iter().map(|s| Arg::Str(s.to_string())).collect())
}

const PAIR: &str = "struct P {\n  k: Int,\n  id: Int,\n}\n\n";

fn pairs() -> String {
    format!(
        "{PAIR}{}",
        func(
            "mk() -> List<P>",
            "let mut xs = List.new();\nxs.push(P { k: 2, id: 1 });\nxs.push(P { k: 5, id: 2 });\nxs.push(P { k: 2, id: 3 });\nxs.push(P { k: 5, id: 4 });\nreturn xs;"
        )
    )
}

// ---- List ----

#[test]
fn list_new_push_len_is_empty_get() {
    let defs = func(
        "build() -> List<Int>",
        "let mut xs = List.new();\nxs.push(10);\nxs.push(20);\nreturn xs;",
    );
    assert_eq!(ev_with(&defs, "Int", "build().len()"), "2");
    assert_eq!(ev_with(&defs, "Bool", "build().is_empty()"), "false");
    assert_eq!(
        ev_with(&defs, "Int", "build().get(1).map(|x| x + 0).unwrap_or(0)"),
        "20"
    );
}

#[test]
fn list_get_out_of_range_and_negative_are_none() {
    let f = func(
        "g(xs: &List<Int>, i: Int) -> Option<&Int>",
        "return xs.get(i);",
    );
    let src = format!("{HEAD}{f}");
    for (i, want) in [
        (0, "Some(7)"),
        (2, "Some(9)"),
        (3, "None"),
        (-1, "None"),
        (i64::MIN, "None"),
    ] {
        assert_eq!(
            out(&src, "g", &[ints(&[7, 8, 9]), Arg::Int(i)]),
            want,
            "{i}"
        );
    }
}

#[test]
fn list_get_mut_writes_and_returns_none_out_of_range() {
    let defs = format!(
        "{PAIR}{}{}",
        func(
            "set(xs: &mut List<Int>, i: Int, v: Int) -> Bool",
            "match xs.get_mut(i) {\n    Some(a) => {\n        a = v;\n        return true;\n    }\n    None => {\n        return false;\n    }\n}"
        ),
        func(
            "bump(ps: &mut List<P>, i: Int)",
            "match ps.get_mut(i) {\n    Some(p) => {\n        p.k = p.k + 100;\n    }\n    None => {}\n}"
        ),
    );
    let src = format!(
        "{HEAD}{defs}{}{}",
        func(
            "t() -> List<Int>",
            "let mut xs = List.new();\nxs.push(1);\nxs.push(2);\nlet a = set(&mut xs, 1, 50);\nlet b = set(&mut xs, 2, 60);\nlet c = set(&mut xs, -1, 70);\nif a && !b && !c {\n    return xs;\n}\nreturn List.new();"
        ),
        func(
            "u() -> Int",
            "let mut ps = List.new();\nps.push(P { k: 1, id: 0 });\nbump(&mut ps, 0);\nbump(&mut ps, 5);\nbump(&mut ps, -1);\nreturn ps.get(0).map(|p| p.k).unwrap_or(0);"
        ),
    );
    assert_eq!(out(&src, "t", &[]), "[1, 50]");
    assert_eq!(out(&src, "u", &[]), "101");
}

#[test]
fn list_map_calls_in_order_exactly_once() {
    let src = format!(
        "{HEAD}{}{}",
        func_fx(
            "trace(x: Int) -> Int",
            "console",
            "console.log(&x);\nreturn x * 2;"
        ),
        func_fx(
            "t(xs: &List<Int>) -> List<Int>",
            "console",
            "return xs.map(|x| trace(x));"
        ),
    );
    let o = run(&src, "t", &[ints(&[3, 1, 2])]).unwrap();
    assert_eq!(o.value, "[6, 2, 4]");
    assert_eq!(o.output, "3\n1\n2\n");
}

#[test]
fn list_find_returns_first_match_and_stops_calling_pred() {
    let src = format!(
        "{HEAD}{}{}",
        func_fx(
            "big(x: &Int) -> Bool",
            "console",
            "console.log(&x);\nreturn x > 1;"
        ),
        func_fx(
            "t(xs: &List<Int>) -> Option<&Int>",
            "console",
            "return xs.find(|x| big(&x));"
        ),
    );
    let o = run(&src, "t", &[ints(&[0, 1, 5, 7, 9])]).unwrap();
    assert_eq!(o.value, "Some(5)");
    assert_eq!(
        o.output, "0\n1\n5\n",
        "最初に真になった後は pred を呼ばない"
    );
    let o = run(&src, "t", &[ints(&[0, 1])]).unwrap();
    assert_eq!(o.value, "None");
    assert_eq!(o.output, "0\n1\n");
    assert_eq!(run(&src, "t", &[ints(&[])]).unwrap().value, "None");
}

#[test]
fn list_max_by_min_by_pick_the_first_of_equal_keys() {
    let defs = pairs();
    let tag = |e: &str| ev_with(&defs, "Int", &format!("{e}.map(|p| p.id).unwrap_or(0)"));
    assert_eq!(tag("mk().max_by(|p| p.k)"), "2");
    assert_eq!(tag("mk().min_by(|p| p.k)"), "1");
    let none_src = format!(
        "{HEAD}{PAIR}{}",
        func(
            "e(ps: &List<P>) -> Bool",
            "return ps.max_by(|p| p.k).is_none() && ps.min_by(|p| p.k).is_none();"
        )
    );
    let empty = Arg::List(vec![]);
    assert_eq!(out(&none_src, "e", &[empty]), "true");
}

#[test]
fn list_min_max_pick_the_first_of_equal_elements() {
    // 要素の値が等しいとき、先頭に近い要素が選ばれる。鍵が異なる値でも比較は値全体なので、
    // ここでは文字列と整数と小数で最小・最大を確かめる。
    let src = format!(
        "{HEAD}{}{}{}",
        func(
            "a(xs: &List<Int>) -> Int",
            "return xs.min() * 1000 + xs.max();"
        ),
        func(
            "b(xs: &List<String>) -> Bool",
            "return xs.min() == \"apple\" && xs.max() == \"zoo\";"
        ),
        func(
            "c(xs: &List<Float>) -> Float",
            "return xs.min() + xs.max();"
        ),
    );
    assert_eq!(out(&src, "a", &[ints(&[3, 1, 2, 1, 3])]), "1003");
    assert_eq!(
        out(&src, "b", &[strs(&["pear", "apple", "zoo", "apple"])]),
        "true"
    );
    assert_eq!(
        out(
            &src,
            "c",
            &[Arg::List(vec![Arg::Float(2.5), Arg::Float(-1.5)])]
        ),
        "1.0"
    );
}

#[test]
fn list_sum_is_checked_and_empty_sum_has_the_element_type() {
    let src = format!(
        "{HEAD}{}{}",
        func("i(xs: &List<Int>) -> Int", "return xs.sum();"),
        func("x(xs: &List<Float>) -> Float", "return xs.sum();"),
    );
    assert_eq!(out(&src, "i", &[ints(&[])]), "0");
    assert_eq!(out(&src, "i", &[ints(&[1, 2, 3])]), "6");
    assert_eq!(out(&src, "x", &[Arg::List(vec![])]), "0.0");
    let e = run(&src, "i", &[ints(&[i64::MAX, 1])]).unwrap_err();
    assert!(e.to_string().contains("桁あふれ"), "{e}");
}

#[test]
fn clone_copies_lists_and_structs() {
    let defs = format!(
        "{PAIR}{}",
        func(
            "t(ps: &List<P>) -> Int",
            "let mut c = ps.clone();\nmatch c.get_mut(0) {\n    Some(p) => {\n        p.k = 99;\n    }\n    None => {}\n}\nreturn ps.get(0).map(|p| p.k).unwrap_or(0) * 1000 + c.get(0).map(|p| p.k).unwrap_or(0);"
        )
    );
    let src = format!("{HEAD}{defs}");
    let build = format!(
        "{src}{}",
        func(
            "go() -> Int",
            "let mut ps = List.new();\nps.push(P { k: 1, id: 0 });\nreturn t(&ps);"
        )
    );
    assert_eq!(out(&build, "go", &[]), "1099");
}

#[test]
fn clone_copies_option_result_and_string() {
    let src = format!(
        "{HEAD}{}{}{}",
        func(
            "o(a: &Option<Int>) -> Int",
            "let mut c = a.clone();\nc = Some(9);\nreturn a.clone().unwrap_or(0) * 100 + c.unwrap_or(0);"
        ),
        func(
            "r(a: &Result<Int, Int>) -> Int",
            "let c = a.clone();\nmatch c {\n    Ok(v) => {\n        return v + 1;\n    }\n    Err(e) => {\n        return e;\n    }\n}"
        ),
        func(
            "s(a: &String) -> Int",
            "let mut c = a.clone();\nc = \"\";\nreturn a.len() * 10 + c.len();"
        ),
    );
    let go = format!(
        "{src}{}{}{}",
        func("go_o() -> Int", "let a = Some(4);\nreturn o(&a);"),
        func("go_r() -> Int", "let a = Ok(6);\nreturn r(&a);"),
        func("go_s() -> Int", "let a = \"ab\";\nreturn s(&a);"),
    );
    assert_eq!(out(&go, "go_o", &[]), "409");
    assert_eq!(out(&go, "go_r", &[]), "7");
    assert_eq!(out(&go, "go_s", &[]), "20");
}

// ---- Option / Result ----

#[test]
fn option_functions() {
    let mk = |body: &str, ty: &str| {
        format!(
            "{HEAD}{}{}{}",
            func("some() -> Option<Int>", "return Some(4);"),
            func("nothing() -> Option<Int>", "return None;"),
            func(&format!("t() -> {ty}"), body)
        )
    };
    assert_eq!(
        out(
            &mk("return some().map(|x| x * 3);", "Option<Int>"),
            "t",
            &[]
        ),
        "Some(12)"
    );
    assert_eq!(
        out(
            &mk("return some().ok_or(\"e\");", "Result<Int, String>"),
            "t",
            &[]
        ),
        "Ok(4)"
    );
    assert_eq!(
        out(
            &mk("return nothing().ok_or(\"e\");", "Result<Int, String>"),
            "t",
            &[]
        ),
        "Err(\"e\")"
    );
    assert_eq!(
        out(&mk("return some().unwrap_or(9);", "Int"), "t", &[]),
        "4"
    );
    assert_eq!(
        out(&mk("return nothing().unwrap_or(9);", "Int"), "t", &[]),
        "9"
    );
    assert_eq!(
        out(
            &mk("return some().is_some() && !some().is_none();", "Bool"),
            "t",
            &[]
        ),
        "true"
    );
}

#[test]
fn result_functions() {
    let mk = |body: &str, ty: &str| {
        format!(
            "{HEAD}{}{}{}",
            func("good() -> Result<Int, String>", "return Ok(4);"),
            func("bad() -> Result<Int, String>", "return Err(\"boom\");"),
            func(&format!("t() -> {ty}"), body)
        )
    };
    let t = |body: &str, ty: &str| out(&mk(body, ty), "t", &[]);
    assert_eq!(
        t("return good().is_ok() && bad().is_err();", "Bool"),
        "true"
    );
    assert_eq!(
        t("return good().is_err() || bad().is_ok();", "Bool"),
        "false"
    );
    assert_eq!(t("return bad().unwrap_or(1);", "Int"), "1");
    assert_eq!(t("return good().unwrap_or(1);", "Int"), "4");
    assert_eq!(t("return good().ok();", "Option<Int>"), "Some(4)");
    assert_eq!(t("return bad().ok();", "Option<Int>"), "None");
    assert_eq!(
        t("return good().map(|x| x + 1);", "Result<Int, String>"),
        "Ok(5)"
    );
    assert_eq!(
        t("return bad().map(|x| x + 1);", "Result<Int, String>"),
        "Err(\"boom\")"
    );
    assert_eq!(
        t("return bad().map_err(|e| e.len());", "Result<Int, Int>"),
        "Err(4)"
    );
    assert_eq!(
        t("return good().map_err(|e| e.len());", "Result<Int, Int>"),
        "Ok(4)"
    );
}

#[test]
fn question_mark_returns_none_and_err_unchanged() {
    let src = format!(
        "{HEAD}{}{}",
        func(
            "first(xs: &List<Int>) -> Option<Int>",
            "let a = xs.get(0)?;\nreturn Some(a + 1);"
        ),
        func(
            "num(s: &String) -> Result<Int, ParseError>",
            "let n = s.parse_int()?;\nreturn Ok(n * 2);"
        ),
    );
    assert_eq!(out(&src, "first", &[ints(&[])]), "None");
    assert_eq!(out(&src, "first", &[ints(&[4])]), "Some(5)");
    assert_eq!(out(&src, "num", &[Arg::Str("x".into())]), "Err(ParseError)");
    assert_eq!(out(&src, "num", &[Arg::Str("21".into())]), "Ok(42)");
}

// ---- String ----

#[test]
fn string_functions() {
    assert_eq!(ev("Int", "\"héllo\".len()"), "5");
    assert_eq!(ev("Int", "\"日本\".len()"), "2");
    assert_eq!(ev("Bool", "\"\".is_empty()"), "true");
    assert_eq!(ev("Bool", "\"a\".is_empty()"), "false");
    assert_eq!(ev("String", "\"  a b\\t\\n\".trim()"), "\"a b\"");
    assert_eq!(ev("String", "\"\\u{3000}x\\u{3000}\".trim()"), "\"x\"");
}

#[test]
fn string_lines() {
    let lines = |s: &str| ev("List<String>", &format!("{s}.lines()"));
    assert_eq!(lines("\"a\\nb\""), "[\"a\", \"b\"]");
    assert_eq!(lines("\"a\\r\\nb\\r\\n\""), "[\"a\", \"b\"]");
    assert_eq!(lines("\"a\\n\\nb\\n\""), "[\"a\", \"\", \"b\"]");
    assert_eq!(lines("\"\\n\""), "[\"\"]");
    assert_eq!(lines("\"\""), "[]");
    assert_eq!(lines("\"a\\r\\r\\n\""), "[\"a\\r\"]");
    assert_eq!(lines("\"a\\rb\""), "[\"a\\rb\"]");
}

#[test]
fn string_parse_int() {
    let p = |s: &str| ev("Result<Int, ParseError>", &format!("{s}.parse_int()"));
    assert_eq!(p("\"0\""), "Ok(0)");
    assert_eq!(p("\"-5\""), "Ok(-5)");
    assert_eq!(p("\"007\""), "Ok(7)");
    assert_eq!(p("\"9223372036854775807\""), "Ok(9223372036854775807)");
    assert_eq!(p("\"-9223372036854775808\""), "Ok(-9223372036854775808)");
    for bad in [
        "\"+1\"",
        "\" 1\"",
        "\"1 \"",
        "\"\"",
        "\"-\"",
        "\"--1\"",
        "\"1_0\"",
        "\"1.0\"",
        "\"٣\"",
        "\"9223372036854775808\"",
        "\"-9223372036854775809\"",
    ] {
        assert_eq!(p(bad), "Err(ParseError)", "{bad}");
    }
}

// ---- 数の関数 ----

#[test]
fn to_float_and_to_int() {
    assert_eq!(ev("Float", "to_float(3)"), "3.0");
    assert_eq!(ev("Float", "to_float(-2)"), "-2.0");
    let t = |e: &str| ev("Option<Int>", e);
    assert_eq!(t("to_int(2.9)"), "Some(2)");
    assert_eq!(t("to_int(-2.9)"), "Some(-2)");
    assert_eq!(t("to_int(-0.5)"), "Some(0)");
    assert_eq!(t("to_int(1.0 / 0.0)"), "None");
    assert_eq!(t("to_int(-1.0 / 0.0)"), "None");
    assert_eq!(t("to_int(0.0 / 0.0)"), "None");
    assert_eq!(t("to_int(9223372036854775808.0)"), "None");
    assert_eq!(
        t("to_int(-9223372036854775808.0)"),
        "Some(-9223372036854775808)"
    );
    assert_eq!(t("to_int(1000000000000000000000.0)"), "None");
}

#[test]
fn int_min_value() {
    assert_eq!(ev("Int", "Int.min_value()"), "-9223372036854775808");
    assert_eq!(ev("Int", "Int.min_value() + 1"), "-9223372036854775807");
}

#[test]
fn float_display() {
    let f = |e: &str| ev("Float", e);
    assert_eq!(f("1.0 / 0.0"), "inf");
    assert_eq!(f("-1.0 / 0.0"), "-inf");
    assert_eq!(f("0.0 / 0.0"), "NaN");
    assert_eq!(f("-0.0"), "-0.0");
    assert_eq!(f("0.0"), "0.0");
    assert_eq!(f("1.5"), "1.5");
    assert_eq!(f("100.0"), "100.0");
    assert_eq!(f("0.1 + 0.2"), "0.30000000000000004");
}

// ---- 関数の値を引数に渡す（決定 3） ----

#[test]
fn function_value_can_be_called_repeatedly_and_in_a_loop() {
    let src = format!(
        "{HEAD}{}{}{}{}",
        func_fx(
            "run(f: fn(Int) -> Int, x: Int) -> Int",
            "of(f)",
            "return f(x);"
        ),
        func_fx(
            "twice(f: fn(Int) -> Int) -> Int",
            "of(f)",
            "return run(f, 1) + run(f, 2);"
        ),
        func_fx(
            "looped(f: fn(Int) -> Int, n: Int) -> Int",
            "of(f)",
            "let mut total = 0;\nlet mut i = 0;\nwhile i < n {\n    total = total + run(f, i);\n    i = i + 1;\n}\nreturn total;"
        ),
        func_fx(
            "alias(f: fn(Int) -> Int) -> Int",
            "of(f)",
            "let h = f;\nlet k = f;\nreturn h(1) + k(2);"
        ),
    ) + &func("dbl(x: Int) -> Int", "return x * 2;")
        + &func("use_named() -> Int", "return twice(dbl) + looped(dbl, 4) + alias(dbl);")
        + &func(
            "use_closure(k: Int) -> Int",
            "let g = move |x| x + k;\nreturn run(g, 1) + run(g, 2);",
        )
        + &func("use_lambda() -> Int", "return twice(|x| x + 10);");
    // twice(dbl)=2+4=6, looped(dbl,4)=0+2+4+6=12, alias(dbl)=2+4=6
    assert_eq!(out(&src, "use_named", &[]), "24");
    assert_eq!(out(&src, "use_closure", &[Arg::Int(100)]), "203");
    assert_eq!(out(&src, "use_lambda", &[]), "23");
}

#[test]
fn function_value_effect_is_inherited_by_the_caller() {
    let src = format!(
        "{HEAD}{}{}{}",
        func_fx(
            "apply(f: fn(Int) -> Int, x: Int) -> Int",
            "of(f)",
            "return f(x);"
        ),
        func_fx(
            "noisy(x: Int) -> Int",
            "console",
            "console.log(&x);\nreturn x + 1;"
        ),
        func_fx("go() -> Int", "console", "return apply(noisy, 4);"),
    );
    let o = run(&src, "go", &[]).unwrap();
    assert_eq!(o.value, "5");
    assert_eq!(o.output, "4\n");
}

#[test]
fn function_value_effect_must_be_declared_by_the_caller() {
    let src = format!(
        "{HEAD}{}{}{}",
        func_fx(
            "apply(f: fn(Int) -> Int, x: Int) -> Int",
            "of(f)",
            "return f(x);"
        ),
        func_fx(
            "noisy(x: Int) -> Int",
            "console",
            "console.log(&x);\nreturn x + 1;"
        ),
        func_fx("go() -> Int", "none", "return apply(noisy, 4);"),
    );
    let e = match run(&src, "go", &[]) {
        Ok(o) => panic!("効果の宣言漏れが通った: {}", o.value),
        Err(e) => e.to_string(),
    };
    assert!(e.contains("console"), "{e}");
}

//! ADR 0015 の決定 1: 例 01〜05 と小さな単位の事例を、SIR の検証器・SIR の評価器・ネイティブの実行ファイルで試験する（`common::run*`）。
mod common;

use common::{run, run_sources, run_sources_with, run_with};
use sigil::exec::{Arg, Options, RunError, StopKind};

fn example(path: &str) -> String {
    std::fs::read_to_string(format!("{}/examples/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn floats(xs: &[f64]) -> Arg {
    Arg::List(xs.iter().map(|x| Arg::Float(*x)).collect())
}

fn files(pairs: &[(&str, &str)]) -> Options {
    let mut o = Options::captured();
    o.fs_files = Some(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    );
    o
}

#[test]
fn sir_examples_run() {
    // 例 01
    let avg = example("01-average/c-brace.sigil");
    run(&avg, "average", &[floats(&[1.0, 2.0, 3.0, 4.0])]).unwrap();
    assert!(run(&avg, "average", &[floats(&[])]).is_err());

    // 例 02
    let port = example("02-read-port/c-brace.sigil");
    for content in ["8080", "0", "70000", "abc"] {
        let o = files(&[("p.txt", content)]);
        // Err は値として返るので、実行自体は成功する
        run_with(&port, "read_port", &[Arg::Str("p.txt".into())], &o).unwrap();
    }
    let o = files(&[]);
    run_with(&port, "read_port", &[Arg::Str("none.txt".into())], &o).unwrap();
    run_with(&port, "read_port", &[Arg::Str("".into())], &o).unwrap_err();

    // 例 03
    let own = example("03-ownership/ownership.sigil");
    let r = run(&own, "main", &[]).unwrap();
    assert_eq!(r.output, "150\n150\n");

    // 例 04
    let eff = example("04-effects/effects.sigil");
    let entry = example("04-effects/entry.sigil");
    run_sources(&[&eff, &entry], "bump_twice", &[]).unwrap();
    run_sources(&[&eff, &entry], "tax", &[]).unwrap();
    run(&eff, "add_tax", &[Arg::Int(100), Arg::Int(10)]).unwrap();
    let o = files(&[("prices.txt", "100\n250\n")]);
    run_sources_with(&[&eff], "load_prices", &[Arg::Str("prices.txt".into())], &o).unwrap();
    let o = files(&[]);
    run_sources_with(&[&eff], "load_prices", &[Arg::Str("x".into())], &o).unwrap();

    // 例 05
    let ctl = example("05-control/control.sigil");
    run(&ctl, "sum_range", &[Arg::Int(1), Arg::Int(10)]).unwrap();
    for x in [-5, 3, 50] {
        run(&ctl, "clamp", &[Arg::Int(x), Arg::Int(0), Arg::Int(10)]).unwrap();
        run(&ctl, "sign", &[Arg::Int(x)]).unwrap();
    }
    run(&ctl, "add_to", &[Arg::Int(3), Arg::Int(4)]).unwrap();
}

const HEAD: &str = "module t\n  targets server\n  trust none\n\n";

fn f(name: &str, sig: &str, body: &str) -> String {
    format!("fn {name}{sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n")
}

#[test]
fn sir_stack_exhaustion_kind_matches() {
    let src = format!(
        "{HEAD}{}",
        f("down", "(n: Int) -> Int", "  return down(n + 1);")
    );
    let o = Options {
        stack_bytes: 1 << 20,
        ..Options::captured()
    };
    let e = run_with(&src, "down", &[Arg::Int(0)], &o).unwrap_err();
    match e.error {
        RunError::Stop(p) => assert_eq!(p.kind, StopKind::StackExhausted),
        other => panic!("{other}"),
    }
}

#[test]
fn sir_generic_function_is_monomorphized() {
    let src = format!(
        "{HEAD}fn first<T>(xs: &List<T>) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return xs.len();\n}}\n\n{}",
        f(
            "go",
            "() -> Int",
            "  let mut a = List.new();\n  a.push(1);\n  let mut b = List.new();\n  b.push(\"s\");\n  b.push(\"t\");\n  return first(&a) + first(&b);"
        )
    );
    let r = run(&src, "go", &[]).unwrap();
    assert_eq!(r.value.to_string(), "3");
}

#[test]
fn sir_closure_captures_and_division_by_zero_stop() {
    let src = format!(
        "{HEAD}{}\n{}",
        f(
            "go",
            "(k: Int) -> List<Int>",
            "  let mut xs = List.new();\n  xs.push(1);\n  xs.push(2);\n  return xs.map(|x| x * k);"
        ),
        f("div", "(a: Int, b: Int) -> Int", "  return a / b;")
    );
    run(&src, "go", &[Arg::Int(3)]).unwrap();
    run(&src, "div", &[Arg::Int(7), Arg::Int(0)]).unwrap_err();
    run(&src, "div", &[Arg::Int(i64::MIN), Arg::Int(-1)]).unwrap_err();
    run(&src, "div", &[Arg::Int(i64::MAX), Arg::Int(1)]).unwrap();
}

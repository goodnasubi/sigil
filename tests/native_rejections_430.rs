//! #430: 検査・入口の突き合わせが、異なる拒否やビルド成功を見逃さないことを固定する。
mod common;

use sigil::exec::{Arg, Options};

fn source(value: &str) -> String {
    format!(
        "module m.t\n  targets server\n  trust none\n\nfn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return {value};\n}}\n"
    )
}

#[test]
fn accepting_a_source_that_the_evaluator_rejected_fails_comparison() {
    let bad = source("true");
    let good = source("x");
    let opts = Options::captured();
    let want = sigil::ir_eval::run_with(&bad, "f", &[Arg::Int(1)], &opts);
    assert!(want.is_err());
    let compared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        common::native::check(&[&good], "f", &[Arg::Int(1)], &opts, &want);
    }));
    assert!(compared.is_err(), "検査の拒否とビルド成功の差を見逃した");
}

#[test]
fn a_different_entry_diagnostic_fails_comparison() {
    let src = source("x");
    let opts = Options::captured();
    let want = sigil::ir_eval::run_with(&src, "missing", &[], &opts);
    let compared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        common::native::check(&[&src], "other", &[], &opts, &want);
    }));
    assert!(compared.is_err(), "異なる入口の拒否を同一扱いした");
}

#[test]
fn a_different_runtime_argument_diagnostic_fails_comparison() {
    let src = source("x");
    let opts = Options::captured();
    let want = sigil::ir_eval::run_with(&src, "f", &[Arg::Float(1.5)], &opts);
    let compared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        common::native::check(&[&src], "f", &[Arg::Bool(true)], &opts, &want);
    }));
    assert!(compared.is_err(), "引数拒否の診断の差を見逃した");
}

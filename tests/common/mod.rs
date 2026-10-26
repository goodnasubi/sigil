//! インタープリタと SIR の評価器の両方で実行して結果を突き合わせる試験の補助（ADR 0015 の決定 1）。
//!
//! 関数の形は `sigil::interp::{run, run_with, run_sources, run_sources_with}` と同じで、インタープリタの結果を返す。
//! 値・出力・破棄の記録・停止の報告が食い違えば失敗させる（`StackExhausted` は種類だけを比べる）。
#![allow(dead_code)]

use sigil::interp::{self, Arg, Failure, Options, Outcome, RunError, StopKind};
use sigil::ir_eval;

type Res = Result<Outcome, Failure>;

fn compare(label: &str, a: &Res, b: &Res) {
    match (a, b) {
        (Ok(x), Ok(y)) => {
            assert_eq!(x.value, y.value, "{label}: 値");
            assert_eq!(x.output, y.output, "{label}: 出力");
            assert_eq!(x.drops, y.drops, "{label}: 破棄の記録");
        }
        (Err(x), Err(y)) => match (&x.error, &y.error) {
            (RunError::Stop(p), RunError::Stop(q)) => {
                if p.kind == StopKind::StackExhausted || q.kind == StopKind::StackExhausted {
                    assert_eq!(p.kind, q.kind, "{label}: 停止の種類");
                } else {
                    assert_eq!(p.to_string(), q.to_string(), "{label}: 停止の報告");
                    assert_eq!(x.output, y.output, "{label}: 停止までの出力");
                    assert_eq!(x.drops, y.drops, "{label}: 停止までの破棄の記録");
                }
            }
            (p, q) => assert_eq!(p.to_string(), q.to_string(), "{label}: 失敗"),
        },
        (x, y) => panic!(
            "{label}: 片方だけ成功した\n  interp: {}\n  sir: {}",
            show(x),
            show(y)
        ),
    }
}

fn show(r: &Res) -> String {
    match r {
        Ok(o) => format!("Ok({})", o.value),
        Err(e) => format!("Err({e})"),
    }
}

pub fn run(src: &str, func: &str, args: &[Arg]) -> Res {
    let a = interp::run(src, func, args);
    let b = ir_eval::run(src, func, args);
    compare(func, &a, &b);
    a
}

pub fn run_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Res {
    let a = interp::run_with(src, func, args, opts);
    let b = ir_eval::run_with(src, func, args, opts);
    compare(func, &a, &b);
    a
}

pub fn run_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Res {
    let a = interp::run_sources(srcs, func, args);
    let b = ir_eval::run_sources(srcs, func, args);
    compare(func, &a, &b);
    a
}

pub fn run_sources_with(srcs: &[&str], func: &str, args: &[Arg], opts: &Options) -> Res {
    let a = interp::run_sources_with(srcs, func, args, opts);
    let b = ir_eval::run_sources_with(srcs, func, args, opts);
    compare(func, &a, &b);
    a
}

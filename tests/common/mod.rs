//! インタープリタと SIR の評価器の両方で実行して結果を突き合わせる試験の補助（ADR 0015 の決定 1）。
//!
//! 関数の形は `sigil::interp::{run, run_with, run_sources, run_sources_with}` と同じで、SIR の評価器の結果を返す（ADR 0017）。
//! 値・出力・停止の報告が食い違えば失敗させる（`StackExhausted` は種類だけを比べる）。
//! 破棄の記録はインタープリタとは比べない（インタープリタは破棄の順序の基準ではない）。
//! `sir_native*` はインタープリタを通さず、SIR の評価器とネイティブの実行ファイルだけを突き合わせる。
#![allow(dead_code)]

pub mod native;

use sigil::interp::{self, Arg, Failure, Options, Outcome, RunError, StopKind};
use sigil::ir_eval;

type Res = Result<Outcome, Failure>;

/// SIR の検証器（ADR 0018 の決定 4）。テストの実行では常に通す。
/// SIR にできないソース（未対応の構文・入口の誤り）は、検証の対象外として飛ばす。
fn verify(srcs: &[&str], func: &str) {
    if sigil::ir_eval::lower_sources(srcs, func).is_err() {
        return;
    }
    if let Err(es) = sigil::verify::verify_sources(srcs, func) {
        panic!(
            "SIR の検証に失敗: {func}\n{}\n{}",
            es.join("\n"),
            srcs.join("\n---\n")
        );
    }
}

fn compare(label: &str, a: &Res, b: &Res) {
    match (a, b) {
        (Ok(x), Ok(y)) => {
            assert_eq!(x.value, y.value, "{label}: 値");
            assert_eq!(x.output, y.output, "{label}: 出力");
        }
        (Err(x), Err(y)) => match (&x.error, &y.error) {
            (RunError::Stop(p), RunError::Stop(q)) => {
                if p.kind == StopKind::StackExhausted || q.kind == StopKind::StackExhausted {
                    assert_eq!(p.kind, q.kind, "{label}: 停止の種類");
                } else {
                    assert_eq!(p.to_string(), q.to_string(), "{label}: 停止の報告");
                    assert_eq!(x.output, y.output, "{label}: 停止までの出力");
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
    verify(&[src], func);
    let a = interp::run(src, func, args);
    let b = ir_eval::run(src, func, args);
    compare(func, &a, &b);
    native::check(&[src], func, args, &Options::captured(), &b);
    b
}

pub fn run_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Res {
    verify(&[src], func);
    let a = interp::run_with(src, func, args, opts);
    let b = ir_eval::run_with(src, func, args, opts);
    compare(func, &a, &b);
    native::check(&[src], func, args, opts, &b);
    b
}

pub fn run_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Res {
    verify(srcs, func);
    let a = interp::run_sources(srcs, func, args);
    let b = ir_eval::run_sources(srcs, func, args);
    compare(func, &a, &b);
    native::check(srcs, func, args, &Options::captured(), &b);
    b
}

pub fn run_sources_with(srcs: &[&str], func: &str, args: &[Arg], opts: &Options) -> Res {
    verify(srcs, func);
    let a = interp::run_sources_with(srcs, func, args, opts);
    let b = ir_eval::run_sources_with(srcs, func, args, opts);
    compare(func, &a, &b);
    native::check(srcs, func, args, opts, &b);
    b
}

/// インタプリタと食い違う既知の事例（破棄の順序など）を、SIR の評価器とネイティブだけで突き合わせたいときに使う。
pub fn sir_native(src: &str, func: &str, args: &[Arg]) -> Res {
    verify(&[src], func);
    let b = ir_eval::run(src, func, args);
    native::check(&[src], func, args, &Options::captured(), &b);
    b
}

pub fn sir_native_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Res {
    verify(&[src], func);
    let b = ir_eval::run_with(src, func, args, opts);
    native::check(&[src], func, args, opts, &b);
    b
}

pub fn sir_native_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Res {
    verify(srcs, func);
    let b = ir_eval::run_sources(srcs, func, args);
    native::check(srcs, func, args, &Options::captured(), &b);
    b
}

pub fn sir_native_sources_with(srcs: &[&str], func: &str, args: &[Arg], opts: &Options) -> Res {
    verify(srcs, func);
    let b = ir_eval::run_sources_with(srcs, func, args, opts);
    native::check(srcs, func, args, opts, &b);
    b
}

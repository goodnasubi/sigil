//! SIR の検証器・SIR の評価器・ネイティブの実行ファイルで試験する補助（ADR 0018 の決定 4）。
//!
//! 関数の形は `sigil::ir_eval::{run, run_with, run_sources, run_sources_with}` と同じで、SIR の評価器の結果を返す。
//! 実行の前に SIR の検証器を通し、実行の後にネイティブの実行ファイルの結果と突き合わせる（食い違えば失敗させる）。
#![allow(dead_code)]

pub mod native;

use sigil::exec::{Arg, Failure, Options, Outcome, RunError};
use sigil::ir_eval;

type Res = Result<Outcome, Failure>;

/// SIR にできなかった（`Unsupported`）ソースの数の、既知の上限（テストの実行ファイルごと）。超えたら落とす。
/// 増えるのは、検証されないソースが増えるということなので、増やすときは理由を PR に書く。
const KNOWN_UNSUPPORTED_MAX: usize = 0;

static UNSUPPORTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// SIR の検証器（ADR 0018 の決定 4）。テストの実行では常に通す。
/// 構文・型検査の誤りと入口の誤り（`Check`・`Entry`）は検証の対象がないので飛ばす。
/// 未対応（`Unsupported`）は検証されないので、数えて既知の上限と比べる。
#[allow(clippy::absurd_extreme_comparisons)] // 上限が 0 の間は、1 件でも落ちる
fn verify(srcs: &[&str], func: &str) {
    match sigil::ir_eval::lower_sources(srcs, func) {
        Ok(_) => {}
        Err(RunError::Check(_) | RunError::Entry(_)) => return,
        Err(RunError::Stop(s)) => panic!("lowering が停止を返した: {func}: {s}"),
        Err(RunError::Unsupported(m)) => {
            let n = UNSUPPORTED.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            eprintln!("SIR の検証を飛ばす（未対応 {n} 件目）: {func}: {m}");
            assert!(
                n <= KNOWN_UNSUPPORTED_MAX,
                "SIR にできないソースが既知の上限 {KNOWN_UNSUPPORTED_MAX} を超えた: {func}: {m}"
            );
            return;
        }
    }
    if let Err(es) = sigil::verify::verify_sources(srcs, func) {
        panic!(
            "SIR の検証に失敗: {func}\n{}\n{}",
            es.join("\n"),
            srcs.join("\n---\n")
        );
    }
}

pub fn run(src: &str, func: &str, args: &[Arg]) -> Res {
    run_sources_with(&[src], func, args, &Options::captured())
}

pub fn run_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Res {
    run_sources_with(&[src], func, args, opts)
}

pub fn run_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Res {
    run_sources_with(srcs, func, args, &Options::captured())
}

pub fn run_sources_with(srcs: &[&str], func: &str, args: &[Arg], opts: &Options) -> Res {
    verify(srcs, func);
    let r = ir_eval::run_sources_with(srcs, func, args, opts);
    native::check(srcs, func, args, opts, &r);
    r
}

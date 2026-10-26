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
        Err(RunError::Check(_) | RunError::Module { .. } | RunError::Entry(_)) => return,
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

/// 例 04 を複数モジュールで走らせる組（入口, 取り込まれる側）。入口は `tests/data/effects_main.sigil`、
/// 取り込まれる側は `effects.sigil` の `count_up`・`with_tax` を `pub` にしたもの（例そのものは変えない）。
pub fn effects_pair() -> [String; 2] {
    let eff = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/04-effects/effects.sigil"
    ))
    .unwrap()
    .replace("\nfn count_up", "\npub fn count_up")
    .replace("\nfn with_tax", "\npub fn with_tax");
    let entry = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/effects_main.sigil"
    ))
    .unwrap();
    [entry, eff]
}

/// CLI の試験用。ソース（ファイルの中身）を、`module` の宣言の名前に対応する位置に置いた根を作る
/// （`sigil` の `--root` の既定は現在のディレクトリで、パスと名前が対応しなければ落ちるため）。
/// 戻り値は (根, ファイル)。使い終えたら根を消す。
pub fn stage_src(tag: &str, text: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let name = text
        .lines()
        .find_map(|l| l.strip_prefix("module "))
        .expect("module の宣言がある")
        .trim()
        .to_string();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "sigil-stage-{}-{tag}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let file = root.join(format!("{}.sigil", name.replace('.', "/")));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, text).unwrap();
    (root, file)
}

/// `stage_src` を、リポジトリの中のファイルから行う。
pub fn stage_file(tag: &str, path: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    stage_src(tag, &std::fs::read_to_string(path).unwrap())
}

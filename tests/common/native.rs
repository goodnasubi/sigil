//! ネイティブ出力との突き合わせ（ADR 0015 の決定 1・4、PR4）。
//!
//! `SIGIL_NATIVE=1` のとき、`common::run*` が走らせた同じプログラムを `sigil::native::build` で
//! 実行ファイルにして走らせ、インタープリタの結果（値・出力・終了コード・停止の報告）と突き合わせる。
//! `BuildUnsupported` で止まる構文は、まだ出力できないものとして飛ばす（数を `SIGIL_NATIVE_VERBOSE=1` で表示）。
//! `cc` や `libc.a` がなければ、飛ばさずに失敗させる（暗黙に通らないようにするため）。
#![allow(dead_code)]

use sigil::codegen::Opts;
use sigil::interp::{Arg, Failure, Options, Outcome, RunError, StopKind};
use sigil::native::{self, BuildError};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

pub fn enabled() -> bool {
    std::env::var("SIGIL_NATIVE").is_ok_and(|v| v == "1")
}

/// 作業ディレクトリ（`CARGO_TARGET_TMPDIR` の下。使い終えたら消す）。
pub fn scratch(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sigil-native-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// コマンドラインの引数の書き方に戻す（`parse_arg` と逆）。
pub fn arg_text(a: &Arg) -> String {
    match a {
        Arg::Int(n) => n.to_string(),
        Arg::Float(x) => format!("{x:?}"),
        Arg::Bool(b) => b.to_string(),
        Arg::Str(s) => s.clone(),
        Arg::List(xs) => format!(
            "[{}]",
            xs.iter().map(arg_text).collect::<Vec<_>>().join(",")
        ),
    }
}

fn ambiguous(a: &Arg) -> bool {
    match a {
        Arg::Str(s) => !matches!(sigil::shared_args::parse_arg(s), Ok(Arg::Str(t)) if &t == s),
        Arg::List(xs) => xs.iter().any(ambiguous),
        _ => false,
    }
}

/// 実行ファイルを作る。`Ok(None)` は「まだ出力できない構文」で飛ばすこと。
pub fn try_build(srcs: &[&str], func: &str, out: &std::path::Path) -> Result<Option<()>, String> {
    match native::build(srcs, func, &Opts::default(), out) {
        Ok(()) => Ok(Some(())),
        Err(BuildError::Unsupported(_)) | Err(BuildError::Run(_)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub struct Ran {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
}

pub fn exec(exe: &std::path::Path, args: &[Arg]) -> Ran {
    let o = Command::new(exe)
        .args(args.iter().map(arg_text))
        .output()
        .unwrap();
    Ran {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code(),
    }
}

static SKIPPED: AtomicUsize = AtomicUsize::new(0);
static CHECKED: AtomicUsize = AtomicUsize::new(0);

fn note(what: &str) {
    if std::env::var("SIGIL_NATIVE_VERBOSE").is_ok_and(|v| v == "1") {
        eprintln!(
            "native: {what}（突き合わせ {} / 飛ばし {}）",
            CHECKED.load(Ordering::SeqCst),
            SKIPPED.load(Ordering::SeqCst)
        );
    }
}

/// インタープリタの結果 `want` と、ネイティブの実行結果を突き合わせる。
pub fn check(
    srcs: &[&str],
    func: &str,
    args: &[Arg],
    opts: &Options,
    want: &Result<Outcome, Failure>,
) {
    if !enabled() {
        return;
    }
    let skip = |why: &str| {
        SKIPPED.fetch_add(1, Ordering::SeqCst);
        note(&format!("飛ばす: {why} {func}"));
    };
    if opts.trace_drops || opts.fs_files.is_some() || !opts.capture_console {
        return skip("オプション");
    }
    // コマンドラインの書き方では文字列として渡せない引数（"21" など）は、型の違う引数になってしまう。
    if args.iter().any(ambiguous) {
        return skip("コマンドラインで書けない引数");
    }
    // 入口の指定や検査のエラーは、実行ファイルを作る前の話。
    match want {
        Err(f) if !matches!(f.error, RunError::Stop(_)) => return skip("停止でない失敗"),
        Err(f) => {
            if let RunError::Stop(s) = &f.error {
                if s.kind == StopKind::StackExhausted {
                    return skip("スタックの枯渇");
                }
            }
        }
        _ => {}
    }
    let dir = scratch("both");
    let exe = dir.join("prog");
    match try_build(srcs, func, &exe) {
        Err(e) => panic!("{func}: ネイティブのビルドに失敗: {e}"),
        Ok(None) => {
            let _ = std::fs::remove_dir_all(&dir);
            return skip("BuildUnsupported");
        }
        Ok(Some(())) => {}
    }
    let got = exec(&exe, args);
    let _ = std::fs::remove_dir_all(&dir);
    CHECKED.fetch_add(1, Ordering::SeqCst);
    match want {
        Ok(o) => {
            assert_eq!(got.code, Some(0), "{func}: 終了コード\n{}", got.stderr);
            assert_eq!(
                got.stdout,
                format!("{}{}\n", o.output, o.value),
                "{func}: 標準出力"
            );
        }
        Err(f) => {
            let RunError::Stop(s) = &f.error else {
                unreachable!()
            };
            assert_eq!(got.code, Some(70), "{func}: 終了コード\n{}", got.stderr);
            assert_eq!(got.stdout, f.output, "{func}: 停止までの出力");
            assert_eq!(got.stderr.trim_end(), s.to_string(), "{func}: 停止の報告");
        }
    }
}

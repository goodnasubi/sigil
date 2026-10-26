//! ネイティブ出力との突き合わせ（ADR 0015 の決定 1・4、PR4）。
//!
//! `common::run*` が走らせた同じプログラムを、常に `sigil::native::build` で実行ファイルにして走らせ、
//! インタープリタの結果（値・出力・終了コード・停止の報告・破棄の順序）と突き合わせる。
//! `cc` や `libc.a` がなければ、飛ばさずに失敗させる（暗黙に通らないようにするため。ADR 0015 の決定 8）。
//!
//! 突き合わせの範囲（ADR 0015 の決定 8）:
//! - `fs_files` のある事例は、表の内容を一時ディレクトリの実ファイルに置き（仮想の `p` は `<一時>/<p の先頭の / を除いたもの>`）、
//!   引数と期待値の `p` を実際のパスに置き換えて比べる。表の最後の内容は比べない。
//! - `capture_console=false` の事例は、出力以外（値・停止の報告・破棄の順序・終了コード）を比べる。
//!   成功のときの標準出力は、末尾が値であることだけを確かめる。
//! - 飛ばすのは、停止でない失敗（検査・入口の指定のエラー）と、コマンドラインの書き方で表せない引数だけ。
#![allow(dead_code)]

use sigil::codegen::Opts;
use sigil::interp::{Arg, Failure, Options, Outcome, RunError, StopKind};
use sigil::native;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

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

/// 実行ファイルを作る。作れないのは（`cc` がないことを含め）失敗として返す。
pub fn try_build(
    srcs: &[&str],
    func: &str,
    out: &std::path::Path,
    trace_drops: bool,
) -> Result<(), String> {
    native::build(srcs, func, &Opts { trace_drops }, out).map_err(|e| e.to_string())
}

/// 実行ファイルを `--trace-drops` つきで作って走らせ、標準エラーの破棄の記録（`drop: ` の後ろ）を返す。
pub fn native_drops(srcs: &[&str], func: &str, args: &[Arg]) -> (Ran, Vec<String>) {
    let dir = scratch("drops");
    let exe = dir.join("prog");
    try_build(srcs, func, &exe, true).unwrap_or_else(|e| panic!("{func}: {e}"));
    let got = exec(&exe, args);
    let _ = std::fs::remove_dir_all(&dir);
    let drops = got
        .stderr
        .lines()
        .filter_map(|l| l.strip_prefix("drop: "))
        .map(str::to_string)
        .collect();
    (got, drops)
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

/// 突き合わせと飛ばしの記録。テストのバイナリごとに `native-report-<名前>.tsv` へ書く
/// （`scripts/native-report.sh` が数える）。
fn record(kind: &str, why: &str, func: &str) {
    use std::io::Write;
    static START: std::sync::Once = std::sync::Once::new();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let name = exe.rsplit_once('-').map_or(exe.as_str(), |(a, _)| a);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("native-report-{name}.tsv"));
    START.call_once(|| {
        let _ = std::fs::remove_file(&path);
    });
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(format!("{kind}\t{why}\t{func}\n").as_bytes());
    }
}

fn note(what: &str) {
    if std::env::var("SIGIL_NATIVE_VERBOSE").is_ok_and(|v| v == "1") {
        eprintln!(
            "native: {what}（突き合わせ {} / 飛ばし {}）",
            CHECKED.load(Ordering::SeqCst),
            SKIPPED.load(Ordering::SeqCst)
        );
    }
}

/// `text` の中の `keys` の各語を、対応する値に置き換える（長い語を先に探す。置き換えた中身は探し直さない）。
fn replace_keys(text: &str, keys: &[(String, String)]) -> String {
    let mut out = String::new();
    let mut i = 0;
    'scan: while i < text.len() {
        for (k, v) in keys {
            if text[i..].starts_with(k.as_str()) {
                out.push_str(v);
                i += k.len();
                continue 'scan;
            }
        }
        let c = text[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn map_arg(a: &Arg, keys: &[(String, String)]) -> Arg {
    match a {
        Arg::Str(s) => Arg::Str(replace_keys(s, keys)),
        Arg::List(xs) => Arg::List(xs.iter().map(|x| map_arg(x, keys)).collect()),
        other => other.clone(),
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
    let skip = |why: &str| {
        SKIPPED.fetch_add(1, Ordering::SeqCst);
        record("skip", why, func);
        note(&format!("飛ばす: {why} {func}"));
    };
    // コマンドラインの書き方では文字列として渡せない引数（"21" など）は、型の違う引数になってしまう。
    if args.iter().any(ambiguous) {
        return skip("コマンドラインで書けない引数");
    }
    // 入口の指定や検査のエラーは、実行ファイルを作る前の話。
    if let Err(f) = want {
        if !matches!(f.error, RunError::Stop(_)) {
            return skip("停止でない失敗");
        }
    }
    let dir = scratch("both");
    let exe = dir.join("prog");
    // fs_files は、実ファイルとして置く。仮想のパス p は `<dir>/files/<p の先頭の / を除いたもの>`。
    let mut keys: Vec<(String, String)> = Vec::new();
    if let Some(table) = &opts.fs_files {
        let root = dir.join("files");
        for (p, content) in table {
            let rel = p.trim_start_matches('/');
            if p.is_empty() || rel.is_empty() || rel.split('/').any(|c| c == "..") {
                continue;
            }
            let real = root.join(rel);
            std::fs::create_dir_all(real.parent().unwrap()).unwrap();
            std::fs::write(&real, content).unwrap();
            keys.push((p.clone(), real.to_string_lossy().into_owned()));
        }
        // 表にない絶対パスの引数も一時ディレクトリの下へ移す（親だけ作る。ファイルは作らない）。
        // fs.write が本物のファイルシステムを書き換えないようにするため。
        let mut extra = Vec::new();
        fn paths(a: &Arg, out: &mut Vec<String>) {
            match a {
                Arg::Str(s) if s.starts_with('/') => out.push(s.clone()),
                Arg::List(xs) => xs.iter().for_each(|x| paths(x, out)),
                _ => {}
            }
        }
        args.iter().for_each(|a| paths(a, &mut extra));
        for p in extra {
            let rel = p.trim_start_matches('/');
            if table.contains_key(&p) || rel.is_empty() || rel.split('/').any(|c| c == "..") {
                continue;
            }
            let real = root.join(rel);
            std::fs::create_dir_all(real.parent().unwrap()).unwrap();
            keys.push((p.clone(), real.to_string_lossy().into_owned()));
        }
        keys.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    }
    let args: Vec<Arg> = args.iter().map(|a| map_arg(a, &keys)).collect();
    match native::build(
        srcs,
        func,
        &Opts {
            trace_drops: opts.trace_drops,
        },
        &exe,
    ) {
        Ok(()) => {}
        // コマンドラインで受け取れない型の引数（空の List<構造体> など）は、インタープリタには渡せても実行ファイルにはならない。
        Err(native::BuildError::Run(RunError::Entry(m)))
            if m.contains("コマンドラインから受け取れない") =>
        {
            let _ = std::fs::remove_dir_all(&dir);
            return skip("コマンドラインで書けない引数の型");
        }
        Err(e) => panic!("{func}: ネイティブのビルドに失敗: {e}"),
    }
    let got = exec(&exe, &args);
    let _ = std::fs::remove_dir_all(&dir);
    // スタックの枯渇は、インタープリタの枠の大きさで決まる。実行ファイルの枠は小さく、同じ深さでは枯渇しないことがある
    // （その場合は止まらず値を返す）。止まったときだけ、止まり方を比べる。
    if let Err(f) = want {
        if matches!(&f.error, RunError::Stop(s) if s.kind == StopKind::StackExhausted)
            && got.code != Some(70)
        {
            return skip("スタックの枯渇（実行ファイルでは枯渇しない）");
        }
    }
    CHECKED.fetch_add(1, Ordering::SeqCst);
    record("checked", "", func);
    // `--trace-drops` の行は標準エラーから外して、破棄の順序として比べる。
    let (drops, rest): (Vec<&str>, Vec<&str>) = got
        .stderr
        .lines()
        .partition(|l| opts.trace_drops && l.starts_with("drop: "));
    let drops: Vec<String> = drops
        .iter()
        .map(|l| l["drop: ".len()..].to_string())
        .collect();
    let stderr = rest.join("\n");
    let want_drops: Vec<String> = match want {
        Ok(o) => o.drops.clone(),
        Err(f) => f.drops.to_vec(),
    };
    match want {
        Ok(o) => {
            assert_eq!(got.code, Some(0), "{func}: 終了コード\n{}", got.stderr);
            let value = replace_keys(&o.value, &keys);
            if opts.capture_console {
                let output = replace_keys(&o.output, &keys);
                assert_eq!(got.stdout, format!("{output}{value}\n"), "{func}: 標準出力");
            } else {
                // 本物の標準出力へ書いた出力は比べない。値だけが最後の行にある。
                assert!(
                    got.stdout.ends_with(&format!("{value}\n")),
                    "{func}: 値 {value} が最後の行にない: {}",
                    got.stdout
                );
            }
            if opts.trace_drops {
                assert_eq!(drops, want_drops, "{func}: 破棄の順序");
            }
        }
        Err(f) => {
            let RunError::Stop(s) = &f.error else {
                unreachable!()
            };
            assert_eq!(got.code, Some(70), "{func}: 終了コード\n{}", got.stderr);
            if s.kind == StopKind::StackExhausted {
                // 枠の数は実装で違うので、種類（報告の見出し）だけを比べる。破棄の順序の形は呼び出し側が確かめる。
                let head = |t: &str| t.lines().next().unwrap_or("").to_string();
                assert_eq!(
                    head(stderr.trim_end()),
                    head(&s.to_string()),
                    "{func}: 停止の報告"
                );
                return;
            }
            if opts.capture_console {
                assert_eq!(
                    got.stdout,
                    replace_keys(&f.output, &keys),
                    "{func}: 停止までの出力"
                );
            }
            assert_eq!(
                stderr.trim_end(),
                replace_keys(&s.to_string(), &keys),
                "{func}: 停止の報告"
            );
            if opts.trace_drops {
                assert_eq!(drops, want_drops, "{func}: 破棄の順序");
            }
        }
    }
}

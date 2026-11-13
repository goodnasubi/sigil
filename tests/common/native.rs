//! ネイティブ出力との突き合わせ（ADR 0015 の決定 1・4、PR4）。
//!
//! `common::run*` が走らせた同じプログラムを、常に `sigil::native::build` で実行ファイルにして走らせ、
//! SIR の評価器の結果（値・出力・終了コード・停止の報告・破棄の順序）と突き合わせる。
//! `cc` や `libc.a` がなければ、飛ばさずに失敗させる（暗黙に通らないようにするため。ADR 0015 の決定 8）。
//!
//! 突き合わせの範囲（ADR 0015 の決定 8）:
//! - `fs_files` のある事例は、表の内容を一時ディレクトリの実ファイルに置き（仮想の `p` は `<一時>/<p の先頭の / を除いたもの>`）、
//!   引数と期待値の `p` を実際のパスに置き換えて比べる。表の最後の内容は比べない。
//! - `capture_console=false` の事例は、出力以外（値・停止の報告・破棄の順序・終了コード）を比べる。
//!   成功のときの標準出力は、末尾が値であることだけを確かめる。
//! - コマンドラインで表せない値は、引数を組み立てて元の関数を呼ぶ試験用の入口で突き合わせる。
#![allow(dead_code)]

use sigil::codegen::Opts;
use sigil::exec::{Arg, Failure, Options, Outcome, RunError, StopKind};
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
        Arg::Str(s) => {
            s.contains('\0')
                || !matches!(sigil::shared_args::parse_arg(s), Ok(Arg::Str(t)) if &t == s)
        }
        Arg::List(xs) => xs.iter().any(ambiguous),
        _ => false,
    }
}

/// CLI で値の種類が変わる場合と、空の List<構造体> を渡す場合だけ、
/// ソースの末尾に引数なしの入口を足す。元の関数・契約・行番号は変えない。
/// 通常の参照・Lock の入口は従来どおりビルドで除外する。
fn bound_entry(srcs: &[&str], func: &str, args: &[Arg], opts: &Opts) -> Option<String> {
    use sigil::ast::{Item, Type};
    use sigil::ir::Ty;
    if !args.iter().any(ambiguous)
        && !args
            .iter()
            .any(|arg| matches!(arg, Arg::List(xs) if xs.is_empty()))
    {
        return None;
    }
    let program = sigil::ir_eval::lower_sources_with(srcs, func, &opts.trust_base).unwrap();
    let entry = &program.funcs[program.entry];
    let empty_adt_list = args.iter().zip(&entry.params).any(|(arg, &param)| {
        let mut ty = &entry.locals[param].ty;
        if let Ty::Ref(inner) = ty {
            ty = inner;
        }
        matches!((arg, ty), (Arg::List(xs), Ty::List(inner))
            if xs.is_empty() && matches!(inner.as_ref(), Ty::Adt(_)))
    });
    if !args.iter().any(ambiguous) && !empty_adt_list {
        return None;
    }
    let file = sigil::parse(srcs[0]).unwrap();
    assert!(!file
        .items
        .iter()
        .any(|item| matches!(item, Item::Fn(f) if f.name == "native_comparison_entry")));
    let decl = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(f) if f.name == func => Some(f),
            _ => None,
        })
        .expect("突き合わせの入口は先頭モジュールの関数");
    let mut body = String::new();
    let mut call_args = Vec::new();
    let mut sequence = 0;
    for (arg, param) in args.iter().zip(&decl.params) {
        let (ty, borrow) = match &param.ty {
            Type::Ref { mutable, inner } => (inner.as_ref(), if *mutable { "&mut " } else { "&" }),
            ty => (ty, ""),
        };
        let name = bind_arg(arg, ty, &mut body, &mut sequence);
        call_args.push(format!("{borrow}{name}"));
    }
    let ret = decl
        .ret
        .as_ref()
        .map(type_text)
        .unwrap_or_else(|| "Unit".into());
    let effects = sigil::fmt::normalize_effects(&decl.effects, &decl.params);
    let effects = if effects.is_empty() {
        "none".into()
    } else {
        effects.join(", ")
    };
    Some(format!("{}\n\nfn native_comparison_entry() -> {ret}\n  intent \"突き合わせの引数を渡す\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}  return {func}({});\n}}\n", srcs[0], call_args.join(", ")))
}

fn type_text(ty: &sigil::ast::Type) -> String {
    use sigil::ast::Type;
    match ty {
        Type::Path { path, args } => {
            let name = path.join(".");
            if args.is_empty() {
                name
            } else {
                format!(
                    "{name}<{}>",
                    args.iter().map(type_text).collect::<Vec<_>>().join(", ")
                )
            }
        }
        Type::Ref { mutable, inner } => format!(
            "&{}{}",
            if *mutable { "mut " } else { "" },
            type_text(inner)
        ),
        Type::Tuple(items) => format!(
            "({})",
            items.iter().map(type_text).collect::<Vec<_>>().join(", ")
        ),
        Type::Fn { .. } => panic!("関数の値は Arg では渡せない"),
    }
}

/// 値は Sigil のリテラルと List.new/push で組み立てる。文字列の NUL・区切り文字も CLI を通らない。
fn bind_arg(arg: &Arg, ty: &sigil::ast::Type, body: &mut String, sequence: &mut usize) -> String {
    use sigil::ast::Type;
    let name = format!("native_arg_{}", *sequence);
    *sequence += 1;
    let value = match arg {
        Arg::Int(n) => n.to_string(),
        Arg::Float(n) if n.is_nan() => "(0.0 / 0.0)".into(),
        Arg::Float(n) if n.is_infinite() => {
            format!("({}.0 / 0.0)", if n.is_sign_negative() { -1 } else { 1 })
        }
        Arg::Float(n) => format!("{n:?}"),
        Arg::Bool(b) => b.to_string(),
        Arg::Str(s) => {
            let quoted = format!(
                "\"{}\"",
                s.chars()
                    .map(|c| format!("\\u{{{:x}}}", c as u32))
                    .collect::<String>()
            );
            if matches!(ty, Type::Path { path, .. } if path == &["Path"]) {
                format!("Path.new(&{quoted})")
            } else {
                quoted
            }
        }
        Arg::List(xs) => {
            let Type::Path { args, .. } = ty else {
                panic!("一覧の型がない")
            };
            body.push_str(&format!("  let mut {name} = List.new();\n"));
            for arg in xs {
                let item = bind_arg(arg, &args[0], body, sequence);
                body.push_str(&format!("  {name}.push({item});\n"));
            }
            return name;
        }
    };
    body.push_str(&format!("  let mut {name} = {value};\n"));
    name
}

/// 実行ファイルを作る。作れないのは（`cc` がないことを含め）失敗として返す。
pub fn try_build(
    srcs: &[&str],
    func: &str,
    out: &std::path::Path,
    trace_drops: bool,
) -> Result<(), String> {
    native::build(
        srcs,
        func,
        &Opts {
            trace_drops,
            ..Opts::default()
        },
        out,
    )
    .map_err(|e| e.to_string())
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
    let args: Vec<String> = args.iter().map(arg_text).collect();
    exec_env(exe, &args, &[])
}

/// 環境変数つきで走らせる（引数は書いたままの文字列）。
pub fn exec_env(exe: &std::path::Path, args: &[String], env: &[(&str, &str)]) -> Ran {
    let o = Command::new(exe)
        .args(args)
        .env("SIGIL_WORKERS", "1")
        .envs(env.iter().copied())
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

/// 突き合わせと飛ばしの記録。テストのバイナリとプロセスごとに `native-report-<名前>-<プロセスID>.tsv` へ書く
/// （nextest はテストごとに別プロセスで動くため、プロセスIDでファイルを分けないと、
/// 先に書いた行を後発のプロセスが削除で消してしまう。`scripts/native-report.sh` が数える）。
fn record(kind: &str, why: &str, func: &str) {
    use std::io::Write;
    static START: std::sync::Once = std::sync::Once::new();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_default();
    let name = exe.rsplit_once('-').map_or(exe.as_str(), |(a, _)| a);
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("native-report-{name}-{}.tsv", std::process::id()));
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

/// 実行前の失敗はビルドの診断、引数の型・個数の失敗は実行ファイルの入口診断と比べる。
/// 実行できないソースを成功扱いせず、同じ拒否が確認できたときだけ checked にする。
fn check_rejection(srcs: &[&str], func: &str, args: &[Arg], opts: &Options, want: &Failure) {
    let dir = scratch("rejection");
    let exe = dir.join("prog");
    let build_opts = Opts {
        trace_drops: opts.trace_drops,
        trust_base: opts.trust_base.clone(),
    };
    let built = native::build(srcs, func, &build_opts, &exe);
    // 拒否されたプログラムの入口には到達しない。曖昧な CLI 値を別の種類に変えない。
    let got = if built.is_ok() && matches!(want.error, RunError::Entry(_)) {
        assert!(
            !args.iter().any(ambiguous),
            "{func}: 入口エラーの引数を CLI で同じ種類として渡せない"
        );
        Some(exec(&exe, args))
    } else {
        None
    };
    let exists = exe.exists();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(want.output.is_empty(), "{func}: 拒否前の評価器の出力");
    assert!(want.drops.is_empty(), "{func}: 拒否前の評価器の破棄");
    match (built, got) {
        (Err(native::BuildError::Run(got)), None) => {
            let same = match (&want.error, &got) {
                (RunError::Check(a), RunError::Check(b)) => a == b,
                (
                    RunError::Module { file: a, error: ae },
                    RunError::Module { file: b, error: be },
                ) => a == b && ae == be,
                (RunError::Entry(a), RunError::Entry(b)) => a == b,
                _ => false,
            };
            assert!(same, "{func}: 拒否の診断が違う: {:?} / {got:?}", want.error);
            assert!(!exists, "{func}: ビルドで拒否したのに実行ファイルがある");
        }
        (Ok(()), Some(got)) => {
            assert_eq!(got.code, Some(1), "{func}: 入口エラーの終了コード");
            assert_eq!(got.stdout, "", "{func}: 入口エラーの標準出力");
            assert_eq!(
                got.stderr,
                format!("{}\n", want.error).trim_start(),
                "{func}: 入口エラーの標準エラー"
            );
        }
        (built, _) => panic!("{func}: 同じ拒否にならない: {:?} / {built:?}", want.error),
    }
}

/// SIR の評価器の結果 `want` と、ネイティブの実行結果を突き合わせる。
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
    // 検査・入口の拒否も突き合わせる。ビルド前の拒否と、実行時の引数の拒否を区別する。
    if let Err(f) = want {
        if matches!(f.error, RunError::Unsupported(_)) {
            return skip("停止でない失敗");
        }
        if !matches!(f.error, RunError::Stop(_)) {
            check_rejection(srcs, func, args, opts, f);
            CHECKED.fetch_add(1, Ordering::SeqCst);
            record("checked", "検査・入口の拒否", func);
            note(&format!("検査・入口の拒否を突き合わせた: {func}"));
            return;
        }
    }
    let dir = scratch("both");
    let exe = dir.join("prog");
    // fs_files は、実ファイルとして置く。仮想のパス p は `<dir>/files/<p の先頭の / を除いたもの>`。
    let mut keys: Vec<(String, String)> = Vec::new();
    if let Some(table) = &opts.fs_files {
        let root = dir.join("files");
        for (p, content) in table {
            // 空のパスは Path の契約(requires)で fs に届く前に止まるので、移し替えない。
            if p.is_empty() {
                continue;
            }
            let rel = p.trim_start_matches('/');
            // 一時ディレクトリの外へ出るパスは移し替えられない。黙って本物のファイルを触らないよう、失敗させる。
            assert!(
                !rel.is_empty() && !rel.split('/').any(|c| c == ".."),
                "{func}: fs_files のパス {p:?} は一時ディレクトリの下へ移し替えられない"
            );
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
            if table.contains_key(&p) {
                continue;
            }
            assert!(
                !rel.is_empty() && !rel.split('/').any(|c| c == ".."),
                "{func}: 引数のパス {p:?} は一時ディレクトリの下へ移し替えられない"
            );
            let real = root.join(rel);
            std::fs::create_dir_all(real.parent().unwrap()).unwrap();
            keys.push((p.clone(), real.to_string_lossy().into_owned()));
        }
        keys.sort_by_key(|(k, _)| std::cmp::Reverse(k.len()));
    }
    let args: Vec<Arg> = args.iter().map(|a| map_arg(a, &keys)).collect();
    let build_opts = Opts {
        trace_drops: opts.trace_drops,
        trust_base: opts.trust_base.clone(),
    };
    let bound = bound_entry(srcs, func, &args, &build_opts);
    let (build_srcs, entry, exec_args) = if let Some(ref source) = bound {
        let mut sources = srcs.to_vec();
        sources[0] = source;
        (sources, "native_comparison_entry", &[][..])
    } else {
        (srcs.to_vec(), func, args.as_slice())
    };
    match native::build(&build_srcs, entry, &build_opts, &exe) {
        Ok(()) => {}
        Err(native::BuildError::Run(RunError::Entry(m)))
            if m.contains("コマンドラインから受け取れない") =>
        {
            let _ = std::fs::remove_dir_all(&dir);
            // 参照型・Lock の入口引数は、ビルドで止めるのが仕様（#366）。
            if m.contains("入口の参照型") {
                return skip("仕様上ビルドで止める入口引数（参照・Lock）");
            }
            panic!("{func}: 試験用の入口でも引数を渡せない: {m}");
        }
        Err(e) => panic!("{func}: ネイティブのビルドに失敗: {e}"),
    }
    let got = exec(&exe, exec_args);
    let _ = std::fs::remove_dir_all(&dir);
    // スタックの枯渇は、SIR の評価器の枠の大きさで決まる。実行ファイルの枠は小さく、同じ深さでは枯渇しないことがある
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

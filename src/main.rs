use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "使い方:\n  sigil parse <ファイル>...        字句解析と構文解析だけを行い、最初のエラーを表示する\n  sigil fmt [--check] <ファイル>... 正規形に整形する（--check は書き換えず、最初の違反を表示して 1 で終わる）\n  sigil spec [--json] [--root <根>] <ファイル>    （--root の既定は現在のディレクトリ。trust base の許可は検査しない）仕様（モジュール・型・関数のシグネチャと 4 節）を日本語で表示する\n  sigil check [--json] [--root <根>] [--trust-base <モジュール>]... <ファイル>...        （--root の既定は現在のディレクトリ。`use` を辿る）構文解析のあと型検査を行い、最初の型エラー・効果のエラー・移動のエラー・借用のエラー・クロージャと契約の式のエラーを表示する\n  sigil lsp [--trust-base <モジュール>]...        言語サーバー（LSP）を標準入出力で動かし、診断を返す\n  sigil run [--root <根>] [--trace-drops] [--trust-base <モジュール>]... [--fs-double 仮のパス=実ファイル]... <ファイル> <関数> [引数...] `use` で辿れるモジュールを結んで検査のあと関数を実行する（--root の既定は現在のディレクトリ。引数は true/false・整数・小数・[a,b,...]・それ以外は文字列）。異常停止は終了コード 70\n  sigil build [--root <根>] [--trace-drops] [--trust-base <モジュール>]... <ファイル> <関数> -o <出力> `use` で辿れるモジュールを結んでネイティブの実行ファイルを作る（--root の既定は現在のディレクトリ。cc と glibc の静的ライブラリが要る）";

fn read_text(path: &str) -> Result<String, String> {
    sigil::modpath::read_source(std::path::Path::new(path))
}

fn read(path: &str) -> Option<String> {
    match read_text(path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("{path}: 読めない: {e}");
            None
        }
    }
}

/// 同じディレクトリの一時ファイルに書き、fsync してから rename で置き換える。
/// パーミッションは元のファイルから引き継ぐ。失敗しても元のファイルは変わらない。
fn write_atomic(path: &str, content: &str) -> std::io::Result<()> {
    use std::io::Write;
    let resolved = std::fs::canonicalize(path)?; // シンボリックリンクは先を置き換える
    let target = resolved.as_path();
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => std::path::Path::new("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let perms = std::fs::metadata(target)?.permissions();
    let tmp = dir.join(format!(".{name}.sigil-fmt-{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.set_permissions(perms)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

use sigil::shared_args::parse_arg;

/// 異常停止の終了コード（ADR 0006 の M3 の記録）。種類によらず同じ値。
const EXIT_ABNORMAL_STOP: u8 = 70;

fn done(failed: bool) -> ExitCode {
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("parse") if args.len() > 1 => {
            let mut failed = false;
            for path in &args[1..] {
                let Some(src) = read(path) else {
                    failed = true;
                    continue;
                };
                match sigil::parse(&src) {
                    Ok(_) => println!("{path}: ok"),
                    Err(e) => {
                        eprintln!("{path}:{e}");
                        failed = true;
                    }
                }
            }
            done(failed)
        }
        Some("fmt") => {
            let check = args.get(1).map(String::as_str) == Some("--check");
            let files = &args[if check { 2 } else { 1 }..];
            if files.is_empty() {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            let mut failed = false;
            for path in files {
                let Some(src) = read(path) else {
                    failed = true;
                    continue;
                };
                if check {
                    match sigil::fmt::check(&src) {
                        Ok(None) => println!("{path}: ok"),
                        Ok(Some(d)) => {
                            eprintln!(
                                "{path}:{}: 正規形でない\n  実際: {}\n  正規: {}",
                                d.line, d.found, d.expected
                            );
                            failed = true;
                        }
                        Err(e) => {
                            eprintln!("{path}:{e}");
                            failed = true;
                        }
                    }
                } else {
                    match sigil::fmt::format(&src) {
                        Ok(out) => {
                            if out != src {
                                if let Err(e) = write_atomic(path, &out) {
                                    eprintln!("{path}: 書けない: {e}");
                                    failed = true;
                                    continue;
                                }
                                println!("{path}: 整形した");
                            } else {
                                println!("{path}: 変更なし");
                            }
                        }
                        Err(e) => {
                            eprintln!("{path}:{e}");
                            failed = true;
                        }
                    }
                }
            }
            done(failed)
        }
        Some("check") if args.len() > 1 => {
            let mut trust_base: Vec<String> = vec![];
            let mut json = false;
            let mut root: Option<String> = None;
            let mut i = 1;
            loop {
                match args.get(i).map(String::as_str) {
                    Some("--root") => {
                        let Some(r) = args.get(i + 1) else {
                            eprintln!("--root にはディレクトリが要る");
                            return ExitCode::from(2);
                        };
                        root = Some(r.clone());
                        i += 2;
                    }
                    Some("--trust-base") => {
                        let Some(m) = args.get(i + 1) else {
                            eprintln!("--trust-base にはモジュール名が要る");
                            return ExitCode::from(2);
                        };
                        trust_base.push(m.clone());
                        i += 2;
                    }
                    Some("--json") => {
                        json = true;
                        i += 1;
                    }
                    _ => break,
                }
            }
            if args.len() <= i {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            let copts = sigil::effects::Options {
                trust_base,
                ..Default::default()
            };
            let root_dir = root_dir(root.as_deref());
            let mut failed = false;
            let mut diagnostics = vec![];
            for path in &args[i..] {
                let src = match read_text(path) {
                    Ok(s) => s,
                    Err(e) if json => {
                        // 読めないファイルも診断にする（ADR 0019 決定 2。位置は null）。
                        diagnostics.push(sigil::json::obj(vec![
                            ("file", path.as_str().into()),
                            ("kind", "ReadError".into()),
                            ("phase", "read".into()),
                            ("line", sigil::json::Value::Null),
                            ("col", sigil::json::Value::Null),
                            ("message", format!("読めない: {e}").into()),
                        ]));
                        failed = true;
                        continue;
                    }
                    Err(e) => {
                        eprintln!("{path}: 読めない: {e}");
                        failed = true;
                        continue;
                    }
                };
                let _ = src;
                let (file_path, checked) = match sigil::project::check_project(
                    &root_dir,
                    std::path::Path::new(path),
                    &copts,
                ) {
                    Ok(()) => (path.clone(), Ok(())),
                    Err(sigil::project::ProjectError::Diag { file, error }) => {
                        (file.display().to_string(), Err(error))
                    }
                    Err(sigil::project::ProjectError::Read { file, msg }) => {
                        let f = file.display().to_string();
                        if json {
                            diagnostics.push(sigil::json::obj(vec![
                                ("file", f.as_str().into()),
                                ("kind", "ReadError".into()),
                                ("phase", "read".into()),
                                ("line", sigil::json::Value::Null),
                                ("col", sigil::json::Value::Null),
                                ("message", format!("読めない: {msg}").into()),
                            ]));
                        } else {
                            eprintln!("{f}: 読めない: {msg}");
                        }
                        failed = true;
                        continue;
                    }
                };
                match checked {
                    Ok(()) if !json => println!("{path}: ok"),
                    Ok(()) => {}
                    Err(e) if json => {
                        diagnostics.push(e.to_json(&file_path));
                        failed = true;
                    }
                    Err(e) => {
                        eprintln!("{file_path}:{e}");
                        failed = true;
                    }
                }
            }
            if json {
                let out = sigil::json::obj(vec![
                    ("format", 1i64.into()),
                    ("ok", (!failed).into()),
                    ("diagnostics", sigil::json::Value::Arr(diagnostics)),
                ]);
                let _ = writeln!(std::io::stdout(), "{out}");
            }
            done(failed)
        }
        Some("lsp") => {
            let mut trust_base = vec![];
            let mut i = 1;
            while args.get(i).map(String::as_str) == Some("--trust-base") {
                let Some(m) = args.get(i + 1) else {
                    eprintln!("--trust-base にはモジュール名が要る");
                    return ExitCode::from(2);
                };
                trust_base.push(m.clone());
                i += 2;
            }
            if i < args.len() {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            let stdin = std::io::stdin();
            let code = sigil::lsp::serve(stdin.lock(), std::io::stdout().lock(), trust_base);
            ExitCode::from(code as u8)
        }
        Some("spec") if args.len() >= 2 => {
            let mut json = false;
            let mut root: Option<&str> = None;
            let mut i = 1;
            loop {
                match args.get(i).map(String::as_str) {
                    Some("--json") => {
                        json = true;
                        i += 1;
                    }
                    Some("--root") => {
                        let Some(r) = args.get(i + 1) else {
                            eprintln!("--root にはディレクトリが要る");
                            return ExitCode::from(2);
                        };
                        root = Some(r);
                        i += 2;
                    }
                    _ => break,
                }
            }
            if args.len() != i + 1 {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            let path = &args[i];
            let Some(src) = read(path) else {
                return ExitCode::FAILURE;
            };
            let result = sigil::modpath::verify(&src, &root_dir(root), std::path::Path::new(path))
                .and_then(|()| {
                    if json {
                        sigil::spec::extract(&src).map(|s| format!("{}\n", s.to_json()))
                    } else {
                        sigil::spec::spec(&src)
                    }
                });
            match result {
                Ok(out) => {
                    print!("{out}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{path}:{e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("run") if args.len() >= 3 => {
            // テスト用の差し替え（ADR 0004 の規則 7）。`--fs-double 仮のパス=実ファイル` で、
            // `fs.read(&仮のパス)` が実ファイルの内容を返す。指定がなければ実ファイルを読み書きする。同じ仮のパスの重複指定は使い方の誤り（終了コード 2）。
            let mut doubles: Vec<(String, String)> = vec![];
            let mut trace = false;
            let mut trust_base: Vec<String> = vec![];
            let mut root: Option<String> = None;
            let mut i = 1;
            while matches!(
                args.get(i).map(String::as_str),
                Some("--fs-double" | "--trace-drops" | "--trust-base" | "--root")
            ) {
                if args[i] == "--root" {
                    let Some(r) = args.get(i + 1) else {
                        eprintln!("--root にはディレクトリが要る");
                        return ExitCode::from(2);
                    };
                    root = Some(r.clone());
                    i += 2;
                    continue;
                }
                if args[i] == "--trace-drops" {
                    trace = true;
                    i += 1;
                    continue;
                }
                if args[i] == "--trust-base" {
                    let Some(m) = args.get(i + 1) else {
                        eprintln!("--trust-base にはモジュール名が要る");
                        return ExitCode::from(2);
                    };
                    trust_base.push(m.clone());
                    i += 2;
                    continue;
                }
                let Some((k, v)) = args.get(i + 1).and_then(|a| a.split_once('=')) else {
                    eprintln!("--fs-double には 仮のパス=実ファイル が要る");
                    return ExitCode::from(2);
                };
                if doubles.iter().any(|(d, _)| d == k) {
                    eprintln!("--fs-double の仮のパス {k} が重複している");
                    return ExitCode::from(2);
                }
                doubles.push((k.to_string(), v.to_string()));
                i += 2;
            }
            if args.len() < i + 2 {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            let args: Vec<String> = std::iter::once(args[0].clone())
                .chain(args[i..].iter().cloned())
                .collect();
            let mut opts = sigil::exec::Options {
                trace_drops: trace,
                trust_base,
                ..Default::default()
            };
            if !doubles.is_empty() {
                let mut m = std::collections::BTreeMap::new();
                for (k, v) in doubles {
                    let Some(text) = read(&v) else {
                        return ExitCode::FAILURE;
                    };
                    m.insert(k, text);
                }
                opts.fs_files = Some(m);
            }
            let path = &args[1];
            let root_dir = root_dir(root.as_deref());
            let call = match args[3..]
                .iter()
                .map(|a| parse_arg(a))
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(c) => c,
                Err(m) => {
                    eprintln!("{path}: 入口の指定が誤り: {m}");
                    return ExitCode::FAILURE;
                }
            };
            // 実行は SIR の評価器で行う（ADR 0017）。`sigil build` の実行ファイルと同じ lowering を通る。
            let show_drops = |drops: &[String]| {
                for d in drops {
                    eprintln!("drop: {d}");
                }
            };
            match sigil::ir_eval::run_project_with(
                &root_dir,
                std::path::Path::new(path),
                &args[2],
                &call,
                &opts,
            ) {
                Ok(out) => {
                    show_drops(&out.drops);
                    // 標準出力が閉じていても（Broken pipe など）パニックせず、黙って終える。
                    let _ = writeln!(std::io::stdout(), "{}", out.value);
                    ExitCode::SUCCESS
                }
                Err(f) => match f.error {
                    sigil::exec::RunError::Stop(s) => {
                        show_drops(&f.drops);
                        eprintln!("{path}: {s}");
                        ExitCode::from(EXIT_ABNORMAL_STOP)
                    }
                    e @ sigil::exec::RunError::Module { .. } => {
                        eprintln!("{e}");
                        ExitCode::FAILURE
                    }
                    e => {
                        eprintln!("{path}:{e}");
                        ExitCode::FAILURE
                    }
                },
            }
        }
        Some("build") if args.len() >= 4 => {
            let mut trace = false;
            let mut trust_base: Vec<String> = vec![];
            let mut root: Option<String> = None;
            let mut rest: Vec<&String> = args[1..].iter().collect();
            loop {
                match rest.first().map(|s| s.as_str()) {
                    Some("--trace-drops") => {
                        trace = true;
                        rest.remove(0);
                    }
                    Some("--trust-base") if rest.len() >= 2 => {
                        trust_base.push(rest[1].clone());
                        rest.drain(..2);
                    }
                    Some("--root") if rest.len() >= 2 => {
                        root = Some(rest[1].clone());
                        rest.drain(..2);
                    }
                    _ => break,
                }
            }
            let n = rest.len();
            if n != 4 || rest[n - 2].as_str() != "-o" {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
            let out = rest[n - 1].clone();
            let func = rest[n - 3].clone();
            let file = rest[0];
            let opts = sigil::codegen::Opts {
                trace_drops: trace,
                trust_base,
            };
            match sigil::native::build_project(
                &root_dir(root.as_deref()),
                std::path::Path::new(file),
                &func,
                &opts,
                std::path::Path::new(&out),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(sigil::native::BuildError::Run(e @ sigil::exec::RunError::Module { .. })) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("{file}:{e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

/// `--root`（指定がなければ現在のディレクトリ。ADR 0021 の 2-2c）。
fn root_dir(root: Option<&str>) -> std::path::PathBuf {
    match root {
        Some(r) => std::path::PathBuf::from(r),
        None => std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    }
}

use std::process::ExitCode;

const USAGE: &str = "使い方:\n  sigil parse <ファイル>...        字句解析と構文解析だけを行い、最初のエラーを表示する\n  sigil fmt [--check] <ファイル>... 正規形に整形する（--check は書き換えず、最初の違反を表示して 1 で終わる）\n  sigil spec <ファイル>             仕様（モジュール・型・関数のシグネチャと 4 節）を日本語で表示する\n  sigil check <ファイル>...        構文解析のあと型検査を行い、最初の型エラー・効果のエラー・移動のエラー・借用のエラー・クロージャと契約の式のエラーを表示する\n  sigil run <ファイル> <関数> [引数...] 検査のあと関数を実行する（引数は true/false・整数・小数・[a,b,...]・それ以外は文字列）。異常停止は終了コード 70";

fn read(path: &str) -> Option<String> {
    match std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| String::from_utf8(b).map_err(|_| "UTF-8 でない".to_string()))
    {
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

fn parse_arg(s: &str) -> sigil::interp::Arg {
    use sigil::interp::Arg;
    if s == "true" || s == "false" {
        return Arg::Bool(s == "true");
    }
    if let Ok(i) = s.parse::<i64>() {
        return Arg::Int(i);
    }
    if let Ok(f) = s.parse::<f64>() {
        return Arg::Float(f);
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let items = if inner.trim().is_empty() {
            Vec::new()
        } else {
            inner.split(',').map(|x| parse_arg(x.trim())).collect()
        };
        return Arg::List(items);
    }
    Arg::Str(s.to_string())
}

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
            let mut failed = false;
            for path in &args[1..] {
                let Some(src) = read(path) else {
                    failed = true;
                    continue;
                };
                match sigil::check::check(&src) {
                    Ok(()) => println!("{path}: ok"),
                    Err(e) => {
                        eprintln!("{path}:{e}");
                        failed = true;
                    }
                }
            }
            done(failed)
        }
        Some("spec") if args.len() == 2 => {
            let path = &args[1];
            let Some(src) = read(path) else {
                return ExitCode::FAILURE;
            };
            match sigil::spec::spec(&src) {
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
            let path = &args[1];
            let Some(src) = read(path) else {
                return ExitCode::FAILURE;
            };
            let call: Vec<sigil::interp::Arg> = args[3..].iter().map(|a| parse_arg(a)).collect();
            match sigil::interp::run(&src, &args[2], &call) {
                Ok(out) => {
                    print!("{}", out.output);
                    println!("{}", out.value);
                    ExitCode::SUCCESS
                }
                Err(f) => {
                    print!("{}", f.output);
                    match f.error {
                        sigil::interp::RunError::Stop(s) => {
                            eprintln!("{path}: {s}");
                            ExitCode::from(EXIT_ABNORMAL_STOP)
                        }
                        e => {
                            eprintln!("{path}:{e}");
                            ExitCode::FAILURE
                        }
                    }
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

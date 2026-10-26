use std::io::Write;
use std::process::ExitCode;

const USAGE: &str = "使い方:\n  sigil parse <ファイル>...        字句解析と構文解析だけを行い、最初のエラーを表示する\n  sigil fmt [--check] <ファイル>... 正規形に整形する（--check は書き換えず、最初の違反を表示して 1 で終わる）\n  sigil spec <ファイル>             仕様（モジュール・型・関数のシグネチャと 4 節）を日本語で表示する\n  sigil check <ファイル>...        構文解析のあと型検査を行い、最初の型エラー・効果のエラー・移動のエラー・借用のエラー・クロージャと契約の式のエラーを表示する\n  sigil run [--fs-double 仮のパス=実ファイル]... <ファイル> <関数> [引数...] 検査のあと関数を実行する（引数は true/false・整数・小数・[a,b,...]・それ以外は文字列）。異常停止は終了コード 70";

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
            // テスト用の差し替え（ADR 0004 の規則 7）。`--fs-double 仮のパス=実ファイル` で、
            // `fs.read(&仮のパス)` が実ファイルの内容を返す。指定がなければ実ファイルを読み書きする。同じ仮のパスの重複指定は使い方の誤り（終了コード 2）。
            let mut doubles: Vec<(String, String)> = vec![];
            let mut i = 1;
            while args.get(i).map(String::as_str) == Some("--fs-double") {
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
            let mut opts = sigil::interp::Options::default();
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
            let Some(src) = read(path) else {
                return ExitCode::FAILURE;
            };
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
            match sigil::interp::run_with(&src, &args[2], &call, &opts) {
                Ok(out) => {
                    // 標準出力が閉じていても（Broken pipe など）パニックせず、黙って終える。
                    let _ = writeln!(std::io::stdout(), "{}", out.value);
                    ExitCode::SUCCESS
                }
                Err(f) => match f.error {
                    sigil::interp::RunError::Stop(s) => {
                        eprintln!("{path}: {s}");
                        ExitCode::from(EXIT_ABNORMAL_STOP)
                    }
                    e => {
                        eprintln!("{path}:{e}");
                        ExitCode::FAILURE
                    }
                },
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

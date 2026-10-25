use std::process::ExitCode;

const USAGE: &str = "使い方:\n  sigil parse <ファイル>...        字句解析と構文解析だけを行い、最初のエラーを表示する\n  sigil fmt [--check] <ファイル>... 正規形に整形する（--check は書き換えず、最初の違反を表示して 1 で終わる）\n（M0 の途中。spec は未実装）";

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
                                if let Err(e) = std::fs::write(path, &out) {
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
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

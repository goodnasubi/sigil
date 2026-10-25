use std::process::ExitCode;

const USAGE: &str = "使い方: sigil parse <ファイル>...\n  字句解析と構文解析だけを行い、最初のエラーを表示する（M0 の途中。fmt・spec は未実装）";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("parse") if args.len() > 1 => {
            let mut failed = false;
            for path in &args[1..] {
                let src = match std::fs::read(path)
                    .map_err(|e| e.to_string())
                    .and_then(|b| String::from_utf8(b).map_err(|_| "UTF-8 でない".to_string()))
                {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("{path}: 読めない: {e}");
                        failed = true;
                        continue;
                    }
                };
                match sigil::parse(&src) {
                    Ok(_) => println!("{path}: ok"),
                    Err(e) => {
                        eprintln!("{path}:{e}");
                        failed = true;
                    }
                }
            }
            if failed {
                ExitCode::FAILURE
            } else {
                ExitCode::SUCCESS
            }
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}

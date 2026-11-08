//! ADR 0025 補足 C-7 の 3-7c: ネイティブ出力のリンクを `cc -nostdlib -static -no-pie` へ移行する（ADR 0018 判断 7）。
//!
//! 出力した実行ファイルについて、次を確かめる。
//! - 未解決のシンボルが無い（`nm -u`）。glibc（`__libc_*`・`_IO_*`・`malloc` など）のシンボルを持たない。
//! - プログラムヘッダに `INTERP`・`DYNAMIC` が無い（動的リンカを使わない静的な実行ファイル）。
//! - 実際に走る（標準出力・ファイル・引数・環境・終了コード）。
//!
//! 機能の広いプログラムを使うのは、機能ごとに暫定ランタイムの未解決参照が残っていないことを見るため。
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod common;
use common::native::{exec, exec_env, scratch, try_build};
use sigil::exec::Arg;
use std::path::Path;
use std::process::Command;

const HEAD: &str = "module m.link\n  targets server\n  trust none\n\n";

fn func(sig: &str, effects: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n")
}

fn build(items: &str, entry: &str) -> std::path::PathBuf {
    let src = format!("{HEAD}{items}");
    let exe = scratch("link37c").join("prog");
    try_build(&[&src], entry, &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    exe
}

fn tool(name: &str, args: &[&str], exe: &Path) -> String {
    let o = Command::new(name).args(args).arg(exe).output().unwrap();
    assert!(
        o.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// 未解決のシンボル、glibc のシンボル、`INTERP`・`DYNAMIC` が無いことを確かめる。
fn assert_freestanding(exe: &Path) {
    let undefined = tool("nm", &["-u"], exe);
    assert!(
        undefined.trim().is_empty(),
        "未解決のシンボルがある:\n{undefined}"
    );
    let all = tool("nm", &[], exe);
    for line in all.lines() {
        let name = line.split_whitespace().last().unwrap_or("");
        for bad in [
            "__libc_",
            "_IO_",
            "__GI_",
            "_dl_",
            "__stack_chk",
            "malloc",
            "printf",
            "__errno",
        ] {
            assert!(
                !name.contains(bad),
                "glibc 由来のシンボルが残っている: {name}"
            );
        }
    }
    let headers = tool("readelf", &["-lW"], exe);
    for bad in ["INTERP", "DYNAMIC"] {
        assert!(!headers.contains(bad), "{bad} がある:\n{headers}");
    }
    let sections = tool("readelf", &["-SW"], exe);
    assert!(!sections.contains(".interp"), "{sections}");
    assert!(!sections.contains(".dynamic"), "{sections}");
    let dynamic = Command::new("ldd").arg(exe).output().unwrap();
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&dynamic.stdout),
        String::from_utf8_lossy(&dynamic.stderr)
    );
    assert!(text.contains("not a dynamic executable"), "{text}");
}

#[test]
fn minimal_program_is_freestanding() {
    let exe = build(&func("main() -> Int", "none", "  return 7;"), "main");
    assert_freestanding(&exe);
    let got = exec(&exe, &[]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout, "7\n");
}

#[test]
fn wide_program_is_freestanding_and_runs() {
    // 文字列・一覧・バイト列・小数・標準出力を一通り通る。どれも暫定ランタイムを呼ばない。
    let body = r#"  let mut xs = List.new();
  xs.push(3);
  xs.push(4);
  let s = "  abc  ".trim();
  let b = s.as_bytes();
  let f = 1.5 + 2.25;
  console.log(&s);
  console.log(&f);
  console.log(&b.len());
  return xs.sum() + s.len() + "a\nb\nc".lines().len();"#;
    let exe = build(&func("main() -> Int", "console", body), "main");
    assert_freestanding(&exe);
    let got = exec(&exe, &[]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout, "abc\n3.75\n3\n13\n");
}

#[test]
fn fs_and_path_program_is_freestanding_and_runs() {
    let dir = scratch("link37c-fs");
    let file = dir.join("hello.txt");
    let path = file.to_string_lossy().replace('\\', "/");
    let body = format!(
        "  let p = Path.new(&\"{path}\");\n  match fs.write(&p, &\"こんにちは\") {{\n    Ok(_) => {{}}\n    Err(_) => {{ return 1; }}\n  }}\n  match fs.read(&p) {{\n    Ok(s) => {{ console.log(&s); }}\n    Err(_) => {{ return 2; }}\n  }}\n  return 0;"
    );
    let exe = build(
        &func("main() -> Int", "console, fs.read, fs.write", &body),
        "main",
    );
    assert_freestanding(&exe);
    let got = exec(&exe, &[]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout, "こんにちは\n0\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "こんにちは");
}

#[test]
fn arguments_stop_report_and_live_environment_work() {
    // 入口の引数（argv）、異常停止（終了コード 70）、環境変数 SIGIL_RT_LIVE（envp）が、glibc なしで動く。
    let exe = build(
        &func("main(n: Int) -> Int", "none", "  return n * 2;"),
        "main",
    );
    assert_freestanding(&exe);
    let got = exec(&exe, &[Arg::Int(21)]);
    assert_eq!(got.stdout, "42\n");
    let live = exec_env(&exe, &["1".into()], &[("SIGIL_RT_LIVE", "1")]);
    assert_eq!(live.code, Some(0), "{}", live.stderr);
    assert!(live.stderr.contains("live: "), "{}", live.stderr);
    let stop = build(
        &func("main(n: Int) -> Int", "none", "  return 10 / n;"),
        "main",
    );
    let got = exec(&stop, &[Arg::Int(0)]);
    assert_eq!(got.code, Some(70), "{}", got.stderr);
}

//! Issue #781: 並行実行時の直列化の疑いに関する検証。
//! 単一ワーカー（SIGIL_WORKERS=1）および複数ワーカー（SIGIL_WORKERS=2）において、
//! 多数の並行タスクが同時にメモリ確保・チャネル送受信を行っても
//! デッドロックなく正常に完了し、リークがないことを確かめる。

mod common;

use common::native::scratch;
use sigil::codegen::Opts;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn execute(exe: &Path, workers: Option<&str>) -> Output {
    let mut cmd = Command::new("timeout");
    cmd.arg("20s")
        .arg(exe)
        .env_remove("SIGIL_WORKERS")
        .env("SIGIL_RT_LIVE", "1");
    if let Some(n) = workers {
        cmd.env("SIGIL_WORKERS", n);
    }
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_ne!(
        output.status.code(),
        Some(124),
        "join または待機がハングした"
    );
    if output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr
                .lines()
                .any(|l| l.starts_with("live: ") && l.contains("sigil_blocks=0")),
            "確保した塊が返っていない: {stderr}"
        );
    }
    output
}

fn build(src: &str, exe: &Path) {
    sigil::native::build(
        &[src],
        "run",
        &Opts {
            trust_base: vec!["m.a".into()],
            ..Opts::default()
        },
        exe,
    )
    .unwrap();
}

fn source(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust base\n\n{body}")
}

#[test]
fn single_worker_concurrent_alloc_and_channels() {
    // SIGIL_WORKERS=1 で 16 並行タスクが同時にチャネル送受信とアロケーションを行い正常終了することを確認する。
    let mut tasks = String::new();
    for i in 0..16 {
        tasks.push_str(&format!(
            r#"
    let tx{i} = tx.clone();
    let h{i} = s.spawn(move || {{
      let mut j = 0;
      while j < 50 {{
        let msg = "payload_item_".clone();
        tx{i}.send(msg);
        task.yield();
        j = j + 1;
      }}
      return 0;
    }});
"#
        ));
    }
    let joins: String = (0..16).map(|i| format!("    h{i}.join();\n")).collect();
    let src = source(&format!(
        r#"
fn run() -> Int
  intent "16 の並行タスクが同時にチャネル送受信とメモリ確保を行う"
  requires none
  ensures none
  effects task
{{
  return task.scope(|s| {{
    let (tx, rx) = task.channel();
{tasks}
    let mut total = 0;
    let mut k = 0;
    while k < 800 {{
      let m = rx.recv();
      total = total + m.char_count();
      k = k + 1;
    }}
{joins}
    return total;
  }});
}}
"#
    ));
    let dir = scratch("781-single-worker");
    let exe = dir.join("prog");
    build(&src, &exe);

    // SIGIL_WORKERS=1（協調的非同期モード）
    let o = execute(&exe, Some("1"));
    assert!(o.status.success(), "{o:?}");
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "10400");

    // SIGIL_WORKERS=2（マルチワーカーモード）
    let o2 = execute(&exe, Some("2"));
    assert!(o2.status.success(), "{o2:?}");
    assert_eq!(String::from_utf8_lossy(&o2.stdout).trim(), "10400");

    std::fs::remove_dir_all(dir).unwrap();
}

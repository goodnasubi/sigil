//! ADR 0033 K: シグナル許可表の用途制限と signalfd の受信。
mod common;

fn eval(body: &str) -> Result<String, String> {
    let source = format!(
        "module m.signal\n  targets server\n  trust base\n\nfn run() -> Int\n  intent \"シグナル許可表の検証\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"試験で確保したマスクだけを渡す\"\n{{\n{body}\n}}\n"
    );
    sigil::ir_eval::run_with(
        &source,
        "run",
        &[],
        &sigil::exec::Options {
            trust_base: vec!["m.signal".into()],
            ..Default::default()
        },
    )
    .map(|o| o.value)
    .map_err(|e| format!("{e:?}"))
}

fn denied(body: &str, needle: &str) {
    let error = eval(body).expect_err("限定した用途以外は発行前に拒否する");
    assert!(error.contains(needle), "{error}");
}

#[test]
fn rejects_other_signals_modes_flags_and_unused_arguments() {
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 512); return raw.syscall(289, -1, raw.addr_to_int(m), 8, 526336, 0, 0);", "SIGTERM / SIGINT");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(14, 2, raw.addr_to_int(m), 0, 8, 0, 0);", "SIG_BLOCK / SIG_UNBLOCK");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(14, 0, raw.addr_to_int(m), 0, 16, 0, 0);", "sigsetsize");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(289, -1, raw.addr_to_int(m), 8, 0, 0, 0);", "NONBLOCK|CLOEXEC");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(289, 3, raw.addr_to_int(m), 8, 526336, 0, 0);", "新規 fd");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(14, 0, raw.addr_to_int(m), 0, 8, 1, 0);", "未使用引数");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(289, -1, raw.addr_to_int(m), 8, 526336, 0, 1);", "未使用引数");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 512); return raw.syscall(14, 1, raw.addr_to_int(m), 0, 8, 0, 0);", "SIGTERM / SIGINT");
    denied("return raw.syscall(13, 15, 0, 0, 8, 0, 0);", "許可表にない");
}

#[test]
fn validates_both_input_and_output_memory_before_changing_mask() {
    denied("return raw.syscall(14, 0, 1, 0, 8, 0, 0);", "範囲外");
    denied("let m = raw.alloc(8, 8); raw.store_i64(m, 16386); return raw.syscall(14, 0, raw.addr_to_int(m), 1, 8, 0, 0);", "範囲外");
    denied("return raw.syscall(289, -1, 1, 8, 526336, 0, 0);", "範囲外");
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn evaluator_allows_create_close_and_restore_only_shutdown_bits() {
    let value = eval(
        r#"
  let m = raw.alloc(8, 8);
  let prior_set = raw.alloc(8, 8);
  raw.store_i64(m, 16386);
  let blocked = raw.syscall(14, 0, raw.addr_to_int(m), raw.addr_to_int(prior_set), 8, 0, 0);
  let fd = raw.syscall(289, -1, raw.addr_to_int(m), 8, 526336, 0, 0);
  let mut result = 0;
  if blocked != 0 || fd < 0 { result = 1; }
  if fd >= 0 { raw.syscall(3, fd, 0, 0, 0, 0, 0); }
  raw.store_i64(m, 16386 - (raw.load_i64(prior_set) & 16386));
  let restored = raw.syscall(14, 1, raw.addr_to_int(m), 0, 8, 0, 0);
  raw.free(m, 8, 8);
  raw.free(prior_set, 8, 8);
  return result + restored;
"#,
    )
    .unwrap();
    assert_eq!(value, "0");
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
fn native_signalfd_receives_term_and_int_and_restores_mask() {
    use std::process::Command;
    let source = r#"module std.os.signal_test
  targets server
  trust base

use std.os.io.shutdown_block;
use std.os.io.shutdown_close;
use std.os.io.shutdown_open;
use std.os.io.shutdown_read;
use std.os.io.shutdown_restore;

fn run() -> Int
  intent "SIGTERM/INT を非同期ハンドラ無しで受ける"
  requires none
  ensures none
  effects none
  unchecked "試験プロセス自身だけにシグナルを送り、受信後にマスクを戻す"
{
  let previous = shutdown_block();
  if previous < 0 { return 1; }
  let fd = shutdown_open();
  if fd < 0 { shutdown_restore(previous); return 2; }
  let pid = raw.syscall(39, 0, 0, 0, 0, 0, 0);
  raw.syscall(62, pid, 15, 0, 0, 0, 0);
  let term = shutdown_read(fd);
  raw.syscall(62, pid, 2, 0, 0, 0, 0);
  let interrupt = shutdown_read(fd);
  let flags = raw.syscall(72, fd, 3, 0, 0, 0, 0);
  let close_on_exec = raw.syscall(72, fd, 1, 0, 0, 0, 0);
  shutdown_close(fd);
  let restored = shutdown_restore(previous);
  if term != 15 || interrupt != 2 { return 3; }
  if (flags & 2048) == 0 || close_on_exec != 1 { return 4; }
  return restored;
}
"#;
    let dir = common::native::scratch("shutdown-signal-483");
    let exe = dir.join("signal");
    sigil::native::build(
        &[source],
        "run",
        &sigil::codegen::Opts {
            trust_base: vec!["std.os.signal_test".into()],
            ..Default::default()
        },
        &exe,
    )
    .unwrap();
    let output = Command::new("timeout")
        .arg("10s")
        .arg(exe)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shutdown_signal_is_opaque_and_cannot_be_cloned_in_trust_none() {
    for (body, expected) in [
        ("match net.shutdown_signal() { Ok(s) => { let copy = s.clone(); return 0; } Err(e) => { return 1; } }", "clone"),
        ("match net.shutdown_signal() { Ok(s) => { return s.fd; } Err(e) => { return 1; } }", "fd"),
        ("let s = ShutdownSignal { fd: 1 }; return 0;", "ShutdownSignal"),
    ] {
        let source = format!("module app.signal\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"所有 fd を偽造・複製できない\"\n  requires none\n  ensures none\n  effects net.server\n{{\n {body}\n}}\n");
        let error = sigil::ir_eval::lower_sources_with(&[&source], "main", &[])
            .expect_err("所有 fd のアクセスを拒否する");
        assert!(format!("{error:?}").contains(expected), "{error:?}");
    }
}

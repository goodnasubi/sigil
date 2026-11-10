//! ADR 0031 決定 1-1・1-3・2 / PR 6-3c: ネイティブの `io.wait_fd`（`raw.io_wait_fd`）の中断点、完了の列から実行可能待ち行列への結線、
//! 実時間モード（最初の fd の待ちの登録から時計が実時間に結ぶ）、遅延して作る I/O スレッド、本番の `body` の振り分け。
//!
//! 評価器は `io.wait_fd` を持たない（判断点 D まで `unreachable`）ので、ネイティブだけで試験する。待つ相手は `pipe2` の
//! 記述子で、書き込みの順序や時刻に依らない性質で確かめる。駆動のモジュール `std.os.t`（`trust base`）が入口で、
//! `task.scope` のタスクが `raw.io_wait_fd` を呼ぶ。
mod common;

use common::native::{scratch, Ran};
use sigil::effects::Options as COpts;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n";

fn driver(fns: &[(&str, &str, &str)], run_body: &str) -> String {
    let mut f = String::new();
    for (sig, eff, body) in fns {
        f.push_str(&format!(
            "fn {sig}\n{TAIL}  effects {eff}\n  unchecked \"試験の補助。自分で作った記述子と領域だけを触る\"\n{{\n{body}\n}}\n\n"
        ));
    }
    format!(
        "module std.os.t\n  targets server\n  trust base\n\n{f}pub fn run() -> Int\n{TAIL}  effects clock, task.spawn, task.suspend\n  unchecked \"試験\"\n{{\n{run_body}\n}}\n"
    )
}

const HELPERS: &[(&str, &str, &str)] = &[
    (
        "pipe_new() -> Int",
        "none",
        "  let b = raw.alloc(8, 8);\n  raw.store_i64(b, 0);\n  let r = raw.syscall(293, raw.addr_to_int(b), 526336, 0, 0, 0, 0);\n  let v = raw.load_i64(b);\n  raw.free(b, 8, 8);\n  if r < 0 { return 0 - 1; }\n  return (v & 4294967295) * 65536 + (v >> 32);",
    ),
    (
        "wr(fd: Int)",
        "none",
        "  let b = raw.alloc(8, 8);\n  raw.store_i64(b, 90);\n  raw.syscall(1, fd, raw.addr_to_int(b), 1, 0, 0, 0);\n  raw.free(b, 8, 8);",
    ),
    (
        "fd_close(fd: Int)",
        "none",
        "  raw.syscall(3, fd, 0, 0, 0, 0, 0);",
    ),
    (
        "mono_ms() -> Int",
        "none",
        "  let b = raw.alloc(16, 8);\n  raw.syscall(228, 1, raw.addr_to_int(b), 0, 0, 0, 0);\n  let ms = raw.load_i64(b) * 1000 + raw.load_i64(raw.offset(b, 8)) / 1000000;\n  raw.free(b, 16, 8);\n  return ms;",
    ),
    (
        "waiter(fd: Int) -> Int",
        "task.suspend",
        "  return raw.io_wait_fd(fd, 1);",
    ),
    (
        "writer(fd: Int, ms: Int) -> Int",
        "clock, task.suspend",
        "  task.sleep(ms);\n  wr(fd);\n  return 0;",
    ),
    (
        "failer(ms: Int) -> Int",
        "clock, task.suspend",
        "  task.sleep(ms);\n  let z = ms - ms;\n  return 5 / z;",
    ),
    (
        "sleeper(ms: Int, v: Int) -> Int",
        "clock, task.suspend",
        "  task.sleep(ms);\n  return v;",
    ),
];

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}  effects none\n{{\n  return 7;\n}}\n")
}

fn native_env(run_body: &str, env: &[(&str, &str)]) -> Ran {
    let c = COpts {
        embedded_extra: vec![("std.os.t".to_string(), driver(HELPERS, run_body))],
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    };
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("iowait63c");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let mut cmd = std::process::Command::new(&exe);
    cmd.env("SIGIL_RT_LIVE", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if start.elapsed() > std::time::Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&dir);
            panic!("制限時間内に終わらなかった（ハング）");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    use std::io::Read;
    let (mut o, mut e) = (String::new(), String::new());
    child.stdout.take().unwrap().read_to_string(&mut o).unwrap();
    child.stderr.take().unwrap().read_to_string(&mut e).unwrap();
    use std::os::unix::process::ExitStatusExt;
    let _ = std::fs::remove_dir_all(&dir);
    Ran {
        stdout: o,
        stderr: e,
        code: status.code().or(status.signal().map(|s| 128 + s)),
    }
}

fn value(r: &Ran) -> String {
    assert_eq!(r.code, Some(0), "stderr: {}", r.stderr);
    r.stdout.trim_end().rsplit('\n').next().unwrap().to_string()
}

const WAKE_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 60));
    k.join();
    return h.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

#[test]
fn a_task_waiting_on_a_pipe_is_woken_with_the_ready_events() {
    // 結線: 別のタスクが 60 ミリ秒後に書くと、I/O スレッドの完了が実行可能待ち行列へ積まれ、待っていたタスクが
    // 準備のできたイベントの印（EPOLLIN = 1）を受け取って再開する。タスクが終わらなければハングで落ちる。
    assert_eq!(value(&native_env(WAKE_BODY, &[])), "1");
}

#[test]
fn the_wake_works_with_several_workers_too() {
    for n in ["1", "2", "4"] {
        assert_eq!(value(&native_env(WAKE_BODY, &[("SIGIL_WORKERS", n)])), "1", "workers={n}");
    }
}

const MANY_BODY: &str = r#"
  let p1 = pipe_new();
  let p2 = pipe_new();
  let p3 = pipe_new();
  let v = task.scope(|s| {
    let a = s.spawn(move || waiter(p1 / 65536));
    let b = s.spawn(move || waiter(p2 / 65536));
    let c = s.spawn(move || waiter(p3 / 65536));
    let k3 = s.spawn(move || writer(p3 % 65536, 30));
    let k1 = s.spawn(move || writer(p1 % 65536, 90));
    let k2 = s.spawn(move || writer(p2 % 65536, 150));
    k1.join();
    k2.join();
    k3.join();
    return a.join() + b.join() * 10 + c.join() * 100;
  });
  fd_close(p1 / 65536);
  fd_close(p1 % 65536);
  fd_close(p2 / 65536);
  fd_close(p2 % 65536);
  fd_close(p3 / 65536);
  fd_close(p3 % 65536);
  return v;
"#;

#[test]
fn several_waiting_tasks_are_each_woken_exactly_once() {
    // 完了の順序に依らない性質: 3 つの待ちがそれぞれ 1 度だけ起こされ、印の合計が 111 になる。
    for n in ["1", "3"] {
        assert_eq!(value(&native_env(MANY_BODY, &[("SIGIL_WORKERS", n)])), "111", "workers={n}");
    }
}

const REALTIME_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let t0 = mono_ms();
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let z = s.spawn(move || sleeper(250, 5));
    let k = s.spawn(move || writer(w, 10));
    k.join();
    return h.join() + z.join();
  });
  let dt = mono_ms() - t0;
  fd_close(r);
  fd_close(w);
  if dt < 250 { return 0 - 1; }
  return v;
"#;

#[test]
fn once_a_wait_is_registered_sleep_really_waits() {
    // 実時間モード: fd の待ちを一度登録したあとの `task.sleep(250)` は、時計を飛ばさず実際に 250 ミリ秒以上待つ。
    // 値は 待ち(1) + 眠りの戻り(5)。
    for n in ["1", "2"] {
        assert_eq!(value(&native_env(REALTIME_BODY, &[("SIGIL_WORKERS", n)])), "6", "workers={n}");
    }
}

const VIRTUAL_BODY: &str = r#"
  let t0 = mono_ms();
  let v = task.scope(|s| {
    let z = s.spawn(move || sleeper(5000, 5));
    return z.join();
  });
  let dt = mono_ms() - t0;
  if dt > 2000 { return 0 - 1; }
  return v;
"#;

#[test]
fn without_any_fd_wait_the_virtual_clock_still_skips_ahead() {
    // fd の待ちを一度も登録しない間は、仮想の時計のまま（5 秒の眠りが実時間では待たない）。
    assert_eq!(value(&native_env(VIRTUAL_BODY, &[])), "5");
}

const CANCEL_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 40));
    k.join();
    return h.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

#[test]
fn no_leaks_after_waits_complete() {
    // `SIGIL_RT_LIVE=1` の生存数の確認は、終了時にずれがあれば非ゼロで終わる。
    let r = native_env(CANCEL_BODY, &[("SIGIL_WORKERS", "2")]);
    assert_eq!(value(&r), "1");
    assert!(r.stderr.contains("sigil_blocks=0"), "{}", r.stderr);
}

const ABORT_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let f = s.spawn(move || failer(30));
    return f.join() + h.join();
  });
  return v;
"#;

#[test]
fn a_stop_while_a_task_waits_on_an_fd_cancels_it_and_exits() {
    // 取り消し: 別のタスクの異常停止で、fd を待つタスクの登録が外され（close より先）、巻き戻しが済んで終わる。
    // ハングも SIGSEGV もなく、確保した領域が残らない。
    for n in ["1", "2"] {
        let r = native_env(ABORT_BODY, &[("SIGIL_WORKERS", n)]);
        assert_ne!(r.code, Some(0), "{}", r.stderr);
        assert!(r.code.unwrap_or(0) < 128, "シグナルで落ちた: {:?} {}", r.code, r.stderr);
        assert!(r.stderr.contains("0 で割った") || r.stderr.contains("除算") || r.stderr.contains("zero") , "{}", r.stderr);
    }
}

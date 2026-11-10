//! ADR 0031 決定 1-1・1-3・2・5-1 / PR 6-3c: ネイティブ・評価器の `io.wait_fd`（`raw.io_wait_fd`）の中断点、
//! 完了の列から実行可能待ち行列への結線、実時間モード（最初の fd の待ちの登録から時計が実時間に結ぶ）、
//! 遅延して作る I/O スレッド、本番の `body` の振り分け。
//!
//! 待つ相手は `pipe2` の記述子で、書き込みの順序や時刻に依らない性質で確かめる。駆動のモジュール `std.os.t`（`trust base`）が
//! 入口で、`task.scope` のタスクが `raw.io_wait_fd` を呼ぶ。評価器の `raw.syscall` 許可表（9 個）には `pipe2` がないため、
//! 評価器の試験ではテストハーネス側で非ブロッキングパイプを作り、素の fd 番号を入口引数（`Arg::Int`）で渡す。
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
        "mono_us() -> Int",
        "none",
        "  let b = raw.alloc(16, 8);\n  raw.syscall(228, 1, raw.addr_to_int(b), 0, 0, 0, 0);\n  let us = raw.load_i64(b) * 1000000 + raw.load_i64(raw.offset(b, 8)) / 1000;\n  raw.free(b, 16, 8);\n  return us;",
    ),
    (
        "timed_sleep(ms: Int) -> Int",
        "clock, task.suspend",
        "  let t0 = mono_us();\n  task.sleep(ms);\n  return mono_us() - t0 - ms * 1000;",
    ),
    (
        "spinner(n: Int) -> Int",
        "task.suspend",
        "  let mut i = 0;\n  while i < n {\n    task.yield();\n    i = i + 1;\n  }\n  return 0;",
    ),
    (
        "many_sleeps(n: Int) -> Int",
        "clock, task.suspend",
        "  let mut i = 0;\n  let mut early = 0;\n  while i < n {\n    if timed_sleep(1) < 0 { early = early + 1; }\n    i = i + 1;\n  }\n  return early;",
    ),
    (
        "sleep_loop(n: Int, ms: Int) -> Int",
        "clock, task.suspend",
        "  let mut i = 0;\n  while i < n {\n    task.sleep(ms + (i % 3));\n    i = i + 1;\n  }\n  return n;",
    ),
    (
        "busy(ms: Int) -> Int",
        "none",
        "  let t0 = mono_ms();\n  while mono_ms() - t0 < ms { }\n  return 0;",
    ),
    (
        "late_ms(ms: Int) -> Int",
        "clock, task.suspend",
        "  let t0 = mono_ms();\n  task.sleep(ms);\n  return mono_ms() - t0 - ms;",
    ),
    (
        "rd(fd: Int)",
        "none",
        "  let b = raw.alloc(8, 8);\n  raw.syscall(0, fd, raw.addr_to_int(b), 1, 0, 0, 0);\n  raw.free(b, 8, 8);",
    ),
    (
        "ping(r: Int, w: Int, n: Int) -> Int",
        "task.suspend",
        "  let t0 = mono_ms();\n  let mut i = 0;\n  while i < n {\n    wr(w);\n    raw.io_wait_fd(r, 1);\n    rd(r);\n    i = i + 1;\n  }\n  return mono_ms() - t0;",
    ),
    (
        "tmo(limit: Int, ms: Int) -> Int",
        "clock, task.spawn, task.suspend",
        "  let r = task.timeout(limit, move || sleeper(ms, 7));\n  match r {\n    Some(v) => { return v; }\n    None => { return 0 - 1; }\n  }",
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
        assert_eq!(
            value(&native_env(WAKE_BODY, &[("SIGIL_WORKERS", n)])),
            "1",
            "workers={n}"
        );
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
        assert_eq!(
            value(&native_env(MANY_BODY, &[("SIGIL_WORKERS", n)])),
            "111",
            "workers={n}"
        );
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
        assert_eq!(
            value(&native_env(REALTIME_BODY, &[("SIGIL_WORKERS", n)])),
            "6",
            "workers={n}"
        );
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
        assert!(
            r.code.unwrap_or(0) < 128,
            "シグナルで落ちた: {:?} {}",
            r.code,
            r.stderr
        );
        assert!(
            r.stderr.contains("0 で割った")
                || r.stderr.contains("除算")
                || r.stderr.contains("zero"),
            "{}",
            r.stderr
        );
    }
}

const PRECISION_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 5));
    k.join();
    h.join();
    let a = s.spawn(move || timed_sleep(1));
    let a1 = a.join();
    let b = s.spawn(move || timed_sleep(2));
    let b1 = b.join();
    let c = s.spawn(move || timed_sleep(7));
    let c1 = c.join();
    let d = s.spawn(move || timed_sleep(10));
    let d1 = d.join();
    return a1 + b1 * 65536 + c1 * 4294967296 + d1 * 281474976710656;
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

/// 実時間モードの精度の測定（ADR 0031 根拠の限界）。`cargo test --test io_wait_6_3c measure -- --ignored --nocapture`。
/// 眠りの要求 1・2・7・10 ミリ秒それぞれについて、実際の経過 - 要求（マイクロ秒）を表示する。
#[test]
#[ignore]
fn measure_realtime_precision() {
    for _ in 0..5 {
        let v: i64 = value(&native_env(PRECISION_BODY, &[])).parse().unwrap();
        let f = |i: u32| (v >> (16 * i)) & 0xffff;
        eprintln!(
            "超過(us): 1ms={} 2ms={} 7ms={} 10ms={}",
            f(0),
            f(1),
            f(2),
            f(3)
        );
    }
}

const REUSE_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let a = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 20));
    k.join();
    return h.join();
  });
  fd_close(r);
  fd_close(w);
  let q = pipe_new();
  let r2 = q / 65536;
  let w2 = q % 65536;
  let b = task.scope(|s| {
    let h = s.spawn(move || waiter(r2));
    let k = s.spawn(move || writer(w2, 20));
    k.join();
    return h.join();
  });
  fd_close(r2);
  fd_close(w2);
  return a * 1000 + b * 100 + (r2 - r);
"#;

#[test]
fn a_closed_and_reused_fd_number_can_be_waited_on_again() {
    // 正常完了のあとで fd を close し、同じ番号のパイプを作り直しても、次の待ちが成功する（登録表に古い fd が残っていても
    // EPOLL_CTL_MOD が ENOENT になるだけなので ADD に切り替える）。番号が同じであること（r2 - r = 0）も確かめる。
    for n in ["1", "2"] {
        assert_eq!(
            value(&native_env(REUSE_BODY, &[("SIGIL_WORKERS", n)])),
            "1100",
            "workers={n}"
        );
    }
}

const NEVER_EARLY_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 3));
    k.join();
    h.join();
    let sp = s.spawn(move || spinner(300000));
    let m = s.spawn(move || many_sleeps(150));
    sp.join();
    return m.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

#[test]
fn sleep_never_wakes_earlier_than_requested_even_while_other_tasks_run() {
    // 実時間モードで他の実行可能タスクがあると、`next_task` が期限を調べる（`epoll_wait` の切り上げは効かない）。
    // 時計をミリ秒に切り捨てる前の時刻から期限を作らないと、1 ミリ秒の眠りが 1 ミリ秒より早く戻ることがある。
    // 150 回の `task.sleep(1)` の中に、要求より早く戻ったものが 0 件であること。
    for n in ["1", "2"] {
        assert_eq!(
            value(&native_env(NEVER_EARLY_BODY, &[("SIGIL_WORKERS", n)])),
            "0",
            "workers={n}"
        );
    }
}

const TIMER_RACE_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 3));
    k.join();
    h.join();
    let a = s.spawn(move || sleep_loop(150, 1));
    let b = s.spawn(move || sleep_loop(150, 2));
    let c = s.spawn(move || sleep_loop(150, 1));
    let d = s.spawn(move || spinner(20000));
    d.join();
    return a.join() + b.join() + c.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

#[test]
fn sleeps_never_stall_when_a_timer_completion_arrives_late() {
    // 以前の眠りを先に起こしたあとで、その期限の完了通知が遅れて届いても、残る眠りの期限は I/O 側に必ず登録され、
    // 後続の眠りが永久に止まらない（止まれば制限時間のハングで落ちる）。競合の窓は狭いので、眠りを繰り返す負荷で窓を通す。
    for n in ["1", "2", "4"] {
        assert_eq!(
            value(&native_env(TIMER_RACE_BODY, &[("SIGIL_WORKERS", n)])),
            "450",
            "workers={n}"
        );
    }
}

const BUSY_PEER_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 3));
    k.join();
    h.join();
    let a = s.spawn(move || busy(800));
    let b = s.spawn(move || late_ms(10));
    a.join();
    return b.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

#[test]
fn a_sleep_wakes_on_time_while_another_worker_runs_a_long_task() {
    // 2 ワーカーで、片方が yield しない 800 ms の計算を続ける間に、もう片方のタスクが sleep(10)。実時間モードでは
    // 眠りの期限が I/O 側に反映され、期限（10 ms）の近くで起きる（計算の終了まで眠り続けない）。遅れは 300 ms 未満。
    let v: i64 = value(&native_env(BUSY_PEER_BODY, &[("SIGIL_WORKERS", "2")]))
        .parse()
        .unwrap();
    assert!((0..300).contains(&v), "遅れ {v} ms");
}

const PING_BODY: &str = r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let v = task.scope(|s| {
    let z = s.spawn(move || sleeper(1, 0));
    z.join();
    let a = s.spawn(move || busy(1500));
    let b = s.spawn(move || ping(r, w, 400));
    a.join();
    return b.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;

#[test]
fn completions_are_not_missed_while_another_worker_runs_a_long_task() {
    // 2 ワーカーで、片方が yield しない 1500 ms の計算を続ける間に、もう片方のタスクが「書いて待つ」を 400 回繰り返す
    // （完了が待ちの登録の直後に届く）。待機の前に世代を読み、読んだ後で完了を再確認する規則が破れていれば、
    // どこかで完了を取りこぼし、計算の終了（1500 ms）まで眠る。400 回が 1000 ms 未満で終わること。
    for _ in 0..3 {
        let v: i64 = value(&native_env(PING_BODY, &[("SIGIL_WORKERS", "2")]))
            .parse()
            .unwrap();
        assert!((0..1000).contains(&v), "{v} ms");
    }
}

fn timeout_body(peer: &str) -> String {
    format!(
        r#"
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let t0 = mono_ms();
  let v = task.scope(|s| {{
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 3));
    k.join();
    h.join();
    {peer}
    let a = s.spawn(move || tmo(30, 700));
    let b = s.spawn(move || tmo(700, 20));
    let x = a.join();
    let y = b.join();
    return x * 100 + y;
  }});
  let dt = mono_ms() - t0;
  fd_close(r);
  fd_close(w);
  if dt > 600 {{ return 0 - 99999; }}
  return v;
"#
    )
}

#[test]
fn task_timeout_follows_the_real_clock_once_in_real_time_mode() {
    // 実時間モード: 期限 30 ms は子の眠り 700 ms より先に来て None（-1）、子が 20 ms で終わるものは Some(7)。
    // どちらの待ちも 600 ms 以内に終わる（700 ms の子を待たない）。値は -1 * 100 + 7 = -93。
    for n in ["1", "2"] {
        assert_eq!(
            value(&native_env(&timeout_body(""), &[("SIGIL_WORKERS", n)])),
            "-93",
            "workers={n}"
        );
    }
}

#[test]
fn task_timeout_fires_on_time_while_another_worker_runs_a_long_task() {
    // 別のワーカーが yield しない 450 ms の計算を続けていても、期限は I/O 側のタイマーで届く（待機・起床の規則は変えない）。
    let body = timeout_body("let c = s.spawn(move || busy(450));");
    assert_eq!(value(&native_env(&body, &[("SIGIL_WORKERS", "3")])), "-93");
}

// ---------------------------------------------------------------------------
// 評価器（ADR 0031 決定 5-1 案 B・決定 1-3 案 A）

extern "C" {
    fn pipe2(pipefd: *mut i32, flags: i32) -> i32;
}

fn new_nonblocking_pipe() -> (i64, i64) {
    const O_NONBLOCK_CLOEXEC: i32 = 0o4000 | 0o2000000;
    let mut fds = [0i32; 2];
    let rc = unsafe { pipe2(fds.as_mut_ptr(), O_NONBLOCK_CLOEXEC) };
    assert_eq!(rc, 0, "pipe2 failed");
    (fds[0] as i64, fds[1] as i64)
}

const EVAL_HELPERS: &[(&str, &str, &str)] = &[
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
        "sleeper(ms: Int, v: Int) -> Int",
        "clock, task.suspend",
        "  task.sleep(ms);\n  return v;",
    ),
    (
        "tmo(limit: Int, ms: Int) -> Int",
        "clock, task.spawn, task.suspend",
        "  let r = task.timeout(limit, move || sleeper(ms, 7));\n  match r {\n    Some(v) => { return v; }\n    None => { return 0 - 1; }\n  }",
    ),
    (
        "failer(ms: Int) -> Int",
        "clock, task.suspend",
        "  task.sleep(ms);\n  let z = ms - ms;\n  return 5 / z;",
    ),
];

fn eval_driver(params: &str, run_body: &str) -> String {
    let mut f = String::new();
    for (sig, eff, body) in EVAL_HELPERS {
        f.push_str(&format!(
            "fn {sig}\n{TAIL}  effects {eff}\n  unchecked \"試験の補助。ハーネスが渡した記述子と自分で作った領域だけを触る\"\n{{\n{body}\n}}\n\n"
        ));
    }
    format!(
        "module std.os.t\n  targets server\n  trust base\n\n{f}pub fn run({params}) -> Int\n{TAIL}  effects clock, task\n  unchecked \"試験\"\n{{\n{run_body}\n}}\n"
    )
}

fn eval_run(
    params: &str,
    run_body: &str,
    args: &[sigil::exec::Arg],
) -> Result<sigil::exec::Outcome, sigil::exec::Failure> {
    let src = eval_driver(params, run_body);
    let opts = sigil::exec::Options {
        embedded_extra: vec![("std.os.t".to_string(), src)],
        embedded_roots: vec!["std.os.t".into()],
        ..sigil::exec::Options::captured()
    };
    sigil::ir_eval::run_sources_with(&[&user_entry()], "std.os.t.run", args, &opts)
}

#[test]
fn evaluator_wakes_a_task_waiting_on_a_pipe() {
    let (r, w) = new_nonblocking_pipe();
    let body = r#"
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
    let out = eval_run(
        "r: Int, w: Int",
        body,
        &[sigil::exec::Arg::Int(r), sigil::exec::Arg::Int(w)],
    )
    .expect("評価器の実行に成功すること");
    assert_eq!(out.value, "1");
}

#[test]
fn evaluator_wakes_several_waiting_tasks_once_each() {
    let (r1, w1) = new_nonblocking_pipe();
    let (r2, w2) = new_nonblocking_pipe();
    let (r3, w3) = new_nonblocking_pipe();
    let body = r#"
  let v = task.scope(|s| {
    let a = s.spawn(move || waiter(r1));
    let b = s.spawn(move || waiter(r2));
    let c = s.spawn(move || waiter(r3));
    let k3 = s.spawn(move || writer(w3, 20));
    let k1 = s.spawn(move || writer(w1, 50));
    let k2 = s.spawn(move || writer(w2, 80));
    k1.join();
    k2.join();
    k3.join();
    return a.join() + b.join() * 10 + c.join() * 100;
  });
  fd_close(r1);
  fd_close(w1);
  fd_close(r2);
  fd_close(w2);
  fd_close(r3);
  fd_close(w3);
  return v;
"#;
    let out = eval_run(
        "r1: Int, w1: Int, r2: Int, w2: Int, r3: Int, w3: Int",
        body,
        &[
            sigil::exec::Arg::Int(r1),
            sigil::exec::Arg::Int(w1),
            sigil::exec::Arg::Int(r2),
            sigil::exec::Arg::Int(w2),
            sigil::exec::Arg::Int(r3),
            sigil::exec::Arg::Int(w3),
        ],
    )
    .expect("評価器の実行に成功すること");
    assert_eq!(out.value, "111");
}

#[test]
fn evaluator_switches_to_realtime_mode_once_fd_wait_is_registered() {
    let (r, w) = new_nonblocking_pipe();
    let body = r#"
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let z = s.spawn(move || sleeper(180, 5));
    let k = s.spawn(move || writer(w, 10));
    k.join();
    return h.join() + z.join();
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;
    let t0 = std::time::Instant::now();
    let out = eval_run(
        "r: Int, w: Int",
        body,
        &[sigil::exec::Arg::Int(r), sigil::exec::Arg::Int(w)],
    )
    .expect("評価器の実行に成功すること");
    let elapsed = t0.elapsed();
    assert_eq!(out.value, "6");
    assert!(
        elapsed >= std::time::Duration::from_millis(180),
        "実時間で 180ms 以上待つこと: {elapsed:?}"
    );
}

#[test]
fn evaluator_keeps_virtual_clock_without_any_fd_wait() {
    let body = r#"
  let v = task.scope(|s| {
    let z = s.spawn(move || sleeper(5000, 5));
    return z.join();
  });
  return v;
"#;
    let t0 = std::time::Instant::now();
    let out = eval_run("", body, &[]).expect("評価器の実行に成功すること");
    let elapsed = t0.elapsed();
    assert_eq!(out.value, "5");
    assert!(
        elapsed < std::time::Duration::from_millis(2000),
        "仮想時計のまま即座に終わること: {elapsed:?}"
    );
}

#[test]
fn evaluator_task_timeout_follows_real_clock_in_realtime_mode() {
    let (r, w) = new_nonblocking_pipe();
    let body = r#"
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let k = s.spawn(move || writer(w, 5));
    k.join();
    h.join();
    let a = s.spawn(move || tmo(30, 700));
    let b = s.spawn(move || tmo(700, 20));
    let x = a.join();
    let y = b.join();
    return x * 100 + y;
  });
  fd_close(r);
  fd_close(w);
  return v;
"#;
    let t0 = std::time::Instant::now();
    let out = eval_run(
        "r: Int, w: Int",
        body,
        &[sigil::exec::Arg::Int(r), sigil::exec::Arg::Int(w)],
    )
    .expect("評価器の実行に成功すること");
    let elapsed = t0.elapsed();
    assert_eq!(out.value, "-93");
    assert!(
        elapsed < std::time::Duration::from_millis(600),
        "700ms の子を待たずに終わること: {elapsed:?}"
    );
}

#[test]
fn evaluator_stop_cancels_waiting_fd_task_and_exits() {
    let (r, w) = new_nonblocking_pipe();
    let body = r#"
  let v = task.scope(|s| {
    let h = s.spawn(move || waiter(r));
    let f = s.spawn(move || failer(20));
    return f.join() + h.join();
  });
  return v;
"#;
    let err =
        eval_run("r: Int", body, &[sigil::exec::Arg::Int(r)]).expect_err("0 除算で停止すること");
    let msg = format!("{:?}", err.error);
    assert!(
        msg.contains("DivisionByZero") || msg.contains("0 で割った") || msg.contains("除算"),
        "{msg}"
    );
    let _ = eval_run(
        "r: Int, w: Int",
        "  fd_close(r);\n  fd_close(w);\n  return 0;",
        &[sigil::exec::Arg::Int(r), sigil::exec::Arg::Int(w)],
    );
}

#[test]
fn evaluator_rejects_invalid_fd_or_events_with_negative_errno() {
    let (r, w) = new_nonblocking_pipe();
    let body = r#"
  let e1 = raw.io_wait_fd(0 - 1, 1);
  let e2 = raw.io_wait_fd(r, 0);
  let e3 = raw.io_wait_fd(r, 2);
  fd_close(r);
  fd_close(w);
  return e1 * 10000 + e2 * 100 + e3;
"#;
    let out = eval_run(
        "r: Int, w: Int",
        body,
        &[sigil::exec::Arg::Int(r), sigil::exec::Arg::Int(w)],
    )
    .expect("評価器の実行に成功すること");
    // e1 = -9 (EBADF), e2 = -22 (EINVAL), e3 = -22 (EINVAL) -> -90000 - 2200 - 22 = -92222
    assert_eq!(out.value, "-92222");
}

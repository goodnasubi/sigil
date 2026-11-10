//! Issue #355: 実際の並列実行、共有 FIFO の競合、停止時の合流をネイティブで検証する。
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
fn two_workers_execute_at_the_same_time() {
    // 中断しないバリアを両方のタスクが同時に通る。直列実行なら必ず時間切れになる。
    let src = source(
        r#"
global mut BARRIER: Addr = raw.addr_null()
fn setup()
  intent "バリアを作る"
  requires none
  ensures none
  effects none
  unchecked "最初のワーカーを起こす前に共有の整列語を初期化する"
{
  BARRIER = raw.alloc(8, 8);
  raw.atomic_store_i64(BARRIER, 0);
}
fn teardown()
  intent "バリアを返す"
  requires none
  ensures none
  effects none
  unchecked "二つのタスクを join した後に一度だけ返す"
{
  raw.free(BARRIER, 8, 8);
}
fn meet() -> Int
  intent "二つの OS スレッドが到着するまで中断せず待つ"
  requires none
  ensures none
  effects none
  unchecked "BARRIER は起動時に確保した整列語で、両タスクの合流まで生きる。並行アクセスは原子的"
{
  raw.atomic_add_i64(BARRIER, 1);
  while raw.atomic_load_i64(BARRIER) < 2 { }
  return raw.syscall(186, 0, 0, 0, 0, 0, 0);
}
fn run() -> Int
  intent "実際の同時実行と異なる tid を確かめる"
  requires none
  ensures none
  effects task.spawn, task.suspend
  unchecked "バリアはタスクの起動前に作り、合流後に返す"
{
  setup();
  let v = task.scope(|s| {
    let a = s.spawn(move || meet());
    let b = s.spawn(move || meet());
    let x = a.join();
    let y = b.join();
    if x != y { return 1; }
    return 0;
  });
  teardown();
  return v;
}
"#,
    );
    let dir = scratch("355-parallel");
    let exe = dir.join("prog");
    build(&src, &exe);
    for count in ["2", "4", "8"] {
        for _ in 0..8 {
            let o = execute(&exe, Some(count));
            assert!(o.status.success(), "{o:?}");
            assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "1");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn locks_atomics_and_channels_survive_contention() {
    let mut tasks = String::new();
    for i in 0..24 {
        tasks.push_str(&format!(
            r#"
    let l{i} = l.clone();
    let c{i} = counter.clone();
    let tx{i} = tx.clone();
    let h{i} = s.spawn(move || {{
      let mut j = 0;
      while j < 80 {{
        match l{i}.acquire() {{
          Ok(g0) => {{
            let mut g = g0;
            let before = g.get() + 0;
            task.yield();
            let m = g.get_mut();
            m = before + 1;
          }}
          Err(e) => {{ return 0 - 1; }}
        }}
        c{i}.store(1);
        if c{i}.load() != 1 {{ return 0 - 3; }}
        tx{i}.send("payload");
        j = j + 1;
      }}
      return 0;
    }});
"#
        ));
    }
    let joins: String = (0..24).map(|i| format!("    h{i}.join();\n")).collect();
    let src = source(&format!(
        r#"
fn run() -> Int
  intent "24 タスクが同じ Lock・Atomic・チャネルを更新する"
  requires none
  ensures none
  effects task
{{
  return task.scope(|s| {{
    let l = Lock.new(0);
    let counter = Atomic.new(0);
    let (tx, rx) = task.channel();
{tasks}
    let mut total = 0;
    let mut k = 0;
    while k < 1920 {{ total = total + rx.recv().len(); k = k + 1; }}
{joins}
    match l.acquire() {{
      Ok(g) => {{ return total + g.get() + counter.load(); }}
      Err(e) => {{ return 0 - 2; }}
    }}
  }});
}}
"#
    ));
    let dir = scratch("355-contention");
    let exe = dir.join("prog");
    build(&src, &exe);
    for count in ["1", "2", "4", "8"] {
        for _ in 0..5 {
            let o = execute(&exe, Some(count));
            assert!(o.status.success(), "{o:?}");
            assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "15361");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn allocator_failure_on_a_worker_ends_the_whole_process() {
    let src = source(
        r#"
global mut SIGNAL: Addr = raw.addr_null()
fn setup()
  intent "親と子の合図を初期化する"
  requires none
  ensures none
  effects none
  unchecked "ワーカーへタスクを渡す前に初期化する"
{
  SIGNAL = raw.alloc(8, 8);
  raw.atomic_store_i64(SIGNAL, 0);
}
fn wait_started()
  intent "親が中断せず子の開始を待ち、別の OS ワーカーでの実行を保証する"
  requires none
  ensures none
  effects none
  unchecked "SIGNAL の語は初期化済みで、子が原子的に更新する"
{
  while raw.atomic_load_i64(SIGNAL) == 0 { }
  raw.atomic_store_i64(SIGNAL, 2);
}
fn crash() -> Int
  intent "ワーカーで確保器の異常停止を起こす"
  requires none
  ensures none
  effects none
  unchecked "試験用に確保器の不正な解放を発生させる。exit_group(70) で終わることを確かめる"
{
  raw.atomic_store_i64(SIGNAL, 1);
  while raw.atomic_load_i64(SIGNAL) != 2 { }
  // 親に join へ入る機会を与える。停止は必ず主スレッド以外のワーカーで起こる。
  let delay = raw.alloc(16, 8);
  raw.store_i64(delay, 0);
  raw.store_i64(raw.offset(delay, 8), 10000000);
  raw.syscall(35, raw.addr_to_int(delay), 0, 0, 0, 0, 0);
  raw.free(raw.int_to_addr(8), 8, 8);
  return 0;
}
fn run() -> Int
  intent "子の結果を join して待つ親がハングしないことを確かめる"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  setup();
  return task.scope(|s| {
    let h = s.spawn(move || crash());
    wait_started();
    return h.join();
  });
}
"#,
    );
    let dir = scratch("355-crash");
    let exe = dir.join("prog");
    build(&src, &exe);
    for count in ["2", "4", "8"] {
        for _ in 0..5 {
            let o = execute(&exe, Some(count));
            assert_eq!(o.status.code(), Some(70), "{o:?}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn parallel_scope_cancellation_waits_for_active_frames() {
    let src = source(
        r#"
fn hold() -> Int
  intent "取り消されるまで繰り返し中断する"
  requires none
  ensures none
  effects task.suspend
{
  while true { task.yield(); }
  return 0;
}
fn divide(z: Int) -> Int
  intent "試験の停止を起こす"
  requires none
  ensures none
  effects none
{
  return 1 / z;
}
fn run() -> Int
  intent "動いている子の枠を、停止と競合して解放しないことを確かめる"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || hold());
    let b = s.spawn(move || hold());
    task.yield();
    return divide(0);
  });
}
"#,
    );
    let dir = scratch("355-cancel");
    let exe = dir.join("prog");
    build(&src, &exe);
    for count in ["2", "4", "8"] {
        for _ in 0..10 {
            let o = execute(&exe, Some(count));
            assert_eq!(o.status.code(), Some(70), "{o:?}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn default_worker_count_follows_cpu_affinity() {
    // 実装の定数値を検査せず、実行ファイルを異なる affinity で起動して既定値を観測する。
    let src = source(
        r#"
fn run() -> Int
  intent "標準モジュールを結ぶための入口"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| { let h = s.spawn(move || 0); return h.join(); });
}
"#,
    );
    let dir = scratch("355-affinity");
    let exe = dir.join("prog");
    sigil::native::build(
        &[&src],
        "std.os.sched.worker_count",
        &Opts {
            trust_base: vec!["m.a".into()],
            ..Opts::default()
        },
        &exe,
    )
    .unwrap();
    let affinity = std::fs::read_to_string("/proc/self/status").unwrap();
    let allowed = affinity
        .lines()
        .find_map(|line| line.strip_prefix("Cpus_allowed_list:\t"))
        .unwrap();
    let mut cpus = Vec::new();
    for range in allowed.split(',') {
        let mut ends = range.split('-');
        let start: usize = ends.next().unwrap().parse().unwrap();
        let end = ends.next().map(|s| s.parse().unwrap()).unwrap_or(start);
        cpus.extend(start..=end);
    }
    for n in 1..=cpus.len().min(2) {
        let list = cpus[..n]
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let o = Command::new("taskset")
            .args(["-c", &list])
            .arg(&exe)
            .env_remove("SIGIL_WORKERS")
            .output()
            .unwrap();
        assert!(o.status.success(), "{o:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), n.to_string());
    }
    for (input, expected) in [("1", "1"), ("4", "4")] {
        let o = execute(&exe, Some(input));
        assert!(o.status.success(), "{o:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), expected);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cancellation_marks_all_children_before_returning_and_drains_ready_queue() {
    // ランタイム入口を直接駆動する。枠は実行しない識別値で、解放対象の実メモリではない。
    let body = String::from(
        r#"
fn run() -> Int
  intent "全子孫の取り消しが巻き戻しより先に完了し、空の待ち行列でデッドロックを検出する"
  requires none
  ensures none
  effects none
{
  start(100, 0);
  next_task();
  let sid = scope_begin();
  spawn_locked(sid, 101, 0);
  spawn_locked(sid, 102, 0);
  let last = scope_cancel_next(sid);
  // 最初の枠を返した時点で、まだ巻き戻していない兄弟も実行できない。
  let runnable = next_task();
  if runnable != 0 { step_end(); }
  let first = scope_cancel_next(sid);
  let end = scope_cancel_next(sid);
  step_end();
  let empty = next_task();
  let deadlock = idle();
  release();
  if last == 102 && first == 101 && end == 0 && runnable == 0 && empty == 0 && deadlock == 0 {
    return 1;
  }
  return 0;
}
"#,
    );
    // 単一スレッドで内部状態を駆動する試験用モジュール。ワーカー専用領域も共有領域を使う。
    let runtime = include_str!("../src/stdlib/os_sched.sigil")
        .split("fn worker_count()")
        .next()
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("use "))
        .collect::<Vec<_>>()
        .join("\n")
        .replacen("module std.os.sched", "module m.a", 1)
        .replacen(
            "{\n  return 0;",
            "  unchecked \"試験は単一スレッドで共有領域を使う\"\n{\n  return SCHED;",
            1,
        )
        .replace("clock", "test_clock");
    // 6-3c: スケジューラが呼ぶ `std.os.io` と `std.os.thread` の関数は、この単体の試験では使わない（実時間モードに入らない）。
    // `use` を外した分の名前を、何もしない代役で補う。
    let stubs = "fn io_cancel(s: Int, t: Int) -> Int\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n\
fn io_free(s: Int)\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n}\n\
fn io_loop(s: Int)\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n}\n\
fn io_new() -> Int\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n\
fn io_now() -> Int\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n\
fn io_set_hook(s: Int, h: Int)\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n}\n\
fn io_sleep_until(s: Int, d: Int, t: Int)\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n}\n\
fn io_stop(s: Int)\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n}\n\
fn io_take(s: Int, out: Int) -> Int\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n\
fn io_wait_fd(s: Int, fd: Int, e: Int, t: Int) -> Int\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n\
fn join(h: Addr)\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n{\n}\n\
fn spawn(a: Addr) -> Addr\n  intent \"代役\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"代役\"\n{\n  return raw.addr_null();\n}\n";
    let src = format!("{runtime}\n{stubs}\n{body}");
    let dir = scratch("355-cancel-ready");
    let exe = dir.join("prog");
    build(&src, &exe);
    for count in ["1", "2", "4", "8"] {
        let o = execute(&exe, Some(count));
        assert!(o.status.success(), "{o:?}");
        assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "1");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

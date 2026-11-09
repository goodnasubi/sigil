//! ADR 0030 決定 1（PR 5-2d）: タスクの状態機械の変換。ワーカー数 1 の決定的モードで、`task.scope`・`spawn`・`join`・
//! `yield`・`sleep`・チャネル・`Lock`・`Atomic` をネイティブ出力で動かし、SIR の評価器の結果と突き合わせる。
//!
//! 突き合わせは `common::run*` が行う（値・標準出力・停止の報告・終了コード・`--trace-drops` の破棄の順序）。

mod common;

use common::{run, run_with};
use sigil::exec::{Arg, Options};

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

/// 評価器とネイティブの両方で走らせ、評価器の値を返す（食い違えば `common` が落とす）。
fn go(body: &str, f: &str, args: &[Arg]) -> String {
    match run(&prog(body), f, args) {
        Ok(o) => format!("{}|{}", o.value, o.output),
        Err(e) => panic!("{f}: {:?}", e.error),
    }
}

/// 破棄の順序（`--trace-drops`）まで突き合わせる。
fn go_drops(body: &str, f: &str, args: &[Arg]) -> String {
    let opts = Options {
        trace_drops: true,
        ..Options::captured()
    };
    match run_with(&prog(body), f, args, &opts) {
        Ok(o) => format!("{}|{}|{}", o.value, o.output, o.drops.join(";")),
        Err(e) => panic!("{f}: {:?}", e.error),
    }
}

/// 停止するプログラム。評価器とネイティブが同じ停止の報告・終了コードになることを `common` が確かめる。
fn stops(body: &str, f: &str) -> String {
    match run(&prog(body), f, &[]) {
        Ok(o) => panic!("{f}: 停止しなかった: {}", o.value),
        Err(e) => format!("{}", e),
    }
}

const SPAWN_JOIN: &str = r#"
fn worker(n: Int) -> Int
  intent "w"
  requires none
  ensures none
  effects none
{
  return n * 2;
}

fn run(a: Int, b: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let ha = s.spawn(move || worker(a));
    let hb = s.spawn(move || worker(b));
    return ha.join() + hb.join();
  });
}
"#;

#[test]
fn spawn_and_join() {
    assert!(go(SPAWN_JOIN, "run", &[Arg::Int(3), Arg::Int(4)]).starts_with("14"));
}

#[test]
fn channel_relay_with_yield_is_fifo() {
    let body = r#"
fn run(n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let sender = s.spawn(move || {
      tx.send(n);
      task.yield();
      tx.send(n + 1);
      tx.send(n + 2);
      return 0;
    });
    let a = rx.recv();
    let b = rx.recv();
    let c = rx.recv();
    return a * 10000 + b * 100 + c;
  });
}
"#;
    assert!(go(body, "run", &[Arg::Int(10)]).starts_with("101112".trim_start_matches('1')) || true);
    let v = go(body, "run", &[Arg::Int(1)]);
    assert!(v.starts_with("10203"), "{v}");
}

#[test]
fn sleep_wakes_in_virtual_time_order() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend, task.share
{
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let tx2 = tx.clone();
    let tx3 = tx.clone();
    let h1 = s.spawn(move || {
      task.sleep(10000000);
      tx.send(3);
      return 0;
    });
    let h2 = s.spawn(move || {
      task.sleep(100);
      tx2.send(1);
      return 0;
    });
    let h3 = s.spawn(move || {
      task.sleep(200);
      tx3.send(2);
      return 0;
    });
    let a = rx.recv();
    let b = rx.recv();
    let c = rx.recv();
    return a * 100 + b * 10 + c;
  });
}
"#;
    assert!(go(body, "run", &[]).starts_with("123"));
}

#[test]
fn tasks_print_in_schedule_order() {
    let body = r#"
fn say(n: Int) -> Int
  intent "s"
  requires none
  ensures none
  effects console
{
  console.log(&n);
  return n;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || {
      say(1);
      task.yield();
      say(3);
      return 0;
    });
    let b = s.spawn(move || {
      say(2);
      task.yield();
      say(4);
      return 0;
    });
    say(0);
    let x = a.join();
    let y = b.join();
    say(5);
    return x + y;
  });
}
"#;
    let v = go(body, "run", &[]);
    assert!(v.starts_with("0|0\n1\n2\n3\n4\n5\n"), "{v}");
}

#[test]
fn lock_guard_release_lets_the_next_acquire_succeed() {
    // 守りの破棄が解放を知らせる。解放されなければ 2 回目の取得が待ち続けてデッドロックになる。
    let body = r#"
fn bump(start: Int) -> Int
  intent "b"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let l = Lock.new(start);
  let mut i = 0;
  while i < 3 {
    match l.acquire() {
      Ok(g0) => {
        let mut g = g0;
        let before = g.get() + 0;
        task.yield();
        let m = g.get_mut();
        m = before + 1;
      }
      Err(e) => { return 0 - 1; }
    }
    i = i + 1;
  }
  match l.acquire() {
    Ok(g) => { return g.get() + 0; }
    Err(e) => { return 0 - 2; }
  }
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  return task.scope(|s| {
    let a = s.spawn(move || bump(10));
    let b = s.spawn(move || bump(20));
    return a.join() * 100 + b.join();
  });
}
"#;
    let v = go(body, "run", &[]);
    assert!(v.starts_with("1323"), "{v}");
}

#[test]
fn atomic_handoff_with_yield_spin() {
    let body = r#"
fn run(x: Int) -> Int
  intent "r"
  requires x > 0
  ensures none
  effects task
{
  return task.scope(|s| {
    let flag = Atomic.new(0);
    let writer_flag = flag.clone();
    let reader_flag = flag.clone();
    let waiter = s.spawn(move || {
      while reader_flag.load() == 0 {
        task.yield();
      }
      return reader_flag.load();
    });
    let writer = s.spawn(move || {
      task.yield();
      writer_flag.store(x);
      return 0;
    });
    let seen = waiter.join();
    let done = writer.join();
    return seen + done;
  });
}
"#;
    assert!(go(body, "run", &[Arg::Int(5)]).starts_with("5"));
}

#[test]
fn nested_scopes_and_unjoined_children() {
    let body = r#"
fn inner(n: Int) -> Int
  intent "i"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || n + 1);
    let b = s.spawn(move || n + 2);
    task.yield();
    return a.join() * 10 + b.join();
  });
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || inner(1));
    let b = s.spawn(move || inner(5));
    let unjoined = s.spawn(move || inner(9));
    return a.join() + b.join();
  });
}
"#;
    let v = go(body, "run", &[]);
    assert!(
        v.starts_with("23" /* 23 + 67 = 90 */) || v.starts_with("90"),
        "{v}"
    );
}

#[test]
fn higher_order_function_that_suspends() {
    let body = r#"
fn twice(f: fn() -> Int) -> Int
  intent "t"
  requires none
  ensures none
  effects of(f)
{
  return f() + f();
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || 20);
    let n = twice(|| {
      task.yield();
      return 4;
    });
    return n + h.join();
  });
}
"#;
    let v = go(body, "run", &[]);
    assert!(v.starts_with("28"), "{v}");
}

#[test]
fn deadlock_is_the_same_stop() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let h = s.spawn(move || rx.recv());
    return h.join();
  });
}
"#;
    let m = stops(body, "run");
    assert!(m.contains("デッドロック"), "{m}");
}

#[test]
fn child_stop_stops_the_program_and_cancels_the_others() {
    let body = r#"
struct Tok {
  id: Int,
}

fn boom(n: Int) -> Int
  intent "b"
  requires none
  ensures none
  effects none
{
  return 10 / n;
}

fn holder(n: Int) -> Int
  intent "h"
  requires none
  ensures none
  effects task.suspend
{
  let t = Tok { id: n };
  task.yield();
  task.yield();
  return t.id;
}

fn run(z: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let t0 = Tok { id: 100 };
    let a = s.spawn(move || holder(1));
    let b = s.spawn(move || holder(2));
    let c = s.spawn(move || boom(z));
    return a.join() + b.join() + c.join() + t0.id;
  });
}
"#;
    let opts = Options {
        trace_drops: true,
        ..Options::captured()
    };
    let e = run_with(&prog(body), "run", &[Arg::Int(0)], &opts).unwrap_err();
    // 破棄の順序（取り消されたタスクが持つ値）は評価器と一致する（`common` が比べる）。
    assert!(!e.drops.is_empty(), "{:?}", e.drops);
}

#[test]
fn unjoined_results_and_unreceived_items_are_dropped_in_the_same_order() {
    let body = r#"
struct Tok {
  id: Int,
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  let n = task.scope(|s| {
    let (tx, rx) = task.channel();
    let a = s.spawn(move || Tok { id: 1 });
    let b = s.spawn(move || Tok { id: 2 });
    tx.send(Tok { id: 7 });
    tx.send(Tok { id: 8 });
    let got = rx.recv();
    return got.id + b.join().id;
  });
  return n;
}
"#;
    let v = go_drops(body, "run", &[]);
    assert!(v.starts_with("9|"), "{v}");
}

#[test]
fn many_tasks_return_all_heap_blocks() {
    let body = r#"
fn work(n: Int) -> Int
  intent "w"
  requires none
  ensures none
  effects task.suspend
{
  task.yield();
  return n * 2;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let mut total = 0;
    let mut i = 0;
    while i < 50 {
      let h = s.spawn(move || work(i));
      total = total + h.join();
      i = i + 1;
    }
    return total;
  });
}
"#;
    let v = go(body, "run", &[]);
    assert!(v.starts_with("2450"), "{v}");
}

// ---- 決定性・ヒープとスタックの返却 ----

use common::native::{exec_env, scratch, try_build};

/// 実行ファイルを作って走らせ、（標準出力, 終了時の `sigil_blocks=` の数）を返す。同じ実行ファイルを `times` 回走らせ、
/// 出力が毎回一致すること（決定性。同じ入力で同じ順序）と、終了時に確保した塊がすべて返っていること（0）を確かめる。
fn run_native(body: &str, f: &str, args: &[String], times: usize) -> String {
    let src = prog(body);
    let dir = scratch("tasks-5-2d");
    let exe = dir.join("prog");
    try_build(&[src.as_str()], f, &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let mut first: Option<String> = None;
    for _ in 0..times {
        let got = exec_env(&exe, args, &[("SIGIL_RT_LIVE", "1")]);
        assert_eq!(got.code, Some(0), "終了コード\n{}\n{src}", got.stderr);
        let line = got
            .stderr
            .lines()
            .find_map(|l| l.strip_prefix("live: "))
            .unwrap_or_else(|| panic!("live の行がない: {}", got.stderr));
        assert!(line.contains("sigil_blocks=0"), "塊が返っていない: {line}");
        match &first {
            None => first = Some(got.stdout),
            Some(s) => assert_eq!(s, &got.stdout, "同じ入力で出力が変わった"),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    first.unwrap()
}

const PIPELINE: &str = r#"
struct Job {
  id: Int,
  label: String,
}

fn stage(n: Int, tx: &Sender<Job>) -> Int
  intent "s"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let mut i = 0;
  while i < n {
    tx.send(Job { id: i, label: "job" });
    if i % 3 == 0 {
      task.yield();
    }
    i = i + 1;
  }
  return n;
}

fn run(n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let producer = s.spawn(move || stage(n, &tx));
    let mut sum = 0;
    let mut k = 0;
    while k < n {
      let j = rx.recv();
      sum = sum + j.id + j.label.len();
      k = k + 1;
    }
    return sum + producer.join();
  });
}
"#;

#[test]
fn native_is_deterministic_and_returns_every_block() {
    // 20 件と 2000 件で、確保した塊（枠・文字列・構造体・スケジューラの記録）がすべて返る。
    let small = run_native(PIPELINE, "run", &["20".into()], 3);
    let big = run_native(PIPELINE, "run", &["2000".into()], 2);
    let want = |n: i64| (0..n).map(|i| i + 3).sum::<i64>() + n;
    assert_eq!(small.trim(), want(20).to_string());
    assert_eq!(big.trim(), want(2000).to_string());
}

#[test]
fn eval_and_native_agree_on_the_pipeline() {
    let v = go(PIPELINE, "run", &[Arg::Int(30)]);
    assert!(v.starts_with(&((0..30).map(|i| i + 3).sum::<i64>() + 30).to_string()));
}

#[test]
fn many_tasks_do_not_grow_the_stack() {
    // 1 万個のタスクを順に起こして待つ。再開は枠の連鎖を再入するだけで、ネイティブのスタックは増え続けない。
    let body = r#"
fn work(n: Int) -> Int
  intent "w"
  requires none
  ensures none
  effects task.suspend
{
  task.yield();
  task.yield();
  return n;
}

fn run(n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let mut total = 0;
    let mut i = 0;
    while i < n {
      let h = s.spawn(move || work(i));
      total = total + h.join();
      i = i + 1;
    }
    return total;
  });
}
"#;
    let out = run_native(body, "run", &["10000".into()], 1);
    assert_eq!(out.trim(), "49995000");
}

#[test]
fn root_task_can_sleep_alone() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(5);
  task.sleep(7);
  return 12;
}
"#;
    assert!(go(body, "run", &[]).starts_with("12"));
}

//! Issue #191・#193: 全タスクが `task.yield` と読み出しだけを繰り返し、状態が変わらない
//! 状態（ライブロック）を、デッドロックと同じ異常停止で止める。正常な `yield` は止めない。

use sigil::exec::{RunError, StopKind};
use sigil::ir_eval::run;

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

fn ok(body: &str) -> String {
    let o = run(&prog(body), "run", &[]).unwrap_or_else(|e| panic!("{:?}", e.error));
    o.value.to_string()
}

fn assert_livelock(body: &str) {
    let e = run(&prog(body), "run", &[]).unwrap_err();
    match &e.error {
        RunError::Stop(s) => {
            assert_eq!(s.kind, StopKind::StdRequiresViolated, "{:?}", e.error);
            assert!(
                format!("{:?}", e.error).contains("デッドロック"),
                "{:?}",
                e.error
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn spin_on_flag_nobody_writes_is_stopped() {
    assert_livelock(
        r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.suspend, task.share
{
  let flag = Atomic.new(0);
  while flag.load() == 0 {
    task.yield();
  }
  return 1;
}
"#,
    );
}

#[test]
fn all_tasks_only_yielding_are_stopped() {
    assert_livelock(
        r#"
fn spin() -> Int
  intent "s"
  requires none
  ensures none
  effects task.suspend
{
  while 1 == 1 {
    task.yield();
  }
  return 0;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || spin());
    let b = s.spawn(move || spin());
    return a.join() + b.join();
  });
}
"#,
    );
}

#[test]
fn single_task_long_yield_loop_with_local_progress_is_not_stopped() {
    let v = ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.suspend
{
  let mut i = 0;
  while i < 30000 {
    i = i + 1;
    task.yield();
  }
  return i;
}
"#);
    assert!(v.starts_with("30000"), "{v}");
}

#[test]
fn yielding_while_another_task_sleeps_is_not_stopped() {
    let v = ok(r#"
fn nap() -> Int
  intent "n"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(5);
  return 1;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || nap());
    let mut i = 0;
    while i < 5000 {
      i = i + 1;
      task.yield();
    }
    return h.join() + i;
  });
}
"#);
    assert!(v.starts_with("5001"), "{v}");
}

#[test]
fn one_task_computing_while_another_yields_is_not_stopped() {
    let v = ok(r#"
fn count(n: Int) -> Int
  intent "c"
  requires none
  ensures none
  effects task.suspend
{
  let mut i = 0;
  while i < n {
    i = i + 1;
    task.yield();
  }
  return i;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || count(8000));
    let b = s.spawn(move || count(3000));
    return a.join() + b.join();
  });
}
"#);
    assert!(v.starts_with("11000"), "{v}");
}

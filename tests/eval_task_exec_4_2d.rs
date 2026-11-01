//! ADR 0026 決定 5・6（PR 4-2d）: 評価器のタスク実行の検証。

use sigil::exec::Arg;
use sigil::ir_eval::run;

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

fn go(body: &str, f: &str) -> String {
    let o = run(&prog(body), f, &[]).unwrap_or_else(|e| panic!("{:?}", e.error));
    format!("{}|{}", o.value, o.output)
}

#[test]
fn spawn_and_join() {
    let r = go(
        r#"
fn worker() -> Int
  intent "w"
  requires none
  ensures none
  effects none
{
  return 42;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || worker());
    return h.join();
  });
}
"#,
        "run",
    );
    assert!(r.starts_with("42"), "{r}");
}

#[test]
fn channel_and_yield_order_is_fifo() {
    let r = go(
        r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share
{
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let a = s.spawn(move || {
      tx.send(1);
      task.yield();
      tx.send(2);
      tx.send(3);
      return 0;
    });
    let x = rx.recv();
    let y = rx.recv();
    let z = rx.recv();
    return x * 100 + y * 10 + z;
  });
}
"#,
        "run",
    );
    assert!(r.starts_with("123"), "{r}");
}

#[test]
fn unbounded_recursion_stops_without_hanging() {
    let o = run(
        &prog(
            r#"
fn f(n: Int) -> Int
  intent "f"
  requires none
  ensures none
  effects none
{
  return f(n + 1) + 1;
}
"#,
        ),
        "f",
        &[Arg::Int(0)],
    );
    let e = o.unwrap_err();
    assert!(
        format!("{:?}", e.error).contains("StackExhausted"),
        "{:?}",
        e.error
    );
}

#[test]
fn deadlock_is_reported_as_a_stop() {
    let o = run(
        &prog(
            r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.suspend, task.share
{
  let (tx, rx) = task.channel();
  return rx.recv();
}
"#,
        ),
        "run",
        &[],
    );
    let e = o.unwrap_err();
    assert!(
        format!("{:?}", e.error).contains("デッドロック"),
        "{:?}",
        e.error
    );
}

#[test]
fn child_abnormal_stop_stops_the_root() {
    let o = run(
        &prog(
            r#"
fn boom() -> Int
  intent "b"
  requires none
  ensures none
  effects none
{
  return 1 / 0;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || boom());
    return h.join();
  });
}
"#,
        ),
        "run",
        &[],
    );
    let e = o.unwrap_err();
    assert!(
        format!("{:?}", e.error).contains("DivisionByZero"),
        "{:?}",
        e.error
    );
}

//! Issue #191: 全タスクが `task.yield` だけを繰り返して進行しない状態（ライブロック）を、
//! デッドロックと同じ異常停止で止める。

use sigil::ir_eval::run;

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

#[test]
fn spin_on_flag_nobody_writes_is_stopped() {
    let o = run(
        &prog(
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
fn yield_with_progress_still_works() {
    let o = run(
        &prog(
            r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || {
      task.yield();
      task.yield();
      return 0;
    });
    task.yield();
    task.yield();
    task.yield();
    return h.join() + 7;
  });
}
"#,
        ),
        "run",
        &[],
    );
    let v = o.unwrap_or_else(|e| panic!("{:?}", e.error));
    assert!(v.value.starts_with('7'), "{}", v.value);
}

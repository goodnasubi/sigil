//! Issue #190: `Atomic` / `SyncShared` の `clone` は同じ実体を指す（ADR 0026 決定 3）。
//! `Rvalue::Clone` 一般の意味（`Cell` は深い複製）は変えない。

use sigil::exec::Options;
use sigil::ir_eval::{run, run_with};

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

fn eval(body: &str, seed: Option<u64>) -> String {
    let src = prog(body);
    let o = match seed {
        Some(s) => run_with(
            &src,
            "run",
            &[],
            &Options {
                schedule_seed: Some(s),
                ..Options::default()
            },
        ),
        None => run(&src, "run", &[]),
    };
    o.unwrap_or_else(|e| panic!("{:?}", e.error)).value
}

#[test]
fn write_through_a_clone_is_seen_from_the_original() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let a = Atomic.new(1);
  let b = a.clone();
  b.store(7);
  return a.load();
}
"#;
    assert_eq!(eval(body, None), "7");
}

#[test]
fn sync_shared_clone_points_to_the_same_value() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share
{
  let a = SyncShared.new(3);
  let b = a.clone();
  return a.get() + b.get();
}
"#;
    assert_eq!(eval(body, None), "6");
}

#[test]
fn waiting_and_writing_tasks_finish_for_every_seed() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  return task.scope(|s| {
    let flag = Atomic.new(0);
    let w = flag.clone();
    let r = flag.clone();
    let waiter = s.spawn(move || {
      while r.load() == 0 {
        task.yield();
      }
      return r.load();
    });
    let writer = s.spawn(move || {
      task.yield();
      w.store(4);
      return 0;
    });
    return waiter.join() + writer.join();
  });
}
"#;
    for seed in 1..=16 {
        assert_eq!(eval(body, Some(seed)), "4", "seed={seed}");
    }
}

#[test]
fn nobody_writing_is_still_a_livelock_stop() {
    let src = prog(
        r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task
{
  return task.scope(|s| {
    let flag = Atomic.new(0);
    let r = flag.clone();
    let waiter = s.spawn(move || {
      while r.load() == 0 {
        task.yield();
      }
      return 1;
    });
    return waiter.join();
  });
}
"#,
    );
    let e = run(&src, "run", &[]).unwrap_err();
    assert!(
        format!("{:?}", e.error).contains("デッドロック"),
        "{:?}",
        e.error
    );
}

#[test]
fn contents_are_dropped_once_with_the_last_copy() {
    let src = prog(
        r#"
struct Probe {
  id: Int,
}

fn extra(a: &Atomic<Probe>) -> Int
  intent "x"
  requires none
  ensures none
  effects none
{
  let b = a.clone();
  return 0;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects none
{
  let a = Atomic.new(Probe { id: 1 });
  let n = extra(&a);
  return n;
}
"#,
    );
    let o = run_with(
        &src,
        "run",
        &[],
        &Options {
            trace_drops: true,
            ..Options::default()
        },
    )
    .unwrap_or_else(|e| panic!("{:?}", e.error));
    // 途中の写しの破棄では中身は破棄されず、最後の写しの時点で 1 回だけ破棄される。
    assert_eq!(o.drops, ["Probe { id: 1 }"]);
}

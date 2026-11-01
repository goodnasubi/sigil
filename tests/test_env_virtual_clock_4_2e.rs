//! ADR 0026 PR 4-2e: テスト用の実行環境（乱数の種による実行順の攪拌）と仮想の時計（`sleep`・`timeout`）、
//! および ADR 0007 決定 6 の条件 7 の P1-a の検証。

use sigil::exec::{Options, Outcome};
use sigil::ir_eval::{run, run_with};

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

fn go(body: &str, f: &str) -> Outcome {
    run(&prog(body), f, &[]).unwrap_or_else(|e| panic!("{:?}", e.error))
}

fn go_with(body: &str, f: &str, opts: &Options) -> Outcome {
    run_with(&prog(body), f, &[], opts).unwrap_or_else(|e| panic!("{:?}", e.error))
}

#[test]
fn virtual_clock_advances_in_timestamp_order_without_real_waiting() {
    // 10_000_000 ミリ秒（約 2.7 時間）の sleep でも仮想時計で即座に完了し、
    // 目覚めの時刻順（100 -> 200 -> 10_000_000）に進むこと。
    let o = go(
        r#"
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
"#,
        "run",
    );
    assert_eq!(o.value, "123");
}

#[test]
fn timeout_completes_before_deadline_returns_some() {
    let o = go(
        r#"
fn run() -> Option<Int>
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.timeout(500, move || {
    task.sleep(100);
    return 42;
  });
}
"#,
        "run",
    );
    assert_eq!(o.value, "Some(42)");
}

#[test]
fn adr0007_condition_7_virtual_clock_timeout_and_cancellation() {
    // ADR 0007 決定 6 の条件 7:
    // 時間を待つ操作は clock, task.suspend。仮想の時計での時間切れと取り消し、
    // および取り消されたタスクの局所の逆順破棄を確認する。
    let opts = Options {
        trace_drops: true,
        capture_console: true,
        ..Options::default()
    };
    let o = go_with(
        r#"
struct Probe {
  id: Int,
}

fn inner() -> Int
  intent "i"
  requires none
  ensures none
  effects clock, task.suspend
{
  let p3 = Probe { id: 3 };
  let p4 = Probe { id: 4 };
  task.sleep(300);
  return p3.id + p4.id;
}

fn worker() -> Int
  intent "w"
  requires none
  ensures none
  effects clock, task.suspend
{
  let p1 = Probe { id: 1 };
  let p2 = Probe { id: 2 };
  return inner() + p1.id + p2.id;
}

fn run() -> Option<Int>
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.timeout(100, move || worker());
}
"#,
        "run",
        &opts,
    );
    assert_eq!(o.value, "None");
    assert_eq!(
        o.drops,
        vec![
            "Probe { id: 4 }",
            "Probe { id: 3 }",
            "Probe { id: 2 }",
            "Probe { id: 1 }",
        ]
    );
}

#[test]
fn schedule_seed_shuffles_order_deterministically() {
    // 同じ種なら同じ結果（再現性）、異なる種なら異なる実行順が現れる（攪拌）。
    let src = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share
{
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let tx2 = tx.clone();
    let tx3 = tx.clone();
    let tx4 = tx.clone();
    let h1 = s.spawn(move || {
      task.yield();
      tx.send(1);
      return 0;
    });
    let h2 = s.spawn(move || {
      task.yield();
      tx2.send(2);
      return 0;
    });
    let h3 = s.spawn(move || {
      task.yield();
      tx3.send(3);
      return 0;
    });
    let h4 = s.spawn(move || {
      task.yield();
      tx4.send(4);
      return 0;
    });
    let a = rx.recv();
    let b = rx.recv();
    let c = rx.recv();
    let d = rx.recv();
    return a * 1000 + b * 100 + c * 10 + d;
  });
}
"#;
    // 既定（None）は FIFO で常に 1234
    assert_eq!(go(src, "run").value, "1234");

    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..=16 {
        let opts = Options {
            schedule_seed: Some(seed),
            capture_console: true,
            ..Options::default()
        };
        let o1 = go_with(src, "run", &opts);
        let o2 = go_with(src, "run", &opts);
        // 同じ種では結果が完全に一致する
        assert_eq!(o1.value, o2.value);
        seen.insert(o1.value);
    }
    // 複数の種で複数の異なる順序が観測される
    assert!(seen.len() >= 3, "seen orders: {seen:?}");
}

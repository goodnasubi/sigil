//! ADR 0030 決定 7・補足 D（PR 5-2g）: 停止・取り消しのネイティブ版。単一ルート内で観測できる機構を
//! 評価器と突き合わせ、評価器との差を表で固定する。
//!
//! # 評価器との差の表（補足 D-3）
//!
//! | 項目 | 評価器 | ネイティブ | 5-2g の扱い |
//! |---|---|---|---|
//! | ルートタスクの数 | 複数（`run_roots_with`） | 1 つ（番号 0） | 固定のみ。入口は段階 6（ADR 0026 決定 4） |
//! | `task.timeout`／`race`／`all_ok` | 対応 | ビルドで止める | 固定のみ。意味づけは取り消しの結線と共に後続 |
//! | 眠りのない本物のライブロックの停止 | 検出して停止 | 無い（走り続ける） | 固定のみ。指紋の比較が必要で費用に見合わない |
//! | 兄弟の部分木の取り消し | 兄弟ごとの再帰（`cancel_scope`） | 作成順の逆の一列 | 埋めた（対応する操作の範囲では観測差なし） |
//!
//! 兄弟の部分木: 評価器も停止・デッドロックでは `stop_root_inner` が作成順の逆の一列で回る（子は親より後に作られる
//! ので部分木は親より先に片付く）。再帰の `cancel_scope` は `race`／`timeout`／`all_ok` の経路でだけ順序の違いを
//! 生むが、これらはネイティブ未対応なので、対応する操作の範囲では差は観測されない。下の試験が評価器と一致を確かめる。

mod common;

use common::native::try_build;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::time::{Duration, Instant};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\nstruct Tok {\n  id: Int,\n}\n\n";

fn src(body: &str) -> String {
    format!("{HEAD}{body}\n")
}

fn drops(body: &str, z: i64) -> Vec<String> {
    let opts = Options {
        trace_drops: true,
        ..Options::captured()
    };
    // 評価器とネイティブの停止の報告・破棄の順序の一致は `common` が確かめる。
    let e = run_with(&src(body), "run", &[Arg::Int(z)], &opts).unwrap_err();
    assert!(
        matches!(e.error, sigil::exec::RunError::Stop(_)),
        "{:?}",
        e.error
    );
    e.drops.to_vec()
}

#[test]
fn sibling_subtrees_are_cancelled_in_one_reverse_creation_row() {
    // A(1)・B(2) の後に、A の子 A1 が作られる（添字 4 以降）。一列なら A1 → B → A の順で、
    // 兄弟ごとの再帰（B → A1 → A）とは異なりうる並び。評価器の停止も同じ一列。
    let body = r#"
fn leaf(n: Int) -> Int
  intent "l"
  requires none
  ensures none
  effects task.suspend
{
  let t = Tok { id: n };
  task.yield();
  task.yield();
  task.yield();
  return t.id;
}

fn parent(n: Int) -> Int
  intent "p"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mine = Tok { id: n };
  task.yield();
  return task.scope(|s| {
    let k = s.spawn(move || leaf(n + 10));
    task.yield();
    task.yield();
    return k.join() + mine.id;
  });
}

fn run(z: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let top = Tok { id: 0 };
    let a = s.spawn(move || parent(1));
    let b = s.spawn(move || leaf(2));
    let c = s.spawn(move || parent(3));
    let d = s.spawn(move || {
      task.yield();
      task.yield();
      task.yield();
      return 10 / z;
    });
    return a.join() + b.join() + c.join() + d.join() + top.id;
  });
}
"#;
    let d = drops(body, 0);
    assert!(d.len() >= 6, "{d:?}");
}

#[test]
fn broken_lock_waiters_and_holders_are_dropped_in_the_same_order() {
    // 単一ルート内で観測できる壊れた `Lock`（守りを持つタスクの取り消し）: 保持者・`Lock` の値が、停止で評価器と同じ順に破棄される。
    let body = r#"
fn hold(n: Int) -> Int
  intent "h"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let mine = Tok { id: n };
  let l = Lock.new(Tok { id: n + 50 });
  match l.acquire() {
    Ok(g) => {
      task.yield();
      task.yield();
      task.yield();
      return mine.id;
    }
    Err(e) => { return 0 - 1; }
  }
}

fn run(z: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share
{
  return task.scope(|s| {
    let a = s.spawn(move || hold(1));
    let b = s.spawn(move || hold(2));
    let c = s.spawn(move || {
      task.yield();
      return 10 / z;
    });
    return a.join() + b.join() + c.join();
  });
}
"#;
    let d = drops(body, 0);
    assert!(!d.is_empty(), "{d:?}");
}

#[test]
fn unsupported_scope_operations_stop_the_build_but_the_evaluator_runs_them() {
    let heads = [
        ("Option<Int>", "return task.timeout(500, move || 1);"),
        (
            "Int",
            "let mut gs = List.new(); gs.push(|| 1); return task.race(gs);",
        ),
        (
            "Result<List<Int>, String>",
            "let mut gs = List.new(); gs.push(|| Ok(1)); return task.all_ok(gs);",
        ),
    ];
    for (ty, call) in heads {
        let eff = if call.contains("timeout") {
            "clock, task.spawn, task.suspend"
        } else {
            "task.spawn, task.suspend"
        };
        let body = format!(
            "fn run() -> {ty}\n  intent \"r\"\n  requires none\n  ensures none\n  effects {eff}\n{{\n  {call}\n}}\n"
        );
        let s = src(&body);
        // 評価器は実行できる。
        let o = sigil::ir_eval::run(&s, "run", &[]).unwrap_or_else(|e| panic!("{:?}", e.error));
        assert!(!o.value.to_string().is_empty());
        let out = common::native::scratch("unsup").join("prog");
        let e = try_build(&[&s], "run", &out, false).unwrap_err();
        assert!(e.contains("未対応"), "{e}");
    }
}

#[test]
fn evaluator_stops_a_pure_yield_spin_but_native_keeps_running() {
    let s = src(r#"
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
"#);
    // 評価器: デッドロックとして停止する。
    let e = sigil::ir_eval::run(&s, "run", &[]).unwrap_err();
    assert!(
        format!("{:?}", e.error).contains("デッドロック"),
        "{:?}",
        e.error
    );
    // ネイティブ: 検出がないので走り続ける（一定時間後に打ち切る）。
    let exe = common::native::scratch("spin").join("prog");
    try_build(&[&s], "run", &exe, false).unwrap();
    let mut child = std::process::Command::new(&exe)
        .env("SIGIL_WORKERS", "1")
        .spawn()
        .unwrap();
    let t0 = Instant::now();
    let mut still = true;
    while t0.elapsed() < Duration::from_secs(2) {
        if child.try_wait().unwrap().is_some() {
            still = false;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(still, "ネイティブが停止した: 差が埋まったなら表を更新する");
}

#[test]
fn evaluator_runs_several_roots_and_isolates_a_stop() {
    // ルートタスクの数: 評価器だけが複数のルートを持つ。ネイティブの入口は 1 つ（段階 6 まで観測できない）。
    let s = src(r#"
fn req(n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects none
{
  return 10 / (n - 2);
}
"#);
    let sets = vec![vec![Arg::Int(1)], vec![Arg::Int(2)], vec![Arg::Int(4)]];
    let o = sigil::ir_eval::run_roots_with(&[&s], "req", &sets, &Options::captured()).unwrap();
    assert!(o.roots[0].is_ok() && o.roots[1].is_err() && o.roots[2].is_ok());
}

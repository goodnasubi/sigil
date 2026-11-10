//! ADR 0031 6-3c / ADR 0026 決定 3: ネイティブの `task.timeout`。fd の待ちを一度も登録しない間は仮想の時計で決定的に動き、
//! 評価器と突き合わせられる（`common::run*` が値・標準出力・停止・破棄の順序まで比べる）。
mod common;

use common::{run, run_with};
use sigil::exec::{Arg, Options};

fn prog(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

fn go(body: &str, f: &str, args: &[Arg]) -> String {
    match run(&prog(body), f, args) {
        Ok(o) => format!("{}|{}", o.value, o.output),
        Err(e) => panic!("{f}: {:?}", e.error),
    }
}

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

const BASIC: &str = r#"
fn slow(ms: Int, v: Int) -> Int
  intent "s"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(ms);
  return v;
}

fn run(limit: Int, ms: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let r = task.timeout(limit, move || slow(ms, 7));
  match r {
    Some(v) => { return v; }
    None => { return 0 - 1; }
  }
}
"#;

#[test]
fn the_child_wins_when_it_finishes_before_the_deadline() {
    assert!(go(BASIC, "run", &[Arg::Int(100), Arg::Int(5)]).starts_with("7"));
}

#[test]
fn the_deadline_wins_and_the_result_is_none() {
    assert!(go(BASIC, "run", &[Arg::Int(10), Arg::Int(500)]).starts_with("-1"));
}

#[test]
fn equal_times_and_zero_limits_agree_with_the_evaluator() {
    // 子の眠りと期限が同じ時刻のとき、先に登録した方（期限）が先に起きる。0 でも評価器と同じ。
    for (l, m) in [(10, 10), (0, 0), (0, 5), (5, 0), (1, 2)] {
        go(BASIC, "run", &[Arg::Int(l), Arg::Int(m)]);
    }
}

const DROPS: &str = r#"

fn work(ms: Int) -> String
  intent "w"
  requires none
  ensures none
  effects clock, task.suspend
{
  let held = "held";
  task.sleep(ms);
  return held;
}

fn run(limit: Int, ms: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let keep = "keep";
  let r = task.timeout(limit, move || work(ms));
  match r {
    Some(s) => { return s.len() + keep.len(); }
    None => { return keep.len(); }
  }
}
"#;

#[test]
fn owned_values_are_dropped_in_the_evaluators_order() {
    // 子の局所の値は、取り消しで破棄される。突き合わせは破棄の順序まで比べる。
    go_drops(DROPS, "run", &[Arg::Int(10), Arg::Int(500)]);
    go_drops(DROPS, "run", &[Arg::Int(500), Arg::Int(5)]);
}

const NESTED: &str = r#"
fn leaf(ms: Int) -> Int
  intent "l"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(ms);
  return ms;
}

fn mid(limit: Int, ms: Int) -> Int
  intent "m"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let r = task.timeout(limit, move || leaf(ms));
  match r {
    Some(v) => { return v; }
    None => { return 0 - 2; }
  }
}

fn run(a: Int, b: Int, c: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let r = task.timeout(a, move || mid(b, c));
  match r {
    Some(v) => { return v; }
    None => { return 0 - 1; }
  }
}
"#;

#[test]
fn nested_timeouts_agree_with_the_evaluator() {
    for (a, b, c) in [
        (100, 10, 5),
        (100, 10, 50),
        (10, 100, 50),
        (10, 100, 5),
        (20, 20, 20),
    ] {
        go(NESTED, "run", &[Arg::Int(a), Arg::Int(b), Arg::Int(c)]);
    }
}

const PARALLEL: &str = r#"
fn leaf(ms: Int) -> Int
  intent "l"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(ms);
  return ms;
}

fn one(limit: Int, ms: Int) -> Int
  intent "o"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let r = task.timeout(limit, move || leaf(ms));
  match r {
    Some(v) => { return v; }
    None => { return 0 - 1; }
  }
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || one(30, 10));
    let b = s.spawn(move || one(30, 100));
    let c = s.spawn(move || one(5, 5));
    return a.join() + b.join() * 10 + c.join() * 100;
  });
}
"#;

#[test]
fn several_timeouts_in_parallel_tasks_agree_with_the_evaluator() {
    go(PARALLEL, "run", &[]);
}

const CHATTY: &str = r#"
fn say() -> Int
  intent "s"
  requires none
  ensures none
  effects console
{
  console.log(&"x");
  return 0;
}

fn chatty() -> Int
  intent "c"
  requires none
  ensures none
  effects console, task.suspend
{
  let mut r = say();
  while true {
    r = say();
    task.yield();
  }
  return r;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, clock, task.spawn, task.suspend
{
  let r = task.timeout(5, move || chatty());
  match r {
    Some(v) => { return v; }
    None => { return 0 - 1; }
  }
}
"#;

#[test]
fn a_child_that_keeps_yielding_does_not_run_after_the_deadline() {
    // 期限の時点で子は実行可能待ち行列から外れる（評価器は期限の処理の中で子孫を取り除く）。子は時間切れのあとに
    // 1 度も実行されない。ネイティブは `console.log` を「状態の変化」と数えないので、空転の検出は 3 回目の出力のあとで
    // 時計を進める（評価器は `console.log` でも版を進めるので、出力の回数は一致しない。5-2g の差と同じ種類の既知の差）。
    // よって評価器とは突き合わせず、ネイティブの出力がちょうど 3 回（期限のあとに 4 回目がない）であることを固定する。
    let src = prog(CHATTY);
    let dir = common::native::scratch("tmo-chatty");
    let exe = dir.join("prog");
    common::native::try_build(&[&src], "run", &exe, false).unwrap();
    let got = common::native::exec(&exe, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.stdout, "x\nx\nx\n-1\n", "{}", got.stderr);
}

const SUBTREE: &str = r#"
struct Tok {
  id: Int,
}

fn a1() -> Int
  intent "a1"
  requires none
  ensures none
  effects clock, task.suspend
{
  let h = Tok { id: 11 };
  task.sleep(1000);
  return h.id;
}

fn a() -> Int
  intent "a"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let h = Tok { id: 1 };
  let n = task.scope(|s| {
    let x = s.spawn(move || a1());
    return x.join();
  });
  return n + h.id;
}

fn b() -> Int
  intent "b"
  requires none
  ensures none
  effects clock, task.suspend
{
  let h = Tok { id: 2 };
  task.sleep(1000);
  return h.id;
}

fn child() -> Int
  intent "c"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.scope(|s| {
    let x = s.spawn(move || a());
    let y = s.spawn(move || b());
    return x.join() + y.join();
  });
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let r = task.timeout(10, move || child());
  match r {
    Some(v) => { return v; }
    None => { return 0 - 1; }
  }
}
"#;

#[test]
fn a_timeout_cancels_sibling_subtrees_in_the_evaluators_order() {
    // 子が入れ子の範囲に兄弟 A・B を作り、A が A1 を作る。評価器の破棄は B → A1 → A（兄弟を逆順に、各兄弟は子孫を
    // 先に）。作成順の逆の一列（A1 → B → A）にはならない。`go_drops` が破棄の記録まで比べる。
    let d = go_drops(SUBTREE, "run", &[]);
    assert!(d.starts_with("-1"), "{d}");
    assert!(d.matches("Tok").count() >= 3, "破棄の記録がない: {d}");
}

//! Issue #188 PR2 / ADR 0026 決定 3: `LockGuard` の中身への参照 `get` / `get_mut`。
//!
//! 読み・書き、`List` の中身の書き換え、2 つのタスクによる交互の取得、借用の規則
//! （ガードより長く参照を持てない）、中断をまたぐ参照の保持（決定 5）、壊れた `Lock`、
//! 効果の完全一致、実行順の種を変えても結果が変わらないこと、`clone` の禁止を固定する。

use sigil::check::check;
use sigil::error::ErrorKind;
use sigil::exec::{Arg, Options, StopKind};
use sigil::ir_eval::{run, run_roots_with, run_with};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn src(body: &str) -> String {
    format!("{HEAD}{body}\n")
}

fn val(body: &str, f: &str, args: &[Arg]) -> String {
    run(&src(body), f, args)
        .unwrap_or_else(|e| panic!("{:?}", e.error))
        .value
}

fn kind(body: &str) -> ErrorKind {
    check(&src(body)).unwrap_err().kind
}

fn ok(body: &str) {
    if let Err(e) = check(&src(body)) {
        panic!("{:?}: {}", e.kind, e.message);
    }
}

#[test]
fn read_and_write_through_the_guard() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let l = Lock.new(10);
  match l.acquire() {
    Ok(g0) => {
      let mut g = g0;
      let before = g.get() + 0;
      let m = g.get_mut();
      m = before + 5;
    }
    Err(e) => { return 0 - 1; }
  }
  match l.acquire() {
    Ok(g) => { return g.get() + 0; }
    Err(e) => { return 0 - 2; }
  }
}
"#;
    assert_eq!(val(body, "run", &[]), "15");
}

#[test]
fn list_contents_can_be_rewritten_through_the_guard() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let mut xs = List.new();
  xs.push(1);
  xs.push(2);
  xs.push(3);
  let l = Lock.new(xs);
  match l.acquire() {
    Ok(g0) => {
      let mut g = g0;
      let ys = g.get_mut();
      ys.push(4);
    }
    Err(e) => { return 0 - 1; }
  }
  match l.acquire() {
    Ok(g) => { return g.get().sum(); }
    Err(e) => { return 0 - 2; }
  }
}
"#;
    assert_eq!(val(body, "run", &[]), "10");
}

/// 読んで、中断（`task.yield`）をはさんで、書く。排他が効いていれば合計が合う。
const ADD: &str = r#"
fn req(l: &Lock<Int>, k: Int) -> Int
  intent "a"
  requires none
  ensures none
  effects task.share, task.suspend
{
  match l.acquire() {
    Ok(g0) => {
      let mut g = g0;
      let cur = g.get() + 0;
      task.yield();
      let m = g.get_mut();
      m = cur + k;
      return cur + k;
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;

fn roots(body: &str, ks: &[i64], seed: Option<u64>) -> Vec<String> {
    let sets: Vec<Vec<Arg>> = ks
        .iter()
        .enumerate()
        .map(|(i, &k)| vec![Arg::Int(if i == 0 { 0 } else { 99 }), Arg::Int(k)])
        .collect();
    let opts = Options {
        schedule_seed: seed,
        ..Options::default()
    };
    let o = run_roots_with(&[&src(body)], "req", &sets, &opts)
        .unwrap_or_else(|e| panic!("{:?}", e.error));
    o.roots
        .iter()
        .map(|r| r.as_ref().unwrap_or_else(|e| panic!("{e:?}")).clone())
        .collect()
}

#[test]
fn two_tasks_taking_turns_add_up() {
    let r = roots(ADD, &[1, 2, 3], None);
    let mut ns: Vec<i64> = r.iter().map(|s| s.parse().unwrap()).collect();
    ns.sort();
    // 取得の順によらず、各タスクの戻り値は累計で、最後の値が合計になる。
    assert_eq!(*ns.last().unwrap(), 6, "{r:?}");
    assert!(ns[0] >= 1 && ns[1] > ns[0], "{r:?}");
}

#[test]
fn result_does_not_depend_on_the_schedule_seed() {
    for seed in 0..16u64 {
        let r = roots(ADD, &[1, 2, 3], Some(seed));
        let max = r.iter().map(|s| s.parse::<i64>().unwrap()).max().unwrap();
        assert_eq!(max, 6, "seed {seed}: {r:?}");
        let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let l = Lock.new(3);
  match l.acquire() {
    Ok(g) => { return g.get() + 4; }
    Err(e) => { return 0 - 1; }
  }
}
"#;
        let opts = Options {
            schedule_seed: Some(seed),
            ..Options::default()
        };
        assert_eq!(run_with(&src(body), "run", &[], &opts).unwrap().value, "7");
    }
}

#[test]
fn reference_held_across_a_suspension_point_is_kept() {
    // 決定 5: 中断をまたいで参照を持ち続けられる（ガードが生きている間）。
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let l = Lock.new(41);
  match l.acquire() {
    Ok(g) => {
      let r = g.get();
      task.yield();
      task.yield();
      return r + 1;
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    assert_eq!(val(body, "run", &[]), "42");
}

#[test]
fn reference_cannot_outlive_a_moved_guard() {
    let body = r#"
fn run(l: &Lock<Int>) -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  match l.acquire() {
    Ok(g) => {
      let r = g.get();
      let h = g;
      return r + 0;
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let k = kind(body);
    assert_eq!(k, ErrorKind::BorrowConflict, "{k:?}");
}

#[test]
fn reference_cannot_escape_the_guard_scope() {
    let body = r#"
fn run(l: &Lock<Int>) -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let r = match l.acquire() {
    Ok(g) => g.get(),
    Err(e) => l.acquire().unwrap_or(0),
  };
  return 0;
}
"#;
    let k = kind(body);
    assert_eq!(k, ErrorKind::TempBorrow, "{k:?}");
}

#[test]
fn broken_lock_gives_no_guard_to_reach_the_value() {
    // 先頭が `Lock` を持ったまま 0 で割って止まる。後続は `Err(LockBroken)` で、中身には届かない。
    let body = r#"
fn req(l: &Lock<Int>, n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  match l.acquire() {
    Ok(g) => {
      task.yield();
      return 100 / n + g.get();
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let sets: Vec<Vec<Arg>> = [0, 5]
        .iter()
        .map(|&n| vec![Arg::Int(7), Arg::Int(n)])
        .collect();
    let o = run_roots_with(&[&src(body)], "req", &sets, &Options::default()).unwrap();
    match &o.roots[0] {
        Err(e) => assert_eq!(e.kind, StopKind::DivisionByZero),
        other => panic!("{other:?}"),
    }
    assert_eq!(o.roots[1].as_ref().unwrap(), "-1");
}

#[test]
fn effects_must_match_exactly() {
    let f = |eff: &str| {
        format!(
            r#"
fn run(l: &Lock<Int>) -> Int
  intent "r"
  requires none
  ensures none
  effects {eff}
{{
  match l.acquire() {{
    Ok(g) => {{ return g.get() + 0; }}
    Err(e) => {{ return 0 - 1; }}
  }}
}}
"#
        )
    };
    ok(&f("task.share, task.suspend"));
    // `acquire` が `task.suspend` を持つので、`get` 単独の確認は別の関数で行う。
    let g = r#"
fn peek(g: &LockGuard<Int>) -> Int
  intent "p"
  requires none
  ensures none
  effects task.share
{
  return g.get() + 0;
}
"#;
    ok(g);
    let none = g.replace("effects task.share", "effects none");
    assert_eq!(kind(&none), ErrorKind::EffectMissing);
    let extra = g.replace("effects task.share", "effects task.share, task.suspend");
    assert_eq!(kind(&extra), ErrorKind::EffectExtra);
}

#[test]
fn clone_of_task_handles_and_guards_is_rejected() {
    let guard = r#"
fn run(l: &Lock<Int>) -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  match l.acquire() {
    Ok(g) => {
      let h = g.clone();
      return 0;
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    assert_eq!(kind(guard), ErrorKind::NoSuchMember);
    let handle = r#"
fn w() -> Int
  intent "w"
  requires none
  ensures none
  effects none
{
  return 1;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || w());
    let h2 = h.clone();
    return h.join();
  });
}
"#;
    assert_eq!(kind(handle), ErrorKind::NoSuchMember);
    let scope = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let t = s.clone();
    return 0;
  });
}
"#;
    assert_eq!(kind(scope), ErrorKind::NoSuchMember);
}

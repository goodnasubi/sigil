//! Issue #396: race／all_ok の値・停止・所有値の破棄を評価器とネイティブで突き合わせる。
mod common;

use sigil::exec::{Arg, Options, RunError, StopKind};

fn source(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

fn compare(body: &str, entry: &str, args: &[Arg]) -> String {
    let opts = Options {
        trace_drops: true,
        ..Options::captured()
    };
    let out = common::run_with(&source(body), entry, args, &opts)
        .unwrap_or_else(|e| panic!("{entry}: {:?}", e.error));
    out.value.to_string()
}

const BASIC: &str = r#"
fn value(ms: Int, n: Int) -> Int
  intent "待って値を返す"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(ms);
  return n;
}

fn race(a: Int, b: Int) -> Int
  intent "最初に完了した値"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  gs.push(move || value(a, 11));
  gs.push(move || value(b, 22));
  return task.race(gs);
}

fn success(ms: Int, n: Int) -> Result<Int, String>
  intent "待って成功する"
  requires none
  ensures none
  effects clock, task.suspend
{
  return Ok(value(ms, n));
}

fn all() -> Result<List<Int>, String>
  intent "完了順と異なる入力順で結果を返す"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  gs.push(|| success(30, 11));
  gs.push(|| success(5, 22));
  gs.push(|| success(15, 33));
  return task.all_ok(gs);
}

fn plain() -> Int
  intent "中断しないクロージャも使える"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  gs.push(|| 7);
  gs.push(|| 9);
  return task.race(gs);
}

fn empty_all() -> Result<List<Int>, String>
  intent "空の all_ok は成功する"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  if false { gs.push(|| Ok(1)); }
  return task.all_ok(gs);
}

fn empty_race() -> Int
  intent "空の race は標準ライブラリの事前条件違反"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  if false { gs.push(|| 1); }
  return task.race(gs);
}
"#;

#[test]
fn race_chooses_first_completion_and_fifo_breaks_ties() {
    assert_eq!(compare(BASIC, "race", &[Arg::Int(20), Arg::Int(5)]), "22");
    assert_eq!(compare(BASIC, "race", &[Arg::Int(5), Arg::Int(20)]), "11");
    assert_eq!(compare(BASIC, "race", &[Arg::Int(5), Arg::Int(5)]), "11");
    assert_eq!(compare(BASIC, "plain", &[]), "7");
}

#[test]
fn all_ok_returns_input_order_and_accepts_empty_input() {
    assert_eq!(compare(BASIC, "all", &[]), "Ok([11, 22, 33])");
    assert_eq!(compare(BASIC, "empty_all", &[]), "Ok([])");
}

#[test]
fn empty_race_stops_instead_of_deadlocking() {
    let e = common::run(&source(BASIC), "empty_race", &[]).unwrap_err();
    assert!(matches!(e.error, RunError::Stop(ref s) if s.kind == StopKind::StdRequiresViolated));
}

const OWNED: &str = r#"
struct Token { id: Int, }

fn success(ms: Int, n: Int) -> Result<Token, String>
  intent "所有値を成功結果へ移す"
  requires none
  ensures none
  effects clock, task.suspend
{
  let t = Token { id: n };
  task.sleep(ms);
  return Ok(t);
}

fn failure(ms: Int) -> Result<Token, String>
  intent "先に失敗する"
  requires none
  ensures none
  effects clock, task.suspend
{
  let held = Token { id: 99 };
  task.sleep(ms);
  return Err("failed");
}

fn all(ms: Int) -> Result<List<Token>, String>
  intent "成功済み結果と実行中の所有値を片付ける"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  gs.push(|| success(2, 1));
  gs.push(|| success(1, 2));
  gs.push(move || failure(ms));
  gs.push(|| success(100, 4));
  return task.all_ok(gs);
}

fn all_success() -> Result<List<Token>, String>
  intent "成功した所有値を入力順に返す"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  gs.push(|| success(5, 1));
  gs.push(|| success(1, 2));
  return task.all_ok(gs);
}

fn captured() -> Int
  intent "取り消しと完了でクロージャ環境の所有値を返す"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let a = Token { id: 1 };
  let b = Token { id: 2 };
  let mut gs = List.new();
  gs.push(move || { task.sleep(20); return a.id; });
  gs.push(move || { task.sleep(1); return b.id; });
  return task.race(gs);
}

fn leaf(ms: Int, n: Int) -> Token
  intent "取り消しを待つ所有値"
  requires none
  ensures none
  effects clock, task.suspend
{
  let t = Token { id: n };
  task.sleep(ms);
  return t;
}

fn subtree(n: Int) -> Token
  intent "子孫を持つ部分木"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.scope(move |s| {
    let held = Token { id: n };
    let child = s.spawn(move || leaf(100, n + 10));
    return child.join();
  });
}

fn race() -> Token
  intent "負けた部分木を新しい兄弟から片付ける"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let mut gs = List.new();
  gs.push(|| subtree(1));
  gs.push(|| subtree(2));
  gs.push(|| leaf(5, 3));
  return task.race(gs);
}

fn cancelled() -> Int
  intent "親の時間切れで race 自体を取り消す"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  let r = task.timeout(1, || race());
  match r {
    Some(t) => { return t.id; }
    None => { return 0; }
  }
}
"#;

#[test]
fn all_ok_early_error_drops_accumulated_results_before_remaining_tasks() {
    assert_eq!(compare(OWNED, "all", &[Arg::Int(5)]), "Err(\"failed\")");
    assert_eq!(compare(OWNED, "all", &[Arg::Int(0)]), "Err(\"failed\")");
}

#[test]
fn race_cancels_nested_subtrees_and_can_itself_be_cancelled() {
    assert_eq!(compare(OWNED, "captured", &[]), "2");
    assert_eq!(
        compare(OWNED, "all_success", &[]),
        "Ok([Token { id: 1 }, Token { id: 2 }])"
    );
    assert_eq!(compare(OWNED, "race", &[]), "Token { id: 3 }");
    assert_eq!(compare(OWNED, "cancelled", &[]), "0");
}

#[test]
fn multiple_workers_finish_both_combinators_without_leaks() {
    let src = source(BASIC);
    let dir = common::native::scratch("race-all-ok-workers");
    for (entry, expected) in [("plain", "7"), ("all", "Ok([11, 22, 33])")] {
        let exe = dir.join(entry);
        common::native::try_build(&[&src], entry, &exe, false).unwrap();
        let got =
            common::native::exec_env(&exe, &[], &[("SIGIL_WORKERS", "4"), ("SIGIL_RT_LIVE", "1")]);
        assert_eq!(got.code, Some(0), "{}", got.stderr);
        if entry == "all" {
            assert_eq!(got.stdout.trim(), expected);
        } else {
            // 複数ワーカーの race は、どちらの完了が先でもよい。
            assert!(["7", "9"].contains(&got.stdout.trim()));
        }
        assert!(got.stderr.contains("sigil_blocks=0"), "{}", got.stderr);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

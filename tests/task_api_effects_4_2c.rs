//! ADR 0026 決定 3 の並行処理 API 群の効果の検査と ADR 0007 決定 6 の条件 4・5・6 の検証テスト。

use sigil::check::check;
use sigil::error::ErrorKind;

fn check_src(body: &str) -> Result<(), sigil::error::Error> {
    let src = format!("module m.a\n  targets server\n  trust none\n\n{body}\n");
    check(&src)
}

fn ok(body: &str) {
    if let Err(e) = check_src(body) {
        panic!(
            "unexpected error {:?}: {}\ncode:\n{}",
            e.kind, e.message, body
        );
    }
}

fn err_kind(body: &str) -> ErrorKind {
    check_src(body).unwrap_err().kind
}

#[test]
fn test_task_scope_and_spawn_and_join() {
    ok(r#"
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
"#);
}

#[test]
fn test_task_sleep_and_yield() {
    ok(r#"
fn pause()
  intent "p"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(100);
  task.yield();
}
"#);
}

#[test]
fn test_task_timeout() {
    ok(r#"
fn compute() -> Int
  intent "c"
  requires none
  ensures none
  effects none
{
  return 123;
}

fn run() -> Option<Int>
  intent "r"
  requires none
  ensures none
  effects clock, task.spawn, task.suspend
{
  return task.timeout(500, move || compute());
}
"#);
}

#[test]
fn test_task_channel() {
    ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let (tx, rx) = task.channel();
  tx.send(10);
  return rx.recv();
}
"#);
}

#[test]
fn test_sync_shared_and_atomic() {
    ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let s = SyncShared.new(1);
  let a = Atomic.new(2);
  a.store(3);
  return s.get() + a.load();
}
"#);
}

#[test]
fn test_lock_and_guard() {
    ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let l = Lock.new(10);
  let res = l.acquire();
  return 0;
}
"#);
}

#[test]
fn test_of_gs_with_all_ok_and_race() {
    ok(r#"
fn work_pure() -> Result<Int, String>
  intent "pure"
  requires none
  ensures none
  effects none
{
  return Ok(1);
}

fn run_pure(gs: List<fn() -> Result<Int, String>>) -> Result<List<Int>, String>
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, of(gs)
{
  return task.all_ok(gs);
}

fn race_pure(gs: List<fn() -> Int>) -> Int
  intent "race"
  requires none
  ensures none
  effects task.spawn, task.suspend, of(gs)
{
  return task.race(gs);
}
"#);
}

#[test]
fn test_adr0007_condition_4_task_effects() {
    // 条件 4: 標準ライブラリの表に task.* を加え、コンパイラを変えずに検査が働く
    // 効果不足でエラー
    let src = r#"
fn run()
  intent "r"
  requires none
  ensures none
  effects none
{
  task.yield();
}
"#;
    assert_eq!(err_kind(src), ErrorKind::EffectMissing);

    // 効果過剰でエラー（完全一致検査）
    let src2 = r#"
fn run()
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  task.yield();
}
"#;
    assert_eq!(err_kind(src2), ErrorKind::EffectExtra);
}

#[test]
fn test_adr0007_condition_5_of_f_inheritance() {
    // 条件 5: 中断する関数を渡すと of(f) が task.suspend に置き換わる
    ok(r#"
fn apply<T>(f: fn() -> T) -> T
  intent "apply"
  requires none
  ensures none
  effects of(f)
{
  return f();
}

fn pause()
  intent "pause"
  requires none
  ensures none
  effects task.suspend
{
  task.yield();
}

fn caller()
  intent "caller"
  requires none
  ensures none
  effects task.suspend
{
  apply(move || pause());
}
"#);
}

#[test]
fn test_adr0007_condition_6_lock_is_task_share_not_state() {
    // 条件 6: Lock の操作が task.share と推論され、state では一致しない
    let src_state = r#"
fn run(l: &Lock<Int>)
  intent "run"
  requires none
  ensures none
  effects state, task.suspend
{
  let r = l.acquire();
}
"#;
    assert_eq!(err_kind(src_state), ErrorKind::EffectMissing);

    let src_ok = r#"
fn run(l: &Lock<Int>)
  intent "run"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let r = l.acquire();
}
"#;
    ok(src_ok);
}

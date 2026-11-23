//! 明示終了で破棄したロックの通知を次の評価に持ち越さない。
use sigil::exec::{Options, RunError};
use sigil::ir_eval::run_with;

const SRC: &str = r#"
module m.process_exit_lock
  targets server
  trust none

fn finish() -> Int
  intent "ロックを保持したまま終了する"
  requires none
  ensures none
  effects process, task.share, task.suspend
{
  let lock = Lock.new(17);
  match lock.acquire() {
    Ok(guard) => {
      process.exit(43);
      let _ = guard.get();
      return 17;
    }
    Err(_) => { return 0; }
  }
}

fn ordinary() -> Int
  intent "後続の評価が通常どおり完了することを確かめる"
  requires none
  ensures none
  effects none
{
  return 39;
}
"#;

#[test]
fn explicit_exit_with_a_live_guard_does_not_affect_the_next_run() {
    let opts = Options::captured();
    let exited = run_with(SRC, "finish", &[], &opts).unwrap_err();
    assert!(matches!(exited.error, RunError::Exit(43)), "{exited:?}");
    assert_eq!(run_with(SRC, "ordinary", &[], &opts).unwrap().value, "39");
}

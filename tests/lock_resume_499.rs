//! プール待機で露呈した、Lock 起床直後の取消で保持者を残さない回帰試験。
mod common;

fn source() -> String {
    let mut s = String::from(
        r#"module lock.resume
  targets server
  trust none
fn contend(lock: &Lock<Int>) -> Int
  intent "期限までロックを競合させ起床直後の取消を検査する"
  requires none
  ensures none
  effects task.spawn, task.share, task.suspend, clock
{
  let timed = task.timeout(10, || {
    while true {
      match lock.acquire() {
        Ok(guard) => { task.sleep(1); }
        Err(e) => { return 1; }
      }
    }
    return 2;
  });
  match timed { None => { return 0; } Some(n) => { return n; } }
}
pub fn main() -> Int
  intent "競合待機の取消後も保持者が消え通常取得できる"
  requires none
  ensures none
  effects task.spawn, task.share, task.suspend, clock
{
  let lock = Lock.new(0);
  let mut round = 0;
  while round < 4 {
    let total = task.scope(|scope| {
"#,
    );
    for i in 0..64 {
        s.push_str(&format!("      let copy{i} = lock.clone();\n      let h{i} = scope.spawn(move || contend(&copy{i}));\n"));
    }
    s.push_str("      let mut total = 0;\n");
    for i in 0..64 {
        s.push_str(&format!("      total = total + h{i}.join();\n"));
    }
    s.push_str(
        r#"      return total;
    });
    if total != 0 { return 1; }
    match task.timeout(100, || {
      match lock.acquire() { Ok(guard) => { return 0; } Err(e) => { return 2; } }
    }) { Some(0) => {} _ => { return 3; } }
    round = round + 1;
  }
  return 0;
}
"#,
    );
    s
}

#[test]
fn cancelled_awakened_lock_acquisitions_release_the_holder() {
    let source = source();
    assert_eq!(
        sigil::ir_eval::run(&source, "main", &[]).unwrap().value,
        "0"
    );
    let dir = common::native::scratch("lock-resume-499");
    let exe = dir.join("lock");
    sigil::native::build(&[&source], "main", &Default::default(), &exe).unwrap();
    for workers in ["1", "4"] {
        let out = common::native::exec_env(
            &exe,
            &[],
            &[("SIGIL_WORKERS", workers), ("SIGIL_RT_LIVE", "1")],
        );
        assert_eq!(out.code, Some(0), "{workers}: {}", out.stderr);
        assert_eq!(out.stdout.trim(), "0", "{workers}: {}", out.stderr);
        assert!(
            out.stderr.contains("live: sigil_blocks=0"),
            "{}",
            out.stderr
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

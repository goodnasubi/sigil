//! ADR 0026 決定 4（PR 4-2f）: 要求の隔離（評価器）。
//!
//! - ルートタスク単位の異常停止の捕捉（他のルートタスクを止めない）
//! - 壊れた `Lock` の伝播（待機者を起こして `Err(LockBroken)`）
//! - 確保器の枠の中の停止は全体停止
//! - 停止の経路での漏れ（破棄の記録と `SIGIL_RT_LIVE`）

use sigil::exec::{Arg, Options, StopKind};
use sigil::ir_eval::{run_roots_with, RootsOutcome};
use std::process::Command;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

const PROBE: &str = r#"
struct Probe {
  id: Int,
}

"#;

fn opts() -> Options {
    Options {
        trace_drops: true,
        capture_console: true,
        ..Options::default()
    }
}

fn roots(src: &str, sets: Vec<Vec<Arg>>) -> RootsOutcome {
    run_roots_with(&[src], "req", &sets, &opts()).unwrap_or_else(|e| panic!("{:?}", e.error))
}

fn ints(ns: &[i64]) -> Vec<Vec<Arg>> {
    ns.iter().map(|&n| vec![Arg::Int(n)]).collect()
}

fn probe_ids(drops: &[String]) -> Vec<i64> {
    let mut v: Vec<i64> = drops
        .iter()
        .map(|s| {
            s.trim_start_matches("Probe { id: ")
                .trim_end_matches(" }")
                .parse()
                .unwrap()
        })
        .collect();
    v.sort();
    v
}

fn kind(r: &Result<String, Box<sigil::exec::Stop>>) -> StopKind {
    r.as_ref().unwrap_err().kind
}

/// 要求を模した入口: 値を持ったまま中断し、`n == 2` のとき 0 で割る。
const REQ_DIV: &str = r#"
fn req(n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.suspend
{
  let p = Probe { id: n };
  task.yield();
  let z = n - 2;
  task.yield();
  return 100 / z;
}
"#;

#[test]
fn root_stop_does_not_stop_other_roots_and_drops_everything() {
    let src = format!("{HEAD}{PROBE}{REQ_DIV}");
    let o = roots(&src, ints(&[1, 2, 3, 4]));
    assert_eq!(o.roots.len(), 4);
    assert_eq!(o.roots[0].as_ref().unwrap(), "-100");
    assert_eq!(kind(&o.roots[1]), StopKind::DivisionByZero);
    assert_eq!(o.roots[2].as_ref().unwrap(), "100");
    assert_eq!(o.roots[3].as_ref().unwrap(), "50");
    // 止めたルートの値も、完了したルートの値も、ちょうど 1 回ずつ破棄される。
    assert_eq!(probe_ids(&o.drops), vec![1, 2, 3, 4]);
}

#[test]
fn isolation_holds_under_every_schedule_seed() {
    let src = format!("{HEAD}{PROBE}{REQ_DIV}");
    for seed in 0..16u64 {
        let o = Options {
            schedule_seed: Some(seed),
            ..opts()
        };
        let o = run_roots_with(&[&src], "req", &ints(&[1, 2, 3]), &o).unwrap();
        assert!(o.roots[0].is_ok() && o.roots[2].is_ok(), "seed {seed}");
        assert_eq!(kind(&o.roots[1]), StopKind::DivisionByZero);
        assert_eq!(probe_ids(&o.drops), vec![1, 2, 3], "seed {seed}");
    }
}

#[test]
fn child_stop_stops_whole_root_including_siblings_but_not_other_roots() {
    // 子の異常停止は、その範囲を含むルート全体を止める。兄弟の子の値も破棄される。他のルートは止まらない。
    let src = format!(
        r#"{HEAD}{PROBE}
fn boom(n: Int) -> Int
  intent "b"
  requires none
  ensures none
  effects task.suspend
{{
  let p = Probe {{ id: n + 100 }};
  task.yield();
  return 1 / (n - 2);
}}

fn idle(n: Int) -> Int
  intent "i"
  requires none
  ensures none
  effects task.suspend, clock
{{
  let p = Probe {{ id: n + 200 }};
  task.sleep(1000000);
  return 0;
}}

fn req(n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, clock
{{
  let mine = Probe {{ id: n }};
  return task.scope(|s| {{
    let a = s.spawn(move || idle(n));
    let b = s.spawn(move || boom(n));
    return a.join() + b.join();
  }});
}}
"#
    );
    let o = roots(&src, ints(&[1, 2, 3]));
    assert_eq!(kind(&o.roots[1]), StopKind::DivisionByZero);
    assert!(o.roots[0].is_ok() && o.roots[2].is_ok(), "{:?}", o.roots);
    // 1: mine, 101, 201 / 2: mine, 102, 202 / 3: mine, 103, 203 がすべて 1 回ずつ。
    assert_eq!(
        probe_ids(&o.drops),
        vec![1, 2, 3, 101, 102, 103, 201, 202, 203]
    );
}

const REQ_LOCK: &str = r#"
fn req(l: &Lock<Int>, n: Int) -> Int
  intent "r"
  requires none
  ensures none
  effects task.share, task.suspend
{
  match l.acquire() {
    Ok(g) => {
      task.yield();
      return 100 / n;
    }
    Err(e) => {
      return 0 - 1;
    }
  }
}
"#;

fn lock_sets(ns: &[i64]) -> Vec<Vec<Arg>> {
    ns.iter().map(|&n| vec![Arg::Int(7), Arg::Int(n)]).collect()
}

#[test]
fn broken_lock_wakes_waiters_with_err() {
    let src = format!("{HEAD}{REQ_LOCK}");
    // 先頭が `Lock` を持ったまま 0 で割って止まる。待っていた次のルートは `Err(LockBroken)` で起きる。
    let o = roots(&src, lock_sets(&[0, 5]));
    assert_eq!(kind(&o.roots[0]), StopKind::DivisionByZero);
    assert_eq!(o.roots[1].as_ref().unwrap(), "-1");
    // 壊れた後に取得しようとしても `Err(LockBroken)`。
    let o = roots(&src, lock_sets(&[0, 5, 4]));
    assert_eq!(o.roots[1].as_ref().unwrap(), "-1");
    assert_eq!(o.roots[2].as_ref().unwrap(), "-1");
}

#[test]
fn normal_release_hands_the_lock_to_the_next_waiter() {
    let src = format!("{HEAD}{REQ_LOCK}");
    let o = roots(&src, lock_sets(&[1, 5, 4]));
    let vals: Vec<&str> = o
        .roots
        .iter()
        .map(|r| r.as_ref().unwrap().as_str())
        .collect();
    assert_eq!(vals, ["100", "20", "25"]);
}

#[test]
fn non_allocator_module_stop_is_isolated() {
    // 確保器（`std.alloc` / `std.os.alloc`）の枠の中の停止は全体停止になる（判定は `sched.rs` の
    // 単体テスト）。標準ライブラリの名前は利用者のモジュールに使えないので、ここでは逆側を確かめる。
    let util = "module lib.util\n  targets server\n  trust none\n\npub fn grab(n: Int) -> Int\n  intent \"g\"\n  requires none\n  ensures none\n  effects none\n{\n  return 10 / n;\n}\n";
    let main = "module app.main\n  targets server\n  trust none\n\nuse lib.util.grab;\n\npub fn req(n: Int) -> Int\n  intent \"r\"\n  requires none\n  ensures none\n  effects task.suspend\n{\n  task.yield();\n  return grab(n);\n}\n";
    let o = run_roots_with(&[main, util], "req", &ints(&[0, 5]), &opts())
        .unwrap_or_else(|e| panic!("{:?}", e.error));
    assert_eq!(kind(&o.roots[0]), StopKind::DivisionByZero);
    assert_eq!(o.roots[1].as_ref().unwrap(), "2");
}

/// `SIGIL_RT_LIVE`: 停止の経路でも、枠が残らない。
fn live_line(body: &str, args: &[&str]) -> (String, String) {
    let dir =
        std::env::temp_dir().join(format!("sigil-4-2f-{}-{}", std::process::id(), body.len()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join("m")).unwrap();
    let file = dir.join("m").join("a.sigil");
    std::fs::write(&file, format!("{HEAD}{PROBE}{body}")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("run")
        .arg("--root")
        .arg(&dir)
        .arg(&file)
        .arg("req")
        .args(args)
        .env("SIGIL_RT_LIVE", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn no_frames_leak_on_stop_path() {
    let (_, ok) = live_line(REQ_DIV, &["1"]);
    assert!(ok.contains("live: frames=0 chunks=0"), "{ok}");
    let (_, stopped) = live_line(REQ_DIV, &["2"]);
    assert!(
        stopped.contains("DivisionByZero") || stopped.contains("0 で"),
        "{stopped}"
    );
    assert!(stopped.contains("live: frames=0 chunks=0"), "{stopped}");
}

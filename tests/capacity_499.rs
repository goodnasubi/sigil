//! ADR 0034: 容量は移動可能な一枠を所有し、取消でも待たずに回収する。
mod common;
use sigil::{check::check, exec::Arg};

const SOURCE: &str = r#"module capacity.test
  targets server
  trust none
struct Held { token: CapacityPermit, }
fn capacity(n: Int) -> Capacity
  intent "有効な容量を生成する"
  requires none
  ensures none
  effects none
{
  match Capacity.new(n) { Some(c) => { return c; } None => { return capacity(1); } }
}
fn acquired(c: &Capacity) -> Bool
  intent "一枠を一時所有して破棄する"
  requires none
  ensures none
  effects task.share
{
  match c.try_acquire() { Some(p) => { let h = Held { token: p }; return true; } None => { return false; } }
}
fn consume(p: CapacityPermit)
  intent "移動先だけが破棄する"
  requires none
  ensures none
  effects none
{}
fn run(mode: Int) -> Int
  intent "容量の境界・入れ子・共有・取消を照合する"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share, clock
{
  if mode == 0 {
    match Capacity.new(0) { None => {} _ => { return 1; } }
    match Capacity.new(4097) { None => {} _ => { return 2; } }
    let c = capacity(1);
    let d = c.clone();
    let p = c.try_acquire();
    if acquired(&d) { return 3; }
    match p { Some(p) => { consume(p); } None => { return 4; } }
    if !acquired(&d) || !acquired(&c) { return 5; }
    return 0;
  }
  if mode == 1 {
    let c = capacity(1);
    let copy = c.clone();
    let result = task.timeout(5, move || {
      let p = copy.try_acquire();
      task.sleep(100);
      return 1;
    });
    match result { None => {} _ => { return 6; } }
    if !acquired(&c) { return 7; }
    return 0;
  }
  if mode == 2 {
    let c = capacity(16);
    let d = capacity(64);
    let mut connections = List.new();
    let mut waiters = List.new();
    let mut i = 0;
    while i < 64 {
      match d.try_acquire() { Some(p) => { waiters.push(p); } None => { return 8; } }
      if i < 16 { match c.try_acquire() { Some(p) => { connections.push(p); } None => { return 9; } } }
      i = i + 1;
    }
    if acquired(&c) || acquired(&d) { return 10; }
    return 0;
  }
  let c = capacity(1);
  return task.scope(|s| {
    let (tx, rx) = task.channel();
    let d = c.clone();
    let h = s.spawn(move || {
      match d.try_acquire() {
        Some(p) => { tx.send(p); return 0; }
        None => { return 11; }
      }
    });
    let p = rx.recv();
    if h.join() != 0 || acquired(&c) { return 12; }
    consume(p);
    if !acquired(&c) { return 13; }
    return 0;
  });
}
"#;

#[test]
fn capacity_recovery_eval_and_native_workers() {
    for mode in 0..4 {
        let got = common::run(SOURCE, "run", &[Arg::Int(mode)]).unwrap();
        assert_eq!(got.value, "0", "mode {mode}");
    }
    let dir = common::native::scratch("capacity499");
    let exe = dir.join("capacity");
    common::native::try_build(&[SOURCE], "run", &exe, false).unwrap();
    for workers in ["1", "4"] {
        for mode in 0..4 {
            let result = common::native::exec_env(
                &exe,
                &[mode.to_string()],
                &[("SIGIL_WORKERS", workers), ("SIGIL_RT_LIVE", "1")],
            );
            assert_eq!(result.code, Some(0), "{workers}/{mode}: {}", result.stderr);
            assert_eq!(result.stdout, "0\n", "{workers}/{mode}: {}", result.stderr);
            assert!(
                result.stderr.contains("live: sigil_blocks=0"),
                "{}",
                result.stderr
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn permits_cannot_be_duplicated_or_shared_even_inside_values() {
    for (ty, expr) in [
        ("CapacityPermit", "p.clone()"),
        ("Option<CapacityPermit>", "p.clone()"),
        ("Held", "p.clone()"),
    ] {
        let source = format!("{SOURCE}\nfn bad(p: {ty}) -> {ty}\n  intent \"複製禁止\"\n  requires none\n  ensures none\n  effects none\n{{ return {expr}; }}\n");
        assert!(check(&source).is_err(), "{ty} clone");
    }
    for bound in ["Copyable", "Shareable"] {
        let source = format!("{SOURCE}\nfn need<T: {bound}>(x: T)\n  intent \"性質\"\n  requires none\n  ensures none\n  effects none\n{{}}\nfn bad(p: CapacityPermit)\n  intent \"共有禁止\"\n  requires none\n  ensures none\n  effects none\n{{ need(p); }}\n");
        assert!(check(&source).is_err(), "permit {bound}");
    }
}

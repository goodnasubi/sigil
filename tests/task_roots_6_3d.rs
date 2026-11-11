//! ADR 0031 決定 4（PR 6-3d）: 複数ルートの入口 `task.roots`・`RootGroup.reserve`・`RootSlot.start`、
//! ルート単位の隔離、ルートをまたぐ `LockBroken`、ルートの漏れの試験（評価器）。
//!
//! 評価器の結果を、`tests/common` がネイティブの実行ファイル（`SIGIL_WORKERS=1`）の出力・破棄の順序と突き合わせる。

mod common;
use common::run_with;
use sigil::exec::{Options, Outcome, RunError, StopKind};

const HEAD: &str = r#"module m.a
  targets server
  trust none

struct Conn {
  id: Int,
}

struct Tmp {
  v: Int,
}
"#;

fn opts() -> Options {
    Options {
        trace_drops: true,
        ..Options::captured()
    }
}

fn ok(body: &str) -> Outcome {
    run_with(&format!("{HEAD}{body}"), "run", &[], &opts())
        .unwrap_or_else(|e| panic!("{:?}\n{}", e.error, e.output))
}

fn stop(body: &str) -> (StopKind, String, Vec<String>, String) {
    let e = run_with(&format!("{HEAD}{body}"), "run", &[], &opts()).expect_err("止まる");
    match e.error {
        RunError::Stop(s) => (s.kind, s.detail.clone(), e.drops.to_vec(), e.output),
        other => panic!("{other:?}"),
    }
}

/// 検査のエラーは実行ファイルを作らないので、評価器を直接呼ぶ（突き合わせの「飛ばし」に数えない）。
fn check_err(body: &str) -> String {
    let e = sigil::ir_eval::run_with(&format!("{HEAD}{body}"), "run", &[], &opts())
        .expect_err("検査で止まる");
    match e.error {
        RunError::Check(c) => format!("{:?} {}", c.kind, c.message),
        other => panic!("{other:?}"),
    }
}

fn count(drops: &[String], s: &str) -> usize {
    drops.iter().filter(|d| d.as_str() == s).count()
}

fn pos(drops: &[String], s: &str) -> usize {
    drops
        .iter()
        .position(|d| d == s)
        .unwrap_or_else(|| panic!("{s} が破棄されていない: {drops:?}"))
}

/// 上限 1 では、`reserve` は起動したルートが終わるまで待つ。接続は正常終了で 1 回だけ破棄される。
#[test]
fn reserve_waits_for_a_free_slot_and_conn_is_dropped_once() {
    let o = ok(r#"
fn serve(c: &Conn)
  intent "s"
  requires none
  ensures none
  effects console, task.suspend
{
  console.log(&c.id);
  task.yield();
  console.log(&(10 + c.id));
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend
{
  return task.roots(1, |g| {
    let mut i = 1;
    while i <= 3 {
      let s = g.reserve();
      console.log(&(100 + i));
      s.start(Conn { id: i }, move |c| { serve(&c); }, move |c| { console.log(&(200 + c.id)); });
      i = i + 1;
    }
    return 7;
  });
}
"#);
    assert_eq!(o.value, "7");
    assert_eq!(o.output, "101\n1\n11\n102\n2\n12\n103\n3\n13\n");
    for i in 1..=3 {
        assert_eq!(
            count(&o.drops, &format!("Conn {{ id: {i} }}")),
            1,
            "{:?}",
            o.drops
        );
    }
}

/// 使わずに捨てた `RootSlot` は上限を返す（返さなければ 2 回目の `reserve` が進まない）。
#[test]
fn dropping_an_unused_slot_returns_it() {
    let o = ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend
{
  return task.roots(1, |g| {
    if true {
      let s = g.reserve();
    }
    let s2 = g.reserve();
    s2.start(Conn { id: 1 }, move |c| { console.log(&c.id); }, move |c| { });
    return 7;
  });
}
"#);
    assert_eq!((o.value.as_str(), o.output.as_str()), ("7", "1\n"));
}

const WORK: &str = r#"
fn work(c: &Conn) -> Int
  intent "w"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share, clock
{
  let t = Tmp { v: c.id * 10 };
  let (tx, rx) = task.channel();
  tx.send(Tmp { v: c.id * 10 + 1 });
  let l = Lock.new(Tmp { v: c.id * 10 + 2 });
  let id = c.id;
  return task.scope(|s| {
    let h = s.spawn(move || {
      let u = Tmp { v: id * 10 + 3 };
      task.sleep(1000);
      return 0;
    });
    task.yield();
    return 10 / (id - 2);
  });
}
"#;

/// 異常停止したルートだけが止まる。そのルートの値（局所・子のタスク・通信路の項・`Lock` の値）はすべて
/// 破棄され、接続は破棄されずに `on_abort` に渡り、その後で 1 回だけ破棄される。ほかのルートは動き続ける。
#[test]
fn aborted_root_is_isolated_and_its_values_are_all_dropped() {
    let o = ok(&format!(
        r#"{WORK}
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend, task.share, clock
{{
  return task.roots(4, |g| {{
    let mut i = 1;
    while i <= 3 {{
      let s = g.reserve();
      s.start(Conn {{ id: i }}, move |c| {{ let r = work(&c); console.log(&r); }}, move |c| {{ console.log(&(200 + c.id)); }});
      i = i + 1;
    }}
    return 7;
  }});
}}
"#
    ));
    assert_eq!(o.value, "7");
    let lines: Vec<&str> = o.output.lines().collect();
    assert!(lines.contains(&"202"), "{}", o.output);
    assert!(
        lines.contains(&"-10") && lines.contains(&"10"),
        "{}",
        o.output
    );
    assert!(
        !lines.contains(&"201") && !lines.contains(&"203"),
        "{}",
        o.output
    );
    let d = &o.drops;
    for v in [20, 21, 22, 23] {
        assert_eq!(count(d, &format!("Tmp {{ v: {v} }}")), 1, "{d:?}");
        assert!(
            pos(d, &format!("Tmp {{ v: {v} }}")) < pos(d, "Conn { id: 2 }"),
            "{d:?}"
        );
    }
    for i in 1..=3 {
        assert_eq!(count(d, &format!("Conn {{ id: {i} }}")), 1, "{d:?}");
    }
}

/// `on_abort` 自身が異常停止したら、接続を破棄して終わる（それ以上は続けない）。親は動き続ける。
#[test]
fn abort_of_on_abort_drops_the_conn_once() {
    let o = ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend
{
  return task.roots(1, |g| {
    let mut i = 1;
    while i <= 2 {
      let s = g.reserve();
      s.start(Conn { id: i }, move |c| { let q = 1 / (c.id - c.id); }, move |c| {
        console.log(&(200 + c.id));
        let z = 1 / (c.id - c.id);
      });
      i = i + 1;
    }
    return 7;
  });
}
"#);
    assert_eq!((o.value.as_str(), o.output.as_str()), ("7", "201\n202\n"));
    assert_eq!(count(&o.drops, "Conn { id: 1 }"), 1, "{:?}", o.drops);
    assert_eq!(count(&o.drops, "Conn { id: 2 }"), 1, "{:?}", o.drops);
}

/// `g` のルートが `Lock` を持ったまま異常停止すると、その `Lock` は壊れ、別のルート（`on_abort`）の取得は
/// `Err(LockBroken)` になる（ADR 0031 決定 4-3）。
#[test]
fn lock_held_by_an_aborted_root_is_broken_for_another_root() {
    let o = ok(r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend, task.share
{
  return task.roots(1, |g| {
    let s = g.reserve();
    s.start(Lock.new(5), move |l| {
      match l.acquire() {
        Ok(gd) => {
          console.log(&1);
          let q = 10 / (gd.get() - 5);
        }
        Err(e) => { console.log(&9); }
      }
    }, move |l| {
      match l.acquire() {
        Ok(gd) => { console.log(&2); }
        Err(e) => { console.log(&3); }
      }
    });
    return 7;
  });
}
"#);
    assert_eq!((o.value.as_str(), o.output.as_str()), ("7", "1\n3\n"));
}

const SLEEPY: &str = r#"
fn nap(c: &Conn)
  intent "n"
  requires none
  ensures none
  effects clock, task.suspend
{
  let t = Tmp { v: c.id };
  task.sleep(1000);
}
"#;

/// 集まりの取り消し（`task.timeout` による本体の取り消し）では、`on_abort` を呼ばずに接続を破棄する。
#[test]
fn cancelling_the_group_drops_conns_without_on_abort() {
    let o = ok(&format!(
        r#"{SLEEPY}
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, clock, task.spawn, task.suspend
{{
  let r = task.timeout(50, || task.roots(2, |g| {{
    let mut i = 1;
    while i <= 2 {{
      let s = g.reserve();
      s.start(Conn {{ id: i }}, move |c| {{ nap(&c); }}, move |c| {{ console.log(&(200 + c.id)); }});
      i = i + 1;
    }}
    return 7;
  }}));
  match r {{
    Some(v) => {{ return v; }}
    None => {{ return 0 - 1; }}
  }}
}}
"#
    ));
    assert_eq!((o.value.as_str(), o.output.as_str()), ("-1", ""));
    let d = &o.drops;
    for i in 1..=2 {
        assert_eq!(count(d, &format!("Conn {{ id: {i} }}")), 1, "{d:?}");
        assert_eq!(count(d, &format!("Tmp {{ v: {i} }}")), 1, "{d:?}");
        assert!(pos(d, &format!("Tmp {{ v: {i} }}")) < pos(d, &format!("Conn {{ id: {i} }}")));
    }
    // 起動の逆順に取り消す。
    assert!(pos(d, "Conn { id: 2 }") < pos(d, "Conn { id: 1 }"), "{d:?}");
}

/// 本体 `f` の異常停止は、`task.roots` の外のコードの停止なので、従来どおりプログラム全体の停止。
/// 起動済みのルートは取り消され、接続は破棄される（`on_abort` は呼ばない）。
#[test]
fn abort_of_the_body_stops_the_program_and_drops_conns() {
    let (kind, _, drops, out) = stop(&format!(
        r#"{SLEEPY}
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, clock, task.spawn, task.suspend
{{
  return task.roots(2, |g| {{
    let s = g.reserve();
    s.start(Conn {{ id: 1 }}, move |c| {{ nap(&c); }}, move |c| {{ console.log(&(200 + c.id)); }});
    task.yield();
    let mut z = 0;
    return 1 / z;
  }});
}}
"#
    ));
    assert_eq!(kind, StopKind::DivisionByZero);
    assert_eq!(out, "");
    assert_eq!(count(&drops, "Conn { id: 1 }"), 1, "{drops:?}");
    assert_eq!(count(&drops, "Tmp { v: 1 }"), 1, "{drops:?}");
}

#[test]
fn limit_out_of_range_stops() {
    for n in ["0", "4097"] {
        let (kind, detail, _, _) = stop(&format!(
            r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.suspend
{{
  return task.roots({n}, |g| {{ return 7; }});
}}
"#
        ));
        assert_eq!(kind, StopKind::StdRequiresViolated, "{detail}");
        assert!(detail.contains("limit"), "{detail}");
    }
}

/// 範囲の外へ持ち出したスロットでは起動できない（起動したルートが範囲より長く生きないため）。
#[test]
fn start_after_the_group_ended_stops() {
    let (kind, detail, _, _) = stop(
        r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let s = task.roots(1, |g| { return g.reserve(); });
  s.start(Conn { id: 1 }, move |c| { }, move |c| { });
  return 7;
}
"#,
    );
    assert_eq!(kind, StopKind::StdRequiresViolated, "{detail}");
    assert!(detail.contains("範囲が終わっている"), "{detail}");
}

#[test]
fn closures_passed_to_start_cannot_capture_borrows() {
    for (g, a) in [
        ("|c| { let n = k.id; }", "move |c| { }"),
        ("move |c| { }", "|c| { let n = k.id; }"),
    ] {
        let e = check_err(&format!(
            r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{{
  let k = Conn {{ id: 9 }};
  return task.roots(1, |g| {{
    let s = g.reserve();
    s.start(Conn {{ id: 1 }}, {g}, {a});
    return 7;
  }});
}}
"#
        ));
        assert!(e.starts_with("SpawnCaptureBorrow"), "{e}");
    }
}

#[test]
fn conn_must_be_sendable_and_handles_cannot_be_cloned() {
    let e = check_err(
        r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.roots(1, |g| {
    let s = g.reserve();
    s.start(Shared.new(1), move |c| { }, move |c| { });
    return 7;
  });
}
"#,
    );
    assert!(e.contains("Sendable"), "{e}");
    let e = check_err(
        r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.suspend
{
  return task.roots(1, |g| {
    let h = g.clone();
    return 7;
  });
}
"#,
    );
    assert!(e.starts_with("NoSuchMember"), "{e}");
}

/// 漏れの試験（実行ファイルの `sigil run`）: 異常停止したルートを含めて、終わった時点で生きている枠が無く、
/// 停止の報告は標準エラーに出る。
#[test]
fn no_frames_leak_after_isolated_abort() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("task_roots_6_3d");
    std::fs::create_dir_all(&dir).expect("作業場所");
    let src = format!(
        r#"{}{WORK}
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share, clock
{{
  return task.roots(4, |g| {{
    let mut i = 1;
    while i <= 3 {{
      let s = g.reserve();
      s.start(Conn {{ id: i }}, move |c| {{ let r = work(&c); }}, move |c| {{ let n = c.id; }});
      i = i + 1;
    }}
    return 7;
  }});
}}
"#,
        HEAD.replace("module m.a", "module leak")
    );
    let file = dir.join("leak.sigil");
    std::fs::write(&file, src).expect("書き込み");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["run", "--root"])
        .arg(&dir)
        .arg(&file)
        .arg("run")
        .env("SIGIL_RT_LIVE", "1")
        .output()
        .expect("sigil run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout.trim(), "7", "{stderr}");
    assert!(stderr.contains("live: frames=0 chunks=0"), "{stderr}");
    assert_eq!(stderr.matches("0 での除算").count(), 1, "{stderr}");
}

const STUCK_ROOTS: &str = r#"
  return task.roots(2, |g| {
    let s = g.reserve();
    s.start(Conn { id: 1 }, move |c| { console.log(&1); }, move |c| { console.log(&201); });
    let s2 = g.reserve();
    s2.start(Conn { id: 2 }, move |c| { let r = stuck(&c); }, move |c| { console.log(&202); });
    return 7;
  });
"#;

const STUCK: &str = r#"
fn stuck(c: &Conn) -> Int
  intent "s"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let (tx, rx) = task.channel();
  return rx.recv();
}
"#;

/// 起動したルートが進行不能（デッドロック）なら、親ではなくそのルートを止め、`on_abort` に接続を渡す
/// （隔離。PR #397 の指摘）。
#[test]
fn deadlocked_started_root_is_stopped_instead_of_the_parent() {
    let o = ok(&format!(
        "{STUCK}\nfn run() -> Int\n  intent \"r\"\n  requires none\n  ensures none\n  effects console, task.spawn, task.suspend, task.share\n{{{STUCK_ROOTS}}}\n"
    ));
    assert_eq!((o.value.as_str(), o.output.as_str()), ("7", "1\n202\n"));
    assert_eq!(count(&o.drops, "Conn { id: 2 }"), 1, "{:?}", o.drops);
}

/// 実時間モード（`raw.io_wait_fd` を一度登録した後）でも同じ。待つ相手は読める普通のファイルの記述子。
#[test]
fn deadlocked_started_root_is_stopped_in_real_time_mode_too() {
    use std::os::fd::AsRawFd;
    let file = std::fs::File::open(env!("CARGO_MANIFEST_PATH")).expect("読めるファイル");
    let head = HEAD.replace(
        "module m.a\n  targets server\n  trust none",
        "module std.os.t\n  targets server\n  trust base",
    );
    let src = format!(
        "{head}{STUCK}\npub fn run(fd: Int) -> Int\n  intent \"r\"\n  requires none\n  ensures none\n  effects console, task.spawn, task.suspend, task.share\n  unchecked \"試験。ハーネスが開いたファイルの記述子を待つだけ\"\n{{\n  let w = raw.io_wait_fd(fd, 1);\n{STUCK_ROOTS}}}\n"
    );
    let entry = "module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 7;\n}\n";
    let o = Options {
        embedded_extra: vec![("std.os.t".to_string(), src)],
        embedded_roots: vec!["std.os.t".into()],
        trace_drops: true,
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(
        &[entry],
        "std.os.t.run",
        &[sigil::exec::Arg::Int(i64::from(file.as_raw_fd()))],
        &o,
    )
    .unwrap_or_else(|e| panic!("{:?}\n{}", e.error, e.output));
    assert_eq!((r.value.as_str(), r.output.as_str()), ("7", "1\n202\n"));
    assert_eq!(
        count(&r.drops, "std.os.t.Conn { id: 2 }"),
        1,
        "{:?}",
        r.drops
    );
}

/// 正常に終わったルート（ここでは `on_abort`）が自分の通信路に残した値は、スロットを返す時点で破棄される
/// （要求ごとに積み上がらない。PR #397 の指摘）。上限 1 なので、次の `reserve` が戻るのは前のルートの後。
#[test]
fn values_left_in_a_finished_roots_channel_are_dropped_when_the_slot_returns() {
    let o = ok(r#"
fn keep(c: Conn)
  intent "k"
  requires none
  ensures none
  effects task.share, task.suspend
{
  let (tx, rx) = task.channel();
  tx.send(c);
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share
{
  return task.roots(1, |g| {
    let mut i = 1;
    while i <= 3 {
      let s = g.reserve();
      if true {
        let m = Tmp { v: 100 + i };
      }
      s.start(Conn { id: i }, move |c| { let z = 1 / (c.id - c.id); }, move |c| { keep(c); });
      i = i + 1;
    }
    return 7;
  });
}
"#);
    assert_eq!(o.value, "7");
    let d = &o.drops;
    for i in 1..=3 {
        assert_eq!(count(d, &format!("Conn {{ id: {i} }}")), 1, "{d:?}");
    }
    assert!(pos(d, "Conn { id: 1 }") < pos(d, "Tmp { v: 102 }"), "{d:?}");
    assert!(pos(d, "Conn { id: 2 }") < pos(d, "Tmp { v: 103 }"), "{d:?}");
}

/// 回収した通信路・`Lock` を、ルートの外へ出た持ち手で後から使うと、黙って値を失わずに停止する。
#[test]
fn using_a_reclaimed_handle_stops_explicitly() {
    for (make, give, use_it) in [
        (
            "let (t2, r2) = task.channel();",
            "c.send(t2);",
            "let t2 = rx.recv();\n    t2.send(5);",
        ),
        (
            "let l2 = Lock.new(5);",
            "c.send(l2);",
            "let l2 = rx.recv();\n    match l2.acquire() {\n      Ok(gd) => { return gd.get() + 0; }\n      Err(e) => { return 0 - 1; }\n    }",
        ),
    ] {
        let (kind, detail, _, _) = stop(&format!(
            r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share
{{
  return task.roots(1, |g| {{
    let (tx, rx) = task.channel();
    let s = g.reserve();
    s.start(tx, move |c| {{ {make} {give} }}, move |c| {{ }});
    let s2 = g.reserve();
    {use_it}
    return 7;
  }});
}}
"#
        ));
        assert_eq!(kind, StopKind::StdRequiresViolated, "{detail}");
        assert!(detail.contains("回収済み"), "{detail}");
    }
}

/// ネイティブの漏れの試験: 異常停止したルートを含めて、終わった時点で生きている塊が無く、停止の報告は標準エラーに
/// 1 件だけ出る。並列のワーカーでも値と漏れは同じ（ワーカーの数で結果が変わらない）。
#[test]
fn native_no_blocks_leak_after_isolated_abort_with_one_or_more_workers() {
    let src = format!(
        r#"{HEAD}{WORK}
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend, task.share, clock
{{
  return task.roots(4, |g| {{
    let mut i = 1;
    while i <= 3 {{
      let s = g.reserve();
      s.start(Conn {{ id: i }}, move |c| {{ let r = work(&c); }}, move |c| {{ let n = c.id; }});
      i = i + 1;
    }}
    return 7;
  }});
}}
"#
    );
    let dir = common::native::scratch("roots-leak");
    let exe = dir.join("prog");
    common::native::try_build(&[&src], "run", &exe, false).expect("ビルド");
    for workers in ["1", "4"] {
        let r = common::native::exec_env(
            &exe,
            &[],
            &[("SIGIL_RT_LIVE", "1"), ("SIGIL_WORKERS", workers)],
        );
        assert_eq!(r.code, Some(0), "{workers}: {}", r.stderr);
        assert_eq!(r.stdout, "7\n", "{workers}: {}", r.stderr);
        assert!(
            r.stderr.contains("live: sigil_blocks=0"),
            "{workers}: {}",
            r.stderr
        );
        assert_eq!(
            r.stderr.matches("0 での除算").count(),
            1,
            "{workers}: {}",
            r.stderr
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 起動したルートの中に範囲（子のタスク）があっても、集まりの取り消し（`task.timeout`）で全部が巻き戻され、
/// 外側の取り消しも正しく終わる（ネイティブで、入れ子の取り消しの状態を退避する）。
#[test]
fn cancelling_the_group_unwinds_scopes_inside_started_roots() {
    let o = ok(r#"
fn nap(c: &Conn)
  intent "n"
  requires none
  ensures none
  effects clock, task.suspend, task.spawn
{
  let id = c.id;
  let v = task.scope(|s| {
    let h = s.spawn(move || {
      let t = Tmp { v: id };
      task.sleep(1000 + id);
      return id;
    });
    task.yield();
    return h.join();
  });
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, clock, task.spawn, task.suspend
{
  let r = task.timeout(50, || task.roots(2, |g| {
    let mut i = 1;
    while i <= 4 {
      let s = g.reserve();
      s.start(Conn { id: i }, move |c| { nap(&c); }, move |c| { console.log(&(200 + c.id)); });
      i = i + 1;
    }
    return 7;
  }));
  match r {
    Some(v) => { return v; }
    None => { return 0 - 1; }
  }
}
"#);
    assert_eq!((o.value.as_str(), o.output.as_str()), ("-1", ""));
    for i in 1..=2 {
        assert_eq!(
            count(&o.drops, &format!("Conn {{ id: {i} }}")),
            1,
            "{:?}",
            o.drops
        );
        assert_eq!(
            count(&o.drops, &format!("Tmp {{ v: {i} }}")),
            1,
            "{:?}",
            o.drops
        );
    }
}

/// 起動したルートが作った `Lock` の持ち手を親へ渡し、そのルートが持ったまま異常停止したら、待っている親を
/// `Err(LockBroken)` で起こす（待機列から黙って外さない。PR #398 の指摘）。
#[test]
fn a_parent_waiting_on_a_lock_of_an_aborted_root_gets_lock_broken() {
    let o = ok(LOCK_TO_PARENT);
    assert_eq!((o.value.as_str(), o.output.as_str()), ("7", "3\n200\n"));
}

const LOCK_TO_PARENT: &str = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend, task.share
{
  return task.roots(1, |g| {
    let (tx, rx) = task.channel();
    let s = g.reserve();
    s.start(tx, move |c| {
      let l = Lock.new(5);
      c.send(l.clone());
      match l.acquire() {
        Ok(gd) => {
          task.yield();
          task.yield();
          let q = 10 / (gd.get() - 5);
        }
        Err(e) => { console.log(&9); }
      }
    }, move |c| { console.log(&200); });
    let l2 = rx.recv();
    match l2.acquire() {
      Ok(gd) => { console.log(&2); }
      Err(e) => { console.log(&3); }
    }
    return 7;
  });
}
"#;

/// 同じ形を並列のワーカーで: 取得の順は決まらない（親が先に取れば `Ok`）が、親は待ち続けずに終わり、漏れもない。
#[test]
fn native_parent_waiting_on_a_broken_lock_finishes_with_several_workers() {
    let dir = common::native::scratch("roots-lock");
    let exe = dir.join("prog");
    common::native::try_build(&[&format!("{HEAD}{LOCK_TO_PARENT}")], "run", &exe, false)
        .expect("ビルド");
    for _ in 0..5 {
        let r =
            common::native::exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1"), ("SIGIL_WORKERS", "4")]);
        assert_eq!(r.code, Some(0), "{}", r.stderr);
        assert!(r.stdout.ends_with("7\n"), "{}", r.stdout);
        assert!(r.stderr.contains("live: sigil_blocks=0"), "{}", r.stderr);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 起動したルートの範囲の子が異常停止すると、そのルートの全タスク（兄弟の子と範囲を持つルートのタスク）が
/// 巻き戻され、`on_abort` が動く（ネイティブで、片付け済みの失敗したタスクの停止を待ち続けない）。
const CHILD_FAILS: &str = r#"
fn boom(n: Int) -> Int
  intent "b"
  requires none
  ensures none
  effects none
{
  let mut i = 0;
  let mut a = 0;
  while i < 2000 {
    a = a + i;
    i = i + 1;
  }
  return a / (n - n);
}

fn work(c: &Conn) -> Int
  intent "w"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  let id = c.id;
  return task.scope(|s| {
    let a = s.spawn(move || boom(id));
    let b = s.spawn(move || boom(id));
    let d = s.spawn(move || boom(id));
    return a.join() + b.join() + d.join();
  });
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend
{
  return task.roots(4, |g| {
    let mut i = 1;
    while i <= 6 {
      let s = g.reserve();
      s.start(Conn { id: i }, move |c| { let r = work(&c); }, move |c| { console.log(&(200 + c.id)); });
      i = i + 1;
    }
    return 7;
  });
}
"#;

#[test]
fn a_failing_child_in_a_scope_stops_its_root_only() {
    let o = ok(CHILD_FAILS);
    assert_eq!(o.value, "7");
    assert_eq!(o.output, "201\n202\n203\n204\n205\n206\n");
}

/// 並列のワーカーで、同じルートの子が同時に異常停止しても、そのルートの停止は 1 つのワーカーだけが行う
/// （PR #398 の指摘）。毎回、全部の `on_abort` が 1 回ずつ動き、漏れもデッドロックもない。
#[test]
fn simultaneous_failures_in_one_root_are_handled_once_with_several_workers() {
    let dir = common::native::scratch("roots-race");
    let exe = dir.join("prog");
    common::native::try_build(&[&format!("{HEAD}{CHILD_FAILS}")], "run", &exe, false)
        .expect("ビルド");
    for _ in 0..10 {
        let r =
            common::native::exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1"), ("SIGIL_WORKERS", "4")]);
        assert_eq!(r.code, Some(0), "{}", r.stderr);
        let mut ids: Vec<&str> = r.stdout.lines().collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            ["201", "202", "203", "204", "205", "206", "7"],
            "{}",
            r.stdout
        );
        assert!(r.stderr.contains("live: sigil_blocks=0"), "{}", r.stderr);
        assert_eq!(r.stderr.matches("異常停止").count(), 6, "{}", r.stderr);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 親が取った守りが指す `Lock` の値は、作ったルートが異常停止しても、守りを放すまで捨てない（PR #398 の指摘）。
#[test]
fn a_guard_held_by_the_parent_keeps_the_value_after_the_creator_aborts() {
    let o = ok(r#"
fn z0() -> Int
  intent "z"
  requires none
  ensures none
  effects none
{
  return 0;
}

fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects console, task.spawn, task.suspend, task.share
{
  return task.roots(1, |g| {
    let (tx, rx) = task.channel();
    let s = g.reserve();
    s.start(tx, move |c| {
      let l = Lock.new(Tmp { v: 41 });
      c.send(l.clone());
      task.yield();
      task.yield();
      let z = 1 / z0();
    }, move |c| { console.log(&200); });
    let l2 = rx.recv();
    match l2.acquire() {
      Ok(gd) => {
        task.yield();
        task.yield();
        task.yield();
        console.log(&(gd.get().v + 1));
      }
      Err(e) => { console.log(&3); }
    }
    return 7;
  });
}
"#);
    assert_eq!((o.value.as_str(), o.output.as_str()), ("7", "200\n42\n"));
    assert_eq!(count(&o.drops, "Tmp { v: 41 }"), 1, "{:?}", o.drops);
}

/// 起動したが一度も動いていない（中断しない）`on_abort` を、集まりの取り消しで捨てるときも、接続を破棄する
/// （PR #398 の指摘）。
#[test]
fn an_unstarted_on_abort_drops_the_conn_when_the_group_is_cancelled() {
    let body = r#"
fn run() -> Int
  intent "r"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.roots(2, |g| {
    let s = g.reserve();
    s.start(Conn { id: 1 }, move |c| { let z = 1 / (c.id - c.id); }, move |c| { let n = c.id; });
    task.yield();
    let mut z = 0;
    return 1 / z;
  });
}
"#;
    // 隔離した停止の報告は、ネイティブでは標準エラーに出る（評価器の捕まえた実行では出ない）ので、
    // 突き合わせの代わりに、評価器とネイティブの破棄の記録をそれぞれ比べる。
    let src = format!("{HEAD}{body}");
    let e = sigil::ir_eval::run_with(&src, "run", &[], &opts()).expect_err("止まる");
    assert_eq!(count(&e.drops, "Conn { id: 1 }"), 1, "{:?}", e.drops);
    let (ran, drops) = common::native::native_drops(&[&src], "run", &[]);
    assert_eq!(ran.code, Some(70), "{}", ran.stderr);
    assert_eq!(drops, e.drops.to_vec(), "{}", ran.stderr);
}

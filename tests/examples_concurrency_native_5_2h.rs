//! ADR 0030 5-2h: 並行処理のサンプル `examples/06-concurrency` を、評価器とネイティブ出力で突き合わせる。
//!
//! 突き合わせは `common::run*` が行う（値・標準出力・停止の報告・終了コード）。`scripts/native-report.sh` の
//! 「突き合わせ」の件数にも数えられる。
//!
//! # サンプルの関数ごとの扱い（ADR 0030 補足 D-3 の差の表と整合）
//!
//! | 関数 | 扱い | 理由 |
//! |---|---|---|
//! | `sum_of_doubles` | 突き合わせる | `task.scope`・`spawn`・`join` |
//! | `relay` | 突き合わせる | チャネル・`task.yield` |
//! | `publish` | 突き合わせる | `Atomic` の `store`・`load` |
//! | `handoff` | 突き合わせる | `Atomic` の `clone` を共有する待つタスクと書くタスク |
//! | `bump` | 突き合わせない（不具合 #366） | 引数 `&Lock<Int>` を入口にすると、評価器は整数を初期値の `Lock` にするが、ネイティブの実行ファイルは SIGSEGV で落ちる。直さず Issue にした |
//! | `first_within` | 突き合わせない（固定のみ） | `task.timeout` はネイティブ未対応（ビルドで止める。補足 D-3） |
//! | `double`・`slow` | 単独では入口にしない | 上の関数の部品 |
//!
//! `Lock` の取得・解放・壊れた `Lock`（`Err(LockBroken)`）は、サンプルの入口からは突き合わせられないので、
//! `tests/native_tasks_5_2d.rs`・`tests/native_stop_5_2g.rs` が引数なしの別のプログラムで固定している。

mod common;

use common::native::try_build;
use common::run;
use sigil::exec::Arg;

fn src() -> String {
    std::fs::read_to_string(format!(
        "{}/examples/06-concurrency/concurrency.sigil",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// 評価器とネイティブで走らせ、評価器の値を返す（食い違えば `common` が落とす）。
fn both(f: &str, args: &[i64]) -> String {
    let a: Vec<Arg> = args.iter().map(|&n| Arg::Int(n)).collect();
    run(&src(), f, &a)
        .unwrap_or_else(|e| panic!("{f}: {:?}", e.error))
        .value
}

#[test]
fn scope_spawn_and_join_match() {
    assert_eq!(both("sum_of_doubles", &[3, 4]), "14");
    assert_eq!(both("sum_of_doubles", &[0, -5]), "-10");
}

#[test]
fn channel_relay_matches() {
    assert_eq!(both("relay", &[10]), "33");
    assert_eq!(both("relay", &[0]), "3");
}

#[test]
fn atomic_roundtrip_matches() {
    assert_eq!(both("publish", &[9]), "9");
}

#[test]
fn atomic_handoff_between_tasks_matches() {
    assert_eq!(both("handoff", &[5]), "5");
}

/// `task.timeout` は、評価器では動き、ネイティブはビルドで止める（補足 D-3。この差を固定する）。
#[test]
fn timeout_is_evaluator_only() {
    let s = src();
    let o = sigil::ir_eval::run(&s, "first_within", &[Arg::Int(50), Arg::Int(7)])
        .unwrap_or_else(|e| panic!("{:?}", e.error));
    assert_eq!(o.value, "None");
    let out = common::native::scratch("ex06-timeout").join("prog");
    let e = try_build(&[&s], "first_within", &out, false).unwrap_err();
    assert!(e.contains("未対応"), "{e}");
}

/// ADR 0007 決定 6 の条件 10 の P1-b: 並行処理を使わないプログラム（核の部分だけ）の実行ファイルは、
/// 実行の割り当て・状態機械・I/O スレッドのコードを含まない。
#[test]
fn task_free_program_does_not_depend_on_the_scheduler() {
    let ctl = std::fs::read_to_string(format!(
        "{}/examples/05-control/control.sigil",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let dir = common::native::scratch("ex05-nm");
    let exe = dir.join("prog");
    try_build(&[&ctl], "sum_range", &exe, false).unwrap();
    let nm = |args: &[&str]| {
        let o = std::process::Command::new("nm")
            .args(args)
            .arg(&exe)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let all = nm(&[]);
    for bad in [
        "sched",
        "sigil_step",
        "sigil_dstep",
        "worker_run",
        "io_loop",
        "epoll",
    ] {
        assert!(
            !all.contains(bad),
            "並行処理のコードが入っている（{bad}）:\n{all}"
        );
    }
    // 対照: 並行処理を使うサンプルの実行ファイルには、実行の割り当てが入る（上の検査が空振りでないことの確かめ）。
    let s = src();
    let exe2 = dir.join("prog2");
    try_build(&[&s], "relay", &exe2, false).unwrap();
    let o = std::process::Command::new("nm")
        .arg(&exe2)
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("sched"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR 0007 決定 6 の条件 8 の P1-b: 借用の引数を持つ中断する関数が、借用を保ったまま `task.yield()` で
/// 中断・再開し、別のタスクが動いたあとでも借用先の値が正しい（評価器と一致する）。
#[test]
fn borrow_is_kept_across_a_suspend_and_matches() {
    let body = r#"module m.a
  targets server
  trust none

fn fill(xs: &mut List<Int>, n: Int) -> Int
  intent "xs に n を足し、譲ってから、もう一度足し、長さを返す"
  requires none
  ensures none
  effects task.suspend
{
  xs.push(n);
  task.yield();
  xs.push(n + 1);
  task.yield();
  return xs.len();
}

fn peek(xs: &List<Int>) -> Int
  intent "譲ってから xs の長さを返す"
  requires none
  ensures none
  effects task.suspend
{
  task.yield();
  return xs.len();
}

fn run(n: Int) -> Int
  intent "借用を持つ中断する関数を、別のタスクと交互に動かす"
  requires none
  ensures none
  effects task.spawn, task.suspend
{
  return task.scope(|s| {
    let a = s.spawn(move || {
      let mut xs = List.new();
      let k = fill(&mut xs, n);
      return k * 100 + peek(&xs);
    });
    let b = s.spawn(move || {
      let mut ys = List.new();
      ys.push(1);
      return fill(&mut ys, 7);
    });
    return a.join() * 10 + b.join();
  });
}
"#;
    let o = run(body, "run", &[Arg::Int(3)]).unwrap_or_else(|e| panic!("{:?}", e.error));
    assert_eq!(o.value, "2023");
}

//! Issue #454 / ADR 0033 B1・F1: 非 Copyable の共有参照と環境変数の公開。
mod common;

use sigil::check::check;
use sigil::exec::{Arg, Options};

fn src(body: &str) -> String {
    format!("module m.a\n  targets server\n  trust none\n\n{body}\n")
}

const BORROW: &str = r#"
struct Config {
  label: String,
  routes: List<Int>,
}
fn read(c: &SyncShared<Config>) -> Int
  intent "共有された設定をコピーせず読む"
  requires none
  ensures none
  effects task.share
{
  let r = c.borrow();
  return r.label.len() + r.routes.sum();
}
fn run() -> Int
  intent "非 Copyable な内容を複数のハンドルから読む"
  requires none
  ensures none
  effects task.share
{
  let mut routes = List.new();
  routes.push(10);
  routes.push(20);
  let c = SyncShared.new(Config { label: "設定", routes: routes });
  let d = c.clone();
  return read(&c) + read(&d);
}
"#;

#[test]
fn borrow_non_copyable_config_matches_native() {
    let opts = Options {
        trace_drops: true,
        ..Options::captured()
    };
    let out = common::run_with(&src(BORROW), "run", &[], &opts).unwrap();
    assert_eq!(out.value, "64");
}

const ENV: &str = r#"
fn run(name: String) -> Option<String>
  intent "環境変数を読む"
  requires none
  ensures none
  effects env
{
  return env.get(&name);
}
"#;

#[test]
fn env_override_and_native_agree() {
    let opts = Options {
        env_vars: Some(
            [
                ("SIGIL_454_CONFIG".into(), "こんにちは=a=b".into()),
                ("SIGIL_454_EMPTY".into(), "".into()),
            ]
            .into(),
        ),
        ..Options::captured()
    };
    for (name, want) in [
        ("SIGIL_454_CONFIG", "Some(\"こんにちは=a=b\")"),
        ("SIGIL_454_EMPTY", "Some(\"\")"),
        ("SIGIL_454_MISSING", "None"),
        ("", "None"),
        ("BAD=NAME", "None"),
        ("PATH", "None"),
        ("SIGIL_WORKERS", "None"),
    ] {
        let out = common::run_with(&src(ENV), "run", &[Arg::Str(name.into())], &opts).unwrap();
        assert_eq!(out.value, want, "{name}");
    }
}

#[test]
fn env_default_reads_process_environment() {
    // 環境は書き換えず、評価器と子プロセスが継承する同じ表を読む。
    let expected = std::env::var_os("PATH").map(|s| s.to_string_lossy().into_owned());
    let out = common::run(&src(ENV), "run", &[Arg::Str("PATH".into())]).unwrap();
    let expected = expected.map_or("None".into(), |s| format!("Some({s:?})"));
    assert_eq!(out.value, expected);
}

#[test]
fn effects_are_exact_and_env_is_os_only() {
    check(&src(BORROW)).unwrap();
    check(&src(ENV)).unwrap();
    assert!(check(&src(&BORROW.replace("effects task.share", "effects none"))).is_err());
    assert!(check(&src(&ENV.replace("effects env", "effects none"))).is_err());
    assert!(check(&src(&ENV.replace("effects env", "effects env, console"))).is_err());
    assert!(check(&src(ENV).replace("targets server", "targets browser")).is_err());
}

#[test]
fn borrow_cannot_escape_or_mutate_the_handle() {
    let escape = r#"
fn run() -> &String
  intent "局所の参照を逃がさない"
  requires none
  ensures none
  effects task.share
{
  let c = SyncShared.new("value");
  return c.borrow();
}
"#;
    assert!(check(&src(escape)).is_err());
    let write = BORROW.replace(
        "return r.label.len() + r.routes.sum();",
        "r.routes.push(40);\n  return 0;",
    );
    assert!(check(&src(&write)).is_err());
}

#[test]
fn borrow_can_cross_suspension_and_be_read_in_a_task() {
    let body = BORROW.replace("effects task.share\n{\n  let mut routes", "effects task.share, task.spawn, task.suspend\n{\n  let mut routes").replace(
        "return read(&c) + read(&d);",
        "let r = c.borrow();\n  task.yield();\n  return task.scope(|s| {\n    let child = d.clone();\n    let h = s.spawn(move || read(&child));\n    return r.routes.sum() + h.join();\n  });",
    );
    assert_eq!(common::run(&src(&body), "run", &[]).unwrap().value, "62");
}

#[test]
fn copyable_get_remains_available() {
    let body = r#"
fn run() -> Int
  intent "既存の Copyable 読み出しを保つ"
  requires none
  ensures none
  effects task.share
{
  let c = SyncShared.new(21);
  return c.get() + c.borrow();
}
"#;
    assert_eq!(common::run(&src(body), "run", &[]).unwrap().value, "42");
}

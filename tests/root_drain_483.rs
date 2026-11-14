//! Issue #483: 受付本体が戻った瞬間から猶予を計り、ルートをドレインする。
mod common;
use sigil::exec::Options;

fn source(grace: i64, delay: i64, body_delay: i64) -> String {
    format!(
        r#"module m.a
  targets server
  trust none

struct Conn {{ id: Int, }}

fn run() -> Int
  intent "停止の合図後のドレイン"
  requires none
  ensures none
  effects clock, console, task.spawn, task.suspend
{{
  return task.roots_grace(1, {grace}, |g| {{
    let slot = g.reserve();
    slot.start(Conn {{ id: 1 }}, move |c| {{
      task.sleep({delay});
      console.log(&c.id);
    }}, move |c| {{ console.log(&99); }});
    task.sleep({body_delay});
    console.log(&2);
    return 7;
  }});
}}
"#
    )
}

fn run(grace: i64, delay: i64, body_delay: i64) -> sigil::exec::Outcome {
    common::run_with(
        &source(grace, delay, body_delay),
        "run",
        &[],
        &Options {
            trace_drops: true,
            ..Options::captured()
        },
    )
    .unwrap_or_else(|e| panic!("{:?}\n{}", e.error, e.output))
}

#[test]
fn drains_existing_connection_and_keeps_body_result() {
    let o = run(100, 20, 1);
    assert_eq!(o.value, "7");
    assert_eq!(o.output, "2\n1\n");
    assert_eq!(
        o.drops
            .iter()
            .filter(|x| x.as_str() == "Conn { id: 1 }")
            .count(),
        1
    );
}

#[test]
fn deadline_cancels_connection_without_on_abort() {
    let o = run(10, 1000, 1);
    assert_eq!(o.value, "7");
    assert_eq!(o.output, "2\n");
    assert_eq!(
        o.drops
            .iter()
            .filter(|x| x.as_str() == "Conn { id: 1 }")
            .count(),
        1
    );
}

#[test]
fn grace_starts_after_accept_body_returns() {
    let o = run(20, 110, 100);
    assert_eq!(o.value, "7");
    assert_eq!(o.output, "2\n1\n");
}

#[test]
fn already_finished_group_returns_immediately() {
    // 先に全ルートが完了している場合も、不要な期限を残さない。
    let o = run(10, 1, 100);
    assert_eq!(o.output, "1\n2\n");
    assert_eq!(o.value, "7");
}

#[test]
fn rejects_unbounded_grace() {
    for ms in [0, -1, 300001] {
        let e = sigil::ir_eval::run_with(&source(ms, 1, 0), "run", &[], &Options::captured())
            .unwrap_err();
        assert!(format!("{:?}", e.error).contains("grace_ms"));
    }
}

#[test]
fn empty_group_does_not_install_a_timer() {
    let src = source(10, 1, 0);
    let start = src.find("    let slot = g.reserve();").unwrap();
    let end = src.find("    task.sleep(0);").unwrap();
    let mut empty = src;
    empty.replace_range(start..end, "");
    let o = common::run_with(&empty, "run", &[], &Options::captured()).unwrap();
    assert_eq!(o.value, "7");
    assert_eq!(o.output, "2\n");
}

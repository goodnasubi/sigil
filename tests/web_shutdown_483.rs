//! ADR 0033 K: 停止の注入で、受付（reserve 待機を含む）中断とドレインを検証する。
use sigil::exec::Options;

fn run(accept: &str, stop: &str, grace: i64) -> sigil::exec::Outcome {
    let src = format!(r#"module m.shutdown
  targets server
  trust none

struct Conn {{ id: Int, }}

fn accept(g: &RootGroup)
  intent "受付と処理を模擬する"
  requires none
  ensures none
  effects task.spawn, task.suspend, clock, console
{{
{accept}
}}

fn stop()
  intent "停止を注入する"
  requires none
  ensures none
  effects task.suspend, clock
{{
{stop}
}}

fn run() -> Int
  intent "停止後に正常復帰する"
  requires none
  ensures none
  effects task.spawn, task.suspend, clock, console
{{
  web.run_until(1, {grace}, accept, stop);
  console.log(&99);
  return 0;
}}
"#);
    sigil::ir_eval::run_with(&src, "run", &[], &Options { trace_drops: true, ..Options::captured() })
        .unwrap_or_else(|e| panic!("{e:?}"))
}

const ACCEPT: &str = r#"
  let slot = g.reserve();
  slot.start(Conn { id: 1 }, |c| {
    task.sleep(20);
    console.log(&c.id);
  }, |c| { console.log(&(100 + c.id)); });
  // 上限に達しているため、この reserve は停止時に取り消される。
  let blocked = g.reserve();
  console.log(&777);
"#;

#[test]
fn stop_cancels_reserve_then_drains_existing_root() {
    let o = run(ACCEPT, "  task.sleep(5);", 30);
    assert_eq!(o.value, "0");
    assert_eq!(o.output, "1\n99\n");
    assert_eq!(o.drops.iter().filter(|d| d.as_str() == "Conn { id: 1 }").count(), 1);
}

#[test]
fn grace_expires_and_cancels_existing_root() {
    let o = run(ACCEPT, "  task.sleep(5);", 3);
    assert_eq!(o.value, "0");
    assert!(!o.output.contains("777"));
    assert!(!o.output.lines().any(|s| s == "1"));
    assert!(o.output.ends_with("99\n"));
    assert_eq!(o.drops.iter().filter(|d| d.as_str() == "Conn { id: 1 }").count(), 1);
}

#[test]
fn stop_cancels_an_idle_accept_loop() {
    let o = run("  task.sleep(100000);\n  console.log(&777);", "  task.sleep(1);", 5);
    assert_eq!(o.output, "99\n");
}

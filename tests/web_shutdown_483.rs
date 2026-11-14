//! ADR 0033 K: 停止の注入で、受付（reserve 待機を含む）中断とドレインを検証する。
use sigil::exec::Options;

fn run(accept: &str, stop: &str, grace: i64) -> sigil::exec::Outcome {
    let accept_spawn = if accept.contains("slot.start") {
        "task.spawn, "
    } else {
        ""
    };
    let src = format!(
        r#"module m.shutdown
  targets server
  trust none

struct Conn {{ id: Int, }}

fn accept(g: &RootGroup)
  intent "受付と処理を模擬する"
  requires none
  ensures none
  effects {accept_spawn}task.suspend, clock, console
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
"#
    );
    sigil::ir_eval::run_with(
        &src,
        "run",
        &[],
        &Options {
            trace_drops: true,
            ..Options::captured()
        },
    )
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
    assert_eq!(
        o.drops
            .iter()
            .filter(|d| d.as_str() == "Conn { id: 1 }")
            .count(),
        1
    );
}

#[test]
fn grace_expires_and_cancels_existing_root() {
    let o = run(ACCEPT, "  task.sleep(5);", 3);
    assert_eq!(o.value, "0");
    assert!(!o.output.contains("777"));
    assert!(!o.output.lines().any(|s| s == "1"));
    assert!(o.output.ends_with("99\n"));
    assert_eq!(
        o.drops
            .iter()
            .filter(|d| d.as_str() == "Conn { id: 1 }")
            .count(),
        1
    );
}

#[test]
fn stop_cancels_an_idle_accept_loop() {
    let o = run(
        "  task.sleep(100000);\n  console.log(&777);",
        "  task.sleep(1);",
        5,
    );
    assert_eq!(o.output, "99\n");
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[test]
fn native_term_and_int_interrupt_idle_socket_accept() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let source = r#"module m.shutdown_native
  targets server
  trust none

fn run() -> Int
  intent "最初のtaskより前に終了合図を準備する"
  requires none
  ensures none
  effects net.server, task.spawn, task.suspend, clock, console
{
  match net.shutdown_signal() {
    Err(_) => { return 10; }
    Ok(signal) => {
      match net.listen(&"127.0.0.1", 0, 4) {
        Err(_) => { return 11; }
        Ok(listener) => {
          console.log(&listener.local_port());
          web.run_until(1, 50, |g| {
            while true {
              let slot = g.reserve();
              match listener.accept() {
                Err(_) => { return; }
                Ok(stream) => {
                  slot.start(stream, |c| { task.sleep(10); }, |c| {});
                }
              }
            }
          }, || { signal.wait(); });
        }
      }
    }
  }
  return 0;
}
"#;
    let dir = std::env::temp_dir().join(format!("sigil-shutdown-483-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("shutdown");
    sigil::native::build(&[source], "run", &sigil::codegen::Opts::default(), &exe).unwrap();
    for signal in ["-TERM", "-INT"] {
        let mut child = Command::new(&exe)
            .env("SIGIL_WORKERS", "2")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            let mut line = String::new();
            let result = BufReader::new(stdout).read_line(&mut line);
            tx.send((result, line)).unwrap();
        });
        let (_, line) = match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(result) => result,
            Err(_) => {
                child.kill().unwrap();
                child.wait().unwrap();
                reader.join().unwrap();
                panic!("終了合図の準備が期限を超えた");
            }
        };
        reader.join().unwrap();
        assert!(line.trim().parse::<u16>().unwrap() > 0);
        assert!(Command::new("kill")
            .args([signal, &child.id().to_string()])
            .status()
            .unwrap()
            .success());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{signal}: {status}");
                break;
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{signal}: accept の中断が期限を超えた");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

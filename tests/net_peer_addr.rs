//! Issue #721: TcpStream.peer_addr と local_addr の動作試験。
//!
//! - IPv4（127.0.0.1）での接続相手・ローカルアドレス取得
//! - IPv6（::1）での接続相手・ローカルアドレス取得
//! - 評価器（Eval）とネイティブ（Native 1, 4 ワーカー）の両方で照合

mod common;

use common::native::scratch;
use std::io::{BufRead, BufReader, Read};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Eval,
    Native(u32),
}

const MODES: [Mode; 3] = [Mode::Eval, Mode::Native(1), Mode::Native(4)];

#[allow(dead_code)]
#[derive(Debug)]
struct Done {
    value: String,
    code: Option<i32>,
    stderr: String,
}

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Prog {
    dir: PathBuf,
    file: PathBuf,
    exe: PathBuf,
}

impl Drop for Prog {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Prog {
    fn new(tag: &str, src: &str) -> Prog {
        let dir = scratch(tag);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let file = dir.join("app").join("main.sigil");
        std::fs::write(&file, src).unwrap();
        let exe = dir.join("prog");
        sigil::native::build(&[src], "main", &sigil::codegen::Opts::default(), &exe)
            .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
        Prog { dir, file, exe }
    }

    fn command(&self, mode: Mode, args: &[i64]) -> Command {
        let mut c = match mode {
            Mode::Eval => {
                let mut c = Command::new(env!("CARGO_BIN_EXE_sigil"));
                c.arg("run")
                    .arg("--root")
                    .arg(&self.dir)
                    .arg(&self.file)
                    .arg("main");
                c
            }
            Mode::Native(w) => {
                let mut c = Command::new(&self.exe);
                c.env("SIGIL_WORKERS", w.to_string());
                c
            }
        };
        c.args(args.iter().map(|a| a.to_string()))
            .env("SIGIL_RT_LIVE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn serve<T>(&self, mode: Mode, args: &[i64], client: impl FnOnce(u16, u32) -> T) -> (T, Done) {
        let mut g = Guard(self.command(mode, args).spawn().unwrap());
        let mut out = BufReader::new(g.0.stdout.take().unwrap());
        let mut first = String::new();
        out.read_line(&mut first).unwrap();
        let Ok(port) = first.trim().parse::<u16>() else {
            let mut e = String::new();
            let _ = g.0.stderr.take().unwrap().read_to_string(&mut e);
            panic!("{mode:?}: 待ち受けの番号が出なかった: {first:?}\n{e}");
        };
        let got = client(port, g.0.id());
        (got, finish(mode, g, out))
    }

    fn run(&self, mode: Mode, args: &[i64]) -> Done {
        let mut g = Guard(self.command(mode, args).spawn().unwrap());
        let out = BufReader::new(g.0.stdout.take().unwrap());
        finish(mode, g, out)
    }
}

fn finish(mode: Mode, mut g: Guard, mut out: BufReader<std::process::ChildStdout>) -> Done {
    let start = Instant::now();
    let status = loop {
        if let Some(st) = g.0.try_wait().unwrap() {
            break st;
        }
        if start.elapsed() > Duration::from_secs(60) {
            panic!("{mode:?}: 制限時間内に終わらなかった（ハング）");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let (mut rest, mut stderr) = (String::new(), String::new());
    out.read_to_string(&mut rest).unwrap();
    g.0.stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    Done {
        value: rest
            .trim_end()
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .to_string(),
        code: status.code().or(status.signal().map(|s| 128 + s)),
        stderr,
    }
}

#[test]
fn test_peer_and_local_addr_ipv4() {
    let src = r#"module app.main
  targets server
  trust none

pub fn main() -> Int
  intent "待ち受けを開始し、accept した stream の peer_addr と local_addr を検証してクライアントへ返す"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host = "127.0.0.1";
  match net.listen(&host, 0, 16) {
    Ok(l) => {
      let lport = l.local_port();
      console.log(&lport);
      match l.accept() {
        Ok(s) => {
          match s.peer_addr() {
            Ok(peer) => {
              let (p_host, p_port) = peer;
              match s.local_addr() {
                Ok(local) => {
                  let (l_host, l_port) = local;
                  if p_host != "127.0.0.1" || p_port <= 0 || p_port > 65535 {
                    return 0 - 4;
                  }
                  if l_host != "127.0.0.1" || l_port != lport {
                    return 0 - 5;
                  }
                  let mut resp = Bytes.new();
                  resp.push_u16_be(p_port);
                  match s.write_all(&resp) {
                    Ok(u) => {}
                    Err(e) => { return 0 - 6; }
                  }
                  match s.shutdown() {
                    Ok(u) => {}
                    Err(e) => { return 0 - 7; }
                  }
                  return 0;
                }
                Err(e) => { return 0 - 3; }
              }
            }
            Err(e) => { return 0 - 2; }
          }
        }
        Err(e) => { return 0 - 8; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let p = Prog::new("peer_addr_v4", src);
    for mode in MODES {
        let (_got_port, d) = p.serve(mode, &[], |port, _| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let my_port = s.local_addr().unwrap().port();
            let mut buf = [0u8; 2];
            s.read_exact(&mut buf).unwrap();
            let peer_reported_port = u16::from_be_bytes(buf);
            assert_eq!(peer_reported_port, my_port, "{mode:?}: ポート番号不一致");
            peer_reported_port
        });
        assert_eq!(d.code, Some(0), "{mode:?}: 失敗: {d:?}");
        assert_eq!(d.value, "0", "{mode:?}: 戻り値不一致: {d:?}");
    }
}

#[test]
fn test_peer_and_local_addr_ipv6() {
    let src = r#"module app.main
  targets server
  trust none

pub fn main() -> Int
  intent "::1 で待ち受け、accept した stream の peer_addr と local_addr を検証して返す"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host = "::1";
  match net.listen(&host, 0, 16) {
    Ok(l) => {
      let lport = l.local_port();
      console.log(&lport);
      match l.accept() {
        Ok(s) => {
          match s.peer_addr() {
            Ok(peer) => {
              let (p_host, p_port) = peer;
              match s.local_addr() {
                Ok(local) => {
                  let (l_host, l_port) = local;
                  if p_host != "::1" || p_port <= 0 || p_port > 65535 {
                    return 0 - 4;
                  }
                  if l_host != "::1" || l_port != lport {
                    return 0 - 5;
                  }
                  let mut resp = Bytes.new();
                  resp.push_u16_be(p_port);
                  match s.write_all(&resp) {
                    Ok(u) => {}
                    Err(e) => { return 0 - 6; }
                  }
                  match s.shutdown() {
                    Ok(u) => {}
                    Err(e) => { return 0 - 7; }
                  }
                  return 0;
                }
                Err(e) => { return 0 - 3; }
              }
            }
            Err(e) => { return 0 - 2; }
          }
        }
        Err(e) => { return 0 - 8; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let p = Prog::new("peer_addr_v6", src);
    for mode in MODES {
        let (_got_port, d) = p.serve(mode, &[], |port, _| {
            let mut s = TcpStream::connect(("::1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            let my_port = s.local_addr().unwrap().port();
            let mut buf = [0u8; 2];
            s.read_exact(&mut buf).unwrap();
            let peer_reported_port = u16::from_be_bytes(buf);
            assert_eq!(peer_reported_port, my_port, "{mode:?}: ポート番号不一致");
            peer_reported_port
        });
        assert_eq!(d.code, Some(0), "{mode:?}: 失敗: {d:?}");
        assert_eq!(d.value, "0", "{mode:?}: 戻り値不一致: {d:?}");
    }
}

#[test]
fn test_pure_sigil_client_to_sigil_server_ipv4() {
    let server_src = r#"module app.main
  targets server
  trust none

pub fn main() -> Int
  intent "127.0.0.1 で待ち受け、accept した stream の peer_addr と local_addr を検証してクライアントへ返す"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host = "127.0.0.1";
  match net.listen(&host, 0, 16) {
    Ok(l) => {
      let lport = l.local_port();
      console.log(&lport);
      match l.accept() {
        Ok(s) => {
          match s.peer_addr() {
            Ok(peer) => {
              let (p_host, p_port) = peer;
              match s.local_addr() {
                Ok(local) => {
                  let (l_host, l_port) = local;
                  if p_host != "127.0.0.1" || p_port <= 0 || p_port > 65535 {
                    return 0 - 4;
                  }
                  if l_host != "127.0.0.1" || l_port != lport {
                    return 0 - 5;
                  }
                  let mut resp = Bytes.new();
                  resp.push_u16_be(p_port);
                  match s.write_all(&resp) {
                    Ok(u) => {}
                    Err(e) => { return 0 - 6; }
                  }
                  match s.shutdown() {
                    Ok(u) => {}
                    Err(e) => { return 0 - 7; }
                  }
                  return 0;
                }
                Err(e) => { return 0 - 3; }
              }
            }
            Err(e) => { return 0 - 2; }
          }
        }
        Err(e) => { return 0 - 8; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let client_src = r#"module app.main
  targets server
  trust none

pub fn main(port: Int) -> Int
  intent "127.0.0.1 のサーバーへ接続し、返されたポート番号を受信する"
  requires none
  ensures none
  effects net.client, task.suspend
{
  let host = "127.0.0.1";
  match net.connect(&host, port) {
    Ok(conn) => {
      let mut c = conn;
      match c.read(2) {
        Ok(b) => {
          if b.byte_len() != 2 {
            return 0 - 2;
          }
          let hi = b.get(0).unwrap_or(0);
          let lo = b.get(1).unwrap_or(0);
          return hi * 256 + lo;
        }
        Err(e) => { return 0 - 3; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let s_prog = Prog::new("pure_srv_v4", server_src);
    let c_prog = Prog::new("pure_cli_v4", client_src);
    for mode in MODES {
        let (cli_done, s_done) =
            s_prog.serve(mode, &[], |port, _| c_prog.run(mode, &[port as i64]));
        assert_eq!(s_done.code, Some(0), "{mode:?}: サーバー失敗: {s_done:?}");
        assert_eq!(
            s_done.value, "0",
            "{mode:?}: サーバー戻り値不一致: {s_done:?}"
        );
        assert_eq!(
            cli_done.code,
            Some(0),
            "{mode:?}: クライアント失敗: {cli_done:?}"
        );
        let port_num = cli_done.value.parse::<i64>().unwrap_or(0);
        assert!(
            port_num > 0 && port_num <= 65535,
            "{mode:?}: 不正なポート番号: {port_num}"
        );
    }
}

#[test]
fn test_pure_sigil_client_to_sigil_server_ipv6() {
    let server_src = r#"module app.main
  targets server
  trust none

pub fn main() -> Int
  intent "::1 で待ち受け、accept した stream の peer_addr と local_addr を検証してクライアントへ返す"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host = "::1";
  match net.listen(&host, 0, 16) {
    Ok(l) => {
      let lport = l.local_port();
      console.log(&lport);
      match l.accept() {
        Ok(s) => {
          match s.peer_addr() {
            Ok(peer) => {
              let (p_host, p_port) = peer;
              match s.local_addr() {
                Ok(local) => {
                  let (l_host, l_port) = local;
                  if p_host != "::1" || p_port <= 0 || p_port > 65535 {
                    return 0 - 4;
                  }
                  if l_host != "::1" || l_port != lport {
                    return 0 - 5;
                  }
                  let mut resp = Bytes.new();
                  resp.push_u16_be(p_port);
                  match s.write_all(&resp) {
                    Ok(u) => {}
                    Err(e) => { return 0 - 6; }
                  }
                  match s.shutdown() {
                    Ok(u) => {}
                    Err(e) => { return 0 - 7; }
                  }
                  return 0;
                }
                Err(e) => { return 0 - 3; }
              }
            }
            Err(e) => { return 0 - 2; }
          }
        }
        Err(e) => { return 0 - 8; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let client_src = r#"module app.main
  targets server
  trust none

pub fn main(port: Int) -> Int
  intent "::1 のサーバーへ接続し、返されたポート番号を受信する"
  requires none
  ensures none
  effects net.client, task.suspend
{
  let host = "::1";
  match net.connect(&host, port) {
    Ok(conn) => {
      let mut c = conn;
      match c.read(2) {
        Ok(b) => {
          if b.byte_len() != 2 {
            return 0 - 2;
          }
          let hi = b.get(0).unwrap_or(0);
          let lo = b.get(1).unwrap_or(0);
          return hi * 256 + lo;
        }
        Err(e) => { return 0 - 3; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let s_prog = Prog::new("pure_srv_v6", server_src);
    let c_prog = Prog::new("pure_cli_v6", client_src);
    for mode in MODES {
        let (cli_done, s_done) =
            s_prog.serve(mode, &[], |port, _| c_prog.run(mode, &[port as i64]));
        assert_eq!(s_done.code, Some(0), "{mode:?}: サーバー失敗: {s_done:?}");
        assert_eq!(
            s_done.value, "0",
            "{mode:?}: サーバー戻り値不一致: {s_done:?}"
        );
        assert_eq!(
            cli_done.code,
            Some(0),
            "{mode:?}: クライアント失敗: {cli_done:?}"
        );
        let port_num = cli_done.value.parse::<i64>().unwrap_or(0);
        assert!(
            port_num > 0 && port_num <= 65535,
            "{mode:?}: 不正なポート番号: {port_num}"
        );
    }
}

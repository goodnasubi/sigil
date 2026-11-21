//! ADR 0051: net.listen / net.connect における IPv6 数値アドレス（::1 等）の受け付け。
//!
//! - `::1` での待ち受けと送受信（Rust std::net クライアントからの接続照合）
//! - 純 Sigil クライアント（`net.connect`）から `::1` サーバーへの接続と送受信
//! - IPv4（`127.0.0.1`）と IPv6（`::1`）の共存および V6-only 分離の確認
//! - 不正なアドレス表記（角括弧、IPv4 射影アドレス、複数 :: 等）の拒絶（NetError.Other）
mod common;

use common::native::scratch;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Eval,
    Native(u32),
}

const MODES: [Mode; 3] = [Mode::Eval, Mode::Native(1), Mode::Native(4)];

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
        value: rest.trim_end().rsplit('\n').next().unwrap().to_string(),
        code: status.code().or(status.signal().map(|s| 128 + s)),
        stderr,
    }
}

fn clean(mode: Mode, d: &Done, value: &str) {
    assert_eq!(d.code, Some(0), "{mode:?}: {d:?}");
    assert_eq!(d.value, value, "{mode:?}: {d:?}");
    let want = match mode {
        Mode::Eval => "live: frames=0 chunks=0",
        Mode::Native(_) => "live: sigil_blocks=0",
    };
    assert!(d.stderr.contains(want), "{mode:?}: 漏れがある: {d:?}");
}

/// 1. ::1 での待ち受けと、Rust std::net による接続・エコー送受信
#[test]
fn test_ipv6_listen_echo_roundtrip() {
    let src = r#"module app.main
  targets server
  trust none

fn read_all(s: &TcpStream) -> Result<Bytes, NetError>
  intent "相手が書き込みを終えるまで読む"
  requires none
  ensures none
  effects net.server, task.suspend
{
  let mut all = Bytes.new();
  let mut go = true;
  while go {
    let b = s.read(4096)?;
    if b.is_empty() {
      go = false;
    } else {
      all.extend(&b);
    }
  }
  return Ok(all);
}

pub fn main() -> Int
  intent "::1 で待ち受け、1 接続を受け付けてそのまま返して終了"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host = "::1";
  match net.listen(&host, 0, 16) {
    Ok(l) => {
      console.log(&l.local_port());
      match l.accept() {
        Ok(s) => {
          match read_all(&s) {
            Ok(b) => {
              let len = b.byte_len();
              match s.write_all(&b) {
                Ok(u) => {}
                Err(e) => { return 0 - 3; }
              }
              match s.shutdown() {
                Ok(u) => {}
                Err(e) => { return 0 - 4; }
              }
              return len;
            }
            Err(e) => { return 0 - 2; }
          }
        }
        Err(e) => { return 0 - 5; }
      }
    }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let p = Prog::new("ipv6_listen_echo", src);
    for mode in MODES {
        let (got, d) = p.serve(mode, &[], |port, _| {
            let mut s = TcpStream::connect(("::1", port)).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            s.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
            let msg = b"Hello IPv6 Sigil World!";
            s.write_all(msg).unwrap();
            s.shutdown(Shutdown::Write).unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            buf
        });
        assert_eq!(got, b"Hello IPv6 Sigil World!");
        clean(mode, &d, "23");
    }
}

/// 2. 純 Sigil クライアント（net.connect）から ::1 への接続
#[test]
fn test_ipv6_sigil_connect() {
    let src = r#"module app.main
  targets server
  trust none

pub fn main(port: Int) -> Int
  intent "net.connect で ::1 に接続し、データを送って応答を読む"
  requires none
  ensures none
  effects net.client, task.suspend
{
  let host = "::1";
  match net.connect(&host, port) {
    Ok(conn) => {
      let mut c = conn;
      let mut req = Bytes.new();
      req.push_u32_be(12345678);
      match c.write_all(&req) {
        Ok(u) => {}
        Err(e) => { return 0 - 1; }
      }
      match c.read(16) {
        Ok(resp) => {
          return resp.read_u32_be(0).unwrap_or(0);
        }
        Err(e) => { return 0 - 2; }
      }
    }
    Err(e) => { return 0 - 3; }
  }
}
"#;
    let p = Prog::new("ipv6_sigil_client", src);
    for mode in MODES {
        let listener = std::net::TcpListener::bind("[::1]:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).unwrap();
            assert_eq!(u32::from_be_bytes(buf), 12345678);
            s.write_all(&87654321u32.to_be_bytes()).unwrap();
        });
        let d = p.run(mode, &[port as i64]);
        handle.join().unwrap();
        clean(mode, &d, "87654321");
    }
}

/// 3. 純 Sigil クライアントから純 Sigil サーバー（両方 ::1）への接続と送受信
#[test]
fn test_pure_sigil_client_to_sigil_server_ipv6() {
    let server_src = r#"module app.main
  targets server
  trust none

pub fn main() -> Int
  intent "::1 で待ち受け、接続を受理して文字列を送受信"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host = "::1";
  match net.listen(&host, 0, 16) {
    Ok(l) => {
      console.log(&l.local_port());
      match l.accept() {
        Ok(s) => {
          match s.read(16) {
            Ok(b) => {
              let mut reply = Bytes.new();
              reply.push_u32_be(99999);
              let _ = s.write_all(&reply);
              let _ = s.shutdown();
              return 1;
            }
            Err(e) => { return 0 - 2; }
          }
        }
        Err(e) => { return 0 - 3; }
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
  intent "::1 のサーバーへ接続して送受信"
  requires none
  ensures none
  effects net.client, task.suspend
{
  let host = "::1";
  match net.connect(&host, port) {
    Ok(conn) => {
      let mut c = conn;
      let mut req = Bytes.new();
      req.push_u32_be(11111);
      match c.write_all(&req) {
        Ok(u) => {}
        Err(e) => { return 0 - 1; }
      }
      match c.read(16) {
        Ok(resp) => {
          return resp.read_u32_be(0).unwrap_or(0);
        }
        Err(e) => { return 0 - 2; }
      }
    }
    Err(e) => { return 0 - 3; }
  }
}
"#;
    let s_prog = Prog::new("sigil_srv_ipv6", server_src);
    let c_prog = Prog::new("sigil_cli_ipv6", client_src);
    for mode in MODES {
        let (val, s_done) = s_prog.serve(mode, &[], |port, _| {
            let c_done = c_prog.run(mode, &[port as i64]);
            clean(mode, &c_done, "99999");
            c_done.value
        });
        clean(mode, &s_done, "1");
        assert_eq!(val, "99999");
    }
}

/// 4. IPv4 と IPv6 の共存および V6-only 分離（ADR 0051 決定 2）
#[test]
fn test_ipv4_and_ipv6_coexistence_and_separation() {
    let src = r#"module app.main
  targets server
  trust none

pub fn main() -> Int
  intent "IPv4(127.0.0.1) と IPv6(::1) の両方で待ち受けを開始し、それぞれのポートを出力して接続を1つずつ受け付ける"
  requires none
  ensures none
  effects net.server, console, task.suspend
{
  let host4 = "127.0.0.1";
  let host6 = "::1";
  match net.listen(&host4, 0, 16) {
    Ok(l4) => {
      match net.listen(&host6, 0, 16) {
        Ok(l6) => {
          let p4 = l4.local_port();
          let p6 = l6.local_port();
          console.log(&p4);
          console.log(&p6);
          // IPv4 接続を受理
          match l4.accept() {
            Ok(s4) => {
              match s4.read(16) {
                Ok(b4) => {
                  let _ = s4.write_all(&b4);
                  let _ = s4.shutdown();
                }
                Err(e) => { return 0 - 1; }
              }
            }
            Err(e) => { return 0 - 2; }
          }
          // IPv6 接続を受理
          match l6.accept() {
            Ok(s6) => {
              match s6.read(16) {
                Ok(b6) => {
                  let _ = s6.write_all(&b6);
                  let _ = s6.shutdown();
                }
                Err(e) => { return 0 - 3; }
              }
            }
            Err(e) => { return 0 - 4; }
          }
          return 100;
        }
        Err(e) => { return 0 - 20; }
      }
    }
    Err(e) => { return 0 - 10; }
  }
}
"#;
    let p = Prog::new("ipv4_ipv6_coexist", src);
    for mode in MODES {
        let mut g = Guard(p.command(mode, &[]).spawn().unwrap());
        let mut out = BufReader::new(g.0.stdout.take().unwrap());
        let mut line4 = String::new();
        out.read_line(&mut line4).unwrap();
        let port4: u16 = line4.trim().parse().unwrap();
        let mut line6 = String::new();
        out.read_line(&mut line6).unwrap();
        let port6: u16 = line6.trim().parse().unwrap();

        // 1. IPv6 のポートに対して IPv4(127.0.0.1) から接続を試みる -> IPV6_V6ONLY のため拒絶（または失敗）する
        let v4_to_v6_err = TcpStream::connect_timeout(
            &std::net::SocketAddr::new(
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
                port6,
            ),
            Duration::from_millis(200),
        );
        assert!(
            v4_to_v6_err.is_err(),
            "IPv6-only 待ち受けポートへ IPv4 からの接続は拒絶されるべき"
        );

        // 2. IPv4 接続
        {
            let mut s = TcpStream::connect(("127.0.0.1", port4)).unwrap();
            s.write_all(b"v4data").unwrap();
            s.shutdown(Shutdown::Write).unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            assert_eq!(buf, b"v4data");
        }

        // 3. IPv6 接続
        {
            let mut s = TcpStream::connect(("::1", port6)).unwrap();
            s.write_all(b"v6data").unwrap();
            s.shutdown(Shutdown::Write).unwrap();
            let mut buf = Vec::new();
            s.read_to_end(&mut buf).unwrap();
            assert_eq!(buf, b"v6data");
        }

        let d = finish(mode, g, out);
        clean(mode, &d, "100");
    }
}

/// 5. 不正なアドレス形式の拒絶（NetError.Other）
#[test]
fn test_ipv6_invalid_address_formats() {
    let src = r#"module app.main
  targets server
  trust none

fn check_invalid(host: String) -> Bool
  intent "不正な host が NetError.Other になることを確認"
  requires none
  ensures none
  effects net.server
{
  match net.listen(&host, 0, 16) {
    Ok(l) => { return false; }
    Err(e) => {
      match e {
        NetError.Other => { return true; }
        NetError.AddrInUse => { return false; }
        NetError.PermissionDenied => { return false; }
        NetError.Closed => { return false; }
      }
    }
  }
}

pub fn main() -> Int
  intent "各種不正なアドレス表記が正しく拒絶されるか確認"
  requires none
  ensures none
  effects net.server
{
  // 角括弧
  if !check_invalid("[::1]") { return 0 - 1; }
  // IPv4 射影アドレス
  if !check_invalid("::ffff:127.0.0.1") { return 0 - 2; }
  if !check_invalid("::ffff:7f00:1") { return 0 - 20; }
  if !check_invalid("0:0:0:0:0:ffff:7f00:1") { return 0 - 21; }
  // 複数 ::
  if !check_invalid("1::2::3") { return 0 - 3; }
  // 3連続コロン
  if !check_invalid(":::1") { return 0 - 4; }
  // 不足グループ（:: なし）
  if !check_invalid("1:2:3") { return 0 - 5; }
  // 超過グループ
  if !check_invalid("1:2:3:4:5:6:7:8:9") { return 0 - 6; }
  // 5桁 hex
  if !check_invalid("12345::1") { return 0 - 7; }
  // 不正文字
  if !check_invalid("fe80::1%eth0") { return 0 - 8; }
  if !check_invalid("not:valid:ipv6") { return 0 - 9; }
  // 単一コロン境界
  if !check_invalid(":1:2:3:4:5:6:7:8") { return 0 - 10; }
  if !check_invalid("1:2:3:4:5:6:7:8:") { return 0 - 11; }
  // :: で 8 グループ（圧縮対象 0 グループ）
  if !check_invalid("1:2:3:4::5:6:7:8") { return 0 - 12; }
  // 不正 IPv4
  if !check_invalid("127.0.0.999") { return 0 - 13; }
  return 42;
}
"#;
    let p = Prog::new("ipv6_invalid_formats", src);
    for mode in MODES {
        let d = p.run(mode, &[]);
        clean(mode, &d, "42");
    }
}

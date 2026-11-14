//! ADR 0034 / #473: 外部模擬サーバーとの Startup/SCRAM 接続。
//! 外部相手との同じ入出力を評価器・native（1/4 workers）で比べる。
//! 模擬サーバーは応答列だけを送り、Python標準暗号を独立した証明・署名の検算に使う。
mod common;

use common::native::scratch;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const MODES: [u32; 3] = [0, 1, 4];

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
    fn new(src: &str, extra: &[(String, String)]) -> Self {
        let dir = scratch("pg-startup");
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let file = dir.join("app/main.sigil");
        std::fs::write(&file, src).unwrap();
        let exe = dir.join("prog");
        let opts = sigil::effects::Options {
            embedded_extra: extra.to_vec(),
            ..Default::default()
        };
        let p = sigil::ir_eval::lower_sources_opts(&[src], "main", &opts).unwrap();
        sigil::verify::verify(&p).unwrap();
        sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
        Self { dir, file, exe }
    }
    fn spawn(&self, mode: u32, args: &[i64]) -> Guard {
        let mut c = if mode == 0 {
            let mut c = Command::new(env!("CARGO_BIN_EXE_sigil"));
            c.arg("run")
                .arg("--root")
                .arg(&self.dir)
                .arg(&self.file)
                .arg("main");
            c
        } else {
            let mut c = Command::new(&self.exe);
            c.env("SIGIL_WORKERS", mode.to_string());
            c
        };
        Guard(
            c.args(args.iter().map(ToString::to_string))
                .env("SIGIL_RT_LIVE", "1")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        )
    }
}
fn finish(mut child: Guard, expected: &str, mode: u32) {
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            start.elapsed() < Duration::from_secs(40),
            "mode {mode}: ハング"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut out = String::new();
    let mut err = String::new();
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut out)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut err)
        .unwrap();
    assert!(status.success(), "mode {mode}: {status:?}\n{out}\n{err}");
    assert_eq!(out.trim(), expected, "mode {mode}: {err}");
    let live = if mode == 0 {
        "live: frames=0 chunks=0"
    } else {
        "live: sigil_blocks=0"
    };
    assert!(err.contains(live), "mode {mode}: {err}");
}
fn listener() -> TcpListener {
    let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    l.set_nonblocking(true).unwrap();
    l
}
fn accept(l: &TcpListener) -> TcpStream {
    let start = Instant::now();
    loop {
        match l.accept() {
            Ok((s, _)) => {
                s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                s.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
                return s;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    start.elapsed() < Duration::from_secs(30),
                    "接続されなかった"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept: {e}"),
        }
    }
}

fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}
fn auth(kind: u32, body: &[u8]) -> Vec<u8> {
    let mut b = kind.to_be_bytes().to_vec();
    b.extend_from_slice(body);
    packet(b'R', &b)
}
fn read_body(s: &mut TcpStream, tagged: bool) -> Vec<u8> {
    if tagged {
        let mut tag = [0];
        s.read_exact(&mut tag).unwrap();
        assert_eq!(tag[0], b'p');
    }
    let mut len = [0; 4];
    s.read_exact(&mut len).unwrap();
    let n = u32::from_be_bytes(len) as usize;
    assert!((4..16384).contains(&n));
    let mut body = vec![0; n - 4];
    s.read_exact(&mut body).unwrap();
    body
}
fn closed(s: &mut TcpStream) {
    let mut b = [0];
    match s.read(&mut b) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        other => panic!("接続が閉じていない: {other:?}"),
    }
}

// 応答列だけの模擬サーバー。製品ドライバの実装を複製しない。
fn exchange(s: &mut TcpStream, case: &str) {
    let startup = read_body(s, false);
    assert_eq!(&startup[..4], &196608u32.to_be_bytes());
    assert!(startup
        .windows(b"client_encoding\0UTF8\0".len())
        .any(|x| x == b"client_encoding\0UTF8\0"));
    assert!(startup
        .windows(b"bytea_output=hex".len())
        .any(|x| x == b"bytea_output=hex"));
    match case {
        "trust" => {
            s.write_all(&auth(0, b"")).unwrap();
            closed(s);
            return;
        }
        "md5" => {
            s.write_all(&auth(5, b"salt")).unwrap();
            closed(s);
            return;
        }
        "plus" => {
            s.write_all(&auth(10, b"SCRAM-SHA-256-PLUS\0\0")).unwrap();
            closed(s);
            return;
        }
        "eof" => {
            s.shutdown(std::net::Shutdown::Both).unwrap();
            return;
        }
        "timeout" => {
            closed(s);
            return;
        }
        _ => {}
    }
    // 分割した認証フレームも受理される。
    let sasl = auth(10, b"SCRAM-SHA-256\0\0");
    for b in sasl {
        s.write_all(&[b]).unwrap();
    }
    let initial = read_body(s, true);
    assert!(initial.starts_with(b"SCRAM-SHA-256\0"));
    let offset = b"SCRAM-SHA-256\0".len();
    let n = u32::from_be_bytes(initial[offset..offset + 4].try_into().unwrap()) as usize;
    let first = std::str::from_utf8(&initial[offset + 4..]).unwrap();
    assert_eq!(n, first.len());
    assert!(first.starts_with("n,,n=,r="));
    let nonce = first.strip_prefix("n,,n=,r=").unwrap();
    assert_eq!(nonce.len(), 32);
    let server_first = match case {
        "nonce" => "r=wrong,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096".to_string(),
        "iterations" => format!("r={nonce}server,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=100001"),
        "duplicate" => format!("r={nonce}server,r={nonce}server,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"),
        _ => format!("r={nonce}server,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096"),
    };
    s.write_all(&auth(11, server_first.as_bytes())).unwrap();
    if matches!(case, "nonce" | "iterations" | "duplicate") {
        closed(s);
        return;
    }
    let final_body = read_body(s, true);
    let final_text = std::str::from_utf8(&final_body).unwrap();
    let (without, proof) = final_text.rsplit_once(",p=").unwrap();
    assert_eq!(without, format!("c=biws,r={nonce}server"));
    let transcript = format!("{},{server_first},{without}", &first[3..]);
    let oracle = Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/data/pg_scram_oracle_473.py"
        ))
        .arg(transcript)
        .output()
        .unwrap();
    assert!(oracle.status.success());
    let oracle = String::from_utf8(oracle.stdout).unwrap();
    let mut lines = oracle.lines();
    assert_eq!(proof, lines.next().unwrap(), "ClientProof");
    let signature = lines.next().unwrap();
    let signature = if case == "signature" {
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    } else {
        signature
    };
    let mut response = auth(12, format!("v={signature}").as_bytes());
    if case == "signature" {
        s.write_all(&response).unwrap();
        closed(s);
        return;
    }
    if case == "order" {
        response.extend(packet(b'Z', b"I"));
    } else {
        response.extend(auth(0, b""));
        response.extend(packet(
            b'S',
            if case == "encoding" {
                b"client_encoding\0LATIN1\0"
            } else {
                b"client_encoding\0UTF8\0"
            },
        ));
        response.extend(packet(b'K', &[0; 8]));
        response.extend(packet(
            b'Z',
            if case == "transaction" { b"T" } else { b"I" },
        ));
    }
    s.write_all(&response).unwrap();
    closed(s);
}

#[test]
fn startup_authentication_and_rejections_match_both_engines() {
    let source = r#"module app.main
  targets server
  trust none
pub fn main(port: Int, timeout: Int) -> Int
  intent "接続結果を固定分類で返す"
  requires none
  ensures none
  effects net.client, random, task.suspend, task.spawn, clock
{
  match run(port, timeout) { Ok(n) => { return n; } Err(e) => { return 99; } }
}
fn run(port: Int, timeout: Int) -> Result<Int, PgError>
  intent "期限内に認証を完了し接続を所有する"
  requires none
  ensures none
  effects net.client, random, task.suspend, task.spawn, clock
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let limits = pg.default_connect_limits().with_timeout(timeout)?;
  match pg.connect(cfg, &limits) {
    Ok(c) => { return Ok(0); }
    Err(PgError.Protocol) => { return Ok(1); }
    Err(PgError.LimitExceeded) => { return Ok(2); }
    Err(PgError.Timeout) => { return Ok(3); }
    Err(PgError.Authentication) => { return Ok(5); }
    Err(PgError.Connection) => { return Ok(6); }
    Err(e) => { return Ok(4); }
  }
}

"#;
    let p = Prog::new(source, &[]);
    for mode in MODES {
        for case in [
            "ok",
            "trust",
            "md5",
            "plus",
            "eof",
            "timeout",
            "nonce",
            "iterations",
            "duplicate",
            "signature",
            "order",
            "encoding",
            "transaction",
        ] {
            let l = listener();
            let port = l.local_addr().unwrap().port();
            let c = p.spawn(
                mode,
                &[i64::from(port), if case == "timeout" { 100 } else { 10000 }],
            );
            let mut s = accept(&l);
            exchange(&mut s, case);
            let expected = match case {
                "ok" => "0",
                "timeout" => "3",
                "iterations" => "2",
                "trust" | "md5" | "plus" | "nonce" | "duplicate" | "signature" | "order" => "5",
                "eof" => "6",
                _ => "1",
            };
            finish(c, expected, mode);
        }
    }
}

#[test]
fn unsupported_credentials_are_rejected_before_connect() {
    let source = r#"module app.main
  targets server
  trust none
pub fn main(port: Int) -> Int
  intent "非対応資格情報を接続せず拒否する"
  requires none
  ensures none
  effects none
{
  let a = pg.config("127.0.0.1", port, "user", "db", Secret.from_bytes("日本語".as_bytes()));
  match a { Err(PgError.UnsupportedCredential) => {} _ => { return 1; } }
  let b = pg.config("127.0.0.1", port, "user", "db", Secret.from_bytes(Bytes.new()));
  match b { Err(PgError.UnsupportedCredential) => {} _ => { return 2; } }
  let c = pg.config("127.0.0.1", port, "u\u{0}ser", "db", Secret.from_bytes("pencil".as_bytes()));
  match c { Err(PgError.UnsupportedCredential) => {} _ => { return 3; } }
  let d = pg.config("127.0.0.1", port, "user", "db", Secret.from_bytes("a\u{a}b".as_bytes()));
  match d { Err(PgError.UnsupportedCredential) => {} _ => { return 4; } }
  return 0;
}
"#;
    let p = Prog::new(source, &[]);
    for mode in MODES {
        let l = listener();
        finish(
            p.spawn(mode, &[i64::from(l.local_addr().unwrap().port())]),
            "0",
            mode,
        );
        assert_eq!(
            l.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

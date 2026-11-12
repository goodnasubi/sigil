//! `std.http` の接続（ADR 0032 決定 3・4・5-2・8、PR 6-3g-c）をソケットで試験する補助。
//!
//! Rust の `std::net` のクライアントが、評価器で動かしたプログラム（`sigil run` の子プロセス）と、ネイティブにビルドして
//! 動かしたプログラム（ワーカー 1 と 4）に同じバイト列を送り、応答のバイト列を比べる（ADR 0031 決定 5-2 と同じ作り）。
//! 待ち受けは `port` を 0 にして、番号を標準出力の 1 行目に出す。時間の上限は小さい値を引数で渡す（実時間モード）。
#![allow(dead_code)]

use crate::common::native::scratch;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Eval,
    Native(u32),
}

pub const MODES: [Mode; 3] = [Mode::Eval, Mode::Native(1), Mode::Native(4)];

/// 上限の引数（`main` の `ms` の並びと同じ）。
#[derive(Clone, Copy, Debug)]
pub struct Lim {
    pub head_ms: i64,
    pub body_ms: i64,
    pub handler_ms: i64,
    pub send_ms: i64,
    pub idle_ms: i64,
    pub requests: i64,
    pub resp_body: i64,
}

impl Default for Lim {
    fn default() -> Lim {
        Lim {
            head_ms: 10_000,
            body_ms: 30_000,
            handler_ms: 30_000,
            send_ms: 30_000,
            idle_ms: 60_000,
            requests: 1000,
            resp_body: 1_048_576,
        }
    }
}

#[derive(Debug)]
pub struct Done {
    pub value: String,
    pub code: Option<i32>,
    pub stderr: String,
}

pub struct Guard(pub Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct Prog {
    dir: PathBuf,
    file: PathBuf,
    exe: PathBuf,
}

impl Drop for Prog {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// 試験用のサーバー。接続を `n` 個だけ受け付け、1 つずつルートとして `serve_conn` に渡す。ハンドラは経路で動きを変える。
pub const SERVER: &str = r#"module app.main
  targets server
  trust none

fn fault(x: Int) -> Int
  intent "契約違反でハンドラを異常停止させる"
  requires x == 0
  ensures none
  effects none
{
  return x;
}

fn reply(status: Int, body: &String) -> HttpResponse
  intent "状態と本文から応答を作る"
  requires 200 <= status && status <= 599
  ensures none
  effects none
{
  return HttpResponse.new(status, body.as_bytes());
}

fn sized(n: Int) -> HttpResponse
  intent "n バイトの本文の応答"
  requires 0 <= n
  ensures none
  effects none
{
  let mut b = Bytes.new();
  let mut i = 0;
  while i < n {
    b.push(97);
    i = i + 1;
  }
  return HttpResponse.new(200, b);
}

fn handle(r: &HttpRequest) -> HttpResponse
  intent "経路で動きを変える試験用のハンドラ"
  requires none
  ensures none
  effects clock, task.suspend
{
  let p = r.path();
  if p == "/echo".as_bytes() {
    let mut b = r.method().as_bytes();
    b.push(32);
    b.extend(&r.body());
    return HttpResponse.new(200, b);
  }
  if p == "/s204".as_bytes() { return HttpResponse.new(204, Bytes.new()); }
  if p == "/s204b".as_bytes() { return reply(204, &"x"); }
  if p == "/s205".as_bytes() { return HttpResponse.new(205, Bytes.new()); }
  if p == "/s205b".as_bytes() { return reply(205, &"x"); }
  if p == "/s304".as_bytes() { return HttpResponse.new(304, Bytes.new()); }
  if p == "/s304b".as_bytes() { return reply(304, &"x"); }
  if p == "/s200".as_bytes() { return reply(200, &"hello"); }
  if p == "/s200e".as_bytes() { return HttpResponse.new(200, Bytes.new()); }
  if p == "/hdr".as_bytes() { return reply(200, &"ok").with_header(&"X-A", &"v".as_bytes()); }
  if p == "/inj".as_bytes() { return reply(200, &"ok").with_header(&"x-a", &"a\r\nb: c".as_bytes()); }
  if p == "/injlf".as_bytes() { return reply(200, &"ok").with_header(&"x-a", &"a\nb".as_bytes()); }
  if p == "/badname".as_bytes() { return reply(200, &"ok").with_header(&"bad name", &"v".as_bytes()); }
  if p == "/emptyname".as_bytes() { return reply(200, &"ok").with_header(&"", &"v".as_bytes()); }
  if p == "/cl".as_bytes() { return reply(200, &"ok").with_header(&"Content-Length", &"5".as_bytes()); }
  if p == "/te".as_bytes() { return reply(200, &"ok").with_header(&"Transfer-Encoding", &"chunked".as_bytes()); }
  if p == "/conn".as_bytes() { return reply(200, &"ok").with_header(&"Connection", &"keep-alive".as_bytes()); }
  if p == "/body16".as_bytes() { return sized(16); }
  if p == "/body17".as_bytes() { return sized(17); }
  if p == "/huge".as_bytes() {
    let mut b = Bytes.new();
    b.push(97);
    let mut i = 0;
    while i < 26 {
      let c = b.clone();
      b.extend(&c);
      i = i + 1;
    }
    return HttpResponse.new(200, b);
  }
  if p == "/slow".as_bytes() {
    task.sleep(2000);
    return reply(200, &"late");
  }
  if p == "/abort".as_bytes() {
    let z = fault(1);
    return reply(200, &"unreachable");
  }
  return reply(404, &"");
}

fn limits(head_ms: Int, body_ms: Int, handler_ms: Int, send_ms: Int, idle_ms: Int, requests: Int, resp_body: Int) -> HttpLimits
  intent "試験用の上限"
  requires none
  ensures none
  effects none
{
  return http.default_limits().with_head_ms(head_ms).with_body_ms(body_ms).with_handler_ms(handler_ms).with_send_ms(send_ms).with_idle_ms(idle_ms).with_requests(requests).with_response_body(resp_body);
}

pub fn main(n: Int, limit: Int, head_ms: Int, body_ms: Int, handler_ms: Int, send_ms: Int, idle_ms: Int, requests: Int, resp_body: Int) -> Int
  intent "n 個の接続を受け付けて serve_conn に渡す"
  requires none
  ensures none
  effects net.server, console, clock, task.spawn, task.suspend, task.share
{
  let host = "127.0.0.1";
  match net.listen(&host, 0, http.default_limits().backlog()) {
    Ok(l) => {
      console.log(&l.local_port());
      let got = task.roots(limit, |g| {
        let mut i = 0;
        while i < n {
          let slot = g.reserve();
          match l.accept() {
            Ok(st) => {
              slot.start(http.conn(st), move |c| { http.serve_conn(&c, &limits(head_ms, body_ms, handler_ms, send_ms, idle_ms, requests, resp_body), handle); }, move |c| { http.abort_conn(c, &limits(head_ms, body_ms, handler_ms, send_ms, idle_ms, requests, resp_body)); });
            }
            Err(e) => {}
          }
          i = i + 1;
        }
        return n;
      });
      return got;
    }
    Err(e) => { return 0 - 100; }
  }
}
"#;

impl Prog {
    pub fn new(tag: &str, src: &str) -> Prog {
        let dir = scratch(tag);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let file = dir.join("app").join("main.sigil");
        std::fs::write(&file, src).unwrap();
        let exe = dir.join("prog");
        sigil::native::build(&[src], "main", &sigil::codegen::Opts::default(), &exe)
            .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
        Prog { dir, file, exe }
    }

    pub fn server(tag: &str) -> Prog {
        Prog::new(tag, SERVER)
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

    /// `n` 個の接続を受け付けるサーバーを動かし、番号とプロセス ID を `client` に渡して、終わりを待つ。
    pub fn serve<T>(
        &self,
        mode: Mode,
        n: i64,
        limit: i64,
        lim: Lim,
        client: impl FnOnce(u16, u32) -> T,
    ) -> (T, Done) {
        let args = [
            n,
            limit,
            lim.head_ms,
            lim.body_ms,
            lim.handler_ms,
            lim.send_ms,
            lim.idle_ms,
            lim.requests,
            lim.resp_body,
        ];
        let mut g = Guard(self.command(mode, &args).spawn().unwrap());
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

/// 正常に終わり、値が `value` で、漏れが無いこと。
pub fn clean(mode: Mode, d: &Done, value: &str) {
    assert_eq!(d.code, Some(0), "{mode:?}: {d:?}");
    assert_eq!(d.value, value, "{mode:?}: {d:?}");
    let want = match mode {
        Mode::Eval => "live: frames=0 chunks=0",
        Mode::Native(_) => "live: sigil_blocks=0",
    };
    assert!(d.stderr.contains(want), "{mode:?}: 漏れがある: {d:?}");
}

pub fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
    s
}

/// 接続して `payload` を `step` バイトずつ（0 は一度に）送り、書き込み側を閉じて、相手が閉じるまでの応答を全部読む。
pub fn exchange(port: u16, payload: &[u8], step: usize) -> Vec<u8> {
    let mut s = connect(port);
    send_in_steps(&mut s, payload, step);
    let _ = s.shutdown(Shutdown::Write);
    read_all(&mut s)
}

pub fn send_in_steps(s: &mut TcpStream, payload: &[u8], step: usize) {
    if step == 0 {
        let _ = s.write_all(payload);
    } else {
        s.set_nodelay(true).unwrap();
        for ch in payload.chunks(step) {
            if s.write_all(ch).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

pub fn read_all(s: &mut TcpStream) -> Vec<u8> {
    let mut got = vec![];
    let _ = s.read_to_end(&mut got);
    got
}

/// 応答の連結を、状態行だけ（`HTTP/1.1 200 ...`）の並びに要約する。
pub fn statuses(bytes: &[u8]) -> Vec<u16> {
    let s = String::from_utf8_lossy(bytes);
    s.match_indices("HTTP/1.1 ")
        .filter_map(|(i, _)| s[i + 9..i + 12].parse::<u16>().ok())
        .collect()
}

pub fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// 1 つの要求の文字列。
pub fn req(method: &str, path: &str, extra: &str, body: &str) -> String {
    format!("{method} {path} HTTP/1.1\r\nHost: x\r\n{extra}\r\n{body}")
}

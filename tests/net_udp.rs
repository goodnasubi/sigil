//! ADR 0057 決定 1〜4 / 実装 PR 2: UDP ソケット（`std.os.net`。`net.udp_bind`・`net.udp_open`・`UdpSocket`・`UdpClient`・`UdpReceiveError`）。
//!
//! 突き合わせ: この試験が Rust の `std::net::UdpSocket` を外の相手として、評価器で動かしたプログラム（`sigil run` の子プロセス）と、
//! ネイティブにビルドして動かしたプログラム（ワーカー 1 と 4）の**両方**に同じデータグラムを送り、応答のバイト列・送信元・
//! 最後の値・終了コード・漏れを比べる。
//!
//! - 待ち受け（またはクライアント）の `local_port` の番号を標準出力の 1 行目に出す。
//! - 漏れ: `SIGIL_RT_LIVE` で、評価器は `frames=0 chunks=0`、ネイティブは `sigil_blocks=0` を確かめる。
//!   fd の漏れは `/proc/<pid>/fd` の数で確かめる。
mod common;

use common::native::scratch;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, UdpSocket};
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

    /// プログラムを起動し、1 行目の番号を読んで `peer(番号, プロセスID)` を動かし、終わりを待つ。
    fn serve<T>(&self, mode: Mode, args: &[i64], peer: impl FnOnce(u16, u32) -> T) -> (T, Done) {
        let mut g = Guard(self.command(mode, args).spawn().unwrap());
        let mut out = BufReader::new(g.0.stdout.take().unwrap());
        let mut first = String::new();
        out.read_line(&mut first).unwrap();
        let Ok(port) = first.trim().parse::<u16>() else {
            let mut e = String::new();
            let _ = g.0.stderr.take().unwrap().read_to_string(&mut e);
            panic!("{mode:?}: 番号が出なかった: {first:?}\n{e}");
        };
        let got = peer(port, g.0.id());
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

/// `/proc/<pid>/fd` のうち、ソケットを指すものの数。
fn socket_fds(pid: u32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|t| t.to_string_lossy().starts_with("socket:"))
        .count()
}

fn peer() -> UdpSocket {
    let s = UdpSocket::bind("127.0.0.1:0").unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s
}

fn payload(i: usize, len: usize) -> Vec<u8> {
    (0..len).map(|k| ((k * 7 + i * 31) % 251) as u8).collect()
}

const HEAD: &str = r#"module app.main
  targets server
  trust none
"#;

// ---------------------------------------------------------------------------
// エフェメラル bind・送信元と宛先・複数データグラムの境界・空と上限ちょうど・上限超過の消費と次の受信・待機後の成功。

/// `n` 個のデータグラムを受け、そのまま送信元へ返す。`max` を超えたものは数えるだけで、返さない。
/// 値は `超過の数 * 100000000 + 返したバイトの合計`（送信元のアドレスが違えば合計から 10^9 を引く）。
fn echo_server() -> String {
    format!(
        r#"{HEAD}
fn too_large(e: &UdpReceiveError) -> Int
  intent "DatagramTooLarge なら 1、ほかは 100000"
  requires none
  ensures none
  effects none
{{
  match e {{
    UdpReceiveError.DatagramTooLarge => {{ return 1; }}
    UdpReceiveError.AddrInUse => {{ return 100000; }}
    UdpReceiveError.PermissionDenied => {{ return 100000; }}
    UdpReceiveError.Closed => {{ return 100000; }}
    UdpReceiveError.Other => {{ return 100000; }}
  }}
}}

pub fn main(n: Int, max: Int) -> Int
  intent "x"
  requires none
  ensures none
  effects net.server, console, task.suspend
{{
  let h = "127.0.0.1";
  match net.udp_bind(&h, 0) {{
    Ok(s0) => {{
      let mut s = s0;
      console.log(&s.local_port());
      let mut i = 0;
      let mut total = 0;
      let mut big = 0;
      while i < n {{
        match s.recv_from(max) {{
          Ok(t) => {{
            let (d, ip, p) = t;
            if ip != "127.0.0.1" {{
              total = total - 1000000000;
            }}
            match s.send_to(&d, &ip, p) {{
              Ok(u) => {{ total = total + d.byte_len(); }}
              Err(e) => {{ total = total - 1000000000; }}
            }}
          }}
          Err(e) => {{ big = big + too_large(&e); }}
        }}
        i = i + 1;
      }}
      return big * 100000000 + total;
    }}
    Err(e) => {{ return 0 - 100; }}
  }}
}}
"#
    )
}

fn recv_echo(c: &UdpSocket, from: SocketAddr) -> Vec<u8> {
    let mut buf = vec![0u8; 70_000];
    let (n, src) = c.recv_from(&mut buf).unwrap();
    assert_eq!(src, from, "応答の送信元はサーバーの番号");
    buf.truncate(n);
    buf
}

/// 空・1 バイト・小・上限ちょうど（65507）の各データグラムを、先にまとめて送ってから受ける。境界が保たれ、順序どおりに返る。
/// サーバーが待っている間に相手が少し遅れて送る（待機後の成功）。送信元のアドレスと番号が応答の宛先になる。
#[test]
fn echo_keeps_datagram_boundaries_between_evaluator_and_native() {
    let p = Prog::new("udp-echo", &echo_server());
    let sizes = [0usize, 1, 17, 1472, 65_507];
    let total: usize = sizes.iter().sum();
    for mode in MODES {
        let (got, d) = p.serve(mode, &[sizes.len() as i64, 65_507], |port, _| {
            let c = peer();
            let to: SocketAddr = ([127, 0, 0, 1], port).into();
            std::thread::sleep(Duration::from_millis(300));
            for (i, &len) in sizes.iter().enumerate() {
                assert_eq!(c.send_to(&payload(i, len), to).unwrap(), len);
            }
            sizes.iter().map(|_| recv_echo(&c, to)).collect::<Vec<_>>()
        });
        for (i, (&len, g)) in sizes.iter().zip(&got).enumerate() {
            assert_eq!(g, &payload(i, len), "{mode:?}: {i} 番目");
        }
        clean(mode, &d, &total.to_string());
    }
}

/// `max` を超えるデータグラムは全体を消費して `DatagramTooLarge` になり、断片は渡らない。次の受信は次のデータグラム。
/// `max` ちょうどは成功する。
#[test]
fn an_oversize_datagram_is_consumed_whole_and_the_next_receive_succeeds() {
    let p = Prog::new("udp-big", &echo_server());
    for mode in MODES {
        let (got, d) = p.serve(mode, &[4, 500], |port, _| {
            let c = peer();
            let to: SocketAddr = ([127, 0, 0, 1], port).into();
            for (i, len) in [600usize, 501, 500, 10].into_iter().enumerate() {
                c.send_to(&payload(i, len), to).unwrap();
                if len <= 500 {
                    // 返るのは max 以下のものだけ。超過のものの断片は返らない。
                    assert_eq!(recv_echo(&c, to), payload(i, len), "{mode:?}");
                } else {
                    // 次の送信が順序どおりに処理されるよう、少し待つ。
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        });
        let () = got;
        clean(mode, &d, &(2 * 100_000_000 + 510).to_string());
    }
}

// ---------------------------------------------------------------------------
// クライアント側（`udp_open`）。

fn client_prog() -> String {
    format!(
        r#"{HEAD}
pub fn main(server: Int) -> Int
  intent "x"
  requires none
  ensures none
  effects net.client, console, task.suspend
{{
  match net.udp_open() {{
    Ok(c0) => {{
      let mut c = c0;
      console.log(&c.local_port());
      let h = "127.0.0.1";
      let q = "ping".as_bytes();
      match c.send_to(&q, &h, server) {{
        Ok(u) => {{}}
        Err(e) => {{ return 0 - 1; }}
      }}
      match c.recv_from(100) {{
        Ok(t) => {{
          let (d, ip, p) = t;
          let mut ok = 0;
          if ip == "127.0.0.1" && p == server {{
            ok = 1;
          }}
          return d.byte_len() * 10 + ok;
        }}
        Err(e) => {{ return 0 - 2; }}
      }}
    }}
    Err(e) => {{ return 0 - 3; }}
  }}
}}
"#
    )
}

/// `udp_open` はエフェメラルの番号に bind し、送信元の番号はその `local_port` と一致する。応答の送信元は宛先のアドレスと番号。
#[test]
fn the_client_sends_from_its_local_port_and_reads_the_reply() {
    let p = Prog::new("udp-client", &client_prog());
    for mode in MODES {
        let srv = peer();
        let sp = srv.local_addr().unwrap().port();
        let ((got, src_port, local), d) = p.serve(mode, &[sp as i64], |port, _| {
            let mut buf = [0u8; 64];
            let (n, src) = srv.recv_from(&mut buf).unwrap();
            srv.send_to(b"pong!", src).unwrap();
            (buf[..n].to_vec(), src.port(), port)
        });
        assert_eq!(got, b"ping", "{mode:?}");
        assert_ne!(local, 0, "{mode:?}");
        assert_eq!(src_port, local, "{mode:?}: 送信元は local_port と同じ");
        clean(mode, &d, "51");
    }
}

// ---------------------------------------------------------------------------
// 数値以外の拒否・契約・分類。

/// 数値の IPv4 以外（名前・IPv6・不正な書式）は `NetError.Other`。送信は 1〜65535 の番号だけ。bind の重複は `AddrInUse`。
#[test]
fn non_numeric_hosts_are_rejected_and_bind_errors_are_matchable() {
    let src = format!(
        r#"{HEAD}
fn kind(r: Result<UdpSocket, NetError>) -> Int
  intent "結果を数にする"
  requires none
  ensures none
  effects none
{{
  match r {{
    Ok(l) => {{ return 0; }}
    Err(e) => {{
      match e {{
        NetError.AddrInUse => {{ return 1; }}
        NetError.PermissionDenied => {{ return 2; }}
        NetError.Closed => {{ return 3; }}
        NetError.Other => {{ return 4; }}
      }}
    }}
  }}
}}

fn bad(host: &String) -> Int
  intent "host で bind を試す"
  requires none
  ensures none
  effects net.server
{{
  return kind(net.udp_bind(&host, 0));
}}

fn send_kind(c: &mut UdpClient, host: &String) -> Int
  intent "host へ空のデータグラムを送る結果を数にする"
  requires none
  ensures none
  effects net.client, task.suspend
{{
  let d = Bytes.new();
  match c.send_to(&d, &host, 9) {{
    Ok(u) => {{ return 0; }}
    Err(e) => {{ return 4; }}
  }}
}}

pub fn main() -> Int
  intent "x"
  requires none
  ensures none
  effects net.server, net.client, task.suspend
{{
  let h = "127.0.0.1";
  let mut v = 0;
  v = v * 10 + bad(&"localhost");
  v = v * 10 + bad(&"::1");
  v = v * 10 + bad(&"256.0.0.1");
  v = v * 10 + bad(&"1.2.3");
  v = v * 10 + bad(&"");
  match net.udp_bind(&h, 0) {{
    Ok(s) => {{
      v = v * 10 + kind(net.udp_bind(&h, s.local_port()));
      v = v * 10 + bad(&"127.0.0.1");
    }}
    Err(e) => {{ v = v * 10 + 9; }}
  }}
  match net.udp_open() {{
    Ok(c0) => {{
      let mut c = c0;
      v = v * 10 + send_kind(&mut c, &"example.com");
      v = v * 10 + send_kind(&mut c, &"::1");
    }}
    Err(e) => {{ v = v * 10 + 9; }}
  }}
  return v;
}}
"#
    );
    let p = Prog::new("udp-err", &src);
    for mode in MODES {
        clean(mode, &p.run(mode, &[]), "444441044");
    }
}

/// 契約の違反（番号・長さ・上限）は、評価器でもネイティブでも異常停止（終了コード 70）になる。
#[test]
fn contract_violations_stop_in_both() {
    for (i, body) in [
        "match net.udp_bind(&h, 65536) { Ok(s) => { return 1; } Err(e) => { return 2; } }",
        "match net.udp_bind(&h, 0 - 1) { Ok(s) => { return 1; } Err(e) => { return 2; } }",
        "match net.udp_open() { Ok(c0) => { let mut c = c0; let d = Bytes.new(); match c.send_to(&d, &h, 0) { Ok(u) => { return 1; } Err(e) => { return 2; } } } Err(e) => { return 3; } }",
        "match net.udp_open() { Ok(c0) => { let mut c = c0; let d = Bytes.new(); match c.send_to(&d, &h, 65536) { Ok(u) => { return 1; } Err(e) => { return 2; } } } Err(e) => { return 3; } }",
        "match net.udp_open() { Ok(c0) => { let mut c = c0; let mut d = Bytes.new(); let mut k = 0; while k < 65508 { d.push(1); k = k + 1; } match c.send_to(&d, &h, 9) { Ok(u) => { return 1; } Err(e) => { return 2; } } } Err(e) => { return 3; } }",
        "match net.udp_open() { Ok(c0) => { let mut c = c0; match c.recv_from(0) { Ok(t) => { return 1; } Err(e) => { return 2; } } } Err(e) => { return 3; } }",
        "match net.udp_open() { Ok(c0) => { let mut c = c0; match c.recv_from(65508) { Ok(t) => { return 1; } Err(e) => { return 2; } } } Err(e) => { return 3; } }",
    ]
    .iter()
    .enumerate()
    {
        let mut effects = vec![if body.contains("udp_open") {
            "net.client"
        } else {
            "net.server"
        }];
        if body.contains("send_to") || body.contains("recv_from") {
            effects.push("task.suspend");
        }
        let effects = effects.join(", ");
        let src = format!(
            "{HEAD}\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n  let h = \"127.0.0.1\";\n  {body}\n}}\n"
        );
        let p = Prog::new(&format!("udp-req{i}"), &src);
        for mode in MODES {
            let d = p.run(mode, &[]);
            assert_eq!(d.code, Some(70), "{mode:?} {body}: {d:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// 取り消し・タイムアウト後の資源回収。

/// 誰も送らない `recv_from` を時間切れで取り消すことを繰り返しても、ソケットは閉じられ、fd は漏れない。
#[test]
fn a_cancelled_receive_closes_its_socket() {
    let src = format!(
        r#"{HEAD}
fn wait_one(sock: &mut UdpSocket) -> Int
  intent "来ないデータグラムを待つ"
  requires none
  ensures none
  effects net.server, task.suspend
{{
  match sock.recv_from(100) {{
    Ok(t) => {{ return 1; }}
    Err(e) => {{ return 2; }}
  }}
}}

fn first(rounds: Int) -> Int
  intent "誰も送らない UDP ソケットの recv_from を、時間切れで取り消すことを繰り返す"
  requires none
  ensures none
  effects net.server, clock, task.spawn, task.suspend
{{
  let h = "127.0.0.1";
  let mut miss = 0;
  let mut i = 0;
  while i < rounds {{
    match net.udp_bind(&h, 0) {{
      Ok(s0) => {{
        let mut s = s0;
        match task.timeout(30, move || wait_one(&mut s)) {{
          Some(v) => {{}}
          None => {{ miss = miss + 1; }}
        }}
      }}
      Err(e) => {{}}
    }}
    i = i + 1;
  }}
  return miss;
}}

pub fn main(rounds: Int) -> Int
  intent "x"
  requires none
  ensures none
  effects net.server, console, clock, task.spawn, task.suspend
{{
  let miss = first(rounds);
  let h = "127.0.0.1";
  match net.udp_bind(&h, 0) {{
    Ok(s0) => {{
      let mut s = s0;
      console.log(&s.local_port());
      match s.recv_from(100) {{
        Ok(t) => {{
          let (d, ip, p) = t;
          return miss * 1000 + d.byte_len();
        }}
        Err(e) => {{ return 0 - 2; }}
      }}
    }}
    Err(e) => {{ return 0 - 3; }}
  }}
}}
"#
    );
    let p = Prog::new("udp-cancel", &src);
    for mode in MODES {
        let (n, d) = p.serve(mode, &[5], |port, pid| {
            // 取り消された 5 つのソケットは閉じられている（開いているのは今の 1 つだけ）。
            let n = socket_fds(pid);
            peer().send_to(b"again", ("127.0.0.1", port)).unwrap();
            n
        });
        assert_eq!(n, 1, "{mode:?}: 取り消したソケットの fd が残っている");
        clean(mode, &d, "5005");
    }
}

/// 値の破棄でソケットが閉じる。多数を作って捨てても fd は増えない（開いたままなのは最後の 1 つだけ）。
#[test]
fn dropping_sockets_does_not_leak_fds() {
    let src = format!(
        r#"{HEAD}
fn churn(n: Int) -> Int
  intent "UdpSocket と UdpClient を作って捨てることを繰り返す"
  requires none
  ensures none
  effects net.server, net.client
{{
  let h = "127.0.0.1";
  let mut ok = 0;
  let mut i = 0;
  while i < n {{
    match net.udp_bind(&h, 0) {{
      Ok(s) => {{ ok = ok + 1; }}
      Err(e) => {{}}
    }}
    match net.udp_open() {{
      Ok(c) => {{ ok = ok + 1; }}
      Err(e) => {{}}
    }}
    i = i + 1;
  }}
  return ok;
}}

pub fn main(n: Int) -> Int
  intent "x"
  requires none
  ensures none
  effects net.server, net.client, console, task.suspend
{{
  let ok = churn(n);
  let h = "127.0.0.1";
  match net.udp_bind(&h, 0) {{
    Ok(s0) => {{
      let mut s = s0;
      console.log(&s.local_port());
      match s.recv_from(100) {{
        Ok(t) => {{ return ok; }}
        Err(e) => {{ return 0 - 2; }}
      }}
    }}
    Err(e) => {{ return 0 - 3; }}
  }}
}}
"#
    );
    let p = Prog::new("udp-drop", &src);
    for mode in MODES {
        let (n, d) = p.serve(mode, &[300], |port, pid| {
            let n = socket_fds(pid);
            peer().send_to(b"x", ("127.0.0.1", port)).unwrap();
            n
        });
        assert_eq!(n, 1, "{mode:?}");
        clean(mode, &d, "600");
    }
}

// ---------------------------------------------------------------------------
// 検査で止まる書き方（実行ファイルを作らないので、評価器の入口を直接呼ぶ）。

fn check_err(src: &str) -> String {
    let e = sigil::ir_eval::run_with(src, "main", &[], &sigil::exec::Options::captured())
        .expect_err("止まるはず");
    match e.error {
        sigil::exec::RunError::Check(c) => format!("{:?} {}", c.kind, c.message),
        sigil::exec::RunError::Unsupported(m) => format!("Unsupported {m}"),
        other => panic!("{other:?}"),
    }
}

fn with_main(items: &str, body: &str) -> String {
    let effects = if body.contains("net.udp_open") {
        "net.client"
    } else {
        "net.server"
    };
    format!(
        "{HEAD}\n{items}\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n  let h = \"127.0.0.1\";\n{body}\n}}\n"
    )
}

const OPEN_SOCK: &str = "  match net.udp_bind(&h, 0) {\n    Ok(s) => {\n";
const END_SOCK: &str = "    }\n    Err(e) => { return 2; }\n  }";

/// fd を所有する値は複製できない（直接でも、構造体の中でも、型引数を通しても）。両方の型で。
#[test]
fn udp_values_cannot_be_cloned() {
    let direct = with_main(
        "",
        &format!("{OPEN_SOCK}      let s2 = s.clone();\n      return 1;\n{END_SOCK}"),
    );
    let m = check_err(&direct);
    assert!(m.starts_with("NoSuchMember") && m.contains("clone"), "{m}");

    let client = with_main(
        "",
        "  match net.udp_open() {\n    Ok(c) => {\n      let c2 = c.clone();\n      return 1;\n    }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&client);
    assert!(m.starts_with("NoSuchMember") && m.contains("clone"), "{m}");

    let wrapped = with_main(
        "struct Holder {\n  s: UdpSocket,\n  n: Int,\n}\n",
        &format!("{OPEN_SOCK}      let a = Holder {{ s: s, n: 1 }};\n      let b = a.clone();\n      return 1;\n{END_SOCK}"),
    );
    let m = check_err(&wrapped);
    assert!(m.starts_with("NoSuchMember") && m.contains("clone"), "{m}");

    let generic = with_main(
        "fn dup<T>(x: &T) -> T\n  intent \"複製\"\n  requires none\n  ensures none\n  effects none\n{\n  return x.clone();\n}\n",
        &format!("{OPEN_SOCK}      let s2 = dup(&s);\n      return 1;\n{END_SOCK}"),
    );
    let m = check_err(&generic);
    assert!(m.contains("clone") && m.contains("UdpSocket"), "{m}");
}

/// 所有は 1 回だけ移る。移した後の使用は止まる。別のタスクへの移動（`Sendable`）は通る。
#[test]
fn udp_ownership_moves_once_and_moves_into_a_task() {
    let moved = with_main(
        "fn take(s: UdpSocket) -> Int\n  intent \"受け取る\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n",
        &format!("{OPEN_SOCK}      let a = take(s);\n      let b = take(s);\n      return a + b;\n{END_SOCK}"),
    );
    let m = check_err(&moved);
    assert!(m.starts_with("UseAfterMove"), "{m}");

    // 別のタスクへ所有ごと移せる（`Sendable`）。`Sendable` を要求する関数へ渡して検査を通る。
    for ty in ["UdpSocket", "UdpClient"] {
        let ctor = if ty == "UdpSocket" {
            "net.udp_bind(&h, 0)"
        } else {
            "net.udp_open()"
        };
        let ok = with_main(
            "fn need<T: Sendable>(x: &T) -> Int\n  intent \"移せる型だけ\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n",
            &format!("  match {ctor} {{\n    Ok(s) => {{ return need(&s); }}\n    Err(e) => {{ return 2; }}\n  }}"),
        );
        let r = sigil::ir_eval::run_with(&ok, "main", &[], &sigil::exec::Options::captured());
        assert!(
            r.is_ok(),
            "{ty}: {:?}",
            r.err().map(|e| format!("{:?}", e.error))
        );
    }
}

/// `Shareable` でない。fd の番号は取り出せず、番号から作ることもできない。
#[test]
fn udp_types_are_not_shareable_and_hide_the_fd() {
    let share = with_main(
        "fn need<T: Shareable>(x: &T) -> Int\n  intent \"共有できる型だけ\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n",
        &format!("{OPEN_SOCK}      return need(&s);\n{END_SOCK}"),
    );
    let m = check_err(&share);
    assert!(m.contains("Shareable"), "{m}");

    let field = with_main("", &format!("{OPEN_SOCK}      return s.fd;\n{END_SOCK}"));
    let m = check_err(&field);
    assert!(m.contains("fd"), "{m}");

    for ty in ["UdpSocket", "UdpClient"] {
        let forge = with_main("", &format!("  let s = {ty} {{ fd: 0 }};\n  return 1;"));
        let m = check_err(&forge);
        assert!(m.contains(ty), "{ty}: {m}");
    }
}

/// 型の名前は前置きの名前で、利用者は同じ名前の型を宣言できない。
#[test]
fn udp_names_are_reserved() {
    for name in ["UdpSocket", "UdpClient", "UdpReceiveError"] {
        let m = check_err(&with_main(
            &format!("struct {name} {{\n  x: Int,\n}}\n"),
            "  return 1;",
        ));
        assert!(m.starts_with("DuplicateDefinition"), "{name}: {m}");
    }
}

/// 効果宣言が足りなければ検査で止まる。`UdpSocket` の操作は `net.server`、`UdpClient` は `net.client`。待機は `task.suspend`。
#[test]
fn missing_effects_are_rejected() {
    let bind = |effects: &str, call: &str| {
        format!(
            "{HEAD}\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n  let h = \"127.0.0.1\";\n  {call}\n}}\n"
        )
    };
    let server_call =
        "match net.udp_bind(&h, 0) { Ok(s) => { return 1; } Err(e) => { return 2; } }";
    let client_call = "match net.udp_open() { Ok(c) => { return 1; } Err(e) => { return 2; } }";
    let m = check_err(&bind("net.client", server_call));
    assert!(m.contains("net.server"), "{m}");
    let m = check_err(&bind("net.server", client_call));
    assert!(m.contains("net.client"), "{m}");

    let recv = "match net.udp_bind(&h, 0) { Ok(s0) => { let mut s = s0; match s.recv_from(10) { Ok(t) => { return 1; } Err(e) => { return 2; } } } Err(e) => { return 3; } }";
    let m = check_err(&bind("net.server", recv));
    assert!(m.contains("task.suspend"), "{m}");
    // `UdpSocket` の操作を `net.client` だけで呼ぶことはできない（サーバー側のソケットを client 効果へ変換しない）。
    let m = check_err(&bind("net.client, task.suspend", recv));
    assert!(m.contains("net.server"), "{m}");
}

/// `bare` を出力先に持つモジュールからは使えない（OS 層。ADR 0014）。ブラウザも同じ。
#[test]
fn udp_is_not_available_on_bare_or_browser() {
    for targets in ["bare, server", "browser, server"] {
        let src = format!(
            "module app.main\n  targets {targets}\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let h = \"127.0.0.1\";\n  match net.udp_bind(&h, 0) {{\n    Ok(s) => {{ return 1; }}\n    Err(e) => {{ return 2; }}\n  }}\n}}\n"
        );
        let m = check_err(&src);
        // bare は OS 層の名前そのものを止め、ブラウザは効果の実行対象で止まる（どちらも検査で止まる）。
        assert!(
            m.starts_with("OsItemInCore") || m.starts_with("EffectTarget"),
            "{targets}: {m}"
        );
    }
}

/// `UdpReceiveError` の腕の並びは、検査器が登録する並びと同じ。
#[test]
fn udp_receive_error_arms_match_the_embedded_declaration() {
    let src = sigil::stdlib::embedded::lookup("std.os.net", &[]).unwrap();
    let body = src
        .split("pub enum UdpReceiveError {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    let arms: Vec<&str> = body
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(arms, sigil::stdlib::UDP_RECEIVE_ERROR_ARMS);
}

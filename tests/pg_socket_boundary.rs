//! ADR 0034 決定 4 / #462: 接続側と OS 暗号乱数の境界。
//! 外部相手との同じ入出力を評価器・native（1/4 workers）で比べる。
//! 稀な errno の注入は既存 embedded_extra を使い、製品 API に試験用 nonce を追加しない。
mod common;

use common::native::scratch;
use sigil::exec::{Arg, Options, RunError, StopKind};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const HEAD: &str = "module app.main\n  targets server\n  trust none\n\n";
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
        let dir = scratch("pg-boundary");
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

#[test]
fn outbound_roundtrip_and_owned_moves_match_eval_and_native() {
    let src = format!(
        r#"{HEAD}
struct Holder {{ s: TcpClient, }}
fn exchange(value: Holder) -> Int
  intent "所有接続を移動し、大きいバイト列を送受信する"
  requires none
  ensures none
  effects net.client, task.suspend
{{
  let mut c = value;
  let mut data = Bytes.new();
  let mut i = 0;
  while i < 180000 {{ data.push(i % 251); i = i + 1; }}
  match c.s.write_all(&data) {{ Ok(u) => {{}} Err(e) => {{ return 0 - 1; }} }}
  let mut all = Bytes.new();
  while all.len() < 180000 {{
    match c.s.read(65536) {{
      Ok(b) => {{ if b.is_empty() {{ return 0 - 2; }} all.extend(&b); }}
      Err(e) => {{ return 0 - 3; }}
    }}
  }}
  if all == data {{ return all.len(); }}
  return 0 - 4;
}}
pub fn main(port: Int) -> Int
  intent "接続側だけの効果を宣言する"
  requires 1 <= port && port <= 65535
  ensures none
  effects net.client, task.suspend
{{
  let h = "127.0.0.1";
  match net.connect(&h, port) {{
    Ok(s) => {{ return exchange(Holder {{ s: s }}); }}
    Err(e) => {{ return 0 - 5; }}
  }}
}}
"#
    );
    let p = Prog::new(&src, &[]);
    let payload: Vec<u8> = (0..180000).map(|i| (i % 251) as u8).collect();
    for mode in MODES {
        let l = listener();
        let c = p.spawn(mode, &[i64::from(l.local_addr().unwrap().port())]);
        let mut s = accept(&l);
        let mut got = vec![0; payload.len()];
        s.read_exact(&mut got).unwrap();
        assert_eq!(got, payload);
        for chunk in payload.chunks(1733) {
            s.write_all(chunk).unwrap();
        }
        let mut tail = Vec::new();
        s.read_to_end(&mut tail).unwrap(); // drop が相手にも見える。
        assert!(tail.is_empty());
        finish(c, "180000", mode);
    }
}

// rare errno は既存の埋め込み差し込みだけで注入し、同じ SIR を両実行系で走らせる。
fn injected(src: &str, module: &str, implementation: String, expected: &str) {
    let extra = vec![(module.to_string(), implementation)];
    let opts = Options {
        embedded_extra: extra.clone(),
        ..Options::captured()
    };
    let value = sigil::ir_eval::run_with(src, "main", &[], &opts)
        .unwrap()
        .value;
    assert_eq!(value, expected);
    let p = Prog::new(src, &extra);
    for mode in [1, 4] {
        finish(p.spawn(mode, &[]), expected, mode);
    }
}

#[test]
fn os_random_is_bounded_and_has_no_fixed_nonce_entry() {
    let src = format!(
        r#"{HEAD}
pub fn main() -> Int
  intent "24 バイトの nonce と上限の乱数を得る。乱数の具体値は比較しない"
  requires none
  ensures none
  effects random
{{
  match random.bytes(24) {{
    Some(a) => {{
      match random.bytes(65536) {{ Some(b) => {{ return a.len() + b.len(); }} None => {{ return 0 - 2; }} }}
    }}
    None => {{ return 0 - 1; }}
  }}
}}
"#
    );
    // 確率的な「非ゼロ」「相異なる」検査はせず、実 OS の成功と長さを比較する。
    let p = Prog::new(&src, &[]);
    for mode in MODES {
        finish(p.spawn(mode, &[]), "65560", mode);
    }
}

#[test]
fn getrandom_partial_reads_eintr_and_failure_are_fail_closed() {
    let src = format!(
        r#"{HEAD}
pub fn main() -> Int
  intent "全量成功だけを公開する"
  requires none
  ensures none
  effects random
{{
  match random.bytes(24) {{
    Some(b) => {{ let mut i = 0; while i < 24 {{ if b.get(i).unwrap_or(0) != 73 {{ return 0 - 1; }} i = i + 1; }} return b.len(); }}
    None => {{ return 0; }}
  }}
}}
"#
    );
    let implementation = sigil::stdlib::embedded::lookup("std.os.random", &[]).unwrap();
    for (last, expected) in [("return k;", "24"), ("return -5;", "0"), ("return 0;", "0")] {
        let fake = format!(
            r#"
global mut CALLS: Int = 0
fn test_getrandom(n: Int, addr: Int, len: Int, flags: Int, a3: Int, a4: Int, a5: Int) -> Int
  intent "試験だけで EINTR と部分読みを注入する"
  requires none
  ensures none
  effects random
  unchecked "試験領域だけに書く"
{{
  if n != 318 {{ return raw.syscall(n, addr, len, flags, a3, a4, a5); }}
  CALLS = CALLS + 1;
  if CALLS == 1 {{ return -4; }}
  let mut k = len;
  if k > 7 {{ k = 7; }}
  raw.set(raw.int_to_addr(addr), 73, k);
  if CALLS > 2 {{ {last} }}
  return k;
}}
"#
        );
        injected(
            &src,
            "std.os.random",
            implementation.replace("raw.syscall(318", "test_getrandom(318") + &fake,
            expected,
        );
    }
}

#[test]
fn client_effects_ownership_and_hidden_fd_are_checked() {
    let wrapper = |items: &str, body: &str, effects: &str| {
        format!("{HEAD}{items}\npub fn main() -> Int\n  intent \"検査\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n")
    };
    let err = |src: &str| {
        format!(
            "{:?}",
            sigil::ir_eval::lower_sources(&[src], "main").unwrap_err()
        )
    };
    for body in [
        "let c = TcpClient { fd: 0 }; return 1;",
        "match net.connect(&\"127.0.0.1\", 1) { Ok(c) => { return c.fd; } Err(e) => { return 0; } }",
        "match net.connect(&\"127.0.0.1\", 1) { Ok(c) => { let d = c.clone(); return 1; } Err(e) => { return 0; } }",
    ] { assert!(!err(&wrapper("", body, "net.client, task.suspend")).is_empty()); }
    let share = "fn need<T: Shareable>(x: &T) -> Int\n intent \"共有\"\n requires none\n ensures none\n effects none\n{ return 1; }\n";
    assert!(err(&wrapper(share, "match net.connect(&\"127.0.0.1\", 1) { Ok(c) => { return need(&c); } Err(e) => { return 0; } }", "net.client, task.suspend")).contains("Shareable"));
    assert!(err(&wrapper(
        "",
        "let x = net.connect(&\"127.0.0.1\", 1); return 1;",
        "net.server, task.suspend"
    ))
    .contains("net.client"));
    assert!(err(&wrapper("", "let b = random.bytes(24); return 1;", "none")).contains("random"));
    let b = "fn read(c: &mut TcpClient) -> Int\n intent \"読む\"\n requires none\n ensures none\n effects net.server, task.suspend\n{ let b = c.read(1); return 1; }\n";
    assert!(err(&wrapper(b, "return 1;", "none")).contains("net.client"));
    let s = "fn read(c: &TcpStream) -> Int\n intent \"読む\"\n requires none\n ensures none\n effects net.client, task.suspend\n{ let b = c.read(1); return 1; }\n";
    assert!(err(&wrapper(s, "return 1;", "none")).contains("net.server"));
}

#[test]
fn new_syscall_shapes_and_memory_are_restricted() {
    let run = |body: &str| {
        let src = format!("module app.main\n targets server\n trust base\nfn main() -> Int\n intent \"許可表\"\n requires none\n ensures none\n effects none\n unchecked \"不正引数の拒否\"\n{{ {body} }}");
        let opts = Options {
            trust_base: vec!["app.main".into()],
            ..Options::captured()
        };
        format!(
            "{:?}",
            sigil::ir_eval::run_with(&src, "main", &[], &opts).unwrap_err()
        )
    };
    for (body, needle) in [
        ("return raw.syscall(318, 1, 24, 0, 0, 0, 0);", "範囲外"),
        ("return raw.syscall(318, 1, 24, 1, 0, 0, 0);", "flags"),
        ("return raw.syscall(318, 1, 65537, 0, 0, 0, 0);", "長さ"),
        ("let p = raw.alloc(8, 8); return raw.syscall(318, raw.addr_to_int(p), 24, 0, 0, 0, 0);", "範囲外"),
        ("return raw.syscall(42, 3, 0, 8, 0, 0, 0);", "addrlen"),
        ("let p = raw.alloc(16, 8); raw.set(p, 0, 16); return raw.syscall(42, 3, raw.addr_to_int(p), 16, 0, 0, 0);", "AF_INET"),
        ("return raw.syscall(55, 3, 0, 4, 0, 0, 0);", "SO_ERROR"),
        ("let p = raw.alloc(8, 8); raw.store_i64(p, 8); return raw.syscall(55, 3, 1, 4, raw.addr_to_int(p), raw.addr_to_int(p), 0);", "optlen"),
        ("return raw.syscall(55, 3, 1, 4, 1, 1, 0);", "範囲外"),
    ] { let e = run(body); assert!(e.contains(needle), "{body}: {e}"); }
}

#[test]
fn failures_read_write_cancellation_and_fd_reuse_do_not_leak() {
    let src = format!(
        r#"{HEAD}
fn checkpoint(port: Int) -> Int
  intent "相手が fd 数を観測する間、接続を所有して待つ"
  requires 1 <= port && port <= 65535
  ensures none
  effects net.client, task.suspend
{{
  match net.connect(&"127.0.0.1", port) {{
    Ok(c) => {{ let mut stream = c; match stream.read(1) {{ Ok(v) => {{ return v.len(); }} Err(e) => {{ return 0; }} }} }}
    Err(e) => {{ return 0; }}
  }}
}}
fn blocked(port: Int, writing: Bool) -> Int
  intent "取消されるタスクの中で接続を所有する"
  requires 1 <= port && port <= 65535
  ensures none
  effects net.client, task.suspend
{{
  match net.connect(&"127.0.0.1", port) {{
    Ok(value) => {{ let mut conn = value; return blocked_io(&mut conn, writing); }}
    Err(e) => {{ return 0 - 9; }}
  }}
}}
fn blocked_io(c: &mut TcpClient, writing: Bool) -> Int
  intent "接続の読み、または部分送信の後に待つ"
  requires none
  ensures none
  effects net.client, task.suspend
{{
  if writing {{
    let mut b = Bytes.new(); let mut i = 0;
    while i < 65536 {{ b.push(73); i = i + 1; }}
    let mut k = 0;
    while k < 1024 {{ match c.write_all(&b) {{ Ok(u) => {{}} Err(e) => {{ return 0 - 2; }} }} k = k + 1; }}
  }} else {{ match c.read(1) {{ Ok(b) => {{ return b.len(); }} Err(e) => {{ return 0 - 1; }} }} }}
  return 0;
}}
pub fn main(port: Int, refused: Int) -> Int
  intent "失敗と取消で所有値を破棄し、再利用した番号を古い破棄で閉じない"
  requires 1 <= port && port <= 65535 && 1 <= refused && refused <= 65535
  ensures none
  effects net.client, task.suspend, task.spawn, clock
{{
  if checkpoint(port) != 1 {{ return 0 - 3; }}
  let mut i = 0;
  while i < 12 {{
    match net.connect(&"127.0.0.1", refused) {{ Ok(c) => {{ return 0 - 4; }} Err(e) => {{}} }}
    match net.connect(&"localhost", port) {{ Ok(c) => {{ return 0 - 5; }} Err(e) => {{}} }}
    i = i + 1;
  }}
  let mut count = 0;
  i = 0;
  while i < 4 {{
    let writing = i % 2 == 1;
    match task.timeout(30, move || blocked(port, writing)) {{ None => {{ count = count + 1; }} Some(n) => {{ return 0 - 6; }} }}
    if checkpoint(port) != 1 {{ return 0 - 10; }}
    i = i + 1;
  }}
  if checkpoint(port) != 1 {{ return 0 - 8; }}
  return count;
}}
"#
    );
    let p = Prog::new(&src, &[]);
    let refused = listener();
    let refused_port = refused.local_addr().unwrap().port();
    drop(refused);
    for mode in MODES {
        let l = listener();
        let c = p.spawn(
            mode,
            &[
                i64::from(l.local_addr().unwrap().port()),
                i64::from(refused_port),
            ],
        );
        let mut s = accept(&l);
        let fds = |pid| {
            std::fs::read_dir(format!("/proc/{pid}/fd"))
                .unwrap()
                .count()
        };
        let baseline = fds(c.0.id());
        s.write_all(b"x").unwrap();
        let mut tail = Vec::new();
        s.read_to_end(&mut tail).unwrap();
        assert!(tail.is_empty());
        for round in 0..4 {
            let mut s = accept(&l);
            // 次の接続は timeout 完了後の印。これを受けるまで読まないので、
            // 評価器の速度によらず書き側に backpressure がかかる。
            let mut marker = accept(&l);
            assert_eq!(fds(c.0.id()), baseline, "mode {mode}: 取消後の fd が増加");
            let mut got = Vec::new();
            s.read_to_end(&mut got).unwrap();
            if round % 2 == 0 {
                assert!(got.is_empty());
            } else {
                assert!(
                    !got.is_empty() && got.len() < 65536 * 1024,
                    "mode {mode}: {}",
                    got.len()
                );
            }
            marker.write_all(b"x").unwrap();
            let mut end = Vec::new();
            marker.read_to_end(&mut end).unwrap();
            assert!(end.is_empty());
        }
        let mut s = accept(&l);
        assert_eq!(fds(c.0.id()), baseline, "mode {mode}: fd が増加");
        s.write_all(b"x").unwrap();
        let mut tail = Vec::new();
        s.read_to_end(&mut tail).unwrap();
        assert!(tail.is_empty());
        finish(c, "4", mode);
    }
}

#[test]
fn cancellation_before_connect_completion_drops_the_owned_fd() {
    // 実ソケットを使い、完了通知だけを readable 待機に差し替えて遅延する。
    // 環境依存の未応答アドレスや firewall を試験の前提にしない。
    let l = listener();
    let port = l.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        for _ in 0..18 {
            let mut s = accept(&l);
            let mut b = Vec::new();
            s.read_to_end(&mut b).unwrap();
            assert!(b.is_empty(), "接続中の取消からデータを送ってはいけない");
        }
    });
    let src = format!(
        r#"{HEAD}
fn pending() -> Int
  intent "完了通知をまだ受けない接続"
  requires none
  ensures none
  effects net.client, task.suspend
{{
  match net.connect(&"127.0.0.1", {port}) {{ Ok(c) => {{ return 1; }} Err(e) => {{ return 0; }} }}
}}
pub fn main() -> Int
  intent "中断前に包んだ接続を取消で閉じる"
  requires none
  ensures none
  effects net.client, task.suspend, task.spawn, clock
{{
  let mut i = 0; let mut count = 0;
  while i < 6 {{
    match task.timeout(30, move || pending()) {{ None => {{ count = count + 1; }} Some(n) => {{ return 0 - 1; }} }}
    i = i + 1;
  }}
  return count;
}}
"#
    );
    let implementation = sigil::stdlib::embedded::lookup("std.os.net", &[])
        .unwrap()
        .replace(
            "if r == 0 {\n    return Ok(c);\n  }",
            "if r == 99999 {\n    return Ok(c);\n  }",
        )
        .replace(
            "if r != -115 && r != -4",
            "if r != 0 && r != -115 && r != -4",
        )
        .replace("raw.io_wait_fd(c.fd, 4)", "raw.io_wait_fd(c.fd, 1)");
    injected(&src, "std.os.net", implementation, "6");
    server.join().unwrap();
}

#[test]
fn limit_random_and_connect_arguments_are_enforced_in_both_engines() {
    for (body, effects, values) in [
        ("match random.bytes(n) { Some(b) => { return b.len(); } None => { return 0; } }", "random", vec![0, 65537]),
        ("match net.connect(&\"127.0.0.1\", n) { Ok(c) => { return 1; } Err(e) => { return 0; } }", "net.client, task.suspend", vec![0, 65536]),
    ] {
        let src = format!("{HEAD}pub fn main(n: Int) -> Int\n intent \"公開入口の上限\"\n requires none\n ensures none\n effects {effects}\n{{ {body} }}");
        for n in values {
            let failure = common::run(&src, "main", &[Arg::Int(n)]).unwrap_err();
            assert!(matches!(failure.error, RunError::Stop(ref s) if matches!(s.kind, StopKind::RequiresViolated | StopKind::StdRequiresViolated)), "{failure:?}");
        }
    }
}

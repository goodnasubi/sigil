//! ADR 0031 決定 3・5-2 / PR 6-3e: TCP のソケット（`std.os.net`。`net.listen`・`TcpListener`・`TcpStream`・`NetError`）。
//!
//! 突き合わせ（決定 5-2）: ソケットのプログラムは外の相手を要るので、この試験が Rust の `std::net` のクライアントとして、
//! 評価器で動かしたプログラム（`sigil run` の子プロセス）と、ネイティブにビルドして動かしたプログラム（ワーカー 1 と 4）の
//! **両方に同じ入力のバイト列を送り**、受け取った応答のバイト列・最後の値・終了コードを比べる。
//!
//! - 待ち受けは `port` を 0 にして、`local_port` の番号を標準出力の 1 行目に出す（タスクを起動する前なので、行は混ざらない）。
//! - タスクの結果は `console.log` でなく通信路で入口へ集め、入口が最後の値として返す（複数ワーカーの行の混ざりに依らない）。
//! - 漏れ: `SIGIL_RT_LIVE` で、評価器は `frames=0 chunks=0`、ネイティブは `sigil_blocks=0` を確かめる。
//!   fd の漏れは `/proc/<pid>/fd` の数で確かめる。
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

/// 終わったプログラムの観測（最後の値・終了コード・標準エラー）。
#[derive(Debug)]
struct Done {
    value: String,
    code: Option<i32>,
    stderr: String,
}

/// 子プロセスを、試験が途中で落ちても残さない。
struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// 1 つの Sigil のプログラム（入口は `main`）。評価器用のソースと、ネイティブの実行ファイルを持つ。
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

    /// プログラムを起動し、1 行目の待ち受けの番号を読んで `client(番号, プロセスID)` を動かし、終わりを待つ。
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

    /// 相手を要らないプログラムを最後まで動かす。
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

/// 正常に終わり、値が `value` で、漏れが無いこと。
fn clean(mode: Mode, d: &Done, value: &str) {
    assert_eq!(d.code, Some(0), "{mode:?}: {d:?}");
    assert_eq!(d.value, value, "{mode:?}: {d:?}");
    let want = match mode {
        Mode::Eval => "live: frames=0 chunks=0",
        Mode::Native(_) => "live: sigil_blocks=0",
    };
    assert!(d.stderr.contains(want), "{mode:?}: 漏れがある: {d:?}");
}

fn connect(port: u16) -> TcpStream {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.set_write_timeout(Some(Duration::from_secs(30))).unwrap();
    s
}

/// 接続して `payload` を送り、書き込み側を閉じて、相手が閉じるまでの応答を全部読む。
fn exchange(port: u16, payload: &[u8]) -> Vec<u8> {
    let mut s = connect(port);
    s.write_all(payload).unwrap();
    s.shutdown(Shutdown::Write).unwrap();
    let mut got = vec![];
    s.read_to_end(&mut got).unwrap();
    got
}

fn payload(i: usize, len: usize) -> Vec<u8> {
    (0..len).map(|k| ((k * 7 + i * 31) % 251) as u8).collect()
}

const HEAD: &str = r#"module app.main
  targets server
  trust none

struct Conn {
  s: TcpStream,
  tx: Sender<Int>,
}

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

fn reply(s: &TcpStream, data: &Bytes) -> Int
  intent "全部書いて、書き込み側を閉じる。書いた数（失敗は負）"
  requires none
  ensures none
  effects net.server, task.suspend
{
  match s.write_all(&data) {
    Ok(u) => {}
    Err(e) => { return 0 - 2; }
  }
  match s.shutdown() {
    Ok(u) => {}
    Err(e) => { return 0 - 3; }
  }
  return data.len();
}

fn echo(c: &Conn) -> Int
  intent "全部読んで、そのまま返す"
  requires none
  ensures none
  effects net.server, task.suspend
{
  match read_all(&c.s) {
    Ok(all) => { return reply(&c.s, &all); }
    Err(e) => { return 0 - 1; }
  }
}
"#;

/// `n` 個の接続を受け付け、上限 `limit` のルートで `handler` を動かし、結果の合計を返す入口。
fn server_main(handler: &str, on_abort: &str) -> String {
    format!(
        r#"
pub fn main(n: Int, limit: Int) -> Int
  intent "x"
  requires none
  ensures none
  effects net.server, console, task.spawn, task.suspend, task.share
{{
  let host = "127.0.0.1";
  match net.listen(&host, 0, 64) {{
    Ok(l) => {{
      console.log(&l.local_port());
      let (tx, rx) = task.channel();
      let got = task.roots(limit, |g| {{
        let mut i = 0;
        let mut started = 0;
        while i < n {{
          let slot = g.reserve();
          match l.accept() {{
            Ok(st) => {{
              slot.start(Conn {{ s: st, tx: tx.clone() }}, move |c| {{ {handler} }}, move |c| {{ {on_abort} }});
              started = started + 1;
            }}
            Err(e) => {{}}
          }}
          i = i + 1;
        }}
        return started;
      }});
      let mut total = 0;
      let mut k = 0;
      while k < got {{
        total = total + rx.recv();
        k = k + 1;
      }}
      return total;
    }}
    Err(e) => {{ return 0 - 100; }}
  }}
}}
"#
    )
}

fn echo_prog(tag: &str) -> Prog {
    Prog::new(
        tag,
        &format!(
            "{HEAD}{}",
            server_main("c.tx.send(echo(&c));", "c.tx.send(0 - 9);")
        ),
    )
}

/// 接続を 1 つずつ逐次に送ると、評価器・ネイティブ（ワーカー 1・4）で応答のバイト列と最後の値が一致する。
/// 大きさ 0・1 回の読みより大きいもの・送信の緩衝より大きいもの（書きの待ちと分割）を含む。
#[test]
fn sequential_echo_matches_between_evaluator_and_native() {
    let p = echo_prog("net-seq");
    let sizes = [0usize, 1, 4096, 4097, 70_000, 600_000];
    let total: usize = sizes.iter().sum();
    let mut seen: Vec<Vec<Vec<u8>>> = vec![];
    for mode in MODES {
        let (got, d) = p.serve(mode, &[sizes.len() as i64, 2], |port, _| {
            sizes
                .iter()
                .enumerate()
                .map(|(i, &n)| exchange(port, &payload(i, n)))
                .collect::<Vec<_>>()
        });
        clean(mode, &d, &total.to_string());
        for (i, &n) in sizes.iter().enumerate() {
            assert!(got[i] == payload(i, n), "{mode:?}: 接続 {i} の応答が違う");
        }
        seen.push(got);
    }
    assert!(
        seen[0] == seen[1] && seen[0] == seen[2],
        "評価器とネイティブの応答が違う"
    );
}

/// 複数の接続を並行に送っても、接続ごとの応答は自分の入力と一致する（順序に依らない性質だけを比べる）。
#[test]
fn concurrent_echo_keeps_each_connection_separate() {
    let p = echo_prog("net-conc");
    let n = 12usize;
    for mode in MODES {
        let (got, d) = p.serve(mode, &[n as i64, 4], |port, _| {
            let hs: Vec<_> = (0..n)
                .map(|i| std::thread::spawn(move || exchange(port, &payload(i, 3000 + 997 * i))))
                .collect();
            hs.into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        let total: usize = (0..n).map(|i| 3000 + 997 * i).sum();
        clean(mode, &d, &total.to_string());
        for (i, g) in got.iter().enumerate() {
            assert!(
                *g == payload(i, 3000 + 997 * i),
                "{mode:?}: 接続 {i} の応答が違う"
            );
        }
    }
}

/// 1 つの接続の上で、読みと書きを何度も行き来する（同じ fd で待ちを繰り返す）。
#[test]
fn a_connection_can_alternate_reads_and_writes() {
    let handler = r#"
fn chat(c: &Conn) -> Int
  intent "読めた分をすぐ返すことを、相手が終えるまで繰り返す"
  requires none
  ensures none
  effects net.server, task.suspend
{
  let mut n = 0;
  let mut go = true;
  while go {
    match c.s.read(100) {
      Ok(b) => {
        if b.is_empty() {
          go = false;
        } else {
          match c.s.write_all(&b) {
            Ok(u) => { n = n + b.len(); }
            Err(e) => { return 0 - 2; }
          }
        }
      }
      Err(e) => { return 0 - 1; }
    }
  }
  return n;
}
"#;
    let p = Prog::new(
        "net-chat",
        &format!(
            "{HEAD}{handler}{}",
            server_main("c.tx.send(chat(&c));", "c.tx.send(0 - 9);")
        ),
    );
    for mode in MODES {
        let (_, d) = p.serve(mode, &[1, 1], |port, _| {
            let mut s = connect(port);
            for round in 0..50usize {
                let msg = payload(round, 1 + round % 90);
                s.write_all(&msg).unwrap();
                let mut back = vec![0u8; msg.len()];
                s.read_exact(&mut back).unwrap();
                assert!(back == msg, "{mode:?}: {round} 回目の応答が違う");
                if round % 10 == 0 {
                    std::thread::sleep(Duration::from_millis(3));
                }
            }
            s.shutdown(Shutdown::Write).unwrap();
            let mut tail = vec![];
            s.read_to_end(&mut tail).unwrap();
            assert!(tail.is_empty(), "{mode:?}");
        });
        let total: usize = (0..50usize).map(|r| 1 + r % 90).sum();
        clean(mode, &d, &total.to_string());
    }
}

/// `NetError` の腕を `match` で取り出せる。形の違う host は `Other`、使用中の番号は `AddrInUse`。
#[test]
fn listen_errors_are_matchable_net_errors() {
    let src = r#"module app.main
  targets server
  trust none

fn kind(r: Result<TcpListener, NetError>) -> Int
  intent "結果を数にする"
  requires none
  ensures none
  effects none
{
  match r {
    Ok(l) => { return 0; }
    Err(e) => {
      match e {
        NetError.AddrInUse => { return 1; }
        NetError.PermissionDenied => { return 2; }
        NetError.Closed => { return 3; }
        NetError.Other => { return 4; }
      }
    }
  }
}

fn bad(host: &String) -> Int
  intent "host で待ち受けを試す"
  requires none
  ensures none
  effects net.server
{
  return kind(net.listen(&host, 0, 1));
}

pub fn main() -> Int
  intent "x"
  requires none
  ensures none
  effects net.server
{
  let h = "127.0.0.1";
  let mut v = 0;
  v = v * 10 + bad(&"localhost");
  v = v * 10 + bad(&"256.0.0.1");
  v = v * 10 + bad(&"1.2.3");
  v = v * 10 + bad(&"01.2.3.4");
  v = v * 10 + bad(&"1.2.3.4.5");
  v = v * 10 + bad(&"");
  v = v * 10 + bad(&"127.0.0.1 ");
  match net.listen(&h, 0, 1) {
    Ok(l) => {
      v = v * 10 + kind(net.listen(&h, l.local_port(), 1));
      v = v * 10 + bad(&"127.0.0.1");
    }
    Err(e) => { v = v * 10 + 9; }
  }
  return v;
}
"#;
    let p = Prog::new("net-err", src);
    for mode in MODES {
        clean(mode, &p.run(mode, &[]), "444444410");
    }
}

/// `requires` の違反（番号・backlog・読みの大きさの範囲）は、評価器でもネイティブでも異常停止（終了コード 70）になる。
#[test]
fn contract_violations_stop_in_both() {
    for (i, call) in [
        "net.listen(&h, 65536, 1)",
        "net.listen(&h, 0 - 1, 1)",
        "net.listen(&h, 0, 0)",
        "net.listen(&h, 0, 1025)",
    ]
    .iter()
    .enumerate()
    {
        let src = format!(
            "module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects net.server\n{{\n  let h = \"127.0.0.1\";\n  match {call} {{\n    Ok(l) => {{ return 1; }}\n    Err(e) => {{ return 2; }}\n  }}\n}}\n"
        );
        let p = Prog::new(&format!("net-req{i}"), &src);
        for mode in MODES {
            let d = p.run(mode, &[]);
            assert_eq!(d.code, Some(70), "{mode:?} {call}: {d:?}");
        }
    }
    let read = r#"
fn greedy(c: &Conn) -> Int
  intent "上限を超える大きさで読む"
  requires none
  ensures none
  effects net.server, task.suspend
{
  match c.s.read(65537) {
    Ok(b) => { return 1; }
    Err(e) => { return 2; }
  }
}
"#;
    // 起動したルートの異常停止は、そのルートだけを止める（接続は `on_abort` に渡る）。
    let p = Prog::new(
        "net-req-read",
        &format!(
            "{HEAD}{read}{}",
            server_main("c.tx.send(greedy(&c));", "c.tx.send(0 - 9);")
        ),
    );
    for mode in MODES {
        let (got, d) = p.serve(mode, &[1, 1], |port, _| exchange(port, b""));
        assert!(got.is_empty(), "{mode:?}");
        clean(mode, &d, "-9");
    }
}

/// 閉じた接続への `write_all` は `Err(Closed)` になり、`SIGPIPE` でプロセスが落ちない。
#[test]
fn writing_to_a_closed_peer_is_closed_not_sigpipe() {
    let handler = r#"
fn flood(c: &Conn) -> Int
  intent "相手が閉じた後も書き続けて、失敗の種類を返す"
  requires none
  ensures none
  effects net.server, task.suspend
{
  match read_all(&c.s) {
    Ok(all) => {}
    Err(e) => {}
  }
  let mut chunk = Bytes.with_capacity(65536);
  let mut i = 0;
  while i < 65536 {
    chunk.push(i % 251);
    i = i + 1;
  }
  let mut k = 0;
  while k < 400 {
    match c.s.write_all(&chunk) {
      Ok(u) => {}
      Err(e) => {
        match e {
          NetError.Closed => { return 1; }
          _ => { return 2; }
        }
      }
    }
    k = k + 1;
  }
  return 0;
}
"#;
    let p = Prog::new(
        "net-closed",
        &format!(
            "{HEAD}{handler}{}",
            server_main("c.tx.send(flood(&c));", "c.tx.send(0 - 9);")
        ),
    );
    for mode in MODES {
        let (_, d) = p.serve(mode, &[1, 1], |port, _| {
            let mut s = connect(port);
            s.write_all(b"bye").unwrap();
            // 読まずに閉じる。相手の書き込みには RST が返る。
            drop(s);
        });
        clean(mode, &d, "1");
    }
}

/// `task.timeout` で待ちを取り消すと、待っていたタスクが持つ `TcpListener` は破棄で閉じられ、漏れない。
/// その後に作ったソケット（同じ fd の番号を再利用しうる）は、古い待ちの登録に邪魔されずに動く。
#[test]
fn a_cancelled_accept_closes_its_listener_and_the_fd_can_be_reused() {
    let src = format!(
        r#"{HEAD}
fn wait_one(l: &TcpListener) -> Int
  intent "来ない接続を待つ"
  requires none
  ensures none
  effects net.server, task.suspend
{{
  match l.accept() {{
    Ok(st) => {{ return 1; }}
    Err(e) => {{ return 2; }}
  }}
}}

fn first(rounds: Int) -> Int
  intent "誰も来ない待ち受けの accept を、時間切れで取り消すことを繰り返す"
  requires none
  ensures none
  effects net.server, clock, task.spawn, task.suspend
{{
  let h = "127.0.0.1";
  let mut miss = 0;
  let mut i = 0;
  while i < rounds {{
    match net.listen(&h, 0, 4) {{
      Ok(l) => {{
        match task.timeout(30, move || wait_one(&l)) {{
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
  match net.listen(&h, 0, 4) {{
    Ok(l) => {{
      console.log(&l.local_port());
      match l.accept() {{
        Ok(st) => {{
          match read_all(&st) {{
            Ok(all) => {{ return miss * 1000 + reply(&st, &all); }}
            Err(e) => {{ return 0 - 1; }}
          }}
        }}
        Err(e) => {{ return 0 - 2; }}
      }}
    }}
    Err(e) => {{ return 0 - 3; }}
  }}
}}
"#
    );
    let p = Prog::new("net-cancel", &src);
    for mode in MODES {
        let (got, d) = p.serve(mode, &[5], |port, pid| {
            // 取り消された 5 つの待ち受けは閉じられている（開いている待ち受けのソケットは今の 1 つだけ）。
            let n = socket_fds(pid);
            (exchange(port, b"again"), n)
        });
        assert_eq!(got.0, b"again", "{mode:?}");
        assert_eq!(got.1, 1, "{mode:?}: 取り消した待ち受けの fd が残っている");
        clean(mode, &d, "5005");
    }
}

/// `/proc/<pid>/fd` のうち、ソケットを指すものの数。
fn socket_fds(pid: u32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|e| std::fs::read_link(e.ok()?.path()).ok())
        .filter(|t| t.to_string_lossy().starts_with("socket:"))
        .count()
}

/// 読みで待っているルートごと、集まりを時間切れで取り消すと、接続は破棄で閉じられ（相手は終了を見る）、漏れない。
#[test]
fn cancelling_the_group_closes_connections_blocked_in_read() {
    let src = format!(
        r#"{HEAD}
fn accept_loop(l: &TcpListener, tx: &Sender<Int>) -> Int
  intent "受け付けたルートは、来ない入力を読みで待つ"
  requires none
  ensures none
  effects net.server, task.spawn, task.suspend, task.share
{{
  return task.roots(8, |g| {{
    while true {{
      let slot = g.reserve();
      match l.accept() {{
        Ok(st) => {{
          slot.start(Conn {{ s: st, tx: tx.clone() }}, move |c| {{ c.tx.send(echo(&c)); }}, move |c| {{ c.tx.send(0 - 9); }});
        }}
        Err(e) => {{}}
      }}
    }}
    return 0;
  }});
}}

pub fn main(ms: Int) -> Int
  intent "x"
  requires none
  ensures none
  effects net.server, console, clock, task.spawn, task.suspend, task.share
{{
  let h = "127.0.0.1";
  match net.listen(&h, 0, 16) {{
    Ok(l) => {{
      console.log(&l.local_port());
      let (tx, rx) = task.channel();
      match task.timeout(ms, move || accept_loop(&l, &tx)) {{
        Some(v) => {{ return v; }}
        None => {{ return 7; }}
      }}
    }}
    Err(e) => {{ return 0 - 3; }}
  }}
}}
"#
    );
    let p = Prog::new("net-cancel-read", &src);
    for mode in MODES {
        let (got, d) = p.serve(mode, &[1500], |port, _| {
            let mut cs: Vec<TcpStream> = (0..3).map(|_| connect(port)).collect();
            // 何も送らない。集まりが取り消されると、相手が閉じて読みが終わる（0 バイト）。
            cs.iter_mut()
                .map(|s| {
                    let mut b = vec![];
                    s.read_to_end(&mut b).map(|_| b.len())
                })
                .collect::<Vec<_>>()
        });
        for r in &got {
            // 読まれていない入力は無いので、閉じ方は FIN（0 バイトの終了）になる。
            assert!(matches!(r, Ok(0)), "{mode:?}: {got:?}");
        }
        clean(mode, &d, "7");
    }
}

/// 起動したルートが異常停止しても、接続は閉じられずに `on_abort` に渡り、500 を書いて閉じられる。
/// ほかの接続は処理され続ける（ADR 0031 決定 4-2）。
#[test]
fn on_abort_receives_the_open_connection_and_other_roots_continue() {
    let handler = r#"
fn risky(c: &Conn) -> Int
  intent "入力の先頭が x なら異常停止する。違えばそのまま返す"
  requires none
  ensures none
  effects net.server, task.suspend
{
  match read_all(&c.s) {
    Ok(all) => {
      let first = all.get(0).unwrap_or(0);
      let d = 10 / (first - 120);
      return reply(&c.s, &all);
    }
    Err(e) => { return 0 - 1; }
  }
}

fn fail500(c: Conn)
  intent "500 を書いて閉じる"
  requires none
  ensures none
  effects net.server, task.suspend, task.share
{
  let mut m = Bytes.new();
  m.push(53);
  m.push(48);
  m.push(48);
  c.tx.send(0 - reply(&c.s, &m));
}
"#;
    let p = Prog::new(
        "net-abort",
        &format!(
            "{HEAD}{handler}{}",
            server_main("c.tx.send(risky(&c));", "fail500(c);")
        ),
    );
    for mode in MODES {
        let (got, d) = p.serve(mode, &[4, 2], |port, _| {
            [&b"ok-1"[..], b"x-boom", b"ok-22", b"xx"]
                .iter()
                .map(|m| exchange(port, m))
                .collect::<Vec<_>>()
        });
        assert_eq!(got[0], b"ok-1", "{mode:?}");
        assert_eq!(got[1], b"500", "{mode:?}");
        assert_eq!(got[2], b"ok-22", "{mode:?}");
        assert_eq!(got[3], b"500", "{mode:?}");
        // 4 + 5 から、on_abort の 2 回分（-3 ずつ）を引く。
        clean(mode, &d, "3");
    }
}

/// 読みで待っている間に、同じルートの子のタスクが異常停止する。待ちの登録は外れ、接続は開いたまま `on_abort` に渡る。
/// `on_abort` は同じ接続（同じ fd）でもう一度待てる（古い待ちの登録に邪魔されない）。
#[test]
fn on_abort_can_wait_again_on_a_connection_whose_wait_was_cancelled() {
    let handler = r#"
fn boom(ms: Int) -> Int
  intent "少し待ってから異常停止する"
  requires none
  ensures none
  effects clock, task.suspend
{
  task.sleep(ms);
  let z = ms - ms;
  return 10 / z;
}

fn stuck(c: &Conn) -> Int
  intent "来ない入力を読みで待つ間に、子のタスクが異常停止する"
  requires none
  ensures none
  effects net.server, clock, task.spawn, task.suspend
{
  return task.scope(|s| {
    let h = s.spawn(move || boom(60));
    let r = echo(&c);
    return r + h.join();
  });
}

fn late500(c: Conn)
  intent "相手の入力を待ってから、500 を書いて閉じる"
  requires none
  ensures none
  effects net.server, task.suspend, task.share
{
  let mut m = Bytes.new();
  m.push(53);
  m.push(48);
  m.push(48);
  match read_all(&c.s) {
    Ok(all) => { c.tx.send(all.len() * 100 + reply(&c.s, &m)); }
    Err(e) => { c.tx.send(0 - 1); }
  }
}
"#;
    let p = Prog::new(
        "net-abort-wait",
        &format!(
            "{HEAD}{handler}{}",
            server_main("c.tx.send(stuck(&c));", "late500(c);")
        )
        .replace(
            "effects net.server, console, task.spawn, task.suspend, task.share\n{\n  let host",
            "effects net.server, console, clock, task.spawn, task.suspend, task.share\n{\n  let host",
        ),
    );
    for mode in MODES {
        let (got, d) = p.serve(mode, &[2, 2], |port, _| {
            let hs: Vec<_> = (0..2)
                .map(|_| {
                    std::thread::spawn(move || {
                        let mut s = connect(port);
                        // 子のタスクが止まり、on_abort が読みで待ち始めた後に送る。
                        std::thread::sleep(Duration::from_millis(400));
                        s.write_all(b"late").unwrap();
                        s.shutdown(Shutdown::Write).unwrap();
                        let mut got = vec![];
                        s.read_to_end(&mut got).unwrap();
                        got
                    })
                })
                .collect();
            hs.into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert!(got.iter().all(|g| g == b"500"), "{mode:?}: {got:?}");
        clean(mode, &d, "806");
    }
}

/// `reserve` の上限にある間は `accept` を呼ばないので、上限を超える接続は受け付けられない（ADR 0020 決定 4 の同時の接続の数）。
#[test]
fn reserve_keeps_accepted_connections_within_the_limit() {
    let handler = r#"
fn greet(c: &Conn) -> Int
  intent "受け付けたらすぐ 1 バイト書き、相手が終えるまで待つ"
  requires none
  ensures none
  effects net.server, task.suspend
{
  let mut m = Bytes.new();
  m.push(65);
  match c.s.write_all(&m) {
    Ok(u) => {}
    Err(e) => { return 0 - 2; }
  }
  match read_all(&c.s) {
    Ok(all) => { return 1; }
    Err(e) => { return 0 - 1; }
  }
}
"#;
    let p = Prog::new(
        "net-limit",
        &format!(
            "{HEAD}{handler}{}",
            server_main("c.tx.send(greet(&c));", "c.tx.send(0 - 9);")
        ),
    );
    for mode in MODES {
        let (_, d) = p.serve(mode, &[4, 2], |port, _| {
            let mut cs: Vec<TcpStream> = (0..4).map(|_| connect(port)).collect();
            let mut b = [0u8; 1];
            // 先の 2 つは受け付けられて挨拶が来る。
            for s in cs.iter_mut().take(2) {
                s.read_exact(&mut b).unwrap();
                assert_eq!(b[0], b'A', "{mode:?}");
            }
            // 上限にある間、3 つ目は（カーネルの待ち行列にあるだけで）受け付けられない。
            cs[2]
                .set_read_timeout(Some(Duration::from_millis(300)))
                .unwrap();
            assert!(
                cs[2].read_exact(&mut b).is_err(),
                "{mode:?}: 上限を超えて受け付けた"
            );
            cs[2]
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            // 先の 2 つが終わると、残りが受け付けられる。
            for s in cs.iter_mut().take(2) {
                s.shutdown(Shutdown::Write).unwrap();
            }
            for s in cs.iter_mut().skip(2) {
                s.read_exact(&mut b).unwrap();
                assert_eq!(b[0], b'A', "{mode:?}");
                s.shutdown(Shutdown::Write).unwrap();
            }
        });
        clean(mode, &d, "4");
    }
}

/// 接続を大量に作って閉じても、開いている fd の数は増えない（fd の漏れが無い。二重の close も無い）。
#[test]
fn fds_do_not_leak_across_many_connections() {
    let p = echo_prog("net-fds");
    /// 数が落ち着くまで待って読む（相手が終了を見た時点では、破棄の `close` がまだのことがある）。
    fn settled(pid: u32) -> usize {
        let mut last = usize::MAX;
        let mut same = 0;
        for _ in 0..500 {
            let n = socket_fds(pid);
            same = if n == last { same + 1 } else { 0 };
            last = n;
            if same >= 5 {
                break;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        last
    }
    for mode in MODES {
        let n = 60usize;
        let ((before, after), d) = p.serve(mode, &[n as i64 + 1, 3], |port, pid| {
            for i in 0..5 {
                exchange(port, &payload(i, 100));
            }
            let before = settled(pid);
            for i in 5..n {
                exchange(port, &payload(i, 100));
            }
            let after = settled(pid);
            exchange(port, &payload(n, 100));
            (before, after)
        });
        // 残るのは待ち受けのソケットだけ。
        assert_eq!((before, after), (1, 1), "{mode:?}");
        clean(mode, &d, &((n + 1) * 100).to_string());
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
    format!(
        "module app.main\n  targets server\n  trust none\n\n{items}\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects net.server\n{{\n  let h = \"127.0.0.1\";\n{body}\n}}\n"
    )
}

/// fd を所有する値は複製できない（直接でも、構造体の中でも、型引数を通しても）。
#[test]
fn fd_owning_values_cannot_be_cloned() {
    let direct = with_main(
        "",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => {\n      let l2 = l.clone();\n      return 1;\n    }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&direct);
    assert!(m.starts_with("NoSuchMember") && m.contains("clone"), "{m}");

    let wrapped = with_main(
        "struct Holder {\n  l: TcpListener,\n  n: Int,\n}\n",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => {\n      let a = Holder { l: l, n: 1 };\n      let b = a.clone();\n      return 1;\n    }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&wrapped);
    assert!(m.starts_with("NoSuchMember") && m.contains("clone"), "{m}");

    let generic = with_main(
        "fn dup<T>(x: &T) -> T\n  intent \"複製\"\n  requires none\n  ensures none\n  effects none\n{\n  return x.clone();\n}\n",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => {\n      let l2 = dup(&l);\n      return 1;\n    }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&generic);
    assert!(m.contains("clone") && m.contains("TcpListener"), "{m}");

    let mapped = with_main(
        "fn same(l: TcpListener) -> TcpListener\n  intent \"そのまま\"\n  requires none\n  ensures none\n  effects none\n{\n  return l;\n}\n",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => {\n      let mut xs = List.new();\n      xs.push(l);\n      let ys = xs.map(same);\n      return 1;\n    }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&mapped);
    assert!(m.starts_with("NoSuchMember") && m.contains("map"), "{m}");

    // 型引数を通した `map` は単相化の後でしか分からないので、lowering が止める。
    let generic_map = with_main(
        "fn same<T>(x: T) -> T\n  intent \"そのまま\"\n  requires none\n  ensures none\n  effects none\n{\n  return x;\n}\n\nfn remap<T>(xs: &List<T>) -> Int\n  intent \"写す\"\n  requires none\n  ensures none\n  effects none\n{\n  let ys = xs.map(same);\n  return ys.len();\n}\n",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => {\n      let mut xs = List.new();\n      xs.push(l);\n      return remap(&xs);\n    }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&generic_map);
    assert!(m.starts_with("Unsupported") && m.contains("map"), "{m}");
}

/// ソケットの型の名前は前置きの名前で、利用者は同じ名前の型を宣言できない。`net` で始まるモジュール名も使えない。
#[test]
fn socket_names_are_reserved() {
    for name in ["TcpListener", "TcpStream", "NetError"] {
        let m = check_err(&with_main(
            &format!("struct {name} {{\n  x: Int,\n}}\n"),
            "  return 1;",
        ));
        assert!(m.starts_with("DuplicateDefinition"), "{name}: {m}");
    }
    let m = check_err(
        "module net.a\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n",
    );
    assert!(m.starts_with("ReservedModuleName"), "{m}");
}

/// `TcpStream` は `Sendable` だが `Shareable` でない。fd の番号は取り出せず、番号から作ることもできない。
#[test]
fn socket_types_are_sendable_not_shareable_and_hide_the_fd() {
    let share = with_main(
        "fn need<T: Shareable>(x: &T) -> Int\n  intent \"共有できる型だけ\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => { return need(&l); }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&share);
    assert!(m.contains("Shareable"), "{m}");

    let field = with_main(
        "",
        "  match net.listen(&h, 0, 1) {\n    Ok(l) => { return l.fd; }\n    Err(e) => { return 2; }\n  }",
    );
    let m = check_err(&field);
    assert!(m.contains("fd"), "{m}");

    let forge = with_main("", "  let s = TcpStream { fd: 0 };\n  return 1;");
    let m = check_err(&forge);
    assert!(m.contains("TcpStream"), "{m}");
}

/// `bare` を出力先に持つモジュールからは使えない（OS 層。ADR 0014）。
#[test]
fn net_is_not_available_on_bare() {
    let src = "module app.main\n  targets bare, server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let h = \"127.0.0.1\";\n  match net.listen(&h, 0, 1) {\n    Ok(l) => { return 1; }\n    Err(e) => { return 2; }\n  }\n}\n";
    let m = check_err(src);
    assert!(m.starts_with("OsItemInCore"), "{m}");
}

/// `std.os.net` の `NetError` の腕の並びは、検査器が登録する並び（`NET_ERROR_ARMS`）と同じ。
#[test]
fn net_error_arms_match_the_embedded_declaration() {
    let src = sigil::stdlib::embedded::lookup("std.os.net", &[]).unwrap();
    let body = src
        .split("pub enum NetError {")
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
    assert_eq!(arms, sigil::stdlib::NET_ERROR_ARMS);
}

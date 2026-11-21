//! ADR 0034 6-5d: 同じ外部応答を評価器/native 1・4 workersへ与える。
//! 認証後のsocketだけを試験差し込みし、query本体・期限・破棄は製品と同じSIRを使う。
mod common;
use common::native::scratch;
use sigil::exec::{Arg, Options};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::Duration;

const SOURCE: &str = r#"module app.main
  targets server
  trust none
pub fn main(port: Int, scenario: Int) -> Int
  intent "外部相手との結果と同期と閉鎖を照合する"
  requires none
  ensures none
  effects net.client, random, task.suspend, task.spawn, clock
{
  match exercise(port, scenario) { Ok(n) => { return n; } Err(e) => { return 99; } }
}
fn exercise(port: Int, scenario: Int) -> Result<Int, PgError>
  intent "認証後の同じ接続で単一文を実行する"
  requires none
  ensures none
  effects net.client, random, task.suspend, task.spawn, clock
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let mut conn = pg.connect(cfg, &pg.default_connect_limits())?;
  let mut params = List.new();
  params.push(pg.param(16, Some(PgValue.Boolean(true)))?);
  params.push(pg.param(21, Some(PgValue.Integer(0 - 32768)))?);
  params.push(pg.param(23, Some(PgValue.Integer(2147483647)))?);
  params.push(pg.param(20, Some(PgValue.Integer((0 - 9223372036854775807) - 1)))?);
  params.push(pg.param(25, Some(PgValue.Text("日本語 ' ; select")))?);
  params.push(pg.param(1043, Some(PgValue.Text("")))?);
  params.push(pg.param(17, Some(PgValue.Binary("\u{0}x".as_bytes())))?);
  params.push(pg.param(25, None)?);
  params.push(pg.param(17, Some(PgValue.Binary(Bytes.new())))?);
  let mut limits = pg.default_query_limits();
  if scenario == 3 || scenario == 15 || scenario == 17 { limits = limits.with_timeout(250)?; }
  if scenario == 8 { limits = limits.with_result_bounds(40, 10000)?; }
  if scenario == 9 { limits = limits.with_result_bounds(4194304, 0)?; }
  if scenario == 16 {
    match task.timeout(250, || conn.query(&"select $1", &params, &limits)) {
      None => { if !conn.is_closed() { return Ok(98); } return Ok(8); }
      Some(v) => { return Ok(78); }
    }
  }
  if scenario == 18 {
    match conn.query(&"bad\u{0}sql", &params, &limits) {
      Err(PgError.Protocol) => { if conn.is_closed() { return Ok(98); } }
      _ => { return Ok(78); }
    }
  }
  let sql = "select $1,$2,$3,$4,$5,$6,$7,$8,$9";
  match conn.query(&sql, &params, &limits) {
    Ok(answer) => {
      if conn.is_closed() { return Ok(90); }
      if scenario == 0 {
        if answer.columns() != 9 || answer.rows() != 1 || answer.command() != "SELECT 1" || answer.status() != 73 { return Ok(91); }
        match answer.column(4) { Some(c) => { if c.type_oid() != 25 || c.column_name() != "c4" { return Ok(92); } } None => { return Ok(93); } }
        match answer.row(0) {
          Some(r) => {
            match r.get(0) { Some(Some(PgValue.Boolean(true))) => {} _ => { return Ok(81); } }
            match r.get(1) { Some(Some(PgValue.Integer(n))) => { if n != 0 - 32768 { return Ok(82); } } _ => { return Ok(83); } }
            match r.get(2) { Some(Some(PgValue.Integer(n))) => { if n != 2147483647 { return Ok(84); } } _ => { return Ok(85); } }
            match r.get(3) { Some(Some(PgValue.Integer(n))) => { if n != (0 - 9223372036854775807) - 1 { return Ok(86); } } _ => { return Ok(87); } }
            match r.get(4) { Some(Some(PgValue.Text(t))) => { if t != "日本語 ' ; select" { return Ok(88); } } _ => { return Ok(89); } }
            match r.get(5) { Some(Some(PgValue.Text(t))) => { if t != "" { return Ok(71); } } _ => { return Ok(72); } }
            match r.get(6) { Some(Some(PgValue.Binary(b))) => { if b.byte_len() != 2 || b.get(0).unwrap_or(1) != 0 { return Ok(73); } } _ => { return Ok(74); } }
            match r.get(7) { Some(None) => {} _ => { return Ok(75); } }
            match r.get(8) { Some(Some(PgValue.Binary(b))) => { if b.byte_len() != 0 { return Ok(76); } } _ => { return Ok(77); } }
          }
          None => { return Ok(94); }
        }
      }
      if scenario == 13 && answer.status() != 84 { return Ok(95); }
      return Ok(0);
    }
    Err(PgError.SqlState(code)) => {
      if code != "42601" || conn.is_closed() { return Ok(96); }
      // 文エラー後は同じfdで次の単一文を実行できる。
      let again = conn.query(&"ROLLBACK", &List.new(), &pg.default_query_limits())?;
      if again.command() != "ROLLBACK" || again.status() != 73 { return Ok(97); }
      return Ok(1);
    }
    Err(PgError.OutcomeUnknown) => { if !conn.is_closed() { return Ok(98); } return Ok(2); }
    Err(PgError.Timeout) => { if !conn.is_closed() { return Ok(98); } return Ok(3); }
    Err(PgError.Protocol) => { if !conn.is_closed() { return Ok(98); } return Ok(4); }
    Err(PgError.TypeMismatch) => { if !conn.is_closed() { return Ok(98); } return Ok(7); }
    Err(PgError.UnsupportedType) => { if !conn.is_closed() { return Ok(98); } return Ok(5); }
    Err(PgError.LimitExceeded) => { if !conn.is_closed() { return Ok(98); } return Ok(6); }
    Err(e) => { return Ok(79); }
  }
}
"#;

fn fixture() -> String {
    let src = include_str!("../src/stdlib/pg.sigil")
        .replace("use std.pg_scram.first;\n", "")
        .replace("use std.pg_scram.respond;\n", "")
        .replace("use std.pg_scram.verify;\n", "");
    let src = {
        let a = src.find("fn scram_error(").unwrap();
        let b = src[a..].find("// 1バイトずつ").unwrap() + a;
        format!("{}{}", &src[..a], &src[b..])
    };
    let a = src.find("fn connect_inner(").unwrap();
    let b = src[a..].find("// 結果の文字列").unwrap() + a;
    format!(
        "{}{}{}",
        &src[..a],
        r#"fn connect_inner(c: &PgConfig) -> Result<PgConn, PgError>
  intent "試験だけで認証済みの外部相手へ接続する"
  requires none
  ensures none
  effects net.client, random, task.suspend
{
  let entropy = random.bytes(1);
  match net.connect(&c.host, c.port) { Ok(s) => { return Ok(PgConn { stream: Some(s), ready_status: 73 }); } Err(e) => { return Err(PgError.Connection); } }
}
"#,
        &src[b..]
    )
}
fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![tag];
    b.extend(((body.len() + 4) as u32).to_be_bytes());
    b.extend(body);
    b
}
fn recv(s: &mut TcpStream, tag: u8) -> Vec<u8> {
    let mut h = [0; 5];
    s.read_exact(&mut h).unwrap();
    assert_eq!(h[0], tag);
    let n = u32::from_be_bytes(h[1..].try_into().unwrap()) as usize;
    assert!((4..=1048576).contains(&n));
    let mut b = vec![0; n - 4];
    s.read_exact(&mut b).unwrap();
    b
}
fn request(s: &mut TcpStream, rollback: bool) {
    let parse = recv(s, b'P');
    let bind = recv(s, b'B');
    assert_eq!(recv(s, b'D'), b"P\0");
    assert_eq!(recv(s, b'E'), [0; 5]);
    assert!(recv(s, b'S').is_empty());
    if rollback {
        assert_eq!(parse, b"\0ROLLBACK\0\0\0");
        return;
    }
    let mut expected = b"\0select $1,$2,$3,$4,$5,$6,$7,$8,$9\0\0\x09".to_vec();
    for oid in [16u32, 21, 23, 20, 25, 1043, 17, 25, 17] {
        expected.extend(oid.to_be_bytes());
    }
    assert_eq!(parse, expected); // 値・引用符・複文風の文字列はSQLへ展開されない。
    let mut expected = vec![0, 0, 0, 0, 0, 9]; // unnamed portal/statement、全てtext
    for value in [
        Some(&b"t"[..]),
        Some(&b"-32768"[..]),
        Some(&b"2147483647"[..]),
        Some(&b"-9223372036854775808"[..]),
        Some("日本語 ' ; select".as_bytes()),
        Some(&b""[..]),
        Some(&b"\\x0078"[..]),
        None,
        Some(&b"\\x"[..]),
    ] {
        if let Some(v) = value {
            expected.extend((v.len() as i32).to_be_bytes());
            expected.extend(v);
        } else {
            expected.extend((-1i32).to_be_bytes());
        }
    }
    expected.extend([0, 0]);
    assert_eq!(bind, expected);
}
fn description(oid: u32, binary: bool) -> Vec<u8> {
    let mut b = 9u16.to_be_bytes().to_vec();
    for (i, col) in [16u32, 21, 23, 20, 25, 1043, 17, 25, 17].iter().enumerate() {
        b.extend(format!("c{i}\0").as_bytes());
        b.extend([0; 6]);
        b.extend(if i == 0 { oid } else { *col }.to_be_bytes());
        b.extend((-1i16).to_be_bytes());
        b.extend((-1i32).to_be_bytes());
        b.extend(if binary { 1u16 } else { 0 }.to_be_bytes());
    }
    packet(b'T', &b)
}
fn row() -> Vec<u8> {
    let mut b = 9u16.to_be_bytes().to_vec();
    for v in [
        Some(&b"t"[..]),
        Some(&b"-32768"[..]),
        Some(&b"2147483647"[..]),
        Some(&b"-9223372036854775808"[..]),
        Some("日本語 ' ; select".as_bytes()),
        Some(&b""[..]),
        Some(&b"\\x0078"[..]),
        None,
        Some(&b"\\x"[..]),
    ] {
        if let Some(v) = v {
            b.extend((v.len() as i32).to_be_bytes());
            b.extend(v);
        } else {
            b.extend((-1i32).to_be_bytes());
        }
    }
    packet(b'D', &b)
}
fn closed(s: &mut TcpStream) {
    let mut b = [0];
    match s.read(&mut b) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        r => panic!("破棄を外部相手が観測できない: {r:?}"),
    }
}
fn exchange(s: &mut TcpStream, case: i64) {
    s.set_read_timeout(Some(Duration::from_secs(15))).unwrap();
    if case == 16 {
        for tag in b"PBDES" {
            recv(s, *tag);
        }
        closed(s);
        return;
    }
    request(s, false);
    if case == 2 {
        return;
    }
    if case == 3 {
        closed(s);
        return;
    }
    if case == 17 {
        s.write_all(&[b'1', 0, 0]).unwrap();
        closed(s);
        return;
    }
    let mut out = Vec::new();
    if case == 1 || case == 15 {
        out.extend(packet(
            b'E',
            b"SERROR\0C42601\0Msecret SQL and password\0\0",
        ));
        if case == 15 {
            s.write_all(&out).unwrap();
            closed(s);
            return;
        }
        out.extend(packet(b'N', b"SNOTICE\0C00000\0Mprivate detail\0\0"));
        out.extend(packet(b'Z', b"E"));
        s.write_all(&out).unwrap();
        request(s, true);
        out = packet(b'1', b"");
        out.extend(packet(b'2', b""));
        out.extend(packet(b'n', b""));
        out.extend(packet(b'C', b"ROLLBACK\0"));
        out.extend(packet(b'Z', b"I"));
        s.write_all(&out).unwrap();
        closed(s);
        return;
    }
    if case == 4 {
        out.extend(packet(b'2', b""));
    } else {
        out.extend(packet(b'1', b""));
        out.extend(packet(b'2', b""));
        if case == 13 || case == 14 {
            out.extend(packet(b'n', b""));
            out.extend(if case == 14 {
                packet(b'I', b"")
            } else {
                packet(b'C', b"BEGIN\0")
            });
            out.extend(packet(b'Z', if case == 13 { b"T" } else { b"I" }));
        } else {
            out.extend(description(if case == 5 { 999 } else { 16 }, case == 6));
            let mut data = row();
            if case == 7 {
                data[7..11].copy_from_slice(&(-2i32).to_be_bytes());
            }
            if case == 10 {
                data[5..7].copy_from_slice(&8u16.to_be_bytes());
            }
            if case == 11 {
                data[11] = 255;
            }
            out.extend(data);
            out.extend(packet(b'C', b"SELECT 1\0"));
            out.extend(packet(b'Z', b"I"));
        }
    }
    if case == 12 {
        out = packet(b'Z', b"I");
    }
    // 分割と結合の両方を同じ列で検査する。
    for chunk in out.chunks(7) {
        if s.write_all(chunk).is_err() {
            break;
        }
    }
    closed(s);
}

#[test]
fn query_results_errors_sync_deadlines_and_closed_connections_match() {
    let extra = vec![("std.pg".into(), fixture())];
    let opts = Options {
        embedded_extra: extra.clone(),
        ..Options::captured()
    };
    let p = sigil::ir_eval::lower_sources_opts(
        &[SOURCE],
        "main",
        &sigil::effects::Options {
            embedded_extra: extra,
            ..Default::default()
        },
    )
    .unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("pg-query-492");
    let exe = dir.join("query");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [0, 1, 4] {
        for (case, expected) in [
            (0, 0),
            (1, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 5),
            (6, 7),
            (7, 4),
            (8, 6),
            (9, 6),
            (10, 4),
            (11, 7),
            (12, 4),
            (13, 0),
            (14, 0),
            (15, 3),
            (16, 8),
            (17, 3),
            (18, 0),
        ] {
            let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = l.local_addr().unwrap().port();
            let server = std::thread::spawn(move || {
                let (mut s, _) = l.accept().unwrap();
                exchange(&mut s, case);
            });
            let actual = if mode == 0 {
                sigil::ir_eval::run_with(
                    SOURCE,
                    "main",
                    &[Arg::Int(port.into()), Arg::Int(case)],
                    &opts,
                )
                .unwrap()
                .value
            } else {
                let out = Command::new(&exe)
                    .env("SIGIL_WORKERS", mode.to_string())
                    .env("SIGIL_RT_LIVE", "1")
                    .arg(port.to_string())
                    .arg(case.to_string())
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "mode={mode} case={case}: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
                String::from_utf8(out.stdout).unwrap().trim().to_owned()
            };
            assert_eq!(actual, expected.to_string(), "mode={mode} case={case}");
            server.join().unwrap();
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

// Startup/SCRAM と query の製品コードを差し替えずに結線する。
// 評価器のPBKDF2接続期限問題は既存 Issue #484 の範囲で、この試験はnativeで実施する。
fn startup(s: &mut TcpStream) {
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut len = [0; 4];
    s.read_exact(&mut len).unwrap();
    let mut body = vec![0; u32::from_be_bytes(len) as usize - 4];
    s.read_exact(&mut body).unwrap();
    assert!(body
        .windows(b"bytea_output=hex".len())
        .any(|b| b == b"bytea_output=hex"));
    let auth = |kind: u32, body: &[u8]| {
        let mut b = kind.to_be_bytes().to_vec();
        b.extend(body);
        packet(b'R', &b)
    };
    s.write_all(&auth(10, b"SCRAM-SHA-256\0\0")).unwrap();
    let first = recv(s, b'p');
    let first = std::str::from_utf8(&first[b"SCRAM-SHA-256\0".len() + 4..]).unwrap();
    let nonce = first.strip_prefix("n,,n=,r=").unwrap();
    let challenge = format!("r={nonce}server,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096");
    s.write_all(&auth(11, challenge.as_bytes())).unwrap();
    let final_msg = recv(s, b'p');
    let final_msg = std::str::from_utf8(&final_msg).unwrap();
    let (without, proof) = final_msg.rsplit_once(",p=").unwrap();
    let transcript = format!("{},{challenge},{without}", &first[3..]);
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
    assert_eq!(proof, lines.next().unwrap());
    let mut out = auth(12, format!("v={}", lines.next().unwrap()).as_bytes());
    out.extend(auth(0, b""));
    out.extend(packet(b'S', b"client_encoding\0UTF8\0"));
    out.extend(packet(b'K', &[0; 8]));
    out.extend(packet(b'Z', b"I"));
    s.write_all(&out).unwrap();
}

#[test]
fn scram_startup_and_query_are_connected_in_production_module() {
    let p = sigil::ir_eval::lower_sources(&[SOURCE], "main").unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("pg-startup-query-492");
    let exe = dir.join("query");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [1, 4] {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            startup(&mut s);
            exchange(&mut s, 0);
        });
        let out = Command::new(&exe)
            .env("SIGIL_WORKERS", mode.to_string())
            .env("SIGIL_RT_LIVE", "1")
            .arg(port.to_string())
            .arg("0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0");
        assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
        server.join().unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

const TAKE_SOCKET: &str = r#"module app.main
  targets server
  trust none
fn discard(empty: Option<TcpClient>)
  intent "元のNoneだけを破棄する"
  requires none
  ensures none
  effects none
{ return; }
fn waiting(pending: &mut Option<TcpClient>) -> Int
  intent "取り出した接続を中断点で所有する"
  requires none
  ensures none
  effects net.client, task.suspend
{
  match pending.take() {
    Some(s) => { return receive_owned(s); }
    None => { return 90; }
  }
}
fn receive_owned(socket: TcpClient) -> Int
  intent "読取り中の取消で所有値を破棄する"
  requires none
  ensures none
  effects net.client, task.suspend
{
  let mut active = socket;
  match active.read(1) { Ok(b) => { return 91; } Err(e) => { return 92; } }
}
pub fn main(port: Int) -> Int
  intent "取消後のfd番号再利用を挟んでも元のNoneが閉じない"
  requires none
  ensures none
  effects net.client, task.suspend, clock, task.spawn
{
  match net.connect(&"127.0.0.1", port) {
    Ok(s) => {
      let mut original = Some(s);
      match task.timeout(150, || waiting(&mut original)) { None => {} Some(n) => { return n; } }
      if !original.is_none() { return 93; }
      match net.connect(&"127.0.0.1", port) {
        Ok(next) => {
          let mut active = next;
          discard(original);
          match active.write_all(&"ping".as_bytes()) { Ok(v) => {} Err(e) => { return 94; } }
          match active.read(4) { Ok(b) => { if b == "pong".as_bytes() { return 0; } } Err(e) => {} }
          return 95;
        }
        Err(e) => { return 96; }
      }
    }
    Err(e) => { return 97; }
  }
}
"#;

#[test]
fn taken_socket_cancel_then_original_drop_does_not_close_reused_fd() {
    let p = sigil::ir_eval::lower_sources(&[TAKE_SOCKET], "main").unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("take-socket-492");
    let exe = dir.join("take");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [0, 1, 4] {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            closed(&mut s);
            let (mut next, _) = l.accept().unwrap();
            next.set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut b = [0; 4];
            next.read_exact(&mut b).unwrap();
            assert_eq!(&b, b"ping");
            next.write_all(b"pong").unwrap();
            closed(&mut next);
        });
        let value = if mode == 0 {
            sigil::ir_eval::run(TAKE_SOCKET, "main", &[Arg::Int(port.into())])
                .unwrap()
                .value
        } else {
            let out = Command::new(&exe)
                .env("SIGIL_WORKERS", mode.to_string())
                .env("SIGIL_RT_LIVE", "1")
                .arg(port.to_string())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        assert_eq!(value, "0", "mode={mode}");
        server.join().unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn taken_socket_partial_write_cancel_closes_before_next_connection() {
    let source = TAKE_SOCKET
        .replace("pending: &mut Option<TcpClient>)", "pending: &mut Option<TcpClient>, data: &Bytes)")
        .replace("return receive_owned(s);", "return send_owned(s, &data);")
        .replace("let mut original = Some(s);", "let mut data = \"x\".as_bytes(); let mut i = 0; while i < 24 { let copy = data.clone(); data.extend(&copy); i = i + 1; } let mut original = Some(s);")
        .replace("waiting(&mut original)", "waiting(&mut original, &data)")
        .replace("task.timeout(150,", "task.timeout(500,") + r#"
fn send_owned(socket: TcpClient, data: &Bytes) -> Int
  intent "部分送信で中断した所有値を取消時に破棄する"
  requires none
  ensures none
  effects net.client, task.suspend
{
  let mut active = socket;
  match active.write_all(&data) { Ok(v) => { return 91; } Err(e) => { return 92; } }
}
"#;
    let p = sigil::ir_eval::lower_sources(&[&source], "main").unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("take-write-492");
    let exe = dir.join("take");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [0, 1, 4] {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut first = [0];
            s.read_exact(&mut first).unwrap();
            assert_eq!(first[0], b'x');
            // 次接続はtimeout後にしか開始しない。そこまで最初のsocketを読まず、
            // write_allが必ず有限の送信バッファで中断するようにする（sleepに依存しない）。
            let (mut next, _) = l.accept().unwrap();
            let mut rest = Vec::new();
            s.read_to_end(&mut rest).unwrap();
            assert!(rest.len() + 1 < 16777216, "期限前に全量送信してしまった");
            next.set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut b = [0; 4];
            next.read_exact(&mut b).unwrap();
            assert_eq!(&b, b"ping");
            next.write_all(b"pong").unwrap();
            closed(&mut next);
        });
        let value = if mode == 0 {
            sigil::ir_eval::run(&source, "main", &[Arg::Int(port.into())])
                .unwrap()
                .value
        } else {
            let out = Command::new(&exe)
                .env("SIGIL_WORKERS", mode.to_string())
                .env("SIGIL_RT_LIVE", "1")
                .arg(port.to_string())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        assert_eq!(value, "0", "mode={mode}");
        server.join().unwrap();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

//! ADR 0034 6-5e: 外部の固定応答列で評価器と native 1・4 workers を照合する。
//! 認証済みソケットの差し込み以外は製品の pool・lease・query・清掃をそのまま使う。
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
fn discard(lease: PgLease)
  intent "明示返却せず所有する接続を破棄する"
  requires none
  ensures none
  effects none
{ return; }
pub fn main(port: Int, scenario: Int) -> Int
  intent "同じ応答列でプールの再利用と破棄を検査する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  match exercise(port, scenario) { Ok(n) => { return n; } Err(e) => { return 99; } }
}
fn exercise(port: Int, scenario: Int) -> Result<Int, PgError>
  intent "最大一接続で清掃と取消の後に枠を再利用する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let limits = pg.default_pool_limits().with_capacity(1, 1)?.with_timeouts(500, 500, 200, 200)?;
  let pool = pg.pool(cfg, &limits)?;
  let mut lease = pool.acquire()?;
  if scenario == 8 || scenario == 9 {
    if scenario == 8 {
      match pool.acquire() { Err(PgError.Timeout) => {} _ => { return Ok(81); } }
    } else {
      match task.timeout(80, || pool.acquire()) { None => {} Some(r) => { return Ok(82); } }
    }
    discard(lease);
    let next = pool.acquire()?;
    discard(next);
    return Ok(0);
  }
  if scenario == 10 { discard(lease); let next = pool.acquire()?; discard(next); return Ok(0); }
  let mut query_limits = pg.default_query_limits();
  if scenario == 5 { query_limits = query_limits.with_timeout(150)?; }
  if scenario == 6 {
    match task.timeout(100, || lease.query(&"SELECT 1", &List.new(), &query_limits)) {
      None => {} Some(r) => { return Ok(83); }
    }
    discard(lease);
  } else {
    match lease.query(&"SELECT 1", &List.new(), &query_limits) {
      Ok(query_result) => { if query_result.status() != 73 && scenario != 1 { return Ok(84); } }
      Err(PgError.SqlState(code)) => { if scenario != 2 || code != "25P02" { return Ok(85); } }
      Err(PgError.Timeout) => { if scenario != 5 { return Ok(86); } }
      Err(e) => { return Ok(87); }
    }
    if scenario == 7 {
      match task.timeout(80, move || lease.release()) { None => {} Some(r) => { return Ok(88); } }
    } else {
      match lease.release() {
        Ok(reused) => { if reused != (scenario == 0) { return Ok(89); } }
        Err(PgError.Timeout) => { if scenario != 4 && scenario != 14 { return Ok(90); } }
        Err(PgError.SqlState(code)) => { if scenario != 3 && scenario != 13 { return Ok(91); } }
        Err(e) => { if scenario != 3 && scenario != 11 && scenario != 12 { return Ok(92); } }
      }
    }
  }
  let mut next = pool.acquire()?;
  let answer = next.query(&"SELECT 2", &List.new(), &pg.default_query_limits())?;
  if answer.command() != "SELECT 2" || answer.status() != 73 { return Ok(93); }
  discard(next);
  return Ok(0);
}
"#;

fn fixture() -> String {
    let src = include_str!("../src/stdlib/pg.sigil")
        .replace("use std.pg_scram.first;\n", "")
        .replace("use std.pg_scram.respond;\n", "")
        .replace("use std.pg_scram.verify;\n", "");
    let a = src.find("fn scram_error(").unwrap();
    let b = src[a..].find("// 1バイトずつ").unwrap() + a;
    let src = format!("{}{}", &src[..a], &src[b..]);
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
  match net.connect(&c.host, c.port) {
    Ok(s) => { return Ok(PgConn { stream: Some(s), ready_status: 73 }); }
    Err(e) => { return Err(PgError.Connection); }
  }
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
fn request(s: &mut TcpStream, sql: &str) {
    let mut expected = vec![0];
    expected.extend(sql.as_bytes());
    expected.extend([0, 0, 0]);
    assert_eq!(recv(s, b'P'), expected);
    assert_eq!(recv(s, b'B'), [0; 8]);
    assert_eq!(recv(s, b'D'), b"P\0");
    assert_eq!(recv(s, b'E'), [0; 5]);
    assert!(recv(s, b'S').is_empty());
}
fn answer(s: &mut TcpStream, command: &str, status: u8) {
    let mut out = packet(b'1', b"");
    out.extend(packet(b'2', b""));
    out.extend(packet(b'n', b""));
    let mut tag = command.as_bytes().to_vec();
    tag.push(0);
    out.extend(packet(b'C', &tag));
    out.extend(packet(b'Z', &[status]));
    for chunk in out.chunks(7) {
        s.write_all(chunk).unwrap();
    }
}
fn sql_error(s: &mut TcpStream, state: &[u8], status: u8) {
    let mut body = b"SERROR\0C".to_vec();
    body.extend(state);
    body.extend(b"\0Mprivate session detail\0\0");
    let mut out = packet(b'E', &body);
    out.extend(packet(b'Z', &[status]));
    s.write_all(&out).unwrap();
}
fn closed(s: &mut TcpStream) {
    let mut b = [0];
    match s.read(&mut b) {
        Ok(0) => {}
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
        r => panic!("接続を捨てる経路が通信/返却した: {r:?}"),
    }
}
fn accept(l: &TcpListener) -> TcpStream {
    let (s, _) = l.accept().unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s
}
fn exchange(l: TcpListener, case: i64) {
    let mut s = accept(&l);
    if [8, 9, 10].contains(&case) {
        closed(&mut s);
        closed(&mut accept(&l));
        return;
    }
    request(&mut s, "SELECT 1");
    if case == 5 || case == 6 {
        // 応答を返さず、期限/取消による閉鎖を外部で観測する。
        closed(&mut s);
    } else {
        if case == 2 {
            sql_error(&mut s, b"25P02", b'E');
        } else {
            answer(&mut s, "SELECT 1", if case == 1 { b'T' } else { b'I' });
        }
        if case == 1 || case == 2 {
            closed(&mut s);
        } else {
            request(&mut s, "DISCARD ALL");
            if case == 3 {
                sql_error(&mut s, b"55000", b'I');
                closed(&mut s);
            } else if case == 4 || case == 7 {
                closed(&mut s);
            } else if case == 11 || case == 12 {
                answer(
                    &mut s,
                    if case == 11 {
                        "SELECT 1"
                    } else {
                        "DISCARD ALL"
                    },
                    if case == 12 { b'T' } else { b'I' },
                );
                closed(&mut s);
            } else {
                answer(&mut s, "DISCARD ALL", b'I');
                request(&mut s, "SET bytea_output = 'hex'");
                if case == 13 {
                    sql_error(&mut s, b"55000", b'I');
                    closed(&mut s);
                } else if case == 14 {
                    closed(&mut s);
                } else {
                    answer(&mut s, "SET", b'I');
                }
            }
        }
    }
    let mut next = if case == 0 { s } else { accept(&l) };
    request(&mut next, "SELECT 2");
    answer(&mut next, "SELECT 2", b'I');
    closed(&mut next);
}
#[test]
fn pool_reuse_cleanup_status_deadlines_and_cancellation_match() {
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
    let dir = scratch("pg-pool-499");
    let exe = dir.join("pool");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [0, 1, 4] {
        for case in 0..=14 {
            let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = l.local_addr().unwrap().port();
            let server = std::thread::spawn(move || exchange(l, case));
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
            assert_eq!(actual, "0", "mode={mode} case={case}");
            server.join().unwrap();
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

const LIMITS: &str = r#"module app.main
  targets server
  trust none
pub fn main() -> Int
  intent "DB固有の容量と期限の上限を検査する"
  requires none
  ensures none
  effects none
{
  if !pg.default_pool_limits().with_capacity(16, 64).is_ok() { return 1; }
  if !pg.default_pool_limits().with_capacity(17, 64).is_err() { return 2; }
  if !pg.default_pool_limits().with_capacity(16, 65).is_err() { return 3; }
  if !pg.default_pool_limits().with_capacity(0, 64).is_err() { return 4; }
  if !pg.default_pool_limits().with_capacity(16, 0).is_err() { return 5; }
  if !pg.default_pool_limits().with_timeouts(10000, 30000, 5000, 30000).is_ok() { return 6; }
  if !pg.default_pool_limits().with_timeouts(10001, 30000, 5000, 30000).is_err() { return 7; }
  if !pg.default_pool_limits().with_timeouts(10000, 30001, 5000, 30000).is_err() { return 8; }
  if !pg.default_pool_limits().with_timeouts(10000, 30000, 5001, 30000).is_err() { return 9; }
  if !pg.default_pool_limits().with_timeouts(10000, 30000, 5000, 30001).is_err() { return 10; }
  if !pg.default_pool_limits().with_timeouts(0, 1, 1, 1).is_err() { return 11; }
  return 0;
}
"#;

#[test]
fn pool_capacity_and_deadline_configuration_boundaries() {
    let p = sigil::ir_eval::lower_sources(&[LIMITS], "main").unwrap();
    sigil::verify::verify(&p).unwrap();
    assert_eq!(sigil::ir_eval::run(LIMITS, "main", &[]).unwrap().value, "0");
    let dir = scratch("pg-pool-limits-499");
    let exe = dir.join("limits");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for workers in [1, 4] {
        let out = Command::new(&exe)
            .env("SIGIL_WORKERS", workers.to_string())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn saturation_source() -> String {
    let mut s = String::from(
        r#"module app.main
  targets server
  trust none
fn discard_all(leases: List<PgLease>)
  intent "全ての貸出接続を通信せず閉じる"
  requires none
  ensures none
  effects none
{ return; }
pub fn main(port: Int, scenario: Int) -> Int
  intent "接続16と借出待機64の実容量を検査する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  match exercise(port, scenario) { Ok(n) => { return n; } Err(e) => { return 99; } }
}
fn exercise(port: Int, scenario: Int) -> Result<Int, PgError>
  intent "満杯の接続と待機を取消後に回収する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let limits = pg.default_pool_limits().with_timeouts(1000, 1000, 1000, 3000)?;
  let pool = pg.pool(cfg, &limits)?;
  let mut leases = List.new();
  let mut i = 0;
  while i < 16 { leases.push(pool.acquire()?); i = i + 1; }
  let total = task.scope(|scope| {
"#,
    );
    for i in 0..65 {
        s.push_str(&format!("    let p{i} = pool.clone();\n    let h{i} = scope.spawn(move || {{ match p{i}.acquire() {{ Err(PgError.LimitExceeded) => {{ return 1; }} Err(PgError.Timeout) => {{ return 2; }} Ok(v) => {{ return 100; }} Err(e) => {{ return 1000; }} }} }});\n"));
    }
    s.push_str("    let mut sum = 0;\n");
    for i in 0..65 {
        s.push_str(&format!("    sum = sum + h{i}.join();\n"));
    }
    s.push_str(
        r#"    return sum;
  });
  if total != 129 { return Ok(total); }
  discard_all(leases);
  let next = pool.acquire()?;
  return Ok(0);
}
"#,
    );
    s
}

#[test]
fn sixteen_connections_and_sixty_four_waiters_are_bounded_and_recovered() {
    let source = saturation_source();
    let extra = vec![("std.pg".into(), fixture())];
    let opts = Options {
        embedded_extra: extra.clone(),
        ..Options::captured()
    };
    let p = sigil::ir_eval::lower_sources_opts(
        &[&source],
        "main",
        &sigil::effects::Options {
            embedded_extra: extra,
            ..Default::default()
        },
    )
    .unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("pg-pool-saturation-499");
    let exe = dir.join("saturation");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [0, 1, 4] {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let mut sockets: Vec<_> = (0..16).map(|_| accept(&l)).collect();
            for s in &mut sockets {
                closed(s);
            }
            // 旧fdの閉鎖を全て観測してから、回収した枠への次の接続を確認する。
            closed(&mut accept(&l));
        });
        let value = if mode == 0 {
            sigil::ir_eval::run_with(
                &source,
                "main",
                &[Arg::Int(port.into()), Arg::Int(0)],
                &opts,
            )
            .unwrap()
            .value
        } else {
            let out = Command::new(&exe)
                .env("SIGIL_WORKERS", mode.to_string())
                .env("SIGIL_RT_LIVE", "1")
                .arg(port.to_string())
                .arg("0")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "mode={mode}: {}",
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
fn lease_does_not_expose_or_duplicate_its_connection() {
    for operation in [
        "let copy = lease.clone();",
        "let socket = lease.stream;",
        "let internal = lease.held;",
        "let duplicate = lease; discard(lease); discard(duplicate);",
    ] {
        let source = SOURCE.replace(
            "  let mut lease = pool.acquire()?;",
            &format!("  let mut lease = pool.acquire()?; {operation}"),
        );
        assert!(
            sigil::ir_eval::lower_sources(&[&source], "main").is_err(),
            "拒否されなかった: {operation}"
        );
    }
}

const AUTH_ABORT: &str = r#"module app.main
  targets server
  trust none
pub fn main(port: Int, scenario: Int) -> Int
  intent "認証中断後に同じ容量で新しい接続を作る"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  match exercise(port, scenario) { Ok(n) => { return n; } Err(e) => { return 99; } }
}
fn exercise(port: Int, scenario: Int) -> Result<Int, PgError>
  intent "接続中の期限切れと取消の枠を回収する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let limits = pg.default_pool_limits().with_capacity(1, 1)?.with_timeouts(150, 500, 200, 1000)?;
  let pool = pg.pool(cfg, &limits)?;
  let mut i = 0;
  while i < 2 {
    if scenario >= 2 {
      match task.timeout(80, || pool.acquire()) { None => {} Some(v) => { return Ok(1); } }
    } else {
      match pool.acquire() { Err(PgError.Timeout) => {} _ => { return Ok(2); } }
    }
    i = i + 1;
  }
  return Ok(0);
}
"#;

fn startup_until_stall(s: &mut TcpStream, stage: i64) {
    let mut len = [0; 4];
    s.read_exact(&mut len).unwrap();
    let n = u32::from_be_bytes(len) as usize;
    assert!((8..=1048576).contains(&n));
    let mut body = vec![0; n - 4];
    s.read_exact(&mut body).unwrap();
    assert_eq!(&body[..4], &196608u32.to_be_bytes());
    if stage == 1 {
        let mut auth = 10u32.to_be_bytes().to_vec();
        auth.extend(b"SCRAM-SHA-256\0\0");
        s.write_all(&packet(b'R', &auth)).unwrap();
        let first = recv(s, b'p');
        assert!(first.starts_with(b"SCRAM-SHA-256\0"));
    }
    closed(s);
}

#[test]
fn startup_and_authentication_deadline_and_cancellation_recover_pool_slots() {
    compare(
        AUTH_ABORT,
        vec![],
        0..4,
        "pg-pool-auth-abort-499",
        |l, case| {
            for _ in 0..2 {
                startup_until_stall(&mut accept(&l), case % 2);
            }
        },
    );
}

fn compare(
    source: &str,
    extra: Vec<(String, String)>,
    cases: std::ops::Range<i64>,
    name: &str,
    server: fn(TcpListener, i64),
) {
    let opts = Options {
        embedded_extra: extra.clone(),
        ..Options::captured()
    };
    let p = sigil::ir_eval::lower_sources_opts(
        &[source],
        "main",
        &sigil::effects::Options {
            embedded_extra: extra,
            ..Default::default()
        },
    )
    .unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch(name);
    let exe = dir.join("program");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    for mode in [0, 1, 4] {
        for case in cases.clone() {
            let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let port = l.local_addr().unwrap().port();
            let server = std::thread::spawn(move || server(l, case));
            let value = if mode == 0 {
                sigil::ir_eval::run_with(
                    source,
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
            assert_eq!(value, "0", "mode={mode} case={case}");
            server.join().unwrap();
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn partial_connect_send_deadline_and_cancellation_close_before_reusing_capacity() {
    // 送信バッファを確実に満たす試験専用の接続前処理。プール/permit/期限/破棄は製品本体。
    // 実際の Startup は小さく loopback だけでは部分送信を強制できないため、境界へ大きな入力を置く。
    let module = fixture().replace(
        "Ok(s) => { return Ok(PgConn { stream: Some(s), ready_status: 73 }); }",
        r#"Ok(s) => {
      let mut stream = s;
      match stream.read(1) {
        Ok(marker) => {
          if marker.get(0).unwrap_or(0) == 0 {
            let mut bytes = "x".as_bytes();
            let mut i = 0;
            while i < 24 { let copy = bytes.clone(); bytes.extend(&copy); i = i + 1; }
            match stream.write_all(&bytes) { Ok(v) => {} Err(e) => { return Err(PgError.Connection); } }
          }
        }
        Err(e) => { return Err(PgError.Connection); }
      }
      return Ok(PgConn { stream: Some(stream), ready_status: 73 });
    }"#,
    );
    assert!(module.contains("while i < 24"));
    let source = AUTH_ABORT
        .replace("while i < 2", "while i < 1")
        .replace("with_timeouts(150,", "with_timeouts(2000,")
        .replace("task.timeout(80,", "task.timeout(1200,")
        .replace(
            "  return Ok(0);",
            "  let next = pool.acquire()?; return Ok(0);",
        );
    compare(
        &source,
        vec![("std.pg".into(), module)],
        1..3,
        "pg-pool-partial-499",
        |l, _case| {
            let mut first = accept(&l);
            first.write_all(&[0]).unwrap();
            let mut byte = [0];
            first.read_exact(&mut byte).unwrap();
            assert_eq!(byte[0], b'x');
            // 最初の送信を読まずに次の接続を待ち、有限の送信バッファによる中断を保証する。
            let mut next = accept(&l);
            let mut rest = Vec::new();
            first.read_to_end(&mut rest).unwrap();
            assert!(
                rest.len() + 1 < 16777216,
                "全量が送信され中断点にならなかった"
            );
            next.write_all(&[1]).unwrap();
            closed(&mut next);
        },
    );
}

const ROOT_ABORT: &str = r#"module app.main
  targets server
  trust none
fn prepared(port: Int) -> Result<PgPool, PgError>
  intent "ルート内で生成したプールに清掃済みidle接続を置く"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let pool = pg.pool(cfg, &pg.default_pool_limits())?;
  let lease = pool.acquire()?;
  if !lease.release()? { return Err(PgError.Protocol); }
  return Ok(pool);
}
pub fn main(port: Int, scenario: Int) -> Int
  intent "所有ルートの異常停止後にidle接続を他ルートへ貸さない"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let (tx, rx) = task.channel();
  return task.roots(1, |g| {
    let slot = g.reserve();
    slot.start(port, move |p| {
      match prepared(p) {
        Ok(pool) => {
          tx.send(pool.clone());
          let zero = p - p;
          let failed = 1 / zero;
        }
        Err(e) => {}
      }
    }, |p| {});
    let shared = rx.recv();
    task.sleep(20);
    match shared.acquire() {
      Err(PgError.LockBroken) => { return 0; }
      _ => { return 1; }
    }
  });
}
"#;

#[test]
fn abnormal_pool_owner_root_closes_idle_and_propagates_lock_broken() {
    compare(
        ROOT_ABORT,
        vec![("std.pg".into(), fixture())],
        0..1,
        "pg-pool-root-abort-499",
        |l, _case| {
            let mut s = accept(&l);
            request(&mut s, "DISCARD ALL");
            answer(&mut s, "DISCARD ALL", b'I');
            request(&mut s, "SET bytea_output = 'hex'");
            answer(&mut s, "SET", b'I');
            // 清掃済みでも所有ルートの異常停止で閉じ、後続のqueryは一切来ない。
            closed(&mut s);
        },
    );
}

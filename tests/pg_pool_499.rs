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
      Ok(result) => { if result.status() != 73 && scenario != 1 { return Ok(84); } }
      Err(PgError.SqlState(code)) => { if scenario != 2 || code != "25P02" { return Ok(85); } }
      Err(PgError.Timeout) => { if scenario != 5 { return Ok(86); } }
      Err(e) => { return Ok(87); }
    }
    if scenario == 7 {
      match task.timeout(80, move || lease.release()) { None => {} Some(r) => { return Ok(88); } }
    } else {
      match lease.release() {
        Ok(reused) => { if reused != (scenario == 0) { return Ok(89); } }
        Err(PgError.Timeout) => { if scenario != 4 { return Ok(90); } }
        Err(PgError.SqlState(code)) => { if scenario != 3 { return Ok(91); } }
        Err(e) => { if scenario != 3 { return Ok(92); } }
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
            } else {
                answer(&mut s, "DISCARD ALL", b'I');
                request(&mut s, "SET bytea_output = 'hex'");
                answer(&mut s, "SET", b'I');
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
        for case in 0..=10 {
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

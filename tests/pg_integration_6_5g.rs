//! ADR 0034 6-5g: PostgreSQL クライアントの統合試験・サンプル動作確認。
//! 6-5a〜6-5f（接続・protocol v3・SCRAM 認証・単一文クエリ・接続プール・実サーバー照合）の成果を統合し、
//! 評価器とネイティブ（1 / 4 workers）の双方で動作を確認する。
mod common;

use common::native::scratch;
use sigil::exec::{Arg, Options};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::time::{Duration, Instant};

const EXAMPLE_PATH: &str = "examples/13-postgres-client/postgres_client.sigil";

fn example_src() -> String {
    std::fs::read_to_string(EXAMPLE_PATH).unwrap()
}

/// サンプルの構文解析・正規形・型検査が通ることを確認する。
#[test]
fn example_parses_checks_and_is_canonical() {
    let src = example_src();
    sigil::parse(&src).unwrap();
    let (root, file) = common::stage_src("ex-pg-check", &src);
    let out = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("check")
        .arg("--root")
        .arg(&root)
        .arg(&file)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(sigil::fmt::format(&src).unwrap(), src);
}

// ============================================================================
// 模擬サーバーヘルパー
// ============================================================================

fn packet(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn recv(s: &mut TcpStream, tag: u8) -> Vec<u8> {
    let mut h = [0; 5];
    s.read_exact(&mut h).unwrap();
    assert_eq!(h[0], tag, "期待したタグと異なる: got {}", h[0] as char);
    let n = u32::from_be_bytes(h[1..].try_into().unwrap()) as usize;
    assert!((4..=1048576).contains(&n));
    let mut b = vec![0; n - 4];
    s.read_exact(&mut b).unwrap();
    b
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
                    "接続タイムアウト"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("accept failed: {e}"),
        }
    }
}

fn drain_request(s: &mut TcpStream) -> String {
    let p = recv(s, b'P');
    assert_eq!(p[0], 0); // unnamed statement
    let sql_end = p[1..].iter().position(|&b| b == 0).unwrap() + 1;
    let sql = String::from_utf8(p[1..sql_end].to_vec()).unwrap();
    let _b = recv(s, b'B');
    assert_eq!(recv(s, b'D'), b"P\0");
    assert_eq!(recv(s, b'E'), [0; 5]);
    assert!(recv(s, b'S').is_empty());
    sql
}

fn send_answer_nodata(s: &mut TcpStream, command: &str, status: u8) {
    let mut out = packet(b'1', b"");
    out.extend(packet(b'2', b""));
    out.extend(packet(b'n', b""));
    let mut tag = command.as_bytes().to_vec();
    tag.push(0);
    out.extend(packet(b'C', &tag));
    out.extend(packet(b'Z', &[status]));
    s.write_all(&out).unwrap();
}

fn send_sql_error(s: &mut TcpStream, state: &[u8], status: u8) {
    let mut body = b"SERROR\0C".to_vec();
    body.extend(state);
    body.extend(b"\0Merror detail\0\0");
    let mut out = packet(b'E', &body);
    out.extend(packet(b'Z', &[status]));
    s.write_all(&out).unwrap();
}

fn send_answer_rows(
    s: &mut TcpStream,
    cols: &[(&str, u32)],
    rows: &[Vec<Option<&[u8]>>],
    command: &str,
    status: u8,
) {
    let mut out = packet(b'1', b"");
    out.extend(packet(b'2', b""));

    // RowDescription
    let mut row_desc = (cols.len() as u16).to_be_bytes().to_vec();
    for (name, oid) in cols {
        row_desc.extend_from_slice(name.as_bytes());
        row_desc.push(0);
        row_desc.extend_from_slice(&0u32.to_be_bytes()); // table oid
        row_desc.extend_from_slice(&0u16.to_be_bytes()); // col idx
        row_desc.extend_from_slice(&oid.to_be_bytes()); // type oid
        row_desc.extend_from_slice(&4u16.to_be_bytes()); // type len
        row_desc.extend_from_slice(&(-1i32).to_be_bytes()); // type mod
        row_desc.extend_from_slice(&0u16.to_be_bytes()); // format text
    }
    out.extend(packet(b'T', &row_desc));

    // DataRows
    for row in rows {
        let mut data_row = (row.len() as u16).to_be_bytes().to_vec();
        for val in row {
            match val {
                Some(b) => {
                    data_row.extend_from_slice(&(b.len() as u32).to_be_bytes());
                    data_row.extend_from_slice(b);
                }
                None => {
                    data_row.extend_from_slice(&(-1i32).to_be_bytes());
                }
            }
        }
        out.extend(packet(b'D', &data_row));
    }

    let mut tag = command.as_bytes().to_vec();
    tag.push(0);
    out.extend(packet(b'C', &tag));
    out.extend(packet(b'Z', &[status]));
    s.write_all(&out).unwrap();
}

/// サンプルが評価器とネイティブ（1 worker, 4 workers）の双方で、
/// 模擬サーバーと正常に対話し終了コード 0 になることを確認する。
#[test]
fn example_runs_against_mock_server_eval_and_native() {
    let src = example_src();
    let extra = vec![("std.pg".into(), fixture())];
    let opts = Options {
        embedded_extra: extra.clone(),
        ..Options::captured()
    };
    let p = sigil::ir_eval::lower_sources_opts(
        &[&src],
        "main",
        &sigil::effects::Options {
            embedded_extra: extra,
            ..Default::default()
        },
    )
    .unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("pg-ex-run");
    let exe = dir.join("prog");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();

    for mode in [0, 1, 4] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            // 1. direct_query の接続
            let mut s1 = accept(&listener);
            let sql = drain_request(&mut s1);
            assert_eq!(sql, "SELECT $1 AS val");
            send_answer_rows(
                &mut s1,
                &[("val", 23)],
                &[vec![Some(b"42")]],
                "SELECT 1",
                b'I',
            );

            // 2. pooled_query の接続
            let mut s2 = accept(&listener);
            let sql = drain_request(&mut s2);
            assert_eq!(sql, "SELECT 1");
            send_answer_rows(
                &mut s2,
                &[("?column?", 23)],
                &[vec![Some(b"1")]],
                "SELECT 1",
                b'I',
            );
            // 返却時の清掃 DISCARD ALL と SET
            let clean_sql = drain_request(&mut s2);
            assert_eq!(clean_sql, "DISCARD ALL");
            send_answer_nodata(&mut s2, "DISCARD ALL", b'I');
            let set_sql = drain_request(&mut s2);
            assert_eq!(set_sql, "SET bytea_output = 'hex'");
            send_answer_nodata(&mut s2, "SET", b'I');
        });

        let value = if mode == 0 {
            sigil::ir_eval::run_with(&src, "main", &[Arg::Int(port.into())], &opts)
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
                "mode={mode}: stderr={}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        server.join().unwrap();
        assert_eq!(value, "0", "mode {mode} failed with value {value}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ============================================================================
// 評価器・ネイティブ統合突き合わせ試験（6-5a〜6-5f の網羅検証）
// ============================================================================

const INTEGRATION_SRC: &str = r#"module app.main
  targets server
  trust none

pub fn main(port: Int, scenario: Int) -> Int
  intent "統合クライアントの全機能を検査する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  match exercise(port, scenario) {
    Ok(n) => { return n; }
    Err(e) => { return 99; }
  }
}

fn exercise(port: Int, scenario: Int) -> Result<Int, PgError>
  intent "接続・複数型クエリ・文エラー同期復旧・プール借出と清掃を実行する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let cfg = pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))?;
  let limits = pg.default_query_limits();

  // 単一接続の検証: パラメータ付きクエリと複数型の検査
  let mut conn = pg.connect(cfg.clone(), &pg.default_connect_limits())?;
  let mut params = List.new();
  params.push(pg.param(16, Some(PgValue.Boolean(true)))?);
  params.push(pg.param(23, Some(PgValue.Integer(42)))?);
  params.push(pg.param(25, Some(PgValue.Text("sigil-test")))?);
  params.push(pg.param(25, None)?);

  let res = conn.query(&"SELECT $1 AS b, $2 AS i, $3 AS t, $4 AS n", &params, &limits)?;
  if res.columns() != 4 || res.rows() != 1 || res.command() != "SELECT 1" || res.status() != 73 {
    return Ok(10);
  }

  // 文エラー後の同一接続での復旧・再クエリ
  match conn.query(&"SYNTAX ERROR", &List.new(), &limits) {
    Err(PgError.SqlState(code)) => {
      if code != "42601" || conn.is_closed() { return Ok(20); }
    }
    _ => { return Ok(21); }
  }
  let again = conn.query(&"SELECT 100", &List.new(), &limits)?;
  if again.status() != 73 || again.command() != "SELECT 1" { return Ok(22); }

  // プール借出・クエリ・正常返却（DISCARD ALL清掃）の検証
  let pool = pg.pool(cfg, &pg.default_pool_limits())?;
  let mut lease = pool.acquire()?;
  let p_res = lease.query(&"SELECT 200", &List.new(), &limits)?;
  if p_res.status() != 73 { return Ok(30); }
  if !lease.release()? { return Ok(31); }

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
  let mut s = match net.connect(&c.host, c.port) { Ok(stream) => Ok(stream), Err(e) => Err(PgError.Connection), }?;
  return Ok(PgConn { stream: Some(s), ready_status: 73 });
}
"#,
        &src[b..]
    )
}

fn handle_integration_server(l: TcpListener, _case: i64) {
    // 1. conn の直接接続
    let mut s1 = accept(&l);

    // SELECT $1 ...
    let sql = drain_request(&mut s1);
    assert!(sql.starts_with("SELECT $1"));
    send_answer_rows(
        &mut s1,
        &[("b", 16), ("i", 23), ("t", 25), ("n", 25)],
        &[vec![Some(b"t"), Some(b"42"), Some(b"sigil-test"), None]],
        "SELECT 1",
        b'I',
    );

    // SYNTAX ERROR
    let sql = drain_request(&mut s1);
    assert_eq!(sql, "SYNTAX ERROR");
    send_sql_error(&mut s1, b"42601", b'I');

    // SELECT 100
    let sql = drain_request(&mut s1);
    assert_eq!(sql, "SELECT 100");
    send_answer_rows(
        &mut s1,
        &[("?column?", 23)],
        &[vec![Some(b"100")]],
        "SELECT 1",
        b'I',
    );

    // 2. pool の接続
    let mut s2 = accept(&l);
    // SELECT 200
    let sql = drain_request(&mut s2);
    assert_eq!(sql, "SELECT 200");
    send_answer_rows(
        &mut s2,
        &[("?column?", 23)],
        &[vec![Some(b"200")]],
        "SELECT 1",
        b'I',
    );

    // DISCARD ALL & SET bytea_output = 'hex'
    let sql = drain_request(&mut s2);
    assert_eq!(sql, "DISCARD ALL");
    send_answer_nodata(&mut s2, "DISCARD ALL", b'I');

    let sql = drain_request(&mut s2);
    assert_eq!(sql, "SET bytea_output = 'hex'");
    send_answer_nodata(&mut s2, "SET", b'I');
}

/// 評価器（mode 0）とネイティブ（mode 1, mode 4）の双方で、
/// 統合クエリ・型変換・エラー復旧・プール清掃が同一の結果（0）を返すことを確認する。
#[test]
fn integration_matches_both_eval_and_native() {
    let extra = vec![("std.pg".into(), fixture())];
    let opts = Options {
        embedded_extra: extra.clone(),
        ..Options::captured()
    };
    let p = sigil::ir_eval::lower_sources_opts(
        &[INTEGRATION_SRC],
        "main",
        &sigil::effects::Options {
            embedded_extra: extra,
            ..Default::default()
        },
    )
    .unwrap();
    sigil::verify::verify(&p).unwrap();

    let dir = scratch("pg-int-6-5g");
    let exe = dir.join("prog");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();

    for mode in [0, 1, 4] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();

        let server = std::thread::spawn(move || {
            handle_integration_server(listener, 0);
        });

        let value = if mode == 0 {
            sigil::ir_eval::run_with(
                INTEGRATION_SRC,
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
                "mode={mode}: stderr={}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        server.join().unwrap();
        assert_eq!(value, "0", "mode {mode} failed with value {value}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

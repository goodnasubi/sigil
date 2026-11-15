//! 6-5f: 製品の Startup/SCRAM・query・pool を実 PostgreSQL と照合する。
//! 固定応答による障害注入は pg_query_492 / pg_pool_499 に残す。
mod common;

use common::native::scratch;
use sigil::exec::Arg;
use std::process::Command;

const SOURCE: &str = r#"module app.main
  targets server
  trust none
pub fn main(port: Int, scenario: Int) -> Int
  intent "実PostgreSQLで製品クライアントを検査する"
  requires none
  ensures none
  effects env, net.client, random, clock, task.spawn, task.suspend, task.share
{
  let user = env.get(&"POSTGRES_USER").unwrap_or("");
  let database = env.get(&"POSTGRES_DB").unwrap_or("");
  let password = env.get(&"POSTGRES_PASSWORD").unwrap_or("");
  match exercise(port, scenario, user, database, password) {
    Ok(n) => { return n; }
    Err(PgError.Timeout) => { return 90; }
    Err(PgError.Protocol) => { return 91; }
    Err(PgError.SqlState(code)) => { return 92; }
    Err(e) => { return 99; }
  }
}
fn exercise(port: Int, scenario: Int, user: String, database: String, password: String) -> Result<Int, PgError>
  intent "本物の認証と値と文エラー後の再利用とプール清掃を確認する"
  requires none
  ensures none
  effects net.client, random, clock, task.spawn, task.suspend, task.share
{
  let cfg = pg.config("127.0.0.1", port, user, database, Secret.from_bytes(password.as_bytes()))?;
  let limits = pg.default_query_limits();
  if scenario == 0 {
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
    let answer = conn.query(&"SELECT $1 AS c0,$2 AS c1,$3 AS c2,$4 AS c3,$5 AS c4,$6 AS c5,$7 AS c6,$8 AS c7,$9 AS c8", &params, &limits)?;
    if answer.columns() != 9 || answer.rows() != 1 || answer.command() != "SELECT 1" || answer.status() != 73 { return Ok(1); }
    match answer.column(4) { Some(c) => { if c.type_oid() != 25 || c.column_name() != "c4" { return Ok(2); } } None => { return Ok(3); } }
    match answer.row(0) {
      Some(r) => {
        match r.get(0) { Some(Some(PgValue.Boolean(true))) => {} _ => { return Ok(4); } }
        match r.get(1) { Some(Some(PgValue.Integer(n))) => { if n != 0 - 32768 { return Ok(5); } } _ => { return Ok(6); } }
        match r.get(2) { Some(Some(PgValue.Integer(n))) => { if n != 2147483647 { return Ok(7); } } _ => { return Ok(8); } }
        match r.get(3) { Some(Some(PgValue.Integer(n))) => { if n != (0 - 9223372036854775807) - 1 { return Ok(9); } } _ => { return Ok(10); } }
        match r.get(4) { Some(Some(PgValue.Text(t))) => { if t != "日本語 ' ; select" { return Ok(11); } } _ => { return Ok(12); } }
        match r.get(5) { Some(Some(PgValue.Text(t))) => { if t != "" { return Ok(13); } } _ => { return Ok(14); } }
        match r.get(6) { Some(Some(PgValue.Binary(b))) => { if b.len() != 2 || b.get(0).unwrap_or(1) != 0 || b.get(1).unwrap_or(0) != 120 { return Ok(15); } } _ => { return Ok(16); } }
        match r.get(7) { Some(None) => {} _ => { return Ok(17); } }
        match r.get(8) { Some(Some(PgValue.Binary(b))) => { if b.len() != 0 { return Ok(18); } } _ => { return Ok(19); } }
      }
      None => { return Ok(20); }
    }
    match conn.query(&"SELEC invalid", &List.new(), &limits) {
      Err(PgError.SqlState(code)) => { if code != "42601" || conn.is_closed() { return Ok(21); } }
      _ => { return Ok(22); }
    }
    let again = conn.query(&"SELECT 42", &List.new(), &limits)?;
    if integer(&again) != 42 || again.status() != 73 { return Ok(23); }
    let begun = conn.query(&"BEGIN", &List.new(), &limits)?;
    if begun.status() != 84 { return Ok(24); }
    let committed = conn.query(&"COMMIT", &List.new(), &limits)?;
    if committed.status() != 73 || committed.command() != "COMMIT" { return Ok(25); }
    let begun_again = conn.query(&"BEGIN", &List.new(), &limits)?;
    let rolled_back = conn.query(&"ROLLBACK", &List.new(), &limits)?;
    if begun_again.status() != 84 || rolled_back.status() != 73 || rolled_back.command() != "ROLLBACK" { return Ok(26); }
    return Ok(0);
  }
  let pool_limits = pg.default_pool_limits().with_capacity(1, 1)?;
  let pool = pg.pool(cfg, &pool_limits)?;
  let mut lease = pool.acquire()?;
  let first = lease.query(&"SELECT pg_backend_pid()", &List.new(), &limits)?;
  let pid = integer(&first);
  if pid < 1 { return Ok(30); }
  let changed = lease.query(&"SET application_name = 'sigil_dirty'", &List.new(), &limits)?;
  let changed_bytea = lease.query(&"SET bytea_output = 'escape'", &List.new(), &limits)?;
  if !lease.release()? { return Ok(31); }
  let mut next = pool.acquire()?;
  let second = next.query(&"SELECT pg_backend_pid()", &List.new(), &limits)?;
  if integer(&second) != pid { return Ok(32); }
  let clean = next.query(&"SHOW application_name", &List.new(), &limits)?;
  match clean.row(0) { Some(r) => { match r.get(0) { Some(Some(PgValue.Text(t))) => { if t != "" { return Ok(33); } } _ => { return Ok(34); } } } None => { return Ok(35); } }
  let hex = next.query(&"SHOW bytea_output", &List.new(), &limits)?;
  match hex.row(0) { Some(r) => { match r.get(0) { Some(Some(PgValue.Text(t))) => { if t != "hex" { return Ok(36); } } _ => { return Ok(37); } } } None => { return Ok(38); } }
  let begun = next.query(&"BEGIN", &List.new(), &limits)?;
  if begun.status() != 84 || next.release()? { return Ok(39); }
  let mut replacement = pool.acquire()?;
  let third = replacement.query(&"SELECT pg_backend_pid()", &List.new(), &limits)?;
  if integer(&third) < 1 || integer(&third) == pid { return Ok(40); }
  if !replacement.release()? { return Ok(41); }
  return Ok(0);
}
fn integer(answer: &PgResult) -> Int
  intent "結果の整数を検査する"
  requires none
  ensures none
  effects none
{
  match answer.row(0) { Some(r) => { match r.get(0) { Some(Some(PgValue.Integer(n))) => { return n; } _ => { return 0 - 1; } } } None => { return 0 - 1; } }
}
"#;

#[test]
#[ignore = "実 PostgreSQL と POSTGRES_USER/DB/PASSWORD/PORT が必要。postgres CI で必須実行"]
fn production_query_and_pool_against_postgresql() {
    for name in ["POSTGRES_USER", "POSTGRES_DB", "POSTGRES_PASSWORD"] {
        assert!(!std::env::var(name)
            .expect("実サーバーの設定が必要")
            .is_empty());
    }
    let port: u16 = std::env::var("POSTGRES_PORT")
        .expect("POSTGRES_PORT が必要")
        .parse()
        .unwrap();
    let p = sigil::ir_eval::lower_sources(&[SOURCE], "main").unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("pg-live-510");
    let exe = dir.join("live");
    sigil::native::build_program(&p, &Default::default(), &exe).unwrap();
    // 評価器も製品の認証・期限のまま実行する。既知 #484 を成功扱いしない。
    for scenario in [0, 1] {
        let out = sigil::ir_eval::run(SOURCE, "main", &[Arg::Int(port.into()), Arg::Int(scenario)])
            .unwrap();
        assert_eq!(out.value, "0", "評価器 scenario={scenario}");
    }
    for workers in [1, 4] {
        for scenario in [0, 1] {
            let out = Command::new(&exe)
                .env("SIGIL_WORKERS", workers.to_string())
                .env("SIGIL_RT_LIVE", "1")
                .arg(port.to_string())
                .arg(scenario.to_string())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "workers={workers} scenario={scenario}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&out.stdout).trim(),
                "0",
                "workers={workers} scenario={scenario}"
            );
            assert!(String::from_utf8_lossy(&out.stderr).contains("live: sigil_blocks=0"));
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

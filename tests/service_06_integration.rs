//! ADR 0018 7-1: P1 完了サンプルのサービス統合試験（examples/06-service）。
//!
//! HTTP 要求受付、篩型による入力検証、PostgreSQL 読み書き、HMAC-SHA256 Bearer 認証、
//! 契約違反の隔離（500 応答と後続継続）を実サーバーで検証する。
mod common;

use common::native::scratch;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SERVICE_PATH: &str = "examples/06-service/main.sigil";

// テスト用の固定 HMAC 鍵・Audience・トークン（tests/data/bearer_507.tsv 由来）
const TEST_KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const TEST_AUD: &str = "sigil.api";
const TEST_TOKEN: &str = "sigil1.AAAAAGVT8QAAAAAAZVP_EAAJc2lnaWwuYXBpAA11c2VyOmFsaWNlLTAx.3riSYYpaTap_2allc3wt95bExsPxSXd68Yoa1ReIVoc";
const TEST_NOW: &str = "1700000000";

fn service_src() -> String {
    std::fs::read_to_string(SERVICE_PATH).expect("06-service/main.sigil が読めること")
}

/// サンプルの構文解析・正規形・型検査が通ることを確認する。
#[test]
fn service_06_parses_checks_and_is_canonical() {
    let src = service_src();
    sigil::parse(&src).expect("構文解析が通ること");
    assert_eq!(
        sigil::fmt::format(&src).expect("フォーマットが通ること"),
        src,
        "正規形であること"
    );

    let (root, file) = common::stage_src("service-06-check", &src);
    let out = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("check")
        .arg("--root")
        .arg(&root)
        .arg(&file)
        .output()
        .expect("sigil check の実行");
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        out.status.success(),
        "sigil check 失敗:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

struct ContainerGuard {
    name: String,
}

impl Drop for ContainerGuard {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", "-v", &self.name])
            .output();
    }
}

struct ChildGuard {
    child: Child,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn send_http(port: u16, raw_request: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("接続成功");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(raw_request.as_bytes())
        .expect("書き込み成功");

    let mut response = Vec::new();
    let mut buf = [0u8; 1024];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(_) => break,
        }
    }
    let res_str = String::from_utf8_lossy(&response).into_owned();
    let status_code = if res_str.starts_with("HTTP/1.1 ") || res_str.starts_with("HTTP/1.0 ") {
        res_str[9..12].parse::<u16>().unwrap_or(0)
    } else {
        0
    };
    (status_code, res_str)
}

/// ADR 0037「決定 3」: 認証失敗は 401、WWW-Authenticate: Bearer、Cache-Control: no-store、Content-Type: text/plain; charset=utf-8
fn assert_unauthorized(code: u16, res: &str, case: &str) {
    assert_eq!(code, 401, "{case} の status は 401: {res}");
    let lower = res.to_ascii_lowercase();
    assert!(
        lower.contains("www-authenticate: bearer"),
        "{case} に WWW-Authenticate: Bearer ヘッダーが含まれること: {res}"
    );
    assert!(
        lower.contains("cache-control: no-store"),
        "{case} に Cache-Control: no-store ヘッダーが含まれること: {res}"
    );
    assert!(
        lower.contains("content-type: text/plain; charset=utf-8"),
        "{case} に Content-Type: text/plain; charset=utf-8 ヘッダーが含まれること: {res}"
    );
    assert!(
        res.contains("unauthorized"),
        "{case} の body は unauthorized であること: {res}"
    );
}

/// コンテナ（または環境変数）で PostgreSQL を用意し、
/// サンプルのサービスを起動して HTTP 要求・入力検証・PostgreSQL 読み書き・認証・契約違反の隔離を検証する。
#[test]
fn service_06_live_integration() {
    let src = service_src();
    // 指摘1: 実サービスコードから固定時刻環境変数を外し、試験側に隔離して std.os.clock の壁時計を差し替える
    let clock = include_str!("../src/stdlib/os_clock.sigil")
        .replace("  return Ok(sec);", &format!("  return Ok({TEST_NOW});"));
    let opts = sigil::effects::Options {
        embedded_extra: vec![("std.os.clock".into(), clock)],
        ..Default::default()
    };
    let p = sigil::ir_eval::lower_sources_opts(&[&src], "main", &opts).expect("lowering 成功");

    let dir = scratch("service-06-live");
    let exe = dir.join("service");
    sigil::native::build_program(&p, &Default::default(), &exe).expect("ネイティブビルド成功");

    // PostgreSQL のポートと資格情報を特定
    let mut _container_guard: Option<ContainerGuard> = None;
    let (pg_port, pg_user, pg_db, pg_pass) = if let Ok(port_str) = std::env::var("POSTGRES_PORT") {
        let port: u16 = port_str.parse().expect("POSTGRES_PORT は数値");
        let user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "sigil_ci".into());
        let db = std::env::var("POSTGRES_DB").unwrap_or_else(|_| "sigil_ci".into());
        let pass = std::env::var("POSTGRES_PASSWORD").unwrap_or_default();
        (port, user, db, pass)
    } else {
        // docker コマンドが使えるか確認
        let docker_check = Command::new("docker").arg("--version").output();
        if docker_check.is_err() || !docker_check.unwrap().status.success() {
            eprintln!("Neither POSTGRES_PORT nor Docker is available; skipping live PostgreSQL service test");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let container_name = format!("sigil-test-pg-{}", std::process::id());
        let guard = ContainerGuard {
            name: container_name.clone(),
        };

        // 一時 PostgreSQL コンテナを起動
        let run_out = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                &container_name,
                "-e",
                "POSTGRES_USER=sigil_ci",
                "-e",
                "POSTGRES_DB=sigil_ci",
                "-e",
                "POSTGRES_PASSWORD=secret",
                "-e",
                "POSTGRES_HOST_AUTH_METHOD=scram-sha-256",
                "-e",
                "POSTGRES_INITDB_ARGS=--auth-host=scram-sha-256 --auth-local=scram-sha-256",
                "-p",
                "0:5432",
                "postgres:17.6-bookworm@sha256:f3bd19c606e442c3d7bdfa8002e03fe260a1023351e0ea4598032022b68dd6e3",
                "-c",
                "password_encryption=scram-sha-256",
            ])
            .output()
            .expect("docker run 実行");
        assert!(
            run_out.status.success(),
            "docker run 失敗: {}",
            String::from_utf8_lossy(&run_out.stderr)
        );

        // ポート番号を取得
        let port_out = Command::new("docker")
            .args(["port", &container_name, "5432"])
            .output()
            .expect("docker port 実行");
        let port_str = String::from_utf8_lossy(&port_out.stdout);
        // 出力形式例: "0.0.0.0:32768" or "[::]:32768"
        let host_port: u16 = port_str
            .trim()
            .rsplit(':')
            .next()
            .expect("ポート取得")
            .parse()
            .expect("ポート番号解析");

        // 起動完了を待機（最大60秒）
        let start = Instant::now();
        let mut ready = false;
        while start.elapsed() < Duration::from_secs(60) {
            let ready_out = Command::new("docker")
                .args([
                    "exec",
                    &container_name,
                    "pg_isready",
                    "-h",
                    "127.0.0.1",
                    "-U",
                    "sigil_ci",
                    "-d",
                    "sigil_ci",
                ])
                .output();
            if let Ok(out) = ready_out {
                if out.status.success() {
                    ready = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        assert!(ready, "PostgreSQL コンテナが時間内に準備完了しなかった");

        _container_guard = Some(guard);
        (
            host_port,
            "sigil_ci".into(),
            "sigil_ci".into(),
            "secret".into(),
        )
    };

    // サービスプロセスを起動（全22要求を確実に受け付けられるよう上限を設定）
    let mut child = Command::new(&exe)
        .env_clear()
        .env("PORT", "0")
        .env("MAX_CONNS", "50")
        .env("POSTGRES_HOST", "127.0.0.1")
        .env("POSTGRES_PORT", pg_port.to_string())
        .env("POSTGRES_USER", &pg_user)
        .env("POSTGRES_DB", &pg_db)
        .env("POSTGRES_PASSWORD", &pg_pass)
        .env("SIGIL_AUTH_HMAC_KEY_HEX", TEST_KEY)
        .env("SIGIL_AUTH_AUDIENCE", TEST_AUD)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("サービス起動");

    let stdout = child.stdout.take().expect("標準出力取得");
    let mut reader = BufReader::new(stdout);
    let mut port_line = String::new();
    reader
        .read_line(&mut port_line)
        .expect("ポート出力読み取り");
    let srv_port: u16 = port_line.trim().parse().expect("サービスポート番号解析");

    let _child_guard = ChildGuard { child };

    // 1. 公開エンドポイント: GET /health
    let (code, res) = send_http(srv_port, "GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(code, 200, "GET /health status: {res}");
    assert!(res.contains("ok"), "GET /health body: {res}");

    // 2. 未認証アクセス: GET /items?id=1
    let (code, res) = send_http(
        srv_port,
        "GET /items?id=1 HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    assert_unauthorized(code, &res, "未認証 GET /items");

    // 3. 不正トークン: GET /items?id=1
    let (code, res) = send_http(
        srv_port,
        "GET /items?id=1 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer invalid\r\n\r\n",
    );
    assert_unauthorized(code, &res, "不正トークン GET /items");

    let auth_header = format!("Authorization: Bearer {TEST_TOKEN}\r\n");

    // 3b. 重複 Authorization ヘッダー（有効 Bearer 先頭 + 無効 Bearer）: 401 で拒否
    let (code, res) = send_http(
        srv_port,
        &format!(
            "GET /items?id=1 HTTP/1.1\r\nHost: localhost\r\n{auth_header}Authorization: Bearer invalid\r\n\r\n"
        ),
    );
    assert_unauthorized(code, &res, "重複 Authorization（有効+無効）");

    // 3c. 重複 Authorization ヘッダー（同値の重複）: 401 で拒否
    let (code, res) = send_http(
        srv_port,
        &format!("GET /items?id=1 HTTP/1.1\r\nHost: localhost\r\n{auth_header}{auth_header}\r\n"),
    );
    assert_unauthorized(code, &res, "重複 Authorization（同値重複）");

    // 3d. 認証なし + 不正クエリ（%ZZ）: Router の 400 応答がそのまま返り保存・読み出しへ進まないこと（指摘1）
    let (code, res) = send_http(
        srv_port,
        "POST /items?id=42&name=test&x=%ZZ HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    assert_eq!(code, 400, "未認証+不正クエリ POST は 400: {res}");
    let (code, res) = send_http(
        srv_port,
        "GET /items?id=42&x=%ZZ HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    assert_eq!(code, 400, "未認証+不正クエリ GET は 400: {res}");

    // 4. 入力検証（不正形式）: POST /items?id=abc&name=test
    let (code, res) = send_http(
        srv_port,
        &format!("POST /items?id=abc&name=test HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 400, "不正形式 id は 400: {res}");

    // 5. 入力検証（篩型範囲外: 0）: POST /items?id=0&name=test
    let (code, res) = send_http(
        srv_port,
        &format!("POST /items?id=0&name=test HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 400, "id=0 は 400: {res}");

    // 6. 入力検証（篩型範囲外: 99999）: POST /items?id=99999&name=test
    let (code, res) = send_http(
        srv_port,
        &format!("POST /items?id=99999&name=test HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 400, "id=99999 は 400: {res}");

    // 6b. 入力検証（桁あふれ: 9223372036854775808）: GET /items?id=9223372036854775808
    let (code, res) = send_http(
        srv_port,
        &format!(
            "GET /items?id=9223372036854775808 HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"
        ),
    );
    assert_eq!(code, 400, "桁あふれ GET id は 400: {res}");

    // 6c. 入力検証（桁あふれ: 9223372036854775808）: POST /items?id=9223372036854775808&name=test
    let (code, res) = send_http(
        srv_port,
        &format!("POST /items?id=9223372036854775808&name=test HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 400, "桁あふれ POST id は 400: {res}");

    // 6d. 入力検証（重複クエリキー: id重複 POST /items?id=42&id=43&name=test）: 400
    let (code, res) = send_http(
        srv_port,
        &format!(
            "POST /items?id=42&id=43&name=test HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"
        ),
    );
    assert_eq!(code, 400, "重複 id クエリキー POST は 400: {res}");

    // 6e. 入力検証（重複クエリキー: name重複 POST /items?id=42&name=first&name=second）: 400
    let (code, res) = send_http(
        srv_port,
        &format!("POST /items?id=42&name=first&name=second HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 400, "重複 name クエリキー POST は 400: {res}");

    // 6f. 入力検証（重複クエリキー: id重複 GET /items?id=42&id=43）: 400
    let (code, res) = send_http(
        srv_port,
        &format!("GET /items?id=42&id=43 HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 400, "重複 id クエリキー GET は 400: {res}");

    // 7. 正常書き込み: POST /items?id=42&name=sigil_box_42
    let (code, res) = send_http(
        srv_port,
        &format!(
            "POST /items?id=42&name=sigil_box_42 HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"
        ),
    );
    assert_eq!(code, 200, "POST /items 成功: {res}");
    assert!(res.contains("stored"), "POST /items body: {res}");

    // 8. 正常読み出し: GET /items?id=42
    let (code, res) = send_http(
        srv_port,
        &format!("GET /items?id=42 HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 200, "GET /items 成功: {res}");
    assert!(res.contains("sigil_box_42"), "GET /items body: {res}");

    // 8b. URL 復号を伴う書き込み・読み出し（ADR 0033 補足A H1: %20・+・UTF-8の1回復号）: POST /items?id=43&name=hello%20world+sigil
    let (code, res) = send_http(
        srv_port,
        &format!(
            "POST /items?id=43&name=hello%20world+sigil HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"
        ),
    );
    assert_eq!(code, 200, "POST /items (URL復号対象) 成功: {res}");
    assert!(res.contains("stored"), "POST /items body: {res}");

    // 8c. URL 復号値の読み出し確認（hello world sigil として保存されていること）
    let (code, res) = send_http(
        srv_port,
        &format!("GET /items?id=43 HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 200, "GET /items (URL復号確認) 成功: {res}");
    assert!(
        res.contains("hello world sigil"),
        "GET /items body に復号された 'hello world sigil' が含まれること: {res}"
    );
    assert!(
        !res.contains("hello%20world"),
        "GET /items body に生の 'hello%20world' が含まれないこと: {res}"
    );

    // 9. 存在しないアイテム: GET /items?id=999
    let (code, res) = send_http(
        srv_port,
        &format!("GET /items?id=999 HTTP/1.1\r\nHost: localhost\r\n{auth_header}\r\n"),
    );
    assert_eq!(code, 404, "GET /items 未登録は 404: {res}");

    // 10. 契約違反の要求: GET /fault
    let (code, res) = send_http(srv_port, "GET /fault HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(code, 500, "契約違反は 500 で安全に終了: {res}");

    // 11. 契約違反後もサービスが継続して応答できること
    let (code, res) = send_http(srv_port, "GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert_eq!(code, 200, "契約違反後も GET /health が成功: {res}");
    assert!(res.contains("ok"), "GET /health body: {res}");

    // 12. 起動時時計取得不能時の起動拒否（ADR 0037 決定 4: 待受前に壁時計を確認し、取得不能なら待ち受けを開始せず終了コード 1 で終了）
    let fail_clock = include_str!("../src/stdlib/os_clock.sigil")
        .replace("  return Ok(sec);", "  return Err(-1);");
    let fail_opts = sigil::effects::Options {
        embedded_extra: vec![("std.os.clock".into(), fail_clock)],
        ..Default::default()
    };
    let fail_p = sigil::ir_eval::lower_sources_opts(&[&src], "main", &fail_opts)
        .expect("時計失敗 lowering 成功");
    let fail_dir = scratch("service-06-clock-fail");
    let fail_exe = fail_dir.join("service");
    sigil::native::build_program(&fail_p, &Default::default(), &fail_exe)
        .expect("時計失敗ネイティブビルド成功");

    let mut fail_child = Command::new(&fail_exe)
        .env_clear()
        .env("PORT", "0")
        .env("MAX_CONNS", "20")
        .env("POSTGRES_HOST", "127.0.0.1")
        .env("POSTGRES_PORT", pg_port.to_string())
        .env("POSTGRES_USER", &pg_user)
        .env("POSTGRES_DB", &pg_db)
        .env("POSTGRES_PASSWORD", &pg_pass)
        .env("SIGIL_AUTH_HMAC_KEY_HEX", TEST_KEY)
        .env("SIGIL_AUTH_AUDIENCE", TEST_AUD)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("時計失敗サービス起動");

    let fail_stdout = fail_child.stdout.take().expect("標準出力取得");
    let mut fail_reader = BufReader::new(fail_stdout);
    let mut fail_out_line = String::new();
    fail_reader
        .read_line(&mut fail_out_line)
        .expect("出力読み取り");
    let fail_status = fail_child.wait().expect("プロセスの終了待機");
    assert!(fail_status.success(), "プロセス終了");
    assert_eq!(
        fail_out_line.trim(),
        "1",
        "壁時計取得不能エラー（終了コード 1）で待ち受けを開始せず終了すること"
    );
    let _ = std::fs::remove_dir_all(&fail_dir);

    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR 0037「決定 4」: 待ち受け開始前に壁時計を確認し、取得不能時は待ち受けを開始せず起動拒否することを確認する。
#[test]
fn service_06_unavailable_clock_refuses_startup() {
    let src = service_src();
    let fail_clock = include_str!("../src/stdlib/os_clock.sigil")
        .replace("  return Ok(sec);", "  return Err(-1);");
    let fail_opts = sigil::effects::Options {
        embedded_extra: vec![("std.os.clock".into(), fail_clock)],
        ..Default::default()
    };
    let fail_p = sigil::ir_eval::lower_sources_opts(&[&src], "main", &fail_opts)
        .expect("時計失敗 lowering 成功");

    let dir = scratch("service-06-no-clock");
    let exe = dir.join("service");
    sigil::native::build_program(&fail_p, &Default::default(), &exe).expect("ネイティブビルド成功");

    let mut child = Command::new(&exe)
        .env_clear()
        .env("PORT", "0")
        .env("SIGIL_AUTH_HMAC_KEY_HEX", TEST_KEY)
        .env("SIGIL_AUTH_AUDIENCE", TEST_AUD)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("サービス起動");

    let stdout = child.stdout.take().expect("標準出力取得");
    let mut reader = BufReader::new(stdout);
    let mut out_line = String::new();
    reader.read_line(&mut out_line).expect("出力読み取り");
    let status = child.wait().expect("プロセスの終了待機");
    assert!(status.success(), "プロセス終了");
    assert_eq!(
        out_line.trim(),
        "1",
        "壁時計取得不能エラー（終了コード 1）で待ち受けを開始せず終了すること"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR 0037「決定 2」: 必須設定（SIGIL_AUTH_HMAC_KEY_HEX）未設定時は待ち受けを開始せず起動拒否することを確認する。
#[test]
fn service_06_missing_auth_key_refuses_startup() {
    let src = service_src();
    let p = sigil::ir_eval::lower_sources(&[&src], "main").expect("lowering 成功");

    let dir = scratch("service-06-no-key");
    let exe = dir.join("service");
    sigil::native::build_program(&p, &Default::default(), &exe).expect("ネイティブビルド成功");

    let mut child = Command::new(&exe)
        .env_clear()
        .env("PORT", "0")
        .env("SIGIL_AUTH_AUDIENCE", TEST_AUD)
        // SIGIL_AUTH_HMAC_KEY_HEX は意図的に設定しない
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("サービス起動");

    let stdout = child.stdout.take().expect("標準出力取得");
    let mut reader = BufReader::new(stdout);
    let mut out_line = String::new();
    reader.read_line(&mut out_line).expect("出力読み取り");
    let status = child.wait().expect("プロセスの終了待機");
    assert!(status.success(), "プロセス終了");
    assert_eq!(
        out_line.trim(),
        "1",
        "認証設定エラー（終了コード 1）で待ち受けを開始せず終了すること"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

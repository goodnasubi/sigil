//! ADR 0033 E・I・J。評価器とネイティブで同じ入力環境・定型応答を比較する。
mod common;

use sigil::exec::{Arg, Options};
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const DRIVER: &str = r#"
module app.config_test
  targets server
  trust none

fn decimal(value: Int) -> String
  intent "非負整数を十進の文字列にする"
  requires value >= 0
  ensures none
  effects none
{
  let mut reverse = Bytes.new();
  let mut n = value;
  reverse.push(48 + n % 10);
  n = n / 10;
  while n > 0 {
    reverse.push(48 + n % 10);
    n = n / 10;
  }
  let mut digits = Bytes.new();
  let mut i = reverse.len();
  while i > 0 {
    i = i - 1;
    digits.push(reverse.get(i).unwrap_or(48));
  }
  return digits.to_string_utf8().unwrap_or("");
}

fn config() -> String
  intent "設定の全項目を外から読む"
  requires none
  ensures none
  effects env
{
  match web.load_config() {
    Err(e) => { return "error:".as_bytes().concat(&e.name().as_bytes()).to_string_utf8().unwrap_or(""); }
    Ok(c) => {
      let h = c.http_limits();
      let a = c.app_limits();
      let mut out = c.host().as_bytes();
      out.extend(&":".as_bytes());
      out.extend(&decimal(c.port()).as_bytes());
      out.extend(&"|".as_bytes());
      out.extend(&decimal(h.request_line()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.head()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.headers()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.body()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.chunk_line()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.encoded_body()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.conns()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.backlog()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.head_ms()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.body_ms()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.handler_ms()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.send_ms()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.idle_ms()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.requests()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(h.response_body()).as_bytes());
      out.extend(&"|".as_bytes());
      out.extend(&decimal(a.query_params_max()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(a.json_max_depth()).as_bytes());
      out.extend(&",".as_bytes());
      out.extend(&decimal(a.shutdown_grace_ms()).as_bytes());
      return out.to_string_utf8().unwrap_or("");
    }
  }
  return "unreachable";
}

fn startup() -> String
  intent "検証成功時だけ起動用のメモリ見積もりを出す"
  requires none
  ensures none
  effects env, console
{
  match web.load_config() {
    Err(e) => { return e.name(); }
    Ok(c) => { c.log_memory_bound(); return "ready"; }
  }
  return "unreachable";
}

fn response(code: Int) -> String
  intent "固定のエラー応答の全要素を読む"
  requires none
  ensures none
  effects none
{
  let r = web.error_response(code);
  let headers = r.headers();
  let body = r.body().to_string_utf8().unwrap_or("bad utf8");
  let mut out = decimal(r.status()).as_bytes();
  out.extend(&"|".as_bytes());
  out.extend(&body.as_bytes());
  out.extend(&"|".as_bytes());
  let mut i = 0;
  while i < headers.len() {
    match headers.get(i) {
      Some(h) => {
        let value = h.value().to_string_utf8().unwrap_or("bad utf8");
        out.extend(&h.name().as_bytes());
        out.extend(&":".as_bytes());
        out.extend(&value.as_bytes());
        out.extend(&";".as_bytes());
      }
      None => {}
    }
    i = i + 1;
  }
  return out.to_string_utf8().unwrap_or("");
}
fn forwarded() -> String
  intent "前段ヘッダーは未検証の生値だけで要求のメソッドや対象を変えない"
  requires none
  ensures none
  effects none
{
  let limits = http.default_limits();
  let mut parser = http.parser(&limits);
  let wire = "GET /safe?x=1 HTTP/1.1\r\nHost: local\r\nX-Forwarded-For: 127.0.0.1\r\nForwarded: for=127.0.0.1;proto=https;host=admin\r\nX-Forwarded-Proto: https\r\nX-Forwarded-Host: admin\r\nX-Real-IP: 127.0.0.1\r\n\r\n".as_bytes();
  match parser.feed(&wire, 0) {
    Ok(progress) => {
      match progress {
        HttpProgress.Done(req, _) => {
          let mut out = req.method().as_bytes();
          out.extend(&"|".as_bytes());
          out.extend(&req.path());
          out.extend(&"|".as_bytes());
          out.extend(&req.query());
          out.extend(&"|".as_bytes());
          out.extend(&req.header(&"forwarded").unwrap_or(Bytes.new()));
          return out.to_string_utf8().unwrap_or("");
        }
        HttpProgress.NeedMore => {}
      }
    }
    Err(_) => {}
  }
  return "bad request";
}

fn run(mode: Int, code: Int) -> String
  intent "同じ実行ファイルで設定と定型応答を比較する"
  requires none
  ensures none
  effects env, console
{
  if mode == 1 { return startup(); }
  if mode == 2 { return response(code); }
  if mode == 3 { return forwarded(); }
  return config();
}
"#;

fn opts(values: &[(&str, &str)]) -> Options {
    Options {
        env_vars: Some(
            values
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
        ..Options::captured()
    }
}

#[test]
fn limit_app_defaults() {
    let out = probe(0, 0, &opts(&[]));
    assert_eq!(out.value, "\"127.0.0.1:8080|8192,16384,100,1048576,1024,2097152,256,128,10000,30000,30000,30000,60000,1000,1048576|100,64,10000\"");
}

// ネイティブは一度だけビルドし、環境変数を隔離して全境界値を比較する。
fn probe(mode: i64, code: i64, options: &Options) -> sigil::exec::Outcome {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    let exe = EXE.get_or_init(|| {
        sigil::verify::verify_sources(&[DRIVER], "run").unwrap();
        let path = common::native::scratch("config460").join("probe");
        sigil::native::build(&[DRIVER], "run", &sigil::codegen::Opts::default(), &path).unwrap();
        path
    });
    let out = sigil::ir_eval::run_with(DRIVER, "run", &[Arg::Int(mode), Arg::Int(code)], options)
        .unwrap();
    let native = Command::new(exe)
        .args([mode.to_string(), code.to_string()])
        .env_clear()
        .envs(options.env_vars.as_ref().unwrap())
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "{:?}: {}",
        native.status,
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(native.stderr, b"");
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        format!("{}{}\n", out.output, out.value)
    );
    out
}

fn limit(name: &str, cap: i64, slot: usize) {
    for value in [
        "1".to_owned(),
        cap.to_string(),
        "0".into(),
        "-1".into(),
        (cap + 1).to_string(),
        "".into(),
        "not-an-integer".into(),
        "9223372036854775807".into(),
        "-9223372036854775808".into(),
        "9223372036854775808".into(),
        "-9223372036854775809".into(),
        " 1".into(),
        "1.0".into(),
    ] {
        let out = probe(0, 0, &opts(&[(name, &value)]));
        if value == "1" || value == cap.to_string() {
            let all: Vec<_> = out.value.trim_matches('"').split(['|', ',']).collect();
            assert_eq!(all[slot], value, "{name}");
        } else {
            assert_eq!(out.value, format!("\"error:{name}\""), "{name}={value}");
            assert!(out.output.is_empty());
        }
    }
}

macro_rules! limit_test {
    ($test:ident, $name:literal, $cap:expr, $slot:expr) => {
        #[test]
        fn $test() {
            limit($name, $cap, $slot);
        }
    };
}
limit_test!(
    limit_app_http_request_line,
    "SIGIL_HTTP_REQUEST_LINE_BYTES",
    65536,
    1
);
limit_test!(limit_app_http_head, "SIGIL_HTTP_HEAD_BYTES", 262144, 2);
limit_test!(limit_app_http_headers, "SIGIL_HTTP_HEADERS_MAX", 1000, 3);
limit_test!(limit_app_http_body, "SIGIL_HTTP_BODY_BYTES", 67108864, 4);
limit_test!(
    limit_app_http_chunk_line,
    "SIGIL_HTTP_CHUNK_LINE_BYTES",
    1024,
    5
);
limit_test!(
    limit_app_http_encoded_body,
    "SIGIL_HTTP_ENCODED_BODY_BYTES",
    134217728,
    6
);
limit_test!(limit_app_http_conns, "SIGIL_HTTP_CONNS_MAX", 4096, 7);
limit_test!(limit_app_http_backlog, "SIGIL_HTTP_BACKLOG", 1024, 8);
limit_test!(limit_app_http_head_ms, "SIGIL_HTTP_HEAD_MS", 600000, 9);
limit_test!(limit_app_http_body_ms, "SIGIL_HTTP_BODY_MS", 600000, 10);
limit_test!(
    limit_app_http_handler_ms,
    "SIGIL_HTTP_HANDLER_MS",
    600000,
    11
);
limit_test!(limit_app_http_send_ms, "SIGIL_HTTP_SEND_MS", 600000, 12);
limit_test!(limit_app_http_idle_ms, "SIGIL_HTTP_IDLE_MS", 600000, 13);
limit_test!(
    limit_app_http_requests,
    "SIGIL_HTTP_REQUESTS_PER_CONN",
    100000,
    14
);
limit_test!(
    limit_app_http_response_body,
    "SIGIL_HTTP_RESPONSE_BODY_BYTES",
    67108864,
    15
);
limit_test!(
    limit_app_query_params,
    "SIGIL_APP_QUERY_PARAMS_MAX",
    1000,
    16
);
limit_test!(limit_app_json_depth, "SIGIL_APP_JSON_MAX_DEPTH", 128, 17);
limit_test!(
    limit_app_shutdown_grace,
    "SIGIL_APP_SHUTDOWN_GRACE_MS",
    300000,
    18
);

#[test]
fn limit_app_port_and_host() {
    for value in [
        "1",
        "65535",
        "0",
        "-1",
        "65536",
        "",
        "bad",
        "9223372036854775808",
    ] {
        let out = probe(
            0,
            0,
            &opts(&[("SIGIL_APP_PORT", value), ("SIGIL_APP_HOST", "0.0.0.0")]),
        );
        if value == "1" || value == "65535" {
            assert!(out.value.starts_with(&format!("\"0.0.0.0:{value}|")));
        } else {
            assert_eq!(out.value, "\"error:SIGIL_APP_PORT\"");
        }
    }
}

#[test]
fn startup_logs_only_the_memory_estimate_after_validation() {
    let out = probe(1, 0, &opts(&[]));
    let memory = 256_i64 * (2 * (8192 + 16384 + 1048576 + 1048576) + 65536 + 16384);
    assert_eq!(
        out.output,
        format!("sigil.web memory_bound_bytes={memory}\n")
    );
    assert_eq!(out.value, "\"ready\"");
    let out = probe(
        1,
        0,
        &opts(&[(
            "SIGIL_HTTP_BODY_BYTES",
            "/secret/config:42 contract: token=SECRET",
        )]),
    );
    assert_eq!(out.value, "\"SIGIL_HTTP_BODY_BYTES\"");
    assert!(out.output.is_empty());
    let out = probe(
        1,
        0,
        &opts(&[
            ("SIGIL_HTTP_CONNS_MAX", "1"),
            ("SIGIL_HTTP_BODY_BYTES", "1"),
            ("SIGIL_APP_HOST", "sensitive-host"),
        ]),
    );
    let memory = 2 * (8192 + 16384 + 1 + 1048576) + 65536 + 16384;
    assert_eq!(
        out.output,
        format!("sigil.web memory_bound_bytes={memory}\n")
    );
}

#[test]
fn fixed_error_responses_never_include_diagnostics() {
    for (code, text) in [
        (400, "bad request"),
        (401, "unauthorized"),
        (403, "forbidden"),
        (404, "not found"),
        (405, "method not allowed"),
        (408, "request timeout"),
        (413, "content too large"),
        (414, "uri too long"),
        (417, "expectation failed"),
        (431, "request header fields too large"),
        (500, "internal server error"),
        (501, "not implemented"),
        (503, "service unavailable"),
        (505, "http version not supported"),
    ] {
        let out = probe(
            2,
            code,
            &opts(&[("SECRET", "/private/file:42 RequiresViolated token")]),
        );
        assert_eq!(
            out.value,
            format!("\"{code}|{text}|content-type:text/plain; charset=utf-8;\"")
        );
        assert!(out.output.is_empty());
    }
    for code in [i64::MIN, 0, 200, 499, 599, 600, i64::MAX] {
        assert_eq!(
            probe(2, code, &opts(&[])).value,
            "\"500|internal server error|content-type:text/plain; charset=utf-8;\""
        );
    }
}

#[test]
fn forwarded_headers_have_no_trust_configuration_or_client_ip_api() {
    assert_eq!(
        probe(3, 0, &opts(&[])).value,
        "\"GET|/safe|x=1|for=127.0.0.1;proto=https;host=admin\""
    );
    let normal = probe(0, 0, &opts(&[]));
    let spoof = probe(
        0,
        0,
        &opts(&[
            ("SIGIL_APP_TRUST_PROXY", "true"),
            ("HTTP_X_FORWARDED_FOR", "127.0.0.1"),
            ("HTTP_FORWARDED", "for=127.0.0.1;proto=https;host=admin"),
        ]),
    );
    assert_eq!(normal.value, spoof.value);
    for call in [
        "web.client_ip()",
        "web.forwarded_host()",
        "web.trust_proxy()",
    ] {
        let source = format!("module app.bad\n targets server\n trust none\nfn run() -> String\n intent \"信頼経路を公開しない\"\n requires none\n ensures none\n effects none\n{{ return {call}; }}");
        assert!(sigil::check::check(&source).is_err(), "{call}");
    }
}

#[test]
fn module_has_no_ambient_file_or_raw_capability() {
    let src = include_str!("../src/stdlib/web.sigil");
    let file = sigil::parser::parse_std(src).unwrap();
    assert_eq!(file.module.trust, sigil::ast::Trust::None);
    for forbidden in ["fs.read", "fs.write", "raw.", "unchecked"] {
        assert!(!src.contains(forbidden));
    }
}

#[test]
fn config_api_keeps_effects_and_private_limits() {
    let src = "module app.bad\n targets server\n trust none\nfn run() -> Result<AppConfig, ConfigError>\n intent \"環境効果が必要\"\n requires none\n ensures none\n effects none\n{ return web.load_config(); }";
    assert!(sigil::check::check(src).is_err());
    assert!(sigil::check::check(
        &src.replace("targets server", "targets bare")
            .replace("effects none", "effects env")
    )
    .is_err());
    let src = "module app.bare\n targets bare\n trust none\nfn run() -> Int\n intent \"未使用なら核の利用者を妨げない\"\n requires none\n ensures none\n effects none\n{ return 1; }";
    assert_eq!(common::run(src, "run", &[]).unwrap().value, "1");
}

//! ADR 0037 / 6-6c: 起動設定と所有する認証アダプタ。
mod common;
use sigil::exec::Options;

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const HEAD: &str = "module test.config509\n  targets server\n  trust none\n";
fn options(key: Option<&str>, aud: Option<&str>) -> Options {
    let mut env = std::collections::BTreeMap::new();
    if let Some(k) = key {
        env.insert("SIGIL_AUTH_HMAC_KEY_HEX".into(), k.into());
    }
    if let Some(a) = aud {
        env.insert("SIGIL_AUTH_AUDIENCE".into(), a.into());
    }
    Options {
        env_vars: Some(env),
        wall_clock: Some(Ok(1700000000)),
        trace_drops: false,
        ..Options::captured()
    }
}
#[test]
fn config_errors_and_owned_adapter() {
    let src = format!(
        r#"{HEAD}
fn safe_error(e: &ConfigError) -> String
 intent "変数名と固定分類だけを表示する"
 requires none ensures none effects none
{{ return e.name().as_bytes().concat(&":".as_bytes()).concat(&e.kind().as_bytes()).to_string_utf8().unwrap_or(""); }}
fn probe() -> String
 intent "起動設定"
 requires none ensures none effects env, clock
{{
 match web.load_bearer_config() {{
   Err(e) => {{ return safe_error(&e); }}
   Ok(config) => {{
     match config.into_verifier() {{
       Ok(verifier) => {{ let router = Router.new().with_auth(verifier); return "ok"; }}
       Err(e) => {{ return safe_error(&e); }}
     }}
   }}
 }}
}}
"#
    );
    let short = &KEY[..62];
    let long = format!("{KEY}00");
    let upper = KEY.to_uppercase();
    let whitespace = format!(" {KEY}");
    let non_hex = "g".repeat(64);
    let long_aud = "a".repeat(129);
    let dir = common::native::scratch("bearer509-config");
    let exe = dir.join("probe");
    sigil::native::build(&[&src], "probe", &Default::default(), &exe).unwrap();
    for (key, aud, want) in [
        (None, Some("api"), "SIGIL_AUTH_HMAC_KEY_HEX:missing"),
        (Some(""), Some("api"), "SIGIL_AUTH_HMAC_KEY_HEX:invalid"),
        (Some(short), Some("api"), "SIGIL_AUTH_HMAC_KEY_HEX:invalid"),
        (
            Some(long.as_str()),
            Some("api"),
            "SIGIL_AUTH_HMAC_KEY_HEX:invalid",
        ),
        (
            Some(upper.as_str()),
            Some("api"),
            "SIGIL_AUTH_HMAC_KEY_HEX:invalid",
        ),
        (
            Some(whitespace.as_str()),
            Some("api"),
            "SIGIL_AUTH_HMAC_KEY_HEX:invalid",
        ),
        (
            Some(non_hex.as_str()),
            Some("api"),
            "SIGIL_AUTH_HMAC_KEY_HEX:invalid",
        ),
        (Some(KEY), None, "SIGIL_AUTH_AUDIENCE:missing"),
        (Some(KEY), Some(""), "SIGIL_AUTH_AUDIENCE:invalid"),
        (Some(KEY), Some("bad/aud"), "SIGIL_AUTH_AUDIENCE:invalid"),
        (Some(KEY), Some("日本語"), "SIGIL_AUTH_AUDIENCE:invalid"),
        (
            Some(KEY),
            Some(long_aud.as_str()),
            "SIGIL_AUTH_AUDIENCE:invalid",
        ),
        (Some(KEY), Some("api"), "ok"),
    ] {
        let opts = options(key, aud);
        let o = sigil::ir_eval::run_with(&src, "probe", &[], &opts).unwrap();
        assert_eq!(o.value, format!("{want:?}"));
        assert!(o.output.is_empty());
        let mut cmd = std::process::Command::new(&exe);
        cmd.env_clear();
        if let Some(k) = key {
            cmd.env("SIGIL_AUTH_HMAC_KEY_HEX", k);
        }
        if let Some(a) = aud {
            cmd.env("SIGIL_AUTH_AUDIENCE", a);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success());
        assert!(out.stderr.is_empty());
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{want:?}\n")
        );
    }
    let mut opts = options(Some(KEY), Some("api"));
    opts.wall_clock = Some(Err(1));
    let o = sigil::ir_eval::run_with(&src, "probe", &[], &opts).unwrap();
    assert_eq!(o.value, "\"clock.unix_seconds:unavailable\"");
    assert!(o.output.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

const DRIVER: &str = include_str!("data/bearer_web_509.sigil");
const DENIED: &str = "unauthorized|content-type:text/plain; charset=utf-8|www-authenticate:Bearer|cache-control:no-store";
#[test]
fn dispatch_fixed_failures_and_public_routes() {
    for (path, headers, want, hooks) in [
        ("/private", "", DENIED, ""),
        ("/private", "Authorization: nonsense\r\n", DENIED, ""),
        (
            "/private",
            "Authorization: Bearer nope\r\nAUTHORIZATION: Bearer nope\r\n",
            DENIED,
            "",
        ),
        (
            "/public",
            "Authorization: nonsense\r\n",
            "public|x-after:safe",
            "before\nafter\n",
        ),
        (
            "/missing",
            "",
            "not found|content-type:text/plain; charset=utf-8",
            "",
        ),
    ] {
        let wire = format!("GET {path} HTTP/1.1\r\nHost: local\r\n{headers}\r\n");
        let o = common::run_with(
            DRIVER,
            "dispatch",
            &[sigil::exec::Arg::Str(wire)],
            &options(Some(KEY), Some("sigil.api")),
        )
        .unwrap();
        assert_eq!(o.value, format!("{want:?}"));
        assert_eq!(o.output, hooks);
    }
}

fn token() -> String {
    let row = include_str!("data/bearer_507.tsv")
        .lines()
        .find(|l| l.starts_with("valid_iat_equals_now_lifetime_3600\t"))
        .unwrap();
    let hex = row.split('\t').nth(6).unwrap();
    String::from_utf8(
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect(),
    )
    .unwrap()
}
fn build_injected(
    src: &str,
    entry: &str,
    tag: &str,
    fail_after: Option<i32>,
    direct: bool,
) -> (std::path::PathBuf, std::path::PathBuf) {
    build_injected_with_trace(src, entry, tag, fail_after, direct, false)
}
fn build_injected_with_trace(
    src: &str,
    entry: &str,
    tag: &str,
    fail_after: Option<i32>,
    direct: bool,
    trace_drops: bool,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let mut clock = include_str!("../src/stdlib/os_clock.sigil").replace(
        "  let sec = raw.load_i64(ts);",
        "  let sec = raw.load_i64(ts);\n  TEST_CALLS = TEST_CALLS + 1;",
    );
    clock = clock.replace(
        "  return Ok(sec);",
        &match fail_after {
            Some(limit) => {
                format!("  if TEST_CALLS > {limit} {{ return Err(-1); }}\n  return Ok(1700000000);")
            }
            None => "  return Ok(1700000000);".into(),
        },
    );
    clock.push_str("\nglobal mut TEST_CALLS: Int = 0\n");
    // 読込済み設定の後に env.get を呼べば必ず失敗する差替え。
    // 起動時の二変数各一回以降の再読込がないことをネイティブで観測する。
    let mut env = include_str!("../src/stdlib/os_env.sigil").replace(
        "  return lookup(ENVP, np, nl);",
        "  if nl >= 11 && raw.load_u8(raw.offset(np, 0)) == 83 && raw.load_u8(raw.offset(np, 1)) == 73 && raw.load_u8(raw.offset(np, 2)) == 71 && raw.load_u8(raw.offset(np, 3)) == 73 && raw.load_u8(raw.offset(np, 4)) == 76 && raw.load_u8(raw.offset(np, 5)) == 95 && raw.load_u8(raw.offset(np, 6)) == 65 && raw.load_u8(raw.offset(np, 7)) == 85 && raw.load_u8(raw.offset(np, 8)) == 84 && raw.load_u8(raw.offset(np, 9)) == 72 && raw.load_u8(raw.offset(np, 10)) == 95 {\n    TEST_READS = TEST_READS + 1;\n    if TEST_READS > 2 { return raw.addr_null(); }\n  }\n  return lookup(ENVP, np, nl);",
    );
    env.push_str("\nglobal mut TEST_READS: Int = 0\n");
    let mut extra = vec![("std.os.clock".into(), clock), ("std.os.env".into(), env)];
    if direct {
        extra.push(("std.http".into(), direct_http()));
    }
    let opts = sigil::effects::Options {
        embedded_extra: extra,
        ..Default::default()
    };
    let program = sigil::ir_eval::lower_sources_opts(&[src], entry, &opts).unwrap();
    let dir = common::native::scratch(tag);
    let exe = dir.join("probe");
    sigil::native::build_program(
        &program,
        &sigil::codegen::Opts {
            trace_drops,
            ..Default::default()
        },
        &exe,
    )
    .unwrap();
    (dir, exe)
}
fn direct_http() -> String {
    // parser を経由しない HttpRequest を、フィールドの所有モジュール内で作る。
    // 公開 feed 入口だけを試験時に差替え、製品 API/原始操作は増やさない。
    let http = include_str!("../src/stdlib/http.sigil");
    let start = http.find("  pub fn feed(").unwrap();
    let body = http[start..].find('{').unwrap() + start;
    let mut depth = 1;
    let mut end = body + 1;
    while depth > 0 {
        match http.as_bytes()[end] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        end += 1;
    }
    let replacement = r#"
      let mut headers = List.new();
      headers.push(HttpHeader { h_name: "Authorization", h_value: "Bearer invalid".as_bytes() });
      headers.push(HttpHeader { h_name: "AUTHORIZATION", h_value: "Bearer invalid".as_bytes() });
      return Ok(HttpProgress.Done(HttpRequest { req_method: "GET", req_path: "/private".as_bytes(), req_query: Bytes.new(), req_version: 11, req_headers: headers, req_body: Bytes.new() }, 0));
    }"#;
    format!("{}{}{}", &http[..body + 1], replacement, &http[end..])
}
#[test]
fn verified_identity_and_uniform_rejections_eval_native() {
    let (dir, exe) = build_injected(DRIVER, "dispatch", "bearer509-dispatch", None, false);
    let valid = token();
    let mut cases = vec![
        (
            "/private",
            format!("Authorization: bEaReR {valid}\r\n"),
            "user:alice-01|x-after:safe",
            "before\nafter\n",
        ),
        (
            "/public",
            format!("Authorization: Bearer {valid}\r\n"),
            "public|x-after:safe",
            "before\nafter\n",
        ),
    ];
    for auth in [
        format!("Bearer {valid} "),
        format!("Bearer  {valid}"),
        format!("Bearer\t{valid}"),
        format!("Bearer {valid},foo"),
        format!("Bearer {}x", valid),
        "Basic whatever".into(),
    ] {
        // 外側OWSはHTTP parserで除かれるので、末尾SPだけは正常として扱う。
        let want = if auth.ends_with(' ') && !auth.ends_with("  ") {
            "user:alice-01|x-after:safe"
        } else {
            DENIED
        };
        let hooks = if want == DENIED {
            ""
        } else {
            "before\nafter\n"
        };
        cases.push((
            "/private",
            format!("Authorization: {auth}\r\n"),
            want,
            hooks,
        ));
    }
    cases.push((
        "/private",
        format!("Authorization: Bearer {valid}\r\nAUTHORIZATION: Bearer {valid}\r\n"),
        DENIED,
        "",
    ));
    for (path, headers, want, hooks) in cases {
        let wire = format!("GET {path} HTTP/1.1\r\nHost: local\r\n{headers}\r\n");
        let o = sigil::ir_eval::run_with(
            DRIVER,
            "dispatch",
            &[sigil::exec::Arg::Str(wire.clone())],
            &options(Some(KEY), Some("sigil.api")),
        )
        .unwrap();
        assert_eq!(o.value, format!("{want:?}"));
        assert_eq!(o.output, hooks);
        let out = std::process::Command::new(&exe)
            .arg(&wire)
            .env_clear()
            .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
            .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{hooks}{want:?}\n")
        );
        assert!(out.stderr.is_empty());
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn direct_dispatch_without_http_normalization_and_request_clock_failure() {
    for (direct, fail) in [(true, false), (false, true)] {
        let (dir, exe) = build_injected(
            DRIVER,
            "dispatch",
            "bearer509-fault",
            if fail { Some(1) } else { None },
            direct,
        );
        let wire = format!(
            "GET /private HTTP/1.1\r\nHost: local\r\nAuthorization: Bearer {}\r\n\r\n",
            token()
        );
        let mut opts = options(Some(KEY), Some("sigil.api"));
        if direct {
            opts.embedded_extra.push(("std.http".into(), direct_http()));
        }
        if fail {
            // 起動時計は成功のまま、要求の取得点だけ OS 失敗値へ置換する。
            // 実際の clock 呼出しも残し、効果検査を回避しない。
            let web = include_str!("../src/stdlib/web.sigil").replace(
                "  let now = clock.unix_seconds();",
                "  let observed = clock.unix_seconds();\n  let now = None;",
            );
            opts.embedded_extra.push(("std.web".into(), web));
        }
        let o = sigil::ir_eval::run_with(
            DRIVER,
            "dispatch",
            &[sigil::exec::Arg::Str(wire.clone())],
            &opts,
        )
        .unwrap();
        assert_eq!(o.value, format!("{DENIED:?}"));
        assert!(o.output.is_empty());
        let out = std::process::Command::new(&exe)
            .arg(&wire)
            .env_clear()
            .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
            .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8(out.stdout).unwrap(),
            format!("{DENIED:?}\n")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn connect_once(exe: &std::path::Path, wire: &str) -> String {
    connect_with_trace(exe, wire, false)
}
fn connect_with_trace(exe: &std::path::Path, wire: &str, trace: bool) -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Command, Stdio};
    use std::time::Duration;
    let mut child = Command::new(exe)
        .env_clear()
        .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
        .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        let mut line = String::new();
        let r = reader.read_line(&mut line);
        tx.send((r, line, reader)).unwrap();
    });
    let (_, line, mut reader) = match rx.recv_timeout(Duration::from_secs(15)) {
        Ok(v) => v,
        Err(e) => {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("port timeout: {e}");
        }
    };
    thread.join().unwrap();
    let port: u16 = line.trim().parse().unwrap_or_else(|_| {
        child.kill().unwrap();
        panic!("port line: {line:?}")
    });
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    socket.write_all(wire.as_bytes()).unwrap();
    socket.shutdown(std::net::Shutdown::Write).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    let mut hooks = String::new();
    reader.read_to_string(&mut hooks).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    if trace {
        let drops = String::from_utf8(out.stderr).unwrap();
        assert!(drops.contains("std.http.HttpBuf(***)"));
        assert!(drops.contains("std.web.AppRequest(***)"));
        assert!(!drops.contains(KEY));
        let token_bytes = token()
            .bytes()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert!(!drops.contains(&token_bytes));
        assert!(!drops.contains("h_value:"));
    } else {
        assert!(out.stderr.is_empty());
    }
    assert!(!response.contains(KEY));
    if response.starts_with("HTTP/1.1 401") {
        assert_eq!(hooks, "0\n");
    } else {
        assert_eq!(hooks, "before\nafter\n0\n");
    }
    response
}
#[test]
fn native_connections_startup_rejection_sharing_and_fixed_response() {
    let (dir, exe) = build_injected(DRIVER, "serve", "bearer509-socket", None, false);
    let valid = token();
    let mut denied = None;
    for auth in [
        "".to_string(),
        "Authorization: Basic foo\r\n".into(),
        format!("Authorization: Bearer {valid}\r\nAUTHORIZATION: Bearer {valid}\r\n"),
        format!("Authorization: Bearer {valid}x\r\n"),
    ] {
        let wire =
            format!("GET /private HTTP/1.1\r\nHost: local\r\nConnection: close\r\n{auth}\r\n");
        let response = connect_once(&exe, &wire);
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        assert!(response.contains("www-authenticate: Bearer\r\n"));
        assert!(response.contains("cache-control: no-store\r\n"));
        assert!(response.ends_with("\r\n\r\nunauthorized"));
        if let Some(previous) = &denied {
            assert_eq!(&response, previous);
        } else {
            denied = Some(response);
        }
    }
    for path in ["/private", "/public"] {
        let response=connect_once(&exe,&format!("GET {path} HTTP/1.1\r\nHost: local\r\nConnection: close\r\nAuthorization: Bearer {valid}\r\n\r\n"));
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(response.ends_with(if path == "/private" {
            "user:alice-01"
        } else {
            "public"
        }));
        assert!(response.contains("x-after: safe\r\n"));
    }
    for (key, aud, name) in [
        ("", "sigil.api", "SIGIL_AUTH_HMAC_KEY_HEX"),
        ("00", "sigil.api", "SIGIL_AUTH_HMAC_KEY_HEX"),
        (KEY, "bad/aud", "SIGIL_AUTH_AUDIENCE"),
    ] {
        let o = std::process::Command::new(&exe)
            .env_clear()
            .env("SIGIL_AUTH_HMAC_KEY_HEX", key)
            .env("SIGIL_AUTH_AUDIENCE", aud)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(o.stdout).unwrap(), format!("{name}\n1\n"));
        assert!(o.stderr.is_empty());
    }
    std::fs::remove_dir_all(dir).unwrap();
    let (dir, exe) = build_injected(DRIVER, "serve", "bearer509-socket-clock", Some(1), false);
    let response=connect_once(&exe,&format!("GET /private HTTP/1.1\r\nHost: local\r\nConnection: close\r\nAuthorization: Bearer {valid}\r\n\r\n"));
    assert_eq!(response, denied.unwrap());
    std::fs::remove_dir_all(dir).unwrap();
    let (dir, exe) = build_injected(DRIVER, "serve", "bearer509-startup-clock", Some(0), false);
    let out = std::process::Command::new(&exe)
        .env_clear()
        .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
        .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
        .output()
        .unwrap();
    assert_eq!(out.stdout, b"clock.unix_seconds\n1\n");
    assert!(out.stderr.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn minimal_effects_ownership_and_traits() {
    let f = |signature: &str, effects: &str, body: &str| {
        format!("{HEAD}\nfn {signature}\n intent \"効果と所有\"\n requires none\n ensures none\n effects {effects}\n{{ {body} }}")
    };
    for (signature, body, effects) in [
        (
            "load() -> Result<BearerConfig, ConfigError>",
            "return web.load_bearer_config();",
            "env",
        ),
        (
            "adapt(c: BearerConfig) -> Result<AuthVerifier, ConfigError>",
            "return c.into_verifier();",
            "clock",
        ),
    ] {
        sigil::check::check(&f(signature, effects, body)).unwrap();
        assert_eq!(
            sigil::check::check(&f(signature, "none", body))
                .unwrap_err()
                .kind,
            sigil::ErrorKind::EffectMissing
        );
        assert_eq!(
            sigil::check::check(&f(signature, "env, clock", body))
                .unwrap_err()
                .kind,
            sigil::ErrorKind::EffectExtra
        );
    }
    let src = format!(
        r#"{HEAD}
fn sent<T: Sendable>(x: &T)
 intent "移送性" requires none ensures none effects none {{}}
fn shared<T: Shareable>(x: &T)
 intent "共有性" requires none ensures none effects none {{}}
fn probe() -> Int
 intent "不変設定と所有検証器の移送と共有"
 requires none ensures none effects env, clock, task.share
{{
 match web.load_bearer_config() {{
   Err(_) => {{ return 1; }}
   Ok(config) => {{
     sent(&config); shared(&config);
     match config.into_verifier() {{
       Err(_) => {{ return 2; }}
       Ok(verifier) => {{ sent(&verifier); shared(&verifier); let router = Router.new().with_auth(verifier); sent(&router); shared(&router); let refs = SyncShared.new(router); let copy = refs.clone(); let first = refs.borrow(); let second = copy.borrow(); return first.public_routes().len() + second.public_routes().len(); }}
     }}
   }}
 }}
}}
"#
    );
    let mut opts = options(Some(KEY), Some("sigil.api"));
    opts.trace_drops = true;
    let eval = sigil::ir_eval::run_with(&src, "probe", &[], &opts).unwrap();
    assert_eq!(eval.value, "0");
    assert_eq!(
        eval.drops
            .iter()
            .filter(|s| s.as_str() == "Secret(***)")
            .count(),
        2
    );
    assert!(!format!("{:?}", eval.drops).contains(KEY));
    assert!(!format!("{:?}", eval.drops).contains("data: Bytes"));
    let dir = common::native::scratch("bearer509-drops");
    let exe = dir.join("probe");
    sigil::native::build(
        &[&src],
        "probe",
        &sigil::codegen::Opts {
            trace_drops: true,
            ..Default::default()
        },
        &exe,
    )
    .unwrap();
    let out = std::process::Command::new(&exe)
        .env_clear()
        .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
        .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(out.stdout, b"0\n");
    let drops = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        drops.lines().filter(|s| *s == "drop: Secret(***)").count(),
        2
    );
    assert!(!drops.contains(KEY));
    assert!(!drops.contains("data: Bytes"));
    std::fs::remove_dir_all(dir).unwrap();
    let moved = f(
        "bad(c: BearerConfig) -> Result<AuthVerifier, ConfigError>",
        "clock",
        "let first = c.into_verifier(); return c.into_verifier();",
    );
    assert!(sigil::check::check(&moved).is_err());
    let display = f("bad(c: &BearerConfig)", "console", "console.log(&c);");
    assert!(sigil::check::check(&display).is_err());
}

#[test]
fn authentication_is_hidden_in_evaluator_and_native_drop_traces() {
    let wire=format!("GET /private HTTP/1.1\r\nHost: local\r\nConnection: close\r\nAuthorization: Bearer {}\r\n\r\n",token());
    let mut opts = options(Some(KEY), Some("sigil.api"));
    opts.trace_drops = true;
    let o = sigil::ir_eval::run_with(
        DRIVER,
        "dispatch",
        &[sigil::exec::Arg::Str(wire.clone())],
        &opts,
    )
    .unwrap();
    let drops = o.drops.join("\n");
    assert!(drops.contains("std.http.HttpRequest(***)"));
    assert!(drops.contains("std.web.AppRequest(***)"));
    assert!(drops.contains("std.http.HttpHeader(***)"));
    assert!(!drops.contains("h_value:"));
    assert!(!drops.contains(KEY));
    let token_bytes = token()
        .bytes()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    assert!(!drops.contains(&token_bytes));
    let (dir, exe) =
        build_injected_with_trace(DRIVER, "serve", "bearer509-trace", None, false, true);
    assert!(connect_with_trace(&exe, &wire, true).ends_with("user:alice-01"));
    std::fs::remove_dir_all(dir).unwrap();
    // 利用者が同名の型を定義しても標準HTTP型として扱わない。
    let src = format!(
        r#"{HEAD}
struct HttpBuf {{ text: String, }}
fn probe() -> Int
 intent "同名の利用者型は秘匿対象でない"
 requires none ensures none effects none
{{ let req=HttpBuf {{ text: "user-value" }}; return 0; }}
"#
    );
    let mut opts = Options::captured();
    opts.trace_drops = true;
    let o = common::run_with(&src, "probe", &[], &opts).unwrap();
    assert!(o.drops.iter().any(|d| d.contains("user-value")));
}

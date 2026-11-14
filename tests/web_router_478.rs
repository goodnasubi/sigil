//! ADR 0033 A・C・D・H・J。ソケットを開かず評価器とネイティブで比較する。
mod common;

use sigil::exec::{Arg, Options};
use std::process::Command;

const DRIVER: &str = include_str!("data/web_router_478.sigil");

#[test]
fn dispatch_matches_native() {
    let request = |method: &str, target: &str, headers: &str, body: &str| {
        format!("{method} {target} HTTP/1.1\r\nHost: local\r\n{headers}Content-Length: {}\r\n\r\n{body}", body.len())
    };
    let mut cases: Vec<(String, i64, String)> = Vec::new();
    let mut add =
        |method: &str, target: &str, headers: &str, body: &str, mode: i64, expected: &str| {
            cases.push((
                request(method, target, headers, body),
                mode,
                expected.into(),
            ));
        };
    for mode in [0, 1] {
        add("GET", "/public", "", "", mode, "200|public");
        add("GET", "/private", "", "", mode, "401|unauthorized");
    }
    add(
        "GET",
        "/private",
        "Authorization: Bearer valid\r\n",
        "",
        1,
        "200|subject",
    );
    add(
        "GET",
        "/private",
        "Authorization: Bearer valid\r\n",
        "",
        0,
        "401|unauthorized",
    );
    add(
        "GET",
        "/public",
        "Authorization: invalid\r\n",
        "",
        1,
        "200|public",
    );
    add(
        "GET",
        "/private",
        "X-Forwarded-For: 127.0.0.1\r\nForwarded: for=127.0.0.1\r\nX-Real-IP: 127.0.0.1\r\n",
        "",
        1,
        "401|unauthorized",
    );
    add("POST", "/public", "", "", 0, "405|method not allowed");
    add("GET", "/missing", "", "", 0, "404|not found");
    add("GET", "/", "", "", 0, "404|not found");
    add("GET", "/users/42", "", "", 0, "200|42");
    add("GET", "/users/%34%32", "", "", 0, "200|42");
    for id in ["0", "100", "-1", "no", "9223372036854775808"] {
        add("GET", &format!("/users/{id}"), "", "", 0, "400|bad request");
    }
    for path in [
        "/public/",
        "/public//x",
        "/./public",
        "/../public",
        "/%2e/public",
        "/%2E%2E/public",
        "/%00",
        "/%",
        "/%2",
        "/%zz",
        "/%ff",
        "/%c0%80",
        "/%ed%a0%80",
        "/echo/a%2fb",
        "/echo/a%5cb",
        "/echo/a%3fb",
        "/echo/a%23b",
        "/echo/a%0ab",
    ] {
        add("GET", path, "", "", 0, "400|bad request");
    }
    add(
        "GET",
        "/echo/%E6%97%A5%E6%9C%AC",
        "",
        "",
        0,
        "200|/echo/日本|GET",
    );
    add("GET", "/echo/a+b", "", "", 0, "200|/echo/a+b|GET");
    add("GET", "/echo/%252e%252e", "", "", 0, "200|/echo/%2e%2e|GET");
    for (query, expected) in [
        ("q=a+b%2Bc%26d%3De", "200|a b+c&d=e"),
        ("q=%E6%97%A5%E6%9C%AC", "200|日本"),
        ("q=", "200|"),
        ("q", "200|"),
        ("q=a&q=b", "400|bad request"),
        ("other=x", "400|bad request"),
        ("q=%00", "400|bad request"),
        ("q=%ff", "400|bad request"),
        ("q=%zz", "400|bad request"),
        ("%zz=x", "400|bad request"),
    ] {
        add("GET", &format!("/query?{query}"), "", "", 0, expected);
    }
    for fields in [100, 101] {
        add(
            "GET",
            &format!("/public?{}", vec!["x=1"; fields].join("&")),
            "",
            "",
            0,
            if fields == 100 {
                "200|public"
            } else {
                "400|bad request"
            },
        );
    }
    add(
        "GET",
        &format!("/public?{}", "&".repeat(100)),
        "",
        "",
        0,
        "400|bad request",
    );
    add("GET", "/public?ignored=%ff", "", "", 0, "400|bad request");
    add(
        "POST",
        "/json",
        "",
        r#"{"id":42,"name":"日本"}"#,
        0,
        "200|日本",
    );
    for body in [
        r#"{"id":0,"name":"a"}"#,
        r#"{"id":"42","name":"a"}"#,
        r#"{"id":42,"name":7}"#,
        r#"{"id":42}"#,
        r#"{"id":1,"id":2,"name":"a"}"#,
        "[]",
        "invalid",
    ] {
        add("POST", "/json", "", body, 0, "400|bad request");
    }
    let deep = format!("{}0{}", "[".repeat(65), "]".repeat(65));
    add("POST", "/json", "", &deep, 0, "400|bad request");
    add(
        "GET",
        "/public",
        "",
        "",
        2,
        "200|public|x-after-two|x-after-one",
    );
    add(
        "GET",
        "/public",
        "X-Stop: yes\r\n",
        "",
        2,
        "201|stopped|x-after-two|x-after-one",
    );
    add("GET", "/private", "", "", 3, "401|unauthorized");
    add("GET", "/public?x=%zz", "", "", 2, "400|bad request");
    add("GET", "/missing", "", "", 2, "404|not found");
    add(
        "GET",
        "/private",
        "Authorization: Bearer valid\r\n",
        "",
        3,
        "200|subject|x-after-two|x-after-one",
    );
    add(
        "GET",
        "/public",
        "",
        "",
        5,
        "GET /public;GET /users/:id;GET /query;POST /json;GET /echo;GET /echo/:name;POST /json-depth;GET /query-int;",
    );
    for (query, expected) in [
        ("42", "200|42"),
        ("0", "400|bad request"),
        ("no", "400|bad request"),
    ] {
        cases.push((
            request("GET", &format!("/query-int?q={query}"), "", ""),
            0,
            expected.into(),
        ));
    }
    for depth in [64, 65] {
        let body = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        cases.push((
            request("POST", "/json-depth", "", &body),
            0,
            if depth == 64 {
                "200|parsed"
            } else {
                "400|bad request"
            }
            .into(),
        ));
    }
    let dir = common::native::scratch("web-router-478");
    let exe = dir.join("dispatch");
    sigil::native::build(&[DRIVER], "probe", &sigil::codegen::Opts::default(), &exe).unwrap();
    let sets: Vec<_> = cases
        .iter()
        .map(|(request, mode, _)| vec![Arg::Str(request.clone()), Arg::Int(*mode)])
        .collect();
    sigil::verify::verify_sources(&[DRIVER], "probe").unwrap();
    let outcomes =
        sigil::ir_eval::run_roots_with(&[DRIVER], "probe", &sets, &Options::captured()).unwrap();
    for ((request, mode, expected), root) in cases.iter().zip(outcomes.roots) {
        let value = root.unwrap();
        assert_eq!(value, format!("{expected:?}"), "mode={mode} {request:?}");
        let native = Command::new(&exe)
            .arg(request)
            .arg(mode.to_string())
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert_eq!(
            String::from_utf8(native.stdout)
                .unwrap()
                .lines()
                .last()
                .unwrap(),
            value
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn registration_rejects_ambiguous_public_and_required_routes() {
    assert_eq!(common::run(DRIVER, "registration", &[]).unwrap().value, "0");
}

#[test]
fn query_limit_uses_validated_config() {
    let mut opts = Options::captured();
    opts.env_vars = Some(std::collections::BTreeMap::from([(
        "SIGIL_APP_QUERY_PARAMS_MAX".into(),
        "2".into(),
    )]));
    for (query, expected) in [
        ("a=1&b=2", "200|public"),
        ("a=1&b=2&c=3", "400|bad request"),
    ] {
        let request = format!("GET /public?{query} HTTP/1.1\r\nHost: local\r\n\r\n");
        let outcome =
            common::run_with(DRIVER, "probe", &[Arg::Str(request), Arg::Int(4)], &opts).unwrap();
        assert_eq!(outcome.value, format!("{expected:?}"));
    }
}

#[test]
fn hooks_order_short_circuit_and_auth_boundary() {
    let cases = [
        ("/public", "", 2, "before-1\nbefore-2\nafter-2\nafter-1\n"),
        (
            "/public",
            "X-Stop: yes\r\n",
            2,
            "before-1\nafter-2\nafter-1\n",
        ),
        ("/private", "", 3, ""),
        ("/public?x=%zz", "", 2, ""),
    ];
    for (path, header, mode, output) in cases {
        let wire = format!("GET {path} HTTP/1.1\r\nHost: local\r\n{header}\r\n");
        assert_eq!(
            common::run(DRIVER, "probe", &[Arg::Str(wire), Arg::Int(mode)])
                .unwrap()
                .output,
            output
        );
    }
}

#[test]
fn sample_matches_native() {
    let source = include_str!("../examples/11-web-router/main.sigil");
    let outcome = common::run(source, "main", &[]).unwrap();
    assert_eq!(outcome.value, "\"hello Sigil\"");
    assert_eq!(outcome.output, "/users/42\n");
}

#[test]
fn web_is_trust_none_and_keeps_capability_boundary() {
    let source = include_str!("../src/stdlib/web.sigil");
    let file = sigil::parse(source).unwrap();
    assert_eq!(file.module.trust, sigil::ast::Trust::None);
    for line in source
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
    {
        for forbidden in ["fs.", "net.", "raw.", "unchecked", "process", "foreign"] {
            assert!(!line.contains(forbidden), "{line}");
        }
    }
    assert!(source.contains(sigil::stdlib::WEB_CALLBACKS.trim()));
    let source = "module app.bad\n targets server\n trust none\nfn dangerous(req: &AppRequest) -> HttpResponse\n intent \"権限を持つ\"\n requires none\n ensures none\n effects fs.read\n{ let data = fs.read(&Path.new(&\"x\")); return HttpResponse.new(200, Bytes.new()); }\nfn store() -> RouteHandler\n intent \"危険なハンドラを格納しようとする\"\n requires none\n ensures none\n effects none\n{ return RouteHandler { handle: dangerous }; }";
    assert_eq!(
        sigil::check::check(source).unwrap_err().kind,
        sigil::ErrorKind::FnEffectExceeds
    );
    let bare = "module app.bad\n targets bare\n trust none\nfn run() -> Router\n intent \"OS型を核に持ち込めない\"\n requires none\n ensures none\n effects none\n{ return Router.new(); }";
    assert!(sigil::check::check(bare).is_err());
}

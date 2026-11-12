//! `std.http` の型と表の行の検査（ADR 0032 決定 2・6、PR 6-3g-b）。
//!
//! - 型は前置きの名前で、利用者は同じ名前の型を宣言できず、中身（フィールド）も作れない・読めない。
//! - 名前空間 `http` は OS 層で、`bare` を出力先に持つモジュールからは使えない。`http` はモジュール名の頭に使えない。
//! - `HttpLimits` は `Sendable` で、起動したタスクへ移せる（決定 2-3 の確かめ）。
//! - 評価器とネイティブで、型の取り出す関数と `HttpError.status` の写しが同じ（`common::run` が突き合わせる）。

mod common;

use sigil::exec::{Options, RunError};

fn prog(items: &str, body: &str, effects: &str) -> String {
    format!(
        "module app.main\n  targets server\n  trust none\n\n{items}\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

fn check_err(src: &str) -> String {
    let e =
        sigil::ir_eval::run_with(src, "main", &[], &Options::captured()).expect_err("止まるはず");
    match e.error {
        RunError::Check(c) => format!("{:?} {}", c.kind, c.message),
        RunError::Module { error, .. } => format!("{:?} {}", error.kind, error.message),
        other => panic!("{other:?}"),
    }
}

fn value(src: &str) -> String {
    common::run(src, "main", &[])
        .unwrap_or_else(|f| panic!("{f:?}"))
        .value
}

#[test]
fn http_types_cannot_be_declared_or_forged() {
    for t in [
        "HttpRequest",
        "HttpHeader",
        "HttpResponse",
        "HttpParser",
        "HttpProgress",
        "HttpLimits",
        "HttpError",
    ] {
        let m = check_err(&prog(
            &format!("struct {t} {{\n  x: Int,\n}}\n"),
            "  return 1;",
            "none",
        ));
        assert!(m.starts_with("DuplicateDefinition"), "{t}: {m}");
    }
    let forge = prog(
        "",
        "  let r = HttpResponse { res_status: 99, res_headers: List.new(), res_body: Bytes.new() };\n  return 1;",
        "none",
    );
    let m = check_err(&forge);
    assert!(m.contains("HttpResponse"), "{m}");
    let field = prog(
        "",
        "  let l = http.default_limits();\n  return l.lim_body;",
        "none",
    );
    let m = check_err(&field);
    assert!(m.contains("lim_body"), "{m}");
}

#[test]
fn http_is_os_layer_and_a_reserved_head() {
    let src = "module app.main\n  targets bare, server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let l = http.default_limits();\n  return l.body();\n}\n";
    let m = check_err(src);
    assert!(m.starts_with("OsItemInCore"), "{m}");
    let reserved = "module http.app\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n";
    let m = check_err(reserved);
    assert!(m.starts_with("ReservedModuleName"), "{m}");
}

/// `HttpLimits` は値（`Int`）だけを持ち、タスクへ移せる。起動したタスクの中で解釈器を作って使える（決定 2-3）。
#[test]
fn http_limits_moves_into_a_task() {
    let src = prog(
        "",
        r#"  return task.scope(|s| {
    let lim = http.default_limits().with_body(5);
    let h = s.spawn(move || {
      let mut p = http.parser(&lim);
      let data = "POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 6\r\n\r\nabcdef".as_bytes();
      match p.feed(&data, 0) {
        Ok(pr) => { return 0; }
        Err(e) => { return e.status(); }
      }
    });
    return h.join();
  });"#,
        "task.spawn, task.suspend",
    );
    assert_eq!(value(&src), "413");
}

/// 取り出す関数と状態コードの写し（評価器とネイティブで同じ。`common::run` が突き合わせる）。
#[test]
fn http_accessors_and_status_codes() {
    let src = prog(
        r#"fn code(data: &Bytes) -> Int
  intent "x"
  requires none
  ensures none
  effects none
{
  let lim = http.default_limits();
  let mut p = http.parser(&lim);
  match p.feed(&data, 0) {
    Ok(pr) => {
      match pr {
        HttpProgress.Done(r, next) => { return next; }
        HttpProgress.NeedMore => { return 0 - p.consumed(); }
      }
    }
    Err(e) => {
      match e {
        HttpError.BadRequest => { return e.status() * 10 + 1; }
        HttpError.UriTooLong => { return e.status() * 10 + 2; }
        HttpError.HeadersTooLarge => { return e.status() * 10 + 3; }
        HttpError.BodyTooLarge => { return e.status() * 10 + 4; }
        HttpError.NotImplemented => { return e.status() * 10 + 5; }
        HttpError.VersionNotSupported => { return e.status() * 10 + 6; }
        HttpError.ExpectationFailed => { return e.status() * 10 + 7; }
        HttpError.Timeout => { return e.status() * 10 + 8; }
      }
    }
  }
}
"#,
        r#"  let lim = http.default_limits();
  let mut p = http.parser(&lim);
  let data = "POST /a/b?x=1 HTTP/1.1\r\nHost: h\r\nX-A: v1\r\nx-a: v2\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\nhi".as_bytes();
  let mut sum = 0;
  match p.feed(&data, 0) {
    Ok(pr) => {
      match pr {
        HttpProgress.Done(r, next) => {
          let m = r.method();
          let hs = r.headers();
          sum = sum + m.len() + r.path().len() * 10 + r.query().len() * 100 + r.version() * 1000;
          sum = sum + hs.len() * 100000 + r.body_len() * 1000000;
          match r.header(&"X-A") {
            Some(v) => { sum = sum + v.len() * 10000000; }
            None => {}
          }
          match hs.get(0) {
            Some(h) => { sum = sum + h.name().len() * 100000000 + h.value().len() * 1000000000; }
            None => {}
          }
          if next != data.len() { return 0 - 1; }
        }
        HttpProgress.NeedMore => { return 0 - 2; }
      }
    }
    Err(e) => { return 0 - 3; }
  }
  // 見出しを読み終えた時点で 100 Continue を送ってよい（本文はまだ）。
  let mut q = http.parser(&lim);
  let head = "PUT / HTTP/1.1\r\nHost: h\r\nExpect: 100-continue\r\nContent-Length: 2\r\n\r\n".as_bytes();
  match q.feed(&head, 0) {
    Ok(pr) => {}
    Err(e) => { return 0 - 4; }
  }
  if !q.wants_continue() || !q.in_body() { return 0 - 5; }
  let res = HttpResponse.new(204, Bytes.new()).with_header(&"X-Y", &"z".as_bytes());
  if res.status() != 204 || res.headers().len() != 1 || res.body().len() != 0 { return 0 - 6; }
  let codes = code(&"GET / HTTP/1.1\r\n\r\n".as_bytes()) + code(&"GET / HTTP/3.0\r\n".as_bytes()) * 10000
    + code(&"TRACE / HTTP/1.1\r\n".as_bytes()) * 100000000;
  if codes != 4001 + 50560000 + 501500000000 { return codes; }
  return sum;"#,
        "none",
    );
    // m=4, path=4, query=3, version=11, 見出し 5, body=2, X-A の最初の値 2, 最初の見出し host=4 と h=1。
    assert_eq!(value(&src), "1422511344");
}

/// `HttpResponse.new` の状態は 200〜599（1xx・101 を返させない。決定 6）。
#[test]
fn http_response_status_range() {
    for (status, ok) in [
        (199, false),
        (200, true),
        (599, true),
        (600, false),
        (101, false),
    ] {
        let src = prog(
            "",
            &format!("  let r = HttpResponse.new({status}, Bytes.new());\n  return r.status();"),
            "none",
        );
        let got = sigil::ir_eval::run_with(&src, "main", &[], &Options::captured());
        match got {
            Ok(o) => assert!(ok, "{status}: {}", o.value),
            Err(f) => {
                assert!(!ok, "{status}: {f:?}");
                assert!(format!("{f:?}").contains("RequiresViolated"), "{f:?}");
            }
        }
    }
}

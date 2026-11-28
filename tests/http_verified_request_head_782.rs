//! Issue #782: 検証済みの要求ヘッドを公開するパーサー API の試験
//!
//! - パーサーが検証した要求ヘッド（リクエスト行・ヘッダー）を、呼び出し側がそのまま取得できる公開 API
//!   （`HttpParser.request_head`・`HttpParser.head`・`HttpRequest.head`・`HttpRequest.request_head`・`HttpRequestHead`）
//! - その API が返すヘッダーがパーサーの検証済みの内容と一致することの確認

mod common;

fn prog(items: &str, body: &str, effects: &str) -> String {
    format!(
        "module app.main\n  targets server\n  trust none\n\n{items}\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

fn value(src: &str) -> String {
    common::run(src, "main", &[])
        .unwrap_or_else(|f| panic!("{f:?}"))
        .value
}

/// ヘッダー検証完了前は `request_head()` が `None` を返し、完了後は `Some` を返す
#[test]
fn parser_request_head_lifecycle() {
    let src = prog(
        "",
        r#"  let lim = http.default_limits();
  let mut p = http.parser(&lim);

  // 1. リクエスト行の途中
  let part1 = "POST /api/v1/resource?action=create HTTP/1.1\r\n".as_bytes();
  match p.feed(&part1, 0) {
    Ok(pr) => {}
    Err(e) => { return 0 - 1; }
  }
  if p.request_head().is_some() || p.head().is_some() || p.in_body() {
    return 0 - 2;
  }

  // 2. ヘッダーの途中
  let part2 = "Host: example.com\r\nAuthorization: Bearer secret-token\r\n".as_bytes();
  match p.feed(&part2, 0) {
    Ok(pr) => {}
    Err(e) => { return 0 - 4; }
  }
  if p.request_head().is_some() || p.head().is_some() || p.in_body() {
    return 0 - 5;
  }

  // 3. ヘッダー終了（CRLF の空行）
  let part3 = "Content-Length: 5\r\n\r\n".as_bytes();
  match p.feed(&part3, 0) {
    Ok(pr) => {}
    Err(e) => { return 0 - 6; }
  }
  if !p.in_body() {
    return 0 - 7;
  }

  // 見出し検証完了: request_head() と head() が Some を返す
  match p.request_head() {
    Some(head) => {
      if head.method() != "POST" {
        return 0 - 9;
      }
      if head.path() != "/api/v1/resource".as_bytes() {
        return 0 - 10;
      }
      if head.query() != "action=create".as_bytes() {
        return 0 - 11;
      }
      if head.version() != 11 {
        return 0 - 12;
      }
      if head.headers().len() != 3 {
        return 0 - 13;
      }

      // 4. 本文を消費して完了
      let part4 = "hello".as_bytes();
      match p.feed(&part4, 0) {
        Ok(pr) => {
          match pr {
            HttpProgress.Done(req, next) => {
              let req_head = req.head();
              if req_head.method() != head.method() {
                return 0 - 14;
              }
              if req_head.path() != head.path() {
                return 0 - 15;
              }
              if req_head.query() != head.query() {
                return 0 - 16;
              }
              if req_head.version() != head.version() {
                return 0 - 17;
              }
              if req_head.headers().len() != head.headers().len() {
                return 0 - 18;
              }
            }
            HttpProgress.NeedMore => { return 0 - 19; }
          }
        }
        Err(e) => { return 0 - 20; }
      }
    }
    None => { return 0 - 8; }
  }

  return 100;"#,
        "none",
    );
    assert_eq!(value(&src), "100");
}

/// API が返すヘッダーがパーサーの検証済みの内容と一致することの確認
#[test]
fn verified_request_head_headers_match_parser_content() {
    let src = prog(
        "",
        r#"  let lim = http.default_limits();
  let mut p = http.parser(&lim);
  let text = "POST /submit?debug=true HTTP/1.1\r\nHost:   api.example.com  \r\nUser-Agent: test-client/1.0\r\nX-Custom-Header: custom_value_123\r\nContent-Length: 4\r\n\r\ntest".as_bytes();

  match p.feed(&text, 0) {
    Ok(pr) => {
      match pr {
        HttpProgress.Done(req, next) => {
          let req_head = req.head();
          let parser_headers = req.headers();
          let head_headers = req_head.headers();

          // 件数の一致
          if head_headers.len() != parser_headers.len() {
            return 0 - 1;
          }
          if head_headers.len() != 4 {
            return 0 - 2;
          }

          // 各ヘッダーの名前・値・順序が完全に一致することを確認
          let mut i = 0;
          while i < parser_headers.len() {
            match parser_headers.get(i) {
              Some(ph) => {
                match head_headers.get(i) {
                  Some(hh) => {
                    if ph.name() != hh.name() {
                      return 0 - 10 - i;
                    }
                    if ph.value() != hh.value() {
                      return 0 - 20 - i;
                    }
                  }
                  None => { return 0 - 25; }
                }
              }
              None => { return 0 - 26; }
            }
            i = i + 1;
          }

          // 個別ヘッダーの取得（大文字小文字の不区別）
          match req_head.header(&"HOST") {
            Some(val) => {
              // パーサーにより前後の OWS は除去されている
              if val != "api.example.com".as_bytes() {
                return 0 - 30;
              }
              match req.header(&"host") {
                Some(req_val) => {
                  if val != req_val {
                    return 0 - 31;
                  }
                }
                None => { return 0 - 32; }
              }
            }
            None => { return 0 - 33; }
          }

          match req_head.header(&"x-custom-header") {
            Some(val) => {
              if val != "custom_value_123".as_bytes() {
                return 0 - 34;
              }
              match req.header(&"X-Custom-Header") {
                Some(req_val) => {
                  if val != req_val {
                    return 0 - 35;
                  }
                }
                None => { return 0 - 36; }
              }
            }
            None => { return 0 - 37; }
          }

          // request_head() と head() の等価性
          let req_head2 = req.request_head();
          if req_head2.method() != req_head.method() {
            return 0 - 40;
          }
          if req_head2.headers().len() != req_head.headers().len() {
            return 0 - 41;
          }
        }
        HttpProgress.NeedMore => { return 0 - 98; }
      }
    }
    Err(e) => { return 0 - 99; }
  }

  return 200;"#,
        "none",
    );
    assert_eq!(value(&src), "200");
}

/// 本文なし要求（GET / OPTIONS *）および chunked 要求での検証
#[test]
fn verified_request_head_get_options_chunked() {
    let src = prog(
        "",
        r#"  let lim = http.default_limits();

  // 1. GET 要求（本文なし、即 Done）
  let mut p1 = http.parser(&lim);
  let get_data = "GET /items?category=books&sort=asc HTTP/1.1\r\nHost: store.test\r\nAccept: application/json\r\n\r\n".as_bytes();
  match p1.feed(&get_data, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.method() != "GET" { return 0 - 1; }
      if head.path() != "/items".as_bytes() { return 0 - 2; }
      if head.query() != "category=books&sort=asc".as_bytes() { return 0 - 3; }
      if head.version() != 11 { return 0 - 4; }
      if head.headers().len() != 2 { return 0 - 5; }
      match head.header(&"host") {
        Some(v) => { if v != "store.test".as_bytes() { return 0 - 6; } }
        None => { return 0 - 60; }
      }
      match head.header(&"accept") {
        Some(v) => { if v != "application/json".as_bytes() { return 0 - 7; } }
        None => { return 0 - 70; }
      }

      // ワイヤ形式の bytes() の確認
      let wire = head.bytes();
      let mut p_check = http.parser(&lim);
      match p_check.feed(&wire, 0) {
        Ok(HttpProgress.Done(req_wire, _)) => {
          if req_wire.method() != head.method() { return 0 - 8; }
          if req_wire.path() != head.path() { return 0 - 9; }
          if req_wire.query() != head.query() { return 0 - 10; }
          if req_wire.headers().len() != head.headers().len() { return 0 - 11; }
        }
        _ => { return 0 - 12; }
      }
    }
    _ => { return 0 - 13; }
  }

  // 2. OPTIONS * 要求
  let mut p2 = http.parser(&lim);
  let opt_data = "OPTIONS * HTTP/1.1\r\nHost: server.test\r\n\r\n".as_bytes();
  match p2.feed(&opt_data, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.method() != "OPTIONS" { return 0 - 20; }
      if head.path() != "*".as_bytes() { return 0 - 21; }
      if head.query().byte_len() != 0 { return 0 - 22; }
      if head.headers().len() != 1 { return 0 - 23; }
    }
    _ => { return 0 - 24; }
  }

  // 3. 分割転送（chunked）でのヘッダー取得
  let mut p3 = http.parser(&lim);
  let chunked_head = "POST /stream HTTP/1.1\r\nHost: upload.test\r\nTransfer-Encoding: chunked\r\n\r\n".as_bytes();
  match p3.feed(&chunked_head, 0) {
    Ok(HttpProgress.NeedMore) => {
      if !p3.in_body() { return 0 - 30; }
      match p3.request_head() {
        Some(head) => {
          if head.method() != "POST" { return 0 - 31; }
          if head.headers().len() != 2 { return 0 - 32; }
          match head.header(&"transfer-encoding") {
            Some(v) => { if v != "chunked".as_bytes() { return 0 - 33; } }
            None => { return 0 - 330; }
          }
        }
        None => { return 0 - 34; }
      }
    }
    _ => { return 0 - 340; }
  }

  let chunked_body = "5\r\nhello\r\n0\r\n\r\n".as_bytes();
  match p3.feed(&chunked_body, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.method() != "POST" { return 0 - 35; }
      if req.body() != "hello".as_bytes() { return 0 - 36; }
    }
    _ => { return 0 - 37; }
  }

  return 300;"#,
        "none",
    );
    assert_eq!(value(&src), "300");
}

/// 空クエリ（`?` だけ）とクエリなしを区別し、元の request-target および `head.bytes()` で `?` が保持されることの確認（ADR 0032 契約準拠）
#[test]
fn verified_request_head_empty_query_preserves_target() {
    let src = prog(
        "",
        r#"  let lim = http.default_limits();

  // 1. 空クエリ（/items?）: ? が失われず head.target() および head.bytes() に保持される
  let mut p1 = http.parser(&lim);
  let empty_q = "GET /items? HTTP/1.1\r\nHost: example.com\r\n\r\n".as_bytes();
  match p1.feed(&empty_q, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.target() != "/items?".as_bytes() { return 0 - 1; }
      if head.path() != "/items".as_bytes() { return 0 - 2; }
      if head.query().byte_len() != 0 { return 0 - 3; }

      let wire = head.bytes();
      let mut p1_check = http.parser(&lim);
      match p1_check.feed(&wire, 0) {
        Ok(HttpProgress.Done(req_wire, _)) => {
          let head_wire = req_wire.head();
          if head_wire.target() != "/items?".as_bytes() { return 0 - 4; }
          if head_wire.path() != "/items".as_bytes() { return 0 - 5; }
          if head_wire.query().byte_len() != 0 { return 0 - 6; }
        }
        _ => { return 0 - 7; }
      }
    }
    _ => { return 0 - 8; }
  }

  // 2. クエリなし（/items）: ? は付与されず head.target() および head.bytes() に保持される
  let mut p2 = http.parser(&lim);
  let no_q = "GET /items HTTP/1.1\r\nHost: example.com\r\n\r\n".as_bytes();
  match p2.feed(&no_q, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.target() != "/items".as_bytes() { return 0 - 10; }
      if head.path() != "/items".as_bytes() { return 0 - 11; }
      if head.query().byte_len() != 0 { return 0 - 12; }

      let wire = head.bytes();
      let mut p2_check = http.parser(&lim);
      match p2_check.feed(&wire, 0) {
        Ok(HttpProgress.Done(req_wire, _)) => {
          let head_wire = req_wire.head();
          if head_wire.target() != "/items".as_bytes() { return 0 - 13; }
          if head_wire.path() != "/items".as_bytes() { return 0 - 14; }
          if head_wire.query().byte_len() != 0 { return 0 - 15; }
        }
        _ => { return 0 - 16; }
      }
    }
    _ => { return 0 - 17; }
  }

  // 3. 通常クエリ（/items?k=v）
  let mut p3 = http.parser(&lim);
  let has_q = "GET /items?k=v HTTP/1.1\r\nHost: example.com\r\n\r\n".as_bytes();
  match p3.feed(&has_q, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.target() != "/items?k=v".as_bytes() { return 0 - 20; }
      if head.path() != "/items".as_bytes() { return 0 - 21; }
      if head.query() != "k=v".as_bytes() { return 0 - 22; }

      let wire = head.bytes();
      let mut p3_check = http.parser(&lim);
      match p3_check.feed(&wire, 0) {
        Ok(HttpProgress.Done(req_wire, _)) => {
          let head_wire = req_wire.head();
          if head_wire.target() != "/items?k=v".as_bytes() { return 0 - 23; }
          if head_wire.path() != "/items".as_bytes() { return 0 - 24; }
          if head_wire.query() != "k=v".as_bytes() { return 0 - 25; }
        }
        _ => { return 0 - 26; }
      }
    }
    _ => { return 0 - 27; }
  }

  // 4. OPTIONS * 要求の target()
  let mut p4 = http.parser(&lim);
  let opt_q = "OPTIONS * HTTP/1.1\r\nHost: example.com\r\n\r\n".as_bytes();
  match p4.feed(&opt_q, 0) {
    Ok(HttpProgress.Done(req, _)) => {
      let head = req.head();
      if head.target() != "*".as_bytes() { return 0 - 30; }
      if head.path() != "*".as_bytes() { return 0 - 31; }

      let wire = head.bytes();
      let mut p4_check = http.parser(&lim);
      match p4_check.feed(&wire, 0) {
        Ok(HttpProgress.Done(req_wire, _)) => {
          let head_wire = req_wire.head();
          if head_wire.target() != "*".as_bytes() { return 0 - 32; }
        }
        _ => { return 0 - 33; }
      }
    }
    _ => { return 0 - 34; }
  }

  return 400;"#,
        "none",
    );
    assert_eq!(value(&src), "400");
}

//! ADR 0037 段階 6-6a: 境界の入口（壁時計・ヘッダ全件数・差し込み口検査）。
mod common;

use sigil::exec::{Arg, Options};

// 1. 実壁時計の取得（評価器とネイティブの両方で実 OS の clock_gettime を実行）
#[test]
fn clock_unix_seconds_eval_and_native_real_clock() {
    let source = r#"
module test.clock
  targets server
  trust none

fn check_real_clock() -> Bool
  intent "OS壁時計を取得し妥当なUnix秒範囲であることを確認する"
  requires none
  ensures none
  effects clock
{
  match clock.unix_seconds() {
    Some(sec) => {
      // 2023年（1_700_000_000）以降かつ 9999年末（253402300799）以内
      return sec >= 1700000000 && sec <= 253402300799;
    }
    None => {
      return false;
    }
  }
}
"#;
    let outcome = common::run(source, "check_real_clock", &[]).unwrap();
    assert_eq!(outcome.value, "true");
}

// 2. 効果の完全一致（effects clock がない関数の呼び出し拒否）
#[test]
fn clock_effects_checked_at_type_check_time() {
    let bad_source = r#"
module test.no_clock
  targets server
  trust none

fn bad_call() -> Option<Int>
  intent "effects clock なしで clock.unix_seconds を呼ぶ"
  requires none
  ensures none
  effects none
{
  return clock.unix_seconds();
}
"#;
    let err = sigil::check::check(bad_source).unwrap_err();
    assert_eq!(err.kind, sigil::ErrorKind::EffectMissing);

    let good_source = r#"
module test.good_clock
  targets server
  trust none

fn good_call() -> Option<Int>
  intent "effects clock を明示して clock.unix_seconds を呼ぶ"
  requires none
  ensures none
  effects clock
{
  return clock.unix_seconds();
}
"#;
    assert!(sigil::check::check(good_source).is_ok());
}

// 3. 評価器への明示的な固定時刻・失敗の注入（0..=253402300799 の範囲検査と失敗値の検査）
#[test]
fn evaluator_wall_clock_injection_bounds_and_failure() {
    let source = r#"
module test.inject
  targets server
  trust none

fn probe_clock() -> Int
  intent "注入された壁時計を取得し、None の場合は -999 を返す"
  requires none
  ensures none
  effects clock
{
  match clock.unix_seconds() {
    Some(sec) => { return sec; }
    None => { return -999; }
  }
}
"#;

    // 正常値: 1_700_000_000
    let mut opts = Options::captured();
    opts.wall_clock = Some(Ok(1700000000));
    let outcome = sigil::ir_eval::run_with(source, "probe_clock", &[], &opts).unwrap();
    assert_eq!(outcome.value, "1700000000");

    // 正常上限: 253402300799 (9999-12-31T23:59:59Z)
    opts.wall_clock = Some(Ok(253402300799));
    let outcome = sigil::ir_eval::run_with(source, "probe_clock", &[], &opts).unwrap();
    assert_eq!(outcome.value, "253402300799");

    // 範囲外（上限超過）: 253402300800 -> None (-999)
    opts.wall_clock = Some(Ok(253402300800));
    let outcome = sigil::ir_eval::run_with(source, "probe_clock", &[], &opts).unwrap();
    assert_eq!(outcome.value, "-999");

    // 範囲外（負値）: -1 -> None (-999)
    opts.wall_clock = Some(Ok(-1));
    let outcome = sigil::ir_eval::run_with(source, "probe_clock", &[], &opts).unwrap();
    assert_eq!(outcome.value, "-999");

    // OS エラー注入: Err(1) (EPERM) -> None (-999)
    // 0 や推測値で成功させず、失敗値 None とすることを確認
    opts.wall_clock = Some(Err(1));
    let outcome = sigil::ir_eval::run_with(source, "probe_clock", &[], &opts).unwrap();
    assert_eq!(outcome.value, "-999");
}

// 4. ネイティブと評価器の契約一致（範囲検査の共通挙動）
#[test]
fn native_and_eval_injected_clock_consistency() {
    let source = r#"
module test.consistency
  targets server
  trust none

fn check_bounds(sec: Int) -> Bool
  intent "Unix秒が 0..=253402300799 の範囲内か判定する"
  requires none
  ensures none
  effects none
{
  return sec >= 0 && sec <= 253402300799;
}
"#;
    for (sec, want) in [
        (0, "true"),
        (1700000000, "true"),
        (253402300799, "true"),
        (253402300800, "false"),
        (-1, "false"),
    ] {
        let outcome = common::run(source, "check_bounds", &[Arg::Int(sec)]).unwrap();
        assert_eq!(outcome.value, want);
    }
}

// 5. 符号付き Int 組み立て前の時刻範囲（iat・exp は 0..=253402300799）と桁あふれ検査
#[test]
fn parse_timestamp_be_bounds_and_overflow_protection() {
    let source = r#"
module test.parse_ts
  targets server
  trust none

fn parse_ts(data: &Bytes, offset: Int) -> Option<Int>
  intent "8バイト大端タイムスタンプを解釈し、0..=253402300799 の範囲内のみ返す"
  requires none
  ensures none
  effects none
{
  if offset < 0 || offset + 8 > data.len() {
    return None;
  }
  let b0 = data.get(offset)?;
  let b1 = data.get(offset + 1)?;
  let b2 = data.get(offset + 2)?;
  let b3 = data.get(offset + 3)?;
  let b4 = data.get(offset + 4)?;
  let b5 = data.get(offset + 5)?;
  let b6 = data.get(offset + 6)?;
  let b7 = data.get(offset + 7)?;

  // 上位 3 バイトは必ず 0 でなければならない（0 以外は 253402300799 超または符号あふれ）
  if b0 != 0 || b1 != 0 || b2 != 0 {
    return None;
  }
  // 253402300799 = 0x3a_ff_f4_41_7f
  // b3 は最大 0x3a (58)
  if b3 > 58 {
    return None;
  }
  if b3 == 58 {
    // 下位4バイトは最大 0xfff4417f
    if b5 > 244 {
      return None;
    }
    if b5 == 244 {
      if b6 > 65 {
        return None;
      }
      if b6 == 65 && b7 > 127 {
        return None;
      }
    }
  }
  let high = b3 * 4294967296;
  let mid = b4 * 16777216 + b5 * 65536;
  let low = b6 * 256 + b7;
  let ts = high + mid + low;
  if ts < 0 || ts > 253402300799 {
    return None;
  }
  return Some(ts);
}

fn test_driver(mode: Int) -> Int
  intent "各種タイムスタンプパターンの試験ドライバ"
  requires none
  ensures none
  effects none
{
  let mut b = Bytes.new();
  if mode == 1 {
    // 正常値: 1700000000 = 0x6553f100 -> [0, 0, 0, 0, 101, 83, 241, 0]
    b.push(0); b.push(0); b.push(0); b.push(0);
    b.push(101); b.push(83); b.push(241); b.push(0);
    return parse_ts(&b, 0).unwrap_or(-1);
  } else if mode == 2 {
    // 正常上限: 253402300799 = 0x3afff4417f -> [0, 0, 0, 58, 255, 244, 65, 127]
    b.push(0); b.push(0); b.push(0); b.push(58);
    b.push(255); b.push(244); b.push(65); b.push(127);
    match parse_ts(&b, 0) {
      Some(v) => {
        if v == 253402300799 { return 1; }
        return 0;
      }
      None => { return -1; }
    }
  } else if mode == 3 {
    // 上限超過 (+1): 253402300800 -> [0, 0, 0, 58, 255, 244, 65, 128]
    b.push(0); b.push(0); b.push(0); b.push(58);
    b.push(255); b.push(244); b.push(65); b.push(128);
    return parse_ts(&b, 0).unwrap_or(-99);
  } else if mode == 4 {
    // 符号付き桁あふれ（最上位ビットセット）: [128, 0, 0, 0, 0, 0, 0, 0]
    b.push(128); b.push(0); b.push(0); b.push(0);
    b.push(0); b.push(0); b.push(0); b.push(0);
    return parse_ts(&b, 0).unwrap_or(-99);
  } else if mode == 5 {
    // バイト不足（4バイトのみ）
    b.push(0); b.push(0); b.push(0); b.push(1);
    return parse_ts(&b, 0).unwrap_or(-99);
  }
  return -1;
}
"#;

    assert_eq!(
        common::run(source, "test_driver", &[Arg::Int(1)])
            .unwrap()
            .value,
        "1700000000"
    );
    assert_eq!(
        common::run(source, "test_driver", &[Arg::Int(2)])
            .unwrap()
            .value,
        "1"
    );
    assert_eq!(
        common::run(source, "test_driver", &[Arg::Int(3)])
            .unwrap()
            .value,
        "-99"
    );
    assert_eq!(
        common::run(source, "test_driver", &[Arg::Int(4)])
            .unwrap()
            .value,
        "-99"
    );
    assert_eq!(
        common::run(source, "test_driver", &[Arg::Int(5)])
            .unwrap()
            .value,
        "-99"
    );
}

// 6. AppRequest.header_count によるヘッダ全件数確認と大文字小文字を無視した重複検査
#[test]
fn app_request_header_count_and_duplicate_detection() {
    let source = r#"
module test.headers
  targets server
  trust none

fn handle_count(req: &AppRequest) -> HttpResponse
  intent "ヘッダ件数を返す"
  requires none
  ensures none
  effects none
{
  let count = req.header_count(&"authorization");
  if count == 0 {
    return HttpResponse.new(200, "0".as_bytes());
  } else if count == 1 {
    return HttpResponse.new(200, "1".as_bytes());
  } else if count == 2 {
    return HttpResponse.new(200, "2".as_bytes());
  }
  return HttpResponse.new(200, "other".as_bytes());
}

fn count_auth(request_wire: String) -> Int
  intent "リクエストから Authorization ヘッダーの全出現件数を取得する"
  requires none
  ensures none
  effects clock, console, db, env, random, task
{
  let limits = http.default_limits();
  let mut parser = http.parser(&limits);
  match parser.feed(&request_wire.as_bytes(), 0) {
    Ok(progress) => {
      match progress {
        HttpProgress.Done(http_req, _) => {
          let mut r = Router.new();
          match r.public_route("GET", "/test", RouteHandler { handle: handle_count }) {
            Ok(router) => {
              r = router;
            }
            Err(_) => {
              return -3;
            }
          }

          let res = r.dispatch(&http_req);
          match res.body().to_string_utf8() {
            Ok(s) => {
              return s.parse_int().unwrap_or(-5);
            }
            Err(_) => {
              return -6;
            }
          }
        }
        HttpProgress.NeedMore => {
          return -2;
        }
      }
    }
    Err(_) => {
      return -1;
    }
  }
  return -4;
}
"#;

    // 0 件
    let wire0 = "GET /test HTTP/1.1\r\nHost: local\r\n\r\n";
    assert_eq!(
        common::run(source, "count_auth", &[Arg::Str(wire0.to_string())])
            .unwrap()
            .value,
        "0"
    );

    // 1 件（小文字）
    let wire1 = "GET /test HTTP/1.1\r\nHost: local\r\nauthorization: Bearer token\r\n\r\n";
    assert_eq!(
        common::run(source, "count_auth", &[Arg::Str(wire1.to_string())])
            .unwrap()
            .value,
        "1"
    );

    // 2 件（大文字小文字混合、重複）
    let wire2 = "GET /test HTTP/1.1\r\nHost: local\r\nAuthorization: Bearer token1\r\nAUTHORIZATION: Bearer token2\r\n\r\n";
    assert_eq!(
        common::run(source, "count_auth", &[Arg::Str(wire2.to_string())])
            .unwrap()
            .value,
        "2"
    );
}

// 7. AuthVerifier 差し込み口と型（fn(&AppRequest) -> Result<AuthIdentity, HttpResponse>）の検査
#[test]
fn auth_verifier_slot_and_type_check() {
    let source = r#"
module test.verifier
  targets server
  trust none

fn verify_token(req: &AppRequest) -> Result<AuthIdentity, HttpResponse>
  intent "AuthVerifier の差し込み口関数: ヘッダ全件数確認と検証"
  requires none
  ensures none
  effects none
{
  // 重複検査（ADR 0037 決定 3: 0件も2件以上も失敗）
  let count = req.header_count(&"authorization");
  if count != 1 {
    return Err(HttpResponse.new(401, "unauthorized".as_bytes()));
  }
  let mut auth = Bytes.new();
  match req.header(&"authorization") {
    Some(val) => {
      auth = val;
    }
    None => {
      return Err(HttpResponse.new(401, "unauthorized".as_bytes()));
    }
  }
  if auth == "Bearer valid-token".as_bytes() {
    return Ok(AuthIdentity.new("user-42"));
  }
  return Err(HttpResponse.new(401, "unauthorized".as_bytes()));
}

fn handle_protected(req: &AppRequest) -> HttpResponse
  intent "保護経路のハンドラ"
  requires none
  ensures none
  effects none
{
  let id = match req.identity() {
    Some(i) => i.subject(),
    None => "none",
  };
  return HttpResponse.new(200, id.as_bytes());
}

fn test_router(wire: String) -> Int
  intent "ルーターに AuthVerifier を設定してディスパッチする"
  requires none
  ensures none
  effects clock, console, db, env, random, task
{
  let limits = http.default_limits();
  let mut parser = http.parser(&limits);
  match parser.feed(&wire.as_bytes(), 0) {
    Ok(progress) => {
      match progress {
        HttpProgress.Done(http_req, _) => {
          let verifier = AuthVerifier { verify: verify_token };
          let mut r = Router.new();
          r = r.with_auth(verifier);
          match r.route("GET", "/protected", RouteHandler { handle: handle_protected }) {
            Ok(router) => {
              r = router;
            }
            Err(_) => {
              return -3;
            }
          }

          let res = r.dispatch(&http_req);
          return res.status();
        }
        HttpProgress.NeedMore => {
          return -2;
        }
      }
    }
    Err(_) => {
      return -1;
    }
  }
  return -4;
}
"#;

    // 正常トークン: 200
    let wire_ok =
        "GET /protected HTTP/1.1\r\nHost: local\r\nAuthorization: Bearer valid-token\r\n\r\n";
    assert_eq!(
        common::run(source, "test_router", &[Arg::Str(wire_ok.to_string())])
            .unwrap()
            .value,
        "200"
    );

    // 不正トークン: 401
    let wire_bad =
        "GET /protected HTTP/1.1\r\nHost: local\r\nAuthorization: Bearer wrong-token\r\n\r\n";
    assert_eq!(
        common::run(source, "test_router", &[Arg::Str(wire_bad.to_string())])
            .unwrap()
            .value,
        "401"
    );

    // 欠落（0件）: 401
    let wire_missing = "GET /protected HTTP/1.1\r\nHost: local\r\n\r\n";
    assert_eq!(
        common::run(source, "test_router", &[Arg::Str(wire_missing.to_string())])
            .unwrap()
            .value,
        "401"
    );

    // 重複（2件）: 401
    let wire_dup = "GET /protected HTTP/1.1\r\nHost: local\r\nAuthorization: Bearer valid-token\r\nauthorization: Bearer valid-token\r\n\r\n";
    assert_eq!(
        common::run(source, "test_router", &[Arg::Str(wire_dup.to_string())])
            .unwrap()
            .value,
        "401"
    );
}

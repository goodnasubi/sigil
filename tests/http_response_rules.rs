//! ADR 0032 決定 5-2 / PR 6-3g-c: 応答の組み立ての規則 S1〜S7（`serve_conn` が応答を書く前に確かめる）。
//!
//! 同じバイト列を評価器・ネイティブ（ワーカー 1・4）のサーバーに送り、応答のバイト列が一致することと、期待どおりであることを確かめる。
//! 相手が持続接続で 2 件目の応答を正しく区切れることを、204・205・304・`HEAD` の後に続けて確かめる。
//! ハンドラの誤り（S1〜S5 を破る応答）は、500（`Connection: close`、本文の長さ 0）を書いて接続を閉じ、ルートを異常停止にする。
#![allow(non_snake_case)]
mod common;
#[path = "common/serve.rs"]
mod serve;

use serve::*;
use std::sync::OnceLock;

fn prog() -> &'static Prog {
    static P: OnceLock<Prog> = OnceLock::new();
    P.get_or_init(|| Prog::server("http-resp-rules"))
}

/// 3 つの動かし方で同じバイト列を送り、応答が一致することを確かめて返す。
fn all_modes(payload: &str, lim: Lim, step: usize) -> Vec<u8> {
    let mut seen: Vec<Vec<u8>> = vec![];
    for mode in MODES {
        let (got, d) = prog().serve(mode, 1, 2, lim, |port, _| {
            exchange(port, payload.as_bytes(), step)
        });
        clean(mode, &d, "1");
        seen.push(got);
    }
    assert!(
        seen[0] == seen[1] && seen[0] == seen[2],
        "評価器とネイティブの応答が違う:\n{}\n---\n{}\n---\n{}",
        text(&seen[0]),
        text(&seen[1]),
        text(&seen[2])
    );
    seen.remove(0)
}

const OK200: &str = "HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello";
const E500: &str =
    "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

#[test]
fn rule_S7_other_status_has_content_length_even_when_empty() {
    let got = all_modes(
        &format!(
            "{}{}",
            req("GET", "/s200e", "", ""),
            req("GET", "/s200", "", "")
        ),
        Lim::default(),
        0,
    );
    assert_eq!(
        text(&got),
        format!("HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n{OK200}")
    );
}

#[test]
fn rule_S3_204_has_no_body_and_no_content_length() {
    let got = all_modes(
        &format!(
            "{}{}",
            req("GET", "/s204", "", ""),
            req("GET", "/s200", "", "")
        ),
        Lim::default(),
        0,
    );
    assert_eq!(
        text(&got),
        format!("HTTP/1.1 204 No Content\r\n\r\n{OK200}")
    );
}

#[test]
fn rule_S3_204_with_body_is_a_handler_fault() {
    let got = all_modes(&req("GET", "/s204b", "", ""), Lim::default(), 0);
    assert_eq!(text(&got), E500);
}

#[test]
fn rule_S4_205_has_content_length_zero() {
    let got = all_modes(
        &format!(
            "{}{}",
            req("GET", "/s205", "", ""),
            req("GET", "/s200", "", "")
        ),
        Lim::default(),
        0,
    );
    assert_eq!(
        text(&got),
        format!("HTTP/1.1 205 Reset Content\r\ncontent-length: 0\r\n\r\n{OK200}")
    );
}

#[test]
fn rule_S4_205_with_body_is_a_handler_fault() {
    let got = all_modes(&req("GET", "/s205b", "", ""), Lim::default(), 0);
    assert_eq!(text(&got), E500);
}

#[test]
fn rule_S5_304_has_no_body_and_no_content_length() {
    let got = all_modes(
        &format!(
            "{}{}",
            req("GET", "/s304", "", ""),
            req("GET", "/s200", "", "")
        ),
        Lim::default(),
        0,
    );
    assert_eq!(
        text(&got),
        format!("HTTP/1.1 304 Not Modified\r\n\r\n{OK200}")
    );
}

#[test]
fn rule_S5_304_with_body_is_a_handler_fault() {
    let got = all_modes(&req("GET", "/s304b", "", ""), Lim::default(), 0);
    assert_eq!(text(&got), E500);
}

#[test]
fn rule_S6_head_sends_the_length_but_no_body() {
    let got = all_modes(
        &format!(
            "{}{}",
            req("HEAD", "/s200", "", ""),
            req("GET", "/s200", "", "")
        ),
        Lim::default(),
        0,
    );
    assert_eq!(
        text(&got),
        format!("HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\n{OK200}")
    );
}

#[test]
fn rule_S6_head_with_204_follows_the_204_rule() {
    let got = all_modes(
        &format!(
            "{}{}",
            req("HEAD", "/s204", "", ""),
            req("GET", "/s200", "", "")
        ),
        Lim::default(),
        0,
    );
    assert_eq!(
        text(&got),
        format!("HTTP/1.1 204 No Content\r\n\r\n{OK200}")
    );
}

#[test]
fn rule_S1_handler_cannot_set_framing_headers() {
    for path in ["/cl", "/te", "/conn"] {
        let got = all_modes(&req("GET", path, "", ""), Lim::default(), 0);
        assert_eq!(text(&got), E500, "{path}");
    }
}

#[test]
fn rule_S1_header_names_and_values_are_checked() {
    for path in ["/inj", "/injlf", "/badname", "/emptyname"] {
        let got = all_modes(&req("GET", path, "", ""), Lim::default(), 0);
        assert_eq!(text(&got), E500, "{path}");
    }
    let got = all_modes(&req("GET", "/hdr", "", ""), Lim::default(), 0);
    assert_eq!(
        text(&got),
        "HTTP/1.1 200 OK\r\nx-a: v\r\ncontent-length: 2\r\n\r\nok"
    );
}

#[test]
fn rule_S2_library_errors_use_the_same_framing() {
    // 解釈の失敗の応答も Content-Length: 0 と Connection: close を付ける。
    let got = all_modes("BAD\r\n\r\n", Lim::default(), 0);
    assert_eq!(
        text(&got),
        "HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
}

/// 応答の分け方（送る位置）に依らず、応答の連結は同じ。
#[test]
fn responses_do_not_depend_on_how_the_requests_are_split() {
    let payload = format!(
        "{}{}{}{}",
        req("GET", "/s204", "", ""),
        req("HEAD", "/s200", "", ""),
        req("POST", "/echo", "Content-Length: 3\r\n", "abc"),
        req("GET", "/s304", "", "")
    );
    let whole = all_modes(&payload, Lim::default(), 0);
    let bytewise = all_modes(&payload, Lim::default(), 1);
    assert_eq!(whole, bytewise);
    assert_eq!(statuses(&whole), vec![204, 200, 200, 304]);
}

//! ADR 0032 決定 5 の R1〜R12（要求の密輸の防止）と決定 1 の範囲の、規則ごとの試験（PR 6-3g-b）。
//!
//! 規則ごとに、拒まれる入力と、境目で通る入力を組にする。どの入力も `tests/common/http.rs` の `probe_all` で、
//! 全体・1 バイトずつ・分けて与えた結果が同じこと、消費・保持・往復の性質も確かめる（決定 7 の性質）。
#![allow(non_snake_case)]

#[path = "common/http.rs"]
mod http;

use http::{probe_all, requests, status, Probe};

/// 入力を既定の上限と既定の分け方でまとめて確かめる。
fn run(inputs: &[&[u8]]) -> Vec<Probe> {
    let cases: Vec<(Vec<u8>, Vec<i64>, Vec<i64>)> = inputs
        .iter()
        .map(|b| {
            let n = b.len() as i64;
            (b.to_vec(), vec![1, n / 3, n / 2, n - 1], vec![])
        })
        .collect();
    probe_all(&cases)
}

/// `rejected` はどれも `code` で拒まれ、`passed` はどれも 1 件の要求として通る。
fn pair(rejected: &[(&[u8], u32)], passed: &[&[u8]]) -> Vec<Probe> {
    let mut all: Vec<&[u8]> = rejected.iter().map(|(b, _)| *b).collect();
    all.extend(passed.iter().copied());
    let got = run(&all);
    for (i, (b, code)) in rejected.iter().enumerate() {
        assert_eq!(
            status(&got[i]),
            Some(*code),
            "{:?} は {code} で拒まれる: {}",
            String::from_utf8_lossy(b),
            got[i].whole
        );
    }
    for (j, b) in passed.iter().enumerate() {
        let p = &got[rejected.len() + j];
        assert_eq!(
            (status(p), requests(p).len()),
            (None, 1),
            "{:?} は通る: {}",
            String::from_utf8_lossy(b),
            p.whole
        );
    }
    got
}

#[test]
fn rule_R1_cl_and_te() {
    pair(
        &[
            (b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nContent-Length: 3\r\n\r\nabc", 400),
        ],
        &[
            b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\n\r\nabc",
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ],
    );
}

#[test]
fn rule_R2_duplicate_content_length() {
    pair(
        &[
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nContent-Length: 4\r\n\r\nabcd",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nContent-Length: 3\r\n\r\nabc",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3, 3\r\n\r\nabc",
                400,
            ),
        ],
        &[b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\nX-Length: 3\r\n\r\nabc"],
    );
}

#[test]
fn rule_R3_content_length_digits() {
    let got = pair(
        &[
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: +3\r\n\r\nabc",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: -1\r\n\r\n",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 0x3\r\n\r\nabc",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 3 3\r\n\r\nabc",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length:\r\n\r\n",
                400,
            ),
            (
                b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 99999999999999999999999999\r\n\r\n",
                413,
            ),
        ],
        &[
            b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 003\r\n\r\nabc",
            b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length:  3 \t\r\n\r\nabc",
        ],
    );
    // 先頭の 0 は値として読む（本文は 3 バイト）。
    assert!(
        requests(&got[6])[0].ends_with(";616263"),
        "{}",
        got[6].whole
    );
}

#[test]
fn rule_R4_transfer_encoding_chunked_only() {
    pair(
        &[
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: Chunked\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: CHUNKED\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked \r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding:\tchunked\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding:  chunked\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: gzip, chunked\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding:\r\n\r\n0\r\n\r\n", 400),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: gzip\r\n\r\n0\r\n\r\n", 501),
            (b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: identity\r\n\r\n", 501),
        ],
        &[
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding:chunked\r\n\r\n0\r\n\r\n",
        ],
    );
}

#[test]
fn rule_R5_http10_transfer_encoding() {
    pair(
        &[(b"POST / HTTP/1.0\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n", 400)],
        &[
            b"POST / HTTP/1.0\r\nContent-Length: 3\r\n\r\nabc",
            b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n0\r\n\r\n",
        ],
    );
}

#[test]
fn rule_R6_chunk_size_line() {
    let head = "POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n";
    let c = |s: &str| format!("{head}{s}").into_bytes();
    let (
        lead,
        ext,
        ext_long,
        nine,
        space,
        after,
        lf_after,
        zero_zero,
        empty,
        bare_lf,
        eight,
        upper,
        plain,
    ) = (
        c("03\r\nabc\r\n0\r\n\r\n"),
        c("3;x=1\r\nabc\r\n0\r\n\r\n"),
        c(&format!("3;{}\r\nabc\r\n0\r\n\r\n", "a".repeat(2000))),
        c("100000000\r\n"),
        c("3 \r\nabc\r\n0\r\n\r\n"),
        c("3\r\nabcX\r\n0\r\n\r\n"),
        c("3\r\nabc\n0\r\n\r\n"),
        c("00\r\n\r\n"),
        c("\r\nabc\r\n0\r\n\r\n"),
        c("3\nabc\r\n0\r\n\r\n"),
        c("10000000\r\n"),
        c("A\r\n0123456789\r\n0\r\n\r\n"),
        c("3\r\nabc\r\n0\r\n\r\n"),
    );
    pair(
        &[
            (&lead, 400),
            (&ext, 400),
            (&ext_long, 400),
            (&nine, 400),
            (&space, 400),
            (&after, 400),
            (&lf_after, 400),
            (&zero_zero, 400),
            (&empty, 400),
            (&bare_lf, 400),
            // 8 桁は R6 を通り、本文の上限（1 MiB）で 413 になる（塊の中身は読まない）。
            (&eight, 413),
        ],
        &[&upper, &plain],
    );
}

#[test]
fn rule_R7_obs_fold() {
    pair(
        &[
            (b"GET / HTTP/1.1\r\nHost: a\r\nX-A: 1\r\n 2\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX-A: 1\r\n\t2\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\n Host: a\r\n\r\n", 400),
        ],
        &[b"GET / HTTP/1.1\r\nHost: a\r\nX-A: 1 2\r\n\r\n"],
    );
}

#[test]
fn rule_R8_crlf_only() {
    pair(
        &[
            (b"GET / HTTP/1.1\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\n\n", 400),
            (b"GET / HTTP/1.1\rHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\rX: b\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\n\r\r", 400),
            (b"\r\nGET / HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"\nGET / HTTP/1.1\r\nHost: a\r\n\r\n", 400),
        ],
        &[b"GET / HTTP/1.1\r\nHost: a\r\n\r\n"],
    );
}

#[test]
fn rule_R9_header_name() {
    pair(
        &[
            (b"GET / HTTP/1.1\r\nHost : a\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost\t: a\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX Y: b\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX(Y): b\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\n: b\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nnocolon\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX\xc3\xa9: b\r\n\r\n", 400),
        ],
        &[b"GET / HTTP/1.1\r\nHost: a\r\nX-Weird!#$%&'*+-.^_`|~9: b\r\n\r\n"],
    );
}

#[test]
fn rule_R10_header_value_controls() {
    let got = pair(
        &[
            (b"GET / HTTP/1.1\r\nHost: a\r\nX: a\x00b\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX: a\x01b\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX: a\x7fb\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nX: ab\x1f\r\n\r\n", 400),
        ],
        &[b"GET / HTTP/1.1\r\nHost: a\r\nX: \ta\tb\x80\xff\xc3\xa9\t \r\n\r\n"],
    );
    // 値は前後の OWS を除いた生のバイト列（内側の HTAB と 0x80 以上は残る）。
    assert!(
        requests(&got[4])[0].contains(";78=61096280ffc3a9;"),
        "{}",
        got[4].whole
    );
}

#[test]
fn rule_R11_host_exactly_once() {
    pair(
        &[
            (b"GET / HTTP/1.1\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nhost: b\r\n\r\n", 400),
            (b"GET / HTTP/1.0\r\nHost: a\r\nHost: b\r\n\r\n", 400),
        ],
        &[
            b"GET / HTTP/1.1\r\nhOsT: a\r\n\r\n",
            b"GET / HTTP/1.0\r\n\r\n",
        ],
    );
}

#[test]
fn rule_R12_request_line() {
    pair(
        &[
            (b"GET  / HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET /  HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET\t/ HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1.1 \r\nHost: a\r\n\r\n", 400),
            (b" GET / HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET /a\x80 HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET /a\x01 HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET /a\x7f HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET /a\tb HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"G(T / HTTP/1.1\r\nHost: a\r\n\r\n", 400),
        ],
        &[b"GET /a-._~!$&'()*+,;=:@%20/?x=1&y=/?#z HTTP/1.1\r\nHost: a\r\n\r\n"],
    );
}

/// 決定 1 の範囲: 版・対象の形・メソッド・`Expect`。
#[test]
fn decision1_scope() {
    let got = pair(
        &[
            (b"GET / HTTP/2.0\r\nHost: a\r\n\r\n", 505),
            (b"GET / HTTP/1.2\r\nHost: a\r\n\r\n", 505),
            (b"GET / HTTP/0.9\r\nHost: a\r\n\r\n", 505),
            (b"GET / http/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1.10\r\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1\r\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/a.b\r\nHost: a\r\n\r\n", 400),
            (b"GET http://a/ HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"CONNECT a:443 HTTP/1.1\r\nHost: a\r\n\r\n", 501),
            (b"TRACE / HTTP/1.1\r\nHost: a\r\n\r\n", 501),
            (b"GET * HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"OPTIONS *a HTTP/1.1\r\nHost: a\r\n\r\n", 400),
            (b"GET / HTTP/1.1\r\nHost: a\r\nExpect: 200-ok\r\n\r\n", 417),
            (
                b"GET / HTTP/1.1\r\nHost: a\r\nExpect: 100-continue\r\nExpect: x\r\n\r\n",
                417,
            ),
        ],
        &[
            b"OPTIONS * HTTP/1.1\r\nHost: a\r\n\r\n",
            b"POST / HTTP/1.1\r\nHost: a\r\nExpect: 100-Continue\r\nContent-Length: 1\r\n\r\nx",
            b"get /x HTTP/1.0\r\n\r\n",
        ],
    );
    assert!(
        requests(&got[14])[0].starts_with("4f5054494f4e53;2a;;11;"),
        "{}",
        got[14].whole
    );
    assert!(
        requests(&got[16])[0].starts_with("676574;2f78;;10;0;"),
        "{}",
        got[16].whole
    );
}

/// パイプライン: 1 回の入力に並んだ要求は順に完成し、読み越しは次の要求の先頭になる（決定 1）。
/// 失敗の後のバイト列は解釈しない（決定 5）。
#[test]
fn pipelined_requests_and_stop_after_error() {
    let got = run(&[
        b"GET /1 HTTP/1.1\r\nHost: a\r\n\r\nPOST /2 HTTP/1.1\r\nHost: a\r\nContent-Length: 2\r\n\r\nhiGET /3 HTTP/1.0\r\n\r\nGET /4",
        b"GET /1 HTTP/1.1\r\nHost: a\r\n\r\nGET /2 HTTP/1.1\r\n\r\nGET /3 HTTP/1.1\r\nHost: a\r\n\r\n",
    ]);
    let r = requests(&got[0]);
    assert_eq!(r.len(), 3, "{}", got[0].whole);
    assert!(got[0].whole.ends_with("|P6"), "{}", got[0].whole);
    assert_eq!(requests(&got[1]).len(), 1, "{}", got[1].whole);
    assert_eq!(status(&got[1]), Some(400), "{}", got[1].whole);
}

/// 分割した転送: 本文は復号した塊の連結。トレーラーは見出しに入らず捨てる（ADR 0020 決定 4）。
#[test]
fn chunked_body_and_trailers_are_dropped() {
    let got = run(&[
        b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\nX-T: 1\r\nContent-Length: 9\r\n\r\n",
        b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nBad Name: 1\r\n\r\n",
        b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n folded\r\n\r\n",
    ]);
    let r = requests(&got[0]);
    assert_eq!(r.len(), 1, "{}", got[0].whole);
    assert!(r[0].ends_with(";6162636465"), "{}", r[0]);
    assert!(!r[0].contains(";782d74="), "{}", r[0]);
    assert_eq!(status(&got[1]), Some(400), "{}", got[1].whole);
    assert_eq!(status(&got[2]), Some(400), "{}", got[2].whole);
}

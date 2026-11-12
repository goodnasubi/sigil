//! ADR 0032 決定 4-1 の解釈の行（PR 6-3g-b）の上限の試験（ADR 0020 決定 5 の完了条件: `limit_http_` で始まる名前）。
//!
//! 上限ちょうどの入力は通り、1 つ超えると決めた動き（状態コード）になることを確かめる。既定値の行は既定値のまま、
//! ほかは同じ入力を上限 L（通る）と L - 1（1 つ超える）で与える（ADR 0032 決定 4-3: 小さい値で上限ちょうどと 1 つ超え）。
//! 接続・時間・応答の行（6-3g-c）はここに無い。

#[path = "common/http.rs"]
mod http;

use http::{probe_all, requests, roots, status, Probe, DEFAULTS};
use sigil::exec::Arg;

/// 同じ入力を、i 番目の上限だけ `at` と `at - 1` にして与える（ほかは既定値か `base`）。
fn at_and_below(input: &[u8], i: usize, at: i64, base: &[i64]) -> (Probe, Probe) {
    let mut lim = vec![0i64; 6];
    for (k, v) in base.iter().enumerate() {
        lim[k] = *v;
    }
    let mut below = lim.clone();
    lim[i] = at;
    below[i] = at - 1;
    let n = input.len() as i64;
    let cuts = vec![1, n / 3, n / 2, n - 1];
    let mut got = probe_all(&[
        (input.to_vec(), cuts.clone(), lim),
        (input.to_vec(), cuts, below),
    ]);
    let b = got.pop().unwrap();
    let a = got.pop().unwrap();
    (a, b)
}

fn passes(p: &Probe) {
    assert_eq!((status(p), requests(p).len()), (None, 1), "{}", p.whole);
}

/// ADR 0020 決定 4 の表（と ADR 0032 決定 4-1 の符号化した本文 2 MiB）の既定値が、`http.default_limits()` の値と同じ。
#[test]
fn limit_http_defaults() {
    let got = roots("limit_values", &[vec![]]);
    assert_eq!(
        got[0].as_deref(),
        Ok("8192,16384,100,1048576,1024,2097152,256,128,10000,30000,30000,30000,60000,1000,1048576")
    );
    assert_eq!(
        &DEFAULTS[..],
        &[8192, 16384, 100, 1_048_576, 1024, 2_097_152]
    );
}

/// 各値は `1 <= v && v <= 上限の上限` で受け、0（無制限の意味になりうる値）・負の値・上限の上限の 1 つ上は契約違反。
#[test]
fn limit_http_setters_reject_unbounded_values() {
    let caps: [i64; 15] = [
        65536,
        262144,
        1000,
        67_108_864,
        1024,
        134_217_728,
        4096,
        1024,
        600_000,
        600_000,
        600_000,
        600_000,
        600_000,
        100_000,
        67_108_864,
    ];
    let mut sets = vec![];
    for (i, c) in caps.iter().enumerate() {
        for v in [1, *c, 0, -1, c + 1] {
            sets.push(vec![Arg::Int(i as i64), Arg::Int(v)]);
        }
    }
    let got = roots("set_limit", &sets);
    for (k, (set, r)) in sets.iter().zip(&got).enumerate() {
        let v = match set[1] {
            Arg::Int(v) => v,
            _ => unreachable!(),
        };
        if k % 5 < 2 {
            assert_eq!(r.as_deref(), Ok(v.to_string().as_str()), "{set:?}");
        } else {
            let e = r.as_ref().expect_err("契約違反になる");
            assert!(e.contains("RequiresViolated"), "{set:?}: {e}");
        }
    }
}

/// 要求の行 8 KiB（CRLF を除く）: ちょうどは通り、対象が長くて超えれば 414、ほかの部分で超えれば 400。
#[test]
fn limit_http_request_line() {
    let line = |target_len: usize| {
        // `GET ` + 対象 + ` HTTP/1.1` = 4 + target_len + 9
        let mut s = b"GET /".to_vec();
        s.extend(std::iter::repeat_n(b'a', target_len - 1));
        s.extend_from_slice(b" HTTP/1.1\r\nHost: a\r\n\r\n");
        s
    };
    let exact = line(8192 - 13);
    // 8193 バイト目が対象の中にある（`GET ` の 4 バイト + 対象 8189 バイト）。
    let over = line(8189);
    let mut long_method = vec![b'G'; 8193];
    long_method.extend_from_slice(b" / HTTP/1.1\r\nHost: a\r\n\r\n");
    let mut long_version = b"GET /".to_vec();
    long_version.extend(std::iter::repeat_n(b'a', 8192 - 13));
    long_version.extend_from_slice(b" HTTP/1.11\r\nHost: a\r\n\r\n");
    let got = probe_all(&[
        (exact.clone(), vec![100, 8000], vec![]),
        (over, vec![100, 8000], vec![]),
        (long_method, vec![100, 8000], vec![]),
        (long_version, vec![100, 8000], vec![]),
    ]);
    passes(&got[0]);
    assert_eq!(status(&got[1]), Some(414), "{}", got[1].whole);
    assert_eq!(status(&got[2]), Some(400), "{}", got[2].whole);
    assert_eq!(status(&got[3]), Some(400), "{}", got[3].whole);
    // 超えたときは、超えたバイトを消費しない（要求の行の 8192 バイトまで）。
    assert!(got[1].whole.contains("E414@8192"), "{}", got[1].whole);
    // 小さい値: 行の 24 バイトちょうどで通り、23 では版の最後のバイトで超えるので 400、10 では対象の中で超えるので 414。
    let small = b"GET /0123456789 HTTP/1.1\r\nHost: a\r\n\r\n";
    let (a, b) = at_and_below(small, 0, 24, &[]);
    passes(&a);
    assert_eq!(status(&b), Some(400), "{}", b.whole);
    let c = http::probe_lim(small, &[10]);
    assert!(c.whole.starts_with("E414@10"), "{}", c.whole);
}

/// 見出しの部分 16 KiB（要求の行を除き、各行の CRLF を含み、終わりの空行を除く）: ちょうどは通り、1 バイト超えると 431。
#[test]
fn limit_http_head() {
    let head = |total: usize| {
        let mut s = b"GET / HTTP/1.1\r\nHost: a\r\n".to_vec();
        // `Host: a\r\n` は 9 バイト。残りを 1 行の見出しで埋める（`X: ` + 値 + CRLF）。
        let rest = total - 9;
        s.extend_from_slice(b"X: ");
        s.extend(std::iter::repeat_n(b'v', rest - 5));
        s.extend_from_slice(b"\r\n\r\n");
        s
    };
    let got = probe_all(&[
        (head(16384), vec![30, 9000], vec![]),
        (head(16385), vec![30, 9000], vec![]),
    ]);
    passes(&got[0]);
    assert_eq!(status(&got[1]), Some(431), "{}", got[1].whole);
    // トレーラーも数える（見出しとトレーラーの和）。
    let t = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nT: 1\r\n\r\n";
    // 見出しの行は 9 + 28 = 37、トレーラーは 6。
    let (a, b) = at_and_below(t, 1, 43, &[]);
    passes(&a);
    assert_eq!(status(&b), Some(431), "{}", b.whole);
}

/// 見出しの数 100（トレーラーを含む）: ちょうどは通り、1 つ超えると 431。
#[test]
fn limit_http_headers() {
    let many = |n: usize| {
        let mut s = b"GET / HTTP/1.1\r\nHost: a\r\n".to_vec();
        for i in 1..n {
            s.extend_from_slice(format!("X-{i}: v\r\n").as_bytes());
        }
        s.extend_from_slice(b"\r\n");
        s
    };
    let got = probe_all(&[(many(100), vec![], vec![]), (many(101), vec![], vec![])]);
    passes(&got[0]);
    assert_eq!(status(&got[1]), Some(431), "{}", got[1].whole);
    let t = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n0\r\nT: 1\r\nU: 2\r\n\r\n";
    let (a, b) = at_and_below(t, 2, 4, &[]);
    passes(&a);
    assert_eq!(status(&b), Some(431), "{}", b.whole);
}

/// 本文 1 MiB: `Content-Length` は値を読んだ時点で比べ、超えれば本文を読まずに 413。分割した転送は復号した長さの合計。
#[test]
fn limit_http_body() {
    let got = roots(
        "body_case",
        &[
            vec![Arg::Int(1_048_576), Arg::Int(1_048_576), Arg::Int(0)],
            vec![Arg::Int(1_048_576), Arg::Int(1_048_576), Arg::Int(65536)],
            vec![Arg::Int(1_048_577), Arg::Int(1_048_577), Arg::Int(0)],
        ],
    );
    let ok = got[0].as_deref().unwrap();
    assert!(
        ok.starts_with("R1048629;") && ok.contains(";L1048576|"),
        "{ok}"
    );
    // 分けて与えても同じ要求（保持の欄を除く）。
    assert_eq!(
        ok.rsplit_once("|H").unwrap().0,
        got[1].as_deref().unwrap().rsplit_once("|H").unwrap().0
    );
    // `Content-Length` の行の LF（51 バイト目）で 413。本文は 1 バイトも読まない（保持は要求の行と Host だけ）。
    assert_eq!(got[2].as_deref(), Ok("E413@50|H10"));

    let chunked = roots(
        "chunked_case",
        &[
            vec![
                Arg::List(vec![Arg::Int(524_288), Arg::Int(524_288)]),
                Arg::Int(0),
            ],
            vec![
                Arg::List(vec![Arg::Int(524_288), Arg::Int(524_289)]),
                Arg::Int(4096),
            ],
        ],
    );
    let ok = chunked[0].as_deref().unwrap();
    assert!(ok.contains(";L1048576|"), "{ok}");
    // 2 つ目の大きさの行を読んだ時点で 413（2 つ目の塊は読まない）。
    let over = chunked[1].as_deref().unwrap();
    assert!(over.starts_with("E413@"), "{over}");
    let at: i64 = over[5..].split('|').next().unwrap().parse().unwrap();
    // 見出し 56 + `80000\r\n` 7 + 中身 + CRLF 2 + `80001\r`（LF の前まで）6。
    assert_eq!(at, 56 + 7 + 524_288 + 2 + 6, "{over}");

    // 小さい値: 本文の上限ちょうどは通り、1 つ下げると 413。
    let cl = b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nhello";
    let (a, b) = at_and_below(cl, 3, 5, &[]);
    passes(&a);
    assert!(b.whole.starts_with("E413@44"), "{}", b.whole);
    let ch = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
    let (a, b) = at_and_below(ch, 3, 5, &[]);
    passes(&a);
    assert_eq!(status(&b), Some(413), "{}", b.whole);
}

/// 符号化した本文 2 MiB（ADR 0032 決定 4-1）: 本文の部分として受け取ったバイト数。1 バイトの塊を並べた要求で、
/// 復号した長さが本文の上限の内側でも、符号化した長さが超えれば 413。
#[test]
fn limit_http_encoded_body() {
    let mut ch = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
    for _ in 0..10 {
        ch.extend_from_slice(b"1\r\nx\r\n");
    }
    ch.extend_from_slice(b"0\r\nT: 1\r\n\r\n");
    // 10 個の塊（60）+ 最後の塊（3）+ トレーラー（6）+ 終わりの空行（2）= 71。
    let (a, b) = at_and_below(&ch, 5, 71, &[]);
    passes(&a);
    assert_eq!(status(&b), Some(413), "{}", b.whole);
    // 塊の中身の途中で超える場合（大きな塊）も、超えたバイトを消費しない。
    let big = b"POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n";
    let (a, b) = at_and_below(big, 5, 27, &[]);
    passes(&a);
    let (b2, _) = at_and_below(big, 5, 10, &[]);
    assert_eq!(status(&b), Some(413), "{}", b.whole);
    // 見出し 56 + `10\r\n` 4 + 中身 6（符号化した 10 バイト）で止まる。
    assert!(b2.whole.starts_with("E413@66"), "{}", b2.whole);
    // `Content-Length` の要求では本文の大きさと同じ（値を読んだ時点で比べる）。
    let cl = b"POST / HTTP/1.1\r\nHost: a\r\nContent-Length: 5\r\n\r\nhello";
    let (a, b) = at_and_below(cl, 5, 5, &[]);
    passes(&a);
    assert!(b.whole.starts_with("E413@44"), "{}", b.whole);
}

/// 分割した転送の大きさの行 1 KiB（CRLF を含む）: R6（8 桁まで）で先に 400 になる。小さい値では行の長さで 400。
#[test]
fn limit_http_chunk_line() {
    let head = "POST / HTTP/1.1\r\nHost: a\r\nTransfer-Encoding: chunked\r\n\r\n";
    let long = format!("{head}{}\r\n", "1".repeat(1100)).into_bytes();
    let got = probe_all(&[(long, vec![], vec![])]);
    assert_eq!(status(&got[0]), Some(400), "{}", got[0].whole);
    // 既定値では 9 桁目で止まる（1 KiB まで溜めない）。
    assert!(got[0].whole.starts_with("E400@64"), "{}", got[0].whole);
    let ch = format!("{head}ab\r\n{}\r\n0\r\n\r\n", "z".repeat(0xab)).into_bytes();
    let (a, b) = at_and_below(&ch, 4, 4, &[]);
    passes(&a);
    assert_eq!(status(&b), Some(400), "{}", b.whole);
}

//! ADR 0032 決定 3・4・8 / PR 6-3g-c: HTTP/1.1 の接続（`http.conn`・`http.serve_conn`・`http.abort_conn`）。
//!
//! 同じバイト列を評価器・ネイティブ（ワーカー 1・4）のサーバーへ送り、応答のバイト列の連結が一致することを確かめる。
//! 時間の上限の試験は実時間モードで、小さい上限（数百ミリ秒）を引数で渡す。名前が `limit_http_` で始まる試験は、
//! ADR 0032 決定 4-1 の表の 6-3g-c の行ごとの「上限ちょうどは通り、1 つ超えると決めた動き」を確かめる。
mod common;
#[path = "common/serve.rs"]
mod serve;

use serve::*;
use std::io::{Read, Write};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn prog() -> &'static Prog {
    static P: OnceLock<Prog> = OnceLock::new();
    P.get_or_init(|| Prog::server("http-conn"))
}

const E500: &str =
    "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

/// 3 つの動かし方で、`f` が動かしたクライアントの結果が一致することを確かめる。
fn each_mode<T: PartialEq + std::fmt::Debug>(
    n: i64,
    limit: i64,
    lim: Lim,
    f: impl Fn(u16, u32) -> T,
) -> T {
    let mut seen: Vec<T> = vec![];
    for mode in MODES {
        let (got, d) = prog().serve(mode, n, limit, lim, &f);
        clean(mode, &d, &n.to_string());
        seen.push(got);
    }
    assert!(
        seen[0] == seen[1] && seen[0] == seen[2],
        "評価器とネイティブの結果が違う:\n{:?}\n---\n{:?}\n---\n{:?}",
        seen[0],
        seen[1],
        seen[2]
    );
    seen.remove(0)
}

fn fast() -> Lim {
    Lim::default()
}

// ---- 持続接続・パイプライン ----

/// 応答を待ってから次の要求を送る（持続接続）。1 つの接続で続けて答える。
#[test]
fn keep_alive_serves_many_requests_on_one_connection() {
    let got = each_mode(1, 2, fast(), |port, _| {
        let mut s = connect(port);
        let mut all = vec![];
        for i in 0..5 {
            let body = format!("n{i}");
            s.write_all(
                req(
                    "POST",
                    "/echo",
                    &format!("Content-Length: {}\r\n", body.len()),
                    &body,
                )
                .as_bytes(),
            )
            .unwrap();
            let want = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\nPOST {body}",
                5 + body.len()
            );
            let mut buf = vec![0u8; want.len()];
            s.read_exact(&mut buf).unwrap();
            assert_eq!(text(&buf), want);
            all.extend(buf);
        }
        s.shutdown(std::net::Shutdown::Write).unwrap();
        assert!(read_all(&mut s).is_empty(), "最後は何も送らずに閉じる");
        all
    });
    assert_eq!(statuses(&got), vec![200; 5]);
}

/// 同じ要求の並びを、送る位置を変えて（まとめて・1 バイトずつ・7 バイトずつ）送っても、応答の連結は同じ。
#[test]
fn pipelining_is_independent_of_how_bytes_are_split() {
    let payload = format!(
        "{}{}{}{}",
        req("GET", "/s200", "", ""),
        req("POST", "/echo", "Content-Length: 5\r\n", "hello"),
        req(
            "POST",
            "/echo",
            "Transfer-Encoding: chunked\r\n",
            "3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n"
        ),
        req("GET", "/s204", "Connection: close\r\n", "")
    );
    let mut results = vec![];
    for step in [0usize, 1, 7] {
        let p = payload.clone();
        let got = each_mode(1, 2, fast(), move |port, _| {
            exchange(port, p.as_bytes(), step)
        });
        results.push(got);
    }
    assert_eq!(results[0], results[1]);
    assert_eq!(results[0], results[2]);
    assert_eq!(statuses(&results[0]), vec![200, 200, 200, 204]);
    let t = text(&results[0]);
    assert!(t.contains("POST hello"), "{t}");
    assert!(t.contains("POST abcde"), "{t}");
    assert!(
        t.ends_with("HTTP/1.1 204 No Content\r\nconnection: close\r\n\r\n"),
        "{t}"
    );
}

/// 1 回の読みに多数の要求が並んでも、全部に答える。
#[test]
fn many_pipelined_requests_in_one_read() {
    let payload = req("GET", "/s200", "", "").repeat(200);
    let got = each_mode(1, 2, fast(), move |port, _| {
        exchange(port, payload.as_bytes(), 0)
    });
    assert_eq!(statuses(&got), vec![200; 200]);
}

#[test]
fn http10_and_connection_close_close_after_one_response() {
    let a = format!(
        "{}{}",
        "GET /s200 HTTP/1.0\r\n\r\n",
        req("GET", "/s200", "", "")
    );
    let got = each_mode(1, 2, fast(), move |port, _| exchange(port, a.as_bytes(), 0));
    assert_eq!(
        text(&got),
        "HTTP/1.1 200 OK\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello"
    );
    let b = format!(
        "{}{}",
        req("GET", "/s200", "Connection: Keep-Alive, CLOSE\r\n", ""),
        req("GET", "/s200", "", "")
    );
    let got = each_mode(1, 2, fast(), move |port, _| exchange(port, b.as_bytes(), 0));
    assert_eq!(statuses(&got), vec![200]);
    assert!(text(&got).contains("connection: close"));
}

/// 解釈の失敗の後は、応答を書いて閉じる（続くバイト列は読まない）。
#[test]
fn parse_failure_answers_and_closes() {
    let payload = format!(
        "{}{}",
        "GET / HTTP/1.1\r\nHost: x\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx",
        req("GET", "/s200", "", "")
    );
    let got = each_mode(1, 2, fast(), move |port, _| {
        exchange(port, payload.as_bytes(), 0)
    });
    assert_eq!(
        text(&got),
        "HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
}

/// 本文の大きさが上限を超える `Content-Length` は、本文を読まずに 413。
#[test]
fn oversized_content_length_is_413_without_reading_the_body() {
    let payload = req("POST", "/echo", "Content-Length: 1048577\r\n", "");
    let got = each_mode(1, 2, fast(), move |port, _| {
        exchange(port, payload.as_bytes(), 0)
    });
    assert_eq!(
        text(&got),
        "HTTP/1.1 413 Content Too Large\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
}

// ---- 100-continue ----

#[test]
fn expect_100_continue_is_answered_before_the_body() {
    let got = each_mode(1, 2, fast(), |port, _| {
        let mut s = connect(port);
        s.write_all(
            req(
                "POST",
                "/echo",
                "Content-Length: 3\r\nExpect: 100-continue\r\n",
                "",
            )
            .as_bytes(),
        )
        .unwrap();
        let want = "HTTP/1.1 100 Continue\r\n\r\n";
        let mut buf = vec![0u8; want.len()];
        s.read_exact(&mut buf).unwrap();
        assert_eq!(text(&buf), want);
        s.write_all(b"abc").unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        let mut rest = read_all(&mut s);
        let mut all = buf;
        all.append(&mut rest);
        all
    });
    assert!(
        text(&got).ends_with("HTTP/1.1 200 OK\r\ncontent-length: 8\r\n\r\nPOST abc"),
        "{}",
        text(&got)
    );
}

#[test]
fn expectation_other_than_100_continue_is_417() {
    let payload = req(
        "POST",
        "/echo",
        "Content-Length: 3\r\nExpect: 200-ok\r\n",
        "abc",
    );
    let got = each_mode(1, 2, fast(), move |port, _| {
        exchange(port, payload.as_bytes(), 0)
    });
    assert_eq!(statuses(&got), vec![417]);
}

// ---- 時間の上限（limit_http_） ----

fn timed(lim: Lim, f: impl Fn(u16) -> Vec<u8>) -> (Vec<u8>, Duration) {
    let mut last = (vec![], Duration::ZERO);
    let mut all: Vec<Vec<u8>> = vec![];
    for mode in MODES {
        let (got, d) = prog().serve(mode, 1, 2, lim, |port, _| {
            let t = Instant::now();
            let r = f(port);
            (r, t.elapsed())
        });
        clean(mode, &d, "1");
        all.push(got.0.clone());
        last = got;
    }
    assert!(all[0] == all[1] && all[0] == all[2], "モードで応答が違う");
    last
}

/// 見出しの時間: 上限の内側で送り終えれば通り、1 バイトずつ遅く送って超えると 408 で閉じる。
#[test]
fn limit_http_head_time() {
    let lim = Lim {
        head_ms: 1500,
        ..Lim::default()
    };
    let (ok, _) = timed(lim, |port| {
        let mut s = connect(port);
        let r = req("GET", "/s200", "", "");
        let (a, b) = r.as_bytes().split_at(10);
        s.write_all(a).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        s.write_all(b).unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        read_all(&mut s)
    });
    assert_eq!(statuses(&ok), vec![200]);

    let lim = Lim {
        head_ms: 400,
        ..Lim::default()
    };
    let (slow, took) = timed(lim, |port| {
        let mut s = connect(port);
        for ch in req("GET", "/s200", "", "").as_bytes() {
            if s.write_all(&[*ch]).is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(60));
        }
        read_all(&mut s)
    });
    assert_eq!(
        text(&slow),
        "HTTP/1.1 408 Request Timeout\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
    assert!(took >= Duration::from_millis(400), "{took:?}");
}

/// 新しい接続が 1 バイトも送らなければ、見出しの時間の後に何も書かずに閉じる。
#[test]
fn limit_http_head_time_silent_when_no_byte_arrives() {
    let lim = Lim {
        head_ms: 300,
        ..Lim::default()
    };
    let (got, took) = timed(lim, |port| {
        let mut s = connect(port);
        read_all(&mut s)
    });
    assert!(got.is_empty(), "{}", text(&got));
    assert!(took >= Duration::from_millis(300), "{took:?}");
}

/// 本文の時間: 見出しの後の本文が上限の内側なら通り、遅れると 408。
#[test]
fn limit_http_body_time() {
    let lim = Lim {
        body_ms: 1500,
        ..Lim::default()
    };
    let (ok, _) = timed(lim, |port| {
        let mut s = connect(port);
        s.write_all(req("POST", "/echo", "Content-Length: 4\r\n", "ab").as_bytes())
            .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        s.write_all(b"cd").unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        read_all(&mut s)
    });
    assert_eq!(statuses(&ok), vec![200]);

    let lim = Lim {
        body_ms: 300,
        ..Lim::default()
    };
    let (late, _) = timed(lim, |port| {
        let mut s = connect(port);
        s.write_all(req("POST", "/echo", "Content-Length: 4\r\n", "ab").as_bytes())
            .unwrap();
        read_all(&mut s)
    });
    assert_eq!(
        text(&late),
        "HTTP/1.1 408 Request Timeout\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
}

/// ハンドラの時間: 超えると 503。
#[test]
fn limit_http_handler_time() {
    let lim = Lim {
        handler_ms: 300,
        ..Lim::default()
    };
    let (got, _) = timed(lim, |port| {
        exchange(port, req("GET", "/slow", "", "").as_bytes(), 0)
    });
    assert_eq!(
        text(&got),
        "HTTP/1.1 503 Service Unavailable\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    );
    let lim = Lim {
        handler_ms: 5000,
        ..Lim::default()
    };
    let (got, _) = timed(lim, |port| {
        exchange(port, req("GET", "/s200", "", "").as_bytes(), 0)
    });
    assert_eq!(statuses(&got), vec![200]);
}

/// 接続を保って待つ時間: 最初の応答の後、次の要求が来なければ閉じる。
#[test]
fn limit_http_idle_time() {
    let lim = Lim {
        idle_ms: 400,
        ..Lim::default()
    };
    let (got, took) = timed(lim, |port| {
        let mut s = connect(port);
        s.write_all(req("GET", "/s200", "", "").as_bytes()).unwrap();
        read_all(&mut s)
    });
    assert_eq!(statuses(&got), vec![200]);
    assert!(!text(&got).contains("connection: close"));
    assert!(took >= Duration::from_millis(400), "{took:?}");

    // 上限の内側に次の要求を送れば、同じ接続で答える。
    let lim = Lim {
        idle_ms: 1500,
        ..Lim::default()
    };
    let (got, _) = timed(lim, |port| {
        let mut s = connect(port);
        s.write_all(req("GET", "/s200", "", "").as_bytes()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        s.write_all(req("GET", "/s200", "", "").as_bytes()).unwrap();
        s.shutdown(std::net::Shutdown::Write).unwrap();
        read_all(&mut s)
    });
    assert_eq!(statuses(&got), vec![200, 200]);
}

/// 応答の送信の時間: 相手が読まないと、上限で閉じる（サーバーは止まらず、スロットを返す）。
/// ネイティブだけで確かめる。時間の上限は中断点（書きの待ち）でだけ効く（ADR 0020 決定 4）。評価器は大きな本文の写しが
/// 相手の読む速さより遅く、書きの待ちが起きないので、上限が効く前に全部を書き終える（評価器の処理の遅さで、差ではない）。
#[test]
fn limit_http_send_time() {
    let lim = Lim {
        send_ms: 400,
        resp_body: 32 << 20,
        ..Lim::default()
    };
    for mode in [Mode::Native(1), Mode::Native(4)] {
        let (n, d) = prog().serve(mode, 1, 2, lim, |port, _| {
            let mut s = connect(port);
            s.write_all(req("GET", "/huge", "", "").as_bytes()).unwrap();
            // 応答の最初のバイトが届く（ハンドラが本文を作り終えて送り始める）のを待ってから、読まずに待つ。
            let mut first = [0u8; 1];
            s.peek(&mut first).unwrap();
            let t = Instant::now();
            std::thread::sleep(Duration::from_millis(1500));
            let mut n = 0usize;
            let mut buf = vec![0u8; 1 << 16];
            loop {
                match s.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(k) => n += k,
                }
            }
            assert!(t.elapsed() >= Duration::from_millis(1500));
            n
        });
        clean(mode, &d, "1");
        // 有効な応答の先頭の一部は届くが、本文の全部（32 MiB + 見出し）は届かない。
        assert!(n > 0 && n < (32 << 20), "{mode:?}: 一部だけ届く: {n}");
    }
}

// ---- 回数・大きさの上限 ----

/// 1 つの接続の要求の数: 上限ちょうどまで答え、上限の応答に `Connection: close` を付けて閉じる。
#[test]
fn limit_http_requests_per_connection() {
    let lim = Lim {
        requests: 3,
        ..Lim::default()
    };
    let payload = req("GET", "/s200", "", "").repeat(5);
    let (got, _) = timed(lim, move |port| exchange(port, payload.as_bytes(), 0));
    assert_eq!(statuses(&got), vec![200; 3]);
    let t = text(&got);
    assert_eq!(t.matches("connection: close").count(), 1);
    assert!(
        t.ends_with("connection: close\r\ncontent-length: 5\r\n\r\nhello")
            || t.contains("content-length: 5\r\nconnection: close\r\n\r\nhello"),
        "{t}"
    );

    // 上限を 1 にすると 1 件目で閉じる。
    let lim = Lim {
        requests: 1,
        ..Lim::default()
    };
    let payload = req("GET", "/s200", "", "").repeat(2);
    let (got, _) = timed(lim, move |port| exchange(port, payload.as_bytes(), 0));
    assert_eq!(statuses(&got), vec![200]);
}

/// 応答の本文: 上限ちょうどは通り、1 つ超えると 500 で閉じる。
#[test]
fn limit_http_response_body() {
    let lim = Lim {
        resp_body: 16,
        ..Lim::default()
    };
    let (ok, _) = timed(lim, |port| {
        exchange(port, req("GET", "/body16", "", "").as_bytes(), 0)
    });
    assert_eq!(
        text(&ok),
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: 16\r\n\r\n{}",
            "a".repeat(16)
        )
    );
    let (over, _) = timed(lim, |port| {
        exchange(port, req("GET", "/body17", "", "").as_bytes(), 0)
    });
    assert_eq!(text(&over), E500);
}

// ---- 同時の接続・隔離 ----

/// 同時の接続の数: 上限（ルートの `limit`）に達している間は次の接続を受け付けず、空いたら答える。
#[test]
fn limit_http_concurrent_connections() {
    for mode in MODES {
        let (got, d) = prog().serve(mode, 2, 1, fast(), |port, _| {
            let a = connect(port);
            let mut b = connect(port);
            b.write_all(req("GET", "/s200", "", "").as_bytes()).unwrap();
            b.set_read_timeout(Some(Duration::from_millis(600)))
                .unwrap();
            let mut buf = [0u8; 16];
            // a がスロットを持っている間、b には答えない。
            assert!(
                b.read(&mut buf).is_err(),
                "{mode:?}: 上限に達しているのに答えた"
            );
            drop(a);
            b.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
            b.shutdown(std::net::Shutdown::Write).unwrap();
            read_all(&mut b)
        });
        clean(mode, &d, "2");
        assert_eq!(statuses(&got), vec![200], "{mode:?}");
    }
}

/// 1 つの接続のハンドラの異常停止は、500 を返してその接続だけを閉じ、他の接続を止めない。
#[test]
fn handler_abort_answers_500_and_does_not_stop_other_connections() {
    for mode in MODES {
        let (got, d) = prog().serve(mode, 2, 4, fast(), |port, _| {
            let first = exchange(port, req("GET", "/abort", "", "").as_bytes(), 0);
            let second = exchange(port, req("GET", "/s200", "", "").as_bytes(), 0);
            (first, second)
        });
        clean(mode, &d, "2");
        assert_eq!(text(&got.0), E500, "{mode:?}");
        assert_eq!(statuses(&got.1), vec![200], "{mode:?}");
        assert!(
            !d.stderr.is_empty(),
            "{mode:?}: 異常停止の報告が標準エラーに出る"
        );
    }
}

/// 多数の接続の後に fd が増えない（ネイティブだけ）。
#[test]
fn many_connections_do_not_leak_fds() {
    fn fds(pid: u32) -> usize {
        std::fs::read_dir(format!("/proc/{pid}/fd"))
            .map(|d| d.count())
            .unwrap_or(0)
    }
    for w in [1u32, 4] {
        let mode = Mode::Native(w);
        let n = 120i64;
        let (counts, d) = prog().serve(mode, n + 1, 8, fast(), |port, pid| {
            let before = fds(pid);
            for i in 0..n {
                let payload = if i % 3 == 0 {
                    "BAD\r\n\r\n".to_string()
                } else {
                    req("GET", "/s200", "", "")
                };
                let got = exchange(port, payload.as_bytes(), 0);
                assert!(!got.is_empty());
            }
            std::thread::sleep(Duration::from_millis(200));
            let after = fds(pid);
            let _ = exchange(port, req("GET", "/s200", "", "").as_bytes(), 0);
            (before, after)
        });
        clean(mode, &d, &(n + 1).to_string());
        assert!(counts.1 <= counts.0 + 2, "{mode:?}: fd が増えた {counts:?}");
    }
}

// ---- 起動の時のメモリの見積もり（ADR 0032 決定 4-4） ----

/// 既定の上限での最悪のメモリ: 256 × (2 × (8 KiB + 16 KiB + 1 MiB)（完成時の写しを含む）+ 応答の本文 1 MiB + 64 KiB + `S_http` 16 KiB)。
#[test]
fn memory_bound_follows_the_adr_formula() {
    let src = "module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let l = http.default_limits();\n  return l.memory_bound();\n}\n";
    let o = common::run(src, "main", &[]).unwrap_or_else(|e| panic!("{:?}", e.error));
    let per = 2 * (8192 + 16384 + 1_048_576) + 1_048_576 + 65536 + 16384;
    assert_eq!(o.value, (256 * per).to_string());
}

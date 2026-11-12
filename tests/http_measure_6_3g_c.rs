//! ADR 0032 決定 4-4・8 / PR 6-3g-c: 計測（`S_http`・停止の印の費用）。値を出すだけで判定はしない。
//! `cargo test --test http_measure_6_3g_c -- --ignored --nocapture` で動かし、出た値を ADR 0032 の補足に記録する。
mod common;
#[path = "common/serve.rs"]
mod serve;

use serve::*;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn rss_kib(pid: u32) -> i64 {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
    s.lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
        .unwrap_or(0)
}

/// 同時の接続 256 を、(a) 応答を返した後の待ちで、(b) 本文の途中（見出しは上限いっぱい、本文は 1 MiB の宣言で半分）で
/// 保ったときの常駐メモリ（`VmRSS`）の増分を、接続あたりに割る。ネイティブ（ワーカー 1）。
#[test]
#[ignore]
fn measure_s_http() {
    let prog = Prog::server("http-measure-mem");
    for (label, midbody) in [
        ("idle-after-response", false),
        ("mid-body-half-of-1MiB", true),
    ] {
        let conns = 256i64;
        let (r, d) = prog.serve(
            Mode::Native(1),
            conns,
            conns,
            Lim::default(),
            |port, pid| {
                let base = rss_kib(pid);
                let mut held = vec![];
                for i in 0..conns {
                    let mut s = connect(port);
                    if midbody {
                        // 見出しの部分をほぼ 16 KiB にして、本文は 1 MiB の半分だけ送る。
                        let pad = "x".repeat(1000);
                        let mut head = String::from(
                            "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 1048576\r\n",
                        );
                        for k in 0..15 {
                            head.push_str(&format!("X-P{k}: {pad}\r\n"));
                        }
                        head.push_str("\r\n");
                        s.write_all(head.as_bytes()).unwrap();
                        s.write_all(&vec![b'a'; 524_288]).unwrap();
                    } else {
                        s.write_all(req("GET", "/s200", "", "").as_bytes()).unwrap();
                        let mut buf = [0u8; 64];
                        let _ = s.read(&mut buf).unwrap();
                    }
                    held.push(s);
                    if i % 32 == 31 {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                }
                std::thread::sleep(Duration::from_millis(1500));
                let peak = rss_kib(pid);
                drop(held);
                (base, peak)
            },
        );
        let _ = d;
        let per = (r.1 - r.0) as f64 / conns as f64;
        println!(
            "MEASURE S_http {label}: base={} KiB peak={} KiB per_conn={per:.1} KiB",
            r.0, r.1
        );
    }
}

/// 持続接続で 200 件ずつ並べた要求を送り続けたときの処理量（応答の数 / 秒）。1 つの接続で 800 件（上限 1000 の内側）を送る。
/// ネイティブ（ワーカー 1・4）と評価器。
#[test]
#[ignore]
fn measure_throughput() {
    let prog = Prog::server("http-measure-thr");
    for mode in MODES {
        let conns = if mode == Mode::Eval { 3 } else { 40 };
        let (n, d) = prog.serve(mode, conns, 2, Lim::default(), |port, _| {
            let one = req("GET", "/s200", "", "");
            let batch = one.repeat(200);
            let want = 200 * "HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello".len();
            let mut buf = vec![0u8; want];
            let t = Instant::now();
            for _ in 0..conns {
                let mut s = connect(port);
                for _ in 0..4 {
                    s.write_all(batch.as_bytes()).unwrap();
                    s.read_exact(&mut buf).unwrap();
                }
                s.shutdown(std::net::Shutdown::Write).unwrap();
            }
            (conns * 800) as f64 / t.elapsed().as_secs_f64()
        });
        let _ = d;
        println!("MEASURE throughput {mode:?}: {n:.0} responses/s");
    }
}

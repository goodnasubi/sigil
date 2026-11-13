//! Issue #417: 本文の取り込み（`TcpStream.read` の `recv_once`）を 8 バイトずつ写す形に変えたので、端数（8 の倍数でない長さ）と
//! 1 回の読みの境目（65536）をまたぐ長さの本文を、評価器・ネイティブ（ワーカー 1・4）のサーバーへ送って、そのまま返ってくる
//! （バイト列が壊れない）ことを確かめる。`/echo` は `"POST " + 本文` を返す。
mod common;
#[path = "common/serve.rs"]
mod serve;

use serve::*;

/// 位置ごとに値が違う本文（バイトの並びを入れ替えても気づける）。
fn body(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|k| ((k * 7 + seed * 31 + k / 251) % 256) as u8)
        .collect()
}

#[test]
fn bodies_of_awkward_lengths_come_back_intact() {
    // 端数（0〜17）と、8 の倍数の前後、1 回の読みの上限（65536）の前後。
    let sizes: Vec<usize> = (0..=17)
        .chain([63, 64, 65, 4095, 4096, 4097, 65535, 65536, 65537, 70001])
        .collect();
    let prog = Prog::server("http-body-ingest");
    for mode in MODES {
        let n = sizes.len() as i64;
        let (got, d) = prog.serve(mode, n, 4, Lim::default(), |port, _| {
            sizes
                .iter()
                .enumerate()
                .map(|(i, &len)| {
                    let b = body(len, i);
                    let mut req = format!("POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n").into_bytes();
                    req.extend(&b);
                    (b, exchange(port, &req, 0))
                })
                .collect::<Vec<_>>()
        });
        clean(mode, &d, &n.to_string());
        for (b, resp) in got {
            let split = resp
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("見出しの終わり")
                + 4;
            let mut want = b"POST ".to_vec();
            want.extend(&b);
            assert_eq!(
                &resp[split..],
                &want[..],
                "{mode:?}: 長さ {} の本文が壊れた",
                b.len()
            );
        }
    }
}

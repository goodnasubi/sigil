//! Issue #427: 書き（`send_once`）が、送るバイトを 4 バイトずつ写す形に変わったので、端数（4 の倍数でない長さ）と、
//! 1 回の書きの上限（65536）をまたぐ長さの応答の本文が、評価器・ネイティブ（ワーカー 1・4）で壊れずに届く
//! （バイトの並び・見出しの `content-length` が合う）ことを確かめる。`/echo` は `"POST " + 本文` を返す。
mod common;
#[path = "common/serve.rs"]
mod serve;

use serve::*;

/// 位置ごとに値が違う本文（バイトの並びを入れ替えても気づける）。0〜255 のすべての値を含む。
fn body(len: usize, seed: usize) -> Vec<u8> {
    (0..len)
        .map(|k| ((k * 13 + seed * 29 + k / 251) % 256) as u8)
        .collect()
}

#[test]
fn response_bodies_of_awkward_lengths_arrive_intact() {
    // 応答は `"POST " + 本文`（5 バイト足す）。応答の本文の長さの端数（0〜7）と、書きの上限（65536）の前後、複数回の書き。
    let sizes: Vec<usize> = (0..=7)
        .chain([
            59, 60, 61, 4091, 65530, 65531, 65532, 65536, 131067, 131072, 200003,
        ])
        .collect();
    let prog = Prog::server("http-send-once");
    for mode in MODES {
        let n = sizes.len() as i64;
        let (got, d) = prog.serve(mode, n, 4, Lim::default(), |port, _| {
            sizes
                .iter()
                .enumerate()
                .map(|(i, &len)| {
                    let b = body(len, i);
                    let mut req = format!(
                        "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes();
                    req.extend(&b);
                    (b, exchange(port, &req, 0))
                })
                .collect::<Vec<_>>()
        });
        clean(mode, &d, &n.to_string());
        for (b, resp) in got {
            let mut want = b"POST ".to_vec();
            want.extend(&b);
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                want.len()
            );
            let mut full = head.into_bytes();
            full.extend(&want);
            assert!(resp == full, "{mode:?}: 長さ {} の応答が壊れた", b.len());
        }
    }
}

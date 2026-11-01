//! 言語サーバーの枠（`Content-Length` 付きの見出しと本文）の読み取りのファジング（ADR 0020 の決定 2・4）。
//! 枠は `tests/common/fuzz.rs`。
//!
//! 性質: 任意のバイト列で落ちない／スタックを使い切らない。枠を読み進めるたびに入力が進み、
//! 有限回で `Eof` か `Broken` に至る。本文は上限以下。上限を超える本文は確保せずに `Skipped` になる。

#[path = "common/fuzz.rs"]
mod fuzz;

use fuzz::Rng;
use sigil::lsp::{read_frame, Frame, MAX_BODY};
use std::io::Cursor;

/// 小さい上限（超える・超えないの境目を短い入力で突く）と、実際の上限の両方で読む。
fn drain(input: &[u8], max_body: u64) {
    let mut cur = Cursor::new(input.to_vec());
    // 1 枠ごとに 1 バイト以上進むので、入力の長さ + 1 回で必ず終わる。
    for _ in 0..=input.len() {
        let before = cur.position();
        match read_frame(&mut cur, max_body) {
            Frame::Body(b) => {
                assert!(b.len() as u64 <= max_body, "本文が上限を超えた");
                assert!(cur.position() > before, "本文を返したのに進まない");
            }
            Frame::Skipped(_) => assert!(cur.position() > before, "捨てたのに進まない"),
            Frame::Eof => {
                assert_eq!(before as usize, input.len(), "途中で Eof になった");
                return;
            }
            Frame::Broken(_) => return,
        }
    }
    panic!("入力の長さの回数で終わらない");
}

fn check(input: &[u8]) {
    drain(input, 16);
    drain(input, MAX_BODY);
}

fn frame_of(rng: &mut Rng, out: &mut Vec<u8>) {
    let body: Vec<u8> = (0..rng.below(40))
        .map(|_| *rng.pick(b"{}[]\":, abc01\n"))
        .collect();
    let eol: &[u8] = if rng.chance(1, 8) { b"\n" } else { b"\r\n" };
    for _ in 0..rng.below(3) {
        let h: Vec<u8> = match rng.below(6) {
            0 => b"Content-Type: application/vscode-jsonrpc; charset=utf-8".to_vec(),
            1 => format!("X-{}: {}", "a".repeat(rng.below(1200)), rng.below(9)).into_bytes(),
            2 => b"no colon".to_vec(),
            3 => format!(
                "Content-Length: {}",
                rng.pick(&["abc", "-1", "", " 7 ", "+3"])
            )
            .into_bytes(),
            4 => format!("Content-Length: {}", body.len() + rng.below(3)).into_bytes(),
            _ => format!("content-length: {}", u64::MAX).into_bytes(),
        };
        out.extend_from_slice(&h);
        out.extend_from_slice(eol);
    }
    if !rng.chance(1, 10) {
        out.extend_from_slice(format!("Content-Length: {}", body.len()).as_bytes());
        out.extend_from_slice(eol);
    }
    out.extend_from_slice(eol);
    out.extend_from_slice(&body);
}

fn grammar(rng: &mut Rng) -> Vec<u8> {
    let mut out = vec![];
    for _ in 0..1 + rng.below(4) {
        frame_of(rng, &mut out);
    }
    if rng.chance(1, 5) && !out.is_empty() {
        let i = rng.below(out.len());
        out.truncate(i);
    }
    out
}

#[test]
fn fuzz_lsp_frame() {
    fuzz::run(
        "lsp_frame",
        3000,
        fuzz::corpus("lsp_frame", &[]),
        grammar,
        check,
    );
}

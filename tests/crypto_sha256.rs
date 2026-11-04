//! PR 6-1c: SHA-256（ADR 0024 決定 1 の 3）の利用者向けテスト。
//!
//! - ワンショット `sha256(data)` と、ストリーミング `Sha256`
//!   （new / update / finalize）の両方が、既知のダイジェストと一致する。
//! - テストベクタ: 空・"abc"・56 バイトは RFC 6234 の TEST1 / TEST2。
//!   56 バイトはパディングが 2 ブロック目に広がる境界、64 バイトちょうどは
//!   ブロック境界（パディングが丸ごと次のブロックになる）、640 バイトは
//!   複数ブロックの代表で、この 2 つは OpenSSL（hashlib）で検算した値。
//! - ストリーミングは、分割の仕方（1 バイトずつ、ブロック境界の前後など）
//!   によらずワンショットと一致する（ADR 0024 決定 4 のストリーミング不変性）。
//! - ここでのテストは評価器とネイティブの両方で回る（`common::run` が
//!   突き合わせる）。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module crypto_sha256.t\n  targets server\n  trust none\n\n";

/// ダイジェストと期待バイト列の不一致の数を数える補助（テスト側の関数）。
const MISMATCH_FN: &str = "fn mismatch(d: &Bytes, want: &List<Int>) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n  let mut i = 0;\n  while i < want.len() {\n    let w = match want.get(i) { Some(v) => v, None => 0 };\n    let g = d.get(i).unwrap_or(0 - 1);\n    if g != w {\n      bad = bad + 1;\n    }\n    i = i + 1;\n  }\n  if d.len() != want.len() {\n    bad = bad + 100;\n  }\n  return bad;\n}\n\n";

fn build_bytes(name: &str, bytes: &[u8]) -> String {
    let mut s = format!("  let mut {name} = Bytes.new();\n");
    for b in bytes {
        s.push_str(&format!("  {name}.push({b});\n"));
    }
    s
}

fn build_expected(name: &str, hex: &str) -> String {
    let mut s = format!("  let mut {name} = List.new();\n");
    for i in (0..hex.len()).step_by(2) {
        let v = u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        s.push_str(&format!("  {name}.push({v});\n"));
    }
    s
}

fn run_oneshot(input: &[u8], expected_hex: &str) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let d = sha256(&input);\n{}  return mismatch(&d, &want);\n}}\n",
        build_bytes("input", input),
        build_expected("want", expected_hex),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

/// 入力を `chunks` の大きさで順に切り分けて update し、finalize の結果を
/// 期待値と比べる。分割の列が入力全体をちょうど覆う前提。
fn run_streaming(input: &[u8], chunks: &[usize], expected_hex: &str) -> i64 {
    assert_eq!(chunks.iter().sum::<usize>(), input.len());
    let mut body = String::new();
    body.push_str("  let mut h = Sha256.new();\n");
    let mut pos = 0;
    for (ci, &n) in chunks.iter().enumerate() {
        let part = &input[pos..pos + n];
        body.push_str(&build_bytes(&format!("c{ci}"), part));
        body.push_str(&format!("  h.update(&c{ci});\n"));
        pos += n;
    }
    body.push_str("  let d = h.finalize();\n");
    body.push_str(&build_expected("want", expected_hex));
    body.push_str("  return mismatch(&d, &want);\n");
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}}}\n"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

const EMPTY_HEX: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const ABC_HEX: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const MSG56: &[u8] = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
const MSG56_HEX: &str = "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1";

fn msg64() -> Vec<u8> {
    b"01234567".repeat(8)
}
const MSG64_HEX: &str = "8182cadb21af0e37c06414ece08e19c65bdb22c396d48ba7341012eea9ffdfdd";

fn msg640() -> Vec<u8> {
    b"01234567012345670123456701234567".repeat(20)
}
const MSG640_HEX: &str = "594847328451bdfa85056225462cc1d867d877fb388df0ce35f25ab5562bfbb5";

#[test]
fn sha256_empty() {
    assert_eq!(run_oneshot(b"", EMPTY_HEX), 0);
}

#[test]
fn sha256_abc() {
    assert_eq!(run_oneshot(b"abc", ABC_HEX), 0);
}

#[test]
fn sha256_56_byte_boundary() {
    // 56 バイト: 0x80 と長さ欄が 2 ブロック目に広がる境界。
    assert_eq!(MSG56.len(), 56);
    assert_eq!(run_oneshot(MSG56, MSG56_HEX), 0);
}

#[test]
fn sha256_64_byte_exact_block() {
    // ブロックちょうど: パディングは丸ごと次のブロックになる。
    let msg = msg64();
    assert_eq!(msg.len(), 64);
    assert_eq!(run_oneshot(&msg, MSG64_HEX), 0);
}

#[test]
fn sha256_multi_block() {
    let msg = msg640();
    assert_eq!(msg.len(), 640);
    assert_eq!(run_oneshot(&msg, MSG640_HEX), 0);
}

#[test]
fn sha256_streaming_one_byte_at_a_time() {
    let chunks = vec![1usize; MSG56.len()];
    assert_eq!(run_streaming(MSG56, &chunks, MSG56_HEX), 0);
}

#[test]
fn sha256_streaming_block_edge_splits() {
    // ブロック境界の前後（63 + 1、64 ちょうど、65 = 64 + 1）で割る。
    let msg = msg640();
    assert_eq!(run_streaming(&msg, &[63, 1, 64, 65, 447], MSG640_HEX), 0);
    assert_eq!(
        run_streaming(&msg, &[128, 128, 128, 128, 128], MSG640_HEX),
        0
    );
}

#[test]
fn sha256_streaming_empty_and_single_update() {
    // update なしの finalize は空のダイジェスト、1 回の update は
    // ワンショットと同じ。
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut h = Sha256.new();\n  let d0 = h.finalize();\n{}  let a = mismatch(&d0, &want);\n  let mut h2 = Sha256.new();\n{}  h2.update(&input);\n  let d1 = h2.finalize();\n  return a * 1000 + mismatch(&d1, &want);\n}}\n",
        build_expected("want", EMPTY_HEX),
        build_bytes("input", b""),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    let v: i64 = r.value.parse().unwrap();
    assert_eq!(v, 0);
    assert_eq!(run_streaming(b"abc", &[3], ABC_HEX), 0);
}

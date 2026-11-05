//! PBKDF2-HMAC-SHA-256 の鍵パッド状態の使い回し（PR 6-1e）の値の固定。
//!
//! 鍵（パスワード）は全反復で同じなので、ipad・opad のブロックを処理した状態を
//! 最初に 1 回だけ求めて使い回す（`hmac_from_pads`）。値が `hashlib.pbkdf2_hmac('sha256', ...)`
//! と変わらないことを、鍵長の境界（0・33・64・65 バイト）、ソルトの長さの境界（内側のメッセージが
//! 1 ブロックに収まる 51 バイトと、2 ブロックになる 52 バイト）、出力の長さ、
//! 反復が 1024 の倍数（`task.yield` の協調点）をまたぐ場合で確かめる。
//! 入力は決定的な並び（`(seed*31 + i*7 + 3) % 256`）、期待値は Python の `hashlib` で検算した値。
//! 評価器とネイティブの両方で回る（`common::run` が突き合わせる）。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module crypto_pbkdf2_pads.t\n  targets server\n  trust none\n\n";

const MISMATCH_FN: &str = "fn mismatch(d: &Bytes, want: &List<Int>) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n  let mut i = 0;\n  while i < want.len() {\n    let w = match want.get(i) { Some(v) => v, None => 0 };\n    let g = d.get(i).unwrap_or(0 - 1);\n    if g != w {\n      bad = bad + 1;\n    }\n    i = i + 1;\n  }\n  if d.len() != want.len() {\n    bad = bad + 100;\n  }\n  return bad;\n}\n\n";

fn gen(n: usize, seed: usize) -> Vec<u8> {
    (0..n)
        .map(|i| ((seed * 31 + i * 7 + 3) % 256) as u8)
        .collect()
}

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

/// 不一致の数を返す（0 なら `hashlib` の値と一致）。
fn mismatches(
    pw_len: usize,
    salt_len: usize,
    iterations: i64,
    dk_len: usize,
    expected_hex: &str,
) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects task.suspend\n{{\n{}  let pw = Secret.from_bytes(pb);\n{}  let dk = pbkdf2_sha256(&pw, &salt, {iterations}, {dk_len});\n  let got = dk.expose();\n{}  return mismatch(&got, &want);\n}}\n",
        build_bytes("pb", &gen(pw_len, 1)),
        build_bytes("salt", &gen(salt_len, 2)),
        build_expected("want", expected_hex),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

#[test]
fn empty_password() {
    assert_eq!(
        mismatches(
            0,
            12,
            4096,
            32,
            "3ffd5de173d543eb41dc7402d2b0b913145773b2fd2e510b2a3d87389ca08dc2"
        ),
        0
    );
}

#[test]
fn password_33_bytes() {
    assert_eq!(
        mismatches(
            33,
            20,
            4096,
            32,
            "2846b1b534e6a5d6e1c8bc2fcf816d913cf873299e631c8aca6c0c106369c66e"
        ),
        0
    );
}

#[test]
fn password_64_bytes_exact_block() {
    assert_eq!(
        mismatches(
            64,
            20,
            4096,
            32,
            "cba746e91e4e81b413a957b9f8762e0554cf8979ff69c1fd31ec89ed2e4ea8e6"
        ),
        0
    );
}

#[test]
fn password_65_bytes_is_hashed() {
    assert_eq!(
        mismatches(
            65,
            20,
            4096,
            32,
            "71095fa816f8330ac50fb1b153ee319dabbcbab8f334baefe5e281874e0555ae"
        ),
        0
    );
}

#[test]
fn password_100_bytes_is_hashed() {
    // 出力が複数ブロックになる場合は `crypto_hmac` の `pbkdf2_multi_block_and_truncation` が覆う。
    assert_eq!(
        mismatches(
            100,
            24,
            4096,
            32,
            "a0fce5bc3a4adff703228681748e17632ec8df149c52b8846d0f442144f62bbe"
        ),
        0
    );
}

#[test]
fn salt_51_inner_message_fits_one_block() {
    assert_eq!(
        mismatches(
            9,
            51,
            4096,
            32,
            "097f5c016ca751ea586aad516d25851413baa04e9c6ccc460a00d97bd8b995c2"
        ),
        0
    );
}

#[test]
fn salt_52_inner_message_needs_two_blocks() {
    assert_eq!(
        mismatches(
            9,
            52,
            4096,
            32,
            "d95af5b2e862759284f6852c1100a49311e023cbbb8959d72bf47db739d6283a"
        ),
        0
    );
}

#[test]
fn iterations_cross_yield_boundary() {
    // 4100 反復: 1024・2048・3072・4096 の 4 回の協調点（`task.yield`）をまたぐ。
    assert_eq!(
        mismatches(
            9,
            8,
            4100,
            32,
            "c493d3b9fefcefd7ef03c56b01f48017299b4413f1b203277fcdd3efae377d0e"
        ),
        0
    );
}

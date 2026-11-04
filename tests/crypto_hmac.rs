//! PR 6-1d: HMAC-SHA-256 と PBKDF2-HMAC-SHA-256（ADR 0024 決定 1 の
//! 4・5）の利用者向けテスト。
//!
//! - HMAC のテストベクタは RFC 4231 のケース 1〜7。期待値は Python の
//!   `hmac`（hashlib）で検算し、RFC の公表値と一致することも確認した。
//!   ケース 1〜5 の鍵は 32 バイト未満で、そのままでは `requires`
//!   （`key.len() >= 32`）に反するため、ゼロバイトを足して 32 バイトに
//!   して渡す。HMAC は鍵を 64 バイトへゼロ詰めするので、64 バイト以下
//!   の範囲でのゼロの追加は HMAC の値を変えず、RFC の期待値がそのまま
//!   使える（この等価性も Python で検算した）。
//! - PBKDF2 の入力は RFC 6070 の組（password / salt / 反復回数 / 出力長）
//!   を土台にするが、RFC 6070 の期待値は HMAC-SHA1 用なので流用せず、
//!   期待値は Python の `hashlib.pbkdf2_hmac('sha256', ...)` で検算した
//!   値を使う。反復回数は `requires` の範囲（4096〜100000）に収まる組
//!   だけを使う。
//! - 契約違反（32 バイト未満の鍵、範囲外の反復回数、dk_len = 0）は
//!   異常停止（`StopKind::RequiresViolated`）になることを確かめる。
//! - ここでのテストは評価器とネイティブの両方で回る（`common::run` が
//!   突き合わせる）。PBKDF2 の反復は評価器では遅いため、ベクタは
//!   最小の反復回数（4096）の組に絞る。
//!
//! 逸脱の注記: ADR / Issue は「1024 回ごとに `task.yield`」を求めるが、
//! 効果 `task.suspend` は HOSTED 専用で `std.crypto`（targets に bare を
//! 含む）では宣言できないため、この PR の実装には yield が入って
//! いない（`src/stdlib/crypto.sigil` の注記と Issue #289 への報告）。

mod common;

use sigil::exec::{Arg, RunError, StopKind};

const HEAD: &str = "module crypto_hmac.t\n  targets server\n  trust none\n\n";

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

/// RFC の短い鍵を、ゼロバイトを足して 32 バイトにする（HMAC の値は
/// 変わらない。ファイル冒頭の注記）。
fn pad32(key: &[u8]) -> Vec<u8> {
    assert!(key.len() < 32);
    let mut k = key.to_vec();
    k.resize(32, 0);
    k
}

fn run_hmac(key: &[u8], msg: &[u8], expected_hex: &str) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let key = Secret.from_bytes(kb);\n{}  let mac = hmac_sha256(&key, &msg);\n  let got = mac.expose();\n{}  return mismatch(&got, &want);\n}}\n",
        build_bytes("kb", key),
        build_bytes("msg", msg),
        build_expected("want", expected_hex),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

// ---------------------------------------------------------------------------
// HMAC: RFC 4231 ケース 1〜7
// ---------------------------------------------------------------------------

#[test]
fn hmac_rfc4231_case1() {
    let key = pad32(&[0x0b; 20]);
    assert_eq!(
        run_hmac(
            &key,
            b"Hi There",
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        ),
        0
    );
}

#[test]
fn hmac_rfc4231_case2() {
    let key = pad32(b"Jefe");
    assert_eq!(
        run_hmac(
            &key,
            b"what do ya want for nothing?",
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        ),
        0
    );
}

#[test]
fn hmac_rfc4231_case3() {
    let key = pad32(&[0xaa; 20]);
    assert_eq!(
        run_hmac(
            &key,
            &[0xdd; 50],
            "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe"
        ),
        0
    );
}

#[test]
fn hmac_rfc4231_case4() {
    let key = pad32(&(1u8..=25).collect::<Vec<u8>>());
    assert_eq!(
        run_hmac(
            &key,
            &[0xcd; 50],
            "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b"
        ),
        0
    );
}

#[test]
fn hmac_rfc4231_case5() {
    // RFC は先頭 128 ビットだけを示す。期待値は Python で検算した全体。
    let key = pad32(&[0x0c; 20]);
    assert_eq!(
        run_hmac(
            &key,
            b"Test With Truncation",
            "a3b6167473100ee06e0c796c2955552bfa6f7c0a6a8aef8b93f860aab0cd20c5"
        ),
        0
    );
}

#[test]
fn hmac_rfc4231_case6() {
    // 131 バイトの鍵: 先に SHA-256 で圧縮される経路。
    let key = [0xaa; 131];
    assert_eq!(
        run_hmac(
            &key,
            b"Test Using Larger Than Block-Size Key - Hash Key First",
            "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
        ),
        0
    );
}

#[test]
fn hmac_rfc4231_case7() {
    let key = [0xaa; 131];
    let msg = b"This is a test using a larger than block-size key and a larger than block-size data. The key needs to be hashed before being used by the HMAC algorithm.";
    assert_eq!(msg.len(), 152);
    assert_eq!(
        run_hmac(
            &key,
            msg,
            "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2"
        ),
        0
    );
}

// ---------------------------------------------------------------------------
// HMAC: 鍵の長さの境界（期待値は Python の hmac で検算）
// ---------------------------------------------------------------------------

#[test]
fn hmac_key_exactly_32() {
    assert_eq!(
        run_hmac(
            &[0x0b; 32],
            b"Hi There",
            "198a607eb44bfbc69903a0f1cf2bbdc5ba0aa3f3d9ae3c1c7a3b1696a0b68cf7"
        ),
        0
    );
}

#[test]
fn hmac_key_exactly_64_not_hashed() {
    let key: Vec<u8> = (0u8..64).collect();
    assert_eq!(
        run_hmac(
            &key,
            b"abc",
            "6ab541b4869dca71c4ca11d8bb1b02533b789a557583161429292c7404bc21f6"
        ),
        0
    );
}

#[test]
fn hmac_key_65_is_hashed() {
    let key: Vec<u8> = (0u8..65).collect();
    assert_eq!(
        run_hmac(
            &key,
            b"abc",
            "dfbffee4671bad00ed5d1e1999d55ed3b0cc774ac357f9ebf649c1612414fcec"
        ),
        0
    );
}

#[test]
fn hmac_empty_message() {
    assert_eq!(
        run_hmac(
            b"k".repeat(32).as_slice(),
            b"",
            "a4ad8e340e32bf1d916005cb1dc92795148897965e36ae56c0adc357474b5372"
        ),
        0
    );
}

// ---------------------------------------------------------------------------
// verify_hmac_sha256
// ---------------------------------------------------------------------------

fn run_verify(key: &[u8], msg: &[u8], expected: &[u8]) -> i64 {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let key = Secret.from_bytes(kb);\n{}  {}  if verify_hmac_sha256(&key, &msg, &exp) {{\n    return 1;\n  }}\n  return 0;\n}}\n",
        build_bytes("kb", key),
        build_bytes("msg", msg),
        build_bytes("exp", expected),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn verify_accepts_correct_mac() {
    let mac = hex_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(run_verify(&pad32(&[0x0b; 20]), b"Hi There", &mac), 1);
}

#[test]
fn verify_rejects_wrong_mac_and_wrong_length() {
    let mut mac = hex_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    mac[0] ^= 1;
    assert_eq!(run_verify(&pad32(&[0x0b; 20]), b"Hi There", &mac), 0);
    // 長さの違う期待値も、異常停止ではなく false で返る。
    let short = hex_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c");
    assert_eq!(run_verify(&pad32(&[0x0b; 20]), b"Hi There", &short), 0);
    // メッセージが違えば一致しない。
    let mac = hex_bytes("b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7");
    assert_eq!(run_verify(&pad32(&[0x0b; 20]), b"Hi There!", &mac), 0);
}

// ---------------------------------------------------------------------------
// PBKDF2（期待値は hashlib.pbkdf2_hmac('sha256', ...) で検算）
// ---------------------------------------------------------------------------

fn run_pbkdf2(
    password: &[u8],
    salt: &[u8],
    iterations: i64,
    dk_len: usize,
    expected_hex: &str,
) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let pw = Secret.from_bytes(pb);\n{}  let dk = pbkdf2_sha256(&pw, &salt, {iterations}, {dk_len});\n  let got = dk.expose();\n{}  return mismatch(&got, &want);\n}}\n",
        build_bytes("pb", password),
        build_bytes("salt", salt),
        build_expected("want", expected_hex),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

#[test]
fn pbkdf2_rfc6070_inputs_4096_dk32() {
    assert_eq!(
        run_pbkdf2(
            b"password",
            b"salt",
            4096,
            32,
            "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"
        ),
        0
    );
}

#[test]
fn pbkdf2_multi_block_and_truncation() {
    // dk_len 40: 2 ブロック目を途中まで使う（切り詰め）。
    assert_eq!(
        run_pbkdf2(
            b"passwordPASSWORDpassword",
            b"saltSALTsaltSALTsaltSALTsaltSALTsalt",
            4096,
            40,
            "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9"
        ),
        0
    );
}

// ---------------------------------------------------------------------------
// 契約違反は異常停止になる
// ---------------------------------------------------------------------------

fn stop_kind(src: &str) -> StopKind {
    match common::run(src, "main", &[] as &[Arg]) {
        Ok(o) => panic!("異常停止するはずが成功した: {}", o.value),
        Err(e) => match e.error {
            RunError::Stop(s) => s.kind,
            other => panic!("異常停止でない失敗: {other}"),
        },
    }
}

#[test]
fn hmac_short_key_is_a_contract_violation() {
    // 31 バイトの鍵（requires は 32 バイト以上）。
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let key = Secret.from_bytes(kb);\n{}  let mac = hmac_sha256(&key, &msg);\n  return mac.len();\n}}\n",
        build_bytes("kb", &[7u8; 31]),
        build_bytes("msg", b"abc"),
    );
    assert_eq!(stop_kind(&src), StopKind::RequiresViolated);
}

#[test]
fn verify_short_key_is_a_contract_violation() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let key = Secret.from_bytes(kb);\n{}  {}  if verify_hmac_sha256(&key, &msg, &exp) {{\n    return 1;\n  }}\n  return 0;\n}}\n",
        build_bytes("kb", &[7u8; 31]),
        build_bytes("msg", b"abc"),
        build_bytes("exp", &[0u8; 32]),
    );
    assert_eq!(stop_kind(&src), StopKind::RequiresViolated);
}

#[test]
fn pbkdf2_out_of_range_is_a_contract_violation() {
    for (iterations, dk_len) in [(4095, 32), (100001, 32), (4096, 0)] {
        let src = format!(
            "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  let pw = Secret.from_bytes(pb);\n{}  let dk = pbkdf2_sha256(&pw, &salt, {iterations}, {dk_len});\n  return dk.len();\n}}\n",
            build_bytes("pb", b"password"),
            build_bytes("salt", b"salt"),
        );
        assert_eq!(
            stop_kind(&src),
            StopKind::RequiresViolated,
            "iterations={iterations} dk_len={dk_len}"
        );
    }
}

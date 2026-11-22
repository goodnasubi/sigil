//! ADR 0058 段 1b: AES-128-GCM（`aes128_gcm_seal`・`aes128_gcm_open`）の利用者向けテスト。
//!
//! 出典と検算（ADR 0058 決定 4）:
//! - GCM の既知値は、NIST SP 800-38D の元になった仕様書（McGrew・Viega「The Galois/Counter
//!   Mode of Operation」付録 B）のテストケース 1〜4（鍵 128 ビット、96 ビット IV、128 ビットタグ）。
//!   NIST CAVP の GCM ベクタ（gcmtestvectors.zip）の入手を試したが、この実行環境からは
//!   csrc.nist.gov へ到達できなかったため、CAVP のファイルそのものは取り込んでいない。
//!   仕様書の値は OpenSSL（Python の `cryptography` の AESGCM）で独立に再計算して一致を確認した。
//! - 追加のケース（平文 0〜65 バイト、AAD 0〜32 バイト。ブロック境界の前後を含む）の期待値は、
//!   同じ AESGCM で生成した（入力は決定的な並び `(seed*31 + i*7 + 3) % 256`）。
//! - FIPS 197 の AES 例（付録 B と C.1）は内部のブロック演算の照合で、公開 API からは
//!   見えないため、`embedded_extra` で `std.crypto` に試験用の入口を足した版を評価器で動かす。
//!   ネイティブでの AES ブロックの正しさは、GCM ベクタ（上記）が間接的に固定する。
//! - 公開 API のテストは評価器とネイティブの両方で回る（`common::run` が突き合わせる）。
//!
//! 長さの上限（平文 2^36 − 32 バイト、AAD 2^61 − 1 バイト）は、超える長さのバイト列を
//! この環境では作れないため、実際に超える入力での試験はしていない（`InputTooLong` を返す
//! 分岐は読んで確認した。PR 本文に記載）。

mod common;

use sigil::exec::{Arg, Options};

const HEAD: &str = "module crypto_aes_gcm.t\n  targets server\n  trust none\n\n";

const MISMATCH_FN: &str = "fn mismatch(d: &Bytes, want: &List<Int>) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n  let mut i = 0;\n  while i < want.len() {\n    let w = match want.get(i) { Some(v) => v, None => 0 };\n    let g = d.get(i).unwrap_or(0 - 1);\n    if g != w {\n      bad = bad + 1;\n    }\n    i = i + 1;\n  }\n  if d.byte_len() != want.len() {\n    bad = bad + 100;\n  }\n  return bad;\n}\n\nfn code(e: AeadError) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return match e {\n    AeadError.InvalidKeyLength => 1,\n    AeadError.InvalidNonceLength => 2,\n    AeadError.InputTooShort => 3,\n    AeadError.InputTooLong => 4,\n    AeadError.AuthenticationFailed => 5,\n  };\n}\n\n";

const MAIN_HEAD: &str =
    "fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n";

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

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

fn build_expected(name: &str, bytes: &[u8]) -> String {
    let mut s = format!("  let mut {name} = List.new();\n");
    for b in bytes {
        s.push_str(&format!("  {name}.push({b});\n"));
    }
    s
}

/// 暗号化の出力が期待値（暗号文 + タグ）と一致するか、復号が平文へ戻るかを数える。
/// 0 なら両方とも一致。
fn run_roundtrip(key: &[u8], iv: &[u8], aad: &[u8], pt: &[u8], sealed: &[u8]) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}{MAIN_HEAD}{}{}{}{}{}{}{}",
        build_bytes("kb", key),
        build_bytes("iv", iv),
        build_bytes("aad", aad),
        build_bytes("pb", pt),
        build_expected("want", sealed),
        build_expected("wantp", pt),
        "  let kr = Aes128GcmKey.from_secret(Secret.from_bytes(kb));
  match kr {
    Err(e) => { return 1000 + code(e); }
    Ok(key) => {
      let n1 = Nonce96.from_slice(&iv);
      let n2 = Nonce96.from_slice(&iv);
      match n1 {
        Err(e) => { return 2000 + code(e); }
        Ok(nonce1) => {
          match n2 {
            Err(e) => { return 2000 + code(e); }
            Ok(nonce2) => {
              let sealed = aes128_gcm_seal(&key, nonce1, &aad, &Secret.from_bytes(pb.clone()));
              match sealed {
                Err(e) => { return 3000 + code(e); }
                Ok(out) => {
                  let bad_seal = mismatch(&out, &want);
                  let opened = aes128_gcm_open(&key, &nonce2, &aad, &out);
                  match opened {
                    Err(e) => { return 4000 + code(e); }
                    Ok(secret) => {
                      let got = secret.expose();
                      return bad_seal * 10 + mismatch(&got, &wantp);
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

/// 与えた `sealed` を `open` する。成功なら 0、失敗なら `AeadError` の番号（1〜5）。
fn run_open(key: &[u8], iv: &[u8], aad: &[u8], sealed: &[u8]) -> i64 {
    let src = format!(
        "{HEAD}{MISMATCH_FN}{MAIN_HEAD}{}{}{}{}{}",
        build_bytes("kb", key),
        build_bytes("iv", iv),
        build_bytes("aad", aad),
        build_bytes("sb", sealed),
        "  let kr = Aes128GcmKey.from_secret(Secret.from_bytes(kb));
  match kr {
    Err(e) => { return 1000 + code(e); }
    Ok(key) => {
      match Nonce96.from_slice(&iv) {
        Err(e) => { return 2000 + code(e); }
        Ok(nonce) => {
          match aes128_gcm_open(&key, &nonce, &aad, &sb) {
            Err(e) => { return code(e); }
            Ok(secret) => { return 0; }
          }
        }
      }
    }
  }
}
"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

// ---------------------------------------------------------------------------
// 既知値: GCM 仕様書 付録 B のテストケース 1〜4
// ---------------------------------------------------------------------------

const TC3_K: &str = "feffe9928665731c6d6a8f9467308308";
const TC3_IV: &str = "cafebabefacedbaddecaf888";
const TC3_P: &str = "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255";
const TC3_C: &str = "42831ec2217774244b7221b784d0d49ce3aa212f2c02a4e035c17e2329aca12e21d514b25466931c7d8f6a5aac84aa051ba30b396a0aac973d58e091473f5985";
const TC4_A: &str = "feedfacedeadbeeffeedfacedeadbeefabaddad2";

fn hexcat(a: &str, b: &str) -> Vec<u8> {
    unhex(&format!("{a}{b}"))
}

#[test]
fn gcm_spec_case1_empty_plaintext_and_aad() {
    let sealed = unhex("58e2fccefa7e3061367f1d57a4e7455a");
    assert_eq!(run_roundtrip(&[0; 16], &[0; 12], &[], &[], &sealed), 0);
}

#[test]
fn gcm_spec_case2_one_zero_block() {
    let sealed = hexcat(
        "0388dace60b6a392f328c2b971b2fe78",
        "ab6e47d42cec13bdf53a67b21257bddf",
    );
    assert_eq!(run_roundtrip(&[0; 16], &[0; 12], &[], &[0; 16], &sealed), 0);
}

#[test]
fn gcm_spec_case3_four_blocks_no_aad() {
    let sealed = hexcat(TC3_C, "4d5c2af327cd64a62cf35abd2ba6fab4");
    assert_eq!(
        run_roundtrip(&unhex(TC3_K), &unhex(TC3_IV), &[], &unhex(TC3_P), &sealed),
        0
    );
}

#[test]
fn gcm_spec_case4_partial_block_with_aad() {
    let pt = unhex(TC3_P)[..60].to_vec();
    let ct = &TC3_C[..120];
    let sealed = hexcat(ct, "5bc94fbc3221a5db94fae95ae7121a47");
    assert_eq!(
        run_roundtrip(&unhex(TC3_K), &unhex(TC3_IV), &unhex(TC4_A), &pt, &sealed),
        0
    );
}

// ---------------------------------------------------------------------------
// ブロック境界の前後（OpenSSL で生成）
// ---------------------------------------------------------------------------

/// (seed, 平文長, AAD 長, 暗号文 + タグ)。鍵は gen(16, seed)、IV は gen(12, seed + 50)、
/// 平文は gen(pl, seed + 100)、AAD は gen(al, seed + 150)。
const BOUNDARY: &[(usize, usize, usize, &str)] = &[
    (2, 0, 5, "902397c8aed322a8773d2131dc870b5b"),
    (3, 1, 0, "d6e207e44bfa2fc525a708d199332884e0"),
    (4, 15, 0, "c6ead7a7b7318a564dc92605e8f4bdb93cf4db732ccb8eac40691c603d001f"),
    (5, 16, 16, "18a52c01ebe326fb8088751d329e83e52a25026c88147dff320426cff9b3722d"),
    (6, 17, 1, "b6b193f8f887df2732a208b3153616f93a53a09fb4bbe17a388f37ff02767a474d"),
    (7, 31, 15, "8ed44b02ffc539ec9a72a008596f191dec828ec7872fe062a73883dc7d3900e5c07e67caf8ddfd327308ae2850319f"),
    (8, 32, 17, "114d28d48af1ba0b31c98b92be5b99596aeb844ef84d66459a670b7fbf0b4c8871872acd6aeaee3fc5003fcf6a3b83fd"),
    (9, 33, 32, "fa1edc6e1d0482e1aea03ce86201ddebdf62a956daff364b7b6bdb2b2bbe552ccb769d07f9480f81856eb8e2babdd76fc2"),
];

#[test]
fn gcm_block_boundaries_match_openssl() {
    for (seed, pl, al, sealed_hex) in BOUNDARY {
        let r = run_roundtrip(
            &gen(16, *seed),
            &gen(12, seed + 50),
            &gen(*al, seed + 150),
            &gen(*pl, seed + 100),
            &unhex(sealed_hex),
        );
        assert_eq!(r, 0, "seed={seed} pt={pl} aad={al}");
    }
}

// ---------------------------------------------------------------------------
// 改変・短すぎる入力・長さの違反
// ---------------------------------------------------------------------------

fn case4() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let sealed = hexcat(&TC3_C[..120], "5bc94fbc3221a5db94fae95ae7121a47");
    (unhex(TC3_K), unhex(TC3_IV), unhex(TC4_A), sealed)
}

#[test]
fn open_accepts_the_untouched_input() {
    let (k, iv, a, s) = case4();
    assert_eq!(run_open(&k, &iv, &a, &s), 0);
}

#[test]
fn open_rejects_a_modified_ciphertext_tag_aad_or_nonce() {
    let (k, iv, a, s) = case4();
    let mut c = s.clone();
    c[0] ^= 1;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "暗号文の先頭");
    let mut c = s.clone();
    c[59] ^= 0x80;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "暗号文の末尾");
    let mut c = s.clone();
    c[60] ^= 1;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "タグの先頭");
    let mut c = s.clone();
    c[75] ^= 0x80;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "タグの末尾");
    let mut a2 = a.clone();
    a2[19] ^= 1;
    assert_eq!(run_open(&k, &iv, &a2, &s), 5, "AAD");
    let mut iv2 = iv.clone();
    iv2[11] ^= 1;
    assert_eq!(run_open(&k, &iv2, &a, &s), 5, "nonce");
    let mut k2 = k.clone();
    k2[0] ^= 1;
    assert_eq!(run_open(&k2, &iv, &a, &s), 5, "鍵");
    assert_eq!(run_open(&k, &iv, &a[..19], &s), 5, "AAD の短縮");
}

#[test]
fn open_rejects_inputs_shorter_than_the_tag() {
    let (k, iv, a, s) = case4();
    assert_eq!(run_open(&k, &iv, &a, &[]), 3);
    assert_eq!(run_open(&k, &iv, &a, &s[..15]), 3);
    // ちょうどタグだけ（空の平文）は短すぎではなく、認証に失敗する。
    assert_eq!(run_open(&k, &iv, &a, &s[..16]), 5);
}

#[test]
fn key_and_nonce_lengths_are_checked_at_construction() {
    let (_k, iv, a, s) = case4();
    assert_eq!(run_open(&[0; 15], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 17], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 0], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 16], &iv[..11], &a, &s), 2002);
    assert_eq!(run_open(&[0; 16], &[0; 13], &a, &s), 2002);
}

// ---------------------------------------------------------------------------
// FIPS 197 の AES 例（内部のブロック演算）
// ---------------------------------------------------------------------------

/// FIPS 197 付録 B・C.1 の例を、内部の `aes_expand_key`・`aes_encrypt_block` で照合する。
/// 一致しない数を返す（0 なら全一致）。
fn fips197_probe() -> String {
    let mut s = String::new();
    s.push_str("pub fn fips197_probe() -> Int\n  intent \"FIPS 197 の例を照合する\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n");
    for (i, (k, p, c)) in [
        (
            "2b7e151628aed2a6abf7158809cf4f3c",
            "3243f6a8885a308d313198a2e0370734",
            "3925841d02dc09fbdc118597196a0b32",
        ),
        (
            "000102030405060708090a0b0c0d0e0f",
            "00112233445566778899aabbccddeeff",
            "69c4e0d86a7b0430d8cdb78070b4c55a",
        ),
    ]
    .iter()
    .enumerate()
    {
        s.push_str(&build_bytes(&format!("k{i}"), &unhex(k)));
        s.push_str(&build_bytes(&format!("p{i}"), &unhex(p)));
        s.push_str(&format!(
            "  let rk{i} = aes_expand_key(&k{i});\n  let out{i} = aes_encrypt_block(&rk{i}, &p{i});\n"
        ));
        s.push_str(&build_expected(&format!("c{i}"), &unhex(c)));
        s.push_str(&format!(
            "  let mut j{i} = 0;\n  while j{i} < 16 {{\n    let g = out{i}.get(j{i}).unwrap_or(0 - 1);\n    let w = match c{i}.get(j{i}) {{ Some(v) => v, None => 0 }};\n    if g != w {{\n      bad = bad + 1;\n    }}\n    j{i} = j{i} + 1;\n  }}\n"
        ));
    }
    s.push_str("  return bad;\n}\n");
    s
}

#[test]
fn fips197_aes_examples_match_on_the_internal_block_function() {
    let module = format!(
        "{}\n{}",
        include_str!("../src/stdlib/crypto.sigil"),
        fips197_probe()
    );
    let entry = format!("{HEAD}pub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 7;\n}}\n");
    let o = Options {
        embedded_extra: vec![("std.crypto".to_string(), module)],
        embedded_roots: vec!["std.crypto".into()],
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(&[&entry], "std.crypto.fips197_probe", &[], &o)
        .unwrap_or_else(|f| panic!("{:?}", f.error));
    assert_eq!(r.value, "0");
}

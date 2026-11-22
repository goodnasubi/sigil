//! ADR 0058 段 1b: AES-128-GCM（`aes128_gcm_seal`・`aes128_gcm_open`）の利用者向けテスト。
//!
//! 出典と検算（ADR 0058 決定 4）:
//! - GCM の既知値は、NIST SP 800-38D の元になった仕様書（McGrew・Viega「The Galois/Counter
//!   Mode of Operation」付録 B）のテストケース 1〜4（鍵 128 ビット、96 ビット IV、128 ビットタグ）。
//!   NIST CAVP の GCM ベクタは、公式の配布元（csrc.nist.gov の gcmtestvectors.zip）へこの実行環境から
//!   到達できないため、同じ CAVS 14.0 のファイル（`gcmEncryptExtIV128.rsp`・`gcmDecrypt128.rsp`、
//!   96 ビット IV・128 ビットタグ・鍵 128 ビットの区画）を、pyca/cryptography が `vectors/` に収めた
//!   写しから取得して使う（取得物の SHA-256 は下の `CAVP_*` の説明に記す）。各行は、ファイル名・
//!   `[PTlen]`・`[AADlen]`・`Count` の ID つきで固定してあり、区画（PTlen 0/104/128/256/408 ×
//!   AADlen 0/128/160/384/720）ごとの最初の成功ケースと最初の `FAIL` ケースを選んだ。
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

// ---------------------------------------------------------------------------
// NIST CAVP GCM ベクタ（ADR 0058 決定 4）
// ---------------------------------------------------------------------------
//
// 出典: NIST CAVP の GCM テストベクタ（CAVS 14.0、2012-08-31 生成）。鍵 128 ビット・IV 96 ビット・
// タグ 128 ビットの区画だけを使う。取得元は pyca/cryptography の
// `vectors/cryptography_vectors/ciphers/AES/GCM/` にある同一ファイルの写し。
//   gcmEncryptExtIV128.rsp  SHA-256 30156236f768b0d81e58a3e5bd3bd79ce294cf9ef0ba0a84cf7b869c89ebd9fd
//   gcmDecrypt128.rsp       SHA-256 0d1f98d0c9e176ede1f3e85484dd13de92c3436ca97df378ec3481e04c19a732
// 各行は (ID, Key, IV, AAD, PT, CT, Tag)。復号側の成功ケースは PT と CT、`FAIL` ケースは CT と Tag を
// そのまま固定する（`FAIL` では PT を持たない）。

/// (ID, Key, IV, AAD, PT, CT, Tag)。すべて 16 進。
type CavpCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const CAVP_ENC: &[CavpCase] = &[
    ("gcmEncryptExtIV128.rsp [PTlen = 0] [AADlen = 0] Count = 0", "11754cd72aec309bf52f7687212e8957", "3c819d9a9bed087615030b65", "", "", "", "250327c674aaf477aef2675748cf6971"),
    ("gcmEncryptExtIV128.rsp [PTlen = 0] [AADlen = 128] Count = 0", "77be63708971c4e240d1cb79e8d77feb", "e0e00f19fed7ba0136a797f3", "7a43ec1d9c0a5a78a0b16533a6213cab", "", "", "209fcc8d3675ed938e9c7166709dd946"),
    ("gcmEncryptExtIV128.rsp [PTlen = 0] [AADlen = 160] Count = 0", "2fb45e5b8f993a2bfebc4b15b533e0b4", "5b05755f984d2b90f94b8027", "e85491b2202caf1d7dce03b97e09331c32473941", "", "", "c75b7832b2a2d9bd827412b6ef5769db"),
    ("gcmEncryptExtIV128.rsp [PTlen = 0] [AADlen = 384] Count = 0", "99e3e8793e686e571d8285c564f75e2b", "c2dd0ab868da6aa8ad9c0d23", "b668e42d4e444ca8b23cfdd95a9fedd5178aa521144890b093733cf5cf22526c5917ee476541809ac6867a8c399309fc", "", "", "3f4fba100eaf1f34b0baadaae9995d85"),
    ("gcmEncryptExtIV128.rsp [PTlen = 0] [AADlen = 720] Count = 0", "20b5b6b854e187b058a84d57bc1538b6", "94c1935afc061cbf254b936f", "ca418e71dbf810038174eaa3719b3fcb80531c7110ad9192d105eeaafa15b819ac005668752b344ed1b22faf77048baf03dbddb3b47d6b00e95c4f005e0cc9b7627ccafd3f21b3312aa8d91d3fa0893fe5bff7d44ca46f23afe0", "", "", "b37286ebaf4a54e0ffc2a1deafc9f6db"),
    ("gcmEncryptExtIV128.rsp [PTlen = 128] [AADlen = 0] Count = 0", "7fddb57453c241d03efbed3ac44e371c", "ee283a3fc75575e33efd4887", "", "d5de42b461646c255c87bd2962d3b9a2", "2ccda4a5415cb91e135c2a0f78c9b2fd", "b36d1df9b9d5e596f83e8b7f52971cb3"),
    ("gcmEncryptExtIV128.rsp [PTlen = 128] [AADlen = 128] Count = 0", "c939cc13397c1d37de6ae0e1cb7c423c", "b3d8cc017cbb89b39e0f67e2", "24825602bd12a984e0092d3e448eda5f", "c3b3c41f113a31b73d9a5cd432103069", "93fe7d9e9bfd10348a5606e5cafa7354", "0032a1dc85f1c9786925a2e71d8272dd"),
    ("gcmEncryptExtIV128.rsp [PTlen = 128] [AADlen = 160] Count = 0", "d4a22488f8dd1d5c6c19a7d6ca17964c", "f3d5837f22ac1a0425e0d1d5", "f1c5d424b83f96c6ad8cb28ca0d20e475e023b5a", "7b43016a16896497fb457be6d2a54122", "c2bd67eef5e95cac27e3b06e3031d0a8", "f23eacf9d1cdf8737726c58648826e9c"),
    ("gcmEncryptExtIV128.rsp [PTlen = 128] [AADlen = 384] Count = 0", "89850dd398e1f1e28443a33d40162664", "e462c58482fe8264aeeb7231", "d74e99d1bdaa712864eec422ac507bddbe2b0d4633cd3dff29ce5059b49fe868526c59a2a3a604457bc2afea866e7606", "2805cdefb3ef6cc35cd1f169f98da81a", "ba80e244b7fc9025cd031d0f63677e06", "d84a8c3eac57d1bb0e890a8f461d1065"),
    ("gcmEncryptExtIV128.rsp [PTlen = 128] [AADlen = 720] Count = 0", "bd7c5c63b7542b56a00ebe71336a1588", "87721f23ba9c3c8ea5571abc", "a6ec8075a0d3370eb7598918f3b93e48444751624997b899a87fa6a9939f844e008aa8b70e9f4c3b1a19d3286bf543e7127bfecba1ad17a5ec53fccc26faecacc4c75369498eaa7d706aef634d0009279b11e4ba6c993e5e9ed9", "de15ddbb1e202161e8a79af6a55ac6f3", "41eb28c0fee4d762de972361c863bc80", "9cb567220d0b252eb97bff46e4b00ff8"),
    ("gcmEncryptExtIV128.rsp [PTlen = 104] [AADlen = 0] Count = 0", "fe9bb47deb3a61e423c2231841cfd1fb", "4d328eb776f500a2f7fb47aa", "", "f1cc3818e421876bb6b8bbd6c9", "b88c5c1977b35b517b0aeae967", "43fd4727fe5cdb4b5b42818dea7ef8c9"),
    ("gcmEncryptExtIV128.rsp [PTlen = 104] [AADlen = 128] Count = 0", "dfefde23c6122bf0370ab5890e804b73", "92d6a8029990670f16de79e2", "a2b16d78251de6c191ce350e5c5ef242", "64260a8c287de978e96c7521d0", "bf78de948a847c173649d4b4d0", "9da3829968cdc50794d1c30d41cd4515"),
    ("gcmEncryptExtIV128.rsp [PTlen = 104] [AADlen = 160] Count = 0", "fe0121f42e599f88ff02a985403e19bb", "3bb9eb7724cbe1943d43de21", "2662d895035b6519f3510eae0faa3900ad23cfdf", "fd331ca8646091c29f21e5f0a1", "59fe29b07b0de8d869efbbd9b4", "d24c3e9c1c73c0af1097e26061c857de"),
    ("gcmEncryptExtIV128.rsp [PTlen = 104] [AADlen = 384] Count = 0", "cbd3b8dbfcfb11ce345706e6cd73881a", "dc62bb68d0ec9a5d759d6741", "0944b661fe6294f3c92abb087ec1b259b032dc4e0c5f28681cbe6e63c2178f474326f35ad3ca80c28e3485e7e5b252c8", "85f83bf598dfd55bc8bfde2a64", "206f6b3bb032dfecd39f8340b1", "425a21b2ea90580c889134032b914bb5"),
    ("gcmEncryptExtIV128.rsp [PTlen = 104] [AADlen = 720] Count = 0", "e5b1e7a94e9e1fda0873571eec713429", "5ddde829a81713346af8e5b7", "b0ce75da427fba93da6d3455b2b440a877599a6d8d6d2d66ee90b5cf9a33baaa8329a9ffaac290e8e33f2af2548c2a8a181b3d4d9f8fac860cc26b0d26b9cc53bc9f405afa73605ebeb376f2d1d7fcb065bab92f20f295556ade", "850069e5ed768b5dc9ed7ad485", "c211d9079d5562659db01e17d1", "884893fb035d3d7237d47c363de62bb3"),
    ("gcmEncryptExtIV128.rsp [PTlen = 256] [AADlen = 0] Count = 0", "9971071059abc009e4f2bd69869db338", "07a9a95ea3821e9c13c63251", "", "f54bc3501fed4f6f6dfb5ea80106df0bd836e6826225b75c0222f6e859b35983", "0556c159f84ef36cb1602b4526b12009c775611bffb64dc0d9ca9297cd2c6a01", "7870d9117f54811a346970f1de090c41"),
    ("gcmEncryptExtIV128.rsp [PTlen = 256] [AADlen = 128] Count = 0", "298efa1ccf29cf62ae6824bfc19557fc", "6f58a93fe1d207fae4ed2f6d", "021fafd238463973ffe80256e5b1c6b1", "cc38bccd6bc536ad919b1395f5d63801f99f8068d65ca5ac63872daf16b93901", "dfce4e9cd291103d7fe4e63351d9e79d3dfd391e3267104658212da96521b7db", "542465ef599316f73a7a560509a2d9f2"),
    ("gcmEncryptExtIV128.rsp [PTlen = 256] [AADlen = 160] Count = 0", "fedc7155192d00b23cdd98750db9ebba", "a76b74f55c1a1756a08338b1", "2ad206c4176e7e552aa08836886816fafa77e759", "6831435b8857daf1c513b148820d13b5a72cc490bda79a98a6f520d8763c39d1", "15823805da89a1923bfc1d6f87784d56bad1128b4dffdbdeefbb2fa562c35e68", "d23dc455ced49887c717e8eabeec2984"),
    ("gcmEncryptExtIV128.rsp [PTlen = 256] [AADlen = 384] Count = 0", "48b7f337cdf9252687ecc760bd8ec184", "3e894ebb16ce82a53c3e05b2", "7d924cfd37b3d046a96eb5e132042405c8731e06509787bbeb41f258275746495e884d69871f77634c584bb007312234", "bb2bac67a4709430c39c2eb9acfabc0d456c80d30aa1734e57997d548a8f0603", "d263228b8ce051f67e9baf1ce7df97d10cd5f3bc972362055130c7d13c3ab2e7", "71446737ca1fa92e6d026d7d2ed1aa9c"),
    ("gcmEncryptExtIV128.rsp [PTlen = 256] [AADlen = 720] Count = 0", "8fbf7ca12fd525dde91e625873fe51c2", "200bea517b9790a1cfadaf5e", "a414c07fe2e60bec9ccc409e9e899c6fe60580bb2607c861f7f08523e69cda1b9c3a711d1d9c35091771e4c950b9996d0ad04f2e00d1b3105853542a96e09ffffc2ec80f8cf88728f594f0aeb14f98a688234e8bfbf70327b364", "39d3e6277c4b4963840d1642e6faae0a5be2da97f61c4e55bb57ce021903d4c4", "fe678ef76f69ac95db553b6dadd5a07a9dc8e151fe6a9fa3a1cd621636b87868", "7c860774f88332b9a7ce6bbd0272a727"),
    ("gcmEncryptExtIV128.rsp [PTlen = 408] [AADlen = 0] Count = 0", "594157ec4693202b030f33798b07176d", "49b12054082660803a1df3df", "", "3feef98a976a1bd634f364ac428bb59cd51fb159ec1789946918dbd50ea6c9d594a3a31a5269b0da6936c29d063a5fa2cc8a1c", "c1b7a46a335f23d65b8db4008a49796906e225474f4fe7d39e55bf2efd97fd82d4167de082ae30fa01e465a601235d8d68bc69", "ba92d3661ce8b04687e8788d55417dc2"),
    ("gcmEncryptExtIV128.rsp [PTlen = 408] [AADlen = 128] Count = 0", "b61553bb854895b929751cd0c5f80384", "8863f999ae64e55d0bbd7457", "d914b5f2d1b08ce53ea59cb310587245", "9b1b113217d0c4ea7943cf123c69c6ad2e3c97368c51c9754145d155dde1ee8640c8cafff17a5c9737d26a137eee4bf369096d", "acfab4632b8a25805112f13d85e082bc89dc49bd92164fa8a2dad242c3a1b2f2696f2fdff579025f3f146ea97da3e47dc34b65", "5d9b5f4a9868c1c69cbd6fd851f01340"),
    ("gcmEncryptExtIV128.rsp [PTlen = 408] [AADlen = 160] Count = 0", "fe47fcce5fc32665d2ae399e4eec72ba", "5adb9609dbaeb58cbd6e7275", "88319d6e1d3ffa5f987199166c8a9b56c2aeba5a", "7c0e88c88899a779228465074797cd4c2e1498d259b54390b85e3eef1c02df60e743f1b840382c4bccaf3bafb4ca8429bea063", "98f4826f05a265e6dd2be82db241c0fbbbf9ffb1c173aa83964b7cf5393043736365253ddbc5db8778371495da76d269e5db3e", "291ef1982e4defedaa2249f898556b47"),
    ("gcmEncryptExtIV128.rsp [PTlen = 408] [AADlen = 384] Count = 0", "3c50622868f450aa0928990c15e1eb36", "811d5290768d57e7d87bb6c7", "dae2c7e0a3d3fd2bc04eca19b15178a003b5cf84890c28c2a615f20f8adb427f70698c12b2ef87780c1193fbb8cd1674", "edd0a8f82833e919740fe2bf9edecf4ac86c72dc89490cef7b6983aaaf99fc856c5cc87d63f98a7c861bf3271fea6da86a15ab", "a51425b0608d3b4b46d4ec05ca1ddaf02bdd2089ae0554ecfb2a1c84c63d82dc71ddb9ab1b1f0b49de2ad27c2b5173e7000aa6", "bd9b5efca48008cd973a4f7d2c723844"),
    ("gcmEncryptExtIV128.rsp [PTlen = 408] [AADlen = 720] Count = 0", "2c1f21cf0f6fb3661943155c3e3d8492", "23cb5ff362e22426984d1907", "5d3624879d35e46849953e45a32a624d6a6c536ed9857c613b572b0333e701557a713e3f010ecdf9a6bd6c9e3e44b065208645aff4aabee611b391528514170084ccf587177f4488f33cfb5e979e42b6e1cfc0a60238982a7aec", "42f758836986954db44bf37c6ef5e4ac0adaf38f27252a1b82d02ea949c8a1a2dbc0d68b5615ba7c1220ff6510e259f06655d8", "81824f0e0d523db30d3da369fdc0d60894c7a0a20646dd015073ad2732bd989b14a222b6ad57af43e1895df9dca2a5344a62cc", "57a3ee28136e94c74838997ae9823f3a"),
];

const CAVP_DEC_PASS: &[CavpCase] = &[
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 0] Count = 0", "cf063a34d4a9a76c2c86787d3f96db71", "113b9785971864c83b01c787", "", "", "", "72ac8493e3a5228b5d130a69d2510e42"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 128] Count = 1", "2370e320d4344208e0ff5683f243b213", "04dbb82f044d30831c441228", "d43a8e5089eea0d026c03a85178b27da", "", "", "2a049c049d25aa95969b451d93c31c6e"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 160] Count = 2", "5211242698bed4774a090620a6ca56f3", "b1e1349120b6e832ef976f5d", "b6d729aab8e6416d7002b9faa794c410d8d2f193", "", "", "0f247e7f9c2505de374006738018493b"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 384] Count = 1", "762da58bb000f5d499818bc859989a30", "584c291ff1aa388a5112521e", "91f92e8bbda7b5ec967ade766f4f26e9189eaafad416f37b4891d3e37d70cb9a267aa843dd202858ade020261223dfce", "", "", "9ac7eb2d762facae06086c957959880e"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 720] Count = 1", "8dc7a59a71cd76499de67feac53c786a", "beb45b75ea5b7750e69a409d", "8bade18807ac5fe59cfd34d7eed8ae33dfeeb960d2a15693837f22dd98f68715079d358a49a04a7b231d486ae45f4cd6c0fe3cde6c0d967ce8d84bbf2e4be2db793afb72ea83c7b21a73f65742c214768f7e6133e3814059efb6", "", "", "46e8457d8d409f2a0690e6502f8b7a87"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 0] Count = 0", "e98b72a9881a84ca6b76e0f43e68647a", "8b23299fde174053f3d652ba", "", "28286a321293253c3e0aa2704a278032", "5a3c1cf1985dbb8bed818036fdd5ab42", "23c7ab0f952b7091cd324835043b5eb5"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 128] Count = 0", "816e39070410cf2184904da03ea5075a", "32c367a3362613b27fc3e67e", "f2a30728ed874ee02983c294435d3c16", "ecafe96c67a1646744f1c891f5e69427", "552ebe012e7bcf90fcef712f8344e8f1", "ecaae9fc68276a45ab0ca3cb9dd9539f"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 160] Count = 1", "d9529840200e1c17725ab52c9c927637", "6e9a639d4aecc25530a8ad75", "472a6f4e7771ca391e42065030db3ff418f3b636", "8ae823895ee4e7f08bc8bad04d63c220", "6c779895e78179783c51ade1926436b9", "4522bfdef4a635a38db5784b27d43661"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 384] Count = 2", "abbc49ee0bbe3d81afc2b6b84f70b748", "f11db9f7b99a59ed59ade66f", "d533cf7644a48da46fcdec47ae5c77b9b52db775d6c886896e4f4e00c51affd59499a0e572f324989df511c4ea5f93cd", "5135ba1354cbb80478ecaf3db38a443f", "ce2d76f834942c022044eebc91b461c0", "62df4b04f219554cd3e69d3c870032d2"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 720] Count = 1", "300b8ffab4368cc90f6d4063e4279f2a", "8e69fa64e871d0e98a183a49", "5166309e153447b27c67051453abf441de3f4a7f6b633ec6122ff82dc132cfb422d36c5ec6e7cc90a9ad55caa1ccdcb82dc5022a20062a9c6e9238f34d085b1f554b5eac05eff25b5a5cb6e18e7827d70175dc0662d77033d118", "4953b54859870631e818da71fc69c981", "2d2292da61c280aff86767d25b75e814", "633ee657a8981a7682f87505594c95ad"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 0] Count = 0", "387218b246c1a8257748b56980e50c94", "dd7e014198672be39f95b69d", "", "48f5b426baca03064554cc2b30", "cdba9e73eaf3d38eceb2b04a8d", "ecf90f4a47c9c626d6fb2c765d201556"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 128] Count = 0", "660eb76f3d8b6ec54e01b8a36263124b", "3d8cf16e262880ddfe0c86eb", "8560b10c011a1d4190eb46a3692daa17", "2efbaedfec3cfe4ac32f201fa5", "b1ee05f1415a61d7637e97c5f3", "761cb84a963e1db1a4ab2c5f904c09db"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 160] Count = 0", "c62dc36b9230e739179f3c58e7270ff9", "196a0572d8ff2fbd3522b6a5", "4d10536cbdbd6f1d38b2bd10ab8c1c29ae68138e", "6d8abcee45667e7a9443896cbf", "958062b331f05b3acaa1836fc2", "dc65a20d9a9aec2e7699eaead47afb42"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 384] Count = 0", "ef1da9dd794219ebf8f717d5a98ab0af", "3f3983dc63986e33d1b6bffc", "5abd0c1c52b687e9a1673b69137895e5025c2bd18cbeacdb9472e918fe1587da558c492cc708d270fd10572eea83d2de", "5511872905436c7de38e9501e7", "95ea05701481e915c72446c876", "5c866992662005ca8886810e278c8ab4"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 720] Count = 2", "77b55a5b37690c9b1b01a05820838e3e", "7a8e0d881f023a9954941037", "0bb1ad1d294b927c24ee097d0a9afbaa6a62c8923627b50bd96e5ba852509a2e76f7a10ee3987e37a55b92d08531897e6cd76462403b39fb31508cc9fc7684ab5ec2ccc73e8a7f4104a277319bf207fcf263eceed13a76ca177f", "d164aeccec7dbcadee4f41b6a9", "e0eb3359e443e1108ed4068969", "ea6383077d15d7d0a97220848a7616a9"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 0] Count = 0", "bfd414a6212958a607a0f5d3ab48471d", "86d8ea0ab8e40dcc481cd0e2", "", "a6b76a066e63392c9443e60272ceaeb9d25c991b0f2e55e2804e168c05ea591a", "62171db33193292d930bf6647347652c1ef33316d7feca99d54f1db4fcf513f8", "c28280aa5c6c7a8bd366f28c1cfd1f6e"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 128] Count = 2", "95bcde70c094f04e3dd8259cafd88ce8", "12cf097ad22380432ff40a5c", "c783a0cca10a8d9fb8d27d69659463f2", "32f51e837a9748838925066d69e87180f34a6437e6b396e5643b34cb2ee4f7b1", "8a023ba477f5b809bddcda8f55e09064d6d88aaec99c1e141212ea5b08503660", "562f500dae635d60a769b466e15acd1e"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 160] Count = 0", "f3e60720c7eff3af96a0e7b2a359c322", "8c9cb6af794f8c0fc4c8c06e", "5d15b60acc008f9308731ea0a3098644866fa862", "7e299a25404311ee29eee9349f1e7f876dca42ba81f44295bb9b3a152a27a2af", "73e308d968ead96cefc9337dea6952ac3afbe39d7d14d063b9f59ab89c3f6acc", "658e311f9c9816dbf2567f811e905ab8"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 384] Count = 0", "8453cf505f22445634b18680c1f6b0f3", "fab8e5ce90102286182ef690", "ff76d2210f2caec37490a19352c3945be1c5facb89cb3e9947754cade47ec932d95c88d7d2299a8b6db76b5144ab9516", "e4abb4773f5cc51c9df6322612d75f70696c17733ce41e22427250ae61fd90d3", "5475442af3ba2bd865ae082bc5e92ad7f42cd84b8c64daadcf18f0d4863b6172", "972a7e85787ba26c626db1a1e7c13acb"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 720] Count = 0", "07a6be880a58f572dbc2ad74a56db8b6", "95fc6654e6dc3a8adf5e7a69", "de4269feea1a439d6e8990fd6f9f9d5bc67935294425255ea89b6f6772d680fd656b06581a5d8bc5c017ab532b4a9b83a55fde58cdfb3d2a8fef3aa426bc59d3e32f09d3cc20b1ceb9a9e349d1068a0aa3d39617fae0582ccef0", "7680b48b5d28f38cdeab2d5851769394a3e141b990ec4bdf79a33e5315ac0338", "095635c7e0eac0fc1059e67e1a936b6f72671121f96699fed520e5f8aff777f0", "b2235f6d4bdd7b9c0901711048859d47"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 0] Count = 1", "93ae114052b7985d409a39a40df8c7ee", "8ad733a4a9b8330690238c42", "", "3f3bb0644eac878b97d990d257f5b36e1793490dbc13fea4efe9822cebba7444cce4dee5a7f5dfdf285f96785792812200c279", "bbb5b672a479afca2b11adb0a4c762b698dd565908fee1d101f6a01d63332c91b85d7f03ac48a477897d512b4572f9042cb7ea", "4d78bdcb1366fcba02fdccee57e1ff44"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 128] Count = 0", "bc22f3f05cc40db9311e4192966fee92", "134988e662343c06d3ab83db", "10087e6ed81049b509c31d12fee88c64", "337c1bc992386cf0f957617fe4d5ec1218ae1cc40369305518eb177e9b15c1646b142ff71237efaa58790080cd82e8848b295c", "4c0168ab95d3a10ef25e5924108389365c67d97778995892d9fd46897384af61fc559212b3267e90fe4df7bfd1fbed46f4b9ee", "771357958a316f166bd0dacc98ea801a"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 160] Count = 0", "af57f42c60c0fc5a09adb81ab86ca1c3", "a2dc01871f37025dc0fc9a79", "41dc38988945fcb44faf2ef72d0061289ef8efd8", "3803a0727eeb0ade441e0ec107161ded2d425ec0d102f21f51bf2cf9947c7ec4aa72795b2f69b041596e8817d0a3c16f8fadeb", "b9a535864f48ea7b6b1367914978f9bfa087d854bb0e269bed8d279d2eea1210e48947338b22f9bad09093276a331e9c79c7f4", "4f71e72bde0018f555c5adcce062e005"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 384] Count = 1", "f0305c7b513960533519473976f02beb", "1a7f6ea0e6c9aa5cf8b78b09", "7e2071cc1c70719143981de543cd28dbceb92de0d6021bda4417e7b6417938b126632ecff6e00766e5d0aad3d6f06811", "e5fc990c0739e05bd4655871c7401128117737a11d520372239ab723f7fde78dc4212ac565ee5ee100a014dbb71ea13cdb08eb", "30043bcbe2177ab25e4b00a92ee1cd80e9daaea0bc0a827fc5fcb84e7b07be6395582a5a14e768dde80a20dae0a8b1d8d1d29b", "796c41624f6c3cab762380d21ab6130b"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 720] Count = 0", "da2bb7d581493d692380c77105590201", "44aa3e7856ca279d2eb020c6", "4cd171b23bddb3a53cdf959d5c1710b481eb3785a90eb20a2345ee00d0bb7868c367ab12e6f4dd1dee72af4eee1d197777d1d6499cc541f34edbf45cda6ef90b3c024f9272d72ec1909fb8fba7db88a4d6f7d3d925980f9f9f72", "9bbf4c1a2742f6ac80cb4e8a052e4a8f4f07c43602361355b717381edf9fabd4cb7e3ad65dbd1378b196ac270588dd0621f642", "9290d430c9e89c37f0446dbd620c9a6b34b1274aeb6f911f75867efcf95b6feda69f1af4ee16c761b3c9aeac3da03aa9889c88", "9e3ac938d3eb0cadd6f5c9e35d22ba38"),
];

const CAVP_DEC_FAIL: &[CavpCase] = &[
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 0] Count = 1", "a49a5e26a2f8cb63d05546c2a62f5343", "907763b19b9b4ab6bd4f0281", "", "", "", "a2be08210d8c470a8df6e8fbd79ec5cf"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 128] Count = 0", "d1f6af919cde85661208bdce0c27cb22", "898c6929b435017bf031c3c5", "7c5faa40e636bbc91107e68010c92b9f", "", "", "ae45f11777540a2caeb128be8092468a"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 160] Count = 0", "b1089cecac8e1a56441353857246773c", "1af1e6f40b1b21fcb38f65a5", "ed825cbf40e8f8f198ce1afd1141a3cf391cee73", "", "", "0edb6756481560956d506a8cdb082db0"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 384] Count = 0", "748146a0b1d5e9b6facdd95e8a3295be", "99e8394910c21207541ca90c", "5ce0d8dbac8a82fa8dd7aec334b81301b31ab49f3b40a1857cc540a1d992f27f80945dfcb4730dc81ddfd32dee1825d6", "", "", "847292904038c42168adf62011dc790e"),
    ("gcmDecrypt128.rsp [PTlen = 0] [AADlen = 720] Count = 0", "c1e007d318d0e5c87d28fd890dfb04ba", "d7bc58d494491ff57639b60b", "10c68681c6d700ffc72ea9a790cdc9ddc1904c7e886e23ca166dbfa364a4c5f95205bdd1f22eeebefbbb9375f1a416ec7faed6cfdf706085f8fd2632c7a261a78875811f17fba19baf905f0aa623e67175f2158cda313a5047e3", "", "", "2f1ede16f025bc8c5421854b5f0109d1"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 0] Count = 2", "5164df856f1e9cac04a79b808dc5be39", "e76925d5355e0584ce871b2b", "", "", "0216c899c88d6e32c958c7e553daa5bc", "a145319896329c96df291f64efbe0e3a"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 128] Count = 1", "867fc5d5476d5008f0703d81e3622255", "22945529dff947c3c9264df7", "261a9efd4f32bc3d07c115b4edcf8adf", "", "1c785025e5a2678e4b29b29276e395bb", "87fdf1261846164a950c37a3f2eea17d"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 160] Count = 0", "4cf56dd7eb109452d7445e79bcad4f86", "5f3724b27c345ed5d1757e9e", "20fe2aa2641db7318aa5e339b1e303f952b05dc8", "", "2d0581c040f4e2bb74423682113b818c", "f090c0cba2151af2e78ef407720233db"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 384] Count = 0", "527da7a4f81d6d4f6f395b623aedac3d", "2acf3f0c64f5ab9282a6f4a9", "e5492270c1df154e594c689fc505575c6bf5d7dbde0d1ded5ed32ec65e3e8289072bb10c35e385f499c0600a58f721b2", "", "ef47b52ebc6d4cc0dfaf09d22a587c73", "0530cfe49d93d78bfa9d54ffa6d148ae"),
    ("gcmDecrypt128.rsp [PTlen = 128] [AADlen = 720] Count = 0", "99a9bddbb68be243759bc01c882c4ce7", "6d78e303d165a42082abf5a2", "80a8d9fdefb4b114f12208b0a79299ab5e2c8524d2991617f29b23b31cedef24226fadc72b3475c0f201ea0648aa4936ffff9c80f08a329e042cc23728e0b1569357bfd08e9af1b77369bce491b995f7393903c3eceb3fe0c0e4", "", "1e3a3fdb540717afe12651d3a51b4340", "66dbb3bd4b766df31d37a0282876276c"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 0] Count = 3", "7d7b6c988137b8d470c57bf674a09c87", "9edf2aa970d016ac962e1fd8", "", "", "a85b66c3cb5eab91d5bdc8bc0e", "dc054efc01f3afd21d9c2484819f569a"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 128] Count = 1", "2aa3dfcef68c333d0f03cbbb60a2dac8", "667a61d732780f5694128daf", "227f0dddc71aa2cc20c901fcb8db0e15", "", "9fade7c433cb44d5ebec14095d", "85f30c02962900f8891a9b6987172256"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 160] Count = 1", "0c966ae197bd94cdabcab5f9c4da02cb", "c5bcc7e7044169fa01519ab7", "5feaf91a64887463a0b3299da6c87cf2874ebf0b", "", "49c9a9b0c3980e443cd5195a16", "9843f2181d3db30d1c7694c511a85440"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 384] Count = 1", "2e3968148b973cb79f275676367cd799", "b055618bdaf94170080346ae", "72c50abc8c10b7a83b755ac26b7a86afb37d2310e832a8982c70ea8950f8c1a0c0b01fee79644753f87a553868dcc719", "", "0117a142931082ff569aa58ea1", "db9143f6cdb3aa57c2aab1bab89e6c26"),
    ("gcmDecrypt128.rsp [PTlen = 104] [AADlen = 720] Count = 0", "3eeaa3b5aa99f6902d1d58421b53ddb1", "71240c996af69d590ccc8e8d", "b1176b9e28794cb627c5cb3743846e6d11339ce6d0ce69789833b9f344e92e5360a0eb61b173687f74eae3184ef641f1c42c5b180014ca7e6b0edb00fbc35f2efa7aac7111dd8ec70e851a3ef2e2436ee07e4fba213425b1a8e4", "", "9a50a33a9438f485e7c89caba7", "e2865b861f8d5e0af25757905dd0653d"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 0] Count = 1", "c3aa0400ea3f5107fbe8aa011e1391b3", "9c22008475e7a907f7abc966", "", "", "96a06e1fdb0082ebc6441312dd9cd6ed6f7fac745facc368b5ab2a3951bd75a6", "6e84d66448068f67889404263a047a88"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 128] Count = 0", "3308bf978dca6bd7f5fe4ccd6d0e7c24", "85996c785a83493cc7165c11", "b0ebb26b9e27c44c26543e257b7c7eb4", "", "4c8f0c7ad73fc469b4b385f80741f3014ec236e29ca9068b8e55ed6fb2fc9b9b", "8b5528d6ef87eb0be9b6708fab7d654c"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 160] Count = 2", "aa6d3c010d93c6ce4d574b5f4017b8b3", "9480e879335e1c2f2f0b1201", "5c03cbcbb53d7b44a47e247c0c8b77e5603552c7", "", "752ffdd7014ef7dbb269b9e43d241ca94b578c1ee2052b26225450ba2dcdd114", "63d1a0a5faf1cfa9f72bcec227169fbc"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 384] Count = 2", "ab124c19896f9a725cd84bc65c0b769d", "8be6b8884adb0acb19776719", "f3f8c4d2edf80986c33940a9dfce5c64ac314d9969bad730bb719a4a39551eda6b771b6941ba301a4bcf2c7e69c8a000", "", "a77bc17b558e829efdf4508c62761520a44920840c4d8e8cad0564931ffb6f0f", "890106f841f36315a7fed2ffb57f2dbb"),
    ("gcmDecrypt128.rsp [PTlen = 256] [AADlen = 720] Count = 4", "291567951e7d6e7442261b66372b3a6c", "5a60ab4b00333bb34e292511", "48055efab58264015eb49fe444de55edca701e09b679ec042abcdcff978e2b7eb74d0be0a84c858301cc415f5159f2a531138109fa5c559e770e6040f38ccf9302b790282458bbf99dbb494ae5c348ee4ab0f77d63b9dba6de56", "", "0de6fe0eb4d94284388068226a84d6f86f181e320840d3f28a2bf6b92a17f7ea", "38af26303a1ac4f8a5cb58420fd34803"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 0] Count = 0", "f3c4c058c7f0965a0328f0f011a128e0", "e96098c39d9aa83522119355", "", "", "b42c25155820564b41c525c6aa8ff4363c7c511917772b9bcd574b0be1f6b4ca2153869ddf0c4f76dfad3aef0a76ce056582f9", "44331e39b8a66a81591ed155d7d32372"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 128] Count = 1", "4384629e9b1686927e8508243be1d01a", "84ff7f4feddde7d89d370e7d", "a51cebf08d0f293e3a83bea5a9f1cc6d", "", "8f385490e9622a6c1009900052385c23442890d2af667b210d012781e62d0de311f2149cc377a490afcc686cd92e7c35dc7e5c", "00097982ccc985671477cb1f1a250bd4"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 160] Count = 2", "52bdbbf9cf477f187ec010589cb39d58", "d3be36d3393134951d324b31", "93c4fc6a4135f54d640b0c976bf755a06a292c33", "", "700188da144fa692cf46e4a8499510a53d90903c967f7f13e8a1bd8151a74adc4fe63e32b992760b3a5f99e9a47838867000a9", "8ca4e38aa3dfa6b1d0297021ccf3ea5f"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 384] Count = 0", "e7c650a8f64a900857eaf8d95c49a807", "961a0b83a36b79acc1494da5", "da9ff7c1529f9cb75c88cd890c9a1af493db5fe8148c4687984c070442c5d501434bbaab1dae06cec6de0606d59da3d0", "", "70954661360ee1dcc5ea22918fa89bbcf56aa6ded04b22fccff799d773833617c9aa7ddb0ab9c170d6a44769bce9f7fe287c68", "a0962c9259e7d4a9a21e9328e93c754c"),
    ("gcmDecrypt128.rsp [PTlen = 408] [AADlen = 720] Count = 2", "1986310c725ac94ecfe6422e75fc3ee7", "93ec4214fa8e6dc4e3afc775", "e80b88e62c49c958b5e0b8b54f532d9ff6aa84c8a40132e93e55b59fc24e8decf28463139f155d1e8ce4ee76aaeefcd245baa0fc519f83a5fb9ad9aa40c4b21126013f576c4272c2cb136c8fd091cc4539877a5d1e72d607f960", "", "b178ec72f85a311ac4168f42a4b2c23113fbea4b85f4b9dabb74e143eb1b8b0a361e0243edfd365b90d5b325950df0ada058f9", "8b347853f11d75e81e8a95010be81f17"),
];

/// 暗号化: 暗号文とタグが CAVP の値と一致し、復号が平文へ戻る。
fn check_cavp_roundtrip(cases: &[CavpCase]) {
    for (id, key, iv, aad, pt, ct, tag) in cases {
        let sealed = hexcat(ct, tag);
        assert_eq!(
            run_roundtrip(&unhex(key), &unhex(iv), &unhex(aad), &unhex(pt), &sealed),
            0,
            "{id}"
        );
    }
}

#[test]
fn cavp_gcm_encrypt_ciphertext_and_tag_match() {
    check_cavp_roundtrip(CAVP_ENC);
}

#[test]
fn cavp_gcm_decrypt_pass_cases_recover_plaintext() {
    check_cavp_roundtrip(CAVP_DEC_PASS);
}

#[test]
fn cavp_gcm_decrypt_fail_cases_report_authentication_failed() {
    for (id, key, iv, aad, _pt, ct, tag) in CAVP_DEC_FAIL {
        let sealed = hexcat(ct, tag);
        // AeadError.AuthenticationFailed の番号は 5。
        assert_eq!(
            run_open(&unhex(key), &unhex(iv), &unhex(aad), &sealed),
            5,
            "{id}"
        );
    }
}

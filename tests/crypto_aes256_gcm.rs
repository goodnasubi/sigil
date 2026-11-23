//! ADR 0058 段 2c: AES-256-GCM（`aes256_gcm_seal`・`aes256_gcm_open`）の利用者向けテスト。
//!
//! 出典と検算（ADR 0058 決定 4）:
//! - GCM の既知値は McGrew・Viega「The Galois/Counter Mode of Operation」付録 B のテストケース
//!   13〜16（鍵 256 ビット、96 ビット IV、128 ビットタグ）。Python の `cryptography` 50.0.1
//!   （OpenSSL）の AESGCM で独立に再計算して一致を確認した。
//! - NIST CAVP の GCM ベクタ（CAVS 14.0）は csrc.nist.gov へこの実行環境から到達できないため、
//!   pyca/cryptography が `vectors/cryptography_vectors/ciphers/AES/GCM/` に収めた同一ファイル
//!   `gcmEncryptExtIV256.rsp`・`gcmDecrypt256.rsp`（鍵 256 ビット・IV 96 ビット・タグ 128 ビット）の
//!   写しを使う。区画（PTlen 0/104/128/256/408 × AADlen 0/128/160/384/720）ごとの最初の成功ケースと
//!   最初の `FAIL` ケースを選んだ。各ケースは `cryptography` でも復号／認証失敗を確認した。
//! - 追加のケース（平文 0〜65 バイト、AAD 0〜33 バイト。ブロック境界の前後を含む）は AESGCM で生成した
//!   （入力は決定的な並び `(seed*31 + i*7 + 3) % 256`）。
//! - FIPS 197 付録 C.3（AES-256）と鍵展開は、公開 API から見えないため `embedded_extra` で
//!   `std.crypto` に試験用の入口を足した版を評価器で動かして照合する。
//! - 組合せ確認: HKDF-SHA-384 で導出した 32 バイト鍵と 12 バイト nonce で AES-256-GCM を
//!   seal/open し、`cryptography` の HKDF(SHA384) と AESGCM による独立な期待値と一致させる。
//! - 公開 API のテストは評価器とネイティブの両方で回る（`common::run` が突き合わせる）。
//!
//! 長さの上限（平文 2^36 − 32 バイト、AAD 2^61 − 1 バイト）は、超える長さのバイト列を
//! この環境では作れないため、実際に超える入力での試験はしていない（PR 本文に記載）。

mod common;

use sigil::exec::{Arg, Options};

const HEAD: &str = "module crypto_aes256_gcm.t\n  targets server\n  trust none\n\n";

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
        "  let kr = Aes256GcmKey.from_secret(Secret.from_bytes(kb));
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
              let sealed = aes256_gcm_seal(&key, nonce1, &aad, &Secret.from_bytes(pb.clone()));
              match sealed {
                Err(e) => { return 3000 + code(e); }
                Ok(out) => {
                  let bad_seal = mismatch(&out, &want);
                  let opened = aes256_gcm_open(&key, &nonce2, &aad, &out);
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
        "  let kr = Aes256GcmKey.from_secret(Secret.from_bytes(kb));
  match kr {
    Err(e) => { return 1000 + code(e); }
    Ok(key) => {
      match Nonce96.from_slice(&iv) {
        Err(e) => { return 2000 + code(e); }
        Ok(nonce) => {
          match aes256_gcm_open(&key, &nonce, &aad, &sb) {
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
// 既知値: GCM 仕様書 付録 B のテストケース 13〜16
// ---------------------------------------------------------------------------

const TC15_K: &str = "feffe9928665731c6d6a8f9467308308feffe9928665731c6d6a8f9467308308";
const TC3_IV: &str = "cafebabefacedbaddecaf888";
const TC3_P: &str = "d9313225f88406e5a55909c5aff5269a86a7a9531534f7da2e4c303d8a318a721c3c0c95956809532fcf0e2449a6b525b16aedf5aa0de657ba637b391aafd255";
const TC15_C: &str = "522dc1f099567d07f47f37a32a84427d643a8cdcbfe5c0c97598a2bd2555d1aa8cb08e48590dbb3da7b08b1056828838c5f61e6393ba7a0abcc9f662898015ad";
const TC16_A: &str = "feedfacedeadbeeffeedfacedeadbeefabaddad2";

fn hexcat(a: &str, b: &str) -> Vec<u8> {
    unhex(&format!("{a}{b}"))
}

#[test]
fn gcm_spec_case13_empty_plaintext_and_aad() {
    let sealed = unhex("530f8afbc74536b9a963b4f1c4cb738b");
    assert_eq!(run_roundtrip(&[0; 32], &[0; 12], &[], &[], &sealed), 0);
}

#[test]
fn gcm_spec_case14_one_zero_block() {
    let sealed = hexcat(
        "cea7403d4d606b6e074ec5d3baf39d18",
        "d0d1c8a799996bf0265b98b5d48ab919",
    );
    assert_eq!(run_roundtrip(&[0; 32], &[0; 12], &[], &[0; 16], &sealed), 0);
}

#[test]
fn gcm_spec_case15_four_blocks_no_aad() {
    let sealed = hexcat(TC15_C, "b094dac5d93471bdec1a502270e3cc6c");
    assert_eq!(
        run_roundtrip(&unhex(TC15_K), &unhex(TC3_IV), &[], &unhex(TC3_P), &sealed),
        0
    );
}

#[test]
fn gcm_spec_case16_partial_block_with_aad() {
    let pt = unhex(TC3_P)[..60].to_vec();
    let sealed = hexcat(&TC15_C[..120], "76fc6ece0f4e1768cddf8853bb2d551b");
    assert_eq!(
        run_roundtrip(&unhex(TC15_K), &unhex(TC3_IV), &unhex(TC16_A), &pt, &sealed),
        0
    );
}

// ---------------------------------------------------------------------------
// ブロック境界の前後（OpenSSL で生成）
// ---------------------------------------------------------------------------

/// (seed, 平文長, AAD 長, 暗号文 + タグ)。鍵は gen(32, seed)、IV は gen(12, seed + 50)、
/// 平文は gen(pl, seed + 100)、AAD は gen(al, seed + 150)。
const BOUNDARY: &[(usize, usize, usize, &str)] = &[
    (2, 0, 5, "b71a07bf2b7544e9de5c6d4fe6e74609"),
    (3, 1, 0, "7af51ee9037c85783865e15fb349096cde"),
    (4, 15, 0, "7e4cc24c1565a131376cb39359cb1cf7d0d83ebe86be0d1f602fbfebd4169c"),
    (5, 16, 16, "3b5356205d9bf6671fe5bede34830bf9a9466a0bf72c8e9c8136ffc04c311bd9"),
    (6, 17, 1, "e8181f761c1682d608fb0ac3f372e93ff60baa166e5329136079b7be4352dfc8a5"),
    (7, 31, 15, "0974808ee5295defd15fb8c6ef4bfa17c4a893291744cb5e1175bb2016f9a73c2e7a6e56576bb1a37f4313f83d8941"),
    (8, 32, 17, "53efa205859ddf494b56c0052cd214d55c19c0d3ef801d197591b5c41c5fc399ad08da6867f3ff552415d459206bc37c"),
    (9, 33, 32, "ee3665e8263c627facbcee32fcf9587c3b035d4f3974d52e9a0924e379881bd6ed024676bd500b081e7d8c9da26efd3874"),
    (10, 47, 16, "96b33f9a1f1bf312abf13bd2401752c778e0dabcfb3881801de6cc1e1cb7e905c64947aa8c942d7ab9402484f11269c8f5a0ce0ad536a853afa6f1d4d67201"),
    (11, 48, 0, "81b901fb7b1ce59f7b2d959084cc024ee7a0db47ade4ee200e47596714e8cec5880c48e4b6baa9345107c8890ebfa02c43232ff47168f6dc2f87810fb6d6cc35"),
    (12, 64, 33, "96d85518b9cb81ca7fb501fa6ed7b955d307979f8678108021a419216df54c941454231ffab17b8f6fdd6b502e584e84d561d084df2b35b64fd7c6030f4cfcd762740161dc9accbbed0dc65ca4838f1d"),
    (13, 65, 12, "4e429aadc5d8d4233ce3269661bed16314ca0b981e3d1ec55887e5ed16ab4109c8db663358dba8cd8c87f84cbe4fe631d2692a393560bc002215fb588bda7a4948674cf6e26cd834cc17881251735cddec"),
];

#[test]
fn gcm_block_boundaries_match_openssl() {
    for (seed, pl, al, sealed_hex) in BOUNDARY {
        let r = run_roundtrip(
            &gen(32, *seed),
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

fn case16() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let sealed = hexcat(&TC15_C[..120], "76fc6ece0f4e1768cddf8853bb2d551b");
    (unhex(TC15_K), unhex(TC3_IV), unhex(TC16_A), sealed)
}

#[test]
fn open_accepts_the_untouched_input() {
    let (k, iv, a, s) = case16();
    assert_eq!(run_open(&k, &iv, &a, &s), 0);
}

#[test]
fn open_rejects_a_modified_ciphertext_tag_aad_nonce_or_key() {
    let (k, iv, a, s) = case16();
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
    for pos in [0usize, 15, 16, 31] {
        let mut k2 = k.clone();
        k2[pos] ^= 1;
        assert_eq!(run_open(&k2, &iv, &a, &s), 5, "鍵 {pos}");
    }
    assert_eq!(run_open(&k, &iv, &a[..19], &s), 5, "AAD の短縮");
}

#[test]
fn open_rejects_inputs_shorter_than_the_tag() {
    let (k, iv, a, s) = case16();
    assert_eq!(run_open(&k, &iv, &a, &[]), 3);
    assert_eq!(run_open(&k, &iv, &a, &s[..15]), 3);
    // ちょうどタグだけ（空の平文）は短すぎではなく、認証に失敗する。
    assert_eq!(run_open(&k, &iv, &a, &s[..16]), 5);
}

#[test]
fn key_and_nonce_lengths_are_checked_at_construction() {
    let (_k, iv, a, s) = case16();
    // 16 バイト鍵（AES-128 用）は AES-256 では受け付けない。
    assert_eq!(run_open(&[0; 16], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 24], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 31], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 33], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 0], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 32], &iv[..11], &a, &s), 2002);
    assert_eq!(run_open(&[0; 32], &[0; 13], &a, &s), 2002);
}

// ---------------------------------------------------------------------------
// FIPS 197 の AES-256 例と鍵展開（内部のブロック演算）
// ---------------------------------------------------------------------------

/// FIPS 197 付録 C.3 の例（鍵 000102…1f）を、内部の `aes_expand_key`・`aes_encrypt_block` で照合する。
/// 加えて、展開鍵の長さ（240 バイト）と、先頭 32 バイトが鍵そのもの、末尾 16 バイトが
/// 付録 C.3 の最終ラウンド鍵（round[14].k_sch） `24fc79ccbf0979e9371ac23c6d68de36` であることを確かめる。
/// 一致しない数を返す（0 なら全一致）。
fn fips197_probe() -> String {
    let mut s = String::new();
    s.push_str("pub fn fips197_probe() -> Int\n  intent \"FIPS 197 の例を照合する\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n");
    let k = unhex("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f");
    s.push_str(&build_bytes("k", &k));
    s.push_str(&build_bytes(
        "p",
        &unhex("00112233445566778899aabbccddeeff"),
    ));
    s.push_str("  let rk = aes_expand_key(&k);\n  let out = aes_encrypt_block(&rk, &p);\n");
    s.push_str(&build_expected(
        "c",
        &unhex("8ea2b7ca516745bfeafc49904b496089"),
    ));
    s.push_str(&build_expected(
        "last",
        &unhex("24fc79ccbf0979e9371ac23c6d68de36"),
    ));
    s.push_str("  if rk.len() != 240 {\n    bad = bad + 1000;\n  }\n");
    s.push_str("  let mut j = 0;\n  while j < 16 {\n    let g = out.get(j).unwrap_or(0 - 1);\n    let w = match c.get(j) { Some(v) => v, None => 0 };\n    if g != w {\n      bad = bad + 1;\n    }\n    let lg = match rk.get(224 + j) { Some(v) => v, None => 0 - 1 };\n    let lw = match last.get(j) { Some(v) => v, None => 0 };\n    if lg != lw {\n      bad = bad + 1;\n    }\n    j = j + 1;\n  }\n");
    s.push_str("  let mut m = 0;\n  while m < 32 {\n    let rv = match rk.get(m) { Some(v) => v, None => 0 - 1 };\n    if rv != k.get(m).unwrap_or(0 - 2) {\n      bad = bad + 1;\n    }\n    m = m + 1;\n  }\n");
    s.push_str("  return bad;\n}\n");
    s
}

#[test]
fn fips197_aes256_example_and_key_schedule_match_on_the_internal_functions() {
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
// 出典: NIST CAVP の GCM テストベクタ（CAVS 14.0、2012-08-31 生成）。鍵 256 ビット・IV 96 ビット・
// タグ 128 ビットの区画だけを使う。取得元は pyca/cryptography の
// `vectors/cryptography_vectors/ciphers/AES/GCM/` にある同一ファイルの写し。
//   gcmEncryptExtIV256.rsp  SHA-256 4448b4998c3f58cd9f5542ddf7c573189a721ff3d3ac24cd5177bb4910a787f8
//   gcmDecrypt256.rsp       SHA-256 ed318735a517d5a85c82d2846c23dcf56e57a352fb0f0a163d9e7617a7bd12ad
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
    ("gcmEncryptExtIV256.rsp [PTlen = 0] [AADlen = 0] Count = 0", "b52c505a37d78eda5dd34f20c22540ea1b58963cf8e5bf8ffa85f9f2492505b4", "516c33929df5a3284ff463d7", "", "", "", "bdc1ac884d332457a1d2664f168c76f0"),
    ("gcmEncryptExtIV256.rsp [PTlen = 0] [AADlen = 128] Count = 0", "78dc4e0aaf52d935c3c01eea57428f00ca1fd475f5da86a49c8dd73d68c8e223", "d79cf22d504cc793c3fb6c8a", "b96baa8c1c75a671bfb2d08d06be5f36", "", "", "3e5d486aa2e30b22e040b85723a06e76"),
    ("gcmEncryptExtIV256.rsp [PTlen = 0] [AADlen = 160] Count = 0", "886cff5f3e6b8d0e1ad0a38fcdb26de97e8acbe79f6bed66959a598fa5047d65", "3a8efa1cd74bbab5448f9945", "519fee519d25c7a304d6c6aa1897ee1eb8c59655", "", "", "f6d47505ec96c98a42dc3ae719877b87"),
    ("gcmEncryptExtIV256.rsp [PTlen = 0] [AADlen = 384] Count = 0", "f4069bb739d07d0cafdcbc609ca01597f985c43db63bbaaa0debbb04d384e49c", "d25ff30fdc3d464fe173e805", "3e1449c4837f0892f9d55127c75c4b25d69be334baf5f19394d2d8bb460cbf2120e14736d0f634aa792feca20e455f11", "", "", "805ec2931c2181e5bfb74fa0a975f0cf"),
    ("gcmEncryptExtIV256.rsp [PTlen = 0] [AADlen = 720] Count = 0", "03ccb7dbc7b8425465c2c3fc39ed0593929ffd02a45ff583bd89b79c6f646fe9", "fd119985533bd5520b301d12", "98e68c10bf4b5ae62d434928fc6405147c6301417303ef3a703dcfd2c0c339a4d0a89bd29fe61fecf1066ab06d7a5c31a48ffbfed22f749b17e9bd0dc1c6f8fbd6fd4587184db964d5456132106d782338c3f117ec05229b0899", "", "", "cf54e7141349b66f248154427810c87a"),
    ("gcmEncryptExtIV256.rsp [PTlen = 128] [AADlen = 0] Count = 0", "31bdadd96698c204aa9ce1448ea94ae1fb4a9a0b3c9d773b51bb1822666b8f22", "0d18e06c7c725ac9e362e1ce", "", "2db5168e932556f8089a0622981d017d", "fa4362189661d163fcd6a56d8bf0405a", "d636ac1bbedd5cc3ee727dc2ab4a9489"),
    ("gcmEncryptExtIV256.rsp [PTlen = 128] [AADlen = 128] Count = 0", "92e11dcdaa866f5ce790fd24501f92509aacf4cb8b1339d50c9c1240935dd08b", "ac93a1a6145299bde902f21a", "1e0889016f67601c8ebea4943bc23ad6", "2d71bcfa914e4ac045b2aa60955fad24", "8995ae2e6df3dbf96fac7b7137bae67f", "eca5aa77d51d4a0a14d9c51e1da474ab"),
    ("gcmEncryptExtIV256.rsp [PTlen = 128] [AADlen = 160] Count = 0", "83688deb4af8007f9b713b47cfa6c73e35ea7a3aa4ecdb414dded03bf7a0fd3a", "0b459724904e010a46901cf3", "794a14ccd178c8ebfd1379dc704c5e208f9d8424", "33d893a2114ce06fc15d55e454cf90c3", "cc66bee423e3fcd4c0865715e9586696", "0fb291bd3dba94a1dfd8b286cfb97ac5"),
    ("gcmEncryptExtIV256.rsp [PTlen = 128] [AADlen = 384] Count = 0", "e4fed339c7b0cd267305d11ab0d5c3273632e8872d35bdc367a1363438239a35", "0365882cf75432cfd23cbd42", "8a97d2af5d41160ac2ff7dd8ba098e7aa4d618f0f455957d6a6d0801796747ba57c32dfbaaaf15176528fe3a0e4550c9", "fff39a087de39a03919fbd2f2fa5f513", "8d9e68f03f7e5f4a0ffaa7650d026d08", "3554542c478c0635285a61d1b51f6afa"),
    ("gcmEncryptExtIV256.rsp [PTlen = 128] [AADlen = 720] Count = 0", "80d755e24d129e68a5259ec2cf618e39317074a83c8961d3768ceb2ed8d5c3d7", "7598c07ba7b16cd12cf50813", "0e94f4c48fd0c9690c853ad2a5e197c5de262137b69ed0cdfa28d8d12413e4ffff15374e1cccb0423e8ed829a954a335ed705a272ad7f9abd1057c849bb0d54b768e9d79879ec552461cc04adb6ca0040c5dd5bc733d21a93702", "5e7fd1298c4f15aa0f1c1e47217aa7a9", "5762a38cf3f2fdf3645d2f6696a7eead", "8a6708e69468915c5367573924fe1ae3"),
    ("gcmEncryptExtIV256.rsp [PTlen = 104] [AADlen = 0] Count = 0", "82c4f12eeec3b2d3d157b0f992d292b237478d2cecc1d5f161389b97f999057a", "7b40b20f5f397177990ef2d1", "", "982a296ee1cd7086afad976945", "ec8e05a0471d6b43a59ca5335f", "113ddeafc62373cac2f5951bb9165249"),
    ("gcmEncryptExtIV256.rsp [PTlen = 104] [AADlen = 128] Count = 0", "dad89d9be9bba138cdcf8752c45b579d7e27c3dbb40f53e771dd8cfd500aa2d5", "cfb2aec82cfa6c7d89ee72ff", "6e43784a91851a77667a02198e28dc32", "b526ba1050177d05b0f72f8d67", "8b29e66e924ecae84f6d8f7d68", "1e365805c8f28b2ed8a5cadfd9079158"),
    ("gcmEncryptExtIV256.rsp [PTlen = 104] [AADlen = 160] Count = 0", "69b458f2644af9020463b40ee503cdf083d693815e2659051ae0d039e606a970", "8d1da8ab5f91ccd09205944b", "036ad5e5494ef817a8af2f5828784a4bfedd1653", "f3e0e09224256bf21a83a5de8d", "c0a62d77e6031bfdc6b13ae217", "a794a9aaee48cd92e47761bf1baff0af"),
    ("gcmEncryptExtIV256.rsp [PTlen = 104] [AADlen = 384] Count = 0", "5f671466378f470ba5f5160e2209f3d95a48b7e560625d5a08654414de23aee2", "6b3c08a663d04132243dd96c", "12965559c31d538f937bda6eee9c93b0387318dc5d9496fb1c3a0b9b978dbfebff2a5823974ee9d679834dbe59f7ec51", "c428592d9f8a7f107ec4d0df05", "1d8d7fe4357080c817303ce19c", "e88d6b566fdc7b4fd62106bd2eb806ec"),
    ("gcmEncryptExtIV256.rsp [PTlen = 104] [AADlen = 720] Count = 0", "ff9506b4d46ba54128876fadfcc673a4c927c618ea7d95cfcaa508cbc8f7fc66", "3742ad2208a0484345eee1be", "f1360a27fdc244be8739d85af6491c762a693aafe668c449515fdeeedb6a90aeee3891bbc8b69adc6a6426cb12fcdebc32c9f58c5259d128b91efa28620a3a9a0168b0ff5e76951cb41647ba4aa1f87fac0d97ac580e42cffc7e", "7fd0d6cadc92cad27bb2d7d8c8", "bdb8346b28eb4d7226493611a6", "7484d827b767647f44c7f94a39f8175c"),
    ("gcmEncryptExtIV256.rsp [PTlen = 256] [AADlen = 0] Count = 0", "268ed1b5d7c9c7304f9cae5fc437b4cd3aebe2ec65f0d85c3918d3d3b5bba89b", "9ed9d8180564e0e945f5e5d4", "", "fe29a40d8ebf57262bdb87191d01843f4ca4b2de97d88273154a0b7d9e2fdb80", "791a4a026f16f3a5ea06274bf02baab469860abde5e645f3dd473a5acddeecfc", "05b2b74db0662550435ef1900e136b15"),
    ("gcmEncryptExtIV256.rsp [PTlen = 256] [AADlen = 128] Count = 0", "37ccdba1d929d6436c16bba5b5ff34deec88ed7df3d15d0f4ddf80c0c731ee1f", "5c1b21c8998ed6299006d3f9", "22ed235946235a85a45bc5fad7140bfa", "ad4260e3cdc76bcc10c7b2c06b80b3be948258e5ef20c508a81f51e96a518388", "3b335f8b08d33ccdcad228a74700f1007542a4d1e7fc1ebe3f447fe71af29816", "1fbf49cc46f458bf6e88f6370975e6d4"),
    ("gcmEncryptExtIV256.rsp [PTlen = 256] [AADlen = 160] Count = 0", "5853c020946b35f2c58ec427152b840420c40029636adcbb027471378cfdde0f", "eec313dd07cc1b3e6b068a47", "1389b522c24a774181700553f0246bbabdd38d6f", "ce7458e56aef9061cb0c42ec2315565e6168f5a6249ffd31610b6d17ab64935e", "eadc3b8766a77ded1a58cb727eca2a9790496c298654cda78febf0da16b6903b", "3d49a5b32fde7eafcce90079217ffb57"),
    ("gcmEncryptExtIV256.rsp [PTlen = 256] [AADlen = 384] Count = 0", "dc776f0156c15d032623854b625c61868e5db84b7b6f9fbd3672f12f0025e0f6", "67130951c4a57f6ae7f13241", "fd0920faeb7b212932280a009bac969145e5c316cf3922622c3705c3457c4e9f124b2076994323fbcfb523f8ed16d241", "9378a727a5119595ad631b12a5a6bc8a91756ef09c8d6eaa2b718fe86876da20", "6d958c20870d401a3c1f7a0ac092c97774d451c09f7aae992a8841ff0ab9d60d", "b876831b4ecd7242963b040aa45c4114"),
    ("gcmEncryptExtIV256.rsp [PTlen = 256] [AADlen = 720] Count = 0", "26bf255bee60ef0f653769e7034db95b8c791752754e575c761059e9ee8dcf78", "cecd97ab07ce57c1612744f5", "afebbe9f260f8c118e52b84d8880a34622675faef334cdb41be9385b7d059b79c0f8a432d25f8b71e781b177fce4d4c57ac5734543e85d7513f96382ff4b2d4b95b2f1fdbaf9e78bbd1db13a7dd26e8a4ac83a3e8ab42d1d545f", "96983917a036650763aca2b4e927d95ffc74339519ed40c4336dba91edfbf9ad", "e34b1540a769f7913331d66796e00bdc3ee0f258cf244eb7663375cc5ad6c658", "3841f02beb7a7fca7e578922d0a2f80c"),
    ("gcmEncryptExtIV256.rsp [PTlen = 408] [AADlen = 0] Count = 0", "1fded32d5999de4a76e0f8082108823aef60417e1896cf4218a2fa90f632ec8a", "1f3afa4711e9474f32e70462", "", "06b2c75853df9aeb17befd33cea81c630b0fc53667ff45199c629c8e15dce41e530aa792f796b8138eeab2e86c7b7bee1d40b0", "91fbd061ddc5a7fcc9513fcdfdc9c3a7c5d4d64cedf6a9c24ab8a77c36eefbf1c5dc00bc50121b96456c8cd8b6ff1f8b3e480f", "30096d340f3d5c42d82a6f475def23eb"),
    ("gcmEncryptExtIV256.rsp [PTlen = 408] [AADlen = 128] Count = 0", "5fe01c4baf01cbe07796d5aaef6ec1f45193a98a223594ae4f0ef4952e82e330", "bd587321566c7f1a5dd8652d", "9013617817dda947e135ee6dd3653382", "881dc6c7a5d4509f3c4bd2daab08f165ddc204489aa8134562a4eac3d0bcad7965847b102733bb63d1e5c598ece0c3e5dadddd", "16e375b4973b339d3f746c1c5a568bc7526e909ddff1e19c95c94a6ccff210c9a4a40679de5760c396ac0e2ceb1234f9f5fe26", "abd3d26d65a6275f7a4f56b422acab49"),
    ("gcmEncryptExtIV256.rsp [PTlen = 408] [AADlen = 160] Count = 0", "24501ad384e473963d476edcfe08205237acfd49b5b8f33857f8114e863fec7f", "9ff18563b978ec281b3f2794", "adb5ec720ccf9898500028bf34afccbcaca126ef", "27f348f9cdc0c5bd5e66b1ccb63ad920ff2219d14e8d631b3872265cf117ee86757accb158bd9abb3868fdc0d0b074b5f01b2c", "eb7cb754c824e8d96f7c6d9b76c7d26fb874ffbf1d65c6f64a698d839b0b06145dae82057ad55994cf59ad7f67c0fa5e85fab8", "bc95c532fecc594c36d1550286a7a3f0"),
    ("gcmEncryptExtIV256.rsp [PTlen = 408] [AADlen = 384] Count = 0", "463b412911767d57a0b33969e674ffe7845d313b88c6fe312f3d724be68e1fca", "611ce6f9a6880750de7da6cb", "0a682fbc6192e1b47a5e0868787ffdafe5a50cead3575849990cdd2ea9b3597749403efb4a56684f0c6bde352d4aeec5", "e7d1dcf668e2876861940e012fe52a98dacbd78ab63c08842cc9801ea581682ad54af0c34d0d7f6f59e8ee0bf4900e0fd85042", "8886e196010cb3849d9c1a182abe1eeab0a5f3ca423c3669a4a8703c0f146e8e956fb122e0d721b869d2b6fcd4216d7d4d3758", "2469cecd70fd98fec9264f71df1aee9a"),
    ("gcmEncryptExtIV256.rsp [PTlen = 408] [AADlen = 720] Count = 0", "148579a3cbca86d5520d66c0ec71ca5f7e41ba78e56dc6eebd566fed547fe691", "b08a5ea1927499c6ecbfd4e0", "e4f963f015ffbb99ee3349bbaf7e8e8e6c2a71c230a48f9d59860a29091d2747e01a5ca572347e247d25f56ba7ae8e05cde2be3c97931292c02370208ecd097ef692687fecf2f419d3200162a6480a57dad408a0dfeb492e2c5d", "9d0b15fdf1bd595f91f8b3abc0f7dec927dfd4799935a1795d9ce00c9b879434420fe42c275a7cd7b39d638fb81ca52b49dc41", "2097e372950a5e9383c675e89eea1c314f999159f5611344b298cda45e62843716f215f82ee663919c64002a5c198d7878fd3f", "adbecdb0d5c2224d804d2886ff9a5760"),
];

const CAVP_DEC_PASS: &[CavpCase] = &[
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 0] Count = 0", "f5a2b27c74355872eb3ef6c5feafaa740e6ae990d9d48c3bd9bb8235e589f010", "58d2240f580a31c1d24948e9", "", "", "", "15e051a5e4a5f5da6cea92e2ebee5bac"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 128] Count = 0", "6dfdafd6703c285c01f14fd10a6012862b2af950d4733abb403b2e745b26945d", "3749d0b3d5bacb71be06ade6", "c0d249871992e70302ae008193d1e89f", "", "", "4aa4cc69f84ee6ac16d9bfb4e05de500"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 160] Count = 1", "d33c33b003bda4bc2ae47b4f5e477e90607a6342967a73d8a88151ee99394b1a", "cfc54a48b843763db1cdbaf6", "840befba8ef6ad83a74934919195a70f2d891dc0", "", "", "9dde8b668c585f513f3048b8a2cd1b06"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 384] Count = 1", "752a9b3cc5f29bba64e773460c7396c13f911fe77de054097da5b682ea525d79", "f93e50fe23883216de85d3b4", "346dd8f25abaf85221fcbbccf794fe3c6794c0f16557e2ba14f9c03bffc99ee5539b9142d1952e66af35df91250e690b", "", "", "f02936676e36e7598258c37210b4470f"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 720] Count = 0", "a4851117328a93bf528382f22f35ac94688259fd2f517e4fd27ee9cf9b8c8a2c", "44395ca4943aca24875a281a", "b9a63c85bd7cb93c9d2543572099ac0a0b1ab4dddbea4c75bacfab9755ae763cb1062a594dda9ca860134c74776752ad357cfda32d1c20e896370dac5808c147061ed1545a2a6ff26fe2e0e2e38ec887c1e210cecad4a8c9a86d", "", "", "1b13e6132415fd70d9092e32ff2759be"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 0] Count = 0", "4c8ebfe1444ec1b2d503c6986659af2c94fafe945f72c1e8486a5acfedb8a0f8", "473360e0ad24889959858995", "", "7789b41cb3ee548814ca0b388c10b343", "d2c78110ac7e8f107c0df0570bd7c90c", "c26a379b6d98ef2852ead8ce83a833a7"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 128] Count = 0", "54e352ea1d84bfe64a1011096111fbe7668ad2203d902a01458c3bbd85bfce14", "df7c3bca00396d0c018495d9", "7e968d71b50c1f11fd001f3fef49d045", "85fc3dfad9b5a8d3258e4fc44571bd3b", "426e0efc693b7be1f3018db7ddbb7e4d", "ee8257795be6a1164d7e1d2d6cac77a7"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 160] Count = 0", "aef220035cbb9e47ce605698aa28e3b0ba50b4ffcd473bb8da2017889b38055f", "cde7af095360ea827778761d", "f269837306abbcee2da1722f28be35163e3d8567", "9775db638e5d964fc9c70b5fe456ec14", "bb1cdf25717445e5a77444d488387aee", "e72340deabc1589125e9e4a2755512c7"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 384] Count = 0", "16ac8e71760c1fd4eba37422bcbcaaf67513fe6822d00a14e3ab62a6ca559931", "7021d16596de09585b50673a", "ecdca487c04e574ff11acec743c66e052e80bbc7fe67273246e218e8c82de0e691dcf4945572a2195b57c2257d2c8f41", "5ddecdab11e715d0b5996d46cb18715a", "073988485ae13dddb025f7f07059ca70", "241a2bc57f4ee159a79bc57cf48e023c"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 720] Count = 2", "6d704dd097577fb901fd9e02c1e9a278c4b155d8d32ccac42de0dfeb860375b4", "b61a6b349e29948bf3bf8c83", "02ad29d7f311f6749b2b2fd1e710593c4589535b6e08b95048b57c7a251583be5a92bd5dca8249beb9628497433bf57595ee5918d825bfab291334335cca5ade4d18d341db974d90e4fe3ee38bab81f0b02f863be4f431f20f23", "97bce743a5f5478d001bcefd6128f34f", "2013c24491e7136aa0e91cd2ccd87adc", "464475f7d491183b46f62fd4dfbc5e58"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 0] Count = 1", "a71dac1377a3bf5d7fb1b5e36bee70d2e01de2a84a1c1009ba7448f7f26131dc", "c5b60dda3f333b1146e9da7c", "", "5b87141335f2becac1a559e05f", "43af49ec1ae3738a20755034d6", "6f80b6ef2d8830a55eb63680a8dff9e0"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 128] Count = 1", "2e54b0d8748141aa2348f47d548236ae1ce3d4cf33be8ec7c00a10999cd9a64b", "179d7a48bd6e32c099cb27b6", "12fe663798231f384fe4ac1db3fc3f89", "be6e65c851f5ca9b71a607255d", "32bd95c1e836ae5d388d758bdc", "9b7c419faad417b90261f78f82bc73f1"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 160] Count = 1", "6b879203e1a23c90bc4b92e4ee42001f8466b0a168a3b09df9b644182d8242ab", "3e05199e1d56ca0b120694c8", "b21100419dc0a8ea58ef372e5f07e841b2ee5568", "d167388250a59485b3dde40200", "ef4cd1b06520043c369f832448", "0ac846204a3a5299cef82d7a9b1748a8"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 384] Count = 0", "aa25af7152f30885df7685ede610642a8c25fd87defc0fbfd89060c9cc29cd60", "1f5044e3f08376d84e760c7b", "dda57f316d0dd0a15a0fbbfed9b6c51cf11242964cf1676a73050b2b93b8afc1e190f44aa3eb7e577245f9aa4a7e13e2", "f36ec2c4900d906047092d1655", "00dfc2de0fa7ce4946b71ea300", "633024dd5927cdb153a0772f4358303f"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 720] Count = 0", "886bff628eea3114ed1c62301414327d7d565fc1a39fd27cc92560ed42911144", "147bdedaff607fc47e2309cf", "217e54b1416c1f4ac698272ba5fb95bf2e28ddfa809a4f774629db920336bbfdd9b926ad37547056ff3d8c73f0f930e658dd483b6a2265c92c594e9fd0964fa0da6d0211601387ce5def855c032370562e6c2b385cf5b9ae3364", "86a560da8d85631c31f979d250", "736f72611e2d132e6e1e5223a9", "5ada94656899a456638dbe120d741da6"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 0] Count = 0", "c3d99825f2181f4808acd2068eac7441a65bd428f14d2aab43fefc0129091139", "cafabd9672ca6c79a2fbdc22", "", "25431587e9ecffc7c37f8d6d52a9bc3310651d46fb0e3bad2726c8f2db653749", "84e5f23f95648fa247cb28eef53abec947dbf05ac953734618111583840bd980", "79651c875f7941793d42bbd0af1cce7c"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 128] Count = 1", "f0eaf7b41b42f4500635bc05d9cede11a5363d59a6288870f527bcffeb4d6e04", "18f316781077a595c72d4c07", "42cade3a19204b7d4843628c425c2375", "400fb5ef32083b3abea957c4f068abad50c8d86bbf9351fa72e7da5171df38f9", "7a1b61009dce6b7cd4d1ea0203b179f1219dd5ce7407e12ea0a4c56c71bb791b", "4419180b0b963b7289a4fa3f45c535a3"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 160] Count = 0", "c70fcbba915478f66315a15ffc3b11d92c24cbf213cc858f8740713ed9493182", "5b413f5dbfccc6a7c65ff5c4", "ee7b3521f1363c1fcbbb24c2c65f7ecee99d457c", "f135e924c57ca538f72c700fb0c9c103a70fc1935797cfe08024397f1c1a8277", "2ab2d676861b2281d3f740f637c997e25569ec7352e1767ce4c4c9ed5a6ab15a", "9a4a3febec6413d461f26364b51237f3"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 384] Count = 0", "42f6c25159b8655380f052dd5dad180e76813b60eb813665c5015f26cf32e8f1", "7248a5ed48f4f1b4a9db3826", "8e3c74f127dbfe29ac4de0a7c3240ee8aa8d38a82f38ad6b480236c8cd4232057a5502e936bfe22225830fa195a8afce", "9a329cb45b0093e9c00615137dd7dbd1f8b525999af3bfc222315f41817717a7", "26325c3463813a7d59d184a330ef80959637fa6db4f5db3062d3d2ec7e32d82a", "4f39c63d4f215d5b39a58853d3842175"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 720] Count = 1", "6fc9e26edc3989cd664d6b4d7310c45d94bb02f4442270a6ada600b81dd7ec7a", "121328cccef8e87a055e783d", "f9dfbad723ae30288a45b03439fdcd649136a300a78e12608215c8be084fe33a4441b69efabceb65c6365472f920d881895e04c8f1b7050e50005496f578d9d1c3bc2203f306faf08543921b373b651dde26cd69f51768b9a55f", "9e7c2b81f5e3ea441ff6d797507c0dac1b89df6d7c6468584cfae4a031c32766", "abf5e41d0426fc3918e335adbb1c470d0899cf7969eb16927b634825e5dcab99", "146794aad789988e9cca01affe0ef215"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 0] Count = 0", "4433db5fe066960bdd4e1d4d418b641c14bfcef9d574e29dcd0995352850f1eb", "0e396446655582838f27f72f", "", "d602c06b947abe06cf6aa2c5c1562e29062ad6220da9bc9c25d66a60bd85a80d4fbcc1fb4919b6566be35af9819aba836b8b47", "b0d254abe43bdb563ead669192c1e57e9a85c51dba0f1c8501d1ce92273f1ce7e140dcfac94757fabb128caad16912cead0607", "ffd0b02c92dbfcfbe9d58f7ff9e6f506"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 128] Count = 0", "aeb3830cb9ce31cae7b1d47511bb2d3dcc2131714ace202b21b98820e7079792", "e7e87c45ec0a94c8e92353f1", "07d9bb1fa3aea7ceeefbedae87dcd713", "b4d0ecc410c430b61c11a1a42802858a0e9ee12f9a912f2f6b0570c99177f6de4bd79830cf9efb30759055e1f70d21e3f74957", "b20542b61b8fa6f847198334cb82fdbcb2311be855a6b2b3662bdb06ff0796238bea092a8ea21b585d38ace950378f41224269", "3bdd1d0cc2bbcefffe0ed2121aecbd00"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 160] Count = 2", "6450d4501b1e6cfbe172c4c8570363e96b496591b842661c28c2f6c908379cad", "7e4262035e0bf3d60e91668a", "f1c522f026e4c5d43851da516a1b78768ab18171", "17449e236ef5858f6d891412495ead4607bfae2a2d735182a2a0242f9d52fc5345ef912dbe16f3bb4576fe3bcafe336dee6085", "5a99b336fd3cfd82f10fb08f7045012415f0d9a06bb92dcf59c6f0dbe62d433671aacb8a1c52ce7bbf6aea372bf51e2ba79406", "fe93b01636f7bb0458041f213e98de65"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 384] Count = 1", "075217d7166c88d5247ecbf3d78f67755d4d39a4707310e44e11e6821a58ad4a", "b6fa6e0e41aa40ceb0b8cd08", "173abc7d928b779269add386911f254648b326715b2fd467dc7d7aab8809cea0f272f5ac1f9caface6653445a272c356", "7737d48b507ac2bacaa42765c7104082cb6b95078159b53ed8290128c68c1c35bbe93358b7efda9fce36af8f96daffbf747b1d", "c56dfcb147716d407536336a865d231ac0e94cf49d6e2d997f7ae2056fc4c646b6d5b27e0220d8d6d8a4d7ab51cdcf406ce289", "3dd38b731ece2943a62ac8e23955145a"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 720] Count = 4", "5f72046245d3f4a0877e50a86554bfd57d1c5e073d1ed3b5451f6d0fc2a8507a", "ea6f5b391e44b751b26bce6f", "9b3a68c941d42744673fb60fea49075eae77322e7e70e34502c115b6495ebfc796d6290807653c6b53cd84281bd0311656d0013f44619d2748177e99e8f8347c989a7b59f9d8dcf00f31db0684a4a83e037e8777bae55f799b0d", "b0a881b751cc1eb0c912a4cf9bd971983707dbd2411725664503455c55db25cdb19bc669c2654a3a8011de6bf7eff3f9f07834", "0e6e0b2114c40769c15958d965a14dcf50b680e0185a4409d77d894ca15b1e698dd83b3536b18c05d8cd0873d1edce8150ecb5", "fdaaff86ceb937502cd9012d03585800"),
];

const CAVP_DEC_FAIL: &[CavpCase] = &[
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 0] Count = 1", "e5a8123f2e2e007d4e379ba114a2fb66e6613f57c72d4e4f024964053028a831", "51e43385bf533e168427e1ad", "", "", "", "38fe845c66e66bdd884c2aecafd280e6"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 128] Count = 4", "2c392a5eb1a9c705371beda3a901c7c61dca4d93b4291de1dd0dd15ec11ffc45", "0723fb84a08f4ea09841f32a", "140be561b6171eab942c486a94d33d43", "", "", "aa0e1c9b57975bfc91aa137231977d2c"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 160] Count = 0", "c0b46dc989a7bd45c15553e86a0bf206bf85a5222df1d5a7c8620ec8fa94cb95", "c140cffdba21d9226eca8284", "410a4afa61dd30c2d687ba9cba18fe8a0cd37d58", "", "", "54ba2bc9b8621d84e936c839c0de7118"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 384] Count = 0", "8a7425bf862b2ea030c3c365a04a2d5bb537f317f091766743effc628d81bbf5", "d48dbe98ce2548c0e7bcd1e5", "bb8c00e42e217118c9b86deb3663943cad0b256a40f74dfb3af36ea040b4ccaaadb507604dfd0e214419b9b394802d9b", "", "", "9f83a8fd4146dc53909ddc4c09f50ba8"),
    ("gcmDecrypt256.rsp [PTlen = 0] [AADlen = 720] Count = 1", "eb49e7065fd7f9b49d198b43d40653a20656fc44b304a716f4c5f9fb586ef073", "1301a9d18481ba7841d9ccce", "71693a9e655ea0dc084a7bc9ae91a98743adcd77fa9185bd0adcb4c18e67ff752b6a476e8100bded46c7ac2327cf3e804cda520e535bc62ffe19f5f46b866d70cf99e8ce3dbbfd9f40c755859b48bdf2d967a501a58f1c739974", "", "", "5be6d5030b8afb923e3f8ef2767f4e93"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 0] Count = 2", "c997768e2d14e3d38259667a6649079de77beb4543589771e5068e6cd7cd0b14", "835090aed9552dbdd45277e2", "", "", "9f6607d68e22ccf21928db0986be126e", "f32617f67c574fd9f44ef76ff880ab9f"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 128] Count = 2", "9a0343f850a6427120f764789ffec6d237447b898fbf51d2182f065d3861497d", "3deef6f453dd70d92143adcd", "dbb8226a624520863db6897017b2a4f8", "", "e93165935ac18e3a2845d15fe31a9286", "f5fc50d18766bc3d9e16dd136d45816b"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 160] Count = 4", "041ef9cd359ac65b7937dcee46778d19dc32a312c81edc15dbc8918df75081f0", "4bf5a7c50201a6586b1dcdb5", "10980f54fdbd18e49a48cf74ecbbb7e9d22083a3", "", "2c24ef09b6a01b5f200c59284316fa56", "36c2da827d58cdc8cd3faf0a335b1846"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 384] Count = 1", "0fc8ae600fd3396716c3e88327545b5e4308eda1c25e076d1098fabb74a9f0a0", "c529787b4c756a2343f36280", "2c6ac22d787b70bf516f4bf72deb8f9514a588e5e53cb26980becd0d500bb23453d8ed874ec2514b813aa45d2dc36e6a", "", "7dc464c2735f446f9a3f92873688e6c3", "fd6d32ff9a090bb880371a678f37ecaa"),
    ("gcmDecrypt256.rsp [PTlen = 128] [AADlen = 720] Count = 0", "76a75e06f485ac3838993c635faf17bdc8f914a1d341e5bba014de44c37c2340", "acf54108e67dab53a4274a55", "863f751d65431b3aed0f09cef3bdaf81668e4790eed7de202c2edf35b09b57489c5c3184674718e9a06df5cb5d8ea5e24c5979081bac6a4ce78bada842633f2b06c12c54152be4db8d4925dfb5eb7c35b4c2e7065fe96fb23814", "", "b8c8429142a34305a463ba394c6c813c", "0267cff29f96e625dee8cb4ab29b6448"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 0] Count = 0", "8b37c4b8cf634704920059866ad96c49e9da502c63fca4a3a7a4dcec74cb0610", "cb59344d2b06c4ae57cd0ea4", "", "", "66ab935c93555e786b775637a3", "d8733acbb564d8afaa99d7ca2e2f92a9"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 128] Count = 0", "81e196b20b71adb6a239b580e51474137c8dc8d17764745fd02ceb456b6b5624", "65cc53fa7eac34c557e72e87", "f1c22cb0a2c33e7743ef355b6795830b", "", "5c70d88ac4e9404539e85623ab", "3ba8bf8228e4b2aca5adc8f3273a1b41"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 160] Count = 0", "de1bb0d8acd23457c593c490e45858698d9acb887cf12c4a2931ea14a010a985", "a787cb34d6f88921f036b833", "6ffa289ec67aa3c1f8b4068af8d9acadc434cfc2", "", "ca0e93b01a47910918c18f1d7a", "ef745fdf7d78d283b39a4597ca82f8cb"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 384] Count = 1", "29e640cb29c4ef616d646123d5bb5c1bd75e843587d64eba90d50f0295437ff7", "4aede7cf9d70fb80c4ae2941", "91f9d7e7cf938990a458de56c4a1e9d288874dd6a546d6d41d9365c29ee9d45919b8f59b358d8e02ddb0120e24e60480", "", "697eb024c2ec39bc6cf9dd0eff", "56ca5aae42fb30b3a22c8cd7e7edbb6c"),
    ("gcmDecrypt256.rsp [PTlen = 104] [AADlen = 720] Count = 1", "e7e85f27fbb7fb98b18827a475d9e436a232cb884448e6dc88fa91e7e62143d4", "53182059b753f2b8cd65d835", "cd5b88000f5fa6ee680eb1f5a58f4b9e29b2b4447473abaee46c0100bcf47a932edfe5b06af570856e22f0eecd03e564bfd9b0e76aa7a0af7ec06670904d3788bbfad522cb2942d56298c0920844c72a49c01a383337acddce38", "", "63053970ac794cdcb541137bb0", "d227696559f40986538278761db8e874"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 0] Count = 3", "a6da42afcc5b86f989910f7d27f29362a4e4a07ab9b7090d5820cbf97faf0729", "fa099d57bdcec5b25ee2e20d", "", "", "c70900c56aaedf2af746e834d39af7eed53b86b1e832d9f72bcf35ecf8144282", "44d3f379ca0c4afa71b147bc84d49143"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 128] Count = 0", "22ab50af434ac76377118aaa014c008d25e766b23e2d0488c1b2a3b720a9e89e", "123f018807b3f5368c38b1dd", "1c56bf07fdbddc8eebcd0712ab2c16ad", "", "b05cca6533e1f81c9751b42cd32158ac37841afae09eafde4cf51458ed6d234f", "f082c6743dca40f7b98ed44de872d46c"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 160] Count = 1", "c444d2bc6b0f14b306d7afbf9b0c84d9f2fcd2730df6d656402a184143a4eeb1", "89a840f9fede6fb08cce158e", "aaade9efaed6435e3130b9c36d8a82d201cce2a8", "", "ff15d0cdaf9b5390018a881a759183ae5cf702b9550cac2de78ad2c6d0da5af1", "8655bd8d3213f0873d530c326a2fc15c"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 384] Count = 2", "fff8747f38466904e99409f9dd8ed202f0e1a3e9e4768fe7f3a0b39c523bbbef", "bd4ea7d286dfc0948145e37d", "d95546e9a05a8363f848419ce4d96f148a2d722f2bc15e5b6599b7eef1fb8ac52b3a2cf97cd6fecc67fa0bde6367b575", "", "e8b7aed2ece9207ec158dc6d9b6fbf941197964820a9b0e5d8d30969c89b3e77", "2379d116e1152171fbc184bc8b760845"),
    ("gcmDecrypt256.rsp [PTlen = 256] [AADlen = 720] Count = 0", "a0d27f0c13e847ba806f8a001262c9855d1d5bdbb39aaa630ad1d674bbb2e66f", "949d89f59dc0b03d5550fe59", "8ed3ca180df1c00a478fd51ed04b707111cf49637755d0ffd643a940e3852ef0c238a11a39024885aac6582620a0db871d4856272d6054a660705aaf05268d50a94758e59f106d5eb22f6e61b2d890e20af40566bf715c4a0e94", "", "ff3d7ce48bf2a297ed4b14c50639995fc7c049bc5b7462e29473f01798575af0", "f30f13b7f89b1c6ede53a17e4caa5f15"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 0] Count = 1", "28ae911ee685872d906de12d7696351df8ef2234a74a95efa4ea15b327338fe0", "2fe6a815d4865181fade5fac", "", "", "1168442ef64656ef6577fb42c1919c84aae856388e4db9945bb8c9b8412bbe6458bc400444d5d2bf2630f83468f66f9e46e790", "b75f616fd1a3d6563b62b899e5a7e522"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 128] Count = 1", "bcbf4f8694cbc2f2560253dd12a89cf5eca6ec72203bae092652199565c27c2e", "9967fbab1a6498681c958e96", "2145cea37ce88228bbc1812b58615b16", "", "7fbf44ef67832284da11f929ab5af428153c851cae56052133913d207d2c80ec9b2a84549420339e496d48c73b64bbf8c5964a", "3a09823f4d09205929fe178690dfbf75"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 160] Count = 0", "e9d381a9c413bee66175d5586a189836e5c20f5583535ab4d3f3e612dc21700e", "23e81571da1c7821c681c7ca", "6f39c9ae7b8e8a58a95f0dd8ea6a9087cbccdfd6", "", "a25f3f580306cd5065d22a6b7e9660110af7204bb77d370f7f34bee547feeff7b32a596fce29c9040e68b1589aad48da881990", "5b6dcd70eefb0892fab1539298b92a4b"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 384] Count = 0", "ae02edd322afbf5002a22f6d745f6b93c946d34089a9943fcf9114060b942b74", "f345788b7cf2a0f5e1e6fdcf", "d08ecc2575a5cce9190815e0ac2874aae09c11c606963c365d8bde182b6a7ccdaa2489a1e1f82e8bd1bdc890f842ecd2", "", "403946a53b57fd644d2033e9feb5e70ee0d7760c2d4136033b26edf22d1eb430a22b5641fe11cbff9ea704630465eaede5fa5b", "154a3811c02b635a0400577e82a486dd"),
    ("gcmDecrypt256.rsp [PTlen = 408] [AADlen = 720] Count = 0", "e36aca93414b13f5313e76a7244588ee116551d1f34c32859166f2eb0ac1a9b7", "e9e701b1ccef6bddd03391d8", "6a08fe6e55a08f283cec4c4b37676e770f402af6102f548ad473ec6236da764f7076ffd41bbd9611b439362d899682b7b0f839fc5a68d9df54afd1e2b3c4e7d072454ee27111d52193d28b9c4f925d2a8b451675af39191a2cba", "", "5b059ac6733b6de0e8cf5b88b7301c02c993426f71bb12abf692e9deeacfac1ff1644c87d4df130028f515f0feda636309a24d", "43c7c9c93cc265fc8e192000e0417b5b"),
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

// ---------------------------------------------------------------------------
// 組合せ確認: HKDF-SHA-384 で導出した鍵での AES-256-GCM
// ---------------------------------------------------------------------------

/// salt = 000102…0c、IKM = 0x0b × 22、info = f0f1…f9。鍵 = HKDF-SHA-384 の OKM 32 バイト、
/// nonce = info に "iv" を足した HKDF-SHA-384 の OKM 12 バイト。AAD = gen(13, 7)、平文 = gen(37, 9)。
/// 期待値は `cryptography` の HKDF(SHA384) と AESGCM で独立に求めた。
const COMB_KEY: &str = "9b5097a86038b805309076a44b3a9f38063e25b516dcbf369f394cfab43685f7";
const COMB_IV: &str = "7c58804a7fc3449643efab7c";
const COMB_SEALED: &str = "f91bef2594a3cefdbdc872d5e41acc76faa125c55b94f5b806fd09fd7a2f3f7104468816d807703f5b1ac2404f892b35512fc9fa09";

fn comb_program(sealed_src: &str, tail: &str) -> String {
    format!(
        "{HEAD}{MISMATCH_FN}{MAIN_HEAD}{}{}{}{}{}{}{sealed_src}  let salt = Secret.from_bytes(sb);
  let ikm = Secret.from_bytes(ib);
  let prk = hkdf_extract_sha384(Some(salt), &ikm);
  let key_secret = hkdf_expand_sha384(&prk, &info, 32);
  let iv_secret = hkdf_expand_sha384(&prk, &info_iv, 12);
  let iv_bytes = iv_secret.expose();
  let kr = Aes256GcmKey.from_secret(key_secret);
  match kr {{
    Err(e) => {{ return 1000 + code(e); }}
    Ok(key) => {{
      match Nonce96.from_slice(&iv_bytes) {{
        Err(e) => {{ return 2000 + code(e); }}
        Ok(nonce) => {{
{tail}
        }}
      }}
    }}
  }}
}}
",
        build_bytes("sb", &unhex("000102030405060708090a0b0c")),
        build_bytes("ib", &[0x0b; 22]),
        build_bytes("info", &unhex("f0f1f2f3f4f5f6f7f8f9")),
        build_bytes("info_iv", &unhex("f0f1f2f3f4f5f6f7f8f96976")),
        build_bytes("aad", &gen(13, 7)),
        build_bytes("pb", &gen(37, 9)),
    )
}

#[test]
fn hkdf_sha384_derived_key_works_with_aes256_gcm_seal_and_open() {
    let src = comb_program(
        &build_expected("want", &unhex(COMB_SEALED)),
        "          match aes256_gcm_seal(&key, nonce, &aad, &Secret.from_bytes(pb.clone())) {
            Err(e) => { return 3000 + code(e); }
            Ok(out) => {
              let bad_seal = mismatch(&out, &want);
              match Nonce96.from_slice(&iv_bytes) {
                Err(e) => { return 2000 + code(e); }
                Ok(nonce2) => {
                  match aes256_gcm_open(&key, &nonce2, &aad, &out) {
                    Err(e) => { return 4000 + code(e); }
                    Ok(secret) => {
                      let got = secret.expose();
                      let bad_open = mismatch(&got, &pb_list());
                      return bad_seal * 10 + bad_open;
                    }
                  }
                }
              }
            }
          }",
    );
    // `pb_list` は平文（List<Int>）を返す補助関数として足す。
    let helper = format!(
        "fn pb_list() -> List<Int>\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{}  return want;\n}}\n\n",
        build_expected("want", &gen(37, 9))
    );
    let src = src.replacen(MISMATCH_FN, &format!("{MISMATCH_FN}{helper}"), 1);
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    assert_eq!(r.value, "0");
}

#[test]
fn hkdf_sha384_derived_key_and_nonce_match_the_independent_values() {
    // 導出した鍵と nonce 自体が独立な値と一致する（組合せの前提）。
    let src = format!(
        "{HEAD}{MISMATCH_FN}{MAIN_HEAD}{}{}{}{}{}{}  let salt = Secret.from_bytes(sb);
  let ikm = Secret.from_bytes(ib);
  let prk = hkdf_extract_sha384(Some(salt), &ikm);
  let kb = hkdf_expand_sha384(&prk, &info, 32).expose();
  let ivb = hkdf_expand_sha384(&prk, &info_iv, 12).expose();
  return mismatch(&kb, &want_k) * 10 + mismatch(&ivb, &want_iv);
}}
",
        build_bytes("sb", &unhex("000102030405060708090a0b0c")),
        build_bytes("ib", &[0x0b; 22]),
        build_bytes("info", &unhex("f0f1f2f3f4f5f6f7f8f9")),
        build_bytes("info_iv", &unhex("f0f1f2f3f4f5f6f7f8f96976")),
        build_expected("want_k", &unhex(COMB_KEY)),
        build_expected("want_iv", &unhex(COMB_IV)),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    assert_eq!(r.value, "0");
}

//! ADR 0058 段 2a: ChaCha20-Poly1305（`chacha20_poly1305_seal`・`chacha20_poly1305_open`）の利用者向けテスト。
//!
//! 出典と検算（ADR 0058 決定 4）:
//! - RFC 8439 §2.8.2 の AEAD 例（暗号文・タグ・復号）。タグは RFC の `1ae10b594f09e26a7e902ecbd0600691`。
//! - RFC 8439 付録 A.5（Internet-Draft の文書を暗号化する AEAD の復号例。鍵 `1c9240a5…`、タグ
//!   `eead9d67890cbb22392336fea1851f38`）。
//! - RFC 8439 付録 A.2（ChaCha20 の暗号化 3 ケース）と付録 A.3（Poly1305 の 11 ケース）は内部の
//!   `chacha20_xor`・`poly1305_mac` の照合で、公開 API からは見えないため、`embedded_extra` で
//!   `std.crypto` に試験用の入口を足した版を評価器で動かす。ネイティブでの内部演算の正しさは AEAD の
//!   ベクタ（上記と下の境界ケース）が間接的に固定する。付録 A.4（Poly1305 の一回限りの鍵）は
//!   A.2 の 1 つ目のケース（鍵 0・nonce 0・カウンタ 0）の先頭 32 バイトと同じ値で、AEAD のタグが固定する。
//! - 付録の値の入手: rfc-editor.org へはこの実行環境から到達できないため、pyca/cryptography が
//!   `vectors/` に収めた RFC 7539（RFC 8439 の旧版。付録 A のベクタは同一）の写しから取得した。
//!   ChaCha20: `ciphers/ChaCha20/rfc7539.txt`（SHA-256 `1b5a22f0…b6dc`）、Poly1305:
//!   `poly1305/rfc7539.txt`（SHA-256 `8f75ade6…00ab`）、A.5: `ciphers/ChaCha20Poly1305/openssl.txt` の
//!   Count 1（OpenSSL の `evpciph.txt` 由来。RFC 付録 A.5 と同一の入力）。取り込んだ値は
//!   `cryptography`（OpenSSL）で独立に再計算して一致を確認した。
//! - 追加のケース（平文 0〜200 バイト、AAD 0〜33 バイト。64 バイトのブロック境界と Poly1305 の
//!   16 バイト境界の前後を含む）の期待値は、同じ `cryptography` の ChaCha20Poly1305 で生成した
//!   （入力は決定的な並び `(seed*31 + i*7 + 3) % 256`）。
//! - 公開 API のテストは評価器とネイティブの両方で回る（`common::run` が突き合わせる）。
//!
//! 長さの上限（平文 274877906880 バイト）は、超える長さのバイト列をこの環境では作れないため、
//! 実際に超える入力での試験はしていない（`InputTooLong` を返す分岐は読んで確認した。PR 本文に記載）。
//! AAD の上限は 2^64 − 1 で、`Int` の最大（2^63 − 1）より大きいため、超える入力は作れない。

mod common;

use sigil::exec::{Arg, Options};
const HEAD: &str = "module crypto_chacha20_poly1305.t\n  targets server\n  trust none\n\n";

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
        "  let kr = ChaCha20Poly1305Key.from_secret(Secret.from_bytes(kb));
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
              let sealed = chacha20_poly1305_seal(&key, nonce1, &aad, &Secret.from_bytes(pb.clone()));
              match sealed {
                Err(e) => { return 3000 + code(e); }
                Ok(out) => {
                  let bad_seal = mismatch(&out, &want);
                  let opened = chacha20_poly1305_open(&key, &nonce2, &aad, &out);
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
        "  let kr = ChaCha20Poly1305Key.from_secret(Secret.from_bytes(kb));
  match kr {
    Err(e) => { return 1000 + code(e); }
    Ok(key) => {
      match Nonce96.from_slice(&iv) {
        Err(e) => { return 2000 + code(e); }
        Ok(nonce) => {
          match chacha20_poly1305_open(&key, &nonce, &aad, &sb) {
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
// RFC 8439 §2.8.2 と付録 A.5（AEAD）
// ---------------------------------------------------------------------------

#[test]
fn rfc8439_section_2_8_2_seals_and_opens() {
    let sealed = unhex("d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b61161ae10b594f09e26a7e902ecbd0600691");
    assert_eq!(
        run_roundtrip(
            &unhex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f"),
            &unhex("070000004041424344454647"),
            &unhex("50515253c0c1c2c3c4c5c6c7"),
            &unhex("4c616469657320616e642047656e746c656d656e206f662074686520636c617373206f66202739393a204966204920636f756c64206f6666657220796f75206f6e6c79206f6e652074697020666f7220746865206675747572652c2073756e73637265656e20776f756c642062652069742e"),
            &sealed
        ),
        0
    );
}

const A5_KEY: &str = "1c9240a5eb55d38af333888604f6b5f0473917c1402b80099dca5cbc207075c0";
const A5_NONCE: &str = "000000000102030405060708";
const A5_AAD: &str = "f33388860000000000004e91";
const A5_PT: &str = "496e7465726e65742d4472616674732061726520647261667420646f63756d656e74732076616c696420666f722061206d6178696d756d206f6620736978206d6f6e74687320616e64206d617920626520757064617465642c207265706c616365642c206f72206f62736f6c65746564206279206f7468657220646f63756d656e747320617420616e792074696d652e20497420697320696e617070726f70726961746520746f2075736520496e7465726e65742d447261667473206173207265666572656e6365206d6174657269616c206f7220746f2063697465207468656d206f74686572207468616e206173202fe2809c776f726b20696e2070726f67726573732e2fe2809d";
const A5_CT: &str = "64a0861575861af460f062c79be643bd5e805cfd345cf389f108670ac76c8cb24c6cfc18755d43eea09ee94e382d26b0bdb7b73c321b0100d4f03b7f355894cf332f830e710b97ce98c8a84abd0b948114ad176e008d33bd60f982b1ff37c8559797a06ef4f0ef61c186324e2b3506383606907b6a7c02b0f9f6157b53c867e4b9166c767b804d46a59b5216cde7a4e99040c5a40433225ee282a1b0a06c523eaf4534d7f83fa1155b0047718cbc546a0d072b04b3564eea1b422273f548271a0bb2316053fa76991955ebd63159434ecebb4e466dae5a1073a6727627097a1049e617d91d361094fa68f0ff77987130305beaba2eda04df997b714d6c6f2c29a6ad5cb4022b02709b";
const A5_TAG: &str = "eead9d67890cbb22392336fea1851f38";

#[test]
fn rfc8439_appendix_a5_decrypts_and_reseals() {
    let sealed = unhex(&format!("{A5_CT}{A5_TAG}"));
    assert_eq!(
        run_roundtrip(
            &unhex(A5_KEY),
            &unhex(A5_NONCE),
            &unhex(A5_AAD),
            &unhex(A5_PT),
            &sealed
        ),
        0
    );
}

/// 付録 A.5 の暗号文のまま、タグの最後の 4 ビットだけが違う入力（OpenSSL の Count 2）は認証に失敗する。
#[test]
fn rfc8439_appendix_a5_with_a_different_tag_fails_authentication() {
    let bad_tag = "eead9d67890cbb22392336fea1851f39";
    assert_ne!(bad_tag, A5_TAG);
    let sealed = unhex(&format!("{A5_CT}{bad_tag}"));
    assert_eq!(
        run_open(&unhex(A5_KEY), &unhex(A5_NONCE), &unhex(A5_AAD), &sealed),
        5
    );
}
// ---------------------------------------------------------------------------
// ブロック境界の前後（cryptography で生成）
// ---------------------------------------------------------------------------

/// (seed, 平文長, AAD 長, 暗号文 + タグ)。鍵は gen(32, seed)、nonce は gen(12, seed + 50)、
/// 平文は gen(pl, seed + 100)、AAD は gen(al, seed + 150)。
const BOUNDARY: &[(usize, usize, usize, &str)] = &[
    (2, 0, 0, "5f470afa3277be318ea7863da72fca5f"),
    (3, 1, 1, "bffcf339dc9e1a1058baed185a844be185"),
    (4, 15, 15, "5679a2a349475e15abb92adb47b1ac6c1870219d268f07fb2e03bb95022a28"),
    (5, 16, 16, "a277733a23b73b8eaab78a07a08c3cbb25b81d9214a52e1aba9efd998fbc7014"),
    (6, 17, 17, "01d201e7eb619d7e16abbe266d98fdab175fd354969dd98b3e7025a6e690041404"),
    (7, 31, 33, "75f3510de7b1d003cc03df31b59cda6351574aa613271c17158f323c94c9f8f1b3c5fc7d6962543039bce469ed8abe"),
    (8, 63, 0, "25d358dac9697c9d1d30edc38979778c9fbb3a56d7d4f784bdaeb36e15084a7e61f4f24c230cfcdd8170ab4bda8b57250c76af4ac08a23b3a29fcbcb1aebc369eb0dcdc1a61c97a7d82e53de7b123e"),
    (9, 64, 1, "077fee856ae21c53d89406a1ba9b9f36b3eccdd104f404eaff730b7a7589ad43126664a948beeb599d00fdea7d6a375cd4ec8783932d1797fbe909cedc076b151ca790d51fc32cf85a807baa9c92a0fa"),
    (10, 65, 15, "c89929c1c22277243fdd43a99fbe21f9b2839c4b181bf8ba78774ab45c2ebd5bea0dbc52739563824c37b1e833ed7dac71234bcc7490ed55afb3f10fd42dc78bd8894dab486f1017f428c4ea8e996b241f"),
    (11, 127, 16, "889fe1abef284aa5c14d04117426fea09c315027c023f526021d4f2bf85390dcf1b25068a7f3d87f0db13c1446b0359e7730602741cb914986f321c1eb0209ba946ae247805f793261e9749eba662ae0b640566c5d0d59c55005d46d647e0e627be19ca091a6920b479c094c55556bb27caf761d53ca11f46b902bebda8c262cd4dbb02499c88984327aa3dc7b4cea"),
    (12, 128, 17, "5ab35bd643af6af382a0e232c7fbce9720ff816ddf402eb2f15956c2dc910ff8716db10e26773a7bd974bdffa9b14504efbc22fba28db781f302e214867304262bde5ac060471612d929bf570d6d82afb0c2027c266ed339e9b9fa85e14f05cf109bd4b298dfc85d6e4bfc16e17f245c61c8c967de09ecb743e6ced43d10f9cc095cb83705929aa834a4e9e0999b80cc"),
    (13, 129, 33, "dbef6dac3e6191f03eb9da670dafc398b07e67cb3b595f4b3ac3d6618bceb6fcb1f7ae162e7a68c41b1a7cdb8db5784ccfbf029da42f78f261217338f2739b9c36b49c7a286e22f74b36aec9f23e75a936ec02f8128e47e24707ba8540c971a95c2a3d67e777ddb3698a5aba06ec2c6d390f5f61c4d9f7ca5e70c6a51d8158c8f4d00ac5959e461388a050a484a0534054"),
    (14, 200, 0, "747dde188be9a8af9cc51caa023f027eebfa998d4618c9e88db08c7c92d7c5df295d4333ec0fecb29fa8c4deda6d274cffcc515136f0e87ae7861c51c7b348f9ab892c15f447f3c6931c41433abc1c7d163eeb7c977f25ce2bb2814d80ba196d3c62bf607e2f1c961927a7f59afe217dd11013efa3a3840e6a7b20651eb018b1abdc0005c349adfad25099bb26cb5534df2e7801e18cf6e15c5fb85eedd482027ffbd2e80c2e1ed33a4a374bd4e533c9a2b8cb53ece3ec9b1aa502a25c25e264b75b590c9feb0e49544093783fe6c6f944082aef90676f73"),
];

#[test]
fn block_boundaries_match_the_independent_implementation() {
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

fn case() -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    (
        unhex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f"),
        unhex("070000004041424344454647"),
        unhex("50515253c0c1c2c3c4c5c6c7"),
        unhex("d31a8d34648e60db7b86afbc53ef7ec2a4aded51296e08fea9e2b5a736ee62d63dbea45e8ca9671282fafb69da92728b1a71de0a9e060b2905d6a5b67ecd3b3692ddbd7f2d778b8c9803aee328091b58fab324e4fad675945585808b4831d7bc3ff4def08e4b7a9de576d26586cec64b61161ae10b594f09e26a7e902ecbd0600691"),
    )
}

#[test]
fn open_accepts_the_untouched_input() {
    let (k, iv, a, s) = case();
    assert_eq!(run_open(&k, &iv, &a, &s), 0);
}

#[test]
fn open_rejects_a_modified_ciphertext_tag_aad_or_nonce() {
    let (k, iv, a, s) = case();
    let n = s.len();
    let mut c = s.clone();
    c[0] ^= 1;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "暗号文の先頭");
    let mut c = s.clone();
    c[n - 17] ^= 0x80;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "暗号文の末尾");
    let mut c = s.clone();
    c[n - 16] ^= 1;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "タグの先頭");
    let mut c = s.clone();
    c[n - 1] ^= 0x80;
    assert_eq!(run_open(&k, &iv, &a, &c), 5, "タグの末尾");
    let mut a2 = a.clone();
    a2[11] ^= 1;
    assert_eq!(run_open(&k, &iv, &a2, &s), 5, "AAD");
    let mut iv2 = iv.clone();
    iv2[11] ^= 1;
    assert_eq!(run_open(&k, &iv2, &a, &s), 5, "nonce");
    let mut k2 = k.clone();
    k2[0] ^= 1;
    assert_eq!(run_open(&k2, &iv, &a, &s), 5, "鍵");
    assert_eq!(run_open(&k, &iv, &a[..11], &s), 5, "AAD の短縮");
}

#[test]
fn open_rejects_inputs_shorter_than_the_tag() {
    let (k, iv, a, s) = case();
    assert_eq!(run_open(&k, &iv, &a, &[]), 3);
    assert_eq!(run_open(&k, &iv, &a, &s[..15]), 3);
    // ちょうどタグだけ（空の平文）は短すぎではなく、認証に失敗する。
    assert_eq!(run_open(&k, &iv, &a, &s[..16]), 5);
}

#[test]
fn key_and_nonce_lengths_are_checked_at_construction() {
    let (_k, iv, a, s) = case();
    assert_eq!(run_open(&[0; 31], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 33], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 0], &iv, &a, &s), 1001);
    assert_eq!(run_open(&[0; 32], &iv[..11], &a, &s), 2002);
    assert_eq!(run_open(&[0; 32], &[0; 13], &a, &s), 2002);
}
// ---------------------------------------------------------------------------
// RFC 8439 付録 A.2（ChaCha20）と付録 A.3（Poly1305）: 内部の `chacha20_xor`・`poly1305_mac`
// ---------------------------------------------------------------------------

/// (ID, 鍵, nonce, 初期カウンタ, 平文, 暗号文)。付録 A.2 の Test Vector #1〜#3。
const CHACHA_A2: &[(&str, &str, &str, i64, &str, &str)] = &[
    ("A.2 Test Vector #1", "0000000000000000000000000000000000000000000000000000000000000000", "000000000000000000000000", 0, "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000", "76b8e0ada0f13d90405d6ae55386bd28bdd219b8a08ded1aa836efcc8b770dc7da41597c5157488d7724e03fb8d84a376a43b8f41518a11cc387b669b2ee6586"),
    ("A.2 Test Vector #2", "0000000000000000000000000000000000000000000000000000000000000001", "000000000000000000000002", 1, "416e79207375626d697373696f6e20746f20746865204945544620696e74656e6465642062792074686520436f6e7472696275746f7220666f72207075626c69636174696f6e20617320616c6c206f722070617274206f6620616e204945544620496e7465726e65742d4472616674206f722052464320616e6420616e792073746174656d656e74206d6164652077697468696e2074686520636f6e74657874206f6620616e204945544620616374697669747920697320636f6e7369646572656420616e20224945544620436f6e747269627574696f6e222e20537563682073746174656d656e747320696e636c756465206f72616c2073746174656d656e747320696e20494554462073657373696f6e732c2061732077656c6c206173207772697474656e20616e6420656c656374726f6e696320636f6d6d756e69636174696f6e73206d61646520617420616e792074696d65206f7220706c6163652c207768696368206172652061646472657373656420746f", "a3fbf07df3fa2fde4f376ca23e82737041605d9f4f4f57bd8cff2c1d4b7955ec2a97948bd3722915c8f3d337f7d370050e9e96d647b7c39f56e031ca5eb6250d4042e02785ececfa4b4bb5e8ead0440e20b6e8db09d881a7c6132f420e52795042bdfa7773d8a9051447b3291ce1411c680465552aa6c405b7764d5e87bea85ad00f8449ed8f72d0d662ab052691ca66424bc86d2df80ea41f43abf937d3259dc4b2d0dfb48a6c9139ddd7f76966e928e635553ba76c5c879d7b35d49eb2e62b0871cdac638939e25e8a1e0ef9d5280fa8ca328b351c3c765989cbcf3daa8b6ccc3aaf9f3979c92b3720fc88dc95ed84a1be059c6499b9fda236e7e818b04b0bc39c1e876b193bfe5569753f88128cc08aaa9b63d1a16f80ef2554d7189c411f5869ca52c5b83fa36ff216b9c1d30062bebcfd2dc5bce0911934fda79a86f6e698ced759c3ff9b6477338f3da4f9cd8514ea9982ccafb341b2384dd902f3d1ab7ac61dd29c6f21ba5b862f3730e37cfdc4fd806c22f221"),
    ("A.2 Test Vector #3", "1c9240a5eb55d38af333888604f6b5f0473917c1402b80099dca5cbc207075c0", "000000000000000000000002", 42, "2754776173206272696c6c69672c20616e642074686520736c6974687920746f7665730a446964206779726520616e642067696d626c6520696e2074686520776162653a0a416c6c206d696d737920776572652074686520626f726f676f7665732c0a416e6420746865206d6f6d65207261746873206f757467726162652e", "62e6347f95ed87a45ffae7426f27a1df5fb69110044c0d73118effa95b01e5cf166d3df2d721caf9b21e5fb14c616871fd84c54f9d65b283196c7fe4f60553ebf39c6402c42234e32a356b3e764312a61a5532055716ead6962568f87d3f3f7704c6a8d1bcd1bf4d50d6154b6da731b187b58dfd728afa36757a797ac188d1"),
];

/// (ID, 鍵, メッセージ, タグ)。付録 A.3 の Test Vector #1〜#11（`COUNT` は 0 から数える）。
const POLY_A3: &[(&str, &str, &str, &str)] = &[
    ("A.3 Test Vector #1", "0000000000000000000000000000000000000000000000000000000000000000", "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000", "00000000000000000000000000000000"),
    ("A.3 Test Vector #2", "0000000000000000000000000000000036e5f6b5c5e06070f0efca96227a863e", "416e79207375626d697373696f6e20746f20746865204945544620696e74656e6465642062792074686520436f6e7472696275746f7220666f72207075626c69636174696f6e20617320616c6c206f722070617274206f6620616e204945544620496e7465726e65742d4472616674206f722052464320616e6420616e792073746174656d656e74206d6164652077697468696e2074686520636f6e74657874206f6620616e204945544620616374697669747920697320636f6e7369646572656420616e20224945544620436f6e747269627574696f6e222e20537563682073746174656d656e747320696e636c756465206f72616c2073746174656d656e747320696e20494554462073657373696f6e732c2061732077656c6c206173207772697474656e20616e6420656c656374726f6e696320636f6d6d756e69636174696f6e73206d61646520617420616e792074696d65206f7220706c6163652c207768696368206172652061646472657373656420746f", "36e5f6b5c5e06070f0efca96227a863e"),
    ("A.3 Test Vector #3", "36e5f6b5c5e06070f0efca96227a863e00000000000000000000000000000000", "416e79207375626d697373696f6e20746f20746865204945544620696e74656e6465642062792074686520436f6e7472696275746f7220666f72207075626c69636174696f6e20617320616c6c206f722070617274206f6620616e204945544620496e7465726e65742d4472616674206f722052464320616e6420616e792073746174656d656e74206d6164652077697468696e2074686520636f6e74657874206f6620616e204945544620616374697669747920697320636f6e7369646572656420616e20224945544620436f6e747269627574696f6e222e20537563682073746174656d656e747320696e636c756465206f72616c2073746174656d656e747320696e20494554462073657373696f6e732c2061732077656c6c206173207772697474656e20616e6420656c656374726f6e696320636f6d6d756e69636174696f6e73206d61646520617420616e792074696d65206f7220706c6163652c207768696368206172652061646472657373656420746f", "f3477e7cd95417af89a6b8794c310cf0"),
    ("A.3 Test Vector #4", "1c9240a5eb55d38af333888604f6b5f0473917c1402b80099dca5cbc207075c0", "2754776173206272696c6c69672c20616e642074686520736c6974687920746f7665730a446964206779726520616e642067696d626c6520696e2074686520776162653a0a416c6c206d696d737920776572652074686520626f726f676f7665732c0a416e6420746865206d6f6d65207261746873206f757467726162652e", "4541669a7eaaee61e708dc7cbcc5eb62"),
    ("A.3 Test Vector #5", "0200000000000000000000000000000000000000000000000000000000000000", "ffffffffffffffffffffffffffffffff", "03000000000000000000000000000000"),
    ("A.3 Test Vector #6", "02000000000000000000000000000000ffffffffffffffffffffffffffffffff", "02000000000000000000000000000000", "03000000000000000000000000000000"),
    ("A.3 Test Vector #7", "0100000000000000000000000000000000000000000000000000000000000000", "fffffffffffffffffffffffffffffffff0ffffffffffffffffffffffffffffff11000000000000000000000000000000", "05000000000000000000000000000000"),
    ("A.3 Test Vector #8", "0100000000000000000000000000000000000000000000000000000000000000", "fffffffffffffffffffffffffffffffffbfefefefefefefefefefefefefefefe01010101010101010101010101010101", "00000000000000000000000000000000"),
    ("A.3 Test Vector #9", "0200000000000000000000000000000000000000000000000000000000000000", "fdffffffffffffffffffffffffffffff", "faffffffffffffffffffffffffffffff"),
    ("A.3 Test Vector #10", "0100000000000000040000000000000000000000000000000000000000000000", "e33594d7505e43b900000000000000003394d7505e4379cd01000000000000000000000000000000000000000000000001000000000000000000000000000000", "14000000000000005500000000000000"),
    ("A.3 Test Vector #11", "0100000000000000040000000000000000000000000000000000000000000000", "e33594d7505e43b900000000000000003394d7505e4379cd010000000000000000000000000000000000000000000000", "13000000000000000000000000000000"),
];

fn internal_probe() -> String {
    let mut s = String::new();
    s.push_str("pub fn internal_probe() -> Int\n  intent \"RFC 8439 付録 A の内部演算を照合する\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n");
    for (i, (_id, k, n, ctr, p, c)) in CHACHA_A2.iter().enumerate() {
        s.push_str(&build_bytes(&format!("ck{i}"), &unhex(k)));
        s.push_str(&build_bytes(&format!("cn{i}"), &unhex(n)));
        s.push_str(&build_bytes(&format!("cp{i}"), &unhex(p)));
        s.push_str(&build_expected(&format!("cw{i}"), &unhex(c)));
        s.push_str(&format!(
            "  let cs{i} = Secret.from_bytes(ck{i});\n  let co{i} = chacha20_xor(&cs{i}, &cn{i}, {ctr}, &cp{i});\n  bad = bad + mismatch(&co{i}, &cw{i});\n"
        ));
    }
    for (i, (_id, k, m, t)) in POLY_A3.iter().enumerate() {
        s.push_str(&build_bytes(&format!("pk{i}"), &unhex(k)));
        s.push_str(&build_bytes(&format!("pm{i}"), &unhex(m)));
        s.push_str(&build_expected(&format!("pw{i}"), &unhex(t)));
        s.push_str(&format!(
            "  let ps{i} = Secret.from_bytes(pk{i});\n  let po{i} = poly1305_mac(&ps{i}, &pm{i});\n  bad = bad + mismatch(&po{i}, &pw{i});\n"
        ));
    }
    s.push_str("  return bad;\n}\n");
    s
}

#[test]
fn rfc8439_appendix_a2_and_a3_match_on_the_internal_functions() {
    let module = format!(
        "{}\n{}\n{}",
        include_str!("../src/stdlib/crypto.sigil"),
        MISMATCH_FN,
        internal_probe()
    );
    let entry = format!("{HEAD}pub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 7;\n}}\n");
    let o = Options {
        embedded_extra: vec![("std.crypto".to_string(), module)],
        embedded_roots: vec!["std.crypto".into()],
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(&[&entry], "std.crypto.internal_probe", &[], &o)
        .unwrap_or_else(|f| panic!("{:?}", f.error));
    assert_eq!(r.value, "0");
}

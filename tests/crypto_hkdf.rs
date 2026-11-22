//! ADR 0058 段 1a: HKDF-SHA-256（RFC 5869）の利用者向けテスト。
//!
//! - ベクタは RFC 5869 付録 A のテストケース 1〜3（SHA-256）。PRK と OKM を
//!   それぞれバイト単位で比べる。ケース 3 は salt・info が空（salt は
//!   長さ 0 の `Secret`）。salt 省略（`None`）は HashLen 個のゼロの salt と
//!   同じ PRK になることを、ケース 3 の IKM で確かめる。
//! - 出力長は 0、1、HashLen の前後（31・32・33）、255 × HashLen と、超過
//!   （契約違反で異常停止）を確かめる。長さ以外の中身は、先頭の
//!   ブロックが RFC のケース 1 の OKM と一致することと、長さの異なる
//!   出力が先頭一致の関係（OKM の接頭辞）であることで確かめる。
//! - 評価器とネイティブの両方で回る（`common::run` が突き合わせる）。

mod common;

use sigil::exec::{Arg, RunError, StopKind};

const HEAD: &str = "module crypto_hkdf.t\n  targets server\n  trust none\n\n";

const MISMATCH_FN: &str = "fn mismatch(d: &Bytes, want: &List<Int>) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut bad = 0;\n  let mut i = 0;\n  while i < want.len() {\n    let w = match want.get(i) { Some(v) => v, None => 0 };\n    let g = d.get(i).unwrap_or(0 - 1);\n    if g != w {\n      bad = bad + 1;\n    }\n    i = i + 1;\n  }\n  if d.byte_len() != want.len() {\n    bad = bad + 100;\n  }\n  return bad;\n}\n\n";

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

fn unhex(h: &str) -> Vec<u8> {
    (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .collect()
}

/// salt の与え方。
enum Salt<'a> {
    /// `None`（省略）。
    Omitted,
    /// 与えた（長さ 0 を含む）。
    Given(&'a [u8]),
}

fn salt_src(salt: &Salt) -> (String, &'static str) {
    match salt {
        Salt::Omitted => (String::new(), "None"),
        Salt::Given(b) => (
            format!(
                "{}  let salt = Secret.from_bytes(sb);\n",
                build_bytes("sb", b)
            ),
            "Some(salt)",
        ),
    }
}

fn ikm_src(ikm: &[u8]) -> String {
    format!(
        "{}  let ikm_s = Secret.from_bytes(ib);\n",
        build_bytes("ib", ikm)
    )
}

const MAIN_HEAD: &str =
    "fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n";

/// PRK を OKM の先頭 `out_len` バイトの代わりに比べるため、
/// PRK そのものは「info を空・out_len = 32」の Expand ではなく、
/// 内部表現を覗けないので、別の方法で確かめる: PRK を鍵とする Expand の
/// 結果を、Python（hmac）で検算した期待値と比べる（下の `prk_*`）。
fn run_okm(salt: &Salt, ikm: &[u8], info: &[u8], out_len: i64, expected_hex: &str) -> i64 {
    let (ssrc, sarg) = salt_src(salt);
    let src = format!(
        "{HEAD}{MISMATCH_FN}{MAIN_HEAD}{ssrc}{}{}  let prk = hkdf_extract_sha256({sarg}, &ikm_s);\n{}  let okm = hkdf_expand_sha256(&prk, &info, {out_len});\n  let got = okm.expose();\n  return mismatch(&got, &want);\n}}\n",
        ikm_src(ikm),
        build_bytes("info", info),
        build_expected("want", expected_hex),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

// RFC 5869 付録 A.1（基本）
const A1_IKM: &str = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";
const A1_SALT: &str = "000102030405060708090a0b0c";
const A1_INFO: &str = "f0f1f2f3f4f5f6f7f8f9";
const A1_OKM: &str =
    "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865";
// A.2（長い入力）
const A2_OKM: &str = "b11e398dc80327a1c8e7f78c596a49344f012eda2d4efad8a050cc4c19afa97c59045a99cac7827271cb41c65e590e09da3275600c2f09b8367793a9aca3db71cc30c58179ec3e87c14c01d5c1f3434f1d87";
// A.3（salt・info が空）
const A3_IKM: &str = "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b";
const A3_OKM: &str =
    "8da4e775a563c18f715f802a063c5a31b8a11f5c5ee1879ec3454e5f3c738d2d9d201395faa4b61a96c8";

fn a2_inputs() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
    let ikm: Vec<u8> = (0x00u8..=0x4f).collect();
    let salt: Vec<u8> = (0x60u8..=0xaf).collect();
    let info: Vec<u8> = (0xb0u8..=0xff).collect();
    (ikm, salt, info)
}

#[test]
fn hkdf_rfc5869_a1_basic() {
    let ikm = unhex(A1_IKM);
    let salt = unhex(A1_SALT);
    let info = unhex(A1_INFO);
    assert_eq!(run_okm(&Salt::Given(&salt), &ikm, &info, 42, A1_OKM), 0);
}

#[test]
fn hkdf_rfc5869_a2_long_inputs() {
    let (ikm, salt, info) = a2_inputs();
    assert_eq!(run_okm(&Salt::Given(&salt), &ikm, &info, 82, A2_OKM), 0);
}

#[test]
fn hkdf_rfc5869_a3_empty_salt_and_info() {
    let ikm = unhex(A3_IKM);
    assert_eq!(run_okm(&Salt::Given(&[]), &ikm, &[], 42, A3_OKM), 0);
}

#[test]
fn hkdf_omitted_salt_equals_hashlen_zero_salt() {
    // RFC 5869 §2.2: salt 省略は HashLen 個のゼロ。A.3 の IKM で、
    // 省略と 32 個のゼロの salt が同じ OKM になる（RFC 付録 A.3 の
    // 「空の salt」とも同じ値: 空の鍵はゼロ詰めで 32 個のゼロと等しい）。
    let ikm = unhex(A3_IKM);
    assert_eq!(run_okm(&Salt::Omitted, &ikm, &[], 42, A3_OKM), 0);
    assert_eq!(run_okm(&Salt::Given(&[0u8; 32]), &ikm, &[], 42, A3_OKM), 0);
}

// PRK のバイト列は型が内部を見せないため、Extract の結果を直接比べられない。
// 代わりに、RFC の PRK を RFC の OKM と同じ式で使う方法がある: Expand は
// PRK だけに依存するので、OKM が一致することは PRK が一致することを含む
// （T(1) = HMAC(PRK, info | 0x01) で、HMAC は PRK について単射ではないが、
// ここでは RFC の PRK で独立に計算した T(1) と一致するかを比べる）。
// 付録 A の PRK は次の値で、別系統（Python の hmac）で計算した OKM と
// 一致することを `hkdf_prk_matches_rfc_values` で確かめる。
const A1_PRK: &str = "077709362c2e32df0ddc3f0dc47bba6390b6c73bb50f9c3122ec844ad7c2b3e5";
const A2_PRK: &str = "06a6b88c5853361a06104c9ceb35b45cef760014904671014a193f40c15fc244";
const A3_PRK: &str = "19ef24a32c717b167f33a91d6f648bdf96596776afdb6377ac434c1c293ccb04";

/// RFC の PRK を HMAC の鍵とする T(1)（info | 0x01）が、Extract で作った PRK
/// からの Expand（out_len = 32）と一致することで、PRK を比べる。
fn run_prk(salt: &Salt, ikm: &[u8], info: &[u8], prk_hex: &str) -> i64 {
    let (ssrc, sarg) = salt_src(salt);
    let prk = unhex(prk_hex);
    let mut msg = info.to_vec();
    msg.push(1);
    let src = format!(
        "{HEAD}{MISMATCH_FN}{MAIN_HEAD}{ssrc}{}{}{}{}  let prk = hkdf_extract_sha256({sarg}, &ikm_s);\n  let okm = hkdf_expand_sha256(&prk, &info, 32);\n  let got = okm.expose();\n  let rk = Secret.from_bytes(pk);\n  let want_b = hmac_sha256(&rk, &m);\n  let want_e = want_b.expose();\n  let mut want = List.new();\n  let mut i = 0;\n  while i < want_e.byte_len() {{\n    want.push(want_e.get(i).unwrap_or(0));\n    i = i + 1;\n  }}\n  return mismatch(&got, &want);\n}}\n",
        ikm_src(ikm),
        build_bytes("info", info),
        build_bytes("pk", &prk),
        build_bytes("m", &msg),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

#[test]
fn hkdf_prk_matches_rfc_values() {
    let ikm = unhex(A1_IKM);
    let salt = unhex(A1_SALT);
    let info = unhex(A1_INFO);
    assert_eq!(run_prk(&Salt::Given(&salt), &ikm, &info, A1_PRK), 0);
    let (ikm2, salt2, info2) = a2_inputs();
    assert_eq!(run_prk(&Salt::Given(&salt2), &ikm2, &info2, A2_PRK), 0);
    let ikm3 = unhex(A3_IKM);
    assert_eq!(run_prk(&Salt::Given(&[]), &ikm3, &[], A3_PRK), 0);
}

fn stop_kind(src: &str) -> StopKind {
    match common::run(src, "main", &[] as &[Arg]) {
        Ok(o) => panic!("異常停止するはずが成功した: {}", o.value),
        Err(e) => match e.error {
            RunError::Stop(s) => s.kind,
            other => panic!("異常停止でない失敗: {other}"),
        },
    }
}

// ---------------------------------------------------------------------------
// 出力長
// ---------------------------------------------------------------------------

/// 出力長ごとに、長さと「A.1 の OKM との接頭辞の一致」を数える。
/// `out_len` が 42 以下なら全バイトが A.1 の OKM の先頭と一致するはず。
fn run_len(out_len: i64) -> (i64, i64) {
    let ikm = unhex(A1_IKM);
    let salt = unhex(A1_SALT);
    let info = unhex(A1_INFO);
    let want_prefix = unhex(A1_OKM);
    let src = format!(
        "{HEAD}{MAIN_HEAD}{}{}{}{}  let salt = Secret.from_bytes(sb);\n  let ikm = Secret.from_bytes(ib);\n  let prk = hkdf_extract_sha256(Some(salt), &ikm);\n  let okm = hkdf_expand_sha256(&prk, &info, {out_len});\n  let got = okm.expose();\n  let mut bad = 0;\n  let mut i = 0;\n  while i < got.byte_len() && i < pre.byte_len() {{\n    if got.get(i).unwrap_or(0 - 1) != pre.get(i).unwrap_or(0 - 2) {{\n      bad = bad + 1;\n    }}\n    i = i + 1;\n  }}\n  return got.byte_len() * 1000 + bad;\n}}\n",
        build_bytes("sb", &salt),
        build_bytes("ib", &ikm),
        build_bytes("info", &info),
        build_bytes("pre", &want_prefix),
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    let v: i64 = r.value.parse().unwrap();
    (v / 1000, v % 1000)
}

#[test]
fn hkdf_expand_output_lengths() {
    for (n, expect_prefix_ok) in [
        (0, true),
        (1, true),
        (31, true),
        (32, true),
        (33, true),
        (42, true),
    ] {
        let (len, bad) = run_len(n);
        assert_eq!(len, n, "out_len={n}");
        assert!(
            !expect_prefix_ok || bad == 0,
            "out_len={n} 接頭辞の不一致 {bad}"
        );
    }
}

#[test]
fn hkdf_expand_max_length_255_blocks() {
    let (len, _bad) = run_len(255 * 32);
    assert_eq!(len, 8160);
}

#[test]
fn hkdf_expand_over_limit_is_requires_violation() {
    let ikm = unhex(A1_IKM);
    let src = format!(
        "{HEAD}{MAIN_HEAD}{}  let ikm = Secret.from_bytes(ib);\n  let prk = hkdf_extract_sha256(None, &ikm);\n  let info = Bytes.new();\n  let okm = hkdf_expand_sha256(&prk, &info, 8161);\n  return okm.byte_len();\n}}\n",
        build_bytes("ib", &ikm),
    );
    assert_eq!(stop_kind(&src), StopKind::RequiresViolated);
}

#[test]
fn hkdf_expand_negative_length_is_requires_violation() {
    let ikm = unhex(A1_IKM);
    let src = format!(
        "{HEAD}{MAIN_HEAD}{}  let ikm = Secret.from_bytes(ib);\n  let prk = hkdf_extract_sha256(None, &ikm);\n  let info = Bytes.new();\n  let okm = hkdf_expand_sha256(&prk, &info, 0 - 1);\n  return okm.byte_len();\n}}\n",
        build_bytes("ib", &ikm),
    );
    assert_eq!(stop_kind(&src), StopKind::RequiresViolated);
}

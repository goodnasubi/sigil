//! ADR 0058 段 1c。RFC 7748 §5.2・§6.1 の公開ベクタと入力境界を
//! SIR 評価器とネイティブ出力の両方で照合する。

mod common;

use std::fmt::Write;

const HEAD: &str = r#"module crypto_x25519.t
  targets server
  trust none

fn vector() -> Result<Int, X25519Error>
  intent "X25519 の結果を既知値と照合する"
  requires none
  ensures none
  effects none
{
"#;

// RFC 7748 §6.1
const ALICE_PRIVATE: &str = "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a";
const ALICE_PUBLIC: &str = "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a";
const BOB_PRIVATE: &str = "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb";
const BOB_PUBLIC: &str = "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f";
const SHARED: &str = "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742";

const BASE_POINT: &str = "0900000000000000000000000000000000000000000000000000000000000000";

fn source(inputs: &[(&str, &str)], body: &str) -> String {
    let mut src = HEAD.to_owned();
    for &(name, hex) in inputs {
        writeln!(src, "  let mut {name} = Bytes.new();").unwrap();
        for i in (0..hex.len()).step_by(2) {
            let b = u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
            writeln!(src, "  {name}.push({b});").unwrap();
        }
    }
    src.push_str(body);
    src.push_str(
        r#"
}

fn main() -> Int
  intent "X25519 のテスト結果を返す"
  requires none
  ensures none
  effects none
{
  match vector() {
    Ok(code) => { return code; }
    Err(_) => { return 90; }
  }
}
"#,
    );
    src
}

fn run(inputs: &[(&str, &str)], body: &str) -> String {
    common::run(&source(inputs, body), "main", &[])
        .unwrap()
        .value
}

fn check_scalar(scalar: &str, point: &str, expected: &str) {
    assert_eq!(
        run(
            &[("scalar", scalar), ("point", point), ("want", expected)],
            r#"
  let key = X25519PrivateKey.from_secret(Secret.from_bytes(scalar))?;
  let peer = X25519PublicKey.from_bytes(point)?;
  let shared = x25519_shared_secret(&key, &peer)?;
  if ct_eq(&shared.expose(), &want) { return Ok(0); }
  return Ok(4);"#,
        ),
        "0"
    );
}

#[test]
fn rfc7748_section_5_2_scalar_multiplication() {
    // §5.2 の X25519 スカラー倍算ベクタ 1・2。
    check_scalar(
        "a546e36bf0527c9d3b16154b82465edd62144c0ac1fc5a18506a2244ba449ac4",
        "e6db6867583030db3594c1a424b15f7c726624ec26b3353b10a903a6d0ab1c4c",
        "c3da55379de9c6908e94ea4df28d084f32eccf03491c71f754b4075577a28552",
    );
    check_scalar(
        "4b66e9d4d1b4673c5ad22691957d6af5c11b6421e0ea01d42ca4169e7918ba0d",
        "e5210f12786811d3f4b7959d0538ae2c31dbe7106fc03c3efc4cd549c715a493",
        "95cbde9476e8907d7aade45cb4b873f88b595a68799fa152e6f8f7647aac7957",
    );
}

#[test]
fn rfc7748_section_5_2_iteration() {
    // §5.2 の反復ベクタのうち 1 回目と 1,000 回目を照合する。
    assert_eq!(
        run(
            &[
                ("k", BASE_POINT),
                ("u", BASE_POINT),
                (
                    "want_first",
                    "422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079",
                ),
                (
                    "want_last",
                    "684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51",
                ),
            ],
            r#"
  let mut i = 0;
  while i < 1000 {
    let key = X25519PrivateKey.from_secret(Secret.from_bytes(k.clone()))?;
    let peer = X25519PublicKey.from_bytes(u.clone())?;
    let next = x25519_shared_secret(&key, &peer)?;
    u = k.clone();
    k = next.expose();
    if i == 0 && !ct_eq(&k, &want_first) { return Ok(4); }
    i = i + 1;
  }
  if ct_eq(&k, &want_last) { return Ok(0); }
  return Ok(5);"#,
        ),
        "0"
    );
}

#[test]
fn rfc7748_section_6_1_key_exchange() {
    assert_eq!(
        run(
            &[
                ("alice_scalar", ALICE_PRIVATE),
                ("alice_want", ALICE_PUBLIC),
                ("bob_scalar", BOB_PRIVATE),
                ("bob_want", BOB_PUBLIC),
                ("shared_want", SHARED),
            ],
            r#"
  let alice = X25519PrivateKey.from_secret(Secret.from_bytes(alice_scalar))?;
  let bob = X25519PrivateKey.from_secret(Secret.from_bytes(bob_scalar))?;
  let alice_public = x25519_public_key(&alice);
  let bob_public = x25519_public_key(&bob);
  if !ct_eq(&alice_public.as_bytes(), &alice_want) { return Ok(3); }
  if !ct_eq(&bob_public.as_bytes(), &bob_want) { return Ok(4); }
  let alice_shared = x25519_shared_secret(&alice, &bob_public)?;
  let bob_shared = x25519_shared_secret(&bob, &alice_public)?;
  if !ct_eq(&alice_shared.expose(), &shared_want) { return Ok(7); }
  if !ct_eq(&bob_shared.expose(), &shared_want) { return Ok(8); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn rfc7748_section_5_public_key_decoding() {
    // 公開入力の最上位ビットを無視し、p 以上の値は p で剰余を取る。
    // 9 | (1 << 255) と p + 9 はいずれも基点 9 に対応する。
    check_scalar(
        ALICE_PRIVATE,
        "0900000000000000000000000000000000000000000000000000000000000080",
        ALICE_PUBLIC,
    );
    check_scalar(
        ALICE_PRIVATE,
        "f6ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        ALICE_PUBLIC,
    );
}

#[test]
fn rejects_wrong_key_lengths() {
    for n in [0, 31, 33] {
        let hex = "00".repeat(n);
        assert_eq!(
            run(
                &[("secret_bytes", &hex), ("public_bytes", &hex)],
                r#"
  match X25519PrivateKey.from_secret(Secret.from_bytes(secret_bytes)) {
    Err(err) => {
      if !err.is_invalid_key_length() { return Ok(3); }
    }
    Ok(_) => { return Ok(1); }
  }
  match X25519PublicKey.from_bytes(public_bytes) {
    Err(err) => {
      if !err.is_invalid_key_length() { return Ok(3); }
    }
    Ok(_) => { return Ok(2); }
  }
  return Ok(0);"#,
            ),
            "0",
            "鍵長 {n}"
        );
    }
}

#[test]
fn rejects_all_zero_shared_secret() {
    // §6.1 の全ゼロ共有値。§5 の非正規表現 p も 0 として処理する。
    // 低位数の点（位数 8 の点、p-1、p+1 など）も全ゼロ共有値となり拒否されることを確認する。
    for point in [
        "0000000000000000000000000000000000000000000000000000000000000000",
        "0100000000000000000000000000000000000000000000000000000000000000",
        "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157",
        "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800",
    ] {
        assert_eq!(
            run(
                &[("scalar", ALICE_PRIVATE), ("point", point)],
                r#"
  let key = X25519PrivateKey.from_secret(Secret.from_bytes(scalar))?;
  let peer = X25519PublicKey.from_bytes(point)?;
  match x25519_shared_secret(&key, &peer) {
    Err(err) => {
      if err.is_all_zero_shared_secret() { return Ok(0); }
      return Ok(3);
    }
    Ok(_) => { return Ok(4); }
  }"#,
            ),
            "0",
            "公開入力 {point}"
        );
    }
}

#[test]
fn x25519_working_values_are_dropped_as_secrets_on_both_paths() {
    let secret_drops = |point| {
        let src = source(
            &[("scalar", ALICE_PRIVATE), ("point", point)],
            r#"
  let expect_zero = point.get(0).unwrap_or(0) == 0;
  let key = X25519PrivateKey.from_secret(Secret.from_bytes(scalar))?;
  let peer = X25519PublicKey.from_bytes(point)?;
  match x25519_shared_secret(&key, &peer) {
    Ok(secret) => {
      if !expect_zero && secret.byte_len() == 32 { return Ok(0); }
      return Ok(1);
    }
    Err(err) => {
      if expect_zero && err.is_all_zero_shared_secret() { return Ok(0); }
      return Ok(2);
    }
  }"#,
        );
        let (ran, drops) = common::native::native_drops(&[&src], "main", &[]);
        assert_eq!(ran.code, Some(0), "{}", ran.stderr);
        assert_eq!(ran.stdout.trim(), "0");
        drops.iter().filter(|d| *d == "Secret(***)").count()
    };
    let success_drops = secret_drops(BASE_POINT);
    // 各反復で作業用の体の元が破棄され、最後に 32 バイトの共有値も破棄される。
    assert!(success_drops >= 255, "Secret の破棄数 {success_drops}");
    let failure_drops =
        secret_drops("0000000000000000000000000000000000000000000000000000000000000000");
    assert_eq!(failure_drops, success_drops);
}

#[test]
fn private_key_is_not_displayable() {
    let src = source(
        &[("scalar", ALICE_PRIVATE)],
        r#"
  let key = X25519PrivateKey.from_secret(Secret.from_bytes(scalar))?;
  console.log(&key);
  return Ok(0);"#,
    )
    .replace("effects none", "effects console");
    let err = match sigil::ir_eval::lower_sources(&[&src], "main") {
        Ok(_) => panic!("秘密鍵をログに表示できる"),
        Err(err) => err.to_string(),
    };
    assert!(
        err.contains("Displayable") || err.contains("表示"),
        "表示不可が理由のエラーでない: {err}"
    );
}

//! ADR 0064。RFC 6979 付録 A.2.5 の P-256/SHA-256 の公開ベクタと入力境界を
//! SIR 評価器とネイティブ出力の両方で照合する。

mod common;

use std::fmt::Write;

const HEAD: &str = r#"module crypto_ecdsa_p256.t
  targets server
  trust none

fn vector() -> Result<Int, EcdsaP256Error>
  intent "ECDSA P-256 の結果を既知値と照合する"
  requires none
  ensures none
  effects none
{
"#;

// RFC 6979 付録 A.2.5（P-256）
const KEY_D: &str = "c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721";
const KEY_UX: &str = "60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6";
const KEY_UY: &str = "7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299";
// 付録 A.2.5: メッセージ "sample" と "test"（SHA-256）の署名 (r, s)
const SAMPLE_R: &str = "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716";
const SAMPLE_S: &str = "f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const TEST_R: &str = "f1abb023518351cd71d881567b1ea663ed3efcf6c5132b354f28d3b0b7d38367";
const TEST_S: &str = "019f4113742a2b14bd25926b49c649155f267e60d3814b4c0cc84250e46f0083";
const ORDER_N: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551";
const PRIME_P: &str = "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff";

// 独立実装（Python cryptography 50.0.1、OpenSSL 3.0.13）が作った署名。(種類, 秘密鍵, 公開鍵, メッセージ, DER)。
// ランダムな nonce の署名で、r または s が短い（先頭バイトが 0）ものを含む。
const OPENSSL_VECTORS: &[(&str, &str, &str, &str, &str)] = &[
    ("normal", "df89e87e3ff411aff6627484811d776cad2f8655bcc9286282e3467c5600ef7d", "04e70df2941278e27ca238118a25099f8ea8f039968ea59f7260c4f41f01eacadc7fc51c2614ff81239a9f542566af9f33b5249456fa6dfc39d3c584b0ff947e67", "b04381bb104c3040e375bf0e7c9739663efcc56b", "3046022100ff4342bafcbc04c85c2d1d3799cab3082b2f650594280de1faf0966d4b55697f022100b843029c83d64cad979e2459ef9b2979eacf03bb17b8ab6aa83ed392371ed7f1"),
    ("short_r", "23e08c078cb704fa2e0596afbd3b8abd84a401a1ce6a56c2af5969f79daf2836", "04d3dadcca052dafacd451576a234fd6e7f4379d737adfb3a3b59b6f3e30b6495c2798bb02bd4a76f92420de58fef4afe2014608b6b1da691064f6a929cc0eaf2d", "98731f5542012a731747514bce21eb09b2f2e3f0", "304502200086914196f902e647de2cf3276eec3e4ab0577c3e76701ddc85072c3ea3ac9f022100f529b62851379b05c3311f4bed11f714501a18301a2319fcd19b663530c24a36"),
    ("short_s", "6924c3c1d67f55cf2975b641ac4f5bafe23667b1ecca68f15564af6d1f05e1f7", "0451c613bae7df4e50c9c6913ec3656ddf12586833f56c1eaf9dc4d82a7867115834afddcbb96bcfd6615e2c8c24fd861ba6dfebd698285e8e36f4fd6f02af7c13", "740f9ea84bab0585a50fa76dac56d5aa2d78c551", "3044022100963cf80f33e198734af5257c7adc7b54d317826fdb8d5a07113c019e40834d68021f75c251a81b2e933c557171efc0d35e498a4f08072229d5e81bbdb64d74cb0c"),
];

// 決定的な署名（RFC 6979 の独立した Python 実装が作り、OpenSSL が検証したもの）。(秘密鍵, 公開鍵, メッセージ, DER)。
const DETERMINISTIC_VECTORS: &[(&str, &str, &str, &str)] = &[
    ("e00961cc04e55c4533144d93fda113d960b6ad37e54a86a41bbba4f031e29d93", "042d7002503a2342ef9c732f524f5fe06515395123dad24599298eae3b9b186e4c19432295e06df2aa93c916d8e03072cb2f169223d885048e89bb92f33c804c6a", "656364736120703235362064657465726d696e697374696320736565642d61", "304502210091e241c06d7d3e422c5caafc9923c0c1fa0e9edfa271b143d9689ed578f794fa02200fc254833ef6f87a3c342299501b4f2510d269dc500565d5e7cf407aa4ee62a7"),
    ("04f5d0e8d9ed2af7d1bb4641b2e4bd62d4501d6cfd3d65bbab785b3501c9426c", "0450d5e0c2fb7ec8e0c9130188f00f6e649827e118d675bd8b827010594c4e65693fe72db226044bbd42b0b0d7e43048cd158360dacf8e2414559dec3fec00cf47", "656364736120703235362064657465726d696e697374696320736565642d62", "3045022100f8e2cdcddf24961f8ddeae1a700ec1928809f968110c90c805a3539ab059f83302204a03e6b3a79e7c609541ddf549e65aa0549703c6cfcfd6103091dbb5cab3450d"),
];

const MSG_SAMPLE: &str = "73616d706c65"; // "sample"
const MSG_TEST: &str = "74657374"; // "test"

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
  intent "ECDSA P-256 のテスト結果を返す"
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

/// 最短の DER（SEQUENCE { INTEGER r, INTEGER s }）を 16 進で返す。
fn der_hex(r: &str, s: &str) -> String {
    fn int(hex: &str) -> String {
        let mut b: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        while b.len() > 1 && b[0] == 0 {
            b.remove(0);
        }
        if b[0] >= 0x80 {
            b.insert(0, 0);
        }
        let mut out = vec![2u8, b.len() as u8];
        out.extend(b);
        out.iter().map(|x| format!("{x:02x}")).collect()
    }
    let body = format!("{}{}", int(r), int(s));
    format!("30{:02x}{body}", body.len() / 2)
}

#[test]
fn rfc6979_a25_public_key_derivation() {
    let mut pubkey = String::from("04");
    pubkey.push_str(KEY_UX);
    pubkey.push_str(KEY_UY);
    assert_eq!(
        run(
            &[("d", KEY_D), ("want", &pubkey)],
            r#"
  let key = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(d))?;
  let public = ecdsa_p256_public_key(&key);
  if ct_eq(&public.to_bytes(), &want) { return Ok(0); }
  return Ok(4);"#,
        ),
        "0"
    );
}

fn check_sign(msg: &str, r: &str, s: &str) {
    let want = der_hex(r, s);
    assert_eq!(
        run(
            &[("d", KEY_D), ("msg", msg), ("want", &want)],
            r#"
  let key = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(d))?;
  let sig = ecdsa_p256_sign(&key, &msg)?;
  if !ct_eq(&sig.to_der(), &want) { return Ok(4); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn rfc6979_a25_sign_sample() {
    check_sign(MSG_SAMPLE, SAMPLE_R, SAMPLE_S);
}

#[test]
fn rfc6979_a25_sign_test() {
    check_sign(MSG_TEST, TEST_R, TEST_S);
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn flip_last_byte(hex: &str) -> String {
    let mut b = hex_bytes(hex);
    let n = b.len();
    b[n - 1] ^= 1;
    to_hex(&b)
}

/// 先頭の INTEGER r の末尾バイトを反転した DER（長さ・形は変えない）。
fn flip_r_last(der: &str) -> String {
    let mut b = hex_bytes(der);
    let len_r = b[3] as usize;
    b[4 + len_r - 1] ^= 1;
    to_hex(&b)
}

fn verify_body(extra: &str) -> String {
    format!(
        r#"
  let public = EcdsaP256PublicKey.from_bytes(&pubkey)?;
  let sig = EcdsaP256Signature.from_der(&der)?;
  if !ecdsa_p256_verify(&public, &msg, &sig) {{ return Ok(4); }}
  if !ct_eq(&sig.to_der(), &der) {{ return Ok(5); }}
  let mut other_msg = msg.clone();
  other_msg.push(0);
  if ecdsa_p256_verify(&public, &other_msg, &sig) {{ return Ok(6); }}
  let bad_r = EcdsaP256Signature.from_der(&der_bad_r)?;
  if ecdsa_p256_verify(&public, &msg, &bad_r) {{ return Ok(7); }}
  let bad_s = EcdsaP256Signature.from_der(&der_bad_s)?;
  if ecdsa_p256_verify(&public, &msg, &bad_s) {{ return Ok(8); }}
  let other_key = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(other_d))?;
  if ecdsa_p256_verify(&ecdsa_p256_public_key(&other_key), &msg, &sig) {{ return Ok(9); }}
{extra}
  return Ok(0);"#
    )
}

/// 独立実装（OpenSSL）が作った署名を検証できる。形（短い r・s）の往復、改ざんの拒否を確かめる。
fn check_openssl_vector(index: usize) {
    let (kind, d, pubkey, msg, der) = OPENSSL_VECTORS[index];
    let bad_r = flip_r_last(der);
    let bad_s = flip_last_byte(der);
    let one = "0000000000000000000000000000000000000000000000000000000000000001";
    assert_eq!(
        run(
            &[
                ("d", d),
                ("pubkey", pubkey),
                ("msg", msg),
                ("der", der),
                ("der_bad_r", &bad_r),
                ("der_bad_s", &bad_s),
                ("other_d", one),
            ],
            &format!(
                "{}{}",
                // 秘密鍵から導いた公開鍵が、独立実装の公開鍵と一致する。
                r#"
  let key = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(d))?;
  if !ct_eq(&ecdsa_p256_public_key(&key).to_bytes(), &pubkey) { return Ok(3); }"#,
                verify_body("")
            ),
        ),
        "0",
        "{kind}"
    );
}

#[test]
fn openssl_signature_normal_is_verified() {
    check_openssl_vector(0);
}

#[test]
fn openssl_signature_with_short_r_is_verified() {
    check_openssl_vector(1);
}

#[test]
fn openssl_signature_with_short_s_is_verified() {
    check_openssl_vector(2);
}

/// 決定的な署名（独立実装が作り、OpenSSL が検証したもの）と一致し、検証にも通る。
#[test]
fn deterministic_vectors_from_an_independent_implementation() {
    for &(d, pubkey, msg, der) in DETERMINISTIC_VECTORS {
        assert_eq!(
            run(
                &[("d", d), ("pubkey", pubkey), ("msg", msg), ("der", der)],
                r#"
  let key = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(d))?;
  if !ct_eq(&ecdsa_p256_public_key(&key).to_bytes(), &pubkey) { return Ok(3); }
  let sig = ecdsa_p256_sign(&key, &msg)?;
  if !ct_eq(&sig.to_der(), &der) { return Ok(4); }
  let public = EcdsaP256PublicKey.from_bytes(&pubkey)?;
  if !ecdsa_p256_verify(&public, &msg, &sig) { return Ok(5); }
  return Ok(0);"#,
            ),
            "0"
        );
    }
}

/// RFC 6979 の署名も検証できる（`sample`）。
#[test]
fn rfc6979_signature_verifies_and_rejects_changes() {
    let der = der_hex(SAMPLE_R, SAMPLE_S);
    let mut pubkey = String::from("04");
    pubkey.push_str(KEY_UX);
    pubkey.push_str(KEY_UY);
    let bad_r = flip_r_last(&der);
    let bad_s = flip_last_byte(&der);
    let one = "0000000000000000000000000000000000000000000000000000000000000001";
    assert_eq!(
        run(
            &[
                ("pubkey", &pubkey),
                ("msg", MSG_SAMPLE),
                ("der", &der),
                ("der_bad_r", &bad_r),
                ("der_bad_s", &bad_s),
                ("other_d", one),
            ],
            &verify_body(""),
        ),
        "0"
    );
}

fn run_rejections(prefix: &str, ctor: &str, cases: &[String]) {
    // 各行は 16 進の入力。コンストラクタが誤りを返すことを確かめる。
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (i, hex) in cases.iter().enumerate() {
        inputs.push((format!("{prefix}{i}"), hex.clone()));
        body.push_str(&format!(
            "  match {ctor} {{\n    Ok(_) => {{ return Ok({}); }}\n    Err(_) => {{}}\n  }}\n",
            100 + i
        ));
    }
    body.push_str("  return Ok(0);");
    let refs: Vec<(&str, &str)> = inputs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    // ctor 内の `{p}` を各入力名に置き換えるため、行ごとに展開する。
    let mut expanded = String::new();
    for (i, _) in cases.iter().enumerate() {
        let name = format!("{prefix}{i}");
        expanded.push_str(&format!(
            "  match {} {{\n    Ok(_) => {{ return Ok({}); }}\n    Err(_) => {{}}\n  }}\n",
            ctor.replace("{p}", &name),
            100 + i
        ));
    }
    expanded.push_str("  return Ok(0);");
    let _ = body;
    assert_eq!(run(&refs, &expanded), "0", "{ctor}");
}

#[test]
fn private_key_import_boundaries() {
    let zero = "00".repeat(32);
    let mut n_plus_1 = ORDER_N.to_owned();
    n_plus_1 = {
        let mut b = hex_bytes(&n_plus_1);
        let last = b.len() - 1;
        b[last] += 1;
        to_hex(&b)
    };
    let rejected = vec![
        String::new(),
        "11".repeat(31),
        "11".repeat(33),
        zero,
        ORDER_N.to_owned(),
        n_plus_1,
        "ff".repeat(32),
        PRIME_P.to_owned(),
    ];
    run_rejections(
        "k",
        "EcdsaP256PrivateKey.from_secret(Secret.from_bytes({p}))",
        &rejected,
    );
    // 受理する境界: 1 と n - 1。
    let mut one = "00".repeat(31);
    one.push_str("01");
    let mut n_minus_1 = hex_bytes(ORDER_N);
    let last = n_minus_1.len() - 1;
    n_minus_1[last] -= 1;
    let n_minus_1 = to_hex(&n_minus_1);
    assert_eq!(
        run(
            &[("one", &one), ("top", &n_minus_1)],
            r#"
  let a = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(one))?;
  let b = EcdsaP256PrivateKey.from_secret(Secret.from_bytes(top))?;
  let pa = ecdsa_p256_public_key(&a).to_bytes();
  let pb = ecdsa_p256_public_key(&b).to_bytes();
  if pa.byte_len() != 65 || pb.byte_len() != 65 { return Ok(4); }
  if pa.get(0).map(|x| x + 0).unwrap_or(0) != 4 { return Ok(5); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn public_key_import_boundaries() {
    let mut valid = String::from("04");
    valid.push_str(KEY_UX);
    valid.push_str(KEY_UY);
    let with_prefix = |p: &str| format!("{p}{}", &valid[2..]);
    let mut off_curve = valid.clone();
    off_curve = flip_last_byte(&off_curve);
    let x_is_p = format!("04{PRIME_P}{KEY_UY}");
    let y_is_p = format!("04{KEY_UX}{PRIME_P}");
    let x_above_p = format!("04{}{KEY_UY}", "ff".repeat(32));
    let origin = format!("04{}{}", "00".repeat(32), "00".repeat(32));
    let rejected = vec![
        String::new(),
        "04".to_owned(),
        valid[..valid.len() - 2].to_owned(),
        format!("{valid}00"),
        with_prefix("00"),
        with_prefix("02"),
        with_prefix("03"),
        with_prefix("05"),
        with_prefix("06"),
        with_prefix("07"),
        off_curve,
        x_is_p,
        y_is_p,
        x_above_p,
        origin,
    ];
    run_rejections("p", "EcdsaP256PublicKey.from_bytes(&{p})", &rejected);
}

#[test]
fn signature_der_boundaries() {
    let ok = der_hex(SAMPLE_R, SAMPLE_S);
    let one = "00".repeat(31) + "01";
    let zero = "00".repeat(32);
    let mut n_minus_1 = hex_bytes(ORDER_N);
    let last = n_minus_1.len() - 1;
    n_minus_1[last] -= 1;
    let n_minus_1 = to_hex(&n_minus_1);
    let raw = |tag_r: &str, r: &str, tag_s: &str, s: &str| -> String {
        let body = format!("{tag_r}{r}{tag_s}{s}");
        format!("30{:02x}{body}", body.len() / 2)
    };
    let int = |hex: &str| format!("{:02x}{hex}", hex.len() / 2);
    let good_r = int("0100");
    let _ = good_r;
    let r = "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716";
    let s = "f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
    let rejected: Vec<String> = vec![
        // 長さ・全体の形
        String::new(),
        "30".to_owned(),
        ok[..ok.len() - 2].to_owned(),
        format!("{ok}00"),
        format!("31{}", &ok[2..]),
        format!("{}{}", &ok[..2], flip_last_byte(&ok[2..4])),
        // 長形式の長さ（最短でない）
        format!("3081{}{}", &ok[2..4], &ok[4..]),
        // INTEGER の符号・最短表現
        // 先頭 0 のあとが 0x80 未満（最短でない）
        raw(
            "02",
            &int(&format!("007f{}", "11".repeat(30))),
            "02",
            &int(&format!("00{s}")),
        ),
        raw(
            "02",
            &int(&format!("00{r}")),
            "02",
            &int(&format!("007f{}", "11".repeat(30))),
        ),
        raw("02", &int(r), "02", &int(&format!("00{s}"))),
        raw("02", &int(&format!("00{r}")), "02", &int(s)),
        raw("02", "0100", "02", &int(&format!("00{s}"))),
        raw("02", &int(&format!("00{r}")), "02", "0100"),
        raw("02", &int(&format!("00{r}")), "02", "00"),
        raw("02", &int(&format!("00{r}")), "03", &int(&format!("00{s}"))),
        raw("03", &int(&format!("00{r}")), "02", &int(&format!("00{s}"))),
        // 範囲: r・s が 0、n、n 以上
        raw("02", &int("00"), "02", &int(&format!("00{s}"))),
        raw("02", &int(&format!("00{r}")), "02", &int("00")),
        raw(
            "02",
            &int(&format!("00{ORDER_N}")),
            "02",
            &int(&format!("00{s}")),
        ),
        raw(
            "02",
            &int(&format!("00{r}")),
            "02",
            &int(&format!("00{ORDER_N}")),
        ),
        raw(
            "02",
            &int(&format!("00{}", "ff".repeat(32))),
            "02",
            &int(&format!("00{s}")),
        ),
        // 33 バイトの整数で上位が 0 でない / 34 バイト
        raw("02", &int(&format!("01{r}")), "02", &int(&format!("00{s}"))),
        raw(
            "02",
            &int(&format!("0000{r}")),
            "02",
            &int(&format!("00{s}")),
        ),
        // 整数の長さが残りを超える
        "3006020400000000".to_owned(),
    ];
    let _ = (&one, &zero, &n_minus_1);
    run_rejections("s", "EcdsaP256Signature.from_der(&{p})", &rejected);
    // 受理: 1 と n - 1、先頭 0 を持つ短い整数。往復して同じ DER に戻る。
    let accepted = [
        der_hex(&one, &n_minus_1),
        der_hex(&n_minus_1, &one),
        der_hex(SAMPLE_R, TEST_S),
        der_hex(TEST_S, SAMPLE_R),
    ];
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (i, hex) in accepted.iter().enumerate() {
        inputs.push((format!("a{i}"), hex.clone()));
        body.push_str(&format!(
            "  let g{i} = EcdsaP256Signature.from_der(&a{i})?;\n  if !ct_eq(&g{i}.to_der(), &a{i}) {{ return Ok({}); }}\n  if a{i}.byte_len() > 72 {{ return Ok({}); }}\n",
            100 + i,
            200 + i
        ));
    }
    body.push_str("  return Ok(0);");
    let refs: Vec<(&str, &str)> = inputs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(run(&refs, &body), "0");
}

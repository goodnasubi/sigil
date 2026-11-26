//! ADR 0065。RFC 5903 §8.1 の P-256 公開ベクタ、独立実装（Python cryptography 50.0.1 / OpenSSL）の固定ベクタ、
//! Wycheproof の ECDH 試験、入力境界を SIR 評価器とネイティブ出力の両方で照合する。

mod common;

use std::fmt::Write;

const HEAD: &str = r#"module crypto_ecdh_p256.t
  targets server
  trust none

fn vector() -> Result<Int, EcdhP256Error>
  intent "ECDH P-256 の結果を既知値と照合する"
  requires none
  ensures none
  effects none
{
"#;

// RFC 5903 §8.1（256 ビット ランダム ECP グループ）。i と r は両者の秘密鍵、(gix, giy) と (grx, gry) は公開鍵、
// girx は共有値。数値は OpenSSL でも導出して一致を確かめた。
const RFC5903_I: &str = "c88f01f510d9ac3f70a292daa2316de544e9aab8afe84049c62a9c57862d1433";
const RFC5903_GIX: &str = "dad0b65394221cf9b051e1feca5787d098dfe637fc90b9ef945d0c3772581180";
const RFC5903_GIY: &str = "5271a0461cdb8252d61f1c456fa3e59ab1f45b33accf5f58389e0577b8990bb3";
const RFC5903_R: &str = "c6ef9c5d78ae012a011164acb397ce2088685d8f06bf9be0b283ab46476bee53";
const RFC5903_GRX: &str = "d12dfb5289c8d4f81208b70270398c342296970a0bccb74c736fc7554494bf63";
const RFC5903_GRY: &str = "56fbf3ca366cc23e8157854c13c58d6aac23f046ada30f8353e74f33039872ab";
const RFC5903_Z: &str = "d6840f6b42f6edafd13116e0e12565202fef8e9ece7dce03812464d04b9442de";

// 独立実装（Python cryptography 50.0.1、OpenSSL 3.0.13）で導出した固定ベクタ。
const IND_A: &str = "dc655d5635ee95d96ee053435385e24a40f1ad80e7950f904360756f48387b9e";
const IND_A_PUB: &str = "04a805cdfc8546c0e74d694fd2a15c89774ffa4d8e00529059f40486a855f54acfc930ac1988f5c29de45cae21177166a694ad7b54b3a8c6906c31bc49c99f1cf5";
const IND_B: &str = "6800dd60c6258543bfcc7f8589ea568a0fdc6231f9022e4c839384e3599bcfe3";
const IND_B_PUB: &str = "04ba8eeafcdf8c61f0308250df472e5d4d581db258581e5f4394123ac91bec5770c1b7386763c5cce9a08cdf770b519cba02f7ab3bf55dee7c0703ca13e637ca27";
const IND_Z: &str = "781767641a14f33e5a7eb148f6c104d4de8c38e9f0c5ee3b7d1584ae4d78b736";
// d = 1 は基点 G、d = n - 1 は -G。ECDH(1, B) の X は B の X 座標そのもの。
const G_PUB: &str = "046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c2964fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5";
const GX: &str = "6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296";
const NEG_G_PUB: &str = "046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296b01cbd1c01e58065711814b583f061e9d431cca994cea1313449bf97c840ae0a";
const TWO_G_PUB: &str = "047cf27b188d034f7e8a52380304b51ac3c08969e277f21b35a60b48fc4766997807775510db8ed040293d9ac69f7430dbba7dade63ce982299e04b79d227873d1";
const ONE: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const TWO: &str = "0000000000000000000000000000000000000000000000000000000000000002";
const ORDER_N: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551";
const ORDER_N_MINUS_1: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632550";
const ORDER_N_MINUS_2: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc63254f";
const ORDER_N_PLUS_1: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632552";
const PRIME_P: &str = "ffffffff00000001000000000000000000000000ffffffffffffffffffffffff";
const PRIME_P_PLUS_1: &str = "ffffffff00000001000000000000000000000001000000000000000000000000";
const ALL_FF: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
const ZERO32: &str = "0000000000000000000000000000000000000000000000000000000000000000";

// RFC 8448 §5（HelloRetryRequest）の secp256r1 key_share に相当する 2 点。Botan の RFC 8448 再現トランスクリプト
// （ClientHello 2 と ServerHello の key_share、curve 0x0017）から取った。秘密鍵が公表されていないため、
// 共有値は照合できない（公開点の受理と往復のみ）。
const RFC8448_CLIENT_SHARE: &str = "04a6da7392ec591e17abfd535964b99894d13befb221b3def2ebe3830eac8f0151812677c4d6d2237e85cf01d6910cfb83954e76ba7352830534159897e8065780";
const RFC8448_SERVER_SHARE: &str = "04583e054b7a66672ae020ad9d2686fcc85b5ad41a134a0f03ee72b893052bd85b4c8de6776f5b04ac07d83540eab3e3d9c547bc6528c4317d294686093a6cad7d";

// Wycheproof ecdh_secp256r1_ecpoint_test.json（C2SP/wycheproof、testvectors_v1）の valid。(tcId, 公開点, 秘密鍵, 共有値)。
const WYCHEPROOF_VALID: &[(u32, &str, &str, &str)] = &[
    (1, "0462d5bd3372af75fe85a040715d0f502428e07046868b0bfdfa61d731afe44f26ac333a93a9e70a81cd5a95b5bf8d13990eb741c8c38872b4a07d275a014e30cf", "0612465c89a023ab17855b0a6bcebfd3febb53aef84138647b5352e02c10c346", "53020d908b0219328b658b525f26780e3ae12bcd952bb25a93bc0895e1714285"),
    (330, "04023819813ac969847059028ea88a1f30dfbcde03fc791d3a252c6b41211882eaf93e4ae433cc12cf2a43fc0ef26400c0e125508224cdb649380f25479148a4ad", "2a265f8bcbdcaf94d58519141e578124cb40d64a501fba9c11847b28965bc737", "4d4de80f1534850d261075997e3049321a0864082d24a917863366c0724f5ae3"),
    (331, "04cc11887b2d66cbae8f4d306627192522932146b42f01d3c6f92bd5c8ba739b06a2f08a029cd06b46183085bae9248b0ed15b70280c7ef13a457f5af382426031", "313f72ff9fe811bf573176231b286a3bdb6f1b14e05c40146590727a71c3bccd", "831c3f6b5f762d2f461901577af41354ac5f228c2591f84f8a6e51e2e3f17991"),
    (315, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "0000000000000000000000000000000000000000000000000000000000000003", "85a0b58519b28e70a694ec5198f72c4bfdabaa30a70f7143b5b1cd7536f716ca"),
    (316, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "00000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffff", "a329a7d80424ea2d6c904393808e510dfbb28155092f1bac284dceda1f13afe5"),
    (317, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "0100000000000000000000000000000000000000000000000000000000000000", "bd26d0293e8851c51ebe0d426345683ae94026aca545282a4759faa85fde6687"),
    (318, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "7fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff", "ea9350b2490a2010c7abf43fb1a38be729a2de375ea7a6ac34ff58cc87e51b6c"),
    (319, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "8000000000000000000000000000000000000000000000000000000000000000", "34eed3f6673d340b6f716913f6dfa36b5ac85fa667791e2d6a217b0c0b7ba807"),
    (320, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e83f3b9cac2fc632551", "1354ce6692c9df7b6fc3119d47c56338afbedccb62faa546c0fe6ed4959e41c3"),
    (321, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3a9cac2fc632551", "fe7496c30d534995f0bf428b5471c21585aaafc81733916f0165597a55d12cb4"),
    (322, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b1cac2fc632551", "348bf8042e4edf1d03c8b36ab815156e77c201b764ed4562cfe2ee90638ffef5"),
    (323, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac1fc632551", "6e4ec5479a7c20a537501700484f6f433a8a8fe53c288f7a25c8e8c92d39e8dc"),
    (324, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc6324f3", "f7407d61fdf581be4f564621d590ca9b7ba37f31396150f9922f1501da8c83ef"),
    (325, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632533", "82236fd272208693e0574555ca465c6cc512163486084fa57f5e1bd2e2ccc0b3"),
    (326, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632543", "06537149664dba1a9924654cb7f787ed224851b0df25ef53fcf54f8f26cd5f3f"),
    (327, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc63254b", "f2b38539bce995d443c7bfeeefadc9e42cc2c89c60bf4e86eac95d51987bd112"),
    (328, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc63254e", "85a0b58519b28e70a694ec5198f72c4bfdabaa30a70f7143b5b1cd7536f716ca"),
    (329, "0431028f3377fc8f2b1967edaab90213acad0da9f50897f08f57537f78f116744743a1930189363bbde2ac4cbd1649cdc6f451add71dd2f16a8a867f2b17caa16b", "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc63254f", "027b013a6f166db655d69d643c127ef8ace175311e667dff2520f5b5c75b7659"),
    (111, "04851eb1b8699d1cc01c6b6048139797dab3981c924f9ef81bd1f464c3182c870901285b2ae7cbcabfd5bccb9441eb3376b491595f60a1ec21dbe9f972bc32abbe", "e461c5b5e63d75b4c8c123bf8b9cd45e712af08f7e2e494a8f255ac9d80e058b", "6d03fef6b354840f4642fba1c8fce9f7d1cda8ae27b6ca01510fb44573fdaffb"),
    (213, "04276e45461a0e00704bee883b3e61016eec41678d8176d37eba25557a740284e05ab0fd1bf9fdac37fa83f83f5156648bb036d6ce46ff1f6ad4c72bc953214f0f", "809c461d8b39163537ff8f5ef5b977e4cdb980e70e38a7ee0b37cc876729e9ff", "eab63813807eec138a696be8b9e0023dd3c6cee35b7378174aac70838866c216"),
    (314, "04d2f211cfab84e01c8e5544036234debe35ae103bb878d7abcea6825f753e03a385f7f1870e64f1262af67a25ef9880419f45608e7f9da6dee83f5f46ceb53dcb", "809c461d8b39163537ff8f5ef5b977e4cdb980e70e38a7ee0b37cc876729e9ff", "f419febb32c254611adf569c2d583b17542b1538caa0001967f0a4bc34b8b789"),
    (48, "047fffffffffffffffffffffffeecf2230ffffffffffffffffffffffffffffffff00000001c7c30643abed0af0a49fe352cb483ff9b97dccdf427c658e8793240d", "55d55f11bb8da1ea318bca7266f0376662441ea87270aa2077f1b770c4854a48", "6fd26661851a8de3c6d06f834ef3acb8f2a5f9c136a985ffe10d5eeb51edcfa3"),
    (84, "04cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc75552486fd8e6bc424492204207fece59d93fb124390497f841d9c808c8cc3ec", "55d55f11bb8da1ea318bca7266f0376662441ea87270aa2077f1b770c4854a48", "0d2759dae57c906a8cca8541a56f429a6ffedf028212f7f0ad06b696ee655caf"),
    (110, "041c71c71c5555555571c71c71c71c71c71c71c71c8e38e38e38e38e38e38e38e46ef488581fbbd6b4ef78d03cdd161272243fdcc10979fd8e8fd199b98a63d378", "55d55f11bb8da1ea318bca7266f0376662441ea87270aa2077f1b770c4854a48", "7b5020080ce9bd76083ff132dc28ea996314be1a4ac21f987fbbe859f902fab1"),
    (3, "0458fd4168a87795603e2b04390285bdca6e57de6027fe211dd9d25e2212d29e62080d36bd224d7405509295eed02a17150e03b314f96da37445b0d1d29377d12c", "0a0d622a47e48f6bc1038ace438c6f528aa00ad2bd1da5f13ee46bf5f633d71a", "0000000000000000000000000000000000000000000000000000000000000000"),
    (30, "04eb6eb532fe4af2a5a275ab4e9f728ae64e9dbc08e9359091d75e5334a6e08db64c146339c05d1d5782ebeeb86d3eb5c63e9f99f17187c039666c5237b736e9cd", "0a0d622a47e48f6bc1038ace438c6f528aa00ad2bd1da5f13ee46bf5f633d71a", "fffffc000000ffffffc000000ffffffc000000ffffffc000000ffffffbffffff"),
    (65, "04293aa349b934ab2c839cf54b8a737df2304ef9b20fa494e31ad62b315dd6a53cee7e7d46a10b99156571780699e082fe867b3ea257dfbc0ac92e1195926a4af6", "0a0d622a47e48f6bc1038ace438c6f528aa00ad2bd1da5f13ee46bf5f633d71a", "000000000000000000000002099f55d5ffffffffffffffffffffffffffffffff"),
];

// 同 invalid（InvalidCurveAttack 16 件、InvalidEncoding、圧縮点・ツイスト上の圧縮点）。(tcId, 公開点)。
const WYCHEPROOF_INVALID: &[(u32, &str)] = &[
    (2, "0362d5bd3372af75fe85a040715d0f502428e07046868b0bfdfa61d731afe44f26"),
    (332, "0400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000"),
    (333, "0400000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001"),
    (334, "040000000000000000000000000000000000000000000000000000000000000000ffffffff00000001000000000000000000000000fffffffffffffffffffffffe"),
    (335, "040000000000000000000000000000000000000000000000000000000000000000ffffffff00000001000000000000000000000000ffffffffffffffffffffffff"),
    (336, "0400000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000000"),
    (337, "0400000000000000000000000000000000000000000000000000000000000000010000000000000000000000000000000000000000000000000000000000000001"),
    (338, "040000000000000000000000000000000000000000000000000000000000000001ffffffff00000001000000000000000000000000fffffffffffffffffffffffe"),
    (339, "040000000000000000000000000000000000000000000000000000000000000001ffffffff00000001000000000000000000000000ffffffffffffffffffffffff"),
    (340, "04ffffffff00000001000000000000000000000000fffffffffffffffffffffffe0000000000000000000000000000000000000000000000000000000000000000"),
    (341, "04ffffffff00000001000000000000000000000000fffffffffffffffffffffffe0000000000000000000000000000000000000000000000000000000000000001"),
    (342, "04ffffffff00000001000000000000000000000000fffffffffffffffffffffffeffffffff00000001000000000000000000000000fffffffffffffffffffffffe"),
    (343, "04ffffffff00000001000000000000000000000000fffffffffffffffffffffffeffffffff00000001000000000000000000000000ffffffffffffffffffffffff"),
    (344, "04ffffffff00000001000000000000000000000000ffffffffffffffffffffffff0000000000000000000000000000000000000000000000000000000000000000"),
    (345, "04ffffffff00000001000000000000000000000000ffffffffffffffffffffffff0000000000000000000000000000000000000000000000000000000000000001"),
    (346, "04ffffffff00000001000000000000000000000000ffffffffffffffffffffffffffffffff00000001000000000000000000000000fffffffffffffffffffffffe"),
    (347, "04ffffffff00000001000000000000000000000000ffffffffffffffffffffffffffffffff00000001000000000000000000000000ffffffffffffffffffffffff"),
    (348, ""),
    (349, "02fd4bf61763b46581fd9174d623516cf3c81edd40e29ffa2777fb6cb0ae3ce535"),
    (350, "03efdde3b32872a9effcf3b94cbf73aa7b39f9683ece9121b9852167f4e3da609b"),
    (351, "02efdde3b32872a9effcf3b94cbf73aa7b39f9683ece9121b9852167f4e3da609b"),
];

fn push_hex(src: &mut String, name: &str, hex: &str) {
    writeln!(src, "  let mut {name} = Bytes.new();").unwrap();
    for i in (0..hex.len()).step_by(2) {
        let b = u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        writeln!(src, "  {name}.push({b});").unwrap();
    }
}

fn source(inputs: &[(&str, &str)], body: &str) -> String {
    let mut src = HEAD.to_owned();
    for &(name, hex) in inputs {
        push_hex(&mut src, name, hex);
    }
    src.push_str(body);
    src.push_str(
        r#"
}

fn main() -> Int
  intent "ECDH P-256 のテスト結果を返す"
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

fn cat(parts: &[&str]) -> String {
    parts.concat()
}

fn uncompressed(x: &str, y: &str) -> String {
    cat(&["04", x, y])
}

#[test]
fn rfc5903_public_keys() {
    let pub_i = uncompressed(RFC5903_GIX, RFC5903_GIY);
    let pub_r = uncompressed(RFC5903_GRX, RFC5903_GRY);
    assert_eq!(
        run(
            &[
                ("i", RFC5903_I),
                ("r", RFC5903_R),
                ("want_i", &pub_i),
                ("want_r", &pub_r)
            ],
            r#"
  let ki = EcdhP256PrivateKey.from_secret(Secret.from_bytes(i))?;
  let kr = EcdhP256PrivateKey.from_secret(Secret.from_bytes(r))?;
  if !ct_eq(&ecdh_p256_public_key(&ki).to_bytes(), &want_i) { return Ok(4); }
  if !ct_eq(&ecdh_p256_public_key(&kr).to_bytes(), &want_r) { return Ok(5); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn rfc5903_shared_value_both_directions() {
    let pub_i = uncompressed(RFC5903_GIX, RFC5903_GIY);
    let pub_r = uncompressed(RFC5903_GRX, RFC5903_GRY);
    assert_eq!(
        run(
            &[
                ("i", RFC5903_I),
                ("r", RFC5903_R),
                ("pub_i", &pub_i),
                ("pub_r", &pub_r),
                ("want", RFC5903_Z),
            ],
            r#"
  let ki = EcdhP256PrivateKey.from_secret(Secret.from_bytes(i))?;
  let kr = EcdhP256PrivateKey.from_secret(Secret.from_bytes(r))?;
  let peer_r = EcdhP256PublicKey.from_bytes(&pub_r)?;
  let peer_i = EcdhP256PublicKey.from_bytes(&pub_i)?;
  let z1 = ecdh_p256_shared_secret(&ki, &peer_r);
  let z2 = ecdh_p256_shared_secret(&kr, &peer_i);
  if z1.byte_len() != 32 { return Ok(3); }
  if !ct_eq(&z1.expose(), &want) { return Ok(4); }
  if !ct_eq(&z2.expose(), &want) { return Ok(5); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn rfc8448_key_shares_are_accepted_and_round_trip() {
    assert_eq!(
        run(
            &[("c", RFC8448_CLIENT_SHARE), ("s", RFC8448_SERVER_SHARE)],
            r#"
  let pc = EcdhP256PublicKey.from_bytes(&c)?;
  let ps = EcdhP256PublicKey.from_bytes(&s)?;
  if !ct_eq(&pc.to_bytes(), &c) { return Ok(4); }
  if !ct_eq(&ps.to_bytes(), &s) { return Ok(5); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn independent_fixed_vector_and_symmetry() {
    assert_eq!(
        run(
            &[
                ("a", IND_A),
                ("b", IND_B),
                ("pub_a", IND_A_PUB),
                ("pub_b", IND_B_PUB),
                ("want", IND_Z),
            ],
            r#"
  let ka = EcdhP256PrivateKey.from_secret(Secret.from_bytes(a))?;
  let kb = EcdhP256PrivateKey.from_secret(Secret.from_bytes(b))?;
  if !ct_eq(&ecdh_p256_public_key(&ka).to_bytes(), &pub_a) { return Ok(3); }
  if !ct_eq(&ecdh_p256_public_key(&kb).to_bytes(), &pub_b) { return Ok(4); }
  let za = ecdh_p256_shared_secret(&ka, &EcdhP256PublicKey.from_bytes(&pub_b)?);
  let zb = ecdh_p256_shared_secret(&kb, &EcdhP256PublicKey.from_bytes(&pub_a)?);
  if !ct_eq(&za.expose(), &want) { return Ok(5); }
  if !ct_eq(&zb.expose(), &want) { return Ok(6); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn extreme_scalars_one_two_and_n_minus_one() {
    // ECDH(1, P) = X(P)、ECDH(n-1, P) = X(-P) = X(P)、ECDH(2, G) = X(2G)、ECDH(n-2, G) = X(-2G) = X(2G)。
    let two_g_x = &TWO_G_PUB[2..66];
    assert_eq!(
        run(
            &[
                ("one", ONE),
                ("two", TWO),
                ("nm1", ORDER_N_MINUS_1),
                ("nm2", ORDER_N_MINUS_2),
                ("pub_b", IND_B_PUB),
                ("pub_g", G_PUB),
                ("pub_ng", NEG_G_PUB),
                ("pub_2g", TWO_G_PUB),
                ("bx", &IND_B_PUB[2..66]),
                ("gx", GX),
                ("two_gx", two_g_x),
            ],
            r#"
  let k1 = EcdhP256PrivateKey.from_secret(Secret.from_bytes(one))?;
  let k2 = EcdhP256PrivateKey.from_secret(Secret.from_bytes(two))?;
  let km1 = EcdhP256PrivateKey.from_secret(Secret.from_bytes(nm1))?;
  let km2 = EcdhP256PrivateKey.from_secret(Secret.from_bytes(nm2))?;
  let b = EcdhP256PublicKey.from_bytes(&pub_b)?;
  let g = EcdhP256PublicKey.from_bytes(&pub_g)?;
  let ng = EcdhP256PublicKey.from_bytes(&pub_ng)?;
  if !ct_eq(&ecdh_p256_shared_secret(&k1, &b).expose(), &bx) { return Ok(3); }
  if !ct_eq(&ecdh_p256_shared_secret(&km1, &b).expose(), &bx) { return Ok(4); }
  if !ct_eq(&ecdh_p256_shared_secret(&k1, &g).expose(), &gx) { return Ok(5); }
  if !ct_eq(&ecdh_p256_shared_secret(&km1, &g).expose(), &gx) { return Ok(6); }
  if !ct_eq(&ecdh_p256_shared_secret(&k1, &ng).expose(), &gx) { return Ok(7); }
  if !ct_eq(&ecdh_p256_shared_secret(&k2, &g).expose(), &two_gx) { return Ok(8); }
  if !ct_eq(&ecdh_p256_shared_secret(&km2, &g).expose(), &two_gx) { return Ok(9); }
  // 公開鍵の導出も端の値で合う。
  if !ct_eq(&ecdh_p256_public_key(&k1).to_bytes(), &pub_g) { return Ok(10); }
  if !ct_eq(&ecdh_p256_public_key(&km1).to_bytes(), &pub_ng) { return Ok(11); }
  if !ct_eq(&ecdh_p256_public_key(&k2).to_bytes(), &pub_2g) { return Ok(12); }
  // 対称性: ECDH(a, B) = ECDH(b, A) を複数の鍵の組で。
  if !ct_eq(&ecdh_p256_shared_secret(&k2, &EcdhP256PublicKey.from_bytes(&pub_ng)?).expose(),
            &ecdh_p256_shared_secret(&km1, &EcdhP256PublicKey.from_bytes(&pub_2g)?).expose()) { return Ok(13); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn symmetry_for_several_key_pairs() {
    let pub_i = uncompressed(RFC5903_GIX, RFC5903_GIY);
    let pub_r = uncompressed(RFC5903_GRX, RFC5903_GRY);
    assert_eq!(
        run(
            &[
                ("i", RFC5903_I),
                ("r", RFC5903_R),
                ("a", IND_A),
                ("b", IND_B),
                ("nm1", ORDER_N_MINUS_1),
                ("pub_i", &pub_i),
                ("pub_r", &pub_r),
                ("pub_a", IND_A_PUB),
                ("pub_b", IND_B_PUB),
                ("pub_ng", NEG_G_PUB),
            ],
            r#"
  let ki = EcdhP256PrivateKey.from_secret(Secret.from_bytes(i))?;
  let kr = EcdhP256PrivateKey.from_secret(Secret.from_bytes(r))?;
  let ka = EcdhP256PrivateKey.from_secret(Secret.from_bytes(a))?;
  let kb = EcdhP256PrivateKey.from_secret(Secret.from_bytes(b))?;
  let kn = EcdhP256PrivateKey.from_secret(Secret.from_bytes(nm1))?;
  let pi = EcdhP256PublicKey.from_bytes(&pub_i)?;
  let pr = EcdhP256PublicKey.from_bytes(&pub_r)?;
  let pa = EcdhP256PublicKey.from_bytes(&pub_a)?;
  let pb = EcdhP256PublicKey.from_bytes(&pub_b)?;
  let pn = EcdhP256PublicKey.from_bytes(&pub_ng)?;
  if !ct_eq(&ecdh_p256_shared_secret(&ki, &pa).expose(), &ecdh_p256_shared_secret(&ka, &pi).expose()) { return Ok(3); }
  if !ct_eq(&ecdh_p256_shared_secret(&kr, &pb).expose(), &ecdh_p256_shared_secret(&kb, &pr).expose()) { return Ok(4); }
  if !ct_eq(&ecdh_p256_shared_secret(&kn, &pi).expose(), &ecdh_p256_shared_secret(&ki, &pn).expose()) { return Ok(5); }
  if !ct_eq(&ecdh_p256_shared_secret(&ka, &pb).expose(), &ecdh_p256_shared_secret(&kb, &pa).expose()) { return Ok(6); }
  // 別の鍵からは別の共有値になる。
  if ct_eq(&ecdh_p256_shared_secret(&ka, &pb).expose(), &ecdh_p256_shared_secret(&ka, &pr).expose()) { return Ok(7); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn wycheproof_valid_points_give_the_published_shared_value() {
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (n, &(id, public, private, shared)) in WYCHEPROOF_VALID.iter().enumerate() {
        inputs.push((format!("p{n}"), public.to_owned()));
        inputs.push((format!("d{n}"), private.to_owned()));
        inputs.push((format!("z{n}"), shared.to_owned()));
        writeln!(
            body,
            "  let k{n} = EcdhP256PrivateKey.from_secret(Secret.from_bytes(d{n}))?;\n  let q{n} = EcdhP256PublicKey.from_bytes(&p{n})?;\n  if !ct_eq(&ecdh_p256_shared_secret(&k{n}, &q{n}).expose(), &z{n}) {{ return Ok({}); }}",
            100 + n
        )
        .unwrap();
        let _ = id;
    }
    body.push_str("  return Ok(0);");
    let refs: Vec<(&str, &str)> = inputs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(run(&refs, &body), "0");
}

#[test]
fn wycheproof_invalid_points_are_rejected() {
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (n, &(_, public)) in WYCHEPROOF_INVALID.iter().enumerate() {
        inputs.push((format!("p{n}"), public.to_owned()));
        writeln!(
            body,
            "  match EcdhP256PublicKey.from_bytes(&p{n}) {{\n    Ok(_) => {{ return Ok({}); }}\n    Err(_) => {{}}\n  }}",
            100 + n
        )
        .unwrap();
    }
    body.push_str("  return Ok(0);");
    let refs: Vec<(&str, &str)> = inputs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    assert_eq!(run(&refs, &body), "0");
}

/// 公開鍵として受け付けない符号化の一覧。(説明, 16 進)。
fn rejected_public_encodings() -> Vec<(&'static str, String)> {
    let base = IND_B_PUB;
    let (x, y) = (&base[2..66], &base[66..130]);
    vec![
        ("empty", String::new()),
        ("length 64", base[..128].to_owned()),
        ("length 66", cat(&[base, "00"])),
        ("length 33 compressed", cat(&["02", x])),
        ("leading 00", cat(&["00", x, y])),
        ("leading 02", cat(&["02", x, y])),
        ("leading 03", cat(&["03", x, y])),
        ("leading 05", cat(&["05", x, y])),
        ("leading 06 hybrid", cat(&["06", x, y])),
        ("leading 07 hybrid", cat(&["07", x, y])),
        ("x = p", uncompressed(PRIME_P, y)),
        ("y = p", uncompressed(x, PRIME_P)),
        ("x = p + 1", uncompressed(PRIME_P_PLUS_1, y)),
        ("x = 2^256 - 1", uncompressed(ALL_FF, y)),
        ("y = 2^256 - 1", uncompressed(x, ALL_FF)),
        // x ≡ Gx (mod p) になる Gx + p は 256 ビットを超えるので作れない。代わりに x = 0 を（y と合わない）。
        ("off curve: last byte of y flipped", {
            let mut v = base.to_owned();
            v.replace_range(128..130, "28");
            v
        }),
        ("off curve: last byte of x flipped", {
            let mut v = base.to_owned();
            v.replace_range(64..66, "28");
            v
        }),
        ("(0, 0)", uncompressed(ZERO32, ZERO32)),
        ("x = 0, y = 0 with valid y", uncompressed(ZERO32, y)),
        ("swapped X and Y", uncompressed(y, x)),
        ("swapped X and Y of A", {
            let a = IND_A_PUB;
            uncompressed(&a[66..130], &a[2..66])
        }),
        ("(1, 1)", uncompressed(ONE, ONE)),
    ]
}

#[test]
fn malformed_and_off_curve_public_keys_are_rejected() {
    let cases = rejected_public_encodings();
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (n, (_, hex)) in cases.iter().enumerate() {
        inputs.push((format!("p{n}"), hex.clone()));
        writeln!(
            body,
            "  match EcdhP256PublicKey.from_bytes(&p{n}) {{\n    Ok(_) => {{ return Ok({}); }}\n    Err(_) => {{}}\n  }}",
            100 + n
        )
        .unwrap();
    }
    body.push_str("  return Ok(0);");
    let refs: Vec<(&str, &str)> = inputs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let got = run(&refs, &body);
    if got != "0" {
        let idx = got.parse::<usize>().unwrap() - 100;
        panic!("受理された: {} ({})", idx, cases[idx].0);
    }
}

#[test]
fn public_key_rejection_reports_invalid_public_key() {
    assert_eq!(
        run(
            &[("p", &cat(&["04", PRIME_P, PRIME_P]))],
            r#"
  match EcdhP256PublicKey.from_bytes(&p) {
    Ok(_) => { return Ok(1); }
    Err(EcdhP256Error.InvalidPublicKey) => { return Ok(0); }
    Err(EcdhP256Error.InvalidPrivateKey) => { return Ok(2); }
  }"#,
        ),
        "0"
    );
}

#[test]
fn valid_public_keys_are_accepted_and_round_trip() {
    assert_eq!(
        run(
            &[
                ("g", G_PUB),
                ("ng", NEG_G_PUB),
                ("a", IND_A_PUB),
                ("b", IND_B_PUB),
            ],
            r#"
  if !ct_eq(&EcdhP256PublicKey.from_bytes(&g)?.to_bytes(), &g) { return Ok(3); }
  if !ct_eq(&EcdhP256PublicKey.from_bytes(&ng)?.to_bytes(), &ng) { return Ok(4); }
  if !ct_eq(&EcdhP256PublicKey.from_bytes(&a)?.to_bytes(), &a) { return Ok(5); }
  if !ct_eq(&EcdhP256PublicKey.from_bytes(&b)?.to_bytes(), &b) { return Ok(6); }
  return Ok(0);"#,
        ),
        "0"
    );
}

#[test]
fn private_key_range_is_enforced() {
    let rejected: Vec<(&str, String)> = vec![
        ("d = 0", ZERO32.to_owned()),
        ("d = n", ORDER_N.to_owned()),
        ("d = n + 1", ORDER_N_PLUS_1.to_owned()),
        ("d = 2^256 - 1", ALL_FF.to_owned()),
        ("length 31", ONE[2..].to_owned()),
        ("length 33", cat(&["00", ONE])),
        ("length 0", String::new()),
        ("length 64", cat(&[ONE, ONE])),
    ];
    let mut inputs: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for (n, (_, hex)) in rejected.iter().enumerate() {
        inputs.push((format!("d{n}"), hex.clone()));
        writeln!(
            body,
            "  match EcdhP256PrivateKey.from_secret(Secret.from_bytes(d{n})) {{\n    Ok(_) => {{ return Ok({}); }}\n    Err(EcdhP256Error.InvalidPrivateKey) => {{}}\n    Err(EcdhP256Error.InvalidPublicKey) => {{ return Ok({}); }}\n  }}",
            100 + n,
            200 + n
        )
        .unwrap();
    }
    for (name, hex) in [
        ("ok_one", ONE),
        ("ok_nm1", ORDER_N_MINUS_1),
        ("ok_two", TWO),
    ] {
        inputs.push((name.to_owned(), hex.to_owned()));
        writeln!(
            body,
            "  let k_{name} = EcdhP256PrivateKey.from_secret(Secret.from_bytes({name}))?;"
        )
        .unwrap();
    }
    body.push_str("  return Ok(0);");
    let refs: Vec<(&str, &str)> = inputs
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let got = run(&refs, &body);
    if got != "0" {
        let c = got.parse::<usize>().unwrap();
        panic!("秘密鍵の検査が誤り: コード {c} ({})", rejected[c % 100].0);
    }
}

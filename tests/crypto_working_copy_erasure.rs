//! Issue #687: 暗号部品の作業コピーを破棄時にゼロ化する（HMAC・PBKDF2・HKDF）
//!
//! hmac_sha256・verify_hmac_sha256・pbkdf2_sha256・hkdf_sha256（Extract / Expand）
//! および内部 HMAC（hmac_any_key・hmac_from_pads）において、Secret.expose() の複製と
//! 導出途中の作業バッファ（kb, ipad, opad, inner_hash, msg, u, t, okm 等）が
//! 破棄時に Secret として取り込まれ、確実にゼロ化されることを検証する。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module crypto_erasure.t\n  targets server\n  trust none\n\n";

#[test]
fn hmac_working_copies_erased_as_secret() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut kb = Bytes.new();\n  let mut i = 0;\n  while i < 32 {{\n    kb.push(i + 1);\n    i = i + 1;\n  }}\n  let key = Secret.from_bytes(kb);\n  let mut msg = Bytes.new();\n  msg.push(0x61);\n  let mac = hmac_sha256(&key, &msg);\n  return mac.byte_len();\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    // key, mac に加えて、作業コピー（kb, inner, outer, inner_hash）が Secret としてゼロ化・破棄される
    assert!(
        secret_drops >= 5,
        "hmac_sha256 の作業コピーが Secret(***) として破棄されていない: count={secret_drops}, drops={drops:?}"
    );
}

#[test]
fn verify_hmac_working_copies_erased_as_secret() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut kb = Bytes.new();\n  let mut i = 0;\n  while i < 32 {{\n    kb.push(i + 1);\n    i = i + 1;\n  }}\n  let key = Secret.from_bytes(kb);\n  let mut msg = Bytes.new();\n  msg.push(0x61);\n  let mut expected = Bytes.new();\n  let mut j = 0;\n  while j < 32 {{\n    expected.push(0);\n    j = j + 1;\n  }}\n  let ok = verify_hmac_sha256(&key, &msg, &expected);\n  if ok {{ return 1; }} else {{ return 0; }}\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    // key, verify 内の mac, 作業コピー群, got_wipe が Secret としてゼロ化・破棄される
    assert!(
        secret_drops >= 6,
        "verify_hmac_sha256 の作業コピーが Secret(***) として破棄されていない: count={secret_drops}, drops={drops:?}"
    );
}

#[test]
fn hkdf_extract_and_expand_working_copies_erased_as_secret() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut ikm_b = Bytes.new();\n  let mut i = 0;\n  while i < 16 {{\n    ikm_b.push(i + 1);\n    i = i + 1;\n  }}\n  let ikm = Secret.from_bytes(ikm_b);\n  let prk = hkdf_extract_sha256(None, &ikm);\n  let mut info = Bytes.new();\n  info.push(0x5a);\n  let okm = hkdf_expand_sha256(&prk, &info, 42);\n  return okm.byte_len();\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    // ikm, prk.key, okm に加え、Extract の sb_wipe, ikm_wipe, hmac_any_key の作業バッファ、
    // Expand の kb_wipe, msg_wipe, old_t_wipe, t_wipe, okm_wipe 等が Secret としてゼロ化・破棄される
    assert!(
        secret_drops >= 10,
        "hkdf の作業コピーが Secret(***) として破棄されていない: count={secret_drops}, drops={drops:?}"
    );
}

#[test]
fn pbkdf2_working_copies_erased_as_secret() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects task.suspend\n{{\n  let mut pw = Bytes.new();\n  pw.push(0x70);\n  let password = Secret.from_bytes(pw);\n  let mut salt = Bytes.new();\n  salt.push(0x73);\n  let dk = pbkdf2_sha256(&password, &salt, 4096, 32);\n  return dk.byte_len();\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    // password, dk に加え、ipad_wipe, opad_wipe, kb_wipe, msg_wipe, 反復ごとの old_u_wipe / old_t_wipe,
    // 終了時の u_wipe, t_wipe, out_wipe など多数の作業コピーが Secret としてゼロ化・破棄される
    assert!(
        secret_drops >= 100,
        "pbkdf2 の作業コピーが Secret(***) として破棄されていない: count={secret_drops}"
    );
}

#[test]
fn evaluation_and_native_run_match() {
    // 評価器とネイティブの両経路で突き合わせ（common::run）が一致することを検証する
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut kb = Bytes.new();\n  let mut i = 0;\n  while i < 32 {{\n    kb.push(i + 1);\n    i = i + 1;\n  }}\n  let key = Secret.from_bytes(kb);\n  let mut msg = Bytes.new();\n  msg.push(0x61);\n  let mac = hmac_sha256(&key, &msg);\n  let prk = hkdf_extract_sha256(Some(key), &mac);\n  let mut info = Bytes.new();\n  let okm = hkdf_expand_sha256(&prk, &info, 32);\n  return okm.byte_len();\n}}\n"
    );
    let res = common::run(&src, "main", &[] as &[Arg]).unwrap();
    assert_eq!(res.value, "32");
}

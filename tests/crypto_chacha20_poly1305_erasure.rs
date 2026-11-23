//! ADR 0058 段 2a: ChaCha20-Poly1305 の作業領域（キーストリーム、一回限りの Poly1305 鍵、
//! Poly1305 の h・r）を、破棄時に `Secret` としてゼロ化することの確認。
//!
//! 作業領域は `Secret` に収めてあるため、ネイティブの破棄の記録（`native_drops`）に
//! `Secret(***)` として現れる。平文は `Secret` の中身を `Bytes` へ複製せず借用するので、
//! `expose()` 由来の複製の破棄は現れない。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module chacha_erasure.t\n  targets server\n  trust none\n\n";

const PROGRAM: &str = "fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut kb = Bytes.new();\n  let mut i = 0;\n  while i < 32 {\n    kb.push(i + 1);\n    i = i + 1;\n  }\n  let mut iv = Bytes.new();\n  let mut j = 0;\n  while j < 12 {\n    iv.push(j);\n    j = j + 1;\n  }\n  let mut pt = Bytes.new();\n  pt.push(0x61);\n  pt.push(0x62);\n  let aad = Bytes.new();\n  match ChaCha20Poly1305Key.from_secret(Secret.from_bytes(kb)) {\n    Err(e) => { return 0 - 1; }\n    Ok(key) => {\n      match Nonce96.from_slice(&iv) {\n        Err(e) => { return 0 - 1; }\n        Ok(n1) => {\n          match Nonce96.from_slice(&iv) {\n            Err(e) => { return 0 - 1; }\n            Ok(n2) => {\n              match chacha20_poly1305_seal(&key, n1, &aad, &Secret.from_bytes(pt)) {\n                Err(e) => { return 0 - 1; }\n                Ok(sealed) => {\n                  match chacha20_poly1305_open(&key, &n2, &aad, &sealed) {\n                    Err(e) => { return 0 - 1; }\n                    Ok(o) => { return o.byte_len(); }\n                  }\n                }\n              }\n            }\n          }\n        }\n      }\n    }\n  }\n}\n";

#[test]
fn chacha20_poly1305_working_state_is_erased_as_secret() {
    let src = format!("{HEAD}{PROGRAM}");
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    // seal と open は、それぞれキーストリーム（暗号化 1 ブロックと一回限りの鍵 1 ブロック）、
    // Poly1305 の r と h（更新ごとの置き換えを含む）を `Secret` で破棄する。
    // この入力（平文 2 バイト、AAD なし）では合計 15 個を観測した。余裕をみて 12 個以上を要求する。
    assert!(
        secret_drops >= 12,
        "ChaCha20-Poly1305 の作業領域が Secret(***) として破棄されていない: count={secret_drops}, drops={drops:?}"
    );
}

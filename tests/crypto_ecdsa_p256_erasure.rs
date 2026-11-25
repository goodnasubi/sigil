//! ADR 0064 決定 2: ECDSA P-256 の秘密（秘密鍵のスカラー、体演算と点演算の作業領域、
//! nonce と HMAC の状態）を、成功・失敗の両経路で破棄時にゼロ化することの確認。
//!
//! 作業領域は `Secret` に収めてあるため、ネイティブの破棄の記録（`native_drops`）に
//! `Secret(***)` として現れる。nonce の DRBG の状態など `Bytes` に出る中間値は、
//! 使い終わりに `Secret.from_bytes` で取り込んで破棄する（`*_wipe`）ので、同じく現れる。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module ecdsa_erasure.t\n  targets server\n  trust none\n\n";

fn key_bytes(len: usize, fill: &str) -> String {
    let mut s = String::from("  let mut kb = Bytes.new();\n");
    for i in 0..len {
        s.push_str(&format!(
            "  kb.push({});\n",
            if fill == "i" { i + 1 } else { 0 }
        ));
    }
    s
}

fn count(body: &str) -> usize {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    drops.iter().filter(|d| *d == "Secret(***)").count()
}

#[test]
fn signing_working_state_is_erased_as_secret() {
    let body = format!(
        "{}  let mut msg = Bytes.new();\n  msg.push(0x61);\n  match EcdsaP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 1; }}\n    Ok(key) => {{\n      match ecdsa_p256_sign(&key, &msg) {{\n        Err(e) => {{ return 2; }}\n        Ok(sig) => {{ return 0; }}\n      }}\n    }}\n  }}\n",
        key_bytes(32, "i")
    );
    let n = count(&body);
    // 署名 1 回で候補 4 つ分の体演算の作業領域・スカラー・DRBG の状態を `Secret` で破棄する。
    // 実測は下の assert のメッセージに出る。下限は余裕をみた値。
    assert!(
        n >= 5000,
        "署名の作業領域が Secret(***) として破棄されていない: count={n}"
    );
}

#[test]
fn public_key_derivation_working_state_is_erased_as_secret() {
    let body = format!(
        "{}  match EcdsaP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 1; }}\n    Ok(key) => {{\n      let public = ecdsa_p256_public_key(&key);\n      return 0;\n    }}\n  }}\n",
        key_bytes(32, "i")
    );
    let n = count(&body);
    assert!(
        n >= 1000,
        "公開鍵導出の作業領域が Secret(***) として破棄されていない: count={n}"
    );
}

#[test]
fn rejected_private_key_input_is_still_erased() {
    // 失敗経路: 範囲外（0）の入力と長さ違いの入力も、`Secret` として破棄される。
    let zero = format!(
        "{}  match EcdsaP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 0; }}\n    Ok(key) => {{ return 1; }}\n  }}\n",
        key_bytes(32, "0")
    );
    assert!(count(&zero) >= 2, "範囲外の入力の破棄が記録されない");
    let short = format!(
        "{}  match EcdsaP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 0; }}\n    Ok(key) => {{ return 1; }}\n  }}\n",
        key_bytes(31, "i")
    );
    assert!(count(&short) >= 1, "長さ違いの入力の破棄が記録されない");
}

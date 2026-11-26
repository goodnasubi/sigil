//! ADR 0065 決定 2・3: ECDH P-256 の秘密（秘密鍵のスカラー、体演算と点演算の作業領域、共有値）を、
//! 成功・失敗の両経路で破棄時にゼロ化することの確認。作業領域は `Secret` に収めてあるため、
//! ネイティブの破棄の記録（`native_drops`）に `Secret(***)` として現れる。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module ecdh_erasure.t\n  targets server\n  trust none\n\n";

fn bytes_decl(name: &str, bytes: &[u8]) -> String {
    let mut s = format!("  let mut {name} = Bytes.new();\n");
    for b in bytes {
        s.push_str(&format!("  {name}.push({b});\n"));
    }
    s
}

fn key_bytes(len: usize, fill: &str) -> String {
    let v: Vec<u8> = (0..len)
        .map(|i| if fill == "i" { (i + 1) as u8 } else { 0 })
        .collect();
    bytes_decl("kb", &v)
}

/// 基点 G の SEC 1 非圧縮点。
fn peer_bytes() -> String {
    let hex = "046b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c2964fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5";
    let v: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    bytes_decl("pb", &v)
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
fn shared_secret_working_state_is_erased_as_secret() {
    let body = format!(
        "{}{}  match EcdhP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 1; }}\n    Ok(key) => {{\n      match EcdhP256PublicKey.from_bytes(&pb) {{\n        Err(e) => {{ return 2; }}\n        Ok(peer) => {{\n          let z = ecdh_p256_shared_secret(&key, &peer);\n          if z.byte_len() != 32 {{ return 3; }}\n          return 0;\n        }}\n      }}\n    }}\n  }}\n",
        key_bytes(32, "i"),
        peer_bytes()
    );
    let n = count(&body);
    // 256 回の点演算の作業領域・スカラー・共有値が `Secret` で破棄される。実測は assert のメッセージに出る。
    assert!(
        n >= 1000,
        "共有値の作業領域が Secret(***) として破棄されていない: count={n}"
    );
}

#[test]
fn public_key_derivation_working_state_is_erased_as_secret() {
    let body = format!(
        "{}  match EcdhP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 1; }}\n    Ok(key) => {{\n      let public = ecdh_p256_public_key(&key);\n      if public.to_bytes().byte_len() != 65 {{ return 2; }}\n      return 0;\n    }}\n  }}\n",
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
        "{}  match EcdhP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 0; }}\n    Ok(key) => {{ return 1; }}\n  }}\n",
        key_bytes(32, "0")
    );
    assert!(count(&zero) >= 2, "範囲外の入力の破棄が記録されない");
    let short = format!(
        "{}  match EcdhP256PrivateKey.from_secret(Secret.from_bytes(kb)) {{\n    Err(e) => {{ return 0; }}\n    Ok(key) => {{ return 1; }}\n  }}\n",
        key_bytes(31, "i")
    );
    assert!(count(&short) >= 1, "長さ違いの入力の破棄が記録されない");
}

//! ADR 0058 段 2b: SHA-384 と HKDF-SHA-384 の作業コピーを、破棄時に `Secret` としてゼロ化することの確認。
//!
//! 作業コピー（メッセージスケジュール、パディング済みの全体、状態の出力、HMAC の鍵のパッドと内側・外側の連結、
//! PRK から取り出した鍵の複製）は `Secret` に収めてあるため、ネイティブの破棄の記録（`native_drops`）に
//! `Secret(***)` として現れる。SHA-256 と同じ限界として、局所の `Int`（作業変数 a〜h など）は
//! 破棄時に消せない（`Int` は値の型で、記述子による破棄を持たない）。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module sha384_erasure.t\n  targets server\n  trust none\n\n";

#[test]
fn sha384_working_copies_erased_as_secret() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut msg = Bytes.new();\n  msg.push(0x61);\n  let h = sha384(&msg);\n  return h.byte_len();\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    eprintln!("sha384 の Secret(***) の破棄: {secret_drops}");
    // メッセージスケジュール（1 ブロック分）、パディング済みの全体、状態の出力の 3 つ。
    assert!(
        secret_drops >= 3,
        "sha384 の作業コピーが Secret(***) として破棄されていない: count={secret_drops}, drops={drops:?}"
    );
}

#[test]
fn hkdf_sha384_extract_and_expand_working_copies_erased_as_secret() {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut ikm_b = Bytes.new();\n  let mut i = 0;\n  while i < 16 {{\n    ikm_b.push(i + 1);\n    i = i + 1;\n  }}\n  let ikm = Secret.from_bytes(ikm_b);\n  let prk = hkdf_extract_sha384(None, &ikm);\n  let mut info = Bytes.new();\n  info.push(0x5a);\n  let okm = hkdf_expand_sha384(&prk, &info, 42);\n  return okm.byte_len();\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let secret_drops = drops.iter().filter(|d| *d == "Secret(***)").count();
    eprintln!("hkdf_sha384 の Secret(***) の破棄: {secret_drops}");
    // Extract は鍵のパッド・内側・外側・内側のハッシュ・salt と IKM の複製、Expand は 1 ブロック分の同じ作業コピーと
    // 出力の複製を `Secret` で破棄する。この入力（IKM 16 バイト、info 1 バイト、出力 42 バイト）では 34 個を観測した。
    // 余裕をみて 30 個以上を要求する。
    assert!(
        secret_drops >= 30,
        "hkdf_sha384 の作業コピーが Secret(***) として破棄されていない: count={secret_drops}, drops={drops:?}"
    );
}

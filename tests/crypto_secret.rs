//! PR 6-1b: `Secret` 型と定数時間比較の利用者向けテスト（ADR 0024 決定 1）。
//!
//! - `Secret.from_bytes` で取り込み、`expose` でだけ中身を取り出せる。
//! - `Secret` は `Displayable` でない（ADR 0020 R6: ログ・エラーへの混入防止）。
//! - `Secret` は `Copyable` でない（所有権が移る）。
//! - 自由関数 `ct_eq`（std.crypto の主たる比較。実体は `std.core.bytes.ct_equal`）
//!   の意味: 長さが違えば false、同じなら中身の一致で決まる。
//! - 破棄時のゼロ化そのものは、ランタイム（runtime/core）の単体テスト
//!   `secret_drop_zeroizes_and_show_hides` と、記述子の種類（17）を出す
//!   生成系で担保する。ここのテストは評価器とネイティブの両方で回り
//!   （`common::run` が突き合わせる）、破棄の経路も両方で踏む。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module crypto_secret.t\n  targets server\n  trust none\n\n";

fn run(body: &str) -> i64 {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

fn check_err(src: &str) -> String {
    match sigil::ir_eval::lower_sources(&[src], "main") {
        Ok(_) => panic!("検査が通ってしまった"),
        Err(e) => format!("{e}"),
    }
}

#[test]
fn secret_round_trip() {
    // 取り込み → 長さと中身（expose 経由）を取り出す。
    let v = run(
        "  let mut b = Bytes.new();\n  b.push(10);\n  b.push(20);\n  b.push(30);\n  let s = Secret.from_bytes(b);\n  let inner = s.expose();\n  return s.len() * 1000 + inner.get(0).unwrap_or(0) * 100 + inner.get(1).unwrap_or(0) * 10 + inner.get(2).unwrap_or(0);",
    );
    assert_eq!(v, 3000 + 1000 + 200 + 30);
}

#[test]
fn secret_empty() {
    let v = run("  let s = Secret.from_bytes(Bytes.new());\n  return s.len() + s.expose().len();");
    assert_eq!(v, 0);
}

#[test]
fn secret_is_not_displayable() {
    // console.log に渡すと検査で落ちる（Displayable を実装しない）。
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects console\n{{\n  let s = Secret.from_bytes(Bytes.new());\n  console.log(&s);\n  return 0;\n}}\n"
    );
    let e = check_err(&src);
    assert!(
        e.contains("Displayable") || e.contains("表示"),
        "表示不可が理由のエラーでない: {e}"
    );
}

#[test]
fn secret_is_not_copyable() {
    // 値を渡したあとに元の束縛を使うと、移動済みとして検査で落ちる。
    let src = format!(
        "{HEAD}fn take(s: Secret) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return s.len();\n}}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let s = Secret.from_bytes(Bytes.new());\n  let a = take(s);\n  return a + s.len();\n}}\n"
    );
    let e = check_err(&src);
    assert!(!e.is_empty());
}

#[test]
fn ct_eq_meaning() {
    // 等しい → 1、同じ長さで中身が違う → 0、長さが違う → 0。
    let v = run(
        "  let mut a = Bytes.new();\n  a.push(1);\n  a.push(2);\n  let mut b = Bytes.new();\n  b.push(1);\n  b.push(2);\n  let mut c = Bytes.new();\n  c.push(1);\n  c.push(3);\n  let mut d = Bytes.new();\n  d.push(1);\n  let mut acc = 0;\n  if ct_eq(&a, &b) {\n    acc = acc + 1;\n  }\n  if ct_eq(&a, &c) {\n    acc = acc + 10;\n  }\n  if ct_eq(&a, &d) {\n    acc = acc + 100;\n  }\n  return acc;",
    );
    assert_eq!(v, 1);
}

#[test]
fn secret_compare_via_expose_and_ct_eq() {
    // 秘密同士の照合は expose したバイト列を ct_eq で比べる形になる。
    let v = run(
        "  let mut a = Bytes.new();\n  a.push(7);\n  let mut b = Bytes.new();\n  b.push(7);\n  let x = Secret.from_bytes(a);\n  let y = Secret.from_bytes(b);\n  let xb = x.expose();\n  let yb = y.expose();\n  if ct_eq(&xb, &yb) {\n    return 1;\n  }\n  return 0;",
    );
    assert_eq!(v, 1);
}

#[test]
fn user_defined_secret_is_a_plain_struct() {
    // レビュー指摘の回帰試験: 利用者が同名の `struct Secret` を定義しても、
    // 秘密扱い（記述子の種類 17・破棄前のゼロ化）にはならない。名前だけで
    // 判定していたときは、先頭フィールドを `Bytes` とみなしてしまい、
    // (Int, String) 形ではネイティブが segfault し、String や List<Int> が
    // 先頭の形では中身が黙ってゼロ化されていた。3 形とも評価器と
    // ネイティブで同じ結果になることを確かめる（common::run が突き合わせ、
    // ネイティブが落ちれば突き合わせ側で失敗になる）。
    let run_full = |src: &str| -> i64 {
        let r = common::run(src, "main", &[] as &[Arg]).unwrap();
        r.value.parse().unwrap()
    };
    let tuple_first = format!(
        "{HEAD}struct Secret {{\n  xs: (Int, String)\n}}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let s = Secret {{ xs: (7, \"hello\") }};\n  let (n, text) = s.xs;\n  return n + text.len();\n}}\n"
    );
    assert_eq!(run_full(&tuple_first), 7 + 5);
    let string_first = format!(
        "{HEAD}struct Secret {{\n  name: String,\n  n: Int\n}}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let s = Secret {{ name: \"abc\", n: 40 }};\n  return s.n + s.name.len();\n}}\n"
    );
    assert_eq!(run_full(&string_first), 40 + 3);
    let list_first = format!(
        "{HEAD}struct Secret {{\n  xs: List<Int>,\n  n: Int\n}}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut xs = List.new();\n  xs.push(10);\n  xs.push(20);\n  let s = Secret {{ xs: xs, n: 1 }};\n  return s.n + s.xs.len();\n}}\n"
    );
    assert_eq!(run_full(&list_first), 1 + 2);
}

#[test]
fn user_defined_secret_trace_is_not_redacted() {
    // 利用者の `Secret` の破棄は、通常の構造体として記録される
    // （`***` で隠されない）。
    let src = format!(
        "{HEAD}struct Secret {{\n  xs: (Int, String)\n}}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let s = Secret {{ xs: (7, \"hello\") }};\n  return 0;\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert!(
        drops.iter().any(|d| d.starts_with("Secret {")),
        "通常の構造体として記録されていない: {drops:?}"
    );
    assert!(
        !drops.iter().any(|d| d.contains("***")),
        "利用者の型が秘密として隠されている: {drops:?}"
    );
}

#[test]
fn real_secret_trace_is_redacted() {
    // 本物の `Secret`（std.crypto）の破棄は `Secret(***)` とだけ記録される。
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let mut b = Bytes.new();\n  b.push(10);\n  let s = Secret.from_bytes(b);\n  return s.len();\n}}\n"
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert!(
        drops.iter().any(|d| d == "Secret(***)"),
        "本物の Secret が隠されていない: {drops:?}"
    );
}

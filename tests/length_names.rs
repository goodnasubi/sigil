//! ADR 0055: String・Bytes・Secret の長さの名前（char_count / byte_len）および第3段の旧名削除・案内。
//!
//! - `String.char_count()` は Unicode スカラー値の数を返す（走査して数える。長さに比例）。
//! - `String.byte_len()` は UTF-8 のバイト数を返す（定数時間）。
//! - `Bytes.byte_len()` はバイト数を返す（定数時間）。
//! - `Secret.byte_len()` は中身のバイト数を返す（定数時間）。
//! - 旧名（`String.len`, `Bytes.len`, `Secret.len`）は第3段で削除され、型検査で新しい名前が案内される。
//! - `common::run` を通じて SIR 評価器と Cranelift ネイティブコード生成の一致を検証する。

mod common;

use sigil::exec::Arg;

const HEAD: &str = "module length_names.t\n  targets server\n  trust none\n\n";

fn run(body: &str) -> i64 {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    );
    let r = common::run(&src, "main", &[] as &[Arg]).unwrap();
    r.value.parse().unwrap()
}

fn check_err(body: &str) -> String {
    let src = format!(
        "{HEAD}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    );
    match sigil::ir_eval::lower_sources(&[&src], "main") {
        Ok(_) => panic!("型検査が通ってしまった: {body}"),
        Err(e) => format!("{e}"),
    }
}

#[test]
fn string_char_count_and_byte_len() {
    // ASCII 文字列: 文字数 5, バイト数 5
    let v_ascii = run(r#"  let s = "hello";
  return s.char_count() * 100 + s.byte_len() * 10;"#);
    assert_eq!(v_ascii, 550);

    // マルチバイト文字列 (日本語): "こんにちは" -> 文字数 5, バイト数 15 (UTF-8 3バイト×5)
    let v_ja = run(r#"  let s = "こんにちは";
  return s.char_count() * 1000 + s.byte_len() * 10;"#);
    assert_eq!(v_ja, 5000 + 150);

    // 4バイト文字 (絵文字): "🦀" -> 文字数 1, バイト数 4
    let v_emoji = run(r#"  let s = "🦀";
  return s.char_count() * 100 + s.byte_len() * 10;"#);
    assert_eq!(v_emoji, 140);

    // 空文字列
    let v_empty = run(r#"  let s = "";
  return s.char_count() + s.byte_len();"#);
    assert_eq!(v_empty, 0);
}

#[test]
fn bytes_byte_len() {
    // 空の Bytes
    let v_empty = run(r#"  let b = Bytes.new();
  return b.byte_len();"#);
    assert_eq!(v_empty, 0);

    // 3 バイト push
    let v_three = run(r#"  let mut b = Bytes.new();
  b.push(65);
  b.push(66);
  b.push(67);
  return b.byte_len();"#);
    assert_eq!(v_three, 3);
}

#[test]
fn secret_byte_len() {
    // 空の Secret
    let v_empty = run(r#"  let s = Secret.from_bytes(Bytes.new());
  return s.byte_len();"#);
    assert_eq!(v_empty, 0);

    // データありの Secret
    let v_data = run(r#"  let mut b = Bytes.new();
  b.push(1);
  b.push(2);
  b.push(3);
  b.push(4);
  let s = Secret.from_bytes(b);
  return s.byte_len();"#);
    assert_eq!(v_data, 4);
}

#[test]
fn list_len_unaffected() {
    let v = run(r#"  let mut xs = List.new();
  xs.push(10);
  xs.push(20);
  return xs.len();"#);
    assert_eq!(v, 2);
}

#[test]
fn string_len_removed_and_guided() {
    let err = check_err(
        r#"  let s = "hello";
  return s.len();"#,
    );
    assert!(err.contains("型エラー"), "{err}");
    assert!(
        err.contains("`String` に `len` は無い。文字数は `char_count`、バイト数は `byte_len`"),
        "{err}"
    );
}

#[test]
fn bytes_len_removed_and_guided() {
    let err = check_err(
        r#"  let b = Bytes.new();
  return b.len();"#,
    );
    assert!(err.contains("型エラー"), "{err}");
    assert!(
        err.contains("`Bytes` に `len` は無い。バイト数は `byte_len`"),
        "{err}"
    );

    let err_path = check_err(
        r#"  let b = Bytes.new();
  return Bytes.len(&b);"#,
    );
    assert!(err_path.contains("型エラー"), "{err_path}");
    assert!(
        err_path.contains("`Bytes` に `len` は無い。バイト数は `byte_len`"),
        "{err_path}"
    );
}

#[test]
fn secret_len_removed_and_guided() {
    let err = check_err(
        r#"  let s = Secret.from_bytes(Bytes.new());
  return s.len();"#,
    );
    assert!(err.contains("型エラー"), "{err}");
    assert!(
        err.contains("`Secret` に `len` は無い。バイト数は `byte_len`"),
        "{err}"
    );

    let err_path = check_err(
        r#"  let s = Secret.from_bytes(Bytes.new());
  return Secret.len(&s);"#,
    );
    assert!(err_path.contains("型エラー"), "{err_path}");
    assert!(
        err_path.contains("`Secret` に `len` は無い。バイト数は `byte_len`"),
        "{err_path}"
    );
}

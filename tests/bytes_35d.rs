//! ADR 0025 補足 F の 3-5d-i: `Int` の回転（`std.core.int`。分岐なし）の突き合わせ。
//!
//! - 利用者のプログラムの `x.rotl(n, w)`・`x.rotr(n, w)`・`x.rotr32(n)` は、評価器では組み込み、ネイティブでは
//!   Sigil 製の `std.core.int` を呼ぶ。`common::run` が両者を突き合わせ、ここで Rust の `rotate_left`/`rotate_right` とも比べる。
//! - `std.core.int` の関数を評価器の上で直接呼ぶ試験（組み込みを通らない）も置く。
//! - 回転の行列: 幅（32・64・それ以外）× 量 `n`（0・1・w-1・w・w+1・負・大きい値）× 値 `x`（境界の値）。
mod common;

use sigil::exec::{Arg, Options};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";
const HEAD: &str = "module m.rot\n  targets server\n  trust none\n\n";

const XS: &[i64] = &[
    0,
    1,
    -1,
    i64::MAX,
    i64::MIN,
    0x8000_0000,
    0x7FFF_FFFF,
    0x1_0000_0001,
    0x1234_5678_9ABC_DEF0,
    -0x1234_5678_9ABC_DEF0,
    0xF000_000F,
    -2,
];
const NS: &[i64] = &[
    0,
    1,
    2,
    30,
    31,
    32,
    33,
    62,
    63,
    64,
    65,
    -1,
    -33,
    100,
    i64::MAX,
    i64::MIN,
];
const WS: &[i64] = &[32, 64, 0, 16, 33, -1, 0x1_0000_0020, 0x8000_0020, i64::MIN];

fn lit(v: i64) -> String {
    if v == i64::MIN {
        "(-9223372036854775807 - 1)".to_string()
    } else if v < 0 {
        format!("({v})")
    } else {
        v.to_string()
    }
}

/// Rust の参照実装（ADR 補足 F-3 の規則: 有効幅は `w == 32` のとき 32、それ以外は 64）。
fn rot(x: i64, n: i64, w: i64, left: bool) -> i64 {
    if w == 32 {
        let k = (n & 31) as u32;
        let u = x as u32;
        (if left {
            u.rotate_left(k)
        } else {
            u.rotate_right(k)
        }) as i64
    } else {
        let k = (n & 63) as u32;
        let u = x as u64;
        (if left {
            u.rotate_left(k)
        } else {
            u.rotate_right(k)
        }) as i64
    }
}

/// 畳み込み（`(acc << 7) ^ (acc >> 57) ^ r`。Sigil の `<<`/`>>`（論理）と同じ）。
fn mix(acc: i64, r: i64) -> i64 {
    (acc as u64).rotate_left(7) as i64 ^ r
}

fn expect_hash(x: i64) -> i64 {
    let mut acc = 0i64;
    for &n in NS {
        for &w in WS {
            acc = mix(acc, rot(x, n, w, true));
            acc = mix(acc, rot(x, n, w, false));
        }
        acc = mix(acc, rot(x, n, 32, false));
    }
    acc
}

/// `a` を値として、行列のすべての組を畳み込む本体。`rl`・`rr`・`r32` は呼び出しの書き方。
fn matrix_body(
    rl: &dyn Fn(&str, &str) -> String,
    rr: &dyn Fn(&str, &str) -> String,
    r32: &dyn Fn(&str) -> String,
) -> String {
    let mut s = String::from("  let mut acc = 0;\n");
    let m = "acc = (acc << 7) ^ (acc >> 57) ^ ";
    for &n in NS {
        let n = lit(n);
        for &w in WS {
            let w = lit(w);
            s += &format!("  {m}{};\n", rl(&n, &w));
            s += &format!("  {m}{};\n", rr(&n, &w));
        }
        s += &format!("  {m}{};\n", r32(&n));
    }
    s += "  return acc;\n";
    s
}

/// 利用者のプログラム（評価器は組み込み、ネイティブは Sigil 製）。
fn user_src() -> String {
    let body = matrix_body(
        &|n, w| format!("a.rotl({n}, {w})"),
        &|n, w| format!("a.rotr({n}, {w})"),
        &|n| format!("a.rotr32({n})"),
    );
    format!("{HEAD}pub fn f(a: Int) -> Int\n{TAIL}{{\n{body}}}\n")
}

#[test]
fn rotation_matrix_eval_native_and_rust_agree() {
    let src = user_src();
    for &x in XS {
        let r = common::run(&src, "f", &[Arg::Int(x)]).unwrap();
        assert_eq!(r.value, expect_hash(x).to_string(), "x={x}");
    }
}

fn driver(body: &str) -> String {
    format!(
        "module std.core.t\n  targets server\n  trust none\n\nuse std.core.int.rotl;\nuse std.core.int.rotr;\nuse std.core.int.rotr32;\n\npub fn run(a: Int) -> Int\n{TAIL}{{\n{body}}}\n"
    )
}

/// `std.core.int` の関数を評価器の上で直接呼ぶ（組み込みを通らない）。
#[test]
fn std_core_int_called_directly_matches_rust() {
    let body = matrix_body(
        &|n, w| format!("rotl(a, {n}, {w})"),
        &|n, w| format!("rotr(a, {n}, {w})"),
        &|n| format!("rotr32(a, {n})"),
    );
    let entry = format!("{HEAD}pub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n");
    let o = Options {
        embedded_extra: vec![("std.core.t".to_string(), driver(&body))],
        embedded_roots: vec!["std.core.t".into()],
        ..Options::captured()
    };
    for &x in XS {
        let r = sigil::ir_eval::run_sources_with(&[&entry], "std.core.t.run", &[Arg::Int(x)], &o)
            .unwrap_or_else(|f| panic!("x={x}: {:?}", f.error));
        assert_eq!(r.value, expect_hash(x).to_string(), "x={x}");
    }
}

fn one(body: &str, x: i64, n: i64) -> String {
    let src = format!("{HEAD}pub fn f(x: Int, n: Int) -> Int\n{TAIL}{{\n  return {body};\n}}\n");
    common::run(&src, "f", &[Arg::Int(x), Arg::Int(n)])
        .unwrap()
        .value
}

/// 個別の既知の値（評価器とネイティブの一致は `common::run` が見る）。
#[test]
fn known_values() {
    // ADR 補足 F-0 の 5 つの例。
    assert_eq!(one("x.rotl(n, 64)", 1, 40), (1i64 << 40).to_string());
    assert_eq!(one("x.rotr(n, 64)", 1, 1), i64::MIN.to_string());
    assert_eq!(one("x.rotr32(n)", 1, 1), "2147483648");
    assert_eq!(one("x.rotl(n, 32)", 1, 31), "2147483648");
    assert_eq!(one("x.rotl(n, 32)", -1, 32), "4294967295");
    // 32 ビット幅で第 31 ビットが立たない結果。
    assert_eq!(one("x.rotl(n, 32)", 1, 1), "2");
    // 64 ビット幅: 量が 0・64・負。
    assert_eq!(one("x.rotl(n, 64)", 5, 0), "5");
    assert_eq!(one("x.rotl(n, 64)", 5, 64), "5");
    assert_eq!(one("x.rotr(n, 64)", 5, 64), "5");
    assert_eq!(one("x.rotl(n, 64)", 2, -1), "1");
    assert_eq!(one("x.rotr(n, 64)", 1, -1), "2");
    // rotr32 の量が負（-1 は 31 と同じ）。
    assert_eq!(one("x.rotr32(n)", 1, -1), "2");
}

/// 変異の検査用: `std.core.int` のソースが分岐を持たないこと（`if`・`while`・比較演算子を使わない）。
#[test]
fn source_has_no_branches() {
    let (_, src) = sigil::stdlib::embedded::EMBEDDED
        .iter()
        .find(|(n, _)| *n == sigil::stdlib::embedded::INT_MODULE)
        .unwrap();
    let code: String = src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for bad in [
        "if ",
        "while ",
        "match ",
        " == ",
        " != ",
        " < ",
        " > ",
        " <= ",
        " >= ",
        "&&",
        "||",
        "return if",
    ] {
        assert!(!code.contains(bad), "分岐になりうる `{bad}` がある");
    }
}

//! ADR 0025 補足 C-3 の 3-5a: `Float.to_bits` / `Float.from_bits` と、Sigil 製の fmod（`std.core.float`）。
//!
//! - ビット列の往復と境界（NaN・±0・±∞・非正規数）を、評価器とネイティブで突き合わせる。
//! - fmod は Rust の `%` と全ビットで比べる（NaN は NaN であることだけ比べる）。言語には `Float` の `%` がない
//!   （ADR 0011）ので、モジュールのソースを利用者のプログラムの中へ取り込んで走らせる。
//! - 変異の試験: fmod の本体の分岐を 1 つずつ壊すと、この試験が落ちることを確かめる。
mod common;

use sigil::exec::Arg;
use sigil::stdlib::embedded::{EMBEDDED, FLOAT_MODULE};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

fn float_src() -> &'static str {
    EMBEDDED
        .iter()
        .find(|(n, _)| *n == FLOAT_MODULE)
        .map(|(_, s)| *s)
        .expect("Float のモジュールが登録されている")
}

/// モジュール名だけ変えて、利用者のプログラムにする（ソースの本文は本番のまま）。
fn with_driver(src: &str) -> String {
    let body = src.replacen(FLOAT_MODULE, "m.f", 1);
    format!(
        "{body}\n\nfn drive(a: Int, b: Int) -> Int\n{TAIL}{{\n  return Float.to_bits(fmod(Float.from_bits(a), Float.from_bits(b)));\n}}\n\nfn bits(a: Int) -> Int\n{TAIL}{{\n  return Float.to_bits(Float.from_bits(a));\n}}\n\nfn asf(a: Int) -> Float\n{TAIL}{{\n  return Float.from_bits(a);\n}}\n\nfn tob(x: Float) -> Int\n{TAIL}{{\n  return Float.to_bits(x);\n}}\n"
    )
}

fn int_of(v: &str) -> i64 {
    v.parse().unwrap_or_else(|_| panic!("整数でない: {v}"))
}

const NAN: u64 = 0x7FF8_0000_0000_0000;
const SPECIAL: &[u64] = &[
    0x0000_0000_0000_0000, // +0
    0x8000_0000_0000_0000, // -0
    0x0000_0000_0000_0001, // 最小の非正規数
    0x8000_0000_0000_0001,
    0x000F_FFFF_FFFF_FFFF, // 最大の非正規数
    0x0010_0000_0000_0000, // 最小の正規数
    0x8010_0000_0000_0000,
    0x3FF0_0000_0000_0000, // 1.0
    0xBFF0_0000_0000_0000, // -1.0
    0x4000_0000_0000_0000, // 2.0
    0x4008_0000_0000_0000, // 3.0
    0x401E_0000_0000_0000, // 7.5
    0xC01E_0000_0000_0000, // -7.5
    0x3FD0_0000_0000_0000, // 0.25
    0x3FB9_9999_9999_999A, // 0.1
    0x7FEF_FFFF_FFFF_FFFF, // 最大の有限値
    0xFFEF_FFFF_FFFF_FFFF,
    0x7FF0_0000_0000_0000, // +∞
    0xFFF0_0000_0000_0000, // -∞
    NAN,
    0xFFF8_0000_0000_0001,
    0x7FF0_0000_0000_0001, // シグナル NaN
    0x4330_0000_0000_0000, // 2^52
    0x4340_0000_0000_0000, // 2^53
    0x7FE0_0000_0000_0000, // 2^1023
    0x0000_0001_0000_0000, // 非正規数（上位の仮数が 0）
];

#[test]
fn bits_roundtrip_and_boundaries() {
    let src = with_driver(float_src());
    for &b in SPECIAL {
        let r = eval_only(&src, "bits", &[Arg::Int(b as i64)]);
        assert_eq!(int_of(&r.value) as u64, b, "往復 {b:#x}");
        // from_bits の値が Rust の f64::from_bits と同じ（to_bits で戻して比べる）。
        let f = f64::from_bits(b);
        if f.is_nan() || f.is_infinite() {
            continue; // 引数として書けない値（ビット列の側では確かめ済み）
        }
        let t = eval_only(&src, "tob", &[Arg::Float(f)]);
        assert_eq!(int_of(&t.value) as u64, b, "to_bits {b:#x}");
    }
}

/// 評価器とネイティブの突き合わせ（ビット列の往復）。
#[test]
fn bits_native_agrees() {
    let src = with_driver(float_src());
    for b in [
        0u64,
        0x8000_0000_0000_0000,
        1,
        0x7FF0_0000_0000_0000,
        NAN,
        0xFFEF_FFFF_FFFF_FFFF,
        0x3FF0_0000_0000_0000,
    ] {
        let r = common::run(&src, "bits", &[Arg::Int(b as i64)]).unwrap();
        assert_eq!(int_of(&r.value) as u64, b);
    }
}

#[test]
fn to_bits_known_values() {
    let src = with_driver(float_src());
    for (x, want) in [
        (1.0f64, 0x3FF0_0000_0000_0000u64),
        (-2.5, 0xC004_0000_0000_0000),
        (0.0, 0),
        (-0.0, 0x8000_0000_0000_0000),
        (5e-324, 1),
    ] {
        let r = common::run(&src, "tob", &[Arg::Float(x)]).unwrap();
        assert_eq!(int_of(&r.value) as u64, want, "{x:?}");
    }
    // from_bits: 1.0 と -0.0 の表示。
    assert_eq!(
        common::run(&src, "asf", &[Arg::Int(0x3FF0_0000_0000_0000)])
            .unwrap()
            .value,
        "1.0"
    );
}

/// 指数が 0 に近い（非正規数・最小の正規数の付近）の組。結果が非正規数にもなる。
fn near_min_pairs() -> Vec<(u64, u64)> {
    let mut s: u64 = 0xD1B5_4A32_D192_ED03;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    (0..150)
        .map(|_| {
            let (x, y) = (next(), next());
            let ea = (x >> 60) & 0x7; // 0..7
            let eb = (y >> 60) & 0x3; // 0..3
            let a = (x & 0x800F_FFFF_FFFF_FFFF) | (ea << 52);
            let b = (y & 0x000F_FFFF_FFFF_FFFF) | (eb << 52);
            (a, b)
        })
        .collect()
}

fn want_fmod(a: u64, b: u64) -> u64 {
    (f64::from_bits(a) % f64::from_bits(b)).to_bits()
}

/// 評価器だけで走らせる（広く掃くとき用。ネイティブの作成は 1 件ごとに時間がかかる）。
fn eval_only(src: &str, f: &str, args: &[Arg]) -> sigil::exec::Outcome {
    sigil::ir_eval::run_sources_with(&[src], f, args, &sigil::exec::Options::captured()).unwrap()
}

fn check_pair(src: &str, a: u64, b: u64) {
    check_pair_with(src, a, b, false);
}

/// `both` が真なら評価器とネイティブの突き合わせ（`common::run`）も通す。
fn check_pair_with(src: &str, a: u64, b: u64, both: bool) {
    let args = [Arg::Int(a as i64), Arg::Int(b as i64)];
    let r = if both {
        common::run(src, "drive", &args).unwrap()
    } else {
        eval_only(src, "drive", &args)
    };
    let got = int_of(&r.value) as u64;
    let want = want_fmod(a, b);
    if f64::from_bits(want).is_nan() {
        assert!(
            f64::from_bits(got).is_nan(),
            "{a:#x} % {b:#x}: NaN のはず（{got:#x}）"
        );
    } else {
        assert_eq!(got, want, "{a:#x} % {b:#x}");
    }
}

#[test]
fn fmod_matches_rust_on_boundaries() {
    let src = with_driver(float_src());
    for &a in SPECIAL {
        for &b in SPECIAL {
            check_pair(&src, a, b);
        }
    }
}

#[test]
fn fmod_matches_rust_near_subnormal_range() {
    let src = with_driver(float_src());
    for (a, b) in near_min_pairs() {
        check_pair(&src, a, b);
    }
}

/// 評価器とネイティブの突き合わせ（境界の値の一部の全組み合わせ）。
#[test]
fn fmod_native_agrees_on_boundaries() {
    let src = with_driver(float_src());
    let pick = [
        0x0000_0000_0000_0000u64,
        0x8000_0000_0000_0000,
        0x0000_0000_0000_0001,
        0x0010_0000_0000_0000,
        0x401E_0000_0000_0000,
        0xC000_0000_0000_0000,
        0x7FEF_FFFF_FFFF_FFFF,
        0x7FF0_0000_0000_0000,
        0xFFF0_0000_0000_0000,
        NAN,
    ];
    for &a in &pick {
        for &b in &pick {
            check_pair_with(&src, a, b, true);
        }
    }
    for (a, b) in [
        (0x4330_0000_0000_0001u64, 0x3FB9_9999_9999_999A),
        (0x7FE0_0000_0000_0000, 0x0000_0001_0000_0000),
        (0xC1E0_0000_0000_0000, 0x4008_0000_0000_0000),
    ] {
        check_pair_with(&src, a, b, true);
    }
}

#[test]
fn fmod_matches_rust_on_pseudo_random() {
    let src = with_driver(float_src());
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    for i in 0..60 {
        let a = next();
        let mut b = next();
        // 指数を近づけて、ループの回数が小さい場合・大きい場合の両方を踏む。
        if i % 3 == 0 {
            b = (b & 0x800F_FFFF_FFFF_FFFF) | (a & 0x7FF0_0000_0000_0000);
        } else if i % 3 == 1 {
            let e = (a >> 52) & 0x7FF;
            let e2 = e.saturating_sub(i as u64 % 40);
            b = (b & 0x800F_FFFF_FFFF_FFFF) | (e2 << 52);
        }
        check_pair(&src, a, b);
    }
}

#[test]
fn fmod_sign_and_zero_divisor() {
    let src = with_driver(float_src());
    let f = |x: f64, y: f64| {
        let r = eval_only(
            &src,
            "drive",
            &[Arg::Int(x.to_bits() as i64), Arg::Int(y.to_bits() as i64)],
        );
        f64::from_bits(int_of(&r.value) as u64)
    };
    assert_eq!(f(7.5, 2.0), 1.5);
    assert_eq!(f(-7.5, 2.0), -1.5);
    assert_eq!(f(7.5, -2.0), 1.5);
    assert_eq!(f(-7.5, -2.0), -1.5);
    assert!(f(-4.0, 2.0).is_sign_negative() && f(-4.0, 2.0) == 0.0);
    assert!(f(4.0, -2.0).is_sign_positive());
    assert!(f(1.0, 0.0).is_nan());
    assert!(f(1.0, -0.0).is_nan());
    assert!(f(0.0, 0.0).is_nan());
    assert!(f(f64::INFINITY, 2.0).is_nan());
    assert!(f(2.0, f64::NAN).is_nan());
    assert_eq!(f(2.0, f64::INFINITY), 2.0);
    assert_eq!(f(-2.0, f64::NEG_INFINITY), -2.0);
    assert_eq!(f(1e300, 3.0), 1e300 % 3.0);
}

/// 本体の分岐を壊すと落ちる（試験が分岐を踏んでいることの確認）。
#[test]
fn fmod_mutations_are_caught() {
    let orig = float_src();
    let muts: &[(&str, &str)] = &[
        (
            "ay == 0 || ay > 0x7FF0000000000000",
            "ay > 0x7FF0000000000000",
        ),
        ("ax >= 0x7FF0000000000000", "ax > 0x7FF0000000000000"),
        (
            "if ax == ay {\n      return Float.from_bits(sign);",
            "if ax == ay + 1 {\n      return Float.from_bits(sign);",
        ),
        ("sign = xb & Int.min_value()", "sign = 0"),
        ("while e > ey {", "while e > ey + 1 {"),
        ("while e > ey", "while e >= ey"),
        (
            "if d >= 0 {\n      if d == 0 {",
            "if d > 0 {\n      if d == 0 {",
        ),
        (
            "return Float.from_bits(sign | (mx >> (1 - e)));",
            "return Float.from_bits(sign | (mx >> (2 - e)));",
        ),
        ("if e > 0 {", "if e > 2 {"),
    ];
    for (from, to) in muts {
        assert!(orig.contains(from), "変異の元が見つからない: {from}");
        let src = with_driver(&orig.replacen(from, to, 1));
        let caught = std::panic::catch_unwind(|| {
            for &a in SPECIAL {
                for &b in SPECIAL {
                    check_pair(&src, a, b);
                }
            }
            for (a, b) in near_min_pairs() {
                check_pair(&src, a, b);
            }
        })
        .is_err();
        assert!(caught, "変異が見逃された: {from} -> {to}");
    }
}

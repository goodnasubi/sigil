//! ADR 0025 補足 H（3-5f-i）: 表示の土台とスカラー。
//!
//! 入口の戻り値の表示が、SIR の評価器とネイティブで一致すること（`common::run` が両方を走らせて比べる）。
//! `Float` は生成した `sigil_show_float`（正確な多倍長演算による区間方式）の最短表現の行列、
//! `Int` は端の値、`Bool` と `Unit` も確かめる。合成型・引用・`console.log` の付け替えは 3-5f-ii。

mod common;
use common::run;
use sigil::exec::Arg;

const HEAD: &str = "module m.main\n  targets server\n  trust none\n\n";

fn prog(sig: &str, body: &str) -> String {
    format!("{HEAD}fn main{sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n")
}

/// そのまま返す入口（引数が表示の対象）。
fn check(sig: &str, arg: Arg) {
    let src = prog(sig, "  return x;");
    run(&src, "main", &[arg]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

#[test]
fn float_matrix() {
    let vals: &[f64] = &[
        0.0,
        -0.0,
        1.5,
        3.0,
        -0.25,
        0.1,
        2.5,
        100000.0,
        1234567.125,
        0.3,
        1.0 / 3.0,
        2.0 / 3.0,
        0.7,
        123456789.12345679,
        4503599627370496.5,
        9007199254740992.0,
        9007199254740994.0,
        1e15,
        1e16,
        1e21,
        1e22,
        1e23,
        1e-4,
        1e-5,
        1e-7,
        0.9999999999999999,
        1.9999999999999998,
        std::f64::consts::PI,
        std::f64::consts::E,
        f64::MAX,
        f64::MIN,
        f64::MIN_POSITIVE,
        f64::from_bits(1), // 最小の非正規化数（5e-324）
        f64::from_bits(2),
        f64::from_bits(0x000F_FFFF_FFFF_FFFF), // 最大の非正規化数
        f64::from_bits(0x0010_0000_0000_0000), // 最小の正規化数
        f64::from_bits(0x3FF0_0000_0000_0001), // 1.0 の次
        f64::from_bits(0x3FEF_FFFF_FFFF_FFFF), // 1.0 の前
        f64::from_bits(0x3FD5_5555_5555_5555),
        f64::from_bits(0x44B8_7C53_2B0C_EB00),
        f64::from_bits(0x7FEF_FFFF_FFFF_FFFF), // MAX の前
    ];
    for &v in vals {
        check("(x: Float) -> Float", Arg::Float(v));
    }
}

#[test]
fn float_specials() {
    for (body, want) in [
        ("  return 1.0 / 0.0;", "inf"),
        ("  return -1.0 / 0.0;", "-inf"),
        ("  return 0.0 / 0.0;", "NaN"),
    ] {
        let src = prog("() -> Float", body);
        let o = run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
        assert_eq!(o.value, want, "{src}");
    }
}

#[test]
fn int_edges() {
    for v in [0i64, -1, 42, -42, i64::MAX, i64::MIN, i64::MIN + 1] {
        check("(x: Int) -> Int", Arg::Int(v));
    }
}

#[test]
fn bool_and_unit() {
    check("(x: Bool) -> Bool", Arg::Bool(true));
    check("(x: Bool) -> Bool", Arg::Bool(false));
    let src = prog("()", "");
    let o = run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "()");
}

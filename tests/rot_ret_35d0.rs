//! ADR 0025 補足 F-0 / F-9 の 3-5d-0: `int_rotl` / `int_rotr` / `int_rotr32` のネイティブの戻りの読み方。
//!
//! `rt_spec` の戻りが `'i'`（32 ビットへ切り詰めて符号拡張）だと、64 ビット幅の回転と、結果の第 31 ビットが立つ
//! 32 ビット幅の回転が評価器と食い違った。`common::run` は評価器とネイティブの結果を突き合わせるので、
//! ここでは値を直接も固定する。
mod common;

use sigil::exec::Arg;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

fn src(body: &str) -> String {
    format!("module m.rot\n  targets server\n  trust none\n\nfn f(x: Int, n: Int) -> Int\n{TAIL}{{\n  return {body};\n}}\n")
}

fn check(body: &str, x: i64, n: i64, want: i64) {
    let r = common::run(&src(body), "f", &[Arg::Int(x), Arg::Int(n)]).unwrap();
    assert_eq!(r.value, want.to_string(), "{body} x={x} n={n}");
}

#[test]
fn rotl_rotr_rotr32_native_agrees() {
    // ADR 補足 F-0 の 5 つの例。
    check("x.rotl(n, 64)", 1, 40, 1 << 40);
    check("x.rotr(n, 64)", 1, 1, i64::MIN);
    check("x.rotr32(n)", 1, 1, 0x8000_0000);
    check("x.rotl(n, 32)", 1, 31, 0x8000_0000);
    check("x.rotl(n, 32)", -1, 32, 0xFFFF_FFFF);
    // 64 ビット幅: 上位 32 ビットに値がある・負の x・n が 0 / 64 / 負。
    check(
        "x.rotl(n, 64)",
        0x1234_5678_9ABC_DEF0,
        12,
        0x1234_5678_9ABC_DEF0u64.rotate_left(12) as i64,
    );
    check(
        "x.rotr(n, 64)",
        0x1234_5678_9ABC_DEF0,
        12,
        0x1234_5678_9ABC_DEF0u64.rotate_right(12) as i64,
    );
    check("x.rotl(n, 64)", -2, 0, -2);
    check("x.rotl(n, 64)", i64::MIN, 64, i64::MIN);
    check(
        "x.rotl(n, 64)",
        3,
        -1,
        3u64.rotate_left((-1i64) as u32) as i64,
    );
    // 32 ビット幅: 結果の第 31 ビットが立つ・立たない。
    check("x.rotr(n, 32)", 0x1, 1, 0x8000_0000);
    check("x.rotr(n, 32)", 0x8000_0001, 0, 0x8000_0001);
    check("x.rotr32(n)", 0xF000_000F, 4, 0xFF00_0000);
    check("x.rotr32(n)", 0xF000_000F, 36, 0xFF00_0000);
}

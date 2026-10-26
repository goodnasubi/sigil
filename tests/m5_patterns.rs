//! ADR 0018 の決定 4（誤りの埋め込みで見つかった穴）: 複数の欄を持つ組・変種の束縛が、欄ごとに正しい値を取る。
//! 欄の取り違えは SIR の検証器では見つからず、実行の結果の比較（評価器とネイティブ）で見つける。
mod common;
use common::run;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\nenum Pair {\n  Two(Int, Int),\n  Three(Int, Int, Int),\n  Nothing,\n}\n\n";

fn f(sig: &str, body: &str) -> String {
    format!(
        "{HEAD}fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    )
}

fn val(src: &str) -> String {
    run(src, "t", &[])
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

#[test]
fn tuple_pattern_binds_each_field_in_order() {
    let src = f(
        "t() -> Int",
        "let (a, b, c) = (1, 20, 300);\nreturn a * 10000 + b * 100 + c;",
    );
    assert_eq!(val(&src), "12300");
}

#[test]
fn nested_tuple_pattern_binds_each_field() {
    let src = f(
        "t() -> Int",
        "let (a, (b, c)) = (1, (2, 3));\nreturn a * 100 + b * 10 + c;",
    );
    assert_eq!(val(&src), "123");
}

#[test]
fn variant_pattern_binds_each_field_in_order() {
    let src = f(
        "t() -> Int",
        "let p = Pair.Three(1, 2, 3);\nreturn match p {\n  Pair.Two(a, b) => a * 10 + b,\n  Pair.Three(a, b, c) => a * 100 + b * 10 + c,\n  Pair.Nothing => 0,\n};",
    );
    assert_eq!(val(&src), "123");
    let src = f(
        "t() -> Int",
        "let p = Pair.Two(4, 5);\nreturn match p {\n  Pair.Two(a, b) => a * 10 + b,\n  Pair.Three(a, b, c) => a * 100 + b * 10 + c,\n  Pair.Nothing => 0,\n};",
    );
    assert_eq!(val(&src), "45");
}

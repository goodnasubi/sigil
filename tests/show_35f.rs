//! ADR 0025 補足 H: 表示の生成。
//!
//! 3-5f-i: 表示の土台とスカラー。入口の戻り値の表示が、SIR の評価器とネイティブで一致すること
//! （`common::run` が両方を走らせて比べる）。`Float` は生成した `sigil_show_float`
//! （正確な多倍長演算による区間方式）の最短表現の行列、`Int` は端の値、`Bool` と `Unit` も確かめる。
//!
//! 3-5f-ii: 型ごとの `sigil_show_<k>` の生成と呼び出しの付け替え。全型の表示を評価器と突き合わせる。
//! 経路は型ごとに使えるものを使う: `console.log` は `Displayable` な型（スカラー・文字列・パスと
//! その参照）だけ、`console.log`（引用なし）と入口の戻り値（引用あり）で確かめ、合成型は入口と
//! 破棄の記録（移動済みフィールド）で確かめる。
//! `Addr` は評価器に合わせた `Addr(<10 進>)` の回帰試験（補足 H-10）。
//! 秘密は表示の経路（`console.log`・入口）には出せない（`Displayable` でない）ので、
//! 破棄の記録（`--trace-drops`）で `Secret(***)` になることをネイティブだけで確かめる
//! （評価器の破棄の記録は秘匿しないため、突き合わせの対象にしない）。

mod common;
use common::run;
use sigil::exec::{Arg, Options};

const HEAD: &str = "module m.main\n  targets server\n  trust none\n\n";

fn prog(sig: &str, body: &str) -> String {
    format!("{HEAD}fn main{sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n")
}

/// 型定義つき・効果つきの入口。
fn progx(defs: &str, sig: &str, effects: &str, body: &str) -> String {
    format!("{HEAD}{defs}fn main{sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n")
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
fn string_nul_quote() {
    // NUL を含む文字列の引用。Rust の `Debug` は NUL を `\0` で出すので、
    // `sigil_show_quote` も同じになる（名前つきの逃がしに `\0` を足した）。
    // 文字列は引数ではなく中で作る（コマンドラインの引数には NUL を渡せないため）。
    // 3-5f-ii で入口の `String` の表示が生成した `sigil_show_<k>` へ替わったので、
    // この突き合わせが生成した引用そのものを確かめる。
    let src = prog("() -> String", "  return \"a\\u{0}b\";");
    let o = run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "\"a\\0b\"", "{src}");
}

#[test]
fn string_quote_matrix() {
    // 補足 H-4 の行列: 制御文字（U+0001・U+001F・U+007F）、U+0085、U+00AD、結合文字、
    // U+200B、U+FEFF、私用領域、面 1 以降の文字。入口の引用ありで突き合わせる。
    let cases: &[&str] = &[
        "plain",
        "a\"b\\c\nd\te\rf",
        "\u{1}\u{1f}\u{7f}",
        "\u{85}\u{ad}",
        "e\u{301}",
        "\u{200b}\u{feff}",
        "\u{e000}\u{f8ff}",
        "\u{1d165}\u{1f600}",
        "日本語のテスト",
    ];
    for s in cases {
        check("(x: String) -> String", Arg::Str(s.to_string()));
    }
    // 引用なし（`console.log`）は中身そのまま。
    let src = progx(
        "",
        "() -> Int",
        "console",
        "  console.log(&\"a\\\"b\\nc\");\n  return 0;",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

#[test]
fn path_show() {
    check("(x: Path) -> Path", Arg::Str("/tmp/a b".to_string()));
}

#[test]
fn bytes_show() {
    // 空・1 要素・複数（0 と 255 を含む）。`Bytes` は `Displayable` でないので入口で確かめる。
    for pushes in [
        "",
        "  b.push(7);\n",
        "  b.push(0);\n  b.push(255);\n  b.push(10);\n",
    ] {
        let body = format!("  let mut b = Bytes.new();\n{pushes}  return b;");
        let src = prog("() -> Bytes", &body);
        run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    }
}

#[test]
fn list_show() {
    check("(x: List<Int>) -> List<Int>", Arg::List(vec![]));
    check(
        "(x: List<Int>) -> List<Int>",
        Arg::List(vec![Arg::Int(1), Arg::Int(-2), Arg::Int(300)]),
    );
    // 入れ子と、文字列の要素（要素は引用あり）。
    let src = prog(
        "() -> List<List<Int>>",
        "  let mut a = List.new();\n  a.push(1);\n  a.push(2);\n  let mut outer = List.new();\n  outer.push(a);\n  outer.push(List.new());\n  return outer;",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog(
        "() -> List<String>",
        "  let mut xs = List.new();\n  xs.push(\"a b\");\n  xs.push(\"c\\nd\");\n  return xs;",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

#[test]
fn tuple_show() {
    let src = prog("() -> (Int, String, Bool)", "  return (1, \"a\", true);");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog(
        "() -> (Int, (String, Float))",
        "  return (2, (\"x\", 0.5));",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

const STRUCT_DEFS: &str =
    "struct P {\n  k: Int,\n  name: String,\n}\n\nstruct Outer {\n  p: P,\n  n: Int,\n}\n\n";

#[test]
fn struct_show() {
    let src = progx(
        STRUCT_DEFS,
        "() -> P",
        "none",
        "  return P { k: 1, name: \"q\" };",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = progx(
        STRUCT_DEFS,
        "() -> Outer",
        "none",
        "  return Outer { p: P { k: 2, name: \"r s\" }, n: 3 };",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

const ENUM_DEFS: &str = "enum Shape {\n  Point,\n  Circle(Int),\n  Rect(Int, String),\n}\n\n";

#[test]
fn enum_show() {
    // 腕のフィールドの有無と、型名の接頭辞。
    for (body, sig) in [
        ("  return Shape.Point;", "() -> Shape"),
        ("  return Shape.Circle(3);", "() -> Shape"),
        ("  return Shape.Rect(1, \"a\");", "() -> Shape"),
    ] {
        let src = progx(ENUM_DEFS, sig, "none", body);
        run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    }
    // `Option`・`Result` は型名と `.` を省く。
    let src = prog("() -> Option<Int>", "  return Some(3);");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog("() -> Option<Int>", "  return None;");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog("() -> Result<Int, String>", "  return Ok(1);");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog("() -> Result<Int, String>", "  return Err(\"e\");");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

#[test]
fn shared_cell_show() {
    let src = prog("() -> Shared<Int>", "  return Shared.new(5);");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog("() -> Cell<String>", "  return Cell.new(\"x\");");
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    let src = prog(
        "() -> Shared<Cell<Int>>",
        "  return Shared.new(Cell.new(1));",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

#[test]
fn ref_show() {
    // 参照は指す先の値を表示する（受けた引用のまま）。
    // 参照を書けるのは引数・戻り値の一番外と `Option<…>` の中だけ（ADR 0012 の決定 1）。
    // 参照を返せるのは借用の引数がちょうど 1 つの関数だけ（ADR 0003 の規則 3）で、
    // `Option<…>` の中の参照は借用した一覧の要素として得るのが定石。
    // 入口が参照そのものを返す形は Issue #304 の回帰試験（入口の戻り値の型から
    // 参照を外すと、表示も破棄もずれた）。
    let src = prog(
        "(xs: &List<String>) -> Option<&String>",
        "  return xs.get(0);",
    );
    let xs = Arg::List(vec![Arg::Str("hi".to_string())]);
    let o = run(&src, "main", &[xs]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "Some(\"hi\")", "{src}");
    let o = run(&src, "main", &[Arg::List(vec![])]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "None", "{src}");
    // 入口が参照そのものを返す形（Issue #304）。借りた値を表示し、破棄はしない。
    let src = prog("(x: &String) -> &String", "  return x;");
    let o =
        run(&src, "main", &[Arg::Str("hi".to_string())]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "\"hi\"", "{src}");
    let src = prog("(n: &Int) -> &Int", "  return n;");
    let o = run(&src, "main", &[Arg::Int(42)]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "42", "{src}");
    let src = prog("(xs: &List<String>) -> &List<String>", "  return xs;");
    let xs = Arg::List(vec![Arg::Str("a".to_string()), Arg::Str("b".to_string())]);
    let o = run(&src, "main", &[xs]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "[\"a\", \"b\"]", "{src}");
    // `&mut` の形も同じ。
    let src = prog("(x: &mut String) -> &mut String", "  return x;");
    let o =
        run(&src, "main", &[Arg::Str("hi".to_string())]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "\"hi\"", "{src}");
    // `console.log` へ借用で渡る経路（引用なし）も、文字列で確かめる。
    let src = progx(
        "",
        "() -> Int",
        "console",
        "  let s = \"hi\";\n  console.log(&s);\n  return 0;",
    );
    run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
}

#[test]
fn fn_value_show() {
    let defs = "fn inc(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return x + 1;\n}\n\n";
    let src = progx(
        defs,
        "() -> fn(Int) -> Int effects none",
        "none",
        "  return inc;",
    );
    let o = run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "<関数>", "{src}");
}

#[test]
fn parse_error_show() {
    // `Int` の解析の失敗は `ParseError` の値になる（`Result` の接頭辞なしと合わせて）。
    let src = prog(
        "() -> Result<Int, ParseError>",
        "  return \"x\".parse_int();",
    );
    let o = run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "Err(ParseError)", "{src}");
}

#[test]
fn addr_show() {
    // 補足 H-10 の回帰試験: 評価器は `Addr(<10 進>)`、移行前のネイティブは `<関数>` だった。
    let src = "module m.main\n  targets server\n  trust base\n\nfn mk(n: Int) -> Addr\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"試験用の番地を作る\"\n{\n  return raw.int_to_addr(n);\n}\n\nfn main() -> Addr\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return mk(1234);\n}\n";
    let opts = Options {
        trust_base: vec!["m.main".to_string()],
        ..Options::captured()
    };
    let o = common::run_with(src, "main", &[], &opts).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "Addr(1234)", "{src}");
}

#[test]
fn moved_fields_show_as_unit() {
    // フィールドを取り出した（移動済みの）構造体は、破棄の記録でそのフィールドが `()` になる。
    let defs = "struct Inner {\n  s: String,\n  xs: List<Int>,\n}\n\nstruct Outer2 {\n  inner: Inner,\n  n: Int,\n}\n\n";
    let src = progx(
        defs,
        "() -> Int",
        "none",
        "  let mut xs = List.new();\n  xs.push(1);\n  xs.push(2);\n  let out = Outer2 { inner: Inner { s: \"hello\", xs: xs }, n: 7 };\n  let taken_s = out.inner.s;\n  let taken_xs = out.inner.xs;\n  return out.n + taken_s.len() + taken_xs.len();",
    );
    let opts = Options {
        trace_drops: true,
        ..Options::captured()
    };
    let o = common::run_with(&src, "main", &[], &opts).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert!(
        o.drops
            .iter()
            .any(|d| d == "Outer2 { inner: Inner { s: (), xs: () }, n: 7 }"),
        "移動済みフィールドの表示が違う: {:?}",
        o.drops
    );
}

#[test]
fn secret_trace_hides() {
    // 秘密の表示は破棄の記録で確かめる（`console.log`・入口には出せない）。
    let src = prog(
        "() -> Int",
        "  let mut b = Bytes.new();\n  b.push(10);\n  b.push(20);\n  let s = Secret.from_bytes(b);\n  return s.len();",
    );
    let (got, drops) = common::native::native_drops(&[&src], "main", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert!(
        drops.iter().any(|d| d == "Secret(***)"),
        "本物の Secret が隠されていない: {drops:?}"
    );
}

#[test]
fn bool_and_unit() {
    check("(x: Bool) -> Bool", Arg::Bool(true));
    check("(x: Bool) -> Bool", Arg::Bool(false));
    let src = prog("()", "");
    let o = run(&src, "main", &[]).unwrap_or_else(|e| panic!("{e:?}\n{src}"));
    assert_eq!(o.value, "()");
}

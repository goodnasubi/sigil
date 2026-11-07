//! ADR 0025 補足 I: 入口の引数（3-5g-ii）。
//!
//! リテラルの読み取り（I-2）と `Float` の解析（I-3）、合わせ込み `sigil_coerce_<k>`（I-4）を
//! 生成側へ移したので、入口の引数を SIR の評価器とネイティブで突き合わせる。
//! 合う引数は `common::run`（評価器＋ネイティブ）、誤り（型の不一致・個数の不一致・
//! `Int` の範囲外）はネイティブの実行結果（終了コード 1 と標準エラー）を直接確かめる。
//! `Float` の行列は Rust の `parse` を oracle にする（I-3・I-9）。

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

/// 署名と本体ごとに一度だけ組み立てた実行ファイル（組み立てが重いので使い回す）。
fn native_exe(sig: &str, body: &str) -> std::path::PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, std::path::PathBuf>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{sig}|{body}");
    if let Some(p) = cache.lock().unwrap().get(&key) {
        return p.clone();
    }
    let src = prog(sig, body);
    let dir = common::native::scratch("entry");
    let exe = dir.join("prog");
    common::native::try_build(&[&src], "main", &exe, false)
        .unwrap_or_else(|e| panic!("{e}\n{src}"));
    cache.lock().unwrap().insert(key, exe.clone());
    exe
}

/// ネイティブだけを、生の文字列の引数で走らせる。
fn native_run(sig: &str, body: &str, args: &[&str]) -> common::native::Ran {
    let exe = native_exe(sig, body);
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    common::native::exec_env(&exe, &args, &[])
}

/// 入口の誤り: 終了コード 1 で、標準エラーが文言どおり、標準出力は空。
fn expect_err(sig: &str, args: &[&str], want: &str) {
    expect_err_body(sig, "  return x;", args, want);
}

fn expect_err_body(sig: &str, body: &str, args: &[&str], want: &str) {
    let r = native_run(sig, body, args);
    assert_eq!(r.code, Some(1), "{sig} {args:?}: 終了コード");
    assert_eq!(r.stdout, "", "{sig} {args:?}: 標準出力");
    assert_eq!(r.stderr, format!("{want}\n"), "{sig} {args:?}: 標準エラー");
}

#[test]
fn scalars_matrix() {
    for n in [0, 42, -7, i64::MAX, i64::MIN] {
        check("(x: Int) -> Int", Arg::Int(n));
    }
    for x in [
        1.5,
        -0.25,
        0.1,
        3.0,
        1e300,
        f64::from_bits(1),
        f64::MAX,
        f64::MIN_POSITIVE,
    ] {
        check("(x: Float) -> Float", Arg::Float(x));
    }
    // 整数のリテラルは `Float` の入口にも渡せる。
    check("(x: Float) -> Float", Arg::Int(3));
    check("(x: Float) -> Float", Arg::Int(-12));
    for b in [true, false] {
        check("(x: Bool) -> Bool", Arg::Bool(b));
    }
    for s in ["hello", "こんにちは", "", "12abc", "[abc", "1.5x", " x "] {
        check("(x: String) -> String", Arg::Str(s.to_string()));
    }
    check("(x: Path) -> Path", Arg::Str("some/path.txt".to_string()));
}

#[test]
fn list_matrix() {
    check(
        "(x: List<Int>) -> List<Int>",
        Arg::List(vec![Arg::Int(1), Arg::Int(2), Arg::Int(3)]),
    );
    check("(x: List<Int>) -> List<Int>", Arg::List(vec![]));
    check(
        "(x: List<Float>) -> List<Float>",
        Arg::List(vec![Arg::Float(1.5), Arg::Int(2)]),
    );
    check(
        "(x: List<String>) -> List<String>",
        Arg::List(vec![Arg::Str("a b".into()), Arg::Str("c".into())]),
    );
    check(
        "(x: List<Bool>) -> List<Bool>",
        Arg::List(vec![Arg::Bool(true), Arg::Bool(false)]),
    );
    // 入れ子は、要素が 1 つだけの一覧ならカンマの癖に触れずに読める。
    check(
        "(x: List<List<Int>>) -> List<List<Int>>",
        Arg::List(vec![
            Arg::List(vec![Arg::Int(1)]),
            Arg::List(vec![Arg::Int(2)]),
        ]),
    );
}

#[test]
fn ref_matrix() {
    let src = prog("(x: &Int) -> &Int", "  return x;");
    run(&src, "main", &[Arg::Int(7)]).unwrap();
    let src = prog("(x: &String) -> &String", "  return x;");
    run(&src, "main", &[Arg::Str("abc".into())]).unwrap();
    let src = prog("(xs: &List<String>) -> &List<String>", "  return xs;");
    run(&src, "main", &[Arg::List(vec![Arg::Str("a".into())])]).unwrap();
    let src = prog("(x: &mut String) -> &mut String", "  return x;");
    run(&src, "main", &[Arg::Str("abc".into())]).unwrap();
}

#[test]
fn mismatch_errors() {
    expect_err(
        "(x: Int) -> Int",
        &["abc"],
        "入口の指定が誤り: 引数 x の型が合わない: Int が要るが、文字列が渡された",
    );
    expect_err(
        "(x: Int) -> Int",
        &["1.5"],
        "入口の指定が誤り: 引数 x の型が合わない: Int が要るが、小数が渡された",
    );
    expect_err(
        "(x: Int) -> Int",
        &["true"],
        "入口の指定が誤り: 引数 x の型が合わない: Int が要るが、真偽値が渡された",
    );
    expect_err(
        "(x: Int) -> Int",
        &["[1]"],
        "入口の指定が誤り: 引数 x の型が合わない: Int が要るが、一覧が渡された",
    );
    expect_err(
        "(x: Float) -> Float",
        &["abc"],
        "入口の指定が誤り: 引数 x の型が合わない: Float が要るが、文字列が渡された",
    );
    expect_err(
        "(x: Float) -> Float",
        &["[1]"],
        "入口の指定が誤り: 引数 x の型が合わない: Float が要るが、一覧が渡された",
    );
    expect_err(
        "(x: Bool) -> Bool",
        &["1"],
        "入口の指定が誤り: 引数 x の型が合わない: Bool が要るが、整数が渡された",
    );
    expect_err(
        "(x: String) -> String",
        &["5"],
        "入口の指定が誤り: 引数 x の型が合わない: String が要るが、整数が渡された",
    );
    expect_err(
        "(x: Path) -> Path",
        &["true"],
        "入口の指定が誤り: 引数 x の型が合わない: Path が要るが、真偽値が渡された",
    );
    expect_err(
        "(x: List<Int>) -> List<Int>",
        &["5"],
        "入口の指定が誤り: 引数 x の型が合わない: List が要るが、整数が渡された",
    );
    // 一覧の要素の不一致は、要素の型の名前で出る。
    expect_err(
        "(x: List<Int>) -> List<Int>",
        &["[1, abc]"],
        "入口の指定が誤り: 引数 x の型が合わない: Int が要るが、文字列が渡された",
    );
    // 入れ子の一覧は最上位のカンマでも割れる（移行前と同じ癖）:
    // `[[1,2]]` の要素は文字列 `[1` と `2]` になる。
    expect_err(
        "(x: List<List<Int>>) -> List<List<Int>>",
        &["[[1,2]]"],
        "入口の指定が誤り: 引数 x の型が合わない: List が要るが、文字列が渡された",
    );
    // 参照の引数も、指す先の型の名前で出る。
    expect_err(
        "(x: &Int) -> &Int",
        &["abc"],
        "入口の指定が誤り: 引数 x の型が合わない: Int が要るが、文字列が渡された",
    );
}

#[test]
fn count_errors() {
    expect_err(
        "(x: Int, y: Int) -> Int",
        &["1"],
        "入口の指定が誤り: main は引数を 2 個とる（1 個渡された）",
    );
    expect_err(
        "(x: Int) -> Int",
        &["1", "2"],
        "入口の指定が誤り: main は引数を 1 個とる（2 個渡された）",
    );
    expect_err_body(
        "() -> Int",
        "  return 0;",
        &["1"],
        "入口の指定が誤り: main は引数を 0 個とる（1 個渡された）",
    );
}

#[test]
fn range_errors() {
    expect_err(
        "(x: Int) -> Int",
        &["9223372036854775808"],
        "入口の指定が誤り: 整数 9223372036854775808 が Int の範囲外",
    );
    expect_err(
        "(x: Int) -> Int",
        &["-9223372036854775809"],
        "入口の指定が誤り: 整数 -9223372036854775809 が Int の範囲外",
    );
    // 一覧の中の範囲外も、読み取りの段階で出る。
    expect_err(
        "(x: List<Int>) -> List<Int>",
        &["[99999999999999999999]"],
        "入口の指定が誤り: 整数 99999999999999999999 が Int の範囲外",
    );
    // 読み取りの誤りは、個数の不一致より先に出る（移行前と同じ順序）。
    expect_err(
        "(x: Int, y: Int) -> Int",
        &["99999999999999999999"],
        "入口の指定が誤り: 整数 99999999999999999999 が Int の範囲外",
    );
    // 数字だけの書き方は、相手が `Float` でも範囲外の誤りになる。
    expect_err(
        "(x: Float) -> Float",
        &["18446744073709551617"],
        "入口の指定が誤り: 整数 18446744073709551617 が Int の範囲外",
    );
    // 符号が `+` なら範囲外の誤りにはならず、小数として読める。
    let r = native_run(
        "(x: Float) -> Float",
        "  return x;",
        &["+9223372036854775808"],
    );
    assert_eq!(r.code, Some(0), "{:?}", r.stderr);
    let want = "+9223372036854775808".parse::<f64>().unwrap();
    assert_eq!(
        r.stdout,
        format!("{}\n", sigil::shared_text::fmt_float(want))
    );
}

/// `Float` の oracle 行列（I-3）: Rust の `parse` が受け付ける書き方はすべて受け付け、
/// ビット列まで一致する。受け付けない書き方は文字列になり、型の不一致で落ちる。
#[test]
fn float_oracle_matrix() {
    let ok: &[&str] = &[
        "0",
        "-0",
        "0.0",
        "-0.0e10",
        "1.5",
        "-2.75",
        ".5",
        "5.",
        "5.e3",
        ".5e-2",
        "1e10",
        "1E10",
        "1e+10",
        "1e-10",
        "123456789.123456789",
        "0.1",
        "0.30000000000000004",
        "3.141592653589793",
        "2.2250738585072014e-308",
        "1.7976931348623157e308",
        "1.7976931348623159e308",
        "1e309",
        "-1e309",
        "1e-330",
        "5e-324",
        "4.9406564584124654e-324",
        "2.4703282292062327e-324",
        "2.4703282292062328e-324",
        "1e-323",
        "0.500000000000000055511151231257827021181583404541015625",
        "0.500000000000000055511151231257827021181583404541015624",
        "0.500000000000000055511151231257827021181583404541015626",
        "inf",
        "INF",
        "infinity",
        "InFiNiTy",
        "+inf",
        "-inf",
        "NaN",
        "nan",
        "-nan",
        "0.0e999",
        "0e-999",
        "1e1000",
        "1e-1000",
        // 20 桁を超える指数（#314、PR #313 の agy レビュー所見 1）。指数の蓄積は
        // `ev < 10^8` の間だけ更新する頭打ちで、飽和しても溢れない。飽和した
        // 大きさは早期判定（10^309 以上は無限大、10^-324 未満は 0）にそのまま効く。
        "1e99999999999999999999",
        "-1e99999999999999999999",
        "1e-99999999999999999999",
        "-1e-99999999999999999999",
        "5e-99999999999999999999",
        // 仮数が 0 なら、指数が飽和しても 0 のまま（大きさの判定より先に 0 を返す）。
        "0e99999999999999999999",
        // 先行する 0 は蓄積を妨げない（頭打ちは、値が小さい間は働かない）。
        "1e0000000000000000000000000309",
        "1234567890123456789012345678901234567890.5",
        "0.00000000000000000000000000000000000000000000000001",
        "9007199254740993",
    ];
    for s in ok {
        // 整数として読める書き方は、先に `Int` として読まれてから `Float` へ合わされる
        // （たとえば `-0` は正の 0.0 になる）ので、oracle もその順序で求める。
        let want = match s.parse::<i64>() {
            Ok(n) => n as f64,
            Err(_) => s
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("oracle が読めない: {s}")),
        };
        let r = native_run("(x: Float) -> Float", "  return x;", &[s]);
        assert_eq!(r.code, Some(0), "{s}: 終了コード（{:?}）", r.stderr);
        assert_eq!(
            r.stdout,
            format!("{}\n", sigil::shared_text::fmt_float(want)),
            "{s}: 表示"
        );
        assert_eq!(r.stderr, "", "{s}: 標準エラー");
    }
    // 1200 桁を超える有効桁（sticky の効き方まで oracle と比べる）。
    let long_digits = format!("1.{}3", "0".repeat(1300));
    let long_mid = format!(
        "0.{}55511151231257827021181583404541015625",
        "0".repeat(1200)
    );
    for s in [&long_digits, &long_mid] {
        let want = s.parse::<f64>().unwrap();
        let r = native_run("(x: Float) -> Float", "  return x;", &[s]);
        assert_eq!(r.code, Some(0), "{s}: 終了コード（{:?}）", r.stderr);
        assert_eq!(
            r.stdout,
            format!("{}\n", sigil::shared_text::fmt_float(want)),
            "{s}: 表示"
        );
    }
    // 組み立てて作る書き方（#314、PR #313 の agy レビュー所見）。
    // 指数の先行ゼロ: 指数部が 50 桁あっても、頭打ちに触れるまでは正しく蓄積され、
    // 実効の指数は 10 になる。
    let long_exp = format!("1e{}10", "0".repeat(48));
    // sticky の tie-break を踏む対（所見 2）: 1.0 とその次（1 + 2^-52）の厳密な中点は
    // 1 + 2^-53 で、10 進では有効桁 54 桁で書き切れる。この中点の後ろに 0 を足して
    // 有効桁を 1201 桁にすると、最後の 1 桁は 1200 桁の保持から落ちる。保持した桁
    // だけの計算はどちらも同じ中点になるので、この対は sticky だけが丸めの向きを
    // 分けることを固定する。
    // - 落ちる桁が 0 だけなら値は中点のままで、同点として偶数側（1.0）へ丸まる。
    // - 落ちる桁に 1 があると sticky が立ち、中点より大きいものとして切り上がる。
    let mid = "1.00000000000000011102230246251565404236316680908203125";
    let tie_even = format!("{mid}{}", "0".repeat(1147));
    let tie_up = format!("{mid}{}1", "0".repeat(1146));
    for (s, want) in [
        (&long_exp, 1e10),
        (&tie_even, 1.0),
        (&tie_up, 1.0 + f64::EPSILON),
    ] {
        // 期待値は手元の計算どおりであることを、oracle 自身でも確かめる。
        assert_eq!(
            s.parse::<f64>().unwrap().to_bits(),
            want.to_bits(),
            "{s}: oracle の期待値"
        );
        let r = native_run("(x: Float) -> Float", "  return x;", &[s]);
        assert_eq!(r.code, Some(0), "{s}: 終了コード（{:?}）", r.stderr);
        assert_eq!(
            r.stdout,
            format!("{}\n", sigil::shared_text::fmt_float(want)),
            "{s}: 表示"
        );
    }
    let ng: &[&str] = &[
        ".e3", "1e", "1e+", "+", "-", ".", "", "0x1", "1_000", " 1", "1 ", "5e+", "e5", ".5e",
        "infx", "nanx", "1.2.3", "--1", "++1", "1ee5",
    ];
    for s in ng {
        assert!(s.parse::<f64>().is_err(), "oracle が読めてしまう: {s:?}");
        expect_err(
            "(x: Float) -> Float",
            &[s],
            "入口の指定が誤り: 引数 x の型が合わない: Float が要るが、文字列が渡された",
        );
    }
}

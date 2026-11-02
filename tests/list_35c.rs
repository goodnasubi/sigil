//! ADR 0025 補足 E の 3-5c-i（`List` の下準備）。
//!
//! - `RtList` の頭を `blocks=` に数える。`List` を作っては捨てる繰り返しで、`blocks=`（Rust の頭）と
//!   `sigil_blocks=`（`List<String>` の要素の文字列・`lines` が作る各行の文字列）が、1000 回と 100000 回で完全に一致する。
//! - `List.min`/`List.max` は codegen のループ（`val_cmp`）。同順位・比べられない値（NaN）・1 要素・空で、
//!   評価器とネイティブが一致する（`common::run` が両方を走らせて突き合わせる）。
//! - `Bytes.from_list` は一覧の中身を `len`/`get` 経由で読む。

mod common;
use common::native::{exec_env, scratch, try_build};
use common::run;
use sigil::exec::Arg;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

const DEFS: &str = "struct P {\n  k: Int,\n  name: String,\n}\n\n";

/// `List` の作る・読む・捨てる経路を一通り通る繰り返し。1 回で 4 を足す（`total`）。
fn churn() -> String {
    let mut s = String::from(HEAD);
    s.push_str(DEFS);
    s.push_str(&func(
        "run(n: Int) -> Int",
        r#"let mut i = 0;
let mut total = 0;
while i < n {
    let mut xs = List.new();
    let mut j = 0;
    while j < 20 {
        xs.push(j);
        j = j + 1;
    }
    let ys = xs.map(|x| x + 1);
    total = total + ys.sum() - 209;
    let f = xs.find(|x| x == 7);
    total = total + f.map(|x| x - 6).unwrap_or(0);
    total = total + xs.min() + xs.max() - 19;
    let mb = ys.max_by(|x| 0 - x);
    total = total + mb.map(|x| x - 0).unwrap_or(0) - 1;
    match xs.get_mut(3) {
        Some(a) => {
            a = 3;
        }
        None => {}
    }
    let zs = xs.clone();
    total = total + zs.len() - 19;
    let mut ss = List.new();
    ss.push("ab");
    ss.push("c");
    ss.push("zz");
    let ss2 = ss.clone();
    total = total + ss2.max().len() + ss2.min().len() - 3;
    let ls = "a\nb\r\nc".lines();
    total = total + ls.len();
    let mut nest = List.new();
    nest.push(ss);
    nest.push(ls);
    let nc = nest.clone();
    total = total + nc.len() - 1;
    let mut ps = List.new();
    ps.push(P { k: 1, name: "q" });
    ps.push(P { k: 2, name: "rr" });
    let pc = ps.clone();
    total = total + pc.len() - 1;
    let mut bs = List.new();
    bs.push(1);
    bs.push(255);
    match Bytes.from_list(&bs) {
        Ok(b) => {
            total = total + b.len() - 1;
        }
        Err(e) => {
            total = total + 1000;
        }
    }
    i = i + 1;
}
return total;"#,
    ));
    s
}

fn live(src: &str, n: i64) -> (i64, i64, String) {
    let dir = scratch("list35c");
    let exe = dir.join("prog");
    try_build(&[src], "run", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec_env(&exe, &[n.to_string()], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "終了コード\n{}\n{src}", got.stderr);
    let line = got
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {}", got.stderr))
        .to_string();
    let num = |key: &str| -> i64 {
        line.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{key} がない: {line}"))
    };
    assert_eq!(num("strs="), 0, "strs= は 0 固定: {line}");
    (num("sigil_blocks="), num("blocks="), got.stdout)
}

/// `blocks=`（Rust の `RtList` の頭）と `sigil_blocks=`（要素の文字列・`lines` の各行）の完全一致。
#[test]
fn list_blocks_are_exact_across_1000_and_100000_iterations() {
    let src = churn();
    let (s1, b1, out1) = live(&src, 1_000);
    let (s2, b2, out2) = live(&src, 100_000);
    assert!(out1.trim_end().ends_with("10000"), "{out1}");
    assert!(out2.trim_end().ends_with("1000000"), "{out2}");
    assert_eq!(
        (s1, b1),
        (s2, b2),
        "1000 回と 100000 回で生きている数が違う"
    );
    // 終了時に残るのは、入口の結果の表示などの一定の分だけ。
    assert_eq!((s2, b2), (0, 0), "終了時に List の塊が残っている");
}

/// 頭は `blocks=` に数えられる: 一覧を作ったまま終わる事例で、数が 0 から動くことを確かめる。
#[test]
fn list_head_is_counted_in_blocks() {
    let mut s = String::from(HEAD);
    s.push_str(&func(
        "run(n: Int) -> List<Int>",
        "let mut xs = List.new();\nxs.push(n);\nreturn xs;",
    ));
    // 入口の結果の一覧は表示後に捨てられるので、終了時は 0。
    let dir = scratch("list35c-head");
    let exe = dir.join("prog");
    try_build(&[&s], "run", &exe, false).unwrap();
    let got = exec_env(&exe, &["5".into()], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert!(got.stderr.contains("blocks=0"), "{}", got.stderr);
}

fn ints(xs: &[i64]) -> Arg {
    Arg::List(xs.iter().map(|n| Arg::Int(*n)).collect())
}

fn floats(xs: &[f64]) -> Arg {
    Arg::List(xs.iter().map(|n| Arg::Float(*n)).collect())
}

fn strs(xs: &[&str]) -> Arg {
    Arg::List(xs.iter().map(|s| Arg::Str(s.to_string())).collect())
}

fn out(src: &str, f: &str, args: &[Arg]) -> String {
    match run(src, f, args) {
        Ok(o) => o.value,
        Err(e) => panic!("{e}\n{src}"),
    }
}

/// min/max: 同順位は先のもの、1 要素、文字列（要素の記述子が `val_cmp` に渡る）。
#[test]
fn list_min_max_loop_matches_evaluator() {
    let src = format!(
        "{HEAD}{DEFS}{}{}{}{}",
        func("mn(xs: &List<Int>) -> Int", "return xs.min();"),
        func("mx(xs: &List<Int>) -> Int", "return xs.max();"),
        func(
            "ms(xs: &List<String>) -> String",
            "return xs.max().clone();"
        ),
        func(
            "ns(xs: &List<String>) -> String",
            "return xs.min().clone();"
        ),
    );
    assert_eq!(out(&src, "mn", &[ints(&[5])]), "5");
    assert_eq!(out(&src, "mx", &[ints(&[5])]), "5");
    assert_eq!(out(&src, "mn", &[ints(&[3, 1, 2, 1, 3])]), "1");
    assert_eq!(out(&src, "mx", &[ints(&[3, 1, 2, 1, 3])]), "3");
    assert_eq!(
        out(&src, "mn", &[ints(&[i64::MIN, 0, i64::MAX])]),
        i64::MIN.to_string()
    );
    assert_eq!(
        out(&src, "mx", &[ints(&[i64::MIN, 0, i64::MAX])]),
        i64::MAX.to_string()
    );
    assert_eq!(
        out(&src, "ms", &[strs(&["pear", "apple", "zoo", "apple"])]),
        "\"zoo\""
    );
    assert_eq!(
        out(&src, "ns", &[strs(&["pear", "apple", "zoo", "apple"])]),
        "\"apple\""
    );
    // 空は契約違反の停止（評価器とネイティブで一致）。
    assert!(run(&src, "mn", &[ints(&[])]).is_err());
    assert!(run(&src, "mx", &[ints(&[])]).is_err());
}

/// 同順位の先勝ち: 鍵が等しい構造体は比べられないので、`Float` で -0.0 と 0.0 の順を見る。
#[test]
fn list_min_max_ties_and_nan() {
    let src = format!(
        "{HEAD}{}{}",
        func("mn(xs: &List<Float>) -> Float", "return xs.min();"),
        func("mx(xs: &List<Float>) -> Float", "return xs.max();"),
    );
    assert_eq!(out(&src, "mn", &[floats(&[2.5, -1.5])]), "-1.5");
    assert_eq!(out(&src, "mx", &[floats(&[2.5, -1.5])]), "2.5");
    // -0.0 と 0.0 は同順位: 先のものが残る。
    let nz = floats(&[-0.0, 0.0]);
    let zn = floats(&[0.0, -0.0]);
    let a = out(&src, "mn", std::slice::from_ref(&nz));
    let b = out(&src, "mn", std::slice::from_ref(&zn));
    assert_ne!(a, b, "同順位は先のものを残す: {a} {b}");
    // NaN: 比べられない相手は取らない。先頭が NaN なら NaN が残る。
    let nan = f64::NAN;
    for xs in [
        vec![nan, 1.0, 2.0],
        vec![1.0, nan, 2.0],
        vec![1.0, 2.0, nan],
        vec![nan],
        vec![nan, nan],
    ] {
        let a = floats(&xs);
        let v1 = out(&src, "mn", std::slice::from_ref(&a));
        let v2 = out(&src, "mx", std::slice::from_ref(&a));
        let _ = (v1, v2);
    }
}

/// `Bytes.from_list`: 範囲内・範囲外・空・長い一覧（`cap` が伸びる）。
#[test]
fn bytes_from_list_reads_through_len_and_get() {
    let src = format!(
        "{HEAD}{}",
        func(
            "bl(xs: List<Int>) -> String",
            "match Bytes.from_list(&xs) {\n    Ok(b) => {\n        return \"ok\";\n    }\n    Err(e) => {\n        return e;\n    }\n}"
        ),
    );
    assert_eq!(out(&src, "bl", &[ints(&[])]), "\"ok\"");
    assert_eq!(out(&src, "bl", &[ints(&[0, 255])]), "\"ok\"");
    assert_eq!(out(&src, "bl", &[ints(&[0, 256])]), "\"0..=255 の範囲外\"");
    assert_eq!(out(&src, "bl", &[ints(&[-1])]), "\"0..=255 の範囲外\"");
    let long: Vec<i64> = (0..100).map(|i| i % 256).collect();
    assert_eq!(out(&src, "bl", &[ints(&long)]), "\"ok\"");
    let mut bad = long.clone();
    bad[99] = 300;
    assert_eq!(out(&src, "bl", &[ints(&bad)]), "\"0..=255 の範囲外\"");
}

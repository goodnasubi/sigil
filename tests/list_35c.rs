//! ADR 0025 補足 E の 3-5c-i（`List` の下準備）。
//!
//! - `List` を作っては捨てる繰り返しで、`blocks=` と `sigil_blocks=` が、1000 回と 100000 回で完全に一致する。
//!   3-5c-i では `blocks=` が Rust の `RtList` の頭を、`sigil_blocks=` が `List<String>` の要素の文字列・`lines` の各行を
//!   数えた。3-5c-ii からは一覧の頭と領域も `sigil_blocks=` が数え、`blocks=` は 0 のまま（E-5）。
//! - `List.min`/`List.max` は codegen のループ（`val_cmp`）。同順位・比べられない値（NaN）・1 要素・空で、
//!   評価器とネイティブが一致する（`common::run` が両方を走らせて突き合わせる）。
//! - `Bytes.from_list` は一覧の中身を `len`/`get` 経由で読む。
//!
//! 3-5c-ii（表現の切り替え。`std.core.list`）:
//! - 一覧の頭と要素の領域は Sigil 製の確保器から取るので、`blocks=` ではなく `sigil_blocks=` が数える（E-5）。
//! - 直接の試験: 駆動モジュール（`std.core.t`）から `std.core.list` の関数を評価器とネイティブで呼び、Rust の `Vec`
//!   の模型と突き合わせる。評価器の塊の表（二重解放・解放後の読み・大きさ違いの解放・取り直した古い領域の読み）。

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

/// `blocks=` と `sigil_blocks=`（一覧の頭と領域・要素の文字列・`lines` の各行）の完全一致。
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

/// 一覧の頭と領域は `sigil_blocks=` に数えられ、`blocks=` には入らない。
/// 一覧を作ったまま終わる駆動で、数が 0 から動く（空は頭の 1 塊、要素を持てば 2 塊）ことと、
/// 取り直しで古い領域が返る（数が増えない）ことを確かめる。
#[test]
fn list_head_is_counted_in_sigil_blocks() {
    let push_n = |n: usize| {
        format!(
            "  let l = create(8);\n  let t = raw.alloc(8, 8);\n  raw.store_i64(t, 5);\n{}  raw.free(t, 8, 8);\n  return len(l);",
            "  push(l, t);\n".repeat(n)
        )
    };
    let leak = |n: usize| {
        let r = native_driver(LU, &push_n(n));
        assert_eq!(r.code, Some(0), "{}", r.stderr);
        live_line(&r)
    };
    // 空の一覧は頭だけ。
    assert_eq!(leak(0), "strs=0 blocks=0 sigil_blocks=1");
    // 追加すると領域が 1 塊増える。4 → 8 → 16 と取り直しても増えない（古い領域は返る）。
    for n in [1, 4, 5, 8, 9, 16, 17] {
        assert_eq!(leak(n), "strs=0 blocks=0 sigil_blocks=2", "n={n}");
    }
    // 作って捨てると 0 に戻る。
    let freed = "  let l = create(8);\n  let t = raw.alloc(8, 8);\n  raw.store_i64(t, 5);\n  push(l, t);\n  raw.free(t, 8, 8);\n  free(l);\n  return 0;";
    let r = native_driver(LU, freed);
    assert_eq!(live_line(&r), "strs=0 blocks=0 sigil_blocks=0");
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
    // NaN: 比べられない相手は取らない（最初の要素を仮の最良とし、`Less`/`Greater` のときだけ取り替える）。
    // 先頭が NaN なら NaN が残る。NaN が途中・末尾にあれば、残りの数どうしで決まる。
    let nan = f64::NAN;
    let cases: [(Vec<f64>, &str, &str); 5] = [
        (vec![nan, 1.0, 2.0], "NaN", "NaN"),
        (vec![1.0, nan, 2.0], "1.0", "2.0"),
        (vec![1.0, 2.0, nan], "1.0", "2.0"),
        (vec![nan], "NaN", "NaN"),
        (vec![nan, nan], "NaN", "NaN"),
    ];
    for (xs, want_min, want_max) in cases {
        let a = floats(&xs);
        assert_eq!(
            out(&src, "mn", std::slice::from_ref(&a)),
            want_min,
            "min {xs:?}"
        );
        assert_eq!(
            out(&src, "mx", std::slice::from_ref(&a)),
            want_max,
            "max {xs:?}"
        );
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

// ---- 3-5c-ii: `std.core.list` の直接の試験（駆動モジュールから呼ぶ） ----

use common::native::Ran;
use sigil::effects::Options as COpts;
use sigil::exec::Options;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";
const MODP: i64 = 1_000_000_007;
/// 駆動が取り込む `std.core.list` の関数（本体が呼ぶものだけが実際に取り込まれる）。
const LU: &[&str] = &["create", "free", "push", "get", "len"];

fn driver(uses: &[&str], body: &str) -> String {
    // 使わない `use` は検査で拒否されるので、本体が呼ぶものだけを（文字の順に）取り込む。
    let called = |f: &str| {
        let pat = format!("{f}(");
        body.match_indices(&pat).any(|(i, _)| {
            !body[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        })
    };
    let mut sorted: Vec<&str> = uses.iter().copied().filter(|f| called(f)).collect();
    sorted.sort_unstable();
    let u: String = sorted
        .iter()
        .map(|f| format!("use std.core.list.{f};\n"))
        .collect();
    format!(
        "module std.core.t\n  targets server\n  trust base\n\n{u}\npub fn run() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n"
    )
}

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n")
}

fn copts(uses: &[&str], body: &str) -> COpts {
    COpts {
        embedded_extra: vec![("std.core.t".to_string(), driver(uses, body))],
        embedded_roots: vec!["std.core.t".into()],
        ..Default::default()
    }
}

fn eval_driver(uses: &[&str], body: &str) -> Result<String, String> {
    let c = copts(uses, body);
    let o = Options {
        embedded_extra: c.embedded_extra,
        embedded_roots: c.embedded_roots,
        ..Options::captured()
    };
    match sigil::ir_eval::run_sources_with(&[&user_entry()], "std.core.t.run", &[], &o) {
        Ok(o) => Ok(o.value),
        Err(f) => Err(format!("{:?}", f.error)),
    }
}

fn native_driver(uses: &[&str], body: &str) -> Ran {
    let c = copts(uses, body);
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.core.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("list35c-drv");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    got
}

fn live_line(r: &common::native::Ran) -> String {
    r.stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {}", r.stderr))
        .to_string()
}

/// 位置 `i` の要素の値（64 ビットの端と負の値を混ぜる）。
fn elem(i: i64, k: i64) -> i64 {
    match i % 5 {
        0 => i64::MAX,
        1 => i64::MIN,
        2 => -1,
        _ => i * 7 + 1 + k,
    }
}

/// `n` 個を追加して全部読み、範囲外の読み（-1・len・len+1000）が空番地であることまで見る断片。結果は `r{k}`。
fn vec_block(n: usize, k: usize) -> String {
    format!(
        "  let l{k} = create(8);\n  let t{k} = raw.alloc(8, 8);\n  let mut i{k} = 0;\n  while i{k} < {n} {{\n    let mut v{k} = i{k} * 7 + 1 + {k};\n    if i{k} % 5 == 0 {{\n      v{k} = 9223372036854775807;\n    }} else if i{k} % 5 == 1 {{\n      v{k} = -9223372036854775807 - 1;\n    }} else if i{k} % 5 == 2 {{\n      v{k} = -1;\n    }}\n    raw.store_i64(t{k}, v{k});\n    push(l{k}, t{k});\n    i{k} = i{k} + 1;\n  }}\n  raw.free(t{k}, 8, 8);\n  let mut h{k} = 0;\n  let mut j{k} = 0;\n  while j{k} < len(l{k}) {{\n    h{k} = (h{k} * 131 + ((raw.load_i64(get(l{k}, j{k})) % {MODP}) + {MODP}) % {MODP}) % {MODP};\n    j{k} = j{k} + 1;\n  }}\n  let mut nb{k} = 0;\n  if raw.addr_to_int(get(l{k}, -1)) == 0 {{\n    nb{k} = nb{k} + 1;\n  }}\n  if raw.addr_to_int(get(l{k}, len(l{k}))) == 0 {{\n    nb{k} = nb{k} + 1;\n  }}\n  if raw.addr_to_int(get(l{k}, len(l{k}) + 1000)) == 0 {{\n    nb{k} = nb{k} + 1;\n  }}\n  let n{k} = len(l{k});\n  free(l{k});\n  let r{k} = (h{k} * 1000 + n{k}) * 4 + nb{k};\n"
    )
}

fn vec_expect(n: usize, k: usize) -> i64 {
    let mut h = 0i64;
    for i in 0..n as i64 {
        let v = elem(i, k as i64);
        h = (h * 131 + v.rem_euclid(MODP)) % MODP;
    }
    (h * 1000 + n as i64) * 4 + 3
}

const SIZES: &[usize] = &[0, 1, 3, 4, 5, 8, 9, 16, 17, 31, 32, 33, 64, 65, 100, 1000];

#[test]
fn list_ops_match_a_vec_model_in_the_evaluator() {
    for (k, &n) in SIZES.iter().enumerate() {
        let body = format!("{}  return r{k};", vec_block(n, k));
        let got = eval_driver(LU, &body).unwrap_or_else(|e| panic!("n={n}: 評価器が失敗: {e}"));
        assert_eq!(got, vec_expect(n, k).to_string(), "n={n}");
    }
}

#[test]
fn list_ops_match_a_vec_model_natively_and_leave_no_blocks() {
    let mut body = String::new();
    let mut acc = String::from("  let mut acc = 0;\n");
    let mut want = 0i64;
    for (k, &n) in SIZES.iter().enumerate() {
        body += &vec_block(n, k);
        acc += &format!("  acc = (acc * 1000003 + r{k}) % {MODP};\n");
        want = (want * 1000003 + vec_expect(n, k)) % MODP;
    }
    body += &acc;
    body += "  return acc;";
    let got = native_driver(LU, &body);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want.to_string()
    );
    assert_eq!(live_line(&got), "strs=0 blocks=0 sigil_blocks=0");
}

/// 評価器の塊の表: 取り直した古い領域は返り、二重解放・解放後の読み・大きさ違いの解放・`elem != 8` は止まる。
#[test]
fn evaluator_block_table_checks_for_lists() {
    let mk = "  let l = create(8);\n  let t = raw.alloc(8, 8);\n  raw.store_i64(t, 11);\n";
    let fin = "  raw.free(t, 8, 8);\n";
    // 正常。
    let ok = format!("{mk}  push(l, t);\n  let r = raw.load_i64(get(l, 0)) + len(l);\n  free(l);\n{fin}  return r;");
    assert_eq!(eval_driver(LU, &ok).unwrap(), "12");
    // 空の一覧の解放（領域なし）。
    let empty = format!("{mk}  free(l);\n{fin}  return 0;");
    assert_eq!(eval_driver(LU, &empty).unwrap(), "0");
    // free(空番地) は何もしない。
    let null = format!("{mk}  free(raw.addr_null());\n  free(l);\n{fin}  return 0;");
    assert_eq!(eval_driver(LU, &null).unwrap(), "0");
    // 4 要素まで同じ領域、5 つ目で取り直す: 古い領域の読みは止まる。
    let four = format!("{mk}  push(l, t);\n  let a = get(l, 0);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  let same = raw.load_i64(a);\n  free(l);\n{fin}  return same;");
    assert_eq!(eval_driver(LU, &four).unwrap(), "11");
    let grow = format!("{mk}  push(l, t);\n  let a = get(l, 0);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  let r = raw.load_i64(a);\n  free(l);\n{fin}  return r;");
    assert!(
        eval_driver(LU, &grow).is_err(),
        "取り直した古い領域が返っていない"
    );
    // 8 → 16 でも同じ。
    let mut g16 = format!("{mk}  push(l, t);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  push(l, t);\n  let a = get(l, 0);\n");
    g16 += "  push(l, t);\n  let r = raw.load_i64(a);\n  free(l);\n";
    g16 += fin;
    g16 += "  return r;";
    assert!(eval_driver(LU, &g16).is_err());
    // 二重解放。
    let dbl = format!("{mk}  push(l, t);\n  free(l);\n  free(l);\n{fin}  return 0;");
    assert!(eval_driver(LU, &dbl).is_err(), "二重解放が止まらない");
    let dbl_empty = format!("{mk}  free(l);\n  free(l);\n{fin}  return 0;");
    assert!(eval_driver(LU, &dbl_empty).is_err());
    // 解放後の len・get・push。
    for after in [
        "return len(l);",
        "return raw.load_i64(get(l, 0));",
        "push(l, t);\n  return 0;",
    ] {
        let b = format!("{mk}  push(l, t);\n  free(l);\n  {after}");
        assert!(eval_driver(LU, &b).is_err(), "解放後が止まらない: {after}");
    }
    // 解放後の領域の読み。
    let uaf =
        format!("{mk}  push(l, t);\n  let a = get(l, 0);\n  free(l);\n  return raw.load_i64(a);");
    assert!(eval_driver(LU, &uaf).is_err());
    // 大きさ違いの解放（cap を書き換えた頭）は止まる。
    let badcap = format!(
        "{mk}  push(l, t);\n  raw.store_i64(raw.offset(l, 16), 3);\n  free(l);\n{fin}  return 0;"
    );
    assert!(
        eval_driver(LU, &badcap).is_err(),
        "cap の食い違いが止まらない"
    );
    // 要素の大きさが 8 以外なら止まる。
    for e in [0, 16, -8, 1] {
        let b = format!("  let l = create({e});\n  free(l);\n  return 0;");
        assert!(eval_driver(LU, &b).is_err(), "create({e}) が止まらない");
    }
}

#[test]
fn native_list_stops_with_70() {
    for body in [
        "  let l = create(0);\n  free(l);\n  return 0;",
        "  let l = create(16);\n  free(l);\n  return 0;",
        "  let l = create(8);\n  free(l);\n  free(l);\n  return 0;",
    ] {
        let r = native_driver(LU, body);
        assert_eq!(r.code, Some(70), "{body}\n{}", r.stderr);
    }
}

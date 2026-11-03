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
        " / ",
        " % ",
        "return if",
    ] {
        assert!(!code.contains(bad), "分岐になりうる `{bad}` がある");
    }
}

// ---------------------------------------------------------------------------
// 3-5d-ii の前提（基準線。ADR 0025 補足 F-9）: `Bytes` の今（旧実装）の振る舞いと生存数の固定。
// 実装は変えない。3-5d-ii は、これらの値を変えずに通ることを確かめる。

use common::native::{exec, exec_env, scratch, try_build};

const MODP: i64 = 1_000_000_007;

/// 列の中身を畳む補助（長さと各バイト）。Rust の `hb` と同じ式。
const HB: &str = "fn hb(b: &Bytes) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut h = b.len();\n  let mut i = 0;\n  while i < b.len() {\n    h = (h * 257 + b.get(i).unwrap_or(0)) % 1000000007;\n    i = i + 1;\n  }\n  return h;\n}\n\n";

fn hb(b: &[u8]) -> i64 {
    let mut h = b.len() as i64;
    for &x in b {
        h = (h * 257 + x as i64) % MODP;
    }
    h
}

/// 基準の列 [10, 20, 30, 40, 250] を作る断片（変数 `b{k}`）。
fn fixed(k: usize) -> String {
    let mut s = format!("  let mut b{k} = Bytes.new();\n");
    for v in [10, 20, 30, 40, 250] {
        s += &format!("  b{k}.push({v});\n");
    }
    s
}
const FIXED: [u8; 5] = [10, 20, 30, 40, 250];

fn fold(_k: usize, code: &str) -> String {
    format!("  acc = (acc * 1000003 + {code}) % {MODP};\n")
}

fn fold_ref(acc: i64, code: i64) -> i64 {
    (acc * 1000003 + code) % MODP
}

fn grid_src(body: &str) -> String {
    format!("{HEAD}{HB}pub fn f() -> Int\n{TAIL}{{\n  let mut acc = 0;\n{body}  return acc;\n}}\n")
}

fn run0(src: &str) -> String {
    common::run(src, "f", &[]).unwrap().value
}

const IDX: &[i64] = &[-1, 0, 4, 5, i64::MAX, i64::MIN];
const VALS: &[i64] = &[-1, 0, 255, 256, i64::MAX, i64::MIN];

/// `set`: 添字だけ不正・値だけ不正・両方不正を含む全組。失敗しても中身は変わらない。
#[test]
fn baseline_set_grid() {
    let mut body = String::new();
    let mut want = 0;
    let mut k = 0;
    for &i in IDX {
        for &v in VALS {
            body += &fixed(k);
            body += &format!(
                "  let r{k} = b{k}.set({}, {});\n  let mut st{k} = 0;\n  match r{k} {{\n    Ok(u) => {{\n      st{k} = 1;\n    }}\n    Err(e) => {{}}\n  }}\n",
                lit(i),
                lit(v)
            );
            body += &fold(k, &format!("hb(&b{k}) * 2 + st{k}"));
            let mut m = FIXED.to_vec();
            let ok = (0..5).contains(&i) && (0..=255).contains(&v);
            if ok {
                m[i as usize] = v as u8;
            }
            want = fold_ref(want, hb(&m) * 2 + ok as i64);
            k += 1;
        }
    }
    assert_eq!(run0(&grid_src(&body)), want.to_string());
}

const SE: &[i64] = &[-1, 0, 2, 5, 6, i64::MAX, i64::MIN];

/// `slice`: 範囲の全組（`s > e`・負・長さ超え・端を含む）。
#[test]
fn baseline_slice_grid() {
    let mut body = String::new();
    let mut want = 0;
    let mut k = 0;
    for &s in SE {
        for &e in SE {
            body += &fixed(k);
            body += &format!(
                "  let mut c{k} = 0;\n  match b{k}.slice({}, {}) {{\n    Ok(x) => {{\n      c{k} = hb(&x) * 2 + 1;\n    }}\n    Err(m) => {{}}\n  }}\n",
                lit(s),
                lit(e)
            );
            body += &fold(k, &format!("c{k}"));
            let ok = 0 <= s && s <= e && e <= 5;
            let c = if ok {
                hb(&FIXED[s as usize..e as usize]) * 2 + 1
            } else {
                0
            };
            want = fold_ref(want, c);
            k += 1;
        }
    }
    assert_eq!(run0(&grid_src(&body)), want.to_string());
}

/// `push` は下位 8 ビットを足す（0..=255 の外の値でも止まらない）。`push_u16_be` などは下位の 16/32/64 ビット。
#[test]
fn baseline_push_and_encodings() {
    let vs: &[i64] = &[
        0,
        1,
        255,
        256,
        -1,
        0x1FF,
        0x1234_5678_9ABC_DEF0,
        i64::MAX,
        i64::MIN,
    ];
    let mut body = String::new();
    let mut want = 0;
    let mut k = 0;
    for &v in vs {
        let kinds = [
            "push",
            "push_u16_be",
            "push_u32_be",
            "push_u64_be",
            "push_u32_le",
        ];
        for (ki, m) in kinds.iter().enumerate() {
            body += &fixed(k);
            body += &format!("  b{k}.{m}({});\n", lit(v));
            body += &fold(k, &format!("hb(&b{k})"));
            let mut r = FIXED.to_vec();
            match ki {
                0 => r.push(v as u8),
                1 => r.extend((v as u16).to_be_bytes()),
                2 => r.extend((v as u32).to_be_bytes()),
                3 => r.extend((v as u64).to_be_bytes()),
                _ => r.extend((v as u32).to_le_bytes()),
            }
            want = fold_ref(want, hb(&r));
            k += 1;
        }
    }
    assert_eq!(run0(&grid_src(&body)), want.to_string());
}

/// `read_u32_be` の `off` の境界（負・先頭・`len - 4`・`len - 3`・`Int` の端）。
#[test]
fn baseline_read_u32_be_bounds() {
    let data: [u8; 8] = [1, 2, 3, 4, 0x80, 0xFF, 0, 0x7F];
    let offs: &[i64] = &[-1, 0, 1, 3, 4, 5, 8, i64::MAX, i64::MIN];
    let mut body = String::from("  let mut b = Bytes.new();\n");
    for d in data {
        body += &format!("  b.push({d});\n");
    }
    let mut want = 0;
    for (k, &o) in offs.iter().enumerate() {
        body += &format!(
            "  let mut c{k} = 0;\n  match b.read_u32_be({}) {{\n    Some(x) => {{\n      c{k} = x + 1;\n    }}\n    None => {{}}\n  }}\n",
            lit(o)
        );
        body += &fold(k, &format!("c{k}"));
        let c = if (0..=4).contains(&o) {
            let o = o as usize;
            u32::from_be_bytes(data[o..o + 4].try_into().unwrap()) as i64 + 1
        } else {
            0
        };
        want = fold_ref(want, c);
    }
    assert_eq!(run0(&grid_src(&body)), want.to_string());
}

/// `ct_eq` と `is_empty` の断片と期待値。長さ違い・同じ長さで先頭だけ/末尾だけ違う・空どうし。
fn ct_eq_case() -> (String, i64) {
    let pairs: &[(&str, &str)] = &[
        ("", ""),
        ("abc", "abc"),
        ("abc", "xbc"),
        ("abc", "abx"),
        ("abc", "abcd"),
        ("abcd", "abc"),
        ("", "a"),
    ];
    let mut body = String::new();
    let mut want = 0;
    for (k, (a, b)) in pairs.iter().enumerate() {
        body += &format!(
            "  let x{k} = \"{a}\".as_bytes();\n  let y{k} = \"{b}\".as_bytes();\n  let mut c{k} = 0;\n  let q{k} = ct_eq(&x{k}, &y{k});\n  if q{k} {{\n    c{k} = 1;\n  }}\n  let e{k} = x{k}.is_empty();\n  if e{k} {{\n    c{k} = c{k} + 2;\n  }}\n"
        );
        body += &fold(k, &format!("c{k}"));
        want = fold_ref(want, (a == b) as i64 + 2 * a.is_empty() as i64);
    }
    (grid_src(&body), want)
}

/// 評価器の上の `ct_eq`・`is_empty`。ネイティブは下の試験（既知の不具合で無視中）。
#[test]
fn baseline_ct_eq_and_is_empty_evaluator() {
    let (src, want) = ct_eq_case();
    let r = sigil::ir_eval::run_sources_with(&[&src], "f", &[], &Options::captured())
        .unwrap_or_else(|f| panic!("{:?}", f.error));
    assert_eq!(r.value, want.to_string());
}

/// 既知の不具合: 旧実装のネイティブは `ct_eq`・`is_empty` を含むプログラムを作れない（codegen の `CtEq`・`BytesIsEmpty` の腕が、
/// すでに 64 ビットに伸ばした `rt` の戻りへもう一度 `uextend` して、Cranelift の検証器が落ちる）。3-5d-ii（`ct_eq`・`is_empty` を
/// Sigil へ移す PR）で直すときに、`#[ignore]` を外す。
#[test]
#[ignore = "既知の不具合: ct_eq・is_empty はネイティブを作れない（codegen の uextend）"]
fn baseline_ct_eq_and_is_empty_native() {
    let (src, want) = ct_eq_case();
    assert_eq!(run0(&src), want.to_string());
}

/// `from_list` / `as_bytes` / `to_string_utf8` の成功と失敗（範囲外の要素・不正な UTF-8）。
#[test]
fn baseline_from_list_and_utf8() {
    let lists: &[&[i64]] = &[
        &[],
        &[0],
        &[0, 255],
        &[1, 2, 3],
        &[-1],
        &[256],
        &[1, 2, 300, 4],
        &[7, -5],
        &[i64::MAX],
        &[i64::MIN],
    ];
    let utf8: &[&[u8]] = &[
        b"",
        b"abc",
        "é€😀".as_bytes(),
        &[0xC3],
        &[0xE2, 0x82],
        &[0xF0, 0x9F, 0x98],
        &[0xFF],
        &[0x80],
        &[0xC0, 0xAF],
        &[0xED, 0xA0, 0x80],
        &[0xF4, 0x90, 0x80, 0x80],
        &[b'a', 0xC3, 0x28],
        &[b'a', b'b', 0xE2, 0x82],
    ];
    let mut body = String::new();
    let mut want = 0;
    let mut k = 0;
    for l in lists {
        body += &format!("  let mut l{k} = List.new();\n");
        for v in *l {
            body += &format!("  l{k}.push({});\n", lit(*v));
        }
        body += &format!(
            "  let mut c{k} = 0;\n  match Bytes.from_list(&l{k}) {{\n    Ok(x) => {{\n      c{k} = hb(&x) * 2 + 1;\n    }}\n    Err(m) => {{}}\n  }}\n"
        );
        body += &fold(k, &format!("c{k}"));
        let c = if l.iter().all(|v| (0..=255).contains(v)) {
            let bs: Vec<u8> = l.iter().map(|v| *v as u8).collect();
            hb(&bs) * 2 + 1
        } else {
            0
        };
        want = fold_ref(want, c);
        k += 1;
    }
    for u in utf8 {
        body += &format!("  let mut l{k} = List.new();\n");
        for v in *u {
            body += &format!("  l{k}.push({v});\n");
        }
        body += &format!(
            "  let mut c{k} = 0;\n  match Bytes.from_list(&l{k}) {{\n    Ok(x) => {{\n      match x.to_string_utf8() {{\n        Ok(s) => {{\n          let y = s.as_bytes();\n          c{k} = hb(&y) * 2 + 1;\n        }}\n        Err(m) => {{}}\n      }}\n    }}\n    Err(m) => {{}}\n  }}\n"
        );
        body += &fold(k, &format!("c{k}"));
        let c = match String::from_utf8(u.to_vec()) {
            Ok(s) => hb(s.as_bytes()) * 2 + 1,
            Err(_) => 0,
        };
        want = fold_ref(want, c);
        k += 1;
    }
    assert_eq!(run0(&grid_src(&body)), want.to_string());
}

/// 評価器の結果（文字列）。
fn eval_str(src: &str, args: &[i64]) -> String {
    let a: Vec<Arg> = args.iter().map(|n| Arg::Int(*n)).collect();
    let r = sigil::ir_eval::run_sources_with(&[src], "f", &a, &Options::captured())
        .unwrap_or_else(|f| panic!("{:?}", f.error));
    r.value
}

/// ネイティブの標準出力（文字列の戻り値の表示）。実行ファイルは 1 回だけ作る。
fn native_strs(src: &str, cases: &[Vec<i64>]) -> Vec<String> {
    let dir = scratch("bytes35d");
    let exe = dir.join("prog");
    try_build(&[src], "f", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let out = cases
        .iter()
        .map(|c| {
            let a: Vec<Arg> = c.iter().map(|n| Arg::Int(*n)).collect();
            let r = exec(&exe, &a);
            assert_eq!(r.code, Some(0), "{c:?}: {}", r.stderr);
            r.stdout.trim_end_matches('\n').to_string()
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// `set` の `Err` の文言（ADR 補足 F-8）。評価器とネイティブで違う文言を、それぞれ個別に固定する。
/// 添字と値が両方不正のとき、ネイティブは添字を先に調べる（`インデックスが範囲外`）。評価器は `不正な引数`。
#[test]
fn baseline_set_error_messages() {
    let src = format!(
        "{HEAD}pub fn f(i: Int, v: Int) -> String\n{TAIL}{{\n  let mut b = Bytes.new();\n  b.push(1);\n  match b.set(i, v) {{\n    Ok(u) => {{\n      return \"ok\";\n    }}\n    Err(m) => {{\n      return m;\n    }}\n  }}\n}}\n"
    );
    // (添字, 値, 評価器, ネイティブ)
    let cases: &[(i64, i64, &str, &str)] = &[
        (0, 5, "ok", "ok"),
        (1, 5, "インデックス範囲外", "インデックスが範囲外"),
        (-1, 5, "不正な引数", "インデックスが範囲外"),
        (0, 256, "不正な引数", "バイト値が 0..=255 の範囲外"),
        (0, -1, "不正な引数", "バイト値が 0..=255 の範囲外"),
        (1, 256, "不正な引数", "インデックスが範囲外"),
        (-1, -1, "不正な引数", "インデックスが範囲外"),
        (i64::MAX, i64::MIN, "不正な引数", "インデックスが範囲外"),
    ];
    let nat = native_strs(
        &src,
        &cases.iter().map(|c| vec![c.0, c.1]).collect::<Vec<_>>(),
    );
    for (c, n) in cases.iter().zip(&nat) {
        assert_eq!(n, &format!("\"{}\"", c.3), "ネイティブ {c:?}");
        let e = eval_str(&src, &[c.0, c.1]);
        assert_eq!(e, format!("\"{}\"", c.2), "評価器 {c:?}");
    }
}

/// `slice` の `Err` の文言（評価器とネイティブで違う）。
#[test]
fn baseline_slice_error_messages() {
    let src = format!(
        "{HEAD}pub fn f(s: Int, e: Int) -> String\n{TAIL}{{\n  let mut b = Bytes.new();\n  b.push(1);\n  b.push(2);\n  match b.slice(s, e) {{\n    Ok(x) => {{\n      return \"ok\";\n    }}\n    Err(m) => {{\n      return m;\n    }}\n  }}\n}}\n"
    );
    let cases: &[(i64, i64, &str, &str)] = &[
        (0, 2, "ok", "ok"),
        (1, 1, "ok", "ok"),
        (-1, 1, "範囲不正", "スライス範囲が不正"),
        (0, 3, "範囲外", "スライス範囲が不正"),
        (2, 1, "範囲不正", "スライス範囲が不正"),
        (i64::MAX, i64::MIN, "範囲不正", "スライス範囲が不正"),
    ];
    let nat = native_strs(
        &src,
        &cases.iter().map(|c| vec![c.0, c.1]).collect::<Vec<_>>(),
    );
    for (c, n) in cases.iter().zip(&nat) {
        assert_eq!(n, &format!("\"{}\"", c.3), "ネイティブ {c:?}");
        let e = eval_str(&src, &[c.0, c.1]);
        assert_eq!(e, format!("\"{}\"", c.2), "評価器 {c:?}");
    }
}

/// `to_string_utf8` の `Err` の文言は評価器とネイティブで同じ（Rust の `FromUtf8Error`）。2 つの文言の両方を含む。
#[test]
fn baseline_utf8_error_messages() {
    let src = format!(
        "{HEAD}pub fn f(n: Int, a: Int, b: Int, c: Int, d: Int) -> String\n{TAIL}{{\n  let mut l = List.new();\n  if n > 0 {{\n    l.push(a);\n  }}\n  if n > 1 {{\n    l.push(b);\n  }}\n  if n > 2 {{\n    l.push(c);\n  }}\n  if n > 3 {{\n    l.push(d);\n  }}\n  match Bytes.from_list(&l) {{\n    Ok(x) => {{\n      match x.to_string_utf8() {{\n        Ok(s) => {{\n          return \"ok\";\n        }}\n        Err(m) => {{\n          return m;\n        }}\n      }}\n    }}\n    Err(m) => {{\n      return \"list\";\n    }}\n  }}\n}}\n"
    );
    let seqs: &[&[u8]] = &[
        &[],
        &[0x61],
        &[0xC3],
        &[0xE2, 0x82],
        &[0xF0, 0x9F, 0x98],
        &[0xFF],
        &[0x80],
        &[0xC0, 0xAF],
        &[0xED, 0xA0, 0x80],
        &[0xF4, 0x90, 0x80, 0x80],
        &[0x61, 0xC3, 0x28],
        &[0x61, 0x62, 0xE2, 0x82],
    ];
    let cases: Vec<Vec<i64>> = seqs
        .iter()
        .map(|s| {
            let mut v = vec![s.len() as i64];
            for i in 0..4 {
                v.push(*s.get(i).unwrap_or(&0) as i64);
            }
            v
        })
        .collect();
    let nat = native_strs(&src, &cases);
    let mut kinds = std::collections::BTreeSet::new();
    for ((s, c), n) in seqs.iter().zip(&cases).zip(&nat) {
        let want = match String::from_utf8(s.to_vec()) {
            Ok(_) => "ok".to_string(),
            Err(e) => e.to_string(),
        };
        kinds.insert(want.split(' ').next().unwrap().to_string());
        assert_eq!(n, &format!("\"{want}\""), "ネイティブ {s:x?}");
        let e = eval_str(&src, c);
        assert!(e.contains(&want), "評価器 {s:x?}: {e}");
    }
    assert!(kinds.len() >= 3, "文言の種類が足りない: {kinds:?}");
}

// ---- 生存数（`blocks=`・`sigil_blocks=`）の完全一致 ----

/// `Bytes` を作っては捨てる繰り返し。1 回で 0 を足す（`total`）。
fn churn() -> String {
    format!(
        "{HEAD}pub fn run(n: Int) -> Int\n{TAIL}{{\n{}}}\n",
        r#"  let mut i = 0;
  let mut total = 0;
  while i < n {
    let mut a = Bytes.new();
    a.push(1);
    a.push_u32_be(7);
    let mut c = Bytes.with_capacity(64);
    c.push(2);
    let d = a.clone();
    let e = d.concat(&c);
    a.extend(&c);
    match e.slice(1, 3) {
      Ok(x) => {
        total = total + x.len() - 2;
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    match e.slice(3, 1) {
      Ok(x) => {
        total = total + 1000;
      }
      Err(m) => {}
    }
    let s = "héllo".as_bytes();
    match s.to_string_utf8() {
      Ok(t) => {
        total = total + t.len() - 6;
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    let mut l = List.new();
    l.push(1);
    l.push(255);
    match Bytes.from_list(&l) {
      Ok(x) => {
        total = total + x.len() - 2;
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    match Bytes.from_list(&l) {
      Ok(x) => {
        let mut y = x.clone();
        let r = y.set(0, 300);
        total = total + y.len() - 2;
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    let mut bl = List.new();
    bl.push(a.clone());
    bl.push(Bytes.new());
    let bl2 = bl.clone();
    total = total + bl2.len() - 2;
    i = i + 1;
  }
  return total;
"#
    )
}

fn live(src: &str, n: i64) -> (i64, i64, String) {
    let dir = scratch("bytes35d-live");
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
    (num("blocks="), num("sigil_blocks="), got.stdout)
}

/// 作って捨てる繰り返しで、`blocks=` と `sigil_blocks=` が 1000 回と 100000 回で完全に一致し、終了時に 0 になる。
/// 旧実装では `Bytes` は Rust の頭を `blocks=` に数える。3-5d-ii では `Bytes` の分が `sigil_blocks=` へ移るので、
/// 移すときはこの期待（`blocks=` が 0 でなければならない箇所）を変えずに、終了時の合計 0 と回数への非依存を保つ。
#[test]
fn baseline_bytes_blocks_are_exact_across_1000_and_100000_iterations() {
    let src = churn();
    let (b1, s1, out1) = live(&src, 1_000);
    let (b2, s2, out2) = live(&src, 100_000);
    assert!(out1.trim_end().ends_with('0'), "{out1}");
    assert!(out2.trim_end().ends_with('0'), "{out2}");
    assert_eq!(
        (b1, s1),
        (b2, s2),
        "1000 回と 100000 回で生きている数が違う"
    );
    assert_eq!((b2, s2), (0, 0), "終了時に Bytes の塊が残っている");
}

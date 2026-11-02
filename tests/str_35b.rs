//! ADR 0025 補足 D（3-5b-i）: `String` の表現を切り替えた `std.core.string` の試験。
//!
//! - 直接の試験: 評価器の上で `std.core.string` の関数を駆動モジュール（`std.core.t`）から直接呼び、Rust の
//!   `String::from_utf8_lossy`／`std::str::from_utf8` と突き合わせる（D-2 の new_lossy・valid_utf8）。
//!   同じ駆動をネイティブでも走らせる（確保器の本物の経路）。
//! - 塊の表の試験（D-5 の代わりの手当て）: 評価器の塊の表で、頭と中身の解放・二重解放・解放後の読みが止まる。
//!   ネイティブは `sigil_blocks=` の厳密一致で、作って捨てる繰り返しが増えず、残した分だけ数えられる。
//! - 3-5b-ii: `len`・`is_empty`・`trim`・`concat`・`parse_int`・`eq`・`cmp` を、`sigil::shared_text`（評価器の `String` の
//!   実体）と Rust の標準ライブラリに突き合わせる（評価器の上とネイティブの両方）。塊の表と `sigil_blocks=` の完全一致も見る。
//! - 異常: `n < 0` はどちらも止まる（ネイティブは終了コード 70）。
mod common;

use common::native::{exec_env, scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::exec::Options;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";
const MODP: i64 = 1_000_000_007;

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
        .map(|f| format!("use std.core.string.{f};\n"))
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
    let dir = scratch("str35b");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    got
}

fn live_line(r: &Ran) -> String {
    r.stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {}", r.stderr))
        .to_string()
}

const USES: &[&str] = &["new_lossy", "valid_utf8", "free", "data", "bytes"];

/// `bytes` から new_lossy で作り、中身の畳み込みと長さと valid_utf8 を 1 つの整数にする断片（変数名に `k` を添える）。
fn case_block(bytes: &[u8], k: usize) -> String {
    let n = bytes.len();
    let mut s = String::new();
    if n == 0 {
        s += &format!("  let buf{k} = raw.addr_null();\n");
    } else {
        s += &format!("  let buf{k} = raw.alloc({n}, 1);\n");
        for (i, b) in bytes.iter().enumerate() {
            s += &format!("  raw.store_u8(raw.offset(buf{k}, {i}), {b});\n");
        }
    }
    s += &format!(
        "  let s{k} = new_lossy(buf{k}, {n});\n  let n{k} = bytes(s{k});\n  let d{k} = data(s{k});\n  let mut h{k} = 0;\n  let mut i{k} = 0;\n  while i{k} < n{k} {{\n    h{k} = (h{k} * 131 + raw.load_u8(raw.offset(d{k}, i{k}))) % {MODP};\n    i{k} = i{k} + 1;\n  }}\n  let mut v{k} = 0;\n  if valid_utf8(buf{k}, {n}) {{\n    v{k} = 1;\n  }}\n  free(s{k});\n"
    );
    if n > 0 {
        s += &format!("  raw.free(buf{k}, {n}, 1);\n");
    }
    s += &format!("  let r{k} = (h{k} * 65536 + n{k}) * 2 + v{k};\n");
    s
}

/// `case_block` と同じ計算を Rust で行う。
fn expect(bytes: &[u8]) -> i64 {
    let out = String::from_utf8_lossy(bytes);
    let mut h = 0i64;
    for b in out.as_bytes() {
        h = (h * 131 + *b as i64) % MODP;
    }
    let v = std::str::from_utf8(bytes).is_ok() as i64;
    (h * 65536 + out.len() as i64) * 2 + v
}

fn cases() -> Vec<Vec<u8>> {
    let mut v: Vec<Vec<u8>> = vec![
        vec![],
        b"abc".to_vec(),
        "日本語🎉é".as_bytes().to_vec(),
        vec![0xFF],
        vec![0x80],
        vec![0xC0, 0x80],
        vec![0xC1, 0xBF],
        vec![0xE3, 0x81],
        vec![0xE3, 0x81, 0x82, 0xE3],
        vec![0xE0, 0x80, 0x80],
        vec![0xE0, 0xA0],
        vec![0xED, 0xA0, 0x80],
        vec![0xED, 0x9F, 0xBF],
        vec![0xF0, 0x80, 0x80, 0x80],
        vec![0xF0, 0x90, 0x80],
        vec![0xF0, 0x9F, 0x98],
        vec![0xF4, 0x90, 0x80, 0x80],
        vec![0xF4, 0x8F, 0xBF, 0xBF],
        vec![0xF5, 0x80, 0x80, 0x80],
        vec![b'a', 0xF0, 0x9F],
        vec![0xE2, 0x82, 0xAC, 0xE2, 0x82],
        vec![0xC2],
    ];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for _ in 0..24 {
        let mut b = vec![];
        for _ in 0..24 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            // 高いバイトと継続バイトに偏らせる。
            let t = (x >> 20) as u8;
            b.push(match x % 5 {
                0 => t % 128,
                1 => 0x80 | (t % 64),
                2 => 0xC0 | (t % 64),
                3 => 0xE0 | (t % 32),
                _ => 0xF0 | (t % 16),
            });
        }
        v.push(b);
    }
    v
}

#[test]
fn new_lossy_and_valid_utf8_match_rust_in_the_evaluator() {
    for (k, c) in cases().iter().enumerate() {
        let body = format!("{}  return r{k};", case_block(c, k));
        let got = eval_driver(USES, &body).unwrap_or_else(|e| panic!("{c:?}: 評価器が失敗: {e}"));
        assert_eq!(got, expect(c).to_string(), "{c:?}");
    }
}

#[test]
fn new_lossy_and_valid_utf8_match_rust_natively() {
    let cs = cases();
    let mut body = String::new();
    let mut want = 0i64;
    let mut acc = String::from("  let mut acc = 0;\n");
    for (k, c) in cs.iter().enumerate() {
        body += &case_block(c, k);
        acc += &format!("  acc = (acc * 1000003 + r{k}) % {MODP};\n");
        want = (want * 1000003 + expect(c)) % MODP;
    }
    body += &acc;
    body += "  return acc;";
    let got = native_driver(USES, &body);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want.to_string()
    );
    // 作った文字列はすべて解放した。
    assert_eq!(live_line(&got), "strs=0 blocks=0 sigil_blocks=0");
}

// ---- 塊の表（D-5 の代わりの手当て） ----

const LIFE: &[&str] = &["new_lossy", "free", "clone", "data", "bytes"];

#[test]
fn evaluator_block_table_stops_double_free_and_use_after_free() {
    let mk = "  let b = raw.alloc(3, 1);\n  raw.store_u8(b, 97);\n  raw.store_u8(raw.offset(b, 1), 98);\n  raw.store_u8(raw.offset(b, 2), 99);\n  let s = new_lossy(b, 3);\n";
    // 正常: 作る・複製・解放が組になる。
    let ok = format!("{mk}  let t = clone(s);\n  let r = bytes(t) + bytes(s);\n  free(t);\n  free(s);\n  raw.free(b, 3, 1);\n  return r;");
    assert_eq!(eval_driver(LIFE, &ok).unwrap(), "6");
    // 二重解放は止まる。
    let dbl = format!("{mk}  free(s);\n  free(s);\n  return 0;");
    assert!(eval_driver(LIFE, &dbl).is_err(), "二重解放が止まらない");
    // 解放後の頭の読みは止まる。
    let uaf_head = format!("{mk}  free(s);\n  return bytes(s);");
    assert!(
        eval_driver(LIFE, &uaf_head).is_err(),
        "解放後の頭の読みが止まらない"
    );
    // 解放後の中身の読みは止まる（中身の塊も返している）。
    let uaf_body = format!("{mk}  let d = data(s);\n  free(s);\n  return raw.load_u8(d);");
    assert!(
        eval_driver(LIFE, &uaf_body).is_err(),
        "解放後の中身の読みが止まらない"
    );
    // 空の文字列は中身の塊を持たない（頭だけを返す）。二重解放は止まる。
    let empty =
        "  let s = new_lossy(raw.addr_null(), 0);\n  let r = bytes(s);\n  free(s);\n  return r;";
    assert_eq!(eval_driver(LIFE, empty).unwrap(), "0");
    let empty_dbl = "  let s = new_lossy(raw.addr_null(), 0);\n  free(s);\n  free(s);\n  return 0;";
    assert!(eval_driver(LIFE, empty_dbl).is_err());
}

#[test]
fn native_blocks_are_exact_for_loops_and_leaks() {
    let lp = |n: i64| {
        format!("  let b = raw.alloc(4, 1);\n  raw.store_u8(b, 97);\n  raw.store_u8(raw.offset(b, 1), 255);\n  raw.store_u8(raw.offset(b, 2), 98);\n  raw.store_u8(raw.offset(b, 3), 99);\n  let mut i = 0;\n  let mut t = 0;\n  while i < {n} {{\n    let s = new_lossy(b, 4);\n    let c = clone(s);\n    t = t + bytes(c);\n    free(c);\n    free(s);\n    i = i + 1;\n  }}\n  raw.free(b, 4, 1);\n  return t;")
    };
    let a = native_driver(LIFE, &lp(1000));
    let b = native_driver(LIFE, &lp(100_000));
    assert_eq!(a.code, Some(0), "{}", a.stderr);
    assert_eq!(b.code, Some(0), "{}", b.stderr);
    assert_eq!(live_line(&a), "strs=0 blocks=0 sigil_blocks=0");
    assert_eq!(live_line(&a), live_line(&b));
    // 残した文字列は 2 つの塊（頭と中身）、空の文字列は頭だけを数える。
    let leak = "  let b = raw.alloc(2, 1);\n  raw.store_u8(b, 97);\n  raw.store_u8(raw.offset(b, 1), 98);\n  let s = new_lossy(b, 2);\n  raw.free(b, 2, 1);\n  return bytes(s);";
    let r = native_driver(LIFE, leak);
    assert_eq!(live_line(&r), "strs=0 blocks=0 sigil_blocks=2");
    let leak_empty = "  let s = new_lossy(raw.addr_null(), 0);\n  return bytes(s);";
    let r = native_driver(LIFE, leak_empty);
    assert_eq!(live_line(&r), "strs=0 blocks=0 sigil_blocks=1");
}

#[test]
fn native_double_free_and_negative_length_stop_with_70() {
    let dbl = "  let s = new_lossy(raw.addr_null(), 0);\n  free(s);\n  free(s);\n  return 0;";
    let r = native_driver(LIFE, dbl);
    assert_eq!(r.code, Some(70), "{}", r.stderr);
    for body in [
        "  let s = new_lossy(raw.addr_null(), -1);\n  free(s);\n  return 0;",
        "  let s = new(raw.addr_null(), -1);\n  free(s);\n  return 0;",
    ] {
        let uses: &[&str] = if body.contains("new(") {
            &["new", "free"]
        } else {
            LIFE
        };
        assert!(
            eval_driver(uses, body).is_err(),
            "評価器で止まらない: {body}"
        );
        let r = native_driver(uses, body);
        assert_eq!(r.code, Some(70), "{}", r.stderr);
    }
}

/// `raw.alloc`/`raw.free` を差し替えると確保器も文字列も結ばれないので、ネイティブの生成は内部エラーで止まる。
#[test]
fn replaced_raw_ops_make_codegen_stop_with_internal_error() {
    use sigil::effects::RawOp;
    use sigil::stdlib::Layer;
    let op = |sig: &str| RawOp {
        layer: Layer::Core,
        signature: sig.to_string(),
        effects: "state".to_string(),
        requires: "none".to_string(),
        premise: "試験用".to_string(),
    };
    let c = COpts {
        raw_ops: vec![
            op("alloc(size: Int, align: Int) -> Addr"),
            op("free(a: Addr, size: Int, align: Int)"),
        ],
        ..Default::default()
    };
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "main", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    let dir = scratch("str35b-raw");
    let exe = dir.join("a.out");
    let r = sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe);
    let _ = std::fs::remove_dir_all(&dir);
    let e = r.expect_err("結ばれていないのに生成できた");
    assert!(
        e.to_string().contains("std.core.string が結ばれていない"),
        "{e}"
    );
}

// ---- 3-5b-ii: len・is_empty・trim・concat・parse_int・eq・cmp ----

const OPS: &[&str] = &[
    "new",
    "free",
    "data",
    "bytes",
    "len",
    "is_empty",
    "trim",
    "concat",
    "parse_int",
    "eq",
    "cmp",
];

/// `s`（正しい UTF-8）から `new` で文字列 `var` を作る断片。
fn mk(var: &str, s: &str) -> String {
    let n = s.len();
    if n == 0 {
        return format!("  let {var} = new(raw.addr_null(), 0);\n");
    }
    let mut c = format!("  let {var}b = raw.alloc({n}, 1);\n");
    for (i, b) in s.bytes().enumerate() {
        c += &format!("  raw.store_u8(raw.offset({var}b, {i}), {b});\n");
    }
    c += &format!("  let {var} = new({var}b, {n});\n  raw.free({var}b, {n}, 1);\n");
    c
}

/// 文字列 `var` の中身を畳み込んだ値を `hv` に入れる断片（`k` で名前を分ける）。
fn fold(var: &str, hv: &str) -> String {
    format!("  let {hv}n = bytes({var});\n  let {hv}d = data({var});\n  let mut {hv} = 0;\n  let mut {hv}i = 0;\n  while {hv}i < {hv}n {{\n    {hv} = ({hv} * 131 + raw.load_u8(raw.offset({hv}d, {hv}i))) % {MODP};\n    {hv}i = {hv}i + 1;\n  }}\n")
}

/// 組 `(x, y)` について 7 つの操作の結果を 1 つの整数にする断片。結果は `r{k}`。
fn ops_block(x: &str, y: &str, k: usize) -> String {
    let mut s = mk(&format!("x{k}"), x);
    s += &mk(&format!("y{k}"), y);
    s += &format!("  let lx{k} = len(x{k});\n  let mut ex{k} = 0;\n  if is_empty(x{k}) {{\n    ex{k} = 1;\n  }}\n");
    s += &format!("  let t{k} = trim(x{k});\n");
    s += &fold(&format!("t{k}"), &format!("ht{k}"));
    s += &format!("  let tn{k} = bytes(t{k});\n  free(t{k});\n");
    s += &format!("  let c{k} = concat(x{k}, y{k});\n");
    s += &fold(&format!("c{k}"), &format!("hc{k}"));
    s += &format!("  let cn{k} = bytes(c{k});\n  free(c{k});\n");
    s += &format!("  let cm{k} = cmp(x{k}, y{k}) + 2;\n  let mut q{k} = 0;\n  if eq(x{k}, y{k}) {{\n    q{k} = 1;\n  }}\n");
    s += &format!("  let o{k} = raw.alloc(8, 8);\n  raw.store_i64(o{k}, 12345);\n  let ok{k} = parse_int(x{k}, o{k});\n  let pv{k} = ((raw.load_i64(o{k}) % {MODP}) + {MODP}) % {MODP};\n  raw.free(o{k}, 8, 8);\n");
    s += &format!("  free(x{k});\n  free(y{k});\n");
    s += &format!("  let mut r{k} = 0;\n");
    for comp in [
        format!("lx{k}"),
        format!("ex{k}"),
        format!("ht{k}"),
        format!("tn{k}"),
        format!("hc{k}"),
        format!("cn{k}"),
        format!("cm{k}"),
        format!("q{k}"),
        format!("ok{k}"),
        format!("pv{k}"),
    ] {
        s += &format!("  r{k} = (r{k} * 1000003 + {comp}) % {MODP};\n");
    }
    s
}

fn fold_rust(b: &[u8]) -> i64 {
    b.iter().fold(0i64, |h, &c| (h * 131 + c as i64) % MODP)
}

/// `ops_block` と同じ計算を Rust の標準ライブラリと `shared_text` で行う。
fn ops_expect(x: &str, y: &str) -> i64 {
    let t = x.trim();
    let c = format!("{x}{y}");
    let parsed = sigil::shared_text::parse_int(x);
    let ok = parsed.is_some() as i64;
    let pv = (parsed.unwrap_or(12345) % MODP + MODP) % MODP;
    let comps = [
        sigil::shared_text::char_len(x),
        x.is_empty() as i64,
        fold_rust(t.as_bytes()),
        t.len() as i64,
        fold_rust(c.as_bytes()),
        c.len() as i64,
        x.as_bytes().cmp(y.as_bytes()) as i64 + 2,
        (x == y) as i64,
        ok,
        pv,
    ];
    comps.iter().fold(0i64, |r, &v| (r * 1000003 + v) % MODP)
}

/// 空白の符号点と、その前後の符号点を含む文字列の組。
fn op_strings() -> Vec<String> {
    let mut v: Vec<String> = [
        "",
        " ",
        "abc",
        "  a b\t\n",
        "héllo",
        "日本語",
        "🎉x🎉",
        "a\0b",
        "\u{0}",
        "ab",
        "abc ",
        "abd",
        "b",
        "Z",
        "é",
        "e",
        "\u{10FFFF}",
        "\u{FFFD}",
        "\u{7F}",
        "\u{80}",
        "\u{7FF}",
        "\u{800}",
        "\u{FFFF}",
        "\u{10000}",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // White_Space の各符号点（Rust の char::is_whitespace と同じ表）と、その前後の符号点。
    let mut cps: Vec<u32> = vec![];
    for c in [
        0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x1F, 0x20, 0x21, 0x84, 0x85, 0x86, 0x9F, 0xA0,
        0xA1, 0x167F, 0x1680, 0x1681, 0x180E, 0x1FFF, 0x2000, 0x2005, 0x200A, 0x200B, 0x2027,
        0x2028, 0x2029, 0x202A, 0x202E, 0x202F, 0x2030, 0x205E, 0x205F, 0x2060, 0x2FFF, 0x3000,
        0x3001, 0xFEFF,
    ] {
        cps.push(c);
    }
    for c in cps {
        let ch = char::from_u32(c).unwrap();
        v.push(format!("{ch}"));
        v.push(format!("{ch}x{ch}"));
    }
    v.push("  \u{3000}\u{2028} ".to_string());
    v.push("\u{85}\u{a0}".to_string());
    v
}

fn parse_strings() -> Vec<String> {
    let mut v: Vec<String> = [
        "0",
        "42",
        "-7",
        "+1",
        " 1",
        "1 ",
        "1_0",
        "",
        "-",
        "--1",
        "-0",
        "007",
        "-007",
        "1.5",
        "0x10",
        "1e3",
        "9223372036854775806",
        "9223372036854775807",
        "9223372036854775808",
        "9223372036854775810",
        "-9223372036854775807",
        "-9223372036854775808",
        "-9223372036854775809",
        "-9223372036854775810",
        "922337203685477580",
        "92233720368547758",
        "-922337203685477580",
        "18446744073709551616",
        "99999999999999999999",
        "00000000000000000000000000000000000000001",
        "abc",
        "1a",
        "a1",
        "１２",
        "-９",
        "\u{0}",
        "-\u{0}",
        "٣",
        "1\n",
        "\n1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut x: u64 = 0xD1B5_4A32_D192_ED03;
    for _ in 0..24 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        let len = 1 + (x % 20) as usize;
        let mut s = String::new();
        if x >> 40 & 3 == 0 {
            s.push('-');
        }
        let mut y = x;
        for _ in 0..len {
            y = y
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            s.push(char::from(b'0' + ((y >> 33) % 10) as u8));
        }
        v.push(s);
    }
    v
}

fn pairs() -> Vec<(String, String)> {
    let a = op_strings();
    let b = parse_strings();
    let mut all: Vec<String> = a.into_iter().chain(b).collect();
    all.dedup();
    let n = all.len();
    (0..n)
        .map(|i| (all[i].clone(), all[(i * 7 + 3) % n].clone()))
        .chain((0..n).step_by(5).map(|i| (all[i].clone(), all[i].clone())))
        .collect()
}

#[test]
fn string_ops_match_rust_in_the_evaluator() {
    // 評価器は 1 件ずつ走らせるので、間引いた件数にする（全件はネイティブで走らせる）。
    for (k, (x, y)) in pairs().iter().enumerate().filter(|(k, _)| k % 2 == 0) {
        let body = format!("{}  return r{k};", ops_block(x, y, k));
        let got =
            eval_driver(OPS, &body).unwrap_or_else(|e| panic!("{x:?} {y:?}: 評価器が失敗: {e}"));
        assert_eq!(got, ops_expect(x, y).to_string(), "{x:?} {y:?}");
    }
}

#[test]
fn string_ops_match_rust_natively() {
    let ps = pairs();
    for (ci, chunk) in ps.chunks(5).enumerate() {
        let mut body = String::new();
        let mut acc = String::from("  let mut acc = 0;\n");
        let mut want = 0i64;
        for (j, (x, y)) in chunk.iter().enumerate() {
            let k = ci * 1000 + j;
            body += &ops_block(x, y, k);
            acc += &format!("  acc = (acc * 1000003 + r{k}) % {MODP};\n");
            want = (want * 1000003 + ops_expect(x, y)) % MODP;
        }
        body += &acc;
        body += "  return acc;";
        let got = native_driver(OPS, &body);
        assert_eq!(got.code, Some(0), "{}", got.stderr);
        assert_eq!(
            got.stdout.trim_end().rsplit('\n').next().unwrap(),
            want.to_string(),
            "chunk {ci}"
        );
        // 作った文字列はすべて解放した（頭と中身。trim・concat の結果を含む）。
        assert_eq!(
            live_line(&got),
            "strs=0 blocks=0 sigil_blocks=0",
            "chunk {ci}"
        );
    }
}

/// 評価器の塊の表: `trim`・`concat` の結果は解放でき、二重解放と解放後の読みは止まる。
#[test]
fn evaluator_block_table_covers_trim_and_concat() {
    let mk2 = format!("{}{}", mk("a", " ab "), mk("b", "c"));
    let ok = format!("{mk2}  let t = trim(a);\n  let c = concat(t, b);\n  let r = bytes(c) * 10 + len(t);\n  free(c);\n  free(t);\n  free(a);\n  free(b);\n  return r;");
    assert_eq!(eval_driver(OPS, &ok).unwrap(), "32");
    let dbl_t = format!("{mk2}  let t = trim(a);\n  free(t);\n  free(t);\n  return 0;");
    assert!(
        eval_driver(OPS, &dbl_t).is_err(),
        "trim の結果の二重解放が止まらない"
    );
    let dbl_c = format!("{mk2}  let c = concat(a, b);\n  free(c);\n  free(c);\n  return 0;");
    assert!(
        eval_driver(OPS, &dbl_c).is_err(),
        "concat の結果の二重解放が止まらない"
    );
    let uaf = format!(
        "{mk2}  let c = concat(a, b);\n  let d = data(c);\n  free(c);\n  return raw.load_u8(d);"
    );
    assert!(
        eval_driver(OPS, &uaf).is_err(),
        "concat の結果の解放後の読みが止まらない"
    );
    // 入力は変えない（trim・concat は新しい文字列を返す）。
    let keep = format!("{mk2}  let t = trim(a);\n  let c = concat(a, b);\n  let r = bytes(a) * 100 + bytes(t) * 10 + bytes(c);\n  free(t);\n  free(c);\n  free(a);\n  free(b);\n  return r;");
    assert_eq!(eval_driver(OPS, &keep).unwrap(), "425");
    // 空の文字列どうしの連結・空白だけの trim は、頭だけの文字列を返す。
    let empty = "  let a = new(raw.addr_null(), 0);\n  let c = concat(a, a);\n  let t = trim(c);\n  let r = bytes(c) + bytes(t);\n  free(t);\n  free(c);\n  free(a);\n  return r;";
    assert_eq!(eval_driver(OPS, empty).unwrap(), "0");
}

/// ネイティブの `sigil_blocks=` の完全一致: trim・concat を繰り返しても増えず、残した分だけ数える。
#[test]
fn native_blocks_are_exact_for_trim_and_concat() {
    let lp = |n: i64| {
        format!("{}{}  let mut i = 0;\n  let mut t = 0;\n  while i < {n} {{\n    let u = trim(a);\n    let c = concat(u, b);\n    t = t + len(c);\n    free(c);\n    free(u);\n    i = i + 1;\n  }}\n  free(a);\n  free(b);\n  return t;", mk("a", " ab "), mk("b", "é"))
    };
    let a = native_driver(OPS, &lp(1000));
    let b = native_driver(OPS, &lp(50_000));
    assert_eq!(a.code, Some(0), "{}", a.stderr);
    assert_eq!(b.code, Some(0), "{}", b.stderr);
    assert_eq!(live_line(&a), "strs=0 blocks=0 sigil_blocks=0");
    assert_eq!(live_line(&a), live_line(&b));
    // 残した trim の結果は 2 つ、concat の結果は 2 つの塊（頭と中身）。空になる結果は頭だけ。
    let leak_t = format!(
        "{}  let t = trim(a);\n  free(a);\n  return bytes(t);",
        mk("a", " ab ")
    );
    assert_eq!(
        live_line(&native_driver(OPS, &leak_t)),
        "strs=0 blocks=0 sigil_blocks=2"
    );
    let leak_c = format!(
        "{}{}  let c = concat(a, b);\n  free(a);\n  free(b);\n  return bytes(c);",
        mk("a", "x"),
        mk("b", "y")
    );
    assert_eq!(
        live_line(&native_driver(OPS, &leak_c)),
        "strs=0 blocks=0 sigil_blocks=2"
    );
    let leak_e = format!(
        "{}  let t = trim(a);\n  free(a);\n  return bytes(t);",
        mk("a", " \u{3000} ")
    );
    assert_eq!(
        live_line(&native_driver(OPS, &leak_e)),
        "strs=0 blocks=0 sigil_blocks=1"
    );
}

//! ADR 0025 補足 F の 3-5d-ii（`Bytes` の表現を Sigil の `std.core.bytes` へ切り替える）。
//!
//! 駆動モジュール（`std.core.t`）から `std.core.bytes` の関数を、評価器とネイティブで直接呼ぶ。
//! - 評価器の塊の表: 空と `create(0)` は頭だけ、成長で古い領域が返る、二重解放・解放後の使用・`cap` を書き換えた頭・
//!   `from_raw` の `n < 0` は止まる。
//! - 戻り値の規則: `get`/`read_u32_be` の範囲外は 0、`slice` の範囲外は空番地、`set` は添字を先に調べる（0/1/2）。
//! - ネイティブは Rust の `Vec<u8>` の模型と突き合わせ、終了時に `sigil_blocks=0` を保つ。

mod common;
use common::native::{exec_env, scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::exec::Options;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";
const MODP: i64 = 1_000_000_007;
const BU: &[&str] = &[
    "create",
    "free",
    "from_raw",
    "clone",
    "data",
    "len",
    "is_empty",
    "get",
    "set",
    "push",
    "push_u32_be",
    "read_u32_be",
    "slice",
    "concat",
    "extend",
    "ct_equal",
    "cmp",
    "eq",
];

fn driver(body: &str) -> String {
    let called = |f: &str| {
        let pat = format!("{f}(");
        body.match_indices(&pat).any(|(i, _)| {
            !body[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
        })
    };
    let mut sorted: Vec<&str> = BU.iter().copied().filter(|f| called(f)).collect();
    sorted.sort_unstable();
    let u: String = sorted
        .iter()
        .map(|f| format!("use std.core.bytes.{f};\n"))
        .collect();
    format!(
        "module std.core.t\n  targets server\n  trust base\n\n{u}\npub fn run() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n"
    )
}

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n")
}

fn copts(body: &str) -> COpts {
    COpts {
        embedded_extra: vec![("std.core.t".to_string(), driver(body))],
        embedded_roots: vec!["std.core.t".into()],
        ..Default::default()
    }
}

fn eval(body: &str) -> Result<String, String> {
    let c = copts(body);
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

fn native(body: &str) -> Ran {
    let c = copts(body);
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.core.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("bytes35dii-drv");
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

/// `n` 個の `push` を並べる（値は `(i * 37 + 5) % 256`）。
fn pushes(v: &str, n: usize) -> String {
    (0..n)
        .map(|i| format!("  push({v}, {});\n", (i * 37 + 5) % 256))
        .collect()
}

#[test]
fn evaluator_return_conventions() {
    // get: 範囲内は 1 と値、範囲外は 0 で out を変えない。
    let b = format!(
        "  let b = create(0);\n{}  let o = raw.alloc(8, 8);\n  raw.store_i64(o, 99);\n  let r0 = get(b, 2, o);\n  let v0 = raw.load_i64(o);\n  let r1 = get(b, 3, o);\n  let r2 = get(b, -1, o);\n  let v1 = raw.load_i64(o);\n  raw.free(o, 8, 8);\n  free(b);\n  return r0 * 100000 + v0 * 100 + r1 * 10 + r2 + v1 * 1000000;",
        pushes("b", 3)
    );
    // r0=1, v0=(2*37+5)%256=79, r1=0, r2=0, v1=79
    assert_eq!(
        eval(&b).unwrap(),
        (100000 + 7900 + 79 * 1_000_000).to_string()
    );
    // set: 添字を先に調べる（1）、次に値（2）、成功は 0。
    let s = "  let b = create(2);\n  push(b, 1);\n  let a = set(b, 9, 300);\n  let c = set(b, 0, 300);\n  let d = set(b, 0, -1);\n  let e = set(b, 0, 255);\n  let f = set(b, -1, 5);\n  free(b);\n  return a * 10000 + c * 1000 + d * 100 + e * 10 + f;";
    assert_eq!(eval(s).unwrap(), "12201");
    // read_u32_be: off <= len - 4 だけ読める。範囲外は 0。
    let r = "  let b = create(0);\n  push_u32_be(b, 305419896);\n  push(b, 9);\n  let o = raw.alloc(8, 8);\n  raw.store_i64(o, 5);\n  let a = read_u32_be(b, 0, o);\n  let v = raw.load_i64(o);\n  let c = read_u32_be(b, 1, o);\n  let d = read_u32_be(b, 2, o);\n  let e = read_u32_be(b, -1, o);\n  raw.free(o, 8, 8);\n  free(b);\n  return a * 10000 + c * 1000 + d * 100 + e * 10 + v % 7;";
    // a=1, c=1, d=0, e=0, v=0x12345678 % 7
    let v = 0x1234_5678i64 % 7;
    assert_eq!(eval(r).unwrap(), (11000 + v).to_string());
    // slice: 範囲外は空番地、範囲内の空は本物の頭。
    let sl = "  let b = create(0);\n  push(b, 1);\n  push(b, 2);\n  push(b, 3);\n  let bad1 = slice(b, 2, 1);\n  let bad2 = slice(b, 0, 4);\n  let bad3 = slice(b, -1, 2);\n  let z = slice(b, 3, 3);\n  let ok = slice(b, 1, 3);\n  let r = raw.addr_to_int(bad1) + raw.addr_to_int(bad2) + raw.addr_to_int(bad3);\n  let nz = raw.addr_to_int(z);\n  let l = len(ok);\n  free(z);\n  free(ok);\n  free(b);\n  if nz == 0 {\n    return 1000;\n  }\n  return r * 100 + l;";
    assert_eq!(eval(sl).unwrap(), "2");
    // from_raw(_, 0) は p を読まない。
    let fr = "  let b = from_raw(raw.addr_null(), 0);\n  let r = len(b);\n  free(b);\n  return r;";
    assert_eq!(eval(fr).unwrap(), "0");
}

#[test]
fn evaluator_ct_equal_cmp_eq() {
    let mk = |s: &[u8], v: &str| -> String {
        let mut o = format!("  let {v} = create(0);\n");
        for x in s {
            o += &format!("  push({v}, {x});\n");
        }
        o
    };
    let pairs: &[(&[u8], &[u8])] = &[
        (b"", b""),
        (b"abc", b"abc"),
        (b"abc", b"xbc"),
        (b"abc", b"abx"),
        (b"abc", b"abcd"),
        (b"abcd", b"abc"),
        (b"", b"a"),
        (&[0], &[0, 0]),
        (&[255, 0], &[255, 1]),
    ];
    for (a, b) in pairs {
        let body = format!(
            "{}{}  let ce = ct_equal(x, y);\n  let c = cmp(x, y);\n  let e = eq(x, y);\n  free(x);\n  free(y);\n  let mut r = c + 1;\n  if ce {{\n    r = r + 10;\n  }}\n  if e {{\n    r = r + 100;\n  }}\n  return r;",
            mk(a, "x"),
            mk(b, "y")
        );
        let want = (a.cmp(b) as i64 + 1) + 10 * (a == b) as i64 + 100 * (a == b) as i64;
        assert_eq!(eval(&body).unwrap(), want.to_string(), "{a:?} {b:?}");
    }
}

#[test]
fn evaluator_growth_returns_the_old_region() {
    // 最初の push で cap = max(0, 1, 8) = 8。8 個目までは同じ領域、9 個目で 16 に取り直す。
    let same = format!(
        "  let b = create(0);\n{}  let a = data(b);\n  push(b, 1);\n  let r = raw.load_u8(a);\n  free(b);\n  return r;",
        pushes("b", 7)
    );
    assert_eq!(eval(&same).unwrap(), "5");
    let grow = format!(
        "  let b = create(0);\n{}  let a = data(b);\n  push(b, 1);\n  push(b, 1);\n  let r = raw.load_u8(a);\n  free(b);\n  return r;",
        pushes("b", 7)
    );
    assert!(eval(&grow).is_err(), "取り直した古い領域が返っていない");
    // create(c) の領域は c バイト。c 個の push では取り直さない。
    let c = format!(
        "  let b = create(20);\n  let a = data(b);\n{}  let r = raw.load_u8(a);\n  free(b);\n  return r;",
        pushes("b", 20)
    );
    assert_eq!(eval(&c).unwrap(), "5");
    let c2 = format!(
        "  let b = create(20);\n  let a = data(b);\n{}  push(b, 1);\n  let r = raw.load_u8(a);\n  free(b);\n  return r;",
        pushes("b", 20)
    );
    assert!(eval(&c2).is_err());
    // 範囲外の読み（cap の外）は止まる。
    let oob = "  let b = create(4);\n  let a = data(b);\n  let r = raw.load_u8(raw.offset(a, 4));\n  free(b);\n  return r;";
    assert!(eval(oob).is_err());
}

#[test]
fn evaluator_block_table_checks_for_bytes() {
    // 空と create(0) と create(-3) は頭だけ（領域なし）。解放も通る。
    let heads = "  let a = create(0);\n  let b = create(-3);\n  let c = from_raw(raw.addr_null(), 0);\n  let r = len(a) + len(b) + len(c);\n  free(a);\n  free(b);\n  free(c);\n  free(raw.addr_null());\n  return r;";
    assert_eq!(eval(heads).unwrap(), "0");
    // 二重解放（領域あり・なし）。
    for mk in ["create(0)", "create(8)"] {
        let b = format!("  let b = {mk};\n  free(b);\n  free(b);\n  return 0;");
        assert!(eval(&b).is_err(), "二重解放が止まらない: {mk}");
    }
    // 解放後の使用。
    for after in [
        "return len(b);",
        "push(b, 1);\n  return 0;",
        "let c = clone(b);\n  free(c);\n  return 0;",
    ] {
        let b = format!("  let b = create(8);\n  push(b, 1);\n  free(b);\n  {after}");
        assert!(eval(&b).is_err(), "解放後が止まらない: {after}");
    }
    // cap を書き換えた頭の解放。
    let badcap = "  let b = create(8);\n  push(b, 1);\n  raw.store_i64(raw.offset(b, 16), 3);\n  free(b);\n  return 0;";
    assert!(eval(badcap).is_err(), "cap の食い違いが止まらない");
    // from_raw の n < 0。
    let neg = "  let p = raw.alloc(8, 1);\n  let b = from_raw(p, -1);\n  free(b);\n  raw.free(p, 8, 1);\n  return 0;";
    assert!(eval(neg).is_err());
    // from_raw は写す（元を解放しても有効）。
    let cp = "  let p = raw.alloc(4, 1);\n  raw.store_u8(p, 7);\n  raw.store_u8(raw.offset(p, 1), 8);\n  raw.store_u8(raw.offset(p, 2), 9);\n  raw.store_u8(raw.offset(p, 3), 10);\n  let b = from_raw(p, 4);\n  raw.free(p, 4, 1);\n  let r = raw.load_u8(raw.offset(data(b), 3));\n  free(b);\n  return r;";
    assert_eq!(eval(cp).unwrap(), "10");
}

/// ネイティブ: 列の作成・複製・連結・切り出し・拡張を Rust の模型と突き合わせ、最後に塊が残らない。
#[test]
fn native_ops_match_a_vec_model_and_leave_no_blocks() {
    let sizes = [0usize, 1, 7, 8, 9, 16, 17, 40];
    let mut body = String::from("  let mut acc = 0;\n");
    let mut want = 0i64;
    for (k, &n) in sizes.iter().enumerate() {
        let v: Vec<i64> = (0..n).map(|i| ((i * 37 + 5) % 256) as i64).collect();
        body += &format!("  let b{k} = create(0);\n{}", pushes(&format!("b{k}"), n));
        body += &format!(
            "  let c{k} = clone(b{k});\n  let d{k} = concat(b{k}, c{k});\n  extend(c{k}, b{k});\n  let s{k} = slice(d{k}, {}, {});\n  let mut h{k} = len(b{k}) * 1000 + len(c{k}) * 100 + len(d{k}) * 10 + len(s{k});\n  let o{k} = raw.alloc(8, 8);\n  let mut i{k} = 0;\n  while i{k} < len(d{k}) {{\n    let g{k} = get(d{k}, i{k}, o{k});\n    h{k} = (h{k} * 131 + raw.load_i64(o{k}) + g{k}) % {MODP};\n    i{k} = i{k} + 1;\n  }}\n  raw.free(o{k}, 8, 8);\n",
            n / 3,
            n
        );
        body += &format!(
            "  if ct_equal(c{k}, d{k}) {{\n    h{k} = h{k} + 1;\n  }}\n  if eq(b{k}, c{k}) {{\n    h{k} = h{k} + 2;\n  }}\n  h{k} = h{k} + cmp(b{k}, d{k}) + 5;\n  free(b{k});\n  free(c{k});\n  free(d{k});\n  free(s{k});\n  acc = (acc * 1000003 + h{k}) % {MODP};\n"
        );
        // 模型
        let mut d = v.clone();
        d.extend(&v);
        let c = d.clone();
        let s = d[n / 3..n].to_vec();
        let mut h = (v.len() * 1000 + c.len() * 100 + d.len() * 10 + s.len()) as i64;
        for x in &d {
            h = (h * 131 + x + 1) % MODP;
        }
        if c == d {
            h += 1;
        }
        // c = b + b、b == c は n == 0 のときだけ真。
        if v == c {
            h += 2;
        }
        h += (v.cmp(&d) as i64) + 5;
        want = (want * 1000003 + h) % MODP;
    }
    body += "  return acc;";
    let got = native(&body);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want.to_string()
    );
    assert_eq!(live_line(&got), "sigil_blocks=0");
    assert_eq!(eval(&body).unwrap(), want.to_string());
}

#[test]
fn native_bytes_stops_with_70() {
    for body in [
        "  let b = create(8);\n  free(b);\n  free(b);\n  return 0;",
        "  let b = create(0);\n  free(b);\n  free(b);\n  return 0;",
        "  let b = create(8);\n  raw.store_i64(raw.offset(b, 16), 3);\n  free(b);\n  return 0;",
        "  let p = raw.alloc(8, 1);\n  let b = from_raw(p, -1);\n  free(b);\n  return 0;",
    ] {
        let r = native(body);
        assert_eq!(r.code, Some(70), "{body}\n{}", r.stderr);
    }
}

//! ADR 0025 補足 F の 3-5d-iii（残りの `Bytes` の変換を Rust のランタイムから外す）。
//!
//! - `Bytes.to_string_utf8` は Sigil の `std.core.string.from_utf8(p, n, info)` で検査・構築し、失敗の文言は
//!   `valid_up_to` と `error_len` から codegen が組み立てる（Rust の `Utf8Error` の `Display` と同じ。F-5）。
//! - `Bytes.from_list` は codegen のループ、`str.as_bytes` は `str_data`/`str_bytes` と `bytes_from_raw`。
//! - 不正な UTF-8 は、評価器・ネイティブ・Rust の `std::str::from_utf8` の三者で突き合わせる。
//! - `sigil_rt_bytes_to_string_utf8`・`sigil_rt_str_as_bytes`・`sigil_rt_bytes_from_list` は Rust から消えている。

mod common;
use common::native::{exec_env, scratch, try_build, Ran};
use sigil::effects::Options as COpts;
use sigil::exec::Options;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";
const HEAD: &str = "module m.u8\n  targets server\n  trust none\n\n";
const MODP: i64 = 1_000_000_007;

/// 先頭バイトの候補（1 バイト・継続・過長・サロゲート・範囲外の境界）。
const LEADS: [i64; 20] = [
    0x41, 0x7F, 0x80, 0xBF, 0xC0, 0xC1, 0xC2, 0xDF, 0xE0, 0xE1, 0xEC, 0xED, 0xEE, 0xEF, 0xF0, 0xF1,
    0xF3, 0xF4, 0xF5, 0xFF,
];
/// 後続バイトの候補。
const CONTS: [i64; 6] = [0x7F, 0x80, 0x8F, 0x90, 0xA0, 0xBF];

/// `lead(i)`/`cont(i)` の Sigil 定義（`if` の並び）。
fn table_fn(name: &str, vals: &[i64]) -> String {
    let mut s = format!("fn {name}(i: Int) -> Int\n{TAIL}{{\n");
    for (k, v) in vals.iter().enumerate() {
        s += &format!("  if i == {k} {{\n    return {v};\n  }}\n");
    }
    s += "  return 0;\n}\n\n";
    s
}

/// 全ての組を並べた列（前置 `a` の有無 × 先頭 × 後続 0〜3 個）。Rust 側の模型と Sigil 側の `while` が同じ順で回る。
fn cases() -> Vec<Vec<u8>> {
    let mut out = vec![];
    for l in LEADS {
        for pre in 0..2 {
            for k in 0..4u32 {
                for t in 0..(6i64.pow(k)) {
                    let mut v = vec![];
                    if pre == 1 {
                        v.push(b'a');
                    }
                    v.push(l as u8);
                    let mut tt = t;
                    for _ in 0..k {
                        v.push(CONTS[(tt % 6) as usize] as u8);
                        tt /= 6;
                    }
                    out.push(v);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 駆動モジュール（`std.core.t`）から `std.core.string.from_utf8` を直接呼ぶ
// ---------------------------------------------------------------------------

fn driver(body: &str) -> String {
    format!(
        "module std.core.t\n  targets server\n  trust base\n\nuse std.core.string.free;\nuse std.core.string.from_utf8;\n\n{}{}pub fn run() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n",
        table_fn("lead", &LEADS),
        table_fn("cont", &CONTS),
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
    let dir = scratch("bytes35diii-drv");
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

/// `from_utf8` の結果を数にする: 成功は 0、失敗は `1 + valid_up_to * 10 + error_len * 1000`（末尾で切れたら 0）。
fn code_of(v: &[u8]) -> i64 {
    match std::str::from_utf8(v) {
        Ok(_) => 0,
        Err(e) => 1 + e.valid_up_to() as i64 * 10 + e.error_len().unwrap_or(0) as i64 * 1000,
    }
}

/// 全ての組を `from_utf8` に通して畳む本体。
fn grid_body() -> String {
    format!(
        "  let p = raw.alloc(8, 1);\n  let info = raw.alloc(16, 8);\n  let mut acc = 0;\n  let mut li = 0;\n  while li < 20 {{\n    let l = lead(li);\n    let mut pre = 0;\n    while pre < 2 {{\n      let mut k = 0;\n      let mut cnt = 1;\n      while k < 4 {{\n        let mut t = 0;\n        while t < cnt {{\n          let mut pos = 0;\n          if pre == 1 {{\n            raw.store_u8(p, 97);\n            pos = 1;\n          }}\n          raw.store_u8(raw.offset(p, pos), l);\n          pos = pos + 1;\n          let mut tt = t;\n          let mut j = 0;\n          while j < k {{\n            raw.store_u8(raw.offset(p, pos), cont(tt % 6));\n            tt = tt / 6;\n            pos = pos + 1;\n            j = j + 1;\n          }}\n          raw.store_i64(info, 0 - 1);\n          raw.store_i64(raw.offset(info, 8), 0 - 1);\n          let s = from_utf8(p, pos, info);\n          let mut r = 0;\n          if raw.addr_to_int(s) == 0 {{\n            r = 1 + raw.load_i64(info) * 10 + raw.load_i64(raw.offset(info, 8)) * 1000;\n          }} else {{\n            free(s);\n          }}\n          acc = (acc * 1000003 + r) % {MODP};\n          t = t + 1;\n        }}\n        k = k + 1;\n        cnt = cnt * 6;\n      }}\n      pre = pre + 1;\n    }}\n    li = li + 1;\n  }}\n  raw.free(p, 8, 1);\n  raw.free(info, 16, 8);\n  return acc;"
    )
}

fn grid_want() -> i64 {
    let mut acc = 0i64;
    for c in cases() {
        acc = (acc * 1000003 + code_of(&c)) % MODP;
    }
    acc
}

/// 評価器の `from_utf8` が、Rust の `std::str::from_utf8` の `valid_up_to`/`error_len` と全ての組で一致する。
#[test]
fn from_utf8_evaluator_matches_rust() {
    assert_eq!(eval(&grid_body()).unwrap(), grid_want().to_string());
}

/// ネイティブも同じ。終了時に塊が残らない。
#[test]
fn from_utf8_native_matches_rust_and_leaves_no_blocks() {
    let got = native(&grid_body());
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        grid_want().to_string()
    );
    assert_eq!(live_line(&got), "sigil_blocks=0");
}

/// `n == 0` は `p` を読まない（空番地でよい）。成功は文字列の頭を返し、`info` を書き換えない。
#[test]
fn from_utf8_empty_does_not_read_p() {
    let b = "  let info = raw.alloc(16, 8);\n  raw.store_i64(info, 77);\n  raw.store_i64(raw.offset(info, 8), 78);\n  let s = from_utf8(raw.addr_null(), 0, info);\n  let z = raw.addr_to_int(s);\n  let r = raw.load_i64(info) * 1000 + raw.load_i64(raw.offset(info, 8));\n  free(s);\n  raw.free(info, 16, 8);\n  if z == 0 {\n    return 0 - 1;\n  }\n  return r;";
    assert_eq!(eval(b).unwrap(), "77078");
    let got = native(b);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout.trim_end().rsplit('\n').next().unwrap(), "77078");
    assert_eq!(live_line(&got), "sigil_blocks=0");
}

// ---------------------------------------------------------------------------
// 利用者のプログラム: `Bytes.to_string_utf8` の文言・`Bytes.from_list`・`as_bytes`
// ---------------------------------------------------------------------------

const HB: &str = "fn hb(b: &Bytes) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut h = b.len();\n  let mut i = 0;\n  while i < b.len() {\n    h = (h * 257 + b.get(i).unwrap_or(0)) % 1000000007;\n    i = i + 1;\n  }\n  return h;\n}\n\n";

fn hb(b: &[u8]) -> i64 {
    let mut h = b.len() as i64;
    for &x in b {
        h = (h * 257 + x as i64) % MODP;
    }
    h
}

fn run0(src: &str) -> String {
    common::run(src, "f", &[]).unwrap().value
}

/// 全ての組を `Bytes` に積み、`to_string_utf8` の結果（成功なら中身、失敗なら文言）の畳みを取る。
/// 文言は Rust の `Utf8Error` の `to_string()` と同じでなければならない（評価器・ネイティブ・Rust の三者）。
#[test]
fn to_string_utf8_messages_match_rust_in_evaluator_and_native() {
    let body = format!(
        "  let mut acc = 0;\n  let mut li = 0;\n  while li < 20 {{\n    let l = lead(li);\n    let mut pre = 0;\n    while pre < 2 {{\n      let mut k = 0;\n      let mut cnt = 1;\n      while k < 4 {{\n        let mut t = 0;\n        while t < cnt {{\n          let mut b = Bytes.new();\n          if pre == 1 {{\n            b.push(97);\n          }}\n          b.push(l);\n          let mut tt = t;\n          let mut j = 0;\n          while j < k {{\n            b.push(cont(tt % 6));\n            tt = tt / 6;\n            j = j + 1;\n          }}\n          let mut c = 0;\n          match b.to_string_utf8() {{\n            Ok(s) => {{\n              let y = s.as_bytes();\n              c = hb(&y) * 2 + 1;\n            }}\n            Err(m) => {{\n              let y = m.as_bytes();\n              c = hb(&y) * 2;\n            }}\n          }}\n          acc = (acc * 1000003 + c) % {MODP};\n          t = t + 1;\n        }}\n        k = k + 1;\n        cnt = cnt * 6;\n      }}\n      pre = pre + 1;\n    }}\n    li = li + 1;\n  }}\n  return acc;"
    );
    let src = format!(
        "{HEAD}{HB}{}{}pub fn f() -> Int\n{TAIL}{{\n{body}\n}}\n",
        table_fn("lead", &LEADS),
        table_fn("cont", &CONTS)
    );
    let mut want = 0i64;
    let mut kinds = std::collections::BTreeSet::new();
    for c in cases() {
        let code = match std::str::from_utf8(&c) {
            Ok(s) => hb(s.as_bytes()) * 2 + 1,
            Err(e) => {
                let m = e.to_string();
                kinds.insert(m.split(' ').next().unwrap().to_string());
                hb(m.as_bytes()) * 2
            }
        };
        want = (want * 1000003 + code) % MODP;
    }
    assert_eq!(
        kinds.len(),
        2,
        "文言の種類（invalid / incomplete）: {kinds:?}"
    );
    // `common::run` は評価器とネイティブの両方を走らせて、一致を確かめる。
    assert_eq!(run0(&src), want.to_string());
}

/// `Bytes.from_list`: 範囲内・範囲外（負・256・端）の全ての組（長さ 0〜3）。失敗しても Err の文言が同じ。
#[test]
fn from_list_matches_model_in_evaluator_and_native() {
    const VALS: [&str; 7] = [
        "0 - 1",
        "0",
        "1",
        "255",
        "256",
        "9223372036854775807",
        "0 - 9223372036854775807 - 1",
    ];
    const VNUM: [i64; 7] = [-1, 0, 1, 255, 256, i64::MAX, i64::MIN];
    let mut tbl = String::from("fn val(i: Int) -> Int\n");
    tbl += TAIL;
    tbl += "{\n";
    for (k, v) in VALS.iter().enumerate() {
        tbl += &format!("  if i == {k} {{\n    return {v};\n  }}\n");
    }
    tbl += "  return 0;\n}\n\n";
    let body = format!(
        "  let mut acc = 0;\n  let mut k = 0;\n  let mut cnt = 1;\n  while k < 4 {{\n    let mut t = 0;\n    while t < cnt {{\n      let mut l = List.new();\n      let mut tt = t;\n      let mut j = 0;\n      while j < k {{\n        l.push(val(tt % 7));\n        tt = tt / 7;\n        j = j + 1;\n      }}\n      let mut c = 0;\n      match Bytes.from_list(&l) {{\n        Ok(x) => {{\n          c = hb(&x) * 2 + 1;\n        }}\n        Err(m) => {{\n          let y = m.as_bytes();\n          c = hb(&y) * 2;\n        }}\n      }}\n      acc = (acc * 1000003 + c) % {MODP};\n      t = t + 1;\n    }}\n    k = k + 1;\n    cnt = cnt * 7;\n  }}\n  return acc;"
    );
    let src = format!("{HEAD}{HB}{tbl}pub fn f() -> Int\n{TAIL}{{\n{body}\n}}\n");
    let mut want = 0i64;
    let mut cnt = 1i64;
    for k in 0..4u32 {
        for t in 0..cnt {
            let mut tt = t;
            let mut xs = vec![];
            for _ in 0..k {
                xs.push(VNUM[(tt % 7) as usize]);
                tt /= 7;
            }
            let code = if xs.iter().all(|v| (0..=255).contains(v)) {
                let bs: Vec<u8> = xs.iter().map(|v| *v as u8).collect();
                hb(&bs) * 2 + 1
            } else {
                hb("0..=255 の範囲外".as_bytes()) * 2
            };
            want = (want * 1000003 + code) % MODP;
        }
        cnt *= 7;
    }
    assert_eq!(run0(&src), want.to_string());
}

/// `str.as_bytes`: 空・ASCII・多バイト・長い文字列で、中身が Rust の `as_bytes` と同じ。元の文字列（`len` は文字数）は変わらない。
#[test]
fn as_bytes_matches_rust_and_does_not_alias() {
    let strs = [
        "",
        "a",
        "héllo",
        "é€😀",
        "0123456789abcdefghijklmnopqrstuvwxyz",
    ];
    for s in strs {
        let body = format!(
            "  let s = \"{s}\";\n  let mut y = s.as_bytes();\n  y.push(65);\n  return hb(&y) * 1000 + s.len();"
        );
        let src = format!("{HEAD}{HB}pub fn f() -> Int\n{TAIL}{{\n{body}\n}}\n");
        let mut v = s.as_bytes().to_vec();
        v.push(65);
        let want = hb(&v) * 1000 + s.chars().count() as i64;
        assert_eq!(run0(&src), want.to_string(), "{s:?}");
    }
}

// ---------------------------------------------------------------------------
// 生存数: 失敗の経路を含めて `sigil_blocks=` が回数に依らず 0
// ---------------------------------------------------------------------------

fn churn() -> String {
    format!(
        "{HEAD}pub fn run(n: Int) -> Int\n{TAIL}{{\n{}}}\n",
        r#"  let mut i = 0;
  let mut total = 0;
  while i < n {
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
    let mut bl = List.new();
    bl.push(1);
    bl.push(2);
    bl.push(300);
    bl.push(4);
    match Bytes.from_list(&bl) {
      Ok(x) => {
        total = total + 1000;
      }
      Err(m) => {
        total = total + m.len() - 12;
      }
    }
    let mut el = List.new();
    match Bytes.from_list(&el) {
      Ok(x) => {
        total = total + x.len();
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    let mut neg = List.new();
    neg.push(0 - 1);
    match Bytes.from_list(&neg) {
      Ok(x) => {
        total = total + 1000;
      }
      Err(m) => {}
    }
    let mut b = Bytes.new();
    b.push(97);
    b.push(0xC3);
    b.push(0x28);
    match b.to_string_utf8() {
      Ok(s) => {
        total = total + 1000;
      }
      Err(m) => {
        total = total + m.len() - 46;
      }
    }
    let mut c = Bytes.new();
    c.push(97);
    c.push(98);
    c.push(0xE2);
    c.push(0x82);
    match c.to_string_utf8() {
      Ok(s) => {
        total = total + 1000;
      }
      Err(m) => {
        total = total + m.len() - 43;
      }
    }
    let e = Bytes.new();
    match e.to_string_utf8() {
      Ok(s) => {
        total = total + s.len();
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    let ok = "héllo".as_bytes();
    match ok.to_string_utf8() {
      Ok(s) => {
        total = total + s.len() - 5;
      }
      Err(m) => {
        total = total + 1000;
      }
    }
    let z = "".as_bytes();
    total = total + z.len();
    i = i + 1;
  }
  return total;
"#
    )
}

fn live(src: &str, n: i64) -> (i64, String) {
    let dir = scratch("bytes35diii-live");
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
    // 塊はすべて Sigil 製の確保器が数える。漏れ・二重解放は `sigil_blocks=` の厳密一致で見る（補足 G-7）。
    (num("sigil_blocks="), got.stdout)
}

/// 失敗の経路（範囲外の一覧・不正な UTF-8）でも塊を漏らさない。1000 回と 100000 回で完全に一致し、終了時に 0。
#[test]
fn failure_paths_leave_no_blocks_across_1000_and_100000_iterations() {
    let src = churn();
    let (s1, out1) = live(&src, 1_000);
    let (s2, out2) = live(&src, 100_000);
    assert_eq!(out1.trim_end().rsplit('\n').next(), Some("0"), "{out1}");
    assert_eq!(out2.trim_end().rsplit('\n').next(), Some("0"), "{out2}");
    assert_eq!(s1, s2, "1000 回と 100000 回で生きている数が違う");
    assert_eq!(s2, 0, "終了時に塊が残っている");
}

// ---------------------------------------------------------------------------
// Rust のランタイムから消えている
// ---------------------------------------------------------------------------

#[test]
fn the_three_conversions_are_gone_from_the_rust_runtime() {
    let rt = include_str!("../runtime/core/src/lib.rs");
    let cg = include_str!("../src/codegen.rs");
    for name in ["bytes_to_string_utf8", "str_as_bytes", "bytes_from_list"] {
        assert!(
            !rt.contains(&format!("sigil_rt_{name}")),
            "{name} が Rust のランタイムに残っている"
        );
        assert!(
            !cg.contains(&format!("\"{name}\"")),
            "{name} が rt_spec に残っている"
        );
    }
    // `fs.read` 用の `valid_utf8` は残す
    assert!(include_str!("../src/stdlib/core_string.sigil").contains("pub fn valid_utf8"));
}

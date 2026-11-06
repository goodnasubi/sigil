//! ADR 0025 補足 A-2・A-4・A-7 の 3-4i と補足 B: Sigil 製の確保器 `std.os.alloc` とネイティブの差し替え。
//!
//! - 差分試験: `raw.alloc`/`raw.free` を使う利用者のプログラムを評価器（組み込みの確保器）とネイティブ
//!   （Sigil 製の確保器）で走らせ、値が一致することと、ネイティブの `sigil_blocks=` を確かめる。
//! - 影の表の試験（補足 A-2）: 評価器の上で確保器の関数を直接呼ぶ駆動モジュールを走らせる。評価器は
//!   `mmap` の塊の中の読み書きだけを許すので、範囲外の書き込みはそこで止まる。番地の重なりは全バイトの
//!   模様で、整列は番地の剰余で確かめる。同じ駆動をネイティブでも走らせて突き合わせる。
//! - 否定の試験: align の不正・二重解放・不正な class・`munmap` の前の検査（補足 B-1）は `exit_group(70)`。
//! - 変異の試験: 確保器のソースの検査を 1 つずつ外すと、対応する否定の試験が止まらなくなることを確かめる。
//! - 結ぶ範囲（補足 C-2。常に結ぶ）と `sigil spec`／`sigil spec --std` の出力、`SIGIL_RT_LIVE`（補足 B-5・C-5）。
mod common;

use common::native::{exec_env, scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::exec::Options;
use sigil::stdlib::embedded::{ALLOC_FN, ALLOC_MODULE, EMBEDDED, FREE_FN, LIVE_FN};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// 確保器の本番のソース。
fn alloc_src() -> &'static str {
    EMBEDDED
        .iter()
        .find(|(n, _)| *n == ALLOC_MODULE)
        .map(|(_, s)| *s)
        .expect("確保器が登録されている")
}

// ---- 利用者のプログラム（`raw.alloc` を直接使う） ----

fn user_prog(targets: &str, body: &str) -> String {
    format!(
        "module m.t\n  targets {targets}\n  trust base\n\nfn main() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n"
    )
}

fn user_opts() -> Options {
    Options {
        trust_base: vec!["m.t".into()],
        ..Options::captured()
    }
}

/// 評価器で走らせる。成功なら値、停止なら停止の報告。
fn eval_user(src: &str) -> Result<String, String> {
    match sigil::ir_eval::run_sources_with(&[src], "main", &[], &user_opts()) {
        Ok(o) => Ok(o.value),
        Err(f) => Err(format!("{:?}", f.error)),
    }
}

fn native_user(src: &str) -> Ran {
    let dir = scratch("alloc34i");
    let exe = dir.join("a.out");
    sigil::native::build(
        &[src],
        "main",
        &sigil::codegen::Opts {
            trust_base: vec!["m.t".into()],
            ..Default::default()
        },
        &exe,
    )
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

/// 評価器とネイティブで同じ値になることを確かめ、値とネイティブの `live:` の行を返す。
fn both_user(body: &str) -> (String, String) {
    let src = user_prog("server", body);
    let want = eval_user(&src).unwrap_or_else(|e| panic!("評価器が失敗: {e}"));
    let got = native_user(&src);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want,
        "評価器とネイティブの値が違う"
    );
    (want, live_line(&got))
}

/// 評価器もネイティブも止まる（ネイティブは終了コード 70）。
fn both_stop(body: &str, eval_needle: &str) {
    let src = user_prog("server", body);
    match eval_user(&src) {
        Err(m) => assert!(m.contains(eval_needle), "期待 {eval_needle} / 実際 {m}"),
        Ok(v) => panic!("評価器で止まるはずが成功: {v}"),
    }
    let got = native_user(&src);
    assert_eq!(
        got.code,
        Some(70),
        "ネイティブは exit_group(70): {got:?}",
        got = got.stderr
    );
}

// ---- 駆動モジュール（確保器の関数を直接呼ぶ。影の表の試験） ----

fn driver(body: &str) -> String {
    // 使わない `use` は検査で拒否されるので、本体が呼ぶものだけを取り込む。
    let uses: String = ["alloc", "free", "live_blocks"]
        .iter()
        .filter(|f| body.contains(&format!("{f}(")) && !body.contains(&format!("raw.{f}(")))
        .map(|f| format!("use std.os.alloc.{f};\n"))
        .collect();
    format!(
        "module std.os.t\n  targets server\n  trust base\n\n{uses}\npub fn run() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n"
    )
}

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n")
}

fn driver_copts(body: &str, alloc_override: Option<String>) -> COpts {
    let mut extra = vec![("std.os.t".to_string(), driver(body))];
    if let Some(a) = alloc_override {
        extra.push((ALLOC_MODULE.to_string(), a));
    }
    COpts {
        embedded_extra: extra,
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    }
}

/// 駆動を評価器で走らせる。成功なら値、停止なら停止の報告。
fn eval_driver(body: &str, alloc_override: Option<String>) -> Result<String, String> {
    let c = driver_copts(body, alloc_override);
    let o = Options {
        embedded_extra: c.embedded_extra,
        embedded_roots: c.embedded_roots,
        ..Options::captured()
    };
    match sigil::ir_eval::run_sources_with(&[&user_entry()], "std.os.t.run", &[], &o) {
        Ok(o) => Ok(o.value),
        Err(f) => Err(format!("{:?}", f.error)),
    }
}

fn native_driver(body: &str, alloc_override: Option<String>) -> Ran {
    let c = driver_copts(body, alloc_override);
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("alloc34i-drv");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    got
}

/// 駆動を評価器とネイティブで走らせ、同じ値を返すことを確かめる。
fn both_driver(body: &str) -> String {
    let want = eval_driver(body, None).unwrap_or_else(|e| panic!("評価器が失敗: {e}"));
    let got = native_driver(body, None);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want,
        "評価器とネイティブの値が違う"
    );
    want
}

const EXIT70: &str = "exit_group(70)";

/// 駆動が確保器の異常停止で止まる（評価器は exit_group(70) の模擬停止、ネイティブは終了コード 70）。
fn driver_halts(body: &str) {
    match eval_driver(body, None) {
        Err(m) => assert!(m.contains(EXIT70), "exit_group(70) のはず: {m}"),
        Ok(v) => panic!("評価器で止まるはずが成功: {v}"),
    }
    let got = native_driver(body, None);
    assert_eq!(
        got.code,
        Some(70),
        "ネイティブは exit_group(70): {}",
        got.stderr
    );
}

// ---- 差分試験 ----

#[test]
fn small_large_aligned_roundtrip_matches_and_frees_everything() {
    let body = "  let a = raw.alloc(24, 8);\n  let b = raw.alloc(3000, 8);\n  let c = raw.alloc(100, 256);\n  let d = raw.alloc(1, 1);\n  let e = raw.alloc(2048, 16);\n  raw.store_i64(a, 5);\n  raw.store_i64(b, 6);\n  raw.store_i64(c, 7);\n  raw.store_u8(d, 9);\n  raw.store_i64(raw.offset(e, 2040), 11);\n  let v = raw.load_i64(a) + raw.load_i64(b) * 10 + raw.load_i64(c) * 100 + raw.load_u8(d) * 1000 + raw.load_i64(raw.offset(e, 2040)) * 10000 + raw.addr_to_int(c) % 256 + raw.addr_to_int(e) % 16;\n  raw.free(a, 24, 8);\n  raw.free(b, 3000, 8);\n  raw.free(c, 100, 256);\n  raw.free(d, 1, 1);\n  raw.free(e, 2048, 16);\n  return v;";
    let (v, live) = both_user(body);
    assert_eq!(v, "119765");
    assert_eq!(live, "sigil_blocks=0");
}

#[test]
fn alloc_free_loop_matches_and_leaks_are_counted() {
    // 1000 回の確保と解放（小・大・整列）を評価器とネイティブで突き合わせ、最後に残した 3 つだけが数えられる。
    // 番地の再利用と生存数が増えないことは、確保器を直接呼ぶ reuse_returns_the_freed_block_and_loops_do_not_grow で確かめる。
    let body = "  let mut i = 0;\n  let mut s = 0;\n  while i < 1000 {\n    let p = raw.alloc(1 + i % 2100, 8);\n    let q = raw.alloc(64, 1 << (i % 13));\n    raw.store_u8(p, i % 200);\n    s = s + raw.load_u8(p);\n    raw.free(q, 64, 1 << (i % 13));\n    raw.free(p, 1 + i % 2100, 8);\n    i = i + 1;\n  }\n  raw.alloc(16, 8);\n  raw.alloc(5000, 8);\n  raw.alloc(16, 64);\n  return s;";
    let (v, live) = both_user(body);
    assert_eq!(v, "99500");
    assert_eq!(live, "sigil_blocks=3");
}

#[test]
fn many_live_large_blocks_grow_the_table() {
    // 生きた大ブロック 300 個（表の初期容量 256 を超えて作り直す）。全部の模様が保たれ、全部返せる。
    let body = "  let arr = raw.alloc(8 * 300, 8);\n  let mut i = 0;\n  while i < 300 {\n    let p = raw.alloc(4000, 8);\n    raw.store_i64(p, i);\n    raw.store_i64(raw.offset(p, 3992), i * 3);\n    raw.store_addr(raw.offset(arr, 8 * i), p);\n    i = i + 1;\n  }\n  let mut s = 0;\n  i = 0;\n  while i < 300 {\n    let p = raw.load_addr(raw.offset(arr, 8 * i));\n    s = s + raw.load_i64(p) + raw.load_i64(raw.offset(p, 3992));\n    raw.free(p, 4000, 8);\n    i = i + 1;\n  }\n  raw.free(arr, 8 * 300, 8);\n  return s;";
    let (v, live) = both_user(body);
    assert_eq!(v, (0..300).map(|i| i * 4).sum::<i64>().to_string());
    assert_eq!(live, "sigil_blocks=0");
}

/// 影の表: 大きさ 1..3000・整列 1..4096 の 120 個の塊を確保し、全バイトに模様を書き、
/// 互いに重ならない（全バイトが保たれる）・整列している・半分を返して取り直しても壊れないことを確かめる。
const SHADOW: &str = "  let n = 120;\n  let arr = alloc(24 * n, 8);\n  let mut i = 0;\n  while i < n {\n    let size = 1 + (i * 37) % 3000;\n    let align = 1 << (i % 13);\n    let p = alloc(size, align);\n    if raw.addr_to_int(p) % align != 0 {\n      return -1;\n    }\n    raw.set(p, i % 251, size);\n    let e = raw.offset(arr, 24 * i);\n    raw.store_addr(e, p);\n    raw.store_i64(raw.offset(e, 8), size);\n    raw.store_i64(raw.offset(e, 16), align);\n    i = i + 1;\n  }\n  i = 1;\n  while i < n {\n    let e = raw.offset(arr, 24 * i);\n    free(raw.load_addr(e), raw.load_i64(raw.offset(e, 8)), raw.load_i64(raw.offset(e, 16)));\n    let size = 1 + (i * 53) % 2500;\n    let p = alloc(size, 16);\n    raw.set(p, i % 251, size);\n    raw.store_addr(e, p);\n    raw.store_i64(raw.offset(e, 8), size);\n    raw.store_i64(raw.offset(e, 16), 16);\n    i = i + 2;\n  }\n  let mut bad = 0;\n  i = 0;\n  while i < n {\n    let e = raw.offset(arr, 24 * i);\n    let p = raw.load_addr(e);\n    let size = raw.load_i64(raw.offset(e, 8));\n    let mut j = 0;\n    while j < size {\n      if raw.load_u8(raw.offset(p, j)) != i % 251 {\n        bad = bad + 1;\n      }\n      j = j + 1;\n    }\n    free(p, size, raw.load_i64(raw.offset(e, 16)));\n    i = i + 1;\n  }\n  free(arr, 24 * n, 8);\n  return bad * 1000 + live_blocks();";

#[test]
fn shadow_table_blocks_inside_mmap_disjoint_aligned_and_all_returned() {
    assert_eq!(both_driver(SHADOW), "0");
}

#[test]
fn live_blocks_counts_small_and_large() {
    let body = "  let a = alloc(10, 8);\n  let b = alloc(5000, 8);\n  let c = alloc(10, 512);\n  let n = live_blocks();\n  free(b, 5000, 8);\n  free(a, 10, 8);\n  free(c, 10, 512);\n  return n * 10 + live_blocks();";
    assert_eq!(both_driver(body), "30");
}

#[test]
fn large_aligned_layout_matches_b1() {
    // align > 16: ret = base + align、ret-32 に base、ret-24 に map_len = round_up(align + 16 + size)。
    // align <= 16 の大ブロック: ret = base + 16（ページの先頭 + 16）。
    let body = "  let p = alloc(5000, 1024);\n  let a = raw.addr_to_int(p);\n  let base = raw.load_i64(raw.int_to_addr(a - 32));\n  let len = raw.load_i64(raw.int_to_addr(a - 24));\n  let q = alloc(3000, 8);\n  let qa = raw.addr_to_int(q);\n  let r = (a - base) * 1000000 + len * 10 + match base % 4096 == 0 { true => 0, false => 1 } + match qa % 4096 == 16 { true => 0, false => 2 };\n  free(p, 5000, 1024);\n  free(q, 3000, 8);\n  return r;";
    // a - base = 1024、len = round_up(1024 + 16 + 5000, 4096) = 8192。
    assert_eq!(both_driver(body), "1024081920");
}

/// 解放した小ブロックをすぐ取り直すと同じ番地が返り、確保と解放を繰り返しても番地は動かず、生存数も増えない。
const REUSE: &str = "  let p = alloc(64, 8);\n  free(p, 64, 8);\n  let q = alloc(64, 8);\n  let mut same = 0;\n  if raw.addr_to_int(p) == raw.addr_to_int(q) {\n    same = 1;\n  }\n  free(q, 64, 8);\n  let mut moved = 0;\n  let mut grew = 0;\n  let mut i = 0;\n  while i < 1000 {\n    let size = 1 + i % 2100;\n    let al = 1 << (i % 13);\n    let a = alloc(size, 8);\n    let b = alloc(64, al);\n    if live_blocks() != 2 {\n      grew = grew + 1;\n    }\n    free(b, 64, al);\n    free(a, size, 8);\n    let c = alloc(64, 8);\n    if raw.addr_to_int(c) != raw.addr_to_int(p) {\n      moved = moved + 1;\n    }\n    free(c, 64, 8);\n    if live_blocks() != 0 {\n      grew = grew + 1;\n    }\n    i = i + 1;\n  }\n  return same * 100000000 + moved * 10000 + grew;";

#[test]
fn reuse_returns_the_freed_block_and_loops_do_not_grow() {
    assert_eq!(both_driver(REUSE), "100000000");
}

/// 各 (size, align) を確保し、頭の class と size の語を読む（class を 1 桁ずつ並べた数を返す）。
fn class_digits(cases: &[(i64, i64)]) -> String {
    let mut b = String::from("  let mut r = 0;\n");
    for (n, (size, align)) in cases.iter().enumerate() {
        b += &format!("  let p{n} = alloc({size}, {align});\n  let h{n} = raw.offset(p{n}, -16);\n  if raw.load_i64(raw.offset(h{n}, 8)) != {size} || raw.addr_to_int(p{n}) % {align} != 0 {{\n    return -1;\n  }}\n  r = r * 10 + (raw.load_i64(h{n}) - 0x5349474C414C0000);\n");
    }
    b += &format!(
        "  if live_blocks() != {} {{\n    return -2;\n  }}\n",
        cases.len()
    );
    for (n, (size, align)) in cases.iter().enumerate() {
        b += &format!("  free(p{n}, {size}, {align});\n");
    }
    b += "  if live_blocks() != 0 {\n    return -3;\n  }\n  return r;";
    b
}

#[test]
fn size_classes_at_every_boundary() {
    // サイズクラス 16 << k の境界の両側（1..2048）と、2047〜2050 の小・大の境目。
    let small: Vec<(i64, i64)> = [
        1, 16, 17, 32, 33, 64, 65, 128, 129, 256, 257, 512, 513, 1024, 1025, 2047, 2048,
    ]
    .iter()
    .map(|&s| (s, 8))
    .collect();
    assert_eq!(both_driver(&class_digits(&small)), "112233445566777");
    let edge = [
        (2047, 16),
        (2048, 16),
        (2049, 16),
        (2050, 8),
        (2049, 1),
        (2047, 32),
        (2048, 32),
        (2049, 32),
    ];
    assert_eq!(both_driver(&class_digits(&edge)), "77888999");
}

#[test]
fn small_large_boundary_layout() {
    // alloc(2048, 16) はスラブ（class 7、block+16）、alloc(2049, 8) は大ブロック（class 8、ページの先頭 + 16）、
    // alloc(2048, 32) は整列の大ブロック（class 9、ret = base + 32）。生存数も 3。
    let body = "  let a = alloc(2048, 16);\n  let b = alloc(2049, 8);\n  let c = alloc(2048, 32);\n  let m = 0x5349474C414C0000;\n  let ka = raw.load_i64(raw.offset(a, -16)) - m;\n  let kb = raw.load_i64(raw.offset(b, -16)) - m;\n  let kc = raw.load_i64(raw.offset(c, -16)) - m;\n  let cb = raw.load_i64(raw.offset(c, -32));\n  let cl = raw.load_i64(raw.offset(c, -24));\n  let lay = match raw.addr_to_int(b) % 4096 == 16 { true => 0, false => 1 } + match raw.addr_to_int(c) - cb == 32 { true => 0, false => 2 } + match cl == 4096 { true => 0, false => 4 };\n  let n = live_blocks();\n  free(a, 2048, 16);\n  free(b, 2049, 8);\n  free(c, 2048, 32);\n  return ka * 100000 + kb * 10000 + kc * 1000 + n * 100 + lay * 10 + live_blocks();";
    // c の map_len = round_up(32 + 16 + 2048, 4096) = 4096。
    assert_eq!(both_driver(body), "789300");
}

// ---- 否定の試験 ----

#[test]
fn invalid_align_and_size_stop_in_both() {
    both_stop("  raw.alloc(16, 3);\n  return 0;", "2の累乗");
    both_stop("  raw.alloc(16, 8192);\n  return 0;", "align <= 4096");
    both_stop("  raw.alloc(0, 8);\n  return 0;", "size > 0");
    both_stop(
        "  raw.alloc(70368744177665, 8);\n  return 0;",
        "size <= 2^46",
    );
    // 確保器の直接の呼び出しでも同じ（align 0・負・4096 超・2 の冪でない、size 0・上限超）。
    for (s, a) in [
        ("16", "0"),
        ("16", "-8"),
        ("16", "8192"),
        ("16", "24"),
        ("0", "8"),
        ("70368744177665", "8"),
    ] {
        driver_halts(&format!("  alloc({s}, {a});\n  return 0;"));
    }
    driver_halts("  let p = alloc(16, 8);\n  free(p, 16, 3);\n  return 0;");
    driver_halts("  let p = alloc(16, 8);\n  free(p, 16, 8192);\n  return 0;");
}

#[test]
fn double_free_stops_without_sigsegv() {
    // 小ブロックは解放済み印、大ブロックは表で検出して止まる（ネイティブも SIGSEGV ではなく 70）。
    both_stop(
        "  let p = raw.alloc(16, 8);\n  raw.free(p, 16, 8);\n  raw.free(p, 16, 8);\n  return 0;",
        "二重解放",
    );
    both_stop(
        "  let p = raw.alloc(5000, 8);\n  raw.free(p, 5000, 8);\n  raw.free(p, 5000, 8);\n  return 0;",
        "二重解放",
    );
    both_stop(
        "  let p = raw.alloc(100, 512);\n  raw.free(p, 100, 512);\n  raw.free(p, 100, 512);\n  return 0;",
        "二重解放",
    );
    driver_halts("  let p = alloc(16, 8);\n  free(p, 16, 8);\n  free(p, 16, 8);\n  return 0;");
    driver_halts(
        "  let p = alloc(5000, 8);\n  free(p, 5000, 8);\n  free(p, 5000, 8);\n  return 0;",
    );
}

#[test]
fn foreign_or_interior_pointer_stops() {
    // 確保器の外の番地・ブロックの途中・ヌル・size の不一致。
    driver_halts("  let p = alloc(64, 8);\n  free(raw.offset(p, 16), 48, 8);\n  return 0;");
    driver_halts("  free(raw.int_to_addr(4096), 16, 8);\n  return 0;");
    driver_halts("  free(raw.addr_null(), 16, 8);\n  return 0;");
    driver_halts("  let p = alloc(64, 8);\n  free(p, 63, 8);\n  return 0;");
    driver_halts("  let p = alloc(5000, 8);\n  free(p, 4999, 8);\n  return 0;");
    // スラブの端: class 1（ブロック 48B、1 ページに 85 個）の最初のブロック p に対し、p + 4080 は 86 個目の位置で
    // ページに収まらない。頭の位置に正しい頭を偽造しても止まる。
    driver_halts(SLAB_TAIL);
}

const SLAB_TAIL: &str = "  let p = alloc(32, 8);\n  let h = raw.offset(p, 4064);\n  raw.store_i64(h, 0x5349474C414C0001);\n  raw.store_i64(raw.offset(h, 8), 32);\n  free(raw.offset(p, 4080), 32, 8);\n  return 0;";

#[test]
fn bad_class_in_header_stops() {
    // 頭の magic|class を壊す（小・大・整列の大）。
    driver_halts("  let p = alloc(64, 8);\n  raw.store_i64(raw.offset(p, -16), 12345);\n  free(p, 64, 8);\n  return 0;");
    driver_halts("  let p = alloc(64, 8);\n  let h = raw.offset(p, -16);\n  raw.store_i64(h, raw.load_i64(h) + 1);\n  free(p, 64, 8);\n  return 0;");
    // class 8 の塊を 9 と偽る: 塊の外 a - 32 を読む前に a % 32 で止まる。読む位置が隣の写像に
    // 当たるかはホスト次第なので、この検査を外す変異は決定的に検出できず、変異試験は置かない。
    driver_halts("  let p = alloc(5000, 8);\n  let h = raw.offset(p, -16);\n  raw.store_i64(h, raw.load_i64(h) + 1);\n  free(p, 5000, 8);\n  return 0;");
    driver_halts("  let p = alloc(100, 64);\n  let h = raw.offset(p, -16);\n  raw.store_i64(h, raw.load_i64(h) - 1);\n  free(p, 100, 64);\n  return 0;");
}

#[test]
fn munmap_checks_stop_before_unmapping() {
    // align > 16 の大ブロックの {base, map_len} を壊す（補足 B-1）。
    let corrupt = |off: i64, expr: &str| {
        format!("  let p = alloc(100, 64);\n  let m = raw.offset(p, {off});\n  raw.store_i64(m, {expr});\n  free(p, 100, 64);\n  return 0;")
    };
    // base がページ境界でない
    driver_halts(&corrupt(-32, "raw.load_i64(m) + 8"));
    // ret - base が 32 未満（ページ境界を保ったまま base を ret より後ろへずらす）
    driver_halts(&corrupt(-32, "raw.load_i64(m) + 4096"));
    // ret - base が 4096 超
    driver_halts(&corrupt(-32, "raw.load_i64(m) - 4096"));
    // map_len が 4096 の倍数でない
    driver_halts(&corrupt(-24, "raw.load_i64(m) + 1"));
    // map_len が 0
    driver_halts(&corrupt(-24, "0"));
    // map_len が 4096 の倍数だが記録と違う（多すぎ・少なすぎ。等式 len == round_up(d + 16 + size) で止める）
    driver_halts(&corrupt(-24, "raw.load_i64(m) + 4096"));
    driver_halts(LEN_TOO_SHORT);
}

/// align 4096 の大ブロック（map_len = round_up(4096 + 16 + 100) = 8192）の map_len を 4096 に縮める。
const LEN_TOO_SHORT: &str = "  let p = alloc(100, 4096);\n  raw.store_i64(raw.offset(p, -24), 4096);\n  free(p, 100, 4096);\n  return 0;";

// ---- 変異の試験 ----

/// 確保器のソースの `from` を `to` に置き換えた版（置き換えがちょうど 1 か所であることを確かめる）。
fn mutate(from: &str, to: &str) -> String {
    let s = alloc_src();
    assert_eq!(
        s.matches(from).count(),
        1,
        "変異の元が 1 か所でない: {from}"
    );
    s.replace(from, to)
}

/// 本番のソースでは止まり、変異させたソースでは止まらない（その検査が試験で効いている）。
fn mutation_is_caught(body: &str, from: &str, to: &str) {
    match eval_driver(body, None) {
        Err(m) => assert!(m.contains(EXIT70), "{m}"),
        Ok(v) => panic!("本番で止まらない: {v}"),
    }
    let r = eval_driver(body, Some(mutate(from, to)));
    assert!(
        !matches!(&r, Err(m) if m.contains(EXIT70)),
        "変異 {from:?} でも exit_group(70) で止まった（試験が検査を見ていない）: {r:?}"
    );
}

#[test]
fn mutations_of_checks_are_caught() {
    // 小ブロックの二重解放（解放時に解放済み印を書く。印の検査は class の検査と重ねた多重の守り）
    mutation_is_caught(
        "  let p = alloc(16, 8);\n  free(p, 16, 8);\n  free(p, 16, 8);\n  return 0;",
        "  raw.store_i64(h, FREED + k);\n  let slot",
        "  let slot",
    );
    // 不正な class（小）
    mutation_is_caught(
        "  let p = alloc(64, 8);\n  let h = raw.offset(p, -16);\n  raw.store_i64(h, raw.load_i64(h) + 1);\n  free(p, 64, 8);\n  return 0;",
        "if w != MAGIC + k || raw.load_i64(raw.offset(h, 8)) != size {",
        "if raw.load_i64(raw.offset(h, 8)) != size {",
    );
    // ret - base の上限
    mutation_is_caught(
        "  let p = alloc(100, 64);\n  let m = raw.offset(p, -32);\n  raw.store_i64(m, raw.load_i64(m) - 4096);\n  raw.store_i64(raw.offset(m, 8), raw.load_i64(raw.offset(m, 8)) + 4096);\n  free(p, 100, 64);\n  return 0;",
        "d > 4096 || ",
        "",
    );
    // map_len の等式（4096 の倍数でない・多すぎ・少なすぎ）。外すと評価器の munmap の完全一致検査で別の停止になる
    for body in [
        "  let p = alloc(100, 64);\n  let m = raw.offset(p, -24);\n  raw.store_i64(m, raw.load_i64(m) + 1);\n  free(p, 100, 64);\n  return 0;",
        "  let p = alloc(100, 64);\n  let m = raw.offset(p, -24);\n  raw.store_i64(m, raw.load_i64(m) + 4096);\n  free(p, 100, 64);\n  return 0;",
        LEN_TOO_SHORT,
    ] {
        mutation_is_caught(body, " || len != round_page(d + 16 + size)", "");
    }
    // スラブのページの端（ブロックの番号の上限）
    mutation_is_caught(SLAB_TAIL, " || off / bs >= 4096 / bs", "");
    // align の範囲（2 の冪）
    mutation_is_caught(
        "  alloc(16, 24);\n  return 0;",
        "&& (align & (align - 1)) == 0",
        "",
    );
    // size の上限
    mutation_is_caught(
        "  alloc(70368744177665, 8);\n  return 0;",
        "  if size <= 0 || size > MAX_SIZE {\n    halt();\n    return raw.addr_null();",
        "  if size <= 0 {\n    halt();\n    return raw.addr_null();",
    );
}

/// 本番のソースでは `want` を返し、変異させたソースでは別の値か停止になる。
fn mutation_changes_value(body: &str, want: &str, from: &str, to: &str) {
    assert_eq!(eval_driver(body, None).as_deref(), Ok(want), "本番の値");
    let r = eval_driver(body, Some(mutate(from, to)));
    assert!(
        r.as_deref() != Ok(want),
        "変異 {from:?} -> {to:?} でも同じ値（試験が見ていない）"
    );
}

#[test]
fn mutations_of_reuse_and_boundaries_are_caught() {
    // 解放したブロックを空きリストに戻さない
    mutation_changes_value(
        REUSE,
        "100000000",
        "  raw.store_addr(slot, raw.int_to_addr(a));\n  LIVE = LIVE - 1;",
        "  LIVE = LIVE - 1;",
    );
    // 生存数を減らさない（小・大）
    mutation_changes_value(
        REUSE,
        "100000000",
        "  raw.store_addr(slot, raw.int_to_addr(a));\n  LIVE = LIVE - 1;",
        "  raw.store_addr(slot, raw.int_to_addr(a));",
    );
    mutation_changes_value(
        REUSE,
        "100000000",
        "  unmap(base, len);\n  LIVE = LIVE - 1;",
        "  unmap(base, len);",
    );
    // 小・大の境目（size と align）
    let edge = class_digits(&[(2048, 16), (2049, 16), (2049, 8), (2048, 32), (2047, 32)]);
    for (from, to) in [
        (
            "if align <= 16 && size <= 2048 {",
            "if align <= 16 && size <= 2049 {",
        ),
        (
            "if align <= 16 && size <= 2048 {",
            "if align <= 16 && size < 2048 {",
        ),
        (
            "if align <= 16 && size <= 2048 {",
            "if align <= 32 && size <= 2048 {",
        ),
        (
            "if align <= 16 && size <= 2048 {",
            "if align < 16 && size <= 2048 {",
        ),
        ("while (16 << k) < size {", "while (16 << k) <= size {"),
    ] {
        mutation_changes_value(&edge, "78899", from, to);
    }
}

#[test]
fn mutation_of_large_double_free_table_is_caught_natively_as_crash() {
    // 表で引かずに頭を読む変異: 解放済みの大ブロックの頭を読みに行き、ネイティブは 70 ではなくなる
    // （評価器も munmap 済みの塊の読みで別の停止になる）。本番は表で検出して 70 で止まる。
    let body = "  let p = alloc(5000, 8);\n  free(p, 5000, 8);\n  free(p, 5000, 8);\n  return 0;";
    let m = mutate(
        "  let i = tbl_find(ai, true);\n  if i >= 0 {",
        "  let i = 0;\n  if raw.load_i64(raw.int_to_addr(ai - 16)) != 0 {",
    );
    assert_eq!(native_driver(body, None).code, Some(70));
    let got = native_driver(body, Some(m.clone()));
    assert_ne!(got.code, Some(70), "変異でも 70: {}", got.stderr);
    let r = eval_driver(body, Some(m));
    assert!(
        matches!(&r, Err(e) if !e.contains(EXIT70)),
        "評価器は exit_group でない停止のはず: {r:?}"
    );
}

// ---- 結ぶ範囲と表示 ----

fn has_alloc_fns(src: &str) -> bool {
    let p = sigil::ir_eval::lower_sources_with(&[src], "main", &["m.t".to_string()])
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    let names: Vec<&str> = p.funcs.iter().map(|f| f.name.as_str()).collect();
    let n = [ALLOC_FN, FREE_FN, LIVE_FN]
        .iter()
        .filter(|f| names.contains(f))
        .count();
    assert!(
        n == 0 || n == 3,
        "確保器の入口は全部あるか全部ないか: {names:?}"
    );
    n == 3
}

const USES_ALLOC: &str = "  let p = raw.alloc(8, 8);\n  raw.free(p, 8, 8);\n  return 1;";
const NO_ALLOC: &str = "  return raw.addr_to_int(raw.addr_null()) + 1;";

#[test]
fn allocator_is_linked_for_every_entry() {
    // 補足 C-2: 入口の `targets` や `raw.alloc`/`raw.free` の使用の有無によらず、常に結ぶ。
    assert!(has_alloc_fns(&user_prog("server", USES_ALLOC)));
    assert!(has_alloc_fns(&user_prog("server", NO_ALLOC)));
    assert!(has_alloc_fns(&user_prog("browser, server", USES_ALLOC)));
    assert!(has_alloc_fns(&user_prog("browser", NO_ALLOC)));
    assert!(has_alloc_fns(&user_prog("bare", USES_ALLOC)));
    assert!(has_alloc_fns(&user_prog("bare, server", NO_ALLOC)));
    assert!(has_alloc_fns(&user_prog(
        "server",
        "  raw.free(raw.addr_null(), 8, 8);\n  return 1;"
    )));
}

#[test]
fn native_live_line_always_has_sigil_blocks() {
    // 補足 C-2・C-5: すべてのプログラムが確保器を結ぶので、`sigil_blocks=` は常に出る。
    for (targets, body) in [
        ("server", USES_ALLOC),
        ("server", NO_ALLOC),
        ("browser, server", USES_ALLOC),
        ("bare", NO_ALLOC),
    ] {
        let r = native_user(&user_prog(targets, body));
        assert_eq!(r.code, Some(0), "{targets}: {}", r.stderr);
        assert_eq!(live_line(&r), "sigil_blocks=0", "{targets}");
    }
    // 値は Rust の確保器のときと同じ。
    let r = native_user(&user_prog("browser, server", USES_ALLOC));
    assert_eq!(r.stdout.trim_end(), "1");
}

#[test]
fn evaluator_keeps_builtin_allocator_with_strict_align_check() {
    // 既知の差（補足 A-4）: 解放時の align は評価器だけが検査する。評価器が組み込みの確保器のままであることの確かめ。
    let body = "  let p = raw.alloc(24, 8);\n  raw.free(p, 24, 16);\n  return 1;";
    let src = user_prog("server", body);
    let e = eval_user(&src).expect_err("評価器は align の不一致で止まる");
    assert!(e.contains("異なるサイズまたはアライメント"), "{e}");
    let n = native_user(&src);
    assert_eq!(n.code, Some(0), "{}", n.stderr);
    assert_eq!(live_line(&n), "sigil_blocks=0");
}

#[test]
fn spec_hides_embedded_std_and_spec_std_shows_it() {
    // 補足 C-2: `sigil spec` は利用者のモジュールだけ。埋め込みの `std.*` は `sigil spec --std`。
    for src in [
        user_prog("server", USES_ALLOC),
        user_prog("server", NO_ALLOC),
        user_prog("browser, server", USES_ALLOC),
    ] {
        let out = sigil::spec::spec(&src).unwrap();
        assert!(!out.contains("暗黙に結ぶ"), "{out}");
        assert!(!out.contains("std.os.alloc"), "{out}");
        let j = sigil::spec::extract(&src).unwrap().to_json().to_string();
        assert!(!j.contains("\"embedded\""), "{j}");
        assert!(!j.contains("std.os.alloc"), "{j}");
    }
    let out = sigil::spec::std_text(&COpts::default()).unwrap();
    assert!(
        out.contains("標準モジュール: std.os.alloc\n\nモジュール std.os.alloc\n  出力先: サーバー\n  信頼基盤: はい"),
        "{out}"
    );
    assert!(out.contains("標準モジュール: std.core.float\n"), "{out}");
    assert!(out.contains("標準モジュール: std.core.int\n"), "{out}");
    assert!(out.contains("  大域: mut LIVE: Int = 0\n"), "{out}");
    assert!(
        out.contains("  触る大域: FREED（読む）, LIVE（読み書き）, MAGIC（読む）\n"),
        "{out}"
    );
    assert!(!out.contains("表示できない"), "{out}");
    let j = sigil::spec::std_json(&COpts::default())
        .unwrap()
        .to_string();
    assert!(
        j.starts_with("{\"format\":1,\"std\":[{\"format\":1,\"module\":{\"kind\":\"module\",\"name\":\"std.os.alloc\""),
        "{j}"
    );
}

#[test]
fn cli_spec_std_needs_no_file() {
    let bin = env!("CARGO_BIN_EXE_sigil");
    let o = std::process::Command::new(bin)
        .args(["spec", "--std"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8(o.stdout).unwrap();
    assert!(out.contains("標準モジュール: std.os.alloc"), "{out}");
    let o = std::process::Command::new(bin)
        .args(["spec", "--std", "--json"])
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8(o.stdout).unwrap().contains("\"std\":["));
    // ファイルや --root を添えるのは使い方の誤り（終了コード 2）。
    for extra in [
        vec!["spec", "--std", "x.sigil"],
        vec!["spec", "--std", "--root", "."],
    ] {
        let o = std::process::Command::new(bin)
            .args(&extra)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{extra:?}");
    }
}

#[test]
fn user_cannot_name_the_allocator() {
    let src = format!(
        "module m.t\n  targets server\n  trust base\n\nuse std.os.alloc.alloc;\n\nfn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    let r = sigil::ir_eval::run_sources_with(&[&src], "main", &[], &user_opts());
    assert!(r.is_err());
}

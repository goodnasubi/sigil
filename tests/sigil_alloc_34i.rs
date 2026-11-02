//! ADR 0025 補足 A-2・A-4・A-7 の 3-4i と補足 B: Sigil 製の確保器 `std.os.alloc` とネイティブの差し替え。
//!
//! - 差分試験: `raw.alloc`/`raw.free` を使う利用者のプログラムを評価器（組み込みの確保器）とネイティブ
//!   （Sigil 製の確保器）で走らせ、値が一致することと、ネイティブの `sigil_blocks=` を確かめる。
//! - 影の表の試験（補足 A-2）: 評価器の上で確保器の関数を直接呼ぶ駆動モジュールを走らせる。評価器は
//!   `mmap` の塊の中の読み書きだけを許すので、範囲外の書き込みはそこで止まる。番地の重なりは全バイトの
//!   模様で、整列は番地の剰余で確かめる。同じ駆動をネイティブでも走らせて突き合わせる。
//! - 否定の試験: align の不正・二重解放・不正な class・`munmap` の前の検査（補足 B-1）は `exit_group(70)`。
//! - 変異の試験: 確保器のソースの検査を 1 つずつ外すと、対応する否定の試験が止まらなくなることを確かめる。
//! - 結ぶ範囲（補足 B-3・B-4）と `sigil spec` の出力、`SIGIL_RT_LIVE`（補足 B-5）。
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
    assert_eq!(live, "strs=0 blocks=0 sigil_blocks=0");
}

#[test]
fn reuse_loop_does_not_grow_and_leaks_are_counted() {
    // 1000 回の確保と解放（小・大・整列）。生存数は 0 に戻り、最後に残した 3 つだけが数えられる。
    let body = "  let mut i = 0;\n  let mut s = 0;\n  while i < 1000 {\n    let p = raw.alloc(1 + i % 2100, 8);\n    let q = raw.alloc(64, 1 << (i % 13));\n    raw.store_u8(p, i % 200);\n    s = s + raw.load_u8(p);\n    raw.free(q, 64, 1 << (i % 13));\n    raw.free(p, 1 + i % 2100, 8);\n    i = i + 1;\n  }\n  raw.alloc(16, 8);\n  raw.alloc(5000, 8);\n  raw.alloc(16, 64);\n  return s;";
    let (v, live) = both_user(body);
    assert_eq!(v, "99500");
    assert_eq!(live, "strs=0 blocks=0 sigil_blocks=3");
}

#[test]
fn many_live_large_blocks_grow_the_table() {
    // 生きた大ブロック 300 個（表の初期容量 256 を超えて作り直す）。全部の模様が保たれ、全部返せる。
    let body = "  let arr = raw.alloc(8 * 300, 8);\n  let mut i = 0;\n  while i < 300 {\n    let p = raw.alloc(4000, 8);\n    raw.store_i64(p, i);\n    raw.store_i64(raw.offset(p, 3992), i * 3);\n    raw.store_addr(raw.offset(arr, 8 * i), p);\n    i = i + 1;\n  }\n  let mut s = 0;\n  i = 0;\n  while i < 300 {\n    let p = raw.load_addr(raw.offset(arr, 8 * i));\n    s = s + raw.load_i64(p) + raw.load_i64(raw.offset(p, 3992));\n    raw.free(p, 4000, 8);\n    i = i + 1;\n  }\n  raw.free(arr, 8 * 300, 8);\n  return s;";
    let (v, live) = both_user(body);
    assert_eq!(v, (0..300).map(|i| i * 4).sum::<i64>().to_string());
    assert_eq!(live, "strs=0 blocks=0 sigil_blocks=0");
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
}

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
}

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
    // map_len が 4096 の倍数
    mutation_is_caught(
        "  let p = alloc(100, 64);\n  let m = raw.offset(p, -24);\n  raw.store_i64(m, raw.load_i64(m) + 1);\n  free(p, 100, 64);\n  return 0;",
        " || len % 4096 != 0",
        "",
    );
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
fn linked_only_for_server_only_entries_using_raw_alloc() {
    assert!(has_alloc_fns(&user_prog("server", USES_ALLOC)));
    // 入口から辿れない（main は呼ばない）が SIR に入る。
    assert!(!has_alloc_fns(&user_prog("server", NO_ALLOC)));
    assert!(!has_alloc_fns(&user_prog("browser, server", USES_ALLOC)));
    // raw.free だけでも結ぶ。
    assert!(has_alloc_fns(&user_prog(
        "server",
        "  raw.free(raw.addr_null(), 8, 8);\n  return 1;"
    )));
}

#[test]
fn native_live_line_has_sigil_blocks_only_when_linked() {
    let linked = native_user(&user_prog("server", USES_ALLOC));
    assert_eq!(linked.code, Some(0), "{}", linked.stderr);
    assert_eq!(live_line(&linked), "strs=0 blocks=0 sigil_blocks=0");
    let none = native_user(&user_prog("server", NO_ALLOC));
    assert_eq!(live_line(&none), "strs=0 blocks=0");
    // 入口が server だけでなければ Rust の確保器（補足 B-4）。値は同じで、sigil_blocks= は出ない。
    let rust = native_user(&user_prog("browser, server", USES_ALLOC));
    assert_eq!(rust.code, Some(0), "{}", rust.stderr);
    assert_eq!(live_line(&rust), "strs=0 blocks=0");
    assert_eq!(rust.stdout.trim_end(), "1");
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
    assert_eq!(live_line(&n), "strs=0 blocks=0 sigil_blocks=0");
}

#[test]
fn spec_shows_embedded_allocator_only_when_linked() {
    let out = sigil::spec::spec(&user_prog("server", USES_ALLOC)).unwrap();
    assert!(
        out.contains("\n暗黙に結ぶモジュール: std.os.alloc\n\nモジュール std.os.alloc\n  出力先: サーバー\n  信頼基盤: はい"),
        "{out}"
    );
    assert!(out.contains("  大域: mut LIVE: Int = 0\n"), "{out}");
    assert!(
        out.contains("  触る大域: FREED（読む）, LIVE（読み書き）, MAGIC（読む）\n"),
        "{out}"
    );
    assert!(!out.contains("表示できない"), "{out}");
    for src in [
        user_prog("server", NO_ALLOC),
        user_prog("browser, server", USES_ALLOC),
    ] {
        let out = sigil::spec::spec(&src).unwrap();
        assert!(!out.contains("暗黙に結ぶ"), "{out}");
        let j = sigil::spec::extract(&src).unwrap().to_json().to_string();
        assert!(!j.contains("\"embedded\""), "{j}");
    }
    let j = sigil::spec::extract(&user_prog("server", USES_ALLOC))
        .unwrap()
        .to_json()
        .to_string();
    assert!(
        j.contains(
            "\"embedded\":[{\"format\":1,\"module\":{\"kind\":\"module\",\"name\":\"std.os.alloc\""
        ),
        "{j}"
    );
}

#[test]
fn user_cannot_name_the_allocator() {
    let src = format!(
        "module m.t\n  targets server\n  trust base\n\nuse std.os.alloc.alloc;\n\nfn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    let r = sigil::ir_eval::run_sources_with(&[&src], "main", &[], &user_opts());
    assert!(r.is_err());
}

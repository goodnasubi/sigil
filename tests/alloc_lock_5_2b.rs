//! ADR 0030 決定 6 / PR 5-2b: 確保器（`std.os.alloc`）の排他。
//!
//! 公開の入口（`alloc`・`free`・`live_blocks`）は、状態のページの最後の語（4088）のロックを取って内側の本体を囲む。
//! 競合は複数の OS スレッドが動いてから（5-2c 以降）起こるので、ここでは単一スレッドで次を確かめる。
//! - 入口を何度呼んでも、戻ったときロックの語は 0（空き）に戻っている（放し忘れ・二重取得がない）。
//! - 比較交換が排他として働く（取得済みのロックを、もう一度は取れない）。
//! - 排他を足しても、確保器の結果（番地の重なりのなさ・数え）は変わらない。
//!
//! 試験のために、確保器のソースに、ロックの語を読む・取る・放す関数を足した版（`embedded_extra` の差し替え）を使う。
//! 評価器とネイティブで同じ値になることを確かめる。
mod common;

use common::native::{exec_env, scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::exec::Options;
use sigil::stdlib::embedded::{ALLOC_MODULE, EMBEDDED};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// 本番の確保器のソースに、試験用の関数を足したもの。
fn alloc_with_probes() -> String {
    let src = EMBEDDED
        .iter()
        .find(|(n, _)| *n == ALLOC_MODULE)
        .map(|(_, s)| *s)
        .expect("確保器が登録されている");
    format!(
        "{src}\npub fn probe_word() -> Int\n{TAIL}  unchecked \"試験用: ロックの語を読む\"\n{{\n  return raw.atomic_load_i64(lock_addr());\n}}\n\npub fn probe_try() -> Int\n{TAIL}  unchecked \"試験用: ロックを一度だけ取ろうとして、前の値を返す（0 なら取れた）\"\n{{\n  return raw.atomic_cas_i64(lock_addr(), 0, 1);\n}}\n\npub fn probe_release()\n{TAIL}  unchecked \"試験用: 取ったロックを放す\"\n{{\n  unlock();\n}}\n"
    )
}

fn driver(body: &str) -> String {
    let mut names: Vec<&str> = [
        "alloc",
        "free",
        "live_blocks",
        "probe_word",
        "probe_try",
        "probe_release",
    ]
    .into_iter()
    .filter(|f| body.contains(&format!("{f}(")))
    .collect();
    names.sort_unstable(); // `use` は文字の順に並べる
    let uses: String = names
        .iter()
        .map(|f| format!("use std.os.alloc.{f};\n"))
        .collect();
    format!(
        "module std.os.t\n  targets server\n  trust base\n\n{uses}\npub fn run() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n"
    )
}

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n")
}

fn copts(body: &str) -> COpts {
    COpts {
        embedded_extra: vec![
            ("std.os.t".to_string(), driver(body)),
            (ALLOC_MODULE.to_string(), alloc_with_probes()),
        ],
        embedded_roots: vec!["std.os.t".into()],
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
    match sigil::ir_eval::run_sources_with(&[&user_entry()], "std.os.t.run", &[], &o) {
        Ok(o) => Ok(o.value),
        Err(f) => Err(format!("{:?}", f.error)),
    }
}

fn native(body: &str) -> Ran {
    let c = copts(body);
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("alloclock52b");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    got
}

/// 評価器とネイティブで同じ値になることを確かめて、その値を返す。
fn both(body: &str) -> String {
    let want = eval(body).unwrap_or_else(|e| panic!("評価器が失敗: {e}\n{body}"));
    let got = native(body);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want,
        "評価器とネイティブの値が違う"
    );
    want
}

#[test]
fn the_lock_is_free_after_every_entry() {
    // 小・大・整列つきの確保と解放、`live_blocks` を続けても、各入口の後でロックの語は 0 のまま。
    let body = "  let a = alloc(24, 8);\n  let w1 = probe_word();\n  let b = alloc(5000, 64);\n  let w2 = probe_word();\n  let n = live_blocks();\n  let w3 = probe_word();\n  free(a, 24, 8);\n  let w4 = probe_word();\n  free(b, 5000, 64);\n  let w5 = probe_word();\n  let z = live_blocks();\n  let w6 = probe_word();\n  return w1 + w2 + w3 + w4 + w5 + w6 + n * 10 + z * 1000;";
    // 取得が 2 つ生きている間は live_blocks が 2（n * 10 = 20）、全部返したら 0
    assert_eq!(both(body), "20");
}

#[test]
fn the_lock_word_stays_zero_while_the_tables_grow() {
    // 大ブロックを多数持って表・索引を育てても、状態のページの最後の語（ロック）は壊れない。
    let body = "  let mut i = 0;\n  let mut live = 0;\n  let mut keep = alloc(4096, 16);\n  while i < 300 {\n    let p = alloc(4096 + i * 8, 16);\n    raw.store_i64(p, i);\n    live = live + 1;\n    i = i + 1;\n  }\n  let w = probe_word();\n  let n = live_blocks();\n  return w * 1000000 + n;";
    // 300 個 + keep の 1 個 = 301 個が生きている。ロックの語は 0。
    assert_eq!(both(body), "301");
}

#[test]
fn compare_exchange_excludes_a_second_taker() {
    // 取れたら 0 が返り、取得済みの間は 1 が返る。放すと、また取れる。
    let body = "  let a = probe_try();\n  let held = probe_word();\n  let b = probe_try();\n  probe_release();\n  let freed = probe_word();\n  let c = probe_try();\n  probe_release();\n  return a * 100000 + held * 10000 + b * 1000 + freed * 100 + c * 10 + probe_word();";
    // a = 0（取れた）, held = 1, b = 1（取れない）, freed = 0, c = 0（また取れた）, 最後は 0
    assert_eq!(both(body), "11000");
}

#[test]
fn live_blocks_is_zero_before_any_allocation() {
    // 一度も確保していなければ、live_blocks はロックを取らず（状態のページがまだ無いので）0 を返す。
    assert_eq!(both("  return live_blocks();"), "0");
}

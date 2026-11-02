//! ADR 0025 補足 C-3 の 3-5a: `memmove` / `memset` を codegen が生成する（libc を呼ばない）。
//!
//! `raw.copy` / `raw.set` を使う利用者のプログラムを評価器とネイティブで走らせて値を突き合わせ、
//! 重なる複写の向き（前向き・後ろ向き）・長さ 0・1 バイト・端を確かめる。
//! ネイティブの実行ファイルが `memmove` / `memset` を外部から取り込んでいないことも確かめる。
mod common;

use common::native::{exec, scratch};
use sigil::exec::Options;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

fn prog(body: &str) -> String {
    format!(
        "module m.t\n  targets server\n  trust base\n\nfn main() -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n"
    )
}

fn opts() -> Options {
    Options {
        trust_base: vec!["m.t".into()],
        ..Options::captured()
    }
}

/// 長さ 16 のバッファに 0..16 を書き、`ops` を実行して、全バイトを 8 進数字のように畳み込んだ値を返す本文。
fn body(ops: &str) -> String {
    format!(
        "  let a = raw.alloc(32, 16);\n  let mut i = 0;\n  while i < 32 {{\n    raw.store_u8(raw.offset(a, i), i + 1);\n    i = i + 1;\n  }}\n{ops}\n  let mut h = 0;\n  let mut j = 0;\n  while j < 32 {{\n    h = (h * 7 + raw.load_u8(raw.offset(a, j))) % 1000003;\n    j = j + 1;\n  }}\n  raw.free(a, 32, 16);\n  return h;\n"
    )
}

/// 評価器とネイティブの値が同じであることを確かめ、値を返す。
fn both(ops: &str) -> String {
    let src = prog(&body(ops));
    let want = sigil::ir_eval::run_sources_with(&[&src], "main", &[], &opts())
        .unwrap_or_else(|f| panic!("評価器が失敗: {:?}", f.error))
        .value;
    let dir = scratch("memgen");
    let exe = dir.join("a.out");
    sigil::native::build(
        &[&src],
        "main",
        &sigil::codegen::Opts {
            trust_base: vec!["m.t".into()],
            ..Default::default()
        },
        &exe,
    )
    .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = exec(&exe, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want,
        "評価器とネイティブの値が違う: {ops}"
    );
    want
}

/// 同じ操作を Rust の側で行った畳み込み。
fn model(f: impl FnOnce(&mut [u8; 32])) -> String {
    let mut b = [0u8; 32];
    for (i, x) in b.iter_mut().enumerate() {
        *x = i as u8 + 1;
    }
    f(&mut b);
    let mut h: i64 = 0;
    for x in b {
        h = (h * 7 + x as i64) % 1_000_003;
    }
    h.to_string()
}

#[test]
fn copy_non_overlapping() {
    let got = both("  raw.copy(raw.offset(a, 16), a, 8);");
    assert_eq!(got, model(|b| b.copy_within(0..8, 16)));
}

#[test]
fn copy_overlap_dst_after_src() {
    // dst > src で重なる: 後ろから複写しないと壊れる。
    let got = both("  raw.copy(raw.offset(a, 4), a, 16);");
    assert_eq!(got, model(|b| b.copy_within(0..16, 4)));
}

#[test]
fn copy_overlap_dst_before_src() {
    // dst < src で重なる: 前から複写しないと壊れる。
    let got = both("  raw.copy(a, raw.offset(a, 4), 16);");
    assert_eq!(got, model(|b| b.copy_within(4..20, 0)));
}

#[test]
fn copy_overlap_by_one_both_directions() {
    let got = both("  raw.copy(raw.offset(a, 1), a, 31);");
    assert_eq!(got, model(|b| b.copy_within(0..31, 1)));
    let got = both("  raw.copy(a, raw.offset(a, 1), 31);");
    assert_eq!(got, model(|b| b.copy_within(1..32, 0)));
}

#[test]
fn copy_same_address_zero_and_one_byte() {
    let base = model(|_| {});
    assert_eq!(both("  raw.copy(a, a, 32);"), base);
    assert_eq!(both("  raw.copy(raw.offset(a, 3), a, 0);"), base);
    assert_eq!(
        both("  raw.copy(raw.offset(a, 31), a, 1);"),
        model(|b| b[31] = b[0])
    );
    // 末尾のバイトまで（範囲の端）。
    assert_eq!(
        both("  raw.copy(raw.offset(a, 24), raw.offset(a, 16), 8);"),
        model(|b| b.copy_within(16..24, 24))
    );
}

#[test]
fn set_fills_exactly_the_range() {
    assert_eq!(
        both("  raw.set(raw.offset(a, 8), 0xAB, 8);"),
        model(|b| b[8..16].fill(0xAB))
    );
    // 長さ 0 は何も書かない。
    assert_eq!(both("  raw.set(a, 0xFF, 0);"), model(|_| {}));
    // 1 バイトと、先頭・末尾の端。
    assert_eq!(both("  raw.set(a, 0, 1);"), model(|b| b[0] = 0));
    assert_eq!(
        both("  raw.set(raw.offset(a, 31), 7, 1);"),
        model(|b| b[31] = 7)
    );
    assert_eq!(both("  raw.set(a, 0, 32);"), model(|b| b.fill(0)));
}

/// 生成したオブジェクトが libc の `memmove` / `memset` / `memcpy` を未解決のまま取り込んでいない
/// （静的リンクの実行ファイルには libc.a 由来の同名の関数が入るので、リンクの前のオブジェクトを見る）。
#[test]
fn object_does_not_import_libc_mem_functions() {
    let src = prog(&body(
        "  raw.copy(raw.offset(a, 4), a, 16);\n  raw.set(a, 1, 4);",
    ));
    let trust = vec!["m.t".to_string()];
    let p = sigil::ir_eval::lower_sources_with(&[&src], "main", &trust).unwrap();
    let obj = sigil::codegen::compile(
        &p,
        &sigil::codegen::Opts {
            trust_base: trust,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("codegen が失敗: {e:?}"));
    let dir = scratch("memgen-nm");
    let path = dir.join("out.o");
    std::fs::write(&path, &obj).unwrap();
    let out = std::process::Command::new("nm")
        .arg("-u")
        .arg(&path)
        .output();
    let _ = std::fs::remove_dir_all(&dir);
    let Ok(out) = out else {
        eprintln!("nm がないので飛ばす");
        return;
    };
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("sigil_rt_"),
        "nm の出力が読めていない（ランタイムの記号がない）: {text}"
    );
    for line in text.lines() {
        let name = line.split_whitespace().last().unwrap_or("");
        assert!(
            !(name.starts_with("memmove")
                || name.starts_with("memset")
                || name.starts_with("memcpy")),
            "libc のメモリ関数を取り込んでいる: {line}"
        );
    }
}

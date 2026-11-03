//! 定数時間の統計的計測（ADR 0024 決定 3-4、ADR 0020 決定 1 の「計測」）。
//! PR 6-1a で置き場と道具（dudect 方式＋Welch の t 検定）を用意する。
//!
//! 通常の `cargo test` では走らない（`#[ignore]`）。実行するには:
//!
//! ```text
//! cargo test --test ct_measure -- --ignored --nocapture
//! ```
//!
//! 方法（dudect）: 比較関数について、入力の組を 2 つ用意する。
//! 組 A は先頭のバイトだけが違い、組 B は末尾のバイトだけが違う。
//! 早く抜ける比較では A の方が速く終わるので、実行時間の分布に差が出る。
//! 定数時間の比較では、差が出ないはずである。各組を何度も測り、
//! Welch の t 値で差を判定する（|t| < 4.5 なら差は見つからなかった）。
//! わざと早く抜ける比較を陽性の対照として同時に測り、対照で差が出ない
//! 場合は計測自体の感度不足として失敗にする（ADR 0020）。
//!
//! 計測は、対象の比較を大きな回数くり返すネイティブの実行ファイルを
//! 組み立てて、その実行時間を測る形にしている。プロセス起動の雑音が
//! 入るため、1 回の実行の中で比較を十分な回数くり返して、測りたい
//! 時間が支配的になるようにしてある。
//! 本来の計測（ADR 6-1 の完了時など）では、対象は `std.crypto` の実物に
//! 差し替える。この置き場の fixture は、道具の感度を確かめるためのもの。

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

/// Welch の t 値（2 つの標本の平均の差を、分散と大きさで割ったもの）。
fn welch_t(a: &[f64], b: &[f64]) -> f64 {
    let mean = |xs: &[f64]| xs.iter().sum::<f64>() / xs.len() as f64;
    let var = |xs: &[f64]| {
        let m = mean(xs);
        xs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (xs.len() - 1) as f64
    };
    let (ma, mb) = (mean(a), mean(b));
    let denom = (var(a) / a.len() as f64 + var(b) / b.len() as f64).sqrt();
    if denom == 0.0 {
        return 0.0;
    }
    (ma - mb) / denom
}

#[test]
fn welch_t_math_sanity() {
    // 同じ分布どうしでは |t| は小さい。
    let a: Vec<f64> = (0..100).map(|i| (i % 7) as f64).collect();
    let b: Vec<f64> = (0..100).map(|i| ((i + 3) % 7) as f64).collect();
    assert!(welch_t(&a, &b).abs() < 4.5);
    // 大きくずれた分布どうしでは |t| は大きい。
    let c: Vec<f64> = (0..100).map(|i| 100.0 + (i % 7) as f64).collect();
    assert!(welch_t(&a, &c).abs() >= 4.5);
}

const KEY: [i64; 32] = [
    3, 10, 17, 24, 31, 38, 45, 52, 59, 66, 73, 80, 87, 94, 101, 108, 115, 122, 129, 136, 143, 150,
    157, 164, 171, 178, 185, 192, 199, 206, 213, 220,
];

fn pushes(name: &str, bytes: &[i64]) -> String {
    let mut s = format!("  let mut {name} = Bytes.new();\n");
    for b in bytes {
        s += &format!("  {name}.push({b});\n");
    }
    s
}

/// 比較関数 `cmp` と入力の組（どこで違うか）から、計測用のプログラムを作る。
fn measure_program(cmp_name: &str, cmp_src: &str, diff_at: usize) -> String {
    let mut input = KEY;
    input[diff_at] ^= 0xFF;
    format!(
        "module {{MODULE}}\n  targets server\n  trust none\n\n{cmp_src}\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{key}{input}  let mut acc = 0;\n  let mut i = 0;\n  while i < 200000 {{\n    acc = acc + match {cmp_name}(&input, &key) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
        key = pushes("key", &KEY),
        input = pushes("input", &input),
    )
}

const CMP_EARLY: &str = "fn cmp_early(a: &Bytes, b: &Bytes) -> Bool\n  intent \"早く抜ける比較（陽性の対照）\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut i = 0;\n  while i < a.len() {\n    if a.get(i).unwrap_or(0) != b.get(i).unwrap_or(0) {\n      return false;\n    }\n    i = i + 1;\n  }\n  return true;\n}\n";

const CMP_CT: &str = "fn cmp_ct(a: &Bytes, b: &Bytes) -> Bool\n  intent \"定数時間の比較（XOR で積む）\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut diff = 0;\n  let mut i = 0;\n  while i < a.len() {\n    diff = diff | (a.get(i).unwrap_or(0) ^ b.get(i).unwrap_or(0));\n    i = i + 1;\n  }\n  return diff == 0;\n}\n";

static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn build_exe(tag: &str, src: &str) -> (PathBuf, PathBuf) {
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(format!(
        "target/ct_measure_{}_{}_{}",
        std::process::id(),
        tag,
        seq
    ));
    std::fs::create_dir_all(&dir).expect("一時置き場を作れない");
    let module = format!("target.{}.m", dir.file_name().unwrap().to_string_lossy());
    let file = dir.join("m.sigil");
    std::fs::write(&file, src.replace("{MODULE}", &module)).expect("fixture を書けない");
    let exe = dir.join("prog");
    let output = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("build")
        .arg(&file)
        .arg("main")
        .arg("-o")
        .arg(&exe)
        .output()
        .expect("sigil build を実行できない");
    assert!(
        output.status.success(),
        "sigil build が失敗: {}",
        String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(400)
            .collect::<String>()
    );
    (dir, exe)
}

/// 実行ファイルを指定回数だけ実行し、各回の所要時間（ミリ秒）を集める。
fn time_runs(exe: &PathBuf, samples: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        let status = Command::new(exe)
            .output()
            .expect("計測用プログラムを実行できない");
        assert!(status.status.success(), "計測用プログラムが失敗");
        out.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    out
}

/// 1 つの比較関数について、組 A（先頭で違う）と組 B（末尾で違う）の
/// 実行時間の Welch の t 値を返す。
fn measure(cmp_name: &str, cmp_src: &str) -> f64 {
    let (dir_a, exe_a) = build_exe(cmp_name, &measure_program(cmp_name, cmp_src, 0));
    let (dir_b, exe_b) = build_exe(cmp_name, &measure_program(cmp_name, cmp_src, 31));
    let a = time_runs(&exe_a, 25);
    let b = time_runs(&exe_b, 25);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
    welch_t(&a, &b)
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4）。cargo test -- --ignored で実行"]
fn dudect_compare_timing() {
    // 陽性の対照: 早く抜ける比較は、先頭で違う組の方が速いはずで、
    // t 値に差が出なければならない（出なければ計測の感度不足）。
    let t_early = measure("cmp_early", CMP_EARLY);
    println!("陽性の対照（早く抜ける比較）: t = {t_early:.2}");
    assert!(
        t_early.abs() >= 4.5,
        "陽性の対照で差が出なかった（計測の感度不足）: t = {t_early:.2}"
    );
    // 本命: 定数時間の比較では差が出ないはず。
    let t_ct = measure("cmp_ct", CMP_CT);
    println!("定数時間の比較: t = {t_ct:.2}");
    assert!(
        t_ct.abs() < 4.5,
        "定数時間の比較に時間の差が出た: t = {t_ct:.2}"
    );
}

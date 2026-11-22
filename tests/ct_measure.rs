//! 定数時間の統計的計測（ADR 0024 決定 3-4、ADR 0020 決定 1 の「計測」）。
//! `std.crypto` の実物の比較（`ct_eq`、HMAC照合、SCRAM照合、ベアラー署名照合）
//! および陽性の対照（素の `cmp_early`、HMAC包み `verify_hmac_early`）に対して dudect 方式（Welch の t 検定）で計測する。
//!
//! 通常の `cargo test` では走らない（`#[ignore]`）。実行するには:
//!
//! ```text
//! cargo test --release --test ct_measure -- --ignored --nocapture
//! ```
//!
//! 方法（dudect）: 各比較関数について、入力の組を 2 つ用意する。
//! 組 A は先頭のバイトだけが違い、組 B は末尾のバイトだけが違う（HMAC 系は実際の MAC の該当位置を反転）。
//! 2 組を交互に各組実行し（素の比較は 1,000,000 回、HMAC 系は 100,000 回）、
//! Welch の t 値で判定する（|t| < 4.5 なら差は見つからなかった）。
//! 陽性の対照では |t| >= 4.5 となり感度が確認される。

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

#[test]
fn fixture_signatures_match_stdlib() {
    let pg_scram = include_str!("../src/stdlib/pg_scram.sigil");
    assert!(
        pg_scram.contains(SCRAM_SIG.trim()),
        "SCRAM_SIG が src/stdlib/pg_scram.sigil と一致していません"
    );
    let web = include_str!("../src/stdlib/web.sigil");
    assert!(
        web.contains(BEARER_SIG.trim()),
        "BEARER_SIG が src/stdlib/web.sigil と一致していません"
    );
}

const KEY: [i64; 32] = [
    3, 10, 17, 24, 31, 38, 45, 52, 59, 66, 73, 80, 87, 94, 101, 108, 115, 122, 129, 136, 143, 150,
    157, 164, 171, 178, 185, 192, 199, 206, 213, 220,
];

const MSG: [i64; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

fn pushes(name: &str, bytes: &[i64]) -> String {
    let mut s = format!("  let mut {name} = Bytes.new();\n");
    for b in bytes {
        s += &format!("  {name}.push({b});\n");
    }
    s
}

#[derive(Clone, Copy, Debug)]
enum CmpTarget {
    Early,
    EarlyHmac,
    CtEq,
    Scram,
    Hmac,
    Bearer,
}

const CMP_EARLY: &str = "\
fn cmp_early(a: &Bytes, b: &Bytes) -> Bool
  intent \"早く抜ける比較（陽性の対照）\"
  requires none
  ensures none
  effects none
{
  let mut i = 0;
  while i < a.byte_len() {
    if a.get(i).unwrap_or(0) != b.get(i).unwrap_or(0) {
      return false;
    }
    i = i + 1;
  }
  return true;
}
";

const HMAC_EARLY: &str = "\
fn verify_hmac_early(key: &Secret, message: &Bytes, expected: &Bytes) -> Bool
  intent \"HMAC 計算後に早期リターンで比較する（陽性の対照）\"
  requires key.byte_len() >= 32
  ensures none
  effects none
{
  let mac = hmac_sha256(&key, &message);
  let got = mac.expose();
  return cmp_early(&got, &expected);
}
";

const SCRAM_SIG: &str = "\
pub fn ct_signature(signature: &Bytes, expected: &Bytes) -> Bool
  intent \"SCRAM の形式と上限を検査して認証する\"
  requires none
  ensures none
  effects none
{
  return ct_eq(&signature, &expected);
}
";

const BEARER_SIG: &str = "\
fn ct_bearer_signature(key: &Secret, message: &Bytes, tag: &Bytes) -> Bool
  intent \"既存 HMAC の定数時間照合に受信署名対象を渡す\"
  requires none
  ensures none
  effects none
{
  return verify_hmac_sha256(&key, &message, &tag);
}
";

/// 比較対象と入力の組（どこで違うか）から、計測用のプログラムを作る。
fn measure_program(target: CmpTarget, diff_at: usize, iters: usize) -> String {
    let mut input = KEY;
    input[diff_at] ^= 0xFF;

    match target {
        CmpTarget::Early => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\n{CMP_EARLY}\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{key}{input}  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    acc = acc + match cmp_early(&input, &key) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            key = pushes("key", &KEY),
            input = pushes("input", &input),
        ),
        CmpTarget::EarlyHmac => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\n{CMP_EARLY}\n{HMAC_EARLY}\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{raw_key}  let key = Secret.from_bytes(raw_key);\n{msg}  let mut input = hmac_sha256(&key, &msg).expose();\n  let b = input.get({diff_at}).unwrap_or(0);\n  let _ = input.set({diff_at}, b ^ 255);\n  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    acc = acc + match verify_hmac_early(&key, &msg, &input) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            raw_key = pushes("raw_key", &KEY),
            msg = pushes("msg", &MSG),
            diff_at = diff_at,
        ),
        CmpTarget::CtEq => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{key}{input}  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    acc = acc + match ct_eq(&input, &key) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            key = pushes("key", &KEY),
            input = pushes("input", &input),
        ),
        CmpTarget::Scram => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\n{SCRAM_SIG}\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{key}{input}  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    acc = acc + match ct_signature(&input, &key) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            key = pushes("key", &KEY),
            input = pushes("input", &input),
        ),
        CmpTarget::Hmac => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{raw_key}  let key = Secret.from_bytes(raw_key);\n{msg}  let mut input = hmac_sha256(&key, &msg).expose();\n  let b = input.get({diff_at}).unwrap_or(0);\n  let _ = input.set({diff_at}, b ^ 255);\n  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    acc = acc + match verify_hmac_sha256(&key, &msg, &input) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            raw_key = pushes("raw_key", &KEY),
            msg = pushes("msg", &MSG),
            diff_at = diff_at,
        ),
        CmpTarget::Bearer => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\n{BEARER_SIG}\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{raw_key}  let key = Secret.from_bytes(raw_key);\n{msg}  let mut input = hmac_sha256(&key, &msg).expose();\n  let b = input.get({diff_at}).unwrap_or(0);\n  let _ = input.set({diff_at}, b ^ 255);\n  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    acc = acc + match ct_bearer_signature(&key, &msg, &input) {{ true => 1, false => 0 }};\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            raw_key = pushes("raw_key", &KEY),
            msg = pushes("msg", &MSG),
            diff_at = diff_at,
        ),
    }
}

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

/// 2 つの実行ファイルを交互に指定回数（samples）だけ実行し、各回の所要時間（ミリ秒）を集める。
/// 交互に並べることで、時間経過に伴う温度上昇・CPU 周波数変動などの系統的ドリフトを相殺する（ADR 0020）。
fn measure_pair(exe_a: &PathBuf, exe_b: &PathBuf, samples: usize) -> (Vec<f64>, Vec<f64>) {
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start_a = Instant::now();
        let status_a = Command::new(exe_a)
            .output()
            .expect("計測用プログラム A を実行できない");
        assert!(status_a.status.success(), "計測用プログラム A が失敗");
        a.push(start_a.elapsed().as_secs_f64() * 1000.0);

        let start_b = Instant::now();
        let status_b = Command::new(exe_b)
            .output()
            .expect("計測用プログラム B を実行できない");
        assert!(status_b.status.success(), "計測用プログラム B が失敗");
        b.push(start_b.elapsed().as_secs_f64() * 1000.0);
    }
    (a, b)
}

const DEFAULT_SAMPLES: usize = 25;
const DEFAULT_ITERS: usize = 40_000;

// HMAC 系の比較は純粋 Sigil 実装の SHA-256 圧縮関数（Cranelift opt_level: none）を伴い実行時間を要するため、
// 既定では各組 100,000 回（1,250 回反復 × 80 サンプル）で測定する（Issue #672 で共用 runner での感度確保のため 80,000 回から調整）。
// 陽性の対照（verify_hmac_early）で感度確保を目的とした設定（実測での検証対象。定数時間側の差の調査は Issue #674）。
const DEFAULT_HMAC_SAMPLES: usize = 80;
const DEFAULT_HMAC_ITERS: usize = 1_250;

fn get_iters(default: usize) -> usize {
    std::env::var("SIGIL_CT_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 100)
        .unwrap_or(default)
}

fn get_samples(default: usize) -> usize {
    std::env::var("SIGIL_CT_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&v| v >= 5)
        .unwrap_or(default)
}

fn measure_target(target: CmpTarget, name: &str, iters: usize, samples: usize) -> f64 {
    let (dir_a, exe_a) = build_exe(&format!("{name}_a"), &measure_program(target, 0, iters));
    let (dir_b, exe_b) = build_exe(&format!("{name}_b"), &measure_program(target, 31, iters));
    let (a, b) = measure_pair(&exe_a, &exe_b, samples);
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
    let ma: f64 = a.iter().sum::<f64>() / a.len() as f64;
    let mb: f64 = b.iter().sum::<f64>() / b.len() as f64;
    let t = welch_t(&a, &b);
    println!(
        "  [{name}] a mean: {ma:.3} ms, b mean: {mb:.3} ms, diff: {:.3} ms, t = {t:.2}",
        ma - mb
    );
    t
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_positive_control() {
    let iters = get_iters(DEFAULT_ITERS);
    let samples = get_samples(DEFAULT_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::Early, "cmp_early", iters, samples);
    println!("陽性の対照 (cmp_early, 各組 {total} 回): t = {t:.2}");
    assert!(
        t.abs() >= 4.5,
        "陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_positive_control_hmac() {
    let iters = get_iters(DEFAULT_HMAC_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::EarlyHmac, "hmac_early", iters, samples);
    println!("陽性の対照（HMAC 包み, 各組 {total} 回): t = {t:.2}");
    assert!(
        t.abs() >= 4.5,
        "HMAC 陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_ct_eq() {
    let iters = get_iters(DEFAULT_ITERS);
    let samples = get_samples(DEFAULT_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::CtEq, "ct_eq", iters, samples);
    println!("ct_eq (std.core.bytes.ct_equal, 各組 {total} 回): t = {t:.2}");
    assert!(t.abs() < 4.5, "ct_eq に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_scram() {
    let iters = get_iters(DEFAULT_ITERS);
    let samples = get_samples(DEFAULT_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::Scram, "scram_sig", iters, samples);
    println!("SCRAM 照合 (std.pg_scram.ct_signature, 各組 {total} 回): t = {t:.2}");
    assert!(t.abs() < 4.5, "SCRAM 照合に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_hmac() {
    let iters = get_iters(DEFAULT_HMAC_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::Hmac, "hmac_verify", iters, samples);
    println!("HMAC 照合 (std.crypto.verify_hmac_sha256, 各組 {total} 回): t = {t:.2}");
    assert!(t.abs() < 4.5, "HMAC 照合に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_bearer() {
    let iters = get_iters(DEFAULT_HMAC_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::Bearer, "bearer_sig", iters, samples);
    println!("ベアラー署名照合 (std.web.ct_bearer_signature, 各組 {total} 回): t = {t:.2}");
    assert!(
        t.abs() < 4.5,
        "ベアラー署名照合に時間の差が出た: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn dudect_compare_timing() {
    // 互換性のための全対象一括実行
    ct_measure_positive_control();
    ct_measure_positive_control_hmac();
    ct_measure_ct_eq();
    ct_measure_scram();
    ct_measure_hmac();
    ct_measure_bearer();
}

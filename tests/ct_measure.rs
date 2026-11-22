//! 定数時間の統計的計測（ADR 0024 決定 3-4、ADR 0020 決定 1 の「計測」）。
//! `std.crypto` の実物の比較（`ct_eq`、HMAC照合、SCRAM照合、ベアラー署名照合、HKDF-SHA-256、AES-128-GCM、X25519）
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
    Hkdf,
    AesGcm,
    AesGcmLeaky,
    AesGcmFail,
    AesGcmFailLeaky,
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
        // HKDF-SHA-256（ADR 0058 決定 3、段 1a）。IKM が秘密の入力（組 A は
        // 先頭、組 B は末尾のバイトだけ反転）、salt は固定の秘密、info と
        // out_len（42）と各長さは公開で組の間で同じ。Extract と Expand の
        // 両方を通す。
        CmpTarget::Hkdf => format!(
            "module {{MODULE}}\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{raw_salt}{raw_ikm}{info}  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    let salt = Secret.from_bytes(raw_salt.clone());\n    let ikm = Secret.from_bytes(raw_ikm.clone());\n    let prk = hkdf_extract_sha256(Some(salt), &ikm);\n    let okm = hkdf_expand_sha256(&prk, &info, 42);\n    acc = acc + okm.byte_len();\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
            raw_salt = pushes("raw_salt", &KEY),
            raw_ikm = pushes("raw_ikm", &input),
            info = pushes("info", &MSG),
        ),
        // AES-128-GCM（ADR 0058 決定 3、段 1b）。鍵が秘密の入力（組 A は先頭、組 B は末尾の
        // バイトだけ反転）、nonce・AAD・平文の長さは公開で組の間で同じ。暗号化と
        // 復号（タグの照合を含む）の両方を通す。`AesGcmLeaky` は同じ形のまま、鍵の 2 バイト目が
        // 10 のとき（組 B）だけ暗号化をもう 1 回する陽性の対照（秘密に依存する仕事量の差）。
        CmpTarget::AesGcm | CmpTarget::AesGcmLeaky => {
            let leak = if matches!(target, CmpTarget::AesGcmLeaky) {
                "    if raw_key.get(1).unwrap_or(0) == 10 {
      match Nonce96.from_slice(&iv) {
        Err(e) => { return 0 - 1; }
        Ok(n3) => {
          match aes128_gcm_seal(&key, n3, &aad, &Secret.from_bytes(pt.clone())) {
            Err(e) => { return 0 - 1; }
            Ok(extra) => { acc = acc + 1; }
          }
        }
      }
    }
"
            } else {
                ""
            };
            format!(
                "module {{MODULE}}\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{raw_key}{iv}{aad}{pt}  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    let key_opt = Aes128GcmKey.from_secret(Secret.from_bytes(raw_key.clone()));\n    match key_opt {{\n      Err(e) => {{ return 0 - 1; }}\n      Ok(key) => {{\n        match Nonce96.from_slice(&iv) {{\n          Err(e) => {{ return 0 - 1; }}\n          Ok(n1) => {{\n            match Nonce96.from_slice(&iv) {{\n              Err(e) => {{ return 0 - 1; }}\n              Ok(n2) => {{\n                match aes128_gcm_seal(&key, n1, &aad, &Secret.from_bytes(pt.clone())) {{\n                  Err(e) => {{ return 0 - 1; }}\n                  Ok(sealed) => {{\n                    match aes128_gcm_open(&key, &n2, &aad, &sealed) {{\n                      Err(e) => {{ return 0 - 1; }}\n                      Ok(o) => {{ acc = acc + o.byte_len(); }}\n                    }}\n{leak}                  }}\n                }}\n              }}\n            }}\n          }}\n        }}\n      }}\n    }}\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
                raw_key = pushes("raw_key", &input[..16]),
                iv = pushes("iv", &MSG[..12]),
                aad = pushes("aad", &MSG[12..20]),
                pt = pushes("pt", &MSG),
            )
        }
        // AES-128-GCM の認証失敗（ADR 0058 決定 3 の「AEAD のタグ照合では同じ長さと同じ失敗結果で不一致の位置を変える」）。
        // 鍵・nonce・AAD・平文は両組で同じ。seal が返した `sealed` のタグ（末尾 16 バイト）の 1 バイトだけを変えて open に渡す。
        // 組 A はタグの先頭（位置 0）、組 B はタグの末尾（位置 15）を改変する。長さも結果（`AuthenticationFailed`）も同じで、
        // 違うのは改変したタグの位置だけ。open が最初の不一致で打ち切る退行があれば、組の間の所要時間に差が出る。
        // `AesGcmFailLeaky` は同じ形のまま、タグの末尾が違うとき（組 B）だけ暗号化をもう 1 回する陽性の対照（不一致の位置に依存する
        // 仕事量の差）。1 バイトの比較の打ち切りそのものは、この計測の分解能（AES 1 回分の仕事量）より小さい。その構造的な
        // 保証は ct_rules（タグの照合は ct_eq だけを通り、秘密に依存する分岐を持たない）が担い、この計測は「失敗の経路全体の
        // 所要時間が不一致の位置に依存しないこと」を確かめる。
        CmpTarget::AesGcmFail | CmpTarget::AesGcmFailLeaky => {
            let tag_pos = if diff_at <= DIFF_FIRST { 0 } else { 15 };
            let idx = 32 + tag_pos;
            let leak = if matches!(target, CmpTarget::AesGcmFailLeaky) {
                "        if bad.get(47).unwrap_or(0) != sealed.get(47).unwrap_or(0) {
          match Nonce96.from_slice(&iv) {
            Err(e) => { return 0 - 1; }
            Ok(n3) => {
              match aes128_gcm_seal(&key, n3, &aad, &Secret.from_bytes(pt.clone())) {
                Err(e) => { return 0 - 1; }
                Ok(extra) => { acc = acc + 1; }
              }
            }
          }
        }
"
            } else {
                ""
            };
            format!(
                "module {{MODULE}}\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"計測\"\n  requires none\n  ensures none\n  effects none\n{{\n{raw_key}{iv}{aad}{pt}  let mut acc = 0;\n  let mut i = 0;\n  while i < {iters} {{\n    let key_opt = Aes128GcmKey.from_secret(Secret.from_bytes(raw_key.clone()));\n    match key_opt {{\n      Err(e) => {{ return 0 - 1; }}\n      Ok(key) => {{\n        match Nonce96.from_slice(&iv) {{\n          Err(e) => {{ return 0 - 1; }}\n          Ok(n1) => {{\n            match Nonce96.from_slice(&iv) {{\n              Err(e) => {{ return 0 - 1; }}\n              Ok(n2) => {{\n                match aes128_gcm_seal(&key, n1, &aad, &Secret.from_bytes(pt.clone())) {{\n                  Err(e) => {{ return 0 - 1; }}\n                  Ok(sealed) => {{\n                    let mut bad = sealed.clone();\n                    let orig = bad.get({idx}).unwrap_or(0);\n                    match bad.set({idx}, (orig + 1) % 256) {{\n                      Err(e) => {{ return 0 - 1; }}\n                      Ok(u) => {{}}\n                    }}\n                    match aes128_gcm_open(&key, &n2, &aad, &bad) {{\n                      Err(e) => {{ acc = acc + 1; }}\n                      Ok(o) => {{ return 0 - 1; }}\n                    }}\n{leak}                  }}\n                }}\n              }}\n            }}\n          }}\n        }}\n      }}\n    }}\n    i = i + 1;\n  }}\n  return acc;\n}}\n",
                raw_key = pushes("raw_key", &KEY[..16]),
                iv = pushes("iv", &MSG[..12]),
                aad = pushes("aad", &MSG[12..20]),
                pt = pushes("pt", &MSG),
            )
        }
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
/// HKDF（Extract + 2 ブロックの Expand）は HMAC 約 3 回分の計算なので、反復を絞る。
const DEFAULT_HKDF_ITERS: usize = 400;
/// AES-128-GCM（暗号化と復号の 2 回。32 バイトの平文）は 1 反復が重いので、さらに絞る。
const DEFAULT_AEAD_ITERS: usize = 40;

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

/// 組 A（先頭側）と組 B（末尾側）で違うバイトの位置。
///
/// 位置 0 は使わない。`set(0, …)` などの添字 0 はコンパイラが別の命令に畳むため、位置 0 の実行ファイルだけ
/// 関数の配置がずれる。HMAC の圧縮関数のような熱い部分の配置（アラインメント）が変わると、比較とは無関係に
/// 実行時間が約 1% 変わり、定数時間の比較でも |t| が 10 前後に出る（Issue #674、CI の実測）。
/// 位置 1 と 31 の実行ファイルは配置が同じで、違うのは数バイトの即値だけ。
const DIFF_FIRST: usize = 1;
const DIFF_LAST: usize = 31;

/// 2 つの実行ファイルの違うバイト数（長さが違えば None）。
fn differing_bytes(a: &PathBuf, b: &PathBuf) -> Option<usize> {
    let (x, y) = (std::fs::read(a).ok()?, std::fs::read(b).ok()?);
    if x.len() != y.len() {
        return None;
    }
    Some(x.iter().zip(&y).filter(|(p, q)| p != q).count())
}

/// 組 A と組 B の実行ファイルの配置が同じであること（違うのは入力の即値だけ）を確かめる。
/// 配置が違うと、比較の中身と関係のない系統差が t に出て、計測が無効になる（Issue #674）。
/// 同じ配置のときの差は 100 バイト足らず（位置 1 と 31 で 22 バイト）。配置がずれると数万バイト違う。
const MAX_LAYOUT_DIFF_BYTES: usize = 1000;

fn assert_same_layout(name: &str, a: &PathBuf, b: &PathBuf) {
    let d = differing_bytes(a, b);
    assert!(
        matches!(d, Some(n) if n <= MAX_LAYOUT_DIFF_BYTES),
        "[{name}] 組 A と組 B の実行ファイルの配置が違う（違うバイト数: {d:?}）。配置の差が t に混ざるため計測が無効"
    );
}

fn measure_target(target: CmpTarget, name: &str, iters: usize, samples: usize) -> f64 {
    let (dir_a, exe_a) = build_exe(
        &format!("{name}_a"),
        &measure_program(target, DIFF_FIRST, iters),
    );
    let (dir_b, exe_b) = build_exe(
        &format!("{name}_b"),
        &measure_program(target, DIFF_LAST, iters),
    );
    assert_same_layout(name, &exe_a, &exe_b);
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
fn ct_measure_hkdf() {
    let iters = get_iters(DEFAULT_HKDF_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::Hkdf, "hkdf", iters, samples);
    println!(
        "HKDF-SHA-256 (std.crypto.hkdf_extract_sha256 + hkdf_expand_sha256, 各組 {total} 回): t = {t:.2}"
    );
    assert!(t.abs() < 4.5, "HKDF に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_positive_control_aes_gcm() {
    let iters = get_iters(DEFAULT_AEAD_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::AesGcmLeaky, "aes_gcm_leaky", iters, samples);
    println!("陽性の対照（AES-128-GCM 形, 鍵に依存する仕事量, 各組 {total} 回): t = {t:.2}");
    assert!(
        t.abs() >= 4.5,
        "AES-128-GCM 陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_aes_gcm() {
    let iters = get_iters(DEFAULT_AEAD_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::AesGcm, "aes_gcm", iters, samples);
    println!(
        "AES-128-GCM (std.crypto.aes128_gcm_seal + aes128_gcm_open, 各組 {total} 回): t = {t:.2}"
    );
    assert!(t.abs() < 4.5, "AES-128-GCM に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_positive_control_aes_gcm_fail() {
    let iters = get_iters(DEFAULT_AEAD_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(
        CmpTarget::AesGcmFailLeaky,
        "aes_gcm_fail_leaky",
        iters,
        samples,
    );
    println!(
        "陽性の対照（AES-128-GCM 認証失敗の形, タグの不一致位置に依存する仕事量, 各組 {total} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "AES-128-GCM 認証失敗の陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0024 決定 3-4、ADR 0020 決定 1）。cargo test -- --ignored で実行"]
fn ct_measure_aes_gcm_fail() {
    let iters = get_iters(DEFAULT_AEAD_ITERS);
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let total = iters * samples;
    let t = measure_target(CmpTarget::AesGcmFail, "aes_gcm_fail", iters, samples);
    println!(
        "AES-128-GCM 認証失敗（タグの先頭と末尾の改変, aes128_gcm_open, 各組 {total} 回): t = {t:.2}"
    );
    assert!(
        t.abs() < 4.5,
        "AES-128-GCM の認証失敗にタグの不一致位置による時間の差が出た: t = {t:.2}"
    );
}

/// ADR 0058 決定 3。両組で同じネイティブ実行ファイルを使い、同じ長さの
/// スカラーを環境変数で渡す。各半バイトを ASCII 0x40〜0x4f に写す。
fn x25519_program(iters: usize) -> String {
    format!(
        r#"module {{MODULE}}
  targets server
  trust none

fn main() -> Int
  intent "X25519 の公開鍵導出と共有値計算を計測する"
  requires none
  ensures none
  effects env
{{
  let scalar_env = env.get(&"SIGIL_CT_X25519_SCALAR");
  let control_env = env.get(&"SIGIL_CT_X25519_CONTROL");
  let encoded = scalar_env.unwrap_or("").as_bytes();
  let control = control_env.unwrap_or("") == "1";
  if encoded.byte_len() != 64 {{ return 1; }}
  let mut raw_key = Bytes.new();
  let mut base_point = Bytes.new();
  let mut i = 0;
  while i < 32 {{
    let hi = encoded.get(2 * i).unwrap_or(0) & 15;
    let lo = encoded.get(2 * i + 1).unwrap_or(0) & 15;
    raw_key.push((hi << 4) | lo);
    base_point.push(match i == 0 {{ true => 9, false => 0 }});
    i = i + 1;
  }}
  // 陽性対照だけで、クランプで消えないスカラーのビットに仕事量を依存させる。
  let extra = control && (raw_key.get(0).unwrap_or(0) & 8) != 0;
  match X25519PrivateKey.from_secret(Secret.from_bytes(raw_key)) {{
    Err(_) => {{ return 2; }}
    Ok(key) => {{
      match X25519PublicKey.from_bytes(base_point) {{
        Err(_) => {{ return 3; }}
        Ok(peer) => {{
          let mut acc = 0;
          let mut n = 0;
          while n < {iters} {{
            let public_key = x25519_public_key(&key);
            acc = acc + public_key.as_bytes().byte_len();
            match x25519_shared_secret(&key, &peer) {{
              Err(_) => {{ return 4; }}
              Ok(shared) => {{ acc = acc + shared.byte_len(); }}
            }}
            if extra {{
              let repeated = x25519_public_key(&key);
              acc = acc + repeated.as_bytes().byte_len() - 32;
            }}
            n = n + 1;
          }}
          return match acc == {iters} * 64 {{ true => 0, false => 5 }};
        }}
      }}
    }}
  }}
}}
"#
    )
}

fn x25519_scalar_env(scalar: &[u8; 32]) -> String {
    scalar
        .iter()
        .flat_map(|&byte| [char::from(64 + (byte >> 4)), char::from(64 + (byte & 15))])
        .collect()
}

fn measure_x25519(exe: &PathBuf, samples: usize, control: bool) -> f64 {
    let fixed = x25519_scalar_env(&[0; 32]);
    let mut state = 0x4478_51ed_8f26_893bu64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    let timed = |scalar: &str, measurements: &mut Vec<f64>| {
        let mut cmd = Command::new(exe);
        cmd.env("SIGIL_CT_X25519_SCALAR", scalar)
            .env("SIGIL_CT_X25519_CONTROL", if control { "1" } else { "0" });
        let start = Instant::now();
        let output = cmd.output().expect("X25519 計測用プログラムを実行できない");
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(
            output.status.success() && output.stdout == b"0\n",
            "X25519 計測用プログラムが失敗: {} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for _ in 0..samples {
        let mut scalar = std::array::from_fn(|_| next() as u8);
        if control {
            scalar[0] |= 8;
        }
        let varied = x25519_scalar_env(&scalar);
        if next() & 1 == 0 {
            timed(&fixed, &mut a);
            timed(&varied, &mut b);
        } else {
            timed(&varied, &mut b);
            timed(&fixed, &mut a);
        }
    }
    welch_t(&a, &b)
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_x25519 -- --ignored --nocapture"]
fn ct_measure_x25519() {
    let iters = 1;
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let (dir, exe) = build_exe("x25519", &x25519_program(iters));
    let control = measure_x25519(&exe, samples, true);
    let t = measure_x25519(&exe, samples, false);
    let _ = std::fs::remove_dir_all(&dir);
    let cpu_info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpu_info
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(key, value)| (key.trim() == "model name").then_some(value.trim()))
        .unwrap_or("不明");
    println!(
        "X25519 ({cpu}, Cranelift opt_level=none, 各組 {samples} 標本 × {iters} 回): 陽性対照 t = {control:.2}, 対象 t = {t:.2}"
    );
    assert!(
        control.abs() >= 4.5,
        "X25519 の陽性対照で差が出なかった: t = {control:.2}"
    );
    assert!(t.abs() < 4.5, "X25519 に時間の差が出た: t = {t:.2}");
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
    ct_measure_hkdf();
    ct_measure_positive_control_aes_gcm();
    ct_measure_aes_gcm();
    ct_measure_positive_control_aes_gcm_fail();
    ct_measure_aes_gcm_fail();
}

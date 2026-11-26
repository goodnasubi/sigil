//! 定数時間の統計的計測（ADR 0024 決定 3-4、ADR 0020 決定 1 の「計測」）。
//! `std.crypto` の実物の比較（`ct_eq`、HMAC照合、SCRAM照合、ベアラー署名照合、HKDF-SHA-256、AES-128-GCM、AES-256-GCM、ChaCha20-Poly1305、X25519、SHA-384、HKDF-SHA-384）
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

/// ADR 0064 決定 2。両組で同じネイティブ実行ファイルを使い、同じ長さの
/// 秘密鍵を環境変数で渡す。各半バイトを ASCII 0x40〜0x4f に写す。
/// 署名は固定のメッセージに対して 1 回行い、公開鍵導出も同じ秘密鍵で行う。
fn ecdsa_p256_program(iters: usize) -> String {
    format!(
        r#"module {{MODULE}}
  targets server
  trust none

fn main() -> Int
  intent "ECDSA P-256 の公開鍵導出と署名を計測する"
  requires none
  ensures none
  effects env
{{
  let key_env = env.get(&"SIGIL_CT_ECDSA_KEY");
  let control_env = env.get(&"SIGIL_CT_ECDSA_CONTROL");
  let encoded = key_env.unwrap_or("").as_bytes();
  let control = control_env.unwrap_or("") == "1";
  if encoded.byte_len() != 64 {{ return 1; }}
  let mut raw_key = Bytes.new();
  let mut i = 0;
  while i < 32 {{
    let hi = encoded.get(2 * i).unwrap_or(0) & 15;
    let lo = encoded.get(2 * i + 1).unwrap_or(0) & 15;
    raw_key.push((hi << 4) | lo);
    i = i + 1;
  }}
  // 陽性対照だけで、秘密鍵のビットに仕事量を依存させる。
  let extra = control && (raw_key.get(1).unwrap_or(0) & 8) != 0;
  let mut message = Bytes.new();
  message.push(0x73);
  message.push(0x69);
  message.push(0x67);
  match EcdsaP256PrivateKey.from_secret(Secret.from_bytes(raw_key)) {{
    Err(_) => {{ return 2; }}
    Ok(key) => {{
      let mut acc = 0;
      let mut n = 0;
      while n < {iters} {{
        let public_key = ecdsa_p256_public_key(&key);
        acc = acc + public_key.to_bytes().byte_len();
        match ecdsa_p256_sign(&key, &message) {{
          Err(_) => {{ return 3; }}
          Ok(sig) => {{ acc = acc + 1; }}
        }}
        if extra {{
          match ecdsa_p256_sign(&key, &message) {{
            Err(_) => {{ return 4; }}
            Ok(again) => {{ acc = acc + 1; }}
          }}
        }}
        n = n + 1;
      }}
      return match acc >= {iters} * 66 {{ true => 0, false => 5 }};
    }}
  }}
}}
"#
    )
}

fn ecdsa_p256_key_env(key: &[u8; 32]) -> String {
    x25519_scalar_env(key)
}

fn measure_ecdsa_p256(exe: &PathBuf, samples: usize, control: bool) -> f64 {
    let fixed = ecdsa_p256_key_env(&[1; 32]);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    let timed = |key: &str, measurements: &mut Vec<f64>| {
        let mut cmd = Command::new(exe);
        cmd.env("SIGIL_CT_ECDSA_KEY", key)
            .env("SIGIL_CT_ECDSA_CONTROL", if control { "1" } else { "0" });
        let start = Instant::now();
        let output = cmd.output().expect("ECDSA 計測用プログラムを実行できない");
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(
            output.status.success() && output.stdout == b"0\n",
            "ECDSA 計測用プログラムが失敗: {} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for _ in 0..samples {
        let mut key: [u8; 32] = std::array::from_fn(|_| next() as u8);
        key[0] &= 0x7f; // n 未満を保証する
        if control {
            key[1] |= 8;
        }
        let varied = ecdsa_p256_key_env(&key);
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

/// ECDSA P-256 の定数時間計測（ADR 0064 決定 2、ADR 0058 決定 3）。
/// 同一実行ファイルに環境変数で秘密鍵を渡すことでバイナリ配置差を回避している。
/// 計測の境界は「秘密鍵の取り込み・公開鍵導出・署名（DER 符号化前の署名生成まで）」で、
/// 固定鍵（全バイト 0x01）と乱数鍵を比べる。
///
/// 感度の限界: 陽性対照は署名 1 回追加（仕事量の大きな増分）で検出できることを確認するもので、
/// 語単位の微小な値依存の差は検出できない可能性がある。合格は「大きな差は見つからなかった」ことを意味する。
#[test]
#[ignore = "手動計測（ADR 0064 決定 2）。cargo test --release --test ct_measure ct_measure_ecdsa_p256 -- --ignored --nocapture"]
fn ct_measure_ecdsa_p256() {
    let iters = 1;
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let (dir, exe) = build_exe("ecdsa_p256", &ecdsa_p256_program(iters));
    let control = measure_ecdsa_p256(&exe, samples, true);
    let t = measure_ecdsa_p256(&exe, samples, false);
    let _ = std::fs::remove_dir_all(&dir);
    let cpu_info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpu_info
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(key, value)| (key.trim() == "model name").then_some(value.trim()))
        .unwrap_or("不明");
    println!(
        "ECDSA P-256 ({cpu}, Cranelift opt_level=none, 各組 {samples} 標本 × {iters} 回): 陽性対照 t = {control:.2}, 対象 t = {t:.2}"
    );
    assert!(
        control.abs() >= 4.5,
        "ECDSA P-256 の陽性対照で差が出なかった: t = {control:.2}"
    );
    assert!(t.abs() < 4.5, "ECDSA P-256 に時間の差が出た: t = {t:.2}");
}

/// ADR 0065 決定 2。両組で同じネイティブ実行ファイルを使い、同じ長さのスカラーを環境変数で渡す。
/// 各半バイトを ASCII 0x40〜0x4f に写す。相手の公開点は固定（RFC 外の独立実装で作った点）で、
/// 共有値の計算（256 回の点演算）だけを計測する。
fn ecdh_p256_program(iters: usize) -> String {
    format!(
        r#"module {{MODULE}}
  targets server
  trust none

fn main() -> Int
  intent "ECDH P-256 の共有値の計算を計測する"
  requires none
  ensures none
  effects env
{{
  let key_env = env.get(&"SIGIL_CT_ECDH_KEY");
  let control_env = env.get(&"SIGIL_CT_ECDH_CONTROL");
  let encoded = key_env.unwrap_or("").as_bytes();
  let control = control_env.unwrap_or("") == "1";
  if encoded.byte_len() != 64 {{ return 1; }}
  let mut raw_key = Bytes.new();
  let mut i = 0;
  while i < 32 {{
    let hi = encoded.get(2 * i).unwrap_or(0) & 15;
    let lo = encoded.get(2 * i + 1).unwrap_or(0) & 15;
    raw_key.push((hi << 4) | lo);
    i = i + 1;
  }}
  // 陽性対照だけで、スカラーのビットに仕事量を依存させる。
  let extra = control && (raw_key.get(1).unwrap_or(0) & 8) != 0;
  let mut peer_raw = Bytes.new();
  peer_raw.push(4);
  peer_raw.push(186);
  peer_raw.push(142);
  peer_raw.push(234);
  peer_raw.push(252);
  peer_raw.push(223);
  peer_raw.push(140);
  peer_raw.push(97);
  peer_raw.push(240);
  peer_raw.push(48);
  peer_raw.push(130);
  peer_raw.push(80);
  peer_raw.push(223);
  peer_raw.push(71);
  peer_raw.push(46);
  peer_raw.push(93);
  peer_raw.push(77);
  peer_raw.push(88);
  peer_raw.push(29);
  peer_raw.push(178);
  peer_raw.push(88);
  peer_raw.push(88);
  peer_raw.push(30);
  peer_raw.push(95);
  peer_raw.push(67);
  peer_raw.push(148);
  peer_raw.push(18);
  peer_raw.push(58);
  peer_raw.push(201);
  peer_raw.push(27);
  peer_raw.push(236);
  peer_raw.push(87);
  peer_raw.push(112);
  peer_raw.push(193);
  peer_raw.push(183);
  peer_raw.push(56);
  peer_raw.push(103);
  peer_raw.push(99);
  peer_raw.push(197);
  peer_raw.push(204);
  peer_raw.push(233);
  peer_raw.push(160);
  peer_raw.push(140);
  peer_raw.push(223);
  peer_raw.push(119);
  peer_raw.push(11);
  peer_raw.push(81);
  peer_raw.push(156);
  peer_raw.push(186);
  peer_raw.push(2);
  peer_raw.push(247);
  peer_raw.push(171);
  peer_raw.push(59);
  peer_raw.push(245);
  peer_raw.push(93);
  peer_raw.push(238);
  peer_raw.push(124);
  peer_raw.push(7);
  peer_raw.push(3);
  peer_raw.push(202);
  peer_raw.push(19);
  peer_raw.push(230);
  peer_raw.push(55);
  peer_raw.push(202);
  peer_raw.push(39);
  match EcdhP256PublicKey.from_bytes(&peer_raw) {{
    Err(_) => {{ return 2; }}
    Ok(peer) => {{
      match EcdhP256PrivateKey.from_secret(Secret.from_bytes(raw_key)) {{
        Err(_) => {{ return 3; }}
        Ok(key) => {{
          let mut acc = 0;
          let mut n = 0;
          while n < {iters} {{
            let shared = ecdh_p256_shared_secret(&key, &peer);
            acc = acc + shared.byte_len();
            if extra {{
              let again = ecdh_p256_shared_secret(&key, &peer);
              acc = acc + again.byte_len();
            }}
            n = n + 1;
          }}
          return match acc >= {iters} * 32 {{ true => 0, false => 5 }};
        }}
      }}
    }}
  }}
}}
"#
    )
}

fn ecdh_p256_key_env(key: &[u8; 32]) -> String {
    x25519_scalar_env(key)
}

fn measure_ecdh_p256(exe: &PathBuf, samples: usize, control: bool) -> f64 {
    let fixed = ecdh_p256_key_env(&[1; 32]);
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    let timed = |key: &str, measurements: &mut Vec<f64>| {
        let mut cmd = Command::new(exe);
        cmd.env("SIGIL_CT_ECDH_KEY", key)
            .env("SIGIL_CT_ECDH_CONTROL", if control { "1" } else { "0" });
        let start = Instant::now();
        let output = cmd.output().expect("ECDH 計測用プログラムを実行できない");
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(
            output.status.success() && output.stdout == b"0\n",
            "ECDH 計測用プログラムが失敗: {} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for _ in 0..samples {
        let mut key: [u8; 32] = std::array::from_fn(|_| next() as u8);
        key[0] &= 0x7f; // n 未満を保証する
        if control {
            key[1] |= 8;
        }
        let varied = ecdh_p256_key_env(&key);
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

/// ECDH P-256 の定数時間計測（ADR 0065 決定 2、ADR 0058 決定 3）。
/// 同一実行ファイルに環境変数でスカラーを渡すことでバイナリ配置差を回避している。
/// 計測の境界は「スカラーの取り込み・共有値の計算」で、
/// 固定スカラー（全バイト 0x01）と乱数スカラーを比べる。相手の公開点は固定。
///
/// 感度の限界: 陽性対照は共有値の計算を 1 回追加（仕事量の大きな増分）で検出できることを確認するもので、
/// 語単位の微小な値依存の差は検出できない可能性がある。合格は「大きな差は見つからなかった」ことを意味する。
#[test]
#[ignore = "手動計測（ADR 0065 決定 2）。cargo test --release --test ct_measure ct_measure_ecdh_p256 -- --ignored --nocapture"]
fn ct_measure_ecdh_p256() {
    let iters = 1;
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let (dir, exe) = build_exe("ecdh_p256", &ecdh_p256_program(iters));
    let control = measure_ecdh_p256(&exe, samples, true);
    let t = measure_ecdh_p256(&exe, samples, false);
    let _ = std::fs::remove_dir_all(&dir);
    let cpu_info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let cpu = cpu_info
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(key, value)| (key.trim() == "model name").then_some(value.trim()))
        .unwrap_or("不明");
    println!(
        "ECDH P-256 ({cpu}, Cranelift opt_level=none, 各組 {samples} 標本 × {iters} 回): 陽性対照 t = {control:.2}, 対象 t = {t:.2}"
    );
    assert!(
        control.abs() >= 4.5,
        "ECDH P-256 の陽性対照で差が出なかった: t = {control:.2}"
    );
    assert!(t.abs() < 4.5, "ECDH P-256 に時間の差が出た: t = {t:.2}");
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

/// ChaCha20-Poly1305 の計測用プログラム（ADR 0058 決定 3）。両組で同じ実行ファイルを使い、鍵・タグの改変位置・
/// 陽性対照の有無を環境変数で渡す（X25519 と同じ作法）。これでバイナリの配置差による系統誤差を避け、
/// 固定した秘密と変動させた秘密を同じ実行ファイルで比べられる。
///
/// - `SIGIL_CT_CHACHA_KEY`: 32 バイトの鍵を、各半バイトを ASCII 0x40〜0x4f に写した 64 文字。
/// - `SIGIL_CT_CHACHA_MODE`: `ok`（seal の後に open が成功する）か `fail`（seal の後、タグの 1 バイトを変えて open が
///   `AuthenticationFailed` で失敗する）。
/// - `SIGIL_CT_CHACHA_POS`: `fail` のとき、改変するタグの位置（0〜15）を 0x40 + 位置の 1 文字で渡す。
/// - `SIGIL_CT_CHACHA_CONTROL`: `1` のとき陽性対照。`ok` では鍵の先頭バイトのビット 3 が立っているとき、`fail` では
///   改変位置が 15 のときだけ、暗号化をもう 1 回行う（秘密・不一致の位置に依存する仕事量の差）。
///
/// nonce・AAD・平文は両組で同じ（長さも同じ）。
fn chacha_program(iters: usize) -> String {
    format!(
        r#"module {{MODULE}}
  targets server
  trust none

fn main() -> Int
  intent "ChaCha20-Poly1305 の seal と open を計測する"
  requires none
  ensures none
  effects env
{{
  let key_env = env.get(&"SIGIL_CT_CHACHA_KEY");
  let mode_env = env.get(&"SIGIL_CT_CHACHA_MODE");
  let pos_env = env.get(&"SIGIL_CT_CHACHA_POS");
  let control_env = env.get(&"SIGIL_CT_CHACHA_CONTROL");
  let encoded = key_env.unwrap_or("").as_bytes();
  let fail_mode = mode_env.unwrap_or("") == "fail";
  let control = control_env.unwrap_or("") == "1";
  let pos = pos_env.unwrap_or("").as_bytes().get(0).unwrap_or(64) - 64;
  if encoded.byte_len() != 64 {{ return 1; }}
  let mut raw_key = Bytes.new();
  let mut i = 0;
  while i < 32 {{
    let hi = encoded.get(2 * i).unwrap_or(0) & 15;
    let lo = encoded.get(2 * i + 1).unwrap_or(0) & 15;
    raw_key.push((hi << 4) | lo);
    i = i + 1;
  }}
  let mut iv = Bytes.new();
  let mut aad = Bytes.new();
  let mut pt = Bytes.new();
  let mut j = 0;
  while j < 32 {{
    if j < 12 {{ iv.push(j + 1); }}
    if j < 8 {{ aad.push(j + 13); }}
    pt.push(j + 1);
    j = j + 1;
  }}
  let extra = control && match fail_mode {{ true => pos == 15, false => (raw_key.get(0).unwrap_or(0) & 8) != 0 }};
  match ChaCha20Poly1305Key.from_secret(Secret.from_bytes(raw_key)) {{
    Err(e) => {{ return 2; }}
    Ok(key) => {{
      let mut acc = 0;
      let mut n = 0;
      while n < {iters} {{
        match Nonce96.from_slice(&iv) {{
          Err(e) => {{ return 3; }}
          Ok(n1) => {{
            match Nonce96.from_slice(&iv) {{
              Err(e) => {{ return 3; }}
              Ok(n2) => {{
                match chacha20_poly1305_seal(&key, n1, &aad, &Secret.from_bytes(pt.clone())) {{
                  Err(e) => {{ return 4; }}
                  Ok(sealed) => {{
                    if fail_mode {{
                      let mut bad = sealed.clone();
                      let idx = 32 + pos;
                      let orig = bad.get(idx).unwrap_or(0);
                      match bad.set(idx, (orig + 1) % 256) {{
                        Err(e) => {{ return 5; }}
                        Ok(u) => {{}}
                      }}
                      match chacha20_poly1305_open(&key, &n2, &aad, &bad) {{
                        Err(e) => {{ acc = acc + 1; }}
                        Ok(o) => {{ return 6; }}
                      }}
                    }} else {{
                      match chacha20_poly1305_open(&key, &n2, &aad, &sealed) {{
                        Err(e) => {{ return 6; }}
                        Ok(o) => {{ acc = acc + 1; }}
                      }}
                    }}
                  }}
                }}
              }}
            }}
          }}
        }}
        if extra {{
          match Nonce96.from_slice(&iv) {{
            Err(e) => {{ return 3; }}
            Ok(n3) => {{
              match chacha20_poly1305_seal(&key, n3, &aad, &Secret.from_bytes(pt.clone())) {{
                Err(e) => {{ return 4; }}
                Ok(again) => {{ acc = acc + 0 * again.byte_len(); }}
              }}
            }}
          }}
        }}
        n = n + 1;
      }}
      return match acc == {iters} {{ true => 0, false => 7 }};
    }}
  }}
}}
"#
    )
}

/// 鍵・タグの位置の環境変数の符号化（各半バイトを ASCII 0x40〜0x4f に写す）。
fn chacha_key_env(key: &[u8; 32]) -> String {
    key.iter()
        .flat_map(|&byte| [char::from(64 + (byte >> 4)), char::from(64 + (byte & 15))])
        .collect()
}

/// 同じ実行ファイルを、組 A と組 B の環境で交互（順序は乱数）に `samples` 回ずつ実行して Welch の t を返す。
/// 通常系（`fail = false`）: 組 A は固定の鍵（全ゼロ）、組 B は毎回違う乱数の鍵。
/// 失敗系（`fail = true`）: 鍵は両組で同じ固定値で、組 A はタグの先頭（位置 0）、組 B はタグの末尾（位置 15）を改変する。
fn measure_chacha(exe: &PathBuf, samples: usize, fail: bool, control: bool) -> f64 {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let fixed_key: [u8; 32] = std::array::from_fn(|i| (i * 7 + 3) as u8);
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    let timed = |key: &str, pos: u8, measurements: &mut Vec<f64>| {
        let mut cmd = Command::new(exe);
        cmd.env("SIGIL_CT_CHACHA_KEY", key)
            .env("SIGIL_CT_CHACHA_MODE", if fail { "fail" } else { "ok" })
            .env("SIGIL_CT_CHACHA_POS", char::from(64 + pos).to_string())
            .env("SIGIL_CT_CHACHA_CONTROL", if control { "1" } else { "0" });
        let start = Instant::now();
        let output = cmd
            .output()
            .expect("ChaCha20-Poly1305 計測用プログラムを実行できない");
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(
            output.status.success() && output.stdout == b"0\n",
            "ChaCha20-Poly1305 計測用プログラムが失敗: {} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for _ in 0..samples {
        let (key_a, key_b) = if fail {
            (chacha_key_env(&fixed_key), chacha_key_env(&fixed_key))
        } else {
            let mut varied: [u8; 32] = std::array::from_fn(|_| next() as u8);
            if control {
                varied[0] |= 8;
            }
            (chacha_key_env(&[0; 32]), chacha_key_env(&varied))
        };
        let (pos_a, pos_b) = if fail { (0, 15) } else { (0, 0) };
        if next() & 1 == 0 {
            timed(&key_a, pos_a, &mut a);
            timed(&key_b, pos_b, &mut b);
        } else {
            timed(&key_b, pos_b, &mut b);
            timed(&key_a, pos_a, &mut a);
        }
    }
    welch_t(&a, &b)
}

fn chacha_measure(fail: bool, control: bool) -> (f64, usize, usize) {
    let iters = DEFAULT_AEAD_ITERS;
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let (dir, exe) = build_exe("chacha", &chacha_program(iters));
    let t = measure_chacha(&exe, samples, fail, control);
    let _ = std::fs::remove_dir_all(&dir);
    (t, samples, iters)
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_positive_control_chacha20_poly1305 -- --ignored --nocapture"]
fn ct_measure_positive_control_chacha20_poly1305() {
    let (t, samples, iters) = chacha_measure(false, true);
    println!(
        "陽性の対照（ChaCha20-Poly1305 形, 鍵に依存する仕事量, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "ChaCha20-Poly1305 陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_chacha20_poly1305 -- --ignored --nocapture"]
fn ct_measure_chacha20_poly1305() {
    let (t, samples, iters) = chacha_measure(false, false);
    println!(
        "ChaCha20-Poly1305 (chacha20_poly1305_seal + chacha20_poly1305_open, 固定の鍵と乱数の鍵, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() < 4.5,
        "ChaCha20-Poly1305 に時間の差が出た: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_positive_control_chacha20_poly1305_fail -- --ignored --nocapture"]
fn ct_measure_positive_control_chacha20_poly1305_fail() {
    let (t, samples, iters) = chacha_measure(true, true);
    println!(
        "陽性の対照（ChaCha20-Poly1305 認証失敗の形, タグの不一致位置に依存する仕事量, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "ChaCha20-Poly1305 認証失敗の陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

/// 認証失敗（ADR 0058 決定 3 の「同じ長さと同じ失敗結果で不一致の位置を変える」）。鍵・nonce・AAD・平文は両組で同じ。
/// 組 A はタグの先頭（位置 0）、組 B はタグの末尾（位置 15）の 1 バイトだけを変えて open に渡す。結果は同じ `AuthenticationFailed`。
/// 1 バイトの比較の打ち切りそのものは、この計測の分解能（暗号化 1 回分の仕事量）より小さい。その構造的な保証は
/// ct_rules（タグの照合は `ct_eq` だけを通り、秘密に依存する分岐を持たない）が担う。
#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_chacha20_poly1305_fail -- --ignored --nocapture"]
fn ct_measure_chacha20_poly1305_fail() {
    let (t, samples, iters) = chacha_measure(true, false);
    println!(
        "ChaCha20-Poly1305 認証失敗（タグの先頭と末尾の改変, chacha20_poly1305_open, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() < 4.5,
        "ChaCha20-Poly1305 の認証失敗にタグの不一致位置による時間の差が出た: t = {t:.2}"
    );
}

/// AES-256-GCM の計測用プログラム（ADR 0058 決定 3、段 2c）。両組で同じ実行ファイルを使い、鍵・タグの改変位置・
/// 陽性対照の有無を環境変数で渡す（X25519 と同じ作法）。これでバイナリの配置差による系統誤差を避け、
/// 固定した秘密と変動させた秘密を同じ実行ファイルで比べられる。
///
/// - `SIGIL_CT_AES256_KEY`: 32 バイトの鍵を、各半バイトを ASCII 0x40〜0x4f に写した 64 文字。
/// - `SIGIL_CT_AES256_MODE`: `ok`（seal の後に open が成功する）か `fail`（seal の後、タグの 1 バイトを変えて open が
///   `AuthenticationFailed` で失敗する）。
/// - `SIGIL_CT_AES256_POS`: `fail` のとき、改変するタグの位置（0〜15）を 0x40 + 位置の 1 文字で渡す。
/// - `SIGIL_CT_AES256_CONTROL`: `1` のとき陽性対照。`ok` では鍵の先頭バイトのビット 3 が立っているとき、`fail` では
///   改変位置が 15 のときだけ、暗号化をもう 1 回行う（秘密・不一致の位置に依存する仕事量の差）。
///
/// nonce・AAD・平文は両組で同じ（長さも同じ）。
fn aes256_program(iters: usize) -> String {
    format!(
        r#"module {{MODULE}}
  targets server
  trust none

fn main() -> Int
  intent "AES-256-GCM の seal と open を計測する"
  requires none
  ensures none
  effects env
{{
  let key_env = env.get(&"SIGIL_CT_AES256_KEY");
  let mode_env = env.get(&"SIGIL_CT_AES256_MODE");
  let pos_env = env.get(&"SIGIL_CT_AES256_POS");
  let control_env = env.get(&"SIGIL_CT_AES256_CONTROL");
  let encoded = key_env.unwrap_or("").as_bytes();
  let fail_mode = mode_env.unwrap_or("") == "fail";
  let control = control_env.unwrap_or("") == "1";
  let pos = pos_env.unwrap_or("").as_bytes().get(0).unwrap_or(64) - 64;
  if encoded.byte_len() != 64 {{ return 1; }}
  let mut raw_key = Bytes.new();
  let mut i = 0;
  while i < 32 {{
    let hi = encoded.get(2 * i).unwrap_or(0) & 15;
    let lo = encoded.get(2 * i + 1).unwrap_or(0) & 15;
    raw_key.push((hi << 4) | lo);
    i = i + 1;
  }}
  let mut iv = Bytes.new();
  let mut aad = Bytes.new();
  let mut pt = Bytes.new();
  let mut j = 0;
  while j < 32 {{
    if j < 12 {{ iv.push(j + 1); }}
    if j < 8 {{ aad.push(j + 13); }}
    pt.push(j + 1);
    j = j + 1;
  }}
  let extra = control && match fail_mode {{ true => pos == 15, false => (raw_key.get(0).unwrap_or(0) & 8) != 0 }};
  match Aes256GcmKey.from_secret(Secret.from_bytes(raw_key)) {{
    Err(e) => {{ return 2; }}
    Ok(key) => {{
      let mut acc = 0;
      let mut n = 0;
      while n < {iters} {{
        match Nonce96.from_slice(&iv) {{
          Err(e) => {{ return 3; }}
          Ok(n1) => {{
            match Nonce96.from_slice(&iv) {{
              Err(e) => {{ return 3; }}
              Ok(n2) => {{
                match aes256_gcm_seal(&key, n1, &aad, &Secret.from_bytes(pt.clone())) {{
                  Err(e) => {{ return 4; }}
                  Ok(sealed) => {{
                    if fail_mode {{
                      let mut bad = sealed.clone();
                      let idx = 32 + pos;
                      let orig = bad.get(idx).unwrap_or(0);
                      match bad.set(idx, (orig + 1) % 256) {{
                        Err(e) => {{ return 5; }}
                        Ok(u) => {{}}
                      }}
                      match aes256_gcm_open(&key, &n2, &aad, &bad) {{
                        Err(e) => {{ acc = acc + 1; }}
                        Ok(o) => {{ return 6; }}
                      }}
                    }} else {{
                      match aes256_gcm_open(&key, &n2, &aad, &sealed) {{
                        Err(e) => {{ return 6; }}
                        Ok(o) => {{ acc = acc + 1; }}
                      }}
                    }}
                  }}
                }}
              }}
            }}
          }}
        }}
        if extra {{
          match Nonce96.from_slice(&iv) {{
            Err(e) => {{ return 3; }}
            Ok(n3) => {{
              match aes256_gcm_seal(&key, n3, &aad, &Secret.from_bytes(pt.clone())) {{
                Err(e) => {{ return 4; }}
                Ok(again) => {{ acc = acc + 0 * again.byte_len(); }}
              }}
            }}
          }}
        }}
        n = n + 1;
      }}
      return match acc == {iters} {{ true => 0, false => 7 }};
    }}
  }}
}}
"#
    )
}

/// 鍵・タグの位置の環境変数の符号化（各半バイトを ASCII 0x40〜0x4f に写す）。
fn aes256_key_env(key: &[u8; 32]) -> String {
    key.iter()
        .flat_map(|&byte| [char::from(64 + (byte >> 4)), char::from(64 + (byte & 15))])
        .collect()
}

/// 同じ実行ファイルを、組 A と組 B の環境で交互（順序は乱数）に `samples` 回ずつ実行して Welch の t を返す。
/// 通常系（`fail = false`）: 組 A は固定の鍵（全ゼロ）、組 B は毎回違う乱数の鍵。
/// 失敗系（`fail = true`）: 鍵は両組で同じ固定値で、組 A はタグの先頭（位置 0）、組 B はタグの末尾（位置 15）を改変する。
fn measure_aes256(exe: &PathBuf, samples: usize, fail: bool, control: bool) -> f64 {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let fixed_key: [u8; 32] = std::array::from_fn(|i| (i * 7 + 3) as u8);
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    let timed = |key: &str, pos: u8, measurements: &mut Vec<f64>| {
        let mut cmd = Command::new(exe);
        cmd.env("SIGIL_CT_AES256_KEY", key)
            .env("SIGIL_CT_AES256_MODE", if fail { "fail" } else { "ok" })
            .env("SIGIL_CT_AES256_POS", char::from(64 + pos).to_string())
            .env("SIGIL_CT_AES256_CONTROL", if control { "1" } else { "0" });
        let start = Instant::now();
        let output = cmd
            .output()
            .expect("AES-256-GCM 計測用プログラムを実行できない");
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(
            output.status.success() && output.stdout == b"0\n",
            "AES-256-GCM 計測用プログラムが失敗: {} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for _ in 0..samples {
        let (key_a, key_b) = if fail {
            (aes256_key_env(&fixed_key), aes256_key_env(&fixed_key))
        } else {
            let mut varied: [u8; 32] = std::array::from_fn(|_| next() as u8);
            if control {
                varied[0] |= 8;
            }
            (aes256_key_env(&[0; 32]), aes256_key_env(&varied))
        };
        let (pos_a, pos_b) = if fail { (0, 15) } else { (0, 0) };
        if next() & 1 == 0 {
            timed(&key_a, pos_a, &mut a);
            timed(&key_b, pos_b, &mut b);
        } else {
            timed(&key_b, pos_b, &mut b);
            timed(&key_a, pos_a, &mut a);
        }
    }
    welch_t(&a, &b)
}

fn aes256_measure(fail: bool, control: bool) -> (f64, usize, usize) {
    let iters = DEFAULT_AEAD_ITERS;
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let (dir, exe) = build_exe("aes256", &aes256_program(iters));
    let t = measure_aes256(&exe, samples, fail, control);
    let _ = std::fs::remove_dir_all(&dir);
    (t, samples, iters)
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_positive_control_aes256_gcm -- --ignored --nocapture"]
fn ct_measure_positive_control_aes256_gcm() {
    let (t, samples, iters) = aes256_measure(false, true);
    println!(
        "陽性の対照（AES-256-GCM 形, 鍵に依存する仕事量, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "AES-256-GCM 陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_aes256_gcm -- --ignored --nocapture"]
fn ct_measure_aes256_gcm() {
    let (t, samples, iters) = aes256_measure(false, false);
    println!(
        "AES-256-GCM (aes256_gcm_seal + aes256_gcm_open, 固定の鍵と乱数の鍵, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(t.abs() < 4.5, "AES-256-GCM に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_positive_control_aes256_gcm_fail -- --ignored --nocapture"]
fn ct_measure_positive_control_aes256_gcm_fail() {
    let (t, samples, iters) = aes256_measure(true, true);
    println!(
        "陽性の対照（AES-256-GCM 認証失敗の形, タグの不一致位置に依存する仕事量, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "AES-256-GCM 認証失敗の陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

/// 認証失敗（ADR 0058 決定 3 の「同じ長さと同じ失敗結果で不一致の位置を変える」）。鍵・nonce・AAD・平文は両組で同じ。
/// 組 A はタグの先頭（位置 0）、組 B はタグの末尾（位置 15）の 1 バイトだけを変えて open に渡す。結果は同じ `AuthenticationFailed`。
/// 1 バイトの比較の打ち切りそのものは、この計測の分解能（暗号化 1 回分の仕事量）より小さい。その構造的な保証は
/// ct_rules（タグの照合は `ct_eq` だけを通り、秘密に依存する分岐を持たない）が担う。
#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_aes256_gcm_fail -- --ignored --nocapture"]
fn ct_measure_aes256_gcm_fail() {
    let (t, samples, iters) = aes256_measure(true, false);
    println!(
        "AES-256-GCM 認証失敗（タグの先頭と末尾の改変, aes256_gcm_open, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() < 4.5,
        "AES-256-GCM の認証失敗にタグの不一致位置による時間の差が出た: t = {t:.2}"
    );
}

/// ADR 0058 決定 3（段 2b）: SHA-384 と HKDF-SHA-384。同じ実行ファイルを環境変数で切り替える（秘密を変える計測を
/// 2 つの実行ファイルに分けると、配置の差が t に混ざるため。ChaCha20-Poly1305 と同じ作り）。
///
/// - `SIGIL_CT_SHA384_SECRET`: 秘密の 32 バイトを、各半バイトを ASCII 0x40〜0x4f に写した 64 文字。
/// - `SIGIL_CT_SHA384_MODE`: `sha`（`sha384`。秘密を 5 回繰り返した 160 バイトをメッセージにする）か、
///   `hkdf`（`hkdf_extract_sha384` と `hkdf_expand_sha384`。秘密を salt と IKM にし、96 バイトを出力する）。
/// - `SIGIL_CT_SHA384_CONTROL`: `1` のとき陽性対照。秘密の先頭バイトのビット 3 が立っているときだけ、同じ計算をもう 1 回
///   行う（秘密に依存する仕事量の差）。
///
/// メッセージ・salt・IKM・info・出力の長さは両組で同じ（公開の入力長を固定する）。
fn sha384_program(iters: usize) -> String {
    format!(
        r#"module {{MODULE}}
  targets server
  trust none

fn main() -> Int
  intent "SHA-384 と HKDF-SHA-384 を計測する"
  requires none
  ensures none
  effects env
{{
  let secret_env = env.get(&"SIGIL_CT_SHA384_SECRET");
  let mode_env = env.get(&"SIGIL_CT_SHA384_MODE");
  let control_env = env.get(&"SIGIL_CT_SHA384_CONTROL");
  let encoded = secret_env.unwrap_or("").as_bytes();
  let hkdf_mode = mode_env.unwrap_or("") == "hkdf";
  let control = control_env.unwrap_or("") == "1";
  if encoded.byte_len() != 64 {{ return 1; }}
  let mut sec = Bytes.new();
  let mut i = 0;
  while i < 32 {{
    let hi = encoded.get(2 * i).unwrap_or(0) & 15;
    let lo = encoded.get(2 * i + 1).unwrap_or(0) & 15;
    sec.push((hi << 4) | lo);
    i = i + 1;
  }}
  let mut msg = Bytes.new();
  let mut r = 0;
  while r < 5 {{
    msg.extend(&sec);
    r = r + 1;
  }}
  let mut info = Bytes.new();
  let mut j = 0;
  while j < 10 {{
    info.push(j + 1);
    j = j + 1;
  }}
  let extra = control && (sec.get(0).unwrap_or(0) & 8) != 0;
  let mut acc = 0;
  let mut n = 0;
  while n < {iters} {{
    if hkdf_mode {{
      let prk = hkdf_extract_sha384(Some(Secret.from_bytes(sec.clone())), &Secret.from_bytes(sec.clone()));
      let okm = hkdf_expand_sha384(&prk, &info, 96);
      acc = acc + okm.byte_len();
      if extra {{
        let prk2 = hkdf_extract_sha384(Some(Secret.from_bytes(sec.clone())), &Secret.from_bytes(sec.clone()));
        let okm2 = hkdf_expand_sha384(&prk2, &info, 96);
        acc = acc + 0 * okm2.byte_len();
      }}
    }} else {{
      let d = sha384(&msg);
      acc = acc + d.byte_len();
      if extra {{
        let d2 = sha384(&msg);
        acc = acc + 0 * d2.byte_len();
      }}
    }}
    n = n + 1;
  }}
  let want = match hkdf_mode {{ true => {iters} * 96, false => {iters} * 48 }};
  return match acc == want {{ true => 0, false => 7 }};
}}
"#
    )
}

/// 同じ実行ファイルを、組 A（固定の秘密: 全ゼロ）と組 B（毎回違う乱数の秘密）の環境で交互（順序は乱数）に
/// `samples` 回ずつ実行して Welch の t を返す。`mode` は `sha` か `hkdf`。
fn measure_sha384(exe: &PathBuf, samples: usize, mode: &str, control: bool) -> f64 {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut a = Vec::with_capacity(samples);
    let mut b = Vec::with_capacity(samples);
    let timed = |secret: &str, measurements: &mut Vec<f64>| {
        let mut cmd = Command::new(exe);
        cmd.env("SIGIL_CT_SHA384_SECRET", secret)
            .env("SIGIL_CT_SHA384_MODE", mode)
            .env("SIGIL_CT_SHA384_CONTROL", if control { "1" } else { "0" });
        let start = Instant::now();
        let output = cmd
            .output()
            .expect("SHA-384 計測用プログラムを実行できない");
        measurements.push(start.elapsed().as_secs_f64() * 1000.0);
        assert!(
            output.status.success() && output.stdout == b"0\n",
            "SHA-384 計測用プログラムが失敗: {} stdout={:?} stderr={:?}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    for _ in 0..samples {
        let mut varied: [u8; 32] = std::array::from_fn(|_| next() as u8);
        if control {
            varied[0] |= 8;
        }
        let secret_a = chacha_key_env(&[0; 32]);
        let secret_b = chacha_key_env(&varied);
        if next() & 1 == 0 {
            timed(&secret_a, &mut a);
            timed(&secret_b, &mut b);
        } else {
            timed(&secret_b, &mut b);
            timed(&secret_a, &mut a);
        }
    }
    welch_t(&a, &b)
}

/// SHA-384（160 バイト = 圧縮 2 回分）は 1 反復が SHA-256 の 3 倍ほど重い（64 ビット語を 32 ビットずつ足す）。
const DEFAULT_SHA384_ITERS: usize = 100;
/// HKDF-SHA-384（Extract + 2 ブロックの Expand）は HMAC-SHA-384 が 3 回分。
const DEFAULT_HKDF384_ITERS: usize = 30;

fn sha384_measure(mode: &str, control: bool) -> (f64, usize, usize) {
    let iters = match mode {
        "hkdf" => DEFAULT_HKDF384_ITERS,
        _ => DEFAULT_SHA384_ITERS,
    };
    let samples = get_samples(DEFAULT_HMAC_SAMPLES);
    let (dir, exe) = build_exe("sha384", &sha384_program(iters));
    let t = measure_sha384(&exe, samples, mode, control);
    let _ = std::fs::remove_dir_all(&dir);
    (t, samples, iters)
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_positive_control_sha384 -- --ignored --nocapture"]
fn ct_measure_positive_control_sha384() {
    let (t, samples, iters) = sha384_measure("sha", true);
    println!(
        "陽性の対照（SHA-384 形, 秘密に依存する仕事量, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "SHA-384 陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_sha384 -- --ignored --nocapture"]
fn ct_measure_sha384() {
    let (t, samples, iters) = sha384_measure("sha", false);
    println!(
        "SHA-384 (sha384, 160 バイトのメッセージ, 固定の秘密と乱数の秘密, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(t.abs() < 4.5, "SHA-384 に時間の差が出た: t = {t:.2}");
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_positive_control_hkdf_sha384 -- --ignored --nocapture"]
fn ct_measure_positive_control_hkdf_sha384() {
    let (t, samples, iters) = sha384_measure("hkdf", true);
    println!(
        "陽性の対照（HKDF-SHA-384 形, 秘密に依存する仕事量, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(
        t.abs() >= 4.5,
        "HKDF-SHA-384 陽性の対照で差が出なかった（計測の感度不足）: t = {t:.2}"
    );
}

#[test]
#[ignore = "手動計測（ADR 0058 決定 3）。cargo test --release --test ct_measure ct_measure_hkdf_sha384 -- --ignored --nocapture"]
fn ct_measure_hkdf_sha384() {
    let (t, samples, iters) = sha384_measure("hkdf", false);
    println!(
        "HKDF-SHA-384 (hkdf_extract_sha384 + hkdf_expand_sha384 の 96 バイト, 固定の秘密と乱数の秘密, 同一実行ファイル, 各組 {samples} 標本 × {iters} 回): t = {t:.2}"
    );
    assert!(t.abs() < 4.5, "HKDF-SHA-384 に時間の差が出た: t = {t:.2}");
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

/// X25519 の定数時間計測（ADR 0058 決定 3）。
/// 同一実行ファイルに環境変数でスカラーを渡すことでバイナリ配置差を回避している。
///
/// 感度の限界（Issue #702）:
/// 陽性対照は仕事量 +50%（公開鍵導出 1 回追加）で |t| >= 4.5 を超える感度を確認しているが、
/// 検出できるのは 50% 規模の差にとどまり、語単位の微小な値依存の差は検出できない可能性がある。
/// したがって本テストの合格は「大きな差は見つからなかった」ことを意味し、「定数時間を完全に示した」ではない。
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
    ct_measure_positive_control_chacha20_poly1305();
    ct_measure_chacha20_poly1305();
    ct_measure_positive_control_chacha20_poly1305_fail();
    ct_measure_chacha20_poly1305_fail();
    ct_measure_positive_control_aes256_gcm();
    ct_measure_aes256_gcm();
    ct_measure_positive_control_aes256_gcm_fail();
    ct_measure_aes256_gcm_fail();
    ct_measure_positive_control_sha384();
    ct_measure_sha384();
    ct_measure_positive_control_hkdf_sha384();
    ct_measure_hkdf_sha384();
}

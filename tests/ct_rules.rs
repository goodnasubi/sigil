//! 定数時間検査の土台（ADR 0024 決定 3、ADR 0020 決定 1。PR 6-1a）。
//!
//! 暗号の部品は、秘密の値によって実行時間が変わらないこと（定数時間）が要る。
//! このファイルは、その性質を機械的に確かめる 3 層のうち、テストスイートで
//! 常時動かす 2 層を実装する。
//!
//! 1. **対象表と漏れの検査**: 定数時間であるべき関数は `CT_FUNCTIONS` に
//!    明示する。ADR 0024 決定 3 の包括基準（`ct_` 接頭辞／`Secret` を含む
//!    署名／`std.crypto` の関数）に当たるのに対象表にない関数があれば、
//!    テストを失敗にする（載せ忘れの防止）。
//! 2. **静的検査 `ct_rules_static`**: lowering 後の SIR の上で、秘密の値の
//!    流れ（テイント）を追い、ADR 0020 決定 1 の規則 R1〜R6 への違反を
//!    検出する。
//! 3. **CLIF 検査 `ct_rules_clif`**: ネイティブ生成の途中の中間表現
//!    （`SIGIL_CLIF_DUMP`）に、除算命令が現れていないか、条件分岐が SIR の
//!    構造から説明できる数を超えていないかを確かめる。
//!
//! 統計的な計測（第 3 層）は `tests/ct_measure.rs`（`#[ignore]`、手動実行）。
//!
//! ## テイントの付け方の約束（モデリング）
//!
//! - 対象表で「公開」と書かれていない引数は、すべて秘密とする。
//! - **長さは公開情報**とする。`len`・`is_empty` の結果は、秘密の領域に
//!   対しても公開の値として扱う（ADR 0020 決定 1 の対象表のとおり）。
//! - **領域を経由する流れ**（ADR 0020 の必須事項）: 秘密の値を書いた
//!   `Bytes`・一覧・構造体などの領域から読んだ値は、読んだ位置によらず
//!   秘密とする。位置ごとに区別して追うことはしない。実装の上では、
//!   局所（領域の根）単位で「秘密を含む」を伝播させる。
//! - **raw 層の約束**: 標準ライブラリの `Bytes` の頭は「データ番地＋長さ」
//!   の 2 ワードで、`raw.load_i64`・`raw.load_addr` はその頭のワード
//!   （長さと番地＝公開の情報）を読むために使う。したがって、この 2 つの
//!   結果は公開とする。一方 `raw.load_u8` は中身のバイト列を読むので、
//!   秘密の領域から読んだ結果は秘密とする。将来、i64 の並び自体を秘密の
//!   中身として `load_i64` で読む関数が現れたら、この約束の見直しが要る。
//! - 比較の結果（`ct_eq` の戻り値など）は公開の値とする（ADR 0020）。
//!   ただし定数時間の関数の**中**で秘密由来の比較結果を分岐に使うことは、
//!   R1 として扱う（公開になるのは関数の外へ出た後）。
//!
//! ## CLIF 検査の方法と限界
//!
//! 現在のネイティブ生成は Cranelift の最適化水準が `none` で、組み立てた
//! CLIF がそのまま機械語になる。つまり「最適化後の IR」は現状では
//! 「組み立てた CLIF」と同じものである。CLIF の出力には関数名が付かない
//! ため、対象関数の寄与は差分で切り分ける: 入口が対象関数を呼ぶプログラム
//! と、呼ばないプログラムの両方を組み立て、命令数の差を対象関数の分と
//! みなす。対象関数がユーザー定義の関数を呼ばない場合にだけ、この切り
//! 分けは正確になる（このファイルの fixture はその形にしてある）。
//! 標準ライブラリの関数（`std.core.bytes.ct_equal`）については、静的検査
//! で全面を覆い、CLIF 検査は入口から直接呼ぶ形のプログラムで差分が
//! 1 関数分に収まることを確かめたうえで適用する。
//! 最適化水準を上げる変更を入れる場合は、この検査の前提（組み立てた
//! CLIF＝最終形）が崩れるので、ADR 0020 が検討項目とした「分岐の条件の
//! 出所を最適化後の IR で追う」方法への置き換えを検討すること。

use sigil::ct::clif::{
    count_clif, reach_branch_budget, reachable_funcs, sir_branch_budget, ClifCounts,
};
use sigil::ct::rules::PUBLIC_COMPARISONS as CT_PUBLIC_COMPARISONS;
use sigil::ct::rules::{analyze_with_table, Rule, Violation};
use sigil::ct::stdlib::STDLIB_TARGETS as CT_FUNCTIONS;
use sigil::ir::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// 対象表（ADR 0024 決定 3）
// ---------------------------------------------------------------------------
/// 対象表のうち、まだ実装が存在しない行（先行登録）。
/// 実装が現れたら、ここから外して通常の行として扱う。
const CT_AHEAD: &[&str] = &[];

// ---------------------------------------------------------------------------
// 対象表の検査（漏れの防止）
// ---------------------------------------------------------------------------

const X25519_PROBES: &str = r#"
fn x25519_probe_public(b: &Bytes) -> Int
  intent "X25519 の公開鍵導出を探査する"
  requires none
  ensures none
  effects none
{
  match X25519PrivateKey.from_secret(Secret.from_bytes(b.clone())) {
    Err(error) => { return match error.is_invalid_key_length() { true => 0, false => 1 }; }
    Ok(key) => { return x25519_public_key(&key).as_bytes().byte_len(); }
  }
}

fn x25519_probe_shared(b: &Bytes) -> Int
  intent "X25519 の共有値計算を探査する"
  requires none
  ensures none
  effects none
{
  match X25519PrivateKey.from_secret(Secret.from_bytes(b.clone())) {
    Err(_) => { return 0; }
    Ok(key) => {
      match X25519PublicKey.from_bytes(b.clone()) {
        Err(_) => { return 0; }
        Ok(peer) => {
          match x25519_shared_secret(&key, &peer) {
            Err(error) => { return match error.is_all_zero_shared_secret() { true => 0, false => 1 }; }
            Ok(shared) => { return shared.byte_len(); }
          }
        }
      }
    }
  }
}
"#;

/// 標準ライブラリを含むプログラム（コーパス）を 1 つ組み立てる。
/// 入口から届かない関数も含めて、標準ライブラリの関数がすべて SIR に現れる。
fn corpus() -> Program {
    // 埋込みモジュールの試験専用入口から本物の本体を呼び、DCE 後にも
    // SCRAM と既存暗号の対象を残す。製品の公開 API には入口を追加しない。
    let probe = r#"
fn probe() -> Int
  intent "定数時間対象を探査する"
  requires none
  ensures none
  effects task.suspend
{
  let b = Bytes.new();
  let s = Secret.from_bytes(Bytes.new());
  let inner = s.expose();
  let d = sha256(&b);
  let h = hmac_sha256(&s, &b);
  let ok = verify_hmac_sha256(&s, &b, &d);
  let p = pbkdf2_sha256(&s, &b, 4096, 32);
  let prk = hkdf_extract_sha256(None, &s);
  let okm = hkdf_expand_sha256(&prk, &b, 32);
  let d384 = sha384(&b);
  let prk384 = hkdf_extract_sha384(None, &s);
  let okm384 = hkdf_expand_sha384(&prk384, &b, 32);
  let aead = aead_probe(&b) + aead256_probe(&b) + chacha_probe(&b);
  let x25519_public = x25519_probe_public(&b);
  let x25519_shared = x25519_probe_shared(&b);
  let reply = respond(&s, &b, &b);
  let checked = verify(&s, &b);
  let initial = first(&b);
  let bearer = web.verify_bearer(&s, &"api", &b, 0);
  if ct_eq(&inner, &b) && ok { return s.byte_len() + d.byte_len() + d384.byte_len() + okm384.byte_len() + h.byte_len() + p.byte_len() + aead + x25519_public + x25519_shared; }
  return 0;
}

fn chacha_probe(b: &Bytes) -> Int
  intent "ChaCha20-Poly1305 の入口を探査する"
  requires none
  ensures none
  effects none
{
  let s = Secret.from_bytes(b.clone());
  match ChaCha20Poly1305Key.from_secret(Secret.from_bytes(s.expose())) {
    Err(e) => { return 0; }
    Ok(key) => {
      match Nonce96.from_slice(&b) {
        Err(e) => { return 0; }
        Ok(nonce) => {
          match Nonce96.from_slice(&b) {
            Err(e) => { return 0; }
            Ok(nonce2) => {
              match chacha20_poly1305_seal(&key, nonce, &b, &s) {
                Err(e) => { return 0; }
                Ok(sealed) => {
                  match chacha20_poly1305_open(&key, &nonce2, &b, &sealed) {
                    Err(e) => { return 0; }
                    Ok(opened) => { return opened.byte_len(); }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}

fn aead_probe(b: &Bytes) -> Int
  intent "AES-128-GCM の入口を探査する"
  requires none
  ensures none
  effects none
{
  let s = Secret.from_bytes(b.clone());
  match Aes128GcmKey.from_secret(Secret.from_bytes(s.expose())) {
    Err(e) => { return 0; }
    Ok(key) => {
      match Nonce96.from_slice(&b) {
        Err(e) => { return 0; }
        Ok(nonce) => {
          match Nonce96.from_slice(&b) {
            Err(e) => { return 0; }
            Ok(nonce2) => {
              match aes128_gcm_seal(&key, nonce, &b, &s) {
                Err(e) => { return 0; }
                Ok(sealed) => {
                  match aes128_gcm_open(&key, &nonce2, &b, &sealed) {
                    Err(e) => { return 0; }
                    Ok(opened) => { return opened.byte_len(); }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}

fn aead256_probe(b: &Bytes) -> Int
  intent "AES-256-GCM の入口を探査する"
  requires none
  ensures none
  effects none
{
  let s = Secret.from_bytes(b.clone());
  match Aes256GcmKey.from_secret(Secret.from_bytes(s.expose())) {
    Err(e) => { return 0; }
    Ok(key) => {
      match Nonce96.from_slice(&b) {
        Err(e) => { return 0; }
        Ok(nonce) => {
          match Nonce96.from_slice(&b) {
            Err(e) => { return 0; }
            Ok(nonce2) => {
              match aes256_gcm_seal(&key, nonce, &b, &s) {
                Err(e) => { return 0; }
                Ok(sealed) => {
                  match aes256_gcm_open(&key, &nonce2, &b, &sealed) {
                    Err(e) => { return 0; }
                    Ok(opened) => { return opened.byte_len(); }
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}
"#;
    let src = "module ctprobe\n  targets server\n  trust none\npub fn main() -> Int\n  intent \"探査\"\n  requires none\n  ensures none\n  effects none\n{ return 0; }\n";
    let opts = sigil::effects::Options {
        embedded_extra: vec![(
            "std.pg_scram".into(),
            format!(
                "{}\n{probe}\n{X25519_PROBES}",
                include_str!("../src/stdlib/pg_scram.sigil")
            ),
        )],
        embedded_roots: vec!["std.pg_scram".into()],
        ..Default::default()
    };
    sigil::ir_eval::lower_sources_opts(&[src], "std.pg_scram.probe", &opts)
        .expect("コーパスの組み立てに失敗")
}

/// ADR 0024 決定 3 の包括基準に当たる関数かどうか。
fn is_ct_candidate(program: &Program, func: &Func) -> bool {
    let last = func.name.rsplit('.').next().unwrap_or(&func.name);
    if last.starts_with("ct_") {
        return true;
    }
    if func.name.starts_with("std.crypto.") {
        return true;
    }
    // 引数・戻り値に `Secret` 型を含む。
    let mentions_secret = |ty: &Ty| -> bool {
        let mut t = ty;
        loop {
            match t {
                Ty::Adt(id) => {
                    // 秘密かどうかは lower が付けた同一性の印で見る
                    // （名前で比べると利用者の同名の型を拾ってしまう）。
                    return program.types.get(*id).map(|d| d.secret).unwrap_or(false);
                }
                Ty::Ref(inner) | Ty::MutRef(inner) => t = inner,
                _ => return false,
            }
        }
    };
    func.params
        .iter()
        .any(|&l| mentions_secret(&func.locals[l].ty))
        || mentions_secret(&func.ret)
}

#[test]
fn table_is_well_formed() {
    let names: Vec<&str> = CT_FUNCTIONS.iter().map(|(n, _)| *n).collect();
    let set: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), set.len(), "対象表に関数名の重なりがある");
    for name in CT_PUBLIC_COMPARISONS {
        assert!(set.contains(name), "公開比較 {name} は CT 表に必要");
        let program = corpus();
        let func = program.funcs.iter().find(|f| f.name == *name).unwrap();
        assert_eq!(func.ret, Ty::Bool, "公開できる比較結果は Bool だけ");
    }
    for ahead in CT_AHEAD {
        assert!(set.contains(ahead), "先行登録 {ahead} が対象表にない");
    }
}

#[test]
fn completeness_no_missing_targets() {
    let program = corpus();
    let table: BTreeSet<&str> = CT_FUNCTIONS.iter().map(|(n, _)| *n).collect();
    let mut missing = Vec::new();
    for func in &program.funcs {
        if is_ct_candidate(&program, func) && !table.contains(func.name.as_str()) {
            missing.push(func.name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "包括基準に当たるのに対象表にない関数がある: {missing:?}"
    );
    // 対象表の行のうち、実装が存在するものは、公開の引数名が実在することを確かめる。
    for (name, public) in CT_FUNCTIONS {
        if CT_AHEAD.contains(name) {
            continue;
        }
        let func = program
            .funcs
            .iter()
            .find(|f| f.name == *name)
            .unwrap_or_else(|| panic!("対象表の {name} がコーパスに見つからない"));
        let params: BTreeSet<&str> = func
            .params
            .iter()
            .map(|&l| func.locals[l].name.as_str())
            .collect();
        for p in *public {
            assert!(params.contains(p), "{name} の公開引数 {p} が実在しない");
        }
    }
}

// ---------------------------------------------------------------------------
// 静的検査: 実在の定数時間関数
// ---------------------------------------------------------------------------

#[test]
fn static_real_targets_are_clean() {
    let program = corpus();
    let violations = analyze_with_table(&program, CT_FUNCTIONS);
    assert!(
        violations.is_empty(),
        "定数時間の関数に規則違反がある: {violations:?}"
    );
}

// ---------------------------------------------------------------------------
// 静的検査: fixture（違反を埋め込んだ関数と、正しい関数）
// ---------------------------------------------------------------------------

const FIX_HEAD: &str = "module ctfix\n  targets server\n  trust none\n\n";

fn fx(sig: &str, body: &str) -> String {
    fx_fx(sig, "none", body)
}

fn fx_fx(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"fixture\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

/// fixture のプログラムを組み立てる。lowering は入口から届く関数だけを
/// 残すので、`main_body` から対象の関数を呼ぶ形にすること。
fn fixture_program(defs: &str, main_body: &str) -> Program {
    fixture_program_fx(defs, main_body, "none")
}

fn fixture_program_fx(defs: &str, main_body: &str, main_effects: &str) -> Program {
    let src = format!(
        "{FIX_HEAD}{defs}{}",
        fx_fx("main() -> Int", main_effects, main_body)
    );
    sigil::ir_eval::lower_sources(&[&src], "main")
        .unwrap_or_else(|e| panic!("fixture の組み立てに失敗: {e}\n{src}"))
}

fn rules_of(violations: &[(String, Violation)], fname: &str) -> Vec<Rule> {
    violations
        .iter()
        .filter(|(n, _)| n == fname)
        .map(|(_, v)| v.rule)
        .collect()
}

#[test]
fn static_fixture_clean_functions_pass() {
    // 正しい形: 長さ（公開）で回し、バイトを XOR で積む。分岐は長さだけ。
    let defs = format!(
        "{}{}",
        fx(
            "ct_ok(secret: Bytes) -> Int",
            "  let mut acc = 0;\n  let mut i = 0;\n  while i < secret.byte_len() {\n    acc = acc ^ secret.get(i).unwrap_or(0);\n    i = i + 1;\n  }\n  return acc;"
        ),
        fx(
            "ct_wrap(secret: Bytes) -> Bool",
            "  return ct_eq(&secret, &secret);"
        ),
    );
    let program = fixture_program(
        &defs,
        "  let a = ct_ok(Bytes.new());
  let b = ct_wrap(Bytes.new());
  return a;",
    );
    let table: &[(&str, &[&str])] = &[("ct_ok", &[]), ("ct_wrap", &[])];
    let violations = analyze_with_table(&program, table);
    assert!(
        violations.is_empty(),
        "正しい fixture に違反が出た: {violations:?}"
    );
}

#[test]
fn static_fixture_violations_are_caught() {
    let defs = format!(
        "{}{}{}{}{}{}",
        fx(
            "ct_bad_branch(secret: Bytes) -> Int",
            "  if secret.get(0).unwrap_or(0) == 1 {\n    return 1;\n  }\n  return 0;"
        ),
        fx(
            "ct_bad_region(secret: Bytes) -> Int",
            "  let mut copy = Bytes.new();\n  let mut i = 0;\n  while i < secret.byte_len() {\n    copy.push(secret.get(i).unwrap_or(0));\n    i = i + 1;\n  }\n  if copy.get(0).unwrap_or(0) == 1 {\n    return 1;\n  }\n  return 0;"
        ),
        fx(
            "ct_bad_index(secret: Bytes, table: Bytes) -> Int",
            "  return table.get(secret.get(0).unwrap_or(0)).unwrap_or(0);"
        ),
        fx(
            "ct_bad_rem(secret: Int, m: Int) -> Int",
            "  return secret % m;"
        ),
        fx(
            "leak(x: Int) -> Int",
            "  return x + 1;"
        ),
        fx(
            "ct_bad_call(secret: Int) -> Int",
            "  return leak(secret);"
        ),
    );
    let defs = format!(
        "{defs}{}",
        fx_fx(
            "ct_bad_log(secret: Bytes) -> Int",
            "console",
            "  console.log(&secret.get(0).unwrap_or(0));\n  return 0;"
        )
    );
    let program = fixture_program_fx(
        &defs,
        "  let a = ct_bad_branch(Bytes.new());
  let b = ct_bad_region(Bytes.new());
  let c = ct_bad_index(Bytes.new(), Bytes.new());
  let d = ct_bad_rem(7, 3);
  let e = ct_bad_call(7);
  let f = ct_bad_log(Bytes.new());
  return a + b + c + d + e + f;",
        "console",
    );
    let table: &[(&str, &[&str])] = &[
        ("ct_bad_branch", &[]),
        ("ct_bad_region", &[]),
        ("ct_bad_index", &[]),
        ("ct_bad_rem", &["m"]),
        ("ct_bad_call", &[]),
        ("ct_bad_log", &[]),
    ];
    let v = analyze_with_table(&program, table);
    assert!(
        rules_of(&v, "ct_bad_branch").contains(&Rule::R1Branch),
        "R1 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_region").contains(&Rule::R1Branch),
        "領域経由の R1 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_index").contains(&Rule::R2Index),
        "R2 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_rem").contains(&Rule::R3Div),
        "R3 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_call").contains(&Rule::R4Call),
        "R4 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_log").contains(&Rule::R6Output),
        "R6 を検出できない: {v:?}"
    );
    // 違反のない関数 leak 自体は対象外なので、違反は報告されない。
    assert!(rules_of(&v, "leak").is_empty());
}

// ---------------------------------------------------------------------------
// CLIF 検査（ct_rules_clif）
// ---------------------------------------------------------------------------

static CLIF_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `sigil build` を `SIGIL_CLIF_DUMP` つきで実行し、CLIF 出力を得る。
/// ソースはリポジトリの根の下に置かないと組み立てられないため、
/// `target/` の下に一時置き場を作る（モジュール名はパスから決まる）。
fn build_dump(tag: &str, module: &str, src: &str) -> String {
    let seq = CLIF_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(format!(
        "target/ct_rules_{}_{}_{}",
        std::process::id(),
        tag,
        seq
    ));
    std::fs::create_dir_all(&dir).expect("一時置き場を作れない");
    let file = dir.join(format!("{module}.sigil"));
    let full = src.replace(
        "{MODULE}",
        &format!(
            "target.{}.{module}",
            dir.file_name().unwrap().to_string_lossy()
        ),
    );
    std::fs::write(&file, full).expect("fixture を書けない");
    let out = dir.join("prog");
    let output = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("build")
        .arg(&file)
        .arg("main")
        .arg("-o")
        .arg(&out)
        .env("SIGIL_CLIF_DUMP", "1")
        .output()
        .expect("sigil build を実行できない");
    let dump = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "sigil build が失敗: {}",
        dump.chars().take(400).collect::<String>()
    );
    let _ = std::fs::remove_dir_all(&dir);
    dump
}

/// 対象関数を呼ぶプログラムと呼ばないプログラムの CLIF の差分。
/// 対象関数がユーザー定義の関数を呼ばない場合、その差分が対象関数の
/// 命令数になる（ファイル冒頭の「方法と限界」を参照）。
fn clif_delta(tag: &str, defs: &str, call: &str) -> ClifCounts {
    let with = format!(
        "module {{MODULE}}\n  targets server\n  trust none\n\n{defs}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return {call};\n}}\n"
    );
    let without = format!(
        "module {{MODULE}}\n  targets server\n  trust none\n\n{defs}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 0;\n}}\n"
    );
    let a = count_clif(&build_dump(tag, "a", &with));
    let b = count_clif(&build_dump(tag, "b", &without));
    ClifCounts {
        branch: a.branch.saturating_sub(b.branch),
        div: a.div.saturating_sub(b.div),
        store: a.store.saturating_sub(b.store),
    }
}

#[test]
fn clif_fixture_ct_ok_is_clean() {
    let defs = "fn ct_ok(secret: Bytes) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut acc = 0;\n  let mut i = 0;\n  while i < secret.byte_len() {\n    acc = acc ^ secret.get(i).unwrap_or(0);\n    i = i + 1;\n  }\n  return acc;\n}\n\n";
    let delta = clif_delta("ctok", defs, "ct_ok(Bytes.new())");
    assert_eq!(delta.div, 0, "定数時間の fixture に除算命令がある");
    // SIR 側の分岐予算と比較する（対象関数の SIR を組み立て直して数える）。
    let src = format!(
        "{FIX_HEAD}{defs}{}",
        fx("main() -> Int", "  return ct_ok(Bytes.new());")
    );
    let program = sigil::ir_eval::lower_sources(&[&src], "main").expect("組み立て失敗");
    let f = program.funcs.iter().find(|f| f.name == "ct_ok").unwrap();
    let budget = sir_branch_budget(f);
    assert!(
        delta.branch >= 1,
        "差分が取れていない（帰属の前提が崩れた）: {delta:?}"
    );
    assert!(
        delta.branch <= budget,
        "CLIF の分岐 {0} が SIR の予算 {budget} を超えた: {delta:?}",
        delta.branch
    );
}

#[test]
fn clif_detects_division() {
    let defs = "fn with_rem(a: Int, b: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a % b;\n}\n\n";
    let delta = clif_delta("rem", defs, "with_rem(7, 3)");
    assert!(delta.div >= 1, "除算命令を検出できない: {delta:?}");
}

#[test]
fn clif_real_ct_equal_is_clean() {
    // 標準ライブラリの ct_equal（ct_eq の実体）を入口から呼ぶプログラムで、
    // 差分がその関数の分だけになることを使って確かめる。
    let defs = "";
    let delta = clif_delta(
        "cteq",
        defs,
        "match ct_eq(&Bytes.new(), &Bytes.new()) { true => 1, false => 0 }",
    );
    assert_eq!(delta.div, 0, "ct_equal に除算命令がある: {delta:?}");
    let program = corpus();
    let f = program
        .funcs
        .iter()
        .find(|f| f.name == "std.core.bytes.ct_equal")
        .unwrap();
    let budget = sir_branch_budget(f);
    assert!(
        delta.branch <= budget,
        "ct_equal の CLIF 分岐 {0} が SIR の予算 {budget} を超えた: {delta:?}",
        delta.branch
    );
}

#[test]
fn clif_scram_proof_and_signature_have_no_division() {
    // 本番と同じ本体を抽出する。IR 最適化が none の現行生成系では、
    // この CLIF が最終形。分岐の秘密依存は上の SIR テイント検査が担う。
    let source = include_str!("../src/stdlib/pg_scram.sigil");
    let start = source.find("pub fn ct_proof(").unwrap();
    let end = source[start..].find("pub fn respond(").unwrap() + start;
    let defs = &source[start..end];
    for (tag, call) in [
        (
            "scramproof",
            "ct_proof(&Bytes.new(), &Bytes.new()).byte_len()",
        ),
        (
            "scramsignature",
            "match ct_signature(&Bytes.new(), &Bytes.new()) { true => 1, false => 0 }",
        ),
    ] {
        let delta = clif_delta(tag, defs, call);
        assert_eq!(delta.div, 0, "SCRAM の CLIF に除算命令がある: {delta:?}");
    }
}

#[test]
fn static_arbitrary_bool_return_is_not_a_public_comparison() {
    // Bool という型だけで秘密由来の判定を公開しない。明示した照合入口以外は
    // 関数外でもテイントが残り、結果による分岐は引き続き R1 になる。
    let defs = format!(
        "{}{}",
        fx("ct_predicate(secret: Int) -> Bool", "return secret == 1;"),
        fx(
            "ct_caller(secret: Int) -> Int",
            "if ct_predicate(secret) { return 1; } return 0;"
        )
    );
    let program = fixture_program(&defs, "return ct_caller(7);");
    let violations = analyze_with_table(&program, &[("ct_predicate", &[]), ("ct_caller", &[])]);
    assert!(rules_of(&violations, "ct_caller").contains(&Rule::R1Branch));
}

#[test]
fn clif_bearer_signature_adds_only_public_contract_branches() {
    // HMAC と SHA は両側で同じ本体を生成する。差分が新しいラッパーだけに
    // なるよう、基準側も verify_hmac_sha256 を直接呼ぶ（除算は公開の長さの
    // padding に既存のものがあるため、推移的な全命令数をゼロとはしない）。
    let source = include_str!("../src/stdlib/web.sigil");
    let start = source.find("fn ct_bearer_signature(").unwrap();
    let end = source[start..].find("fn bearer_digit(").unwrap() + start;
    let defs = &source[start..end];
    let make = |call: &str| {
        format!(
        "module {{MODULE}}\n  targets server\n  trust none\n{defs}\nfn main() -> Int\n  intent \"CT 境界\"\n  requires none\n  ensures none\n  effects none\n{{ let key = Secret.from_bytes(Bytes.new()); let b = Bytes.new(); return match {call}(&key, &b, &b) {{ true => 1, false => 0 }}; }}"
    )
    };
    let wrapped = count_clif(&build_dump("bearerwrap", "a", &make("ct_bearer_signature")));
    let direct = count_clif(&build_dump(
        "bearerdirect",
        "b",
        &make("verify_hmac_sha256"),
    ));
    let program = corpus();
    let wrapper = program
        .funcs
        .iter()
        .find(|f| f.name == "std.web.ct_bearer_signature")
        .unwrap();
    assert_eq!(wrapped.div, direct.div, "ラッパーが除算を追加した");
    assert!(
        // 追加した関数の入口に guard_stack の公開スタック境界分岐が一つある
        // （src/codegen/func/drop.rs）。SIR の項にはこの入口分岐がない。
        wrapped.branch <= direct.branch + sir_branch_budget(wrapper) + 1,
        "説明できない CLIF 分岐: {wrapped:?}, {direct:?}"
    );
}

// ---------------------------------------------------------------------------
// AES-128-GCM（ADR 0058 決定 3、段 1b）の CLIF 検査
// ---------------------------------------------------------------------------

/// S-box と GF(2^128) の乗算は、除算命令を持たず、条件分岐の数が SIR の項（ループの
/// 公開の継続条件と、検査つき演算・呼び出しの異常停止）で説明できる範囲に収まる。
/// 表引きがあれば番地計算に現れるが、SIR のテイント検査（`static_real_targets_are_clean`）が
/// 秘密による添字を R2 として拒否するため、ここでは分岐と除算の増えなさを見る。
#[test]
fn clif_aes_sbox_and_ghash_mul_have_no_division_and_bounded_branches() {
    let source = include_str!("../src/stdlib/crypto.sigil");
    let program = corpus();
    // SIR の項から見積もる分岐の上限。分岐・検査つき演算・表明は 1 つ、呼び出しは
    // 呼び出し後の異常停止の伝播の確認を含めて 2 つ、各関数の入口に guard_stack の
    // 公開スタック境界分岐が 1 つ、入口の main からの呼び出しが 2 つ。
    let budget = |names: &[&str]| -> usize {
        2 + names
            .iter()
            .map(|n| {
                let f = program
                    .funcs
                    .iter()
                    .find(|f| f.name == format!("std.crypto.{n}"))
                    .unwrap_or_else(|| panic!("{n} がコーパスにない"));
                1 + f
                    .blocks
                    .iter()
                    .map(|b| match b.term {
                        Term::Branch(..)
                        | Term::Switch(..)
                        | Term::CheckedOp { .. }
                        | Term::Assert { .. } => 1,
                        Term::Call { .. } => 2,
                        _ => 0,
                    })
                    .sum::<usize>()
            })
            .sum::<usize>()
    };
    let slice = |from: &str, to: &str| -> String {
        let a = source.find(from).unwrap();
        let b = source[a..].find(to).unwrap() + a;
        source[a..b].to_string()
    };
    let sbox_defs = slice("fn aes_xtime(", "fn aes_expand_key(");
    let ghash_defs = slice("fn ghash_mul(", "fn ghash_words(");
    // GHASH の乗算は `List` の読み書き（組み込みの部品）を呼ぶため、その展開の分岐を
    // `reach_branch_budget` の組み込み呼び出しの許容で見積もる（以前は見積もれず無効だった）。
    for (tag, defs, call, names, check_branches) in [
        (
            "aessbox",
            sbox_defs,
            "aes_sbox(3)".to_string(),
            vec!["aes_xtime", "aes_gmul", "aes_sbox"],
            true,
        ),
        (
            "ghashmul",
            ghash_defs,
            "ghash_mul(&List.new(), &List.new()).len()".to_string(),
            vec!["ghash_mul"],
            true,
        ),
    ] {
        let delta = clif_delta(tag, &defs, &call);
        assert_eq!(delta.div, 0, "{tag} の CLIF に除算命令がある: {delta:?}");
        let limit = if tag == "ghashmul" {
            2 + reach_branch_budget(&program, &reachable_funcs(&program, "std.crypto.ghash_mul"))
        } else {
            budget(&names)
        };
        assert!(
            !check_branches || delta.branch <= limit,
            "{tag} の CLIF 分岐 {} が SIR の予算 {limit} を超えた: {delta:?}",
            delta.branch
        );
    }
}

/// AES-128-GCM の公開入口（seal・open）から呼ばれる関数すべて（鍵展開・ブロック暗号化・
/// CTR・GHASH・タグ生成・照合・標準ライブラリの部品）を、ネイティブ生成した CLIF で検査する。
/// 差分の方法（入口を呼ぶ版と呼ばない版の CLIF の差）は、入口が推移的に呼ぶ全命令を数える。
/// 見るのは次の 3 つ。
/// 1. SIR で、入口から届く除算・剰余の除数がすべて定数（秘密を除数にしない）。
/// 2. CLIF の除算・剰余命令の数が、SIR の除算・剰余の数を超えない（生成が足さない）。
/// 3. CLIF の条件分岐の数が、届く関数すべての SIR の項から説明できる範囲に収まる
///    （関数ごとに入口の公開スタック境界分岐が 1 つ、分岐・検査つき演算・表明は 1 つ、
///    呼び出しは 2 つ、破棄は 1 つ）。
#[test]
fn clif_aes_gcm_entries_cover_all_callees() {
    let key = "match Aes128GcmKey.from_secret(Secret.from_bytes(Bytes.new())) { Err(e) => 0, Ok(key) => match Nonce96.from_slice(&Bytes.new()) { Err(e) => 0, Ok(nonce) => ";
    let seal = format!("{key}match aes128_gcm_seal(&key, nonce, &Bytes.new(), &Secret.from_bytes(Bytes.new())) {{ Err(e) => 0, Ok(sealed) => sealed.byte_len() }} }} }}");
    let open = format!("{key}match aes128_gcm_open(&key, &nonce, &Bytes.new(), &Bytes.new()) {{ Err(e) => 0, Ok(opened) => opened.byte_len() }} }} }}");
    check_aead_clif_entries(&[
        ("aesgcmseal", &["std.crypto.aes128_gcm_seal"], seal),
        ("aesgcmopen", &["std.crypto.aes128_gcm_open"], open),
    ]);
}

/// AES-256-GCM（ADR 0058 段 2c）も同じ検査。鍵展開・ブロック暗号化・GHASH は AES-128-GCM と
/// 共通の関数で、14 ラウンドと 15 本のラウンド鍵（`nk = 8` の追加 SubWord を含む）まで、
/// 公開入口が推移的に呼ぶ全関数を覆う。
#[test]
fn clif_aes256_gcm_entries_cover_all_callees() {
    let key = "match Aes256GcmKey.from_secret(Secret.from_bytes(Bytes.new())) { Err(e) => 0, Ok(key) => match Nonce96.from_slice(&Bytes.new()) { Err(e) => 0, Ok(nonce) => ";
    let seal = format!("{key}match aes256_gcm_seal(&key, nonce, &Bytes.new(), &Secret.from_bytes(Bytes.new())) {{ Err(e) => 0, Ok(sealed) => sealed.byte_len() }} }} }}");
    let open = format!("{key}match aes256_gcm_open(&key, &nonce, &Bytes.new(), &Bytes.new()) {{ Err(e) => 0, Ok(opened) => opened.byte_len() }} }} }}");
    check_aead_clif_entries(&[
        ("aes256seal", &["std.crypto.aes256_gcm_seal"], seal),
        ("aes256open", &["std.crypto.aes256_gcm_open"], open),
    ]);
}

/// ChaCha20-Poly1305（ADR 0058 段 2a）も同じ検査で、ChaCha20 のブロック関数・XOR、Poly1305 の
/// 更新・MAC、タグの組み立て・照合まで、公開入口が推移的に呼ぶ全関数を覆う。
#[test]
fn clif_chacha20_poly1305_entries_cover_all_callees() {
    let key = "match ChaCha20Poly1305Key.from_secret(Secret.from_bytes(Bytes.new())) { Err(e) => 0, Ok(key) => match Nonce96.from_slice(&Bytes.new()) { Err(e) => 0, Ok(nonce) => ";
    let seal = format!("{key}match chacha20_poly1305_seal(&key, nonce, &Bytes.new(), &Secret.from_bytes(Bytes.new())) {{ Err(e) => 0, Ok(sealed) => sealed.byte_len() }} }} }}");
    let open = format!("{key}match chacha20_poly1305_open(&key, &nonce, &Bytes.new(), &Bytes.new()) {{ Err(e) => 0, Ok(opened) => opened.byte_len() }} }} }}");
    check_aead_clif_entries(&[
        ("chachaseal", &["std.crypto.chacha20_poly1305_seal"], seal),
        ("chachaopen", &["std.crypto.chacha20_poly1305_open"], open),
    ]);
}

/// SHA-384 と HKDF-SHA-384（ADR 0058 段 2b）も同じ検査で、64 ビット語の加算・圧縮・HMAC・
/// Extract・Expand まで、公開入口が推移的に呼ぶ全関数を覆う。
#[test]
fn clif_sha384_and_hkdf_entries_cover_all_callees() {
    let extract = "hkdf_extract_sha384(None, &Secret.from_bytes(Bytes.new()))";
    check_aead_clif_entries(&[
        (
            "sha384",
            &["std.crypto.sha384"],
            "sha384(&Bytes.new()).byte_len()".to_string(),
        ),
        (
            "hkdf384",
            &[
                "std.crypto.hkdf_extract_sha384",
                "std.crypto.hkdf_expand_sha384",
            ],
            format!("hkdf_expand_sha384(&{extract}, &Bytes.new(), 32).byte_len()"),
        ),
    ]);
}

fn check_aead_clif_entries(cases: &[(&str, &[&str], String)]) {
    let program = corpus();
    for (tag, entries, call) in cases {
        // 入口が複数のとき（Extract と Expand を 1 つの式で呼ぶ場合）は、届く関数の和集合を見る。
        let mut reach: Vec<usize> = Vec::new();
        for entry in entries.iter() {
            reach.extend(reachable_funcs(&program, entry));
        }
        reach.sort_unstable();
        reach.dedup();
        let mut sir_div = 0usize;
        let budget = reach_branch_budget(&program, &reach);
        for id in &reach {
            let f = &program.funcs[*id];
            for b in &f.blocks {
                if let Term::CheckedOp {
                    op: ArithOp::Div | ArithOp::Rem,
                    b: divisor,
                    ..
                } = &b.term
                {
                    sir_div += 1;
                    if f.name.starts_with("std.crypto.") {
                        assert!(
                            matches!(divisor, Operand::Const(_)),
                            "{} に定数でない除数の除算・剰余がある",
                            f.name
                        );
                    }
                }
            }
        }
        let delta = clif_delta(tag, "", call);
        eprintln!(
            "{tag}: funcs={} sir_div={sir_div} budget={budget} {delta:?}",
            reach.len()
        );
        assert!(
            delta.div <= sir_div,
            "{tag} の CLIF の除算 {} が SIR の {sir_div} を超えた: {delta:?}",
            delta.div
        );
        assert!(
            delta.branch <= budget,
            "{tag} の CLIF 分岐 {} が SIR の予算 {budget} を超えた: {delta:?}",
            delta.branch
        );
    }
}

/// 公開入口から届く体演算を含め、ネイティブ生成が SIR にない分岐と
/// 除算・剰余を持ち込んでいないことを確認する。
#[test]
fn clif_x25519_entries_cover_all_callees() {
    let program = corpus();
    for (tag, probe) in [
        ("x25519public", "x25519_probe_public"),
        ("x25519shared", "x25519_probe_shared"),
    ] {
        let reach = reachable_funcs(&program, &format!("std.pg_scram.{probe}"));
        let budget = reach_branch_budget(&program, &reach);
        let delta = clif_delta(tag, X25519_PROBES, &format!("{probe}(&Bytes.new())"));
        eprintln!("{tag}: funcs={} budget={budget} {delta:?}", reach.len());
        assert_eq!(delta.div, 0, "{tag} の CLIF に除算・剰余がある");
        assert!(
            delta.branch <= budget,
            "{tag} の CLIF 分岐 {} が SIR の予算 {budget} を超えた: {delta:?}",
            delta.branch
        );
    }
}

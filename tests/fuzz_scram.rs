//! SCRAM の通信文（server-first・server-final の解釈）のファジング（ADR 0020 決定 2・ADR 0034 補足 A-2）。
//! 枠は tests/common/fuzz.rs。
//!
//! - 入力: 1 バイト目が mode（0=server-first, 1=server-final）、残りが通信文の生バイト列。
//! - 秘密（パスワード、クライアントナンス、照合署名）は入力・失敗ファイルに含めない。
//! - 性質（ADR 0020 決定 2）:
//!   - 解釈関数（respond / verify）はどの入力にもクラッシュ（panic、segfault）せず Ok/Err を返す。
//!   - 評価器（ir_eval）とネイティブ（Linux x86-64）の出力が完全一致する。
//!   - スタック上限（2 MiB）やメモリ上限を超えない。
//! - 失敗入力の保存と再現手順:
//!   - 検査はネイティブプロセス起動の効率化のため最大 64 件（または 48 KB）ごとのバッチで実行される。
//!   - バッチ実行前に `target/fuzz-failures/scram/batch`（1 行 1 入力の 16 進文字列）と
//!     `batch.meta`（種・開始番号・件数および各行の index、method、mode）が出力され、バッチ成功時にのみ削除される。
//!   - `flush` 内で検査が失敗した場合、枠側（`fuzz::run`）の `fail-<seed>-<index>` や `current` にはバッチを流し出した
//!     時点の入力（多くはバッチ最後の 1 件）が記録されるため、実際の原因入力とは異なる場合がある。
//!   - なお、`fuzz::run` 終了後の端数の最終 flush は枠の外で実行されるため、失敗時は `fail-*` は生成されず
//!     `batch` と `batch.meta` だけが残る。
//!   - 失敗時の再現・回帰登録手順:
//!     1. `target/fuzz-failures/scram/batch.meta` から該当の行（index 等）を特定する。
//!     2. `batch` ファイルの該当行の 16 進文字列をバイト列に復号する。
//!     3. 生のバイト列として `tests/fuzz/regress/scram/` へ 1 ファイルずつ配置する（次回以降のファジングで先頭に自動再生される）。
//! - 回帰テストは tests/fuzz/regress/scram/。
mod common;
#[path = "common/fuzz.rs"]
mod fuzz;

use sigil::exec::{Arg, Options};
use std::sync::{LazyLock, Mutex};

const TARGET: &str = "scram";
const BATCH: usize = 64;
const DEFAULT_ITERS: usize = 12;

static SOURCE: LazyLock<String> = LazyLock::new(|| {
    let scram = include_str!("../src/stdlib/pg_scram.sigil");
    let driver = include_str!("fuzz/scram_driver.sigil");
    let mut body = String::new();
    let mut skip_header = true;
    for line in scram.lines() {
        if skip_header {
            if line.starts_with("module ")
                || line.starts_with("  targets ")
                || line.starts_with("  trust ")
                || line.is_empty()
            {
                continue;
            }
            skip_header = false;
        }
        body.push_str(line);
        body.push('\n');
    }
    format!("{driver}\n{body}")
});

static HARNESS: Mutex<Option<Harness>> = Mutex::new(None);

struct Harness {
    dir: std::path::PathBuf,
    exe: std::path::PathBuf,
    options: Options,
    pending: Vec<Vec<u8>>,
    checked: usize,
    corpus_count: usize,
    regress_count: usize,
}

fn hex(input: &[u8]) -> String {
    input.iter().map(|b| format!("{b:02x}")).collect()
}

fn simple_base64(data: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut i = 0;
    while i < data.len() {
        let a = data[i];
        let b = if i + 1 < data.len() { data[i + 1] } else { 0 };
        let c = if i + 2 < data.len() { data[i + 2] } else { 0 };
        out.push(TABLE[(a >> 2) as usize] as char);
        out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
        if i + 1 < data.len() {
            out.push(TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char);
        } else {
            out.push('=');
        }
        if i + 2 < data.len() {
            out.push(TABLE[(c & 63) as usize] as char);
        } else {
            out.push('=');
        }
        i += 3;
    }
    out
}

fn flush(h: &mut Harness) {
    if h.pending.is_empty() {
        return;
    }
    let wire: String = h.pending.iter().map(|v| format!("{}\n", hex(v))).collect();
    let failure = std::path::Path::new("target/fuzz-failures/scram/batch");
    std::fs::write(failure, &wire).unwrap();
    let meta = failure.with_extension("meta");
    let mut metadata = format!(
        "seed={} start={} count={}\n",
        fuzz::seed(),
        h.checked,
        h.pending.len()
    );
    for (offset, input) in h.pending.iter().enumerate() {
        let index = h.checked + offset;
        let generation = if index < h.regress_count {
            "regression"
        } else if index < h.regress_count + h.corpus_count {
            "corpus"
        } else {
            ["random", "mutation", "grammar"][(index - h.regress_count - h.corpus_count) % 3]
        };
        metadata.push_str(&format!(
            "index={index} method={generation} mode={}\n",
            input.first().copied().unwrap_or(0) % 2
        ));
    }
    std::fs::write(&meta, metadata).unwrap();

    let observed =
        sigil::ir_eval::run_with(&SOURCE, "probe", &[Arg::Str(wire.clone())], &h.options).unwrap();
    assert!(observed.output.is_empty());
    let sigil::json::Value::Str(answers) = sigil::json::parse(&observed.value).unwrap() else {
        panic!("検証結果が文字列でない");
    };
    let results: Vec<_> = answers.lines().collect();
    assert_eq!(results.len(), h.pending.len());

    for (input, answer) in h.pending.iter().zip(results) {
        let mode = input.first().copied().unwrap_or(0) % 2;
        if mode == 0 {
            assert!(
                answer == "err:protocol"
                    || answer == "err:limit"
                    || answer == "err:unsupported"
                    || answer.starts_with("ok:c=biws,"),
                "server-first の不正な結果形式: {answer}"
            );
        } else {
            assert!(
                answer == "ok:true"
                    || answer == "ok:false"
                    || answer == "err:protocol"
                    || answer == "err:limit"
                    || answer == "err:unsupported",
                "server-final の不正な結果形式: {answer}"
            );
        }
    }

    let native = std::process::Command::new(&h.exe)
        .arg(&wire)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        native.status.success(),
        "native failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert!(native.stderr.is_empty());
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        format!("{}\n", observed.value)
    );

    h.checked += h.pending.len();
    h.pending.clear();
    let _ = std::fs::remove_file(failure);
    let _ = std::fs::remove_file(meta);
}

fn check(input: &[u8]) {
    let mut guard = HARNESS.lock().unwrap();
    let h = guard.as_mut().unwrap();
    if !h.pending.is_empty()
        && h.pending.iter().map(|v| v.len() * 2 + 1).sum::<usize>() + input.len() * 2 + 1 > 48_000
    {
        flush(h);
    }
    h.pending.push(input.to_vec());
    if h.pending.len() == BATCH {
        flush(h);
    }
}

fn grammar(rng: &mut fuzz::Rng) -> Vec<u8> {
    let mode = if rng.chance(1, 2) { 0u8 } else { 1u8 };
    let mut out = vec![mode];
    if mode == 0 {
        // server-first
        // client nonce は 24 バイト "012345678901234567890123" -> base64 "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIz"
        let client_nonce = "MDEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIz";
        let server_nonce: String = (0..rng.below(20) + 1)
            .map(|_| {
                *rng.pick(b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.-_")
            })
            .map(|b| b as char)
            .collect();
        let r_val = if rng.chance(9, 10) {
            format!("{client_nonce}{server_nonce}")
        } else {
            // クライアントナンス不一致
            format!("badnonce{server_nonce}")
        };
        let salt_len = rng.below(32) + 1;
        let salt_bytes: Vec<u8> = (0..salt_len).map(|_| rng.next_u64() as u8).collect();
        let salt_b64 = simple_base64(&salt_bytes);
        let iters = *rng.pick(&[
            "4096",
            "4096",
            "4096",
            "4095",
            "100001",
            "0",
            "04096",
            "999999999999",
            "-1",
            "abc",
            "",
        ]);

        let mut attrs = vec![
            ("r".to_string(), r_val),
            ("s".to_string(), salt_b64),
            ("i".to_string(), iters.to_string()),
        ];
        if rng.chance(1, 4) {
            match rng.below(4) {
                0 => attrs.push(("m".into(), "mandatory".into())),
                1 => attrs.push(("a".into(), "custom".into())),
                2 => attrs.push(("i".into(), "4096".into())),
                _ => {
                    attrs.pop();
                }
            }
        }
        if rng.chance(1, 2) {
            attrs.swap(0, 1);
        }
        if rng.chance(1, 3) && attrs.len() >= 3 {
            attrs.swap(1, 2);
        }
        let msg = attrs
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        let mut bytes = msg.into_bytes();
        if rng.chance(1, 6) && !bytes.is_empty() {
            bytes = fuzz::mutate(rng, &bytes, &[bytes.clone()]);
        }
        out.extend(bytes);
    } else {
        // server-final
        let prefix = *rng.pick(&["v=", "v=", "v=", "e=", "x=", "v:"]);
        let sig: String = match rng.below(5) {
            0 => "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=".into(),
            1 => "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".into(),
            2 => "AAAA".into(),
            3 => format!("{}=", "A".repeat(43)),
            _ => {
                let random_sig: Vec<u8> = (0..32).map(|_| rng.next_u64() as u8).collect();
                simple_base64(&random_sig)
            }
        };
        let mut msg = format!("{prefix}{sig}").into_bytes();
        if rng.chance(1, 6) && !msg.is_empty() {
            msg = fuzz::mutate(rng, &msg, &[msg.clone()]);
        }
        out.extend(msg);
    }
    out
}

#[test]
fn fuzz_scram() {
    let started = std::time::Instant::now();
    let options = Options::captured();
    let lower = sigil::effects::Options::default();
    let program = sigil::ir_eval::lower_sources_opts(&[&SOURCE], "probe", &lower).unwrap();
    let dir = common::native::scratch("scram-fuzz");
    let exe = dir.join("probe");
    sigil::native::build_program(&program, &Default::default(), &exe).unwrap();

    let corpus = fuzz::corpus(TARGET, &[]);
    let count = corpus.len();
    let regress_count = std::fs::read_dir("tests/fuzz/regress/scram")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().is_file())
        .count();

    *HARNESS.lock().unwrap() = Some(Harness {
        dir,
        exe,
        options,
        pending: Vec::new(),
        checked: 0,
        corpus_count: count,
        regress_count,
    });

    fuzz::run(TARGET, DEFAULT_ITERS, corpus, grammar, check);

    let mut guard = HARNESS.lock().unwrap();
    let mut h = guard.take().unwrap();
    flush(&mut h);

    eprintln!(
        "scram: seed={} generated={} (random={}, mutation={}, grammar={}), corpus={}, regress={}, total={}, batch={}, elapsed={:?}",
        fuzz::seed(),
        fuzz::iters(DEFAULT_ITERS),
        fuzz::iters(DEFAULT_ITERS).div_ceil(3),
        (fuzz::iters(DEFAULT_ITERS) + 1) / 3,
        fuzz::iters(DEFAULT_ITERS) / 3,
        count,
        regress_count,
        h.checked,
        BATCH,
        started.elapsed()
    );

    std::fs::remove_dir_all(h.dir).unwrap();
}

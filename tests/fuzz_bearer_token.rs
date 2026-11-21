//! ADR 0037 6-6d。入力は mode(0=token, 1=署名するpayload, 2=Authorization)
//! と公開バイト列だけ。鍵・設定・時刻は入力にも失敗ファイルにも含めない。
mod common;
#[path = "common/fuzz.rs"]
mod fuzz;

use sigil::exec::{Arg, Options};
use std::sync::Mutex;

const SOURCE: &str = include_str!("data/bearer_fuzz_512.sigil");
const BATCH: usize = 64;
const DEFAULT_ITERS: usize = 12;
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
const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
fn options() -> Options {
    // HTTP所有モジュールのfeedだけを差し替え、正規化前の任意値と重複を作る。
    let http = include_str!("../src/stdlib/http.sigil");
    let start = http.find("  pub fn feed(").unwrap();
    let body = http[start..].find('{').unwrap() + start;
    let mut depth = 1;
    let mut end = body + 1;
    while depth > 0 {
        match http.as_bytes()[end] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        end += 1;
    }
    let replacement = r#"
      let count = data.get(0).unwrap_or(0) % 4;
      let value = data.slice(1, data.byte_len()).unwrap_or(Bytes.new());
      let mut hs = List.new(); let mut i = 0;
      while i < count {
        let mut name = "Authorization"; if i % 2 == 1 { name = "AUTHORIZATION"; }
        hs.push(HttpHeader { h_name: name, h_value: value.clone() }); i = i + 1;
      }
      return Ok(HttpProgress.Done(HttpRequest { req_method: "GET", req_path: "/private".as_bytes(), req_query: Bytes.new(), req_version: 11, req_headers: hs, req_body: Bytes.new() }, 0));
    }"#;
    let http = format!("{}{}{}", &http[..body + 1], replacement, &http[end..]);
    let clock = include_str!("../src/stdlib/os_clock.sigil")
        .replace("  return Ok(sec);", "  return Ok(1700000000);");
    Options {
        wall_clock: Some(Ok(1700000000)),
        env_vars: Some(std::collections::BTreeMap::from([
            ("SIGIL_AUTH_HMAC_KEY_HEX".into(), KEY.into()),
            ("SIGIL_AUTH_AUDIENCE".into(), "sigil.api".into()),
        ])),
        embedded_extra: vec![("std.http".into(), http), ("std.os.clock".into(), clock)],
        ..Options::captured()
    }
}
fn hex(input: &[u8]) -> String {
    input.iter().map(|b| format!("{b:02x}")).collect()
}
const DENIED: &str = "unauthorized|content-type:text/plain; charset=utf-8|www-authenticate:Bearer|cache-control:no-store";
fn identifier(v: &[u8], max: usize) -> bool {
    !v.is_empty()
        && v.len() <= max
        && v.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(c))
}
fn claims(payload: &[u8]) -> Option<String> {
    if payload.len() < 20 || payload.len() > 404 {
        return None;
    }
    let iat = u64::from_be_bytes(payload.get(..8)?.try_into().ok()?);
    let exp = u64::from_be_bytes(payload.get(8..16)?.try_into().ok()?);
    if iat > 253402300799
        || exp > 253402300799
        || iat > 1700000000
        || exp <= 1700000000
        || !(1..=3600).contains(&exp.checked_sub(iat)?)
    {
        return None;
    }
    let n = u16::from_be_bytes(payload.get(16..18)?.try_into().ok()?) as usize;
    let aud = payload.get(18..18 + n)?;
    if !identifier(aud, 128) || aud != b"sigil.api" {
        return None;
    }
    let pos = 18 + n;
    let n = u16::from_be_bytes(payload.get(pos..pos + 2)?.try_into().ok()?) as usize;
    let sub = payload.get(pos + 2..)?;
    if n != sub.len() || !identifier(sub, 256) {
        return None;
    }
    String::from_utf8(sub.to_vec()).ok()
}
fn known_token(token: &[u8]) -> Option<String> {
    let encoded = hex(token);
    include_str!("data/bearer_507.tsv")
        .lines()
        .filter(|r| !r.starts_with('#'))
        .find_map(|row| {
            let c: Vec<_> = row.split('\t').collect();
            (c[1] == KEY && c[6] == encoded && c[7] == "sigil.api" && c[8] == "1700000000").then(
                || {
                    if c[9] == "ERR" {
                        "err".into()
                    } else {
                        format!("ok:{}", c[9])
                    }
                },
            )
        })
}
fn expected(input: &[u8]) -> Option<String> {
    let mode = input.first().copied().unwrap_or(0);
    let data = input.get(1..).unwrap_or(&[]);
    match mode {
        1 => Some(claims(data).map_or("err".into(), |sub| format!("ok:{sub}"))),
        2 => {
            let value = data.get(1..).unwrap_or(&[]);
            if data.first().copied().unwrap_or(0) % 4 != 1
                || value.len() < 8
                || !value[..7].eq_ignore_ascii_case(b"Bearer ")
            {
                return Some(DENIED.into());
            }
            known_token(&value[7..]).map(|v| if v == "err" { DENIED.into() } else { v })
        }
        _ => known_token(data),
    }
}

fn flush(h: &mut Harness) {
    if h.pending.is_empty() {
        return;
    }
    let wire: String = h.pending.iter().map(|v| format!("{}\n", hex(v))).collect();
    let failure = std::path::Path::new("target/fuzz-failures/bearer_token/batch");
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
            input.first().copied().unwrap_or(0)
        ));
    }
    std::fs::write(&meta, metadata).unwrap();
    let observed =
        sigil::ir_eval::run_with(SOURCE, "probe", &[Arg::Str(wire.clone())], &h.options).unwrap();
    assert!(observed.output.is_empty());
    let sigil::json::Value::Str(answers) = sigil::json::parse(&observed.value).unwrap() else {
        panic!("検証結果が文字列でない");
    };
    let results: Vec<_> = answers.lines().collect();
    assert_eq!(results.len(), h.pending.len());
    for (input, answer) in h.pending.iter().zip(results) {
        assert!(
            answer == "err"
                || answer == DENIED
                || answer
                    .strip_prefix("ok:")
                    .is_some_and(|s| identifier(s.as_bytes(), 256)),
            "固定失敗または有効主体以外の結果"
        );
        if let Some(want) = expected(input) {
            assert_eq!(answer, want, "独立payload仕様または固定ベクタとの不一致");
        }
    }
    let native = std::process::Command::new(&h.exe)
        .arg(&wire)
        .env_clear()
        .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
        .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
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
    std::fs::remove_file(failure).unwrap();
    std::fs::remove_file(meta).unwrap();
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
    // 正しい署名付きの意味解釈まで必ず到達するモード。長さ欄・時刻も変異する。
    let mut payload = Vec::new();
    let now = 1_700_000_000u64;
    let iat = *rng.pick(&[now, now + 1, 0, u64::MAX]);
    let exp = *rng.pick(&[now, now + 1, now + 3600, now + 3601, u64::MAX]);
    payload.extend(iat.to_be_bytes());
    payload.extend(exp.to_be_bytes());
    let aud = *rng.pick(&[b"sigil.api".as_slice(), b"other", b"", b"a\0"]);
    payload.extend((aud.len() as u16).to_be_bytes());
    payload.extend(aud);
    let sub_len = *rng.pick(&[0usize, 1, 13, 256, 257]);
    payload.extend((sub_len as u16).to_be_bytes());
    payload.extend(vec![b'a'; sub_len]);
    if rng.chance(1, 4) {
        payload = fuzz::mutate(rng, &payload, &[payload.clone()]);
    }
    let mut out = vec![1];
    out.extend(payload);
    out
}
#[test]
fn fuzz_bearer_token() {
    let started = std::time::Instant::now();
    let options = options();
    let lower = sigil::effects::Options {
        embedded_extra: options.embedded_extra.clone(),
        ..Default::default()
    };
    let program = sigil::ir_eval::lower_sources_opts(&[SOURCE], "probe", &lower).unwrap();
    let dir = common::native::scratch("bearer512");
    let exe = dir.join("probe");
    sigil::native::build_program(&program, &Default::default(), &exe).unwrap();
    let corpus = fuzz::corpus("bearer_token", &[]);
    let count = corpus.len();
    let regress_count = std::fs::read_dir("tests/fuzz/regress/bearer_token")
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
    fuzz::run("bearer_token", DEFAULT_ITERS, corpus, grammar, check);
    let mut guard = HARNESS.lock().unwrap();
    let mut h = guard.take().unwrap();
    flush(&mut h);
    eprintln!("bearer_token: seed={} generated={} (random={}, mutation={}, grammar={}), corpus={}, regress={}, total={}, batch={}, elapsed={:?}", fuzz::seed(), fuzz::iters(DEFAULT_ITERS), fuzz::iters(DEFAULT_ITERS).div_ceil(3), (fuzz::iters(DEFAULT_ITERS)+1)/3, fuzz::iters(DEFAULT_ITERS)/3, count, regress_count, h.checked, BATCH, started.elapsed());
    std::fs::remove_dir_all(h.dir).unwrap();
}

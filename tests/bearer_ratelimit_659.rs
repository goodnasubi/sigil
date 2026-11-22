//! ADR 0037 / Issue #659: 認証レート制限（60秒で失敗100回まで、超過は429）。
//! 閾値と方式は typesafe-ai 正式判定（2026-11-22）で計画案どおり確定。
mod common;
use sigil::exec::Options;

const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const DRIVER: &str = include_str!("data/bearer_ratelimit_659.sigil");
const DENIED: &str = "unauthorized|content-type:text/plain; charset=utf-8|www-authenticate:Bearer|cache-control:no-store";
const LIMITED60: &str = "too many requests|content-type:text/plain; charset=utf-8|retry-after:60|cache-control:no-store";
const LIMITED1: &str =
    "too many requests|content-type:text/plain; charset=utf-8|retry-after:1|cache-control:no-store";

fn options() -> Options {
    let mut env = std::collections::BTreeMap::new();
    env.insert("SIGIL_AUTH_HMAC_KEY_HEX".into(), KEY.into());
    env.insert("SIGIL_AUTH_AUDIENCE".into(), "sigil.api".into());
    Options {
        env_vars: Some(env),
        wall_clock: Some(Ok(1700000000)),
        trace_drops: false,
        ..Options::captured()
    }
}

fn token() -> String {
    let row = include_str!("data/bearer_507.tsv")
        .lines()
        .find(|l| l.starts_with("valid_iat_equals_now_lifetime_3600\t"))
        .unwrap();
    let hex = row.split('\t').nth(6).unwrap();
    String::from_utf8(
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect(),
    )
    .unwrap()
}

fn eval(entry: &str, want: &str, want_output: &str) {
    let o = sigil::ir_eval::run_with(DRIVER, entry, &[sigil::exec::Arg::Str(token())], &options())
        .unwrap();
    assert_eq!(o.value, format!("{want:?}"), "entry={entry}");
    assert_eq!(o.output, want_output, "entry={entry}");
}

/// os_clock を差し替えたネイティブ実行ファイルを作る（#509 の作法）。
/// `tail` は `return Ok(sec);` の置き換え（時刻の決め方）。
fn build_with_clock(
    entry: &str,
    tag: &str,
    tail: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let mut clock = include_str!("../src/stdlib/os_clock.sigil").replace(
        "  let sec = raw.load_i64(ts);",
        "  let sec = raw.load_i64(ts);\n  TEST_CALLS = TEST_CALLS + 1;",
    );
    clock = clock.replace("  return Ok(sec);", tail);
    clock.push_str("\nglobal mut TEST_CALLS: Int = 0\n");
    let opts = sigil::effects::Options {
        embedded_extra: vec![("std.os.clock".into(), clock)],
        ..Default::default()
    };
    let program = sigil::ir_eval::lower_sources_opts(&[DRIVER], entry, &opts).unwrap();
    let dir = common::native::scratch(tag);
    let exe = dir.join("probe");
    sigil::native::build_program(&program, &Default::default(), &exe).unwrap();
    (dir, exe)
}

fn run_native(exe: &std::path::Path) -> String {
    let out = std::process::Command::new(exe)
        .arg(token())
        .env_clear()
        .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
        .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stderr.is_empty());
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn boundary_limited_and_public_routes_eval() {
    // 境界: 100回目まで401、101回目から429（Retry-After は固定時刻で60）。
    // 制限中は正しいトークンも429。公開経路は制限中も200で、フックは成功時だけ。
    eval(
        "scenario",
        &format!("100;{LIMITED60};{LIMITED60};public|x-after:safe"),
        "before\nafter\n",
    );
    // 成功の扱い: 成功は数えず、リセットもしない（99回の後の成功→次の失敗が100回目で401、その次が429）。
    eval(
        "mixed",
        &format!("99;user:alice-01|x-after:safe;{DENIED};{LIMITED60}"),
        "before\nafter\n",
    );
    // 共有: SyncShared で共有した二つの持ち手からの失敗が合算される。
    eval("shared_scenario", &format!("100;{LIMITED60}"), "");
}

#[test]
fn boundary_native_constant_clock() {
    let (dir, exe) = build_with_clock("scenario", "bearer659-scenario", "  return Ok(1700000000);");
    assert_eq!(
        run_native(&exe),
        format!("before\nafter\n\"100;{LIMITED60};{LIMITED60};public|x-after:safe\"\n")
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn window_edge_native_shifting_clock() {
    // 起動1回＋失敗100回まで基準時刻、次の要求で +59 秒（429、残り1秒）、その次で +60 秒（401へ回復）。
    let (dir, exe) = build_with_clock(
        "window_edge",
        "bearer659-window",
        "  if TEST_CALLS <= 101 { return Ok(1700000000); }\n  if TEST_CALLS == 102 { return Ok(1700000059); }\n  return Ok(1700000060);",
    );
    assert_eq!(run_native(&exe), format!("\"100;{LIMITED1};{DENIED}\"\n"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn clock_failure_stays_401_eval_and_native() {
    // 評価器: 要求時の時計だけを失敗させる（#509 の web 差替えの作法）。
    let web = include_str!("../src/stdlib/web.sigil").replace(
        "  let now = clock.unix_seconds();",
        "  let observed = clock.unix_seconds();\n  let now = None;",
    );
    let mut opts = options();
    opts.embedded_extra.push(("std.web".into(), web));
    let o = sigil::ir_eval::run_with(
        DRIVER,
        "many_failures",
        &[sigil::exec::Arg::Str(token())],
        &opts,
    )
    .unwrap();
    assert_eq!(o.value, format!("{:?}", format!("150;{DENIED}")));
    assert!(o.output.is_empty());
    // ネイティブ: 起動時の1回だけ成功し、要求時の取得が失敗する時計。
    let (dir, exe) = build_with_clock(
        "many_failures",
        "bearer659-clockfail",
        "  if TEST_CALLS > 1 { return Err(-1); }\n  return Ok(1700000000);",
    );
    assert_eq!(run_native(&exe), format!("\"150;{DENIED}\"\n"));
    std::fs::remove_dir_all(dir).unwrap();
}

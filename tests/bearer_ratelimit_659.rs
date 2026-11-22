//! ADR 0037 / Issue #659: 認証レート制限（60秒で失敗100回まで、超過は429）。
//! 閾値と方式は typesafe-ai 正式判定（2026-11-22）で計画案どおり確定。
mod common;
#[path = "common/serve.rs"]
mod serve;
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
fn custom_verifier_429_passes_through_eval() {
    // 利用者定義の検証器が返した 429 は Router がそのまま通す（意図した挙動）。
    // 401 などそれ以外は固定 401 に置き換えられる（web_router_478.rs で検証済み）。
    eval(
        "custom_429",
        "too many requests|content-type:text/plain; charset=utf-8|retry-after:5",
        "",
    );
}

#[test]
fn clock_rewind_opens_new_window_native() {
    // 起動1回＋失敗100回まで基準時刻、その後の要求で時計が1時間戻る。
    // 巻き戻しをウィンドウ内とみなす旧挙動なら429（Retry-After 3660）になるが、
    // 新しいウィンドウとして扱うため401が続く。
    let (dir, exe) = build_with_clock(
        "rewound",
        "bearer659-rewound",
        "  if TEST_CALLS <= 101 { return Ok(1700000000); }\n  return Ok(1699996400);",
    );
    assert_eq!(run_native(&exe), format!("\"100;{DENIED};{DENIED}\"\n"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn shared_rate_limit_across_connections_native() {
    // 実 serve 経路の形: 06-service の serve と同じく、検証器を一度だけ作った
    // Router を SyncShared で共有し、接続ごとに clone した持ち手で振り分ける。
    // 接続1で60回、接続2で40回失敗した後の101回目が429なら、状態は接続を
    // またいで共有されている。接続ごとに分かれていれば101回目は401になる。
    let (dir, exe) = build_with_clock(
        "serve_two",
        "bearer659-serve-two",
        "  return Ok(1700000000);",
    );
    let mut child = std::process::Command::new(&exe)
        .env_clear()
        .env("SIGIL_AUTH_HMAC_KEY_HEX", KEY)
        .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut err_reader = std::io::BufReader::new(child.stderr.take().unwrap());
    let err_thread = std::thread::spawn(move || {
        use std::io::Read;
        let mut s = String::new();
        let _ = err_reader.read_to_string(&mut s);
        s
    });
    let mut line = String::new();
    use std::io::{BufRead, Read};
    reader.read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    let bad = "GET /private HTTP/1.1\r\nHost: local\r\nAuthorization: Bearer invalid\r\n\r\n";
    let first = serve::exchange(port, bad.repeat(60).as_bytes(), 0);
    assert_eq!(serve::statuses(&first), vec![401; 60]);
    let second = serve::exchange(port, bad.repeat(41).as_bytes(), 0);
    let mut want = vec![401; 40];
    want.push(429);
    assert_eq!(serve::statuses(&second), want);
    // 標準出力の読み手を子の終了まで開けておく。先に閉じると子が SIGPIPE で死ぬ。
    let status = child.wait().unwrap();
    let stderr = err_thread.join().unwrap();
    let mut rest = String::new();
    reader.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "0\n");
    assert!(status.success(), "status={status} stderr={stderr:?}");
    assert!(stderr.is_empty());
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

//! 実行時の `fs.read` の上限と種別の検査（Issue #176）。実ファイルを読む経路だけを対象にする。

use sigil::exec::{Arg, Options};
use std::path::PathBuf;

const SRC: &str = include_str!("../examples/02-read-port/c-brace.sigil");
/// `ir_eval` の暫定の上限（64 MiB）。テストからは定数を参照せず、値を書く。
const LIMIT: u64 = 64 * 1024 * 1024;

fn read_port(path: &str) -> String {
    let o = Options {
        capture_console: true,
        ..Options::default()
    };
    sigil::ir_eval::run_sources_with(&[SRC], "read_port", &[Arg::Str(path.into())], &o)
        .unwrap_or_else(|e| panic!("{e}"))
        .value
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-fsread-176-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn sparse(name: &str, len: u64) -> PathBuf {
    let p = tmp(name);
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(len).unwrap();
    p
}

#[test]
fn small_regular_file_still_reads() {
    let p = tmp("ok");
    std::fs::write(&p, "8080\n").unwrap();
    assert_eq!(read_port(p.to_str().unwrap()), "Ok(8080)");
}

#[test]
fn file_over_the_limit_is_rejected() {
    let p = sparse("over", LIMIT + 1);
    let v = read_port(p.to_str().unwrap());
    assert!(v.starts_with("Err(ConfigError.Io(IoError.Other("), "{v}");
}

#[test]
fn file_at_the_limit_is_not_rejected_for_size() {
    // 上限ちょうど（NUL だけ）は大きさでは拒否されない。数値でないので NotANumber になる。
    let p = sparse("exact", LIMIT);
    let v = read_port(p.to_str().unwrap());
    assert!(v.starts_with("Err(ConfigError.NotANumber("), "{v}");
}

#[cfg(unix)]
#[test]
fn device_is_rejected() {
    let v = read_port("/dev/zero");
    assert!(v.starts_with("Err(ConfigError.Io(IoError.Other("), "{v}");
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_without_blocking() {
    let p = tmp("fifo");
    let _ = std::fs::remove_file(&p);
    let ok = std::process::Command::new("mkfifo")
        .arg(&p)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "mkfifo が使えない");
    let v = read_port(p.to_str().unwrap());
    assert!(v.starts_with("Err(ConfigError.Io(IoError.Other("), "{v}");
}

//! `fs.read` のテスト用の差し替え（ADR 0004 の規則 7）と、`Cell`・`Shared` の実行、
//! サンプル `02-read-port`・`04-effects` の実行。

use sigil::interp::{run_sources_with, Arg, Failure, Options, Outcome, RunError};
use std::collections::BTreeMap;

const PORT: &str = include_str!("../examples/02-read-port/c-brace.sigil");
const EFFECTS: &str = include_str!("../examples/04-effects/effects.sigil");

fn files(xs: &[(&str, &str)]) -> Options {
    let m: BTreeMap<String, String> = xs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Options {
        fs_files: Some(m),
        ..Options::default()
    }
}

fn go(srcs: &[&str], f: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    run_sources_with(srcs, f, args, opts)
}

fn read_port(content: &str) -> String {
    let o = files(&[("/etc/port", content)]);
    go(&[PORT], "read_port", &[Arg::Str("/etc/port".into())], &o)
        .unwrap_or_else(|e| panic!("{e}"))
        .value
}

#[test]
fn read_port_returns_the_port() {
    assert_eq!(read_port("8080\n"), "Ok(8080)");
    assert_eq!(read_port("  1 "), "Ok(1)");
    assert_eq!(read_port("65535"), "Ok(65535)");
}

#[test]
fn read_port_out_of_range() {
    assert_eq!(read_port("70000"), "Err(ConfigError.OutOfRange(70000))");
    assert_eq!(read_port("0"), "Err(ConfigError.OutOfRange(0))");
    assert_eq!(read_port("-5"), "Err(ConfigError.OutOfRange(-5))");
}

#[test]
fn read_port_not_a_number() {
    assert_eq!(read_port("abc"), "Err(ConfigError.NotANumber(\"abc\"))");
    assert_eq!(read_port(""), "Err(ConfigError.NotANumber(\"\"))");
}

#[test]
fn read_port_missing_file_is_an_io_error() {
    let o = files(&[]);
    let r = go(&[PORT], "read_port", &[Arg::Str("/nope".into())], &o).unwrap();
    assert_eq!(r.value, "Err(ConfigError.Io(IoError.NotFound(\"/nope\")))");
}

#[test]
fn read_port_empty_path_violates_requires() {
    let o = files(&[("", "1")]);
    let e = go(&[PORT], "read_port", &[Arg::Str("".into())], &o).unwrap_err();
    assert!(matches!(e.error, RunError::Stop(_)), "{e}");
}

#[test]
fn without_a_double_fs_read_is_unsupported() {
    let e = go(
        &[PORT],
        "read_port",
        &[Arg::Str("/etc/port".into())],
        &Options::default(),
    )
    .unwrap_err();
    assert!(matches!(e.error, RunError::Unsupported(_)), "{e}");
}

#[test]
fn load_prices_reads_lines_and_logs() {
    let o = files(&[("p.txt", "10\n20\nx\n30\n")]);
    let r = go(&[EFFECTS], "load_prices", &[Arg::Str("p.txt".into())], &o).unwrap();
    assert_eq!(r.output, "loading prices\n");
    assert_eq!(r.value, "Ok([10, 20, 0, 30])");
    let r = go(&[EFFECTS], "load_prices", &[Arg::Str("q".into())], &o).unwrap();
    assert_eq!(r.value, "Err(IoError.NotFound(\"q\"))");
}

const ENTRY: &str = "module examples.effect_check\n  targets server\n  trust none\n\nfn bump_twice() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects state\n{\n  let c = Shared.new(Cell.new(5));\n  count_up(&c);\n  count_up(&c);\n  return c.get();\n}\n\nfn tax() -> List<Int>\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut xs = List.new();\n  xs.push(100);\n  xs.push(250);\n  return with_tax(&xs);\n}\n";

#[test]
fn count_up_shares_the_cell() {
    let r = go(&[EFFECTS, ENTRY], "bump_twice", &[], &Options::default()).unwrap();
    assert_eq!(r.value, "7");
}

#[test]
fn with_tax_applies_add_tax() {
    let r = go(&[EFFECTS, ENTRY], "tax", &[], &Options::default()).unwrap();
    assert_eq!(r.value, "[110, 275]");
}

#[test]
fn cli_fs_double_runs_read_port() {
    let dir = std::env::temp_dir().join(format!("sigil-fs-double-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("port.txt");
    std::fs::write(&f, "99999\n").unwrap();
    let bin = env!("CARGO_BIN_EXE_sigil");
    let ex = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/02-read-port/c-brace.sigil"
    );
    let o = std::process::Command::new(bin)
        .args(["run", "--fs-double"])
        .arg(format!("/etc/port={}", f.display()))
        .args([ex, "read_port", "/etc/port"])
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "Err(ConfigError.OutOfRange(99999))\n"
    );
    let o = std::process::Command::new(bin)
        .args(["run", ex, "read_port", "/etc/port"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
}

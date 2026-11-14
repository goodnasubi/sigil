//! ADR 0036: バイナリ I/O の公開契約を評価器と Linux ネイティブで突き合わせる。
mod common;

use common::native::{exec, scratch, try_build};
use sigil::exec::{Arg, Options};
use std::collections::BTreeMap;

const HEAD: &str = "module m.binary\n  targets server\n  trust none\n\n";
const READ: &str = r#"
fn read(p: String, max: Int) -> Result<Int, BinaryIoError>
  intent "任意バイトを上限付きで読む"
  requires none
  ensures none
  effects fs.read
{
  match fs.read_bytes(&Path.new(&p), max) {
    Ok(b) => {
      let mut sum = 0;
      let mut i = 0;
      while i < b.len() {
        match b.get(i) {
          Some(x) => { sum = (sum * 257 + x) % 1000000007; }
          None => {}
        }
        i = i + 1;
      }
      return Ok(sum + b.len() * 1000000007);
    }
    Err(e) => { return Err(e); }
  }
}
"#;
const WRITE: &str = r#"
fn write(p: String, max: Int, n: Int) -> Result<Int, BinaryIoError>
  intent "同じ借用 Bytes を失敗後と成功後に再利用する"
  requires none
  ensures none
  effects fs.write
{
  let mut b = Bytes.new();
  let mut i = 0;
  while i < n {
    b.push(i % 256);
    i = i + 1;
  }
  let path = Path.new(&p);
  match fs.write_bytes(&path, &b, max) {
    Ok(u) => {
      let rejected = fs.write_bytes(&path, &b, 0);
      match fs.write_bytes(&path, &b, max) {
        Ok(v) => { return Ok(b.len()); }
        Err(e) => { return Err(e); }
      }
    }
    Err(e) => { return Err(e); }
  }
}
"#;

fn source(body: &str) -> String {
    format!("{HEAD}{body}")
}
fn s(p: &std::path::Path) -> Arg {
    Arg::Str(p.to_string_lossy().into_owned())
}
fn eval(src: &str, entry: &str, args: &[Arg], opts: &Options) -> String {
    sigil::ir_eval::run_sources_with(&[src], entry, args, opts)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}
fn expected(data: &[u8]) -> String {
    let sum = data
        .iter()
        .fold(0i64, |a, &b| (a * 257 + b as i64) % 1000000007);
    format!("Ok({})", sum + data.len() as i64 * 1000000007)
}

#[test]
fn reads_arbitrary_bytes_and_limit_edges() {
    let dir = scratch("binary-read");
    let src = source(READ);
    let exe = dir.join("read");
    try_build(&[&src], "read", &exe, false).unwrap();
    let cases = [vec![], vec![0], vec![255, 192, 128, 0], (0..=255).collect()];
    for (i, data) in cases.iter().enumerate() {
        let path = dir.join(format!("input{i}"));
        std::fs::write(&path, data).unwrap();
        for max in [
            0,
            1,
            data.len() as i64 - 1,
            data.len() as i64,
            data.len() as i64 + 1,
            134217728,
        ] {
            let want = if max < 0 {
                format!("Err(BinaryIoError.InvalidLimit({max}))")
            } else if data.len() as i64 > max {
                format!("Err(BinaryIoError.TooLarge({max}))")
            } else {
                expected(data)
            };
            let args = [s(&path), Arg::Int(max)];
            assert_eq!(eval(&src, "read", &args, &Options::captured()), want);
            let got = exec(&exe, &args);
            assert_eq!(got.code, Some(0), "{}", got.stderr);
            assert_eq!(got.stdout.trim(), want, "case {i}, max {max}");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn writes_all_bytes_truncates_and_preserves_data_for_retry() {
    let dir = scratch("binary-write");
    let src = source(WRITE);
    let exe = dir.join("write");
    try_build(&[&src], "write", &exe, false).unwrap();
    let path = dir.join("output");
    for n in [0, 1, 256, 65537] {
        let bytes: Vec<u8> = (0..n).map(|x| (x % 256) as u8).collect();
        for native in [false, true] {
            std::fs::write(&path, b"previous-longer-output").unwrap();
            let args = [s(&path), Arg::Int(n), Arg::Int(n)];
            let got = if native {
                let got = exec(&exe, &args);
                assert_eq!(got.code, Some(0), "{}", got.stderr);
                got.stdout.trim().to_string()
            } else {
                eval(&src, "write", &args, &Options::captured())
            };
            assert_eq!(got, format!("Ok({n})"));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invalid_inputs_are_rejected_before_touching_output() {
    let dir = scratch("binary-invalid");
    let path = dir.join("preserved");
    let src = source(WRITE);
    for (max, n, want) in [
        (-1, 1, "Err(BinaryIoError.InvalidLimit(-1))"),
        (134217729, 1, "Err(BinaryIoError.InvalidLimit(134217729))"),
        (
            i64::MAX,
            1,
            "Err(BinaryIoError.InvalidLimit(9223372036854775807))",
        ),
        (
            i64::MIN,
            1,
            "Err(BinaryIoError.InvalidLimit(-9223372036854775808))",
        ),
        (0, 1, "Err(BinaryIoError.TooLarge(0))"),
        (1, 2, "Err(BinaryIoError.TooLarge(1))"),
    ] {
        std::fs::write(&path, b"preserved").unwrap();
        let got = common::run(&src, "write", &[s(&path), Arg::Int(max), Arg::Int(n)]).unwrap();
        assert_eq!(got.value, want);
        assert_eq!(std::fs::read(&path).unwrap(), b"preserved");
    }
    // InvalidPath は TooLarge より先、InvalidLimit は InvalidPath より先。
    for p in [format!("{}\0tail", path.display()), "é".repeat(2048)] {
        for (max, want) in [
            (0, "Err(BinaryIoError.InvalidPath)"),
            (-1, "Err(BinaryIoError.InvalidLimit(-1))"),
        ] {
            let got = common::run(
                &src,
                "write",
                &[Arg::Str(p.clone()), Arg::Int(max), Arg::Int(1)],
            )
            .unwrap();
            assert_eq!(got.value, want);
        }
    }
    assert_eq!(std::fs::read(&path).unwrap(), b"preserved");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn read_errors_keep_their_class_and_path() {
    let dir = scratch("binary-errors");
    let src = source(READ);
    let missing = dir.join("missing");
    let got = common::run(&src, "read", &[s(&missing), Arg::Int(1)])
        .unwrap()
        .value;
    assert_eq!(
        got,
        format!(
            "Err(BinaryIoError.Io(IoError.NotFound({:?})))",
            missing.to_str().unwrap()
        )
    );
    let got = common::run(&src, "read", &[s(&dir), Arg::Int(1)])
        .unwrap()
        .value;
    assert_eq!(got, "Err(BinaryIoError.UnsupportedFile)");
    for p in ["a\0b".to_string(), "x".repeat(4096)] {
        let got = common::run(&src, "read", &[Arg::Str(p), Arg::Int(1)])
            .unwrap()
            .value;
        assert_eq!(got, "Err(BinaryIoError.InvalidPath)");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn non_regular_inputs_are_rejected_without_waiting_for_fifo_writer() {
    let dir = scratch("binary-kind");
    let fifo = dir.join("fifo");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let src = source(READ);
    for path in [&fifo, std::path::Path::new("/dev/zero")] {
        let got = common::run(&src, "read", &[s(path), Arg::Int(1)]).unwrap();
        assert_eq!(got.value, "Err(BinaryIoError.UnsupportedFile)");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn byte_override_is_isolated_from_text_and_never_falls_back_to_os() {
    let dir = scratch("binary-override");
    let path = dir.join("real");
    std::fs::write(&path, [99]).unwrap();
    let key = path.to_string_lossy().into_owned();
    let mut opts = Options {
        fs_bytes: Some(BTreeMap::from([(key.clone(), vec![0, 255])])),
        fs_files: Some(BTreeMap::from([(key.clone(), "text".into())])),
        ..Options::captured()
    };
    let src = source(READ);
    assert_eq!(
        eval(&src, "read", &[s(&path), Arg::Int(2)], &opts),
        expected(&[0, 255])
    );
    assert_eq!(
        eval(&src, "read", &[s(&path), Arg::Int(1)], &opts),
        "Err(BinaryIoError.TooLarge(1))"
    );
    opts.fs_bytes = Some(BTreeMap::new());
    assert!(eval(&src, "read", &[s(&path), Arg::Int(1)], &opts).contains("IoError.NotFound"));
    let program = source(
        r#"
fn run(p: String) -> Result<Int, BinaryIoError>
  intent "バイト表だけを書き替える"
  requires none
  ensures none
  effects fs.read, fs.write
{
  let path = Path.new(&p);
  let b = "ab".as_bytes();
  match fs.write_bytes(&path, &b, 2) {
    Ok(u) => { return read(p, 2); }
    Err(e) => { return Err(e); }
  }
}
"#,
    );
    let program = format!("{program}{READ}");
    assert_eq!(eval(&program, "run", &[s(&path)], &opts), expected(b"ab"));
    assert_eq!(std::fs::read(&path).unwrap(), [99]);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn binary_effects_are_exact() {
    for (body, entry, effect) in [(READ, "read", "fs.read"), (WRITE, "write", "fs.write")] {
        let src = source(body);
        assert!(sigil::ir_eval::lower_sources(&[&src], entry).is_ok());
        for changed in ["none", "fs.read, fs.write"] {
            let bad = src.replace(
                &format!("effects {effect}\n"),
                &format!("effects {changed}\n"),
            );
            assert!(
                sigil::ir_eval::lower_sources(&[&bad], entry).is_err(),
                "{bad}"
            );
        }
    }
}

//! M4 の効果操作: 実ファイルの `fs.read`・`fs.write`、`Path` の操作、`Cell` の複製、`--fs-double` の重複。
//! 実ファイルは一時ディレクトリ（プロセスごとに別）で試す。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::path::PathBuf;
use std::process::Command;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\nstruct Probe {\n  id: Int,\n}\n\nstruct Holder {\n  c: Cell<Int>,\n  n: Int,\n}\n\nfn mk(n: Int) -> Probe\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Probe { id: n };\n}\n\n";

fn f(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

fn out(sig: &str, effects: &str, body: &str, args: &[Arg], opts: &Options) -> String {
    let src = format!("{HEAD}{}", f(sig, effects, body));
    run_with(&src, "t", args, opts)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-m4-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn s(p: &std::path::Path) -> Arg {
    Arg::Str(p.to_string_lossy().into_owned())
}

// ---- 実ファイル ----

#[test]
fn fs_read_reads_a_real_file() {
    let d = dir("read");
    let p = d.join("a.txt");
    std::fs::write(&p, "hello\n").unwrap();
    let v = out(
        "t(p: Path) -> Result<String, IoError>",
        "fs.read",
        "return fs.read(&p);",
        &[s(&p)],
        &Options::default(),
    );
    assert_eq!(v, "Ok(\"hello\\n\")");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_read_missing_file_is_not_found() {
    let d = dir("missing");
    let p = d.join("none.txt");
    let v = out(
        "t(p: Path) -> Result<String, IoError>",
        "fs.read",
        "return fs.read(&p);",
        &[s(&p)],
        &Options::default(),
    );
    assert_eq!(v, format!("Err(IoError.NotFound(\"{}\"))", p.display()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_read_a_directory_is_an_other_error() {
    let d = dir("isdir");
    let v = out(
        "t(p: Path) -> Result<String, IoError>",
        "fs.read",
        "return fs.read(&p);",
        &[s(&d)],
        &Options::default(),
    );
    assert!(v.starts_with("Err(IoError.Other("), "{v}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_write_creates_and_overwrites() {
    let d = dir("write");
    let p = d.join("w.txt");
    let body = "let r = fs.write(&p, &text);\nreturn r;";
    let sig = "t(p: Path, text: String) -> Result<Unit, IoError>";
    let v = out(
        sig,
        "fs.write",
        body,
        &[s(&p), Arg::Str("one".into())],
        &Options::default(),
    );
    assert_eq!(v, "Ok(())");
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "one");
    out(
        sig,
        "fs.write",
        body,
        &[s(&p), Arg::Str("two".into())],
        &Options::default(),
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "two");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_write_into_a_missing_directory_is_not_found() {
    let d = dir("nodir");
    let p = d.join("no").join("w.txt");
    let v = out(
        "t(p: Path, text: String) -> Result<Unit, IoError>",
        "fs.write",
        "return fs.write(&p, &text);",
        &[s(&p), Arg::Str("x".into())],
        &Options::default(),
    );
    assert_eq!(v, format!("Err(IoError.NotFound(\"{}\"))", p.display()));
    assert!(!p.exists());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_double_table_is_not_a_real_file_and_fs_write_updates_it() {
    let d = dir("table");
    let p = d.join("t.txt");
    let o = Options {
        fs_files: Some(Default::default()),
        ..Options::default()
    };
    let body = "let r = fs.write(&p, &text);\nreturn fs.read(&p);";
    let v = out(
        "t(p: Path, text: String) -> Result<String, IoError>",
        "fs.read, fs.write",
        body,
        &[s(&p), Arg::Str("in-memory".into())],
        &o,
    );
    assert_eq!(v, "Ok(\"in-memory\")");
    assert!(!p.exists(), "差し替え中は実ファイルに書かない");
    let _ = std::fs::remove_dir_all(&d);
}

// ---- Path ----

#[test]
fn path_operations() {
    let o = Options::default();
    let v = out(
        "t() -> String",
        "none",
        "let p = Path.new(&\"/etc/app\");\nlet q = p.join(&\"conf.toml\");\nreturn q.file_name().unwrap_or(\"\");",
        &[],
        &o,
    );
    assert_eq!(v, "\"conf.toml\"");
    let v = out(
        "t() -> Bool",
        "none",
        "let p = Path.new(&\"/etc/app/conf.toml\");\nreturn match p.parent() {\n  Some(q) => !q.is_empty(),\n  None => false,\n};",
        &[],
        &o,
    );
    assert_eq!(v, "true");
    let v = out(
        "t() -> Bool",
        "none",
        "let p = Path.new(&\"/\");\nreturn p.parent().is_none();",
        &[],
        &o,
    );
    assert_eq!(v, "true");
    let v = out(
        "t() -> Bool",
        "none",
        "let p = Path.new(&\"/\");\nreturn p.file_name().is_none();",
        &[],
        &o,
    );
    assert_eq!(v, "true");
}

#[test]
fn path_join_then_write_and_read_back() {
    let d = dir("pathjoin");
    let v = out(
        "t(base: Path) -> Result<String, IoError>",
        "fs.read, fs.write",
        "let p = base.join(&\"x.txt\");\nlet w = fs.write(&p, &\"abc\");\nreturn fs.read(&p);",
        &[s(&d)],
        &Options::default(),
    );
    assert_eq!(v, "Ok(\"abc\")");
    assert_eq!(std::fs::read_to_string(d.join("x.txt")).unwrap(), "abc");
    let _ = std::fs::remove_dir_all(&d);
}

// ---- Cell の複製 ----

#[test]
fn cell_clone_is_independent_and_shared_clone_aliases() {
    let o = Options::default();
    // Cell 単体の clone は別の Cell。
    let v = out(
        "t() -> Int",
        "state",
        "let c = Cell.new(1);\nlet d = c.clone();\nlet s = Shared.new(c);\nlet u = Shared.new(d);\ns.set(9);\nreturn u.get();",
        &[],
        &o,
    );
    assert_eq!(v, "1");
    // Shared の clone は同じ Cell を指す。
    let v = out(
        "t() -> Int",
        "state",
        "let s = Shared.new(Cell.new(1));\nlet u = s.clone();\nu.set(9);\nreturn s.get();",
        &[],
        &o,
    );
    assert_eq!(v, "9");
}

#[test]
fn cell_inside_a_struct_is_copied_by_clone() {
    let v = out(
        "t() -> Int",
        "state",
        "let h = Holder { c: Cell.new(1), n: 0 };\nlet g = h.clone();\nlet s = Shared.new(h.c);\nlet u = Shared.new(g.c);\ns.set(9);\nreturn u.get();",
        &[],
        &Options::default(),
    );
    assert_eq!(v, "1");
}

#[test]
fn tracing_clone_of_a_cell_makes_two_values() {
    let o = Options {
        trace_drops: true,
        ..Options::default()
    };
    let src = format!(
        "{HEAD}{}",
        f(
            "t() -> Int",
            "none",
            "let c = Cell.new(mk(7));\nlet d = c.clone();\nreturn 0;"
        )
    );
    let r = run_with(&src, "t", &[], &o).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        r.drops.as_slice(),
        ["Probe { id: 7 }", "Probe { id: 7 }"],
        "{:?}",
        r.drops
    );
}

#[test]
fn tracing_clone_of_shared_keeps_one_value() {
    let o = Options {
        trace_drops: true,
        ..Options::default()
    };
    let src = format!(
        "{HEAD}{}",
        f(
            "t() -> Int",
            "none",
            "let c = Shared.new(Cell.new(mk(7)));\nlet d = c.clone();\nreturn 0;"
        )
    );
    let r = run_with(&src, "t", &[], &o).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.drops.as_slice(), ["Probe { id: 7 }"], "{:?}", r.drops);
}

// ---- CLI ----

#[test]
fn cli_duplicate_fs_double_is_a_usage_error() {
    let d = dir("cli");
    let a = d.join("a.txt");
    let b = d.join("b.txt");
    std::fs::write(&a, "1").unwrap();
    std::fs::write(&b, "2").unwrap();
    let ex = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/02-read-port/c-brace.sigil"
    );
    let o = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["run", "--fs-double"])
        .arg(format!("/etc/port={}", a.display()))
        .arg("--fs-double")
        .arg(format!("/etc/port={}", b.display()))
        .args([ex, "read_port", "/etc/port"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("/etc/port"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn cli_real_file_read_without_a_double() {
    let d = dir("cli-real");
    let a = d.join("port.txt");
    std::fs::write(&a, "8080").unwrap();
    let ex = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/02-read-port/c-brace.sigil"
    );
    let o = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["run", ex, "read_port"])
        .arg(&a)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "Ok(8080)\n");
    let _ = std::fs::remove_dir_all(&d);
}

// ---- console.log と Shared (非 Cell) ----

#[test]
fn cli_console_log_writes_to_real_stdout() {
    let ex = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/examples/03-ownership/ownership.sigil"
    );
    let o = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["run", ex, "main"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "150\n150\n()\n");
}

#[test]
fn console_capture_is_a_switch_and_default_leaves_output_empty() {
    let src = format!(
        "{HEAD}{}",
        f("t() -> Int", "console", "console.log(&1);\nreturn 0;")
    );
    let o = run_with(&src, "t", &[], &Options::captured()).unwrap();
    assert_eq!(o.output, "1\n");
    // 既定は本物の標準出力へ書くので、溜めない。
    let o = run_with(&src, "t", &[], &Options::default()).unwrap();
    assert_eq!(o.output, "");
}

#[test]
fn shared_of_a_struct_clone_stays_one_value() {
    let o = Options {
        trace_drops: true,
        ..Options::default()
    };
    let src = format!(
        "{HEAD}{}",
        f(
            "t() -> Int",
            "none",
            "let c = Shared.new(mk(7));\nlet d = c.clone();\nreturn 0;"
        )
    );
    let r = run_with(&src, "t", &[], &o).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(r.drops.as_slice(), ["Probe { id: 7 }"], "{:?}", r.drops);
}

// ---- ADR 0006 決定 3 の条件 2: Shared と別の共有所有の型の間に暗黙の変換がない ----

#[test]
fn no_implicit_conversion_between_shared_and_another_shared_type() {
    // テスト用に定義した別の共有所有の型（`Cell` を 1 つ持つ構造体）。
    let defs =
        "module m.a\n  targets server\n  trust none\n\nstruct MyShared {\n  c: Cell<Int>,\n}\n\n";
    let use_shared = f(
        "take(c: &Shared<Cell<Int>>) -> Int",
        "state",
        "return c.get();",
    );
    let use_mine = f("mine(m: &MyShared) -> Int", "none", "return 0;");
    let to_shared = f("a(m: &MyShared) -> Int", "state", "return take(m);");
    let to_mine = f(
        "b() -> Int",
        "state",
        "let s = Shared.new(Cell.new(1));\nreturn mine(&s);",
    );
    let ok = format!("{defs}{use_shared}{use_mine}");
    sigil::check::check_file(&sigil::parse(&ok).unwrap()).unwrap();
    for bad in [to_shared, to_mine] {
        let src = format!("{ok}{bad}");
        let r = sigil::check::check_file(&sigil::parse(&src).unwrap());
        let e = r.expect_err(&format!("型エラーになるはず:\n{src}"));
        assert_eq!(e.kind, sigil::ErrorKind::TypeMismatch, "{e}\n{src}");
    }
}

//! ADR 0014 の PR1: 出力先 `bare` と効果の表の `bare` の列。

use sigil::effects::EffectTable;
mod common;
use common::run;
use sigil::interp::Arg;
use sigil::{check::check, parse, ErrorKind};

const GOOD_FN: &str = "fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return x;\n}\n";

fn module(targets: &str) -> String {
    format!("module m.a\n  targets {targets}\n  trust none\n\n{GOOD_FN}")
}

fn parse_kind(src: &str) -> sigil::Error {
    match parse(src) {
        Ok(_) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => e,
    }
}

fn core_fn(effects: &str, body: &str) -> String {
    format!(
        "module m.a\n  targets bare\n  trust none\n\nfn g() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

// ---- 構文・整形・仕様 ----

#[test]
fn targets_bare_parses() {
    for t in ["bare", "bare, server", "bare, browser, server"] {
        let src = module(t);
        parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}"));
        assert!(sigil::fmt::check(&src).unwrap().is_none(), "{t}");
    }
}

#[test]
fn targets_bare_unsorted() {
    let e = parse_kind(&module("browser, bare"));
    assert_eq!(e.kind, ErrorKind::TargetsUnsorted);
}

#[test]
fn targets_bare_duplicate() {
    let e = parse_kind(&module("bare, bare"));
    assert_eq!(e.kind, ErrorKind::TargetsDuplicate);
}

#[test]
fn unknown_target_lists_bare() {
    let e = parse_kind(&module("metal"));
    assert_eq!(e.kind, ErrorKind::UnknownTarget);
    assert!(e.message.contains("bare・browser・server"), "{}", e.message);
}

#[test]
fn spec_shows_bare() {
    let src = std::fs::read_to_string("examples/01-average/c-brace.sigil").unwrap();
    let out = sigil::spec::spec(&src).unwrap();
    assert!(
        out.contains("  出力先: 実行環境なし, ブラウザ, サーバー\n"),
        "{out}"
    );
}

// ---- 効果の表 ----

#[test]
fn bare_column_is_state_only() {
    let t = EffectTable::standard();
    let cats = [
        "console",
        "fs.read",
        "fs.write",
        "net.client",
        "net.server",
        "db",
        "clock",
        "random",
        "env",
        "process",
        "dom",
        "browser.storage",
        "state",
        "foreign",
        "task.spawn",
        "task.suspend",
        "task.share",
    ];
    for c in cats {
        assert_eq!(t.allowed_on(c, "bare"), c == "state", "{c}");
    }
}

#[test]
fn bare_rejects_declared_console() {
    let e = check(&core_fn("console", "  return 0;")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::EffectTarget);
}

#[test]
fn bare_rejects_task_effects() {
    let e = check(&core_fn("task.suspend", "  return 0;")).unwrap_err();
    assert_eq!(e.kind, ErrorKind::EffectTarget);
}

#[test]
fn bare_allows_state() {
    let src = core_fn(
        "state",
        "  let s = Shared.new(Cell.new(1));\n  s.set(2);\n  return s.get();",
    );
    check(&src).unwrap_or_else(|e| panic!("{e}\n{src}"));
}

#[test]
fn hosted_targets_still_allow_console() {
    // `server` と `browser` の列は変わらない。
    let t = EffectTable::standard();
    assert!(t.allowed_on("console", "server") && t.allowed_on("console", "browser"));
    assert!(t.allowed_on("fs.read", "server") && !t.allowed_on("fs.read", "browser"));
}

// ---- 参照（ADR 0014 決定 4）----

fn with(targets: &str, trust: &str, items: &str) -> String {
    format!("module m.a\n  targets {targets}\n  trust {trust}\n\n{items}")
}

fn func(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

fn kind_of(src: &str) -> ErrorKind {
    match check(src) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => e.kind,
    }
}

fn ok(src: &str) {
    if let Err(e) = check(src) {
        panic!("{e}\n{src}");
    }
}

fn core_kind(items: &str) -> ErrorKind {
    kind_of(&with("bare", "none", items))
}

fn core_msg(items: &str) -> String {
    check(&with("bare", "none", items)).unwrap_err().message
}

#[test]
fn core_rejects_console_log() {
    // `effects console` を宣言しても、`OsItemInCore` が先に出る。
    for effects in ["none", "console"] {
        let items = func("f() -> Int", effects, "  console.log(&1);\n  return 0;");
        assert_eq!(core_kind(&items), ErrorKind::OsItemInCore, "{effects}");
    }
    let m = core_msg(&func(
        "f() -> Int",
        "none",
        "  console.log(&1);\n  return 0;",
    ));
    assert!(
        m.contains("console.log") && m.contains("std.os") && m.contains("bare"),
        "{m}"
    );
}

#[test]
fn core_rejects_fs_read_and_write() {
    let read = func(
        "f() -> Int",
        "fs.read",
        "  let s = \"a\";\n  let r = fs.read(&s);\n  return 0;",
    );
    assert_eq!(core_kind(&read), ErrorKind::OsItemInCore);
    let write = func(
        "f() -> Int",
        "fs.write",
        "  let s = \"a\";\n  let r = fs.write(&s, &s);\n  return 0;",
    );
    assert_eq!(core_kind(&write), ErrorKind::OsItemInCore);
}

#[test]
fn core_rejects_path_param() {
    let items = func("f(p: &Path) -> Bool", "none", "  return true;");
    assert_eq!(core_kind(&items), ErrorKind::OsItemInCore);
}

#[test]
fn core_rejects_ioerror_in_return() {
    let items = func("f() -> Result<Int, IoError>", "none", "  return Ok(1);");
    assert_eq!(core_kind(&items), ErrorKind::OsItemInCore);
}

#[test]
fn core_rejects_path_new() {
    let items = func(
        "f() -> Int",
        "none",
        "  let s = \"a\";\n  let p = Path.new(&s);\n  return 0;",
    );
    assert_eq!(core_kind(&items), ErrorKind::OsItemInCore);
}

#[test]
fn core_rejects_path_annotation() {
    let items = func(
        "f() -> Int",
        "none",
        "  let p: Option<Path> = None;\n  return 0;",
    );
    assert_eq!(core_kind(&items), ErrorKind::OsItemInCore);
}

#[test]
fn core_rejects_os_type_in_struct_and_enum() {
    assert_eq!(
        core_kind("struct S {\n  p: Path,\n}\n\n"),
        ErrorKind::OsItemInCore
    );
    assert_eq!(
        core_kind("enum E {\n  A(IoError),\n}\n\n"),
        ErrorKind::OsItemInCore
    );
}

#[test]
fn core_rejects_os_type_in_fn_type() {
    let a = func("f(g: fn(&Path) -> Bool) -> Bool", "none", "  return true;");
    assert_eq!(core_kind(&a), ErrorKind::OsItemInCore);
    let b = func(
        "f(g: fn(Int) -> Option<IoError>) -> Bool",
        "none",
        "  return true;",
    );
    assert_eq!(core_kind(&b), ErrorKind::OsItemInCore);
}

#[test]
fn core_rejects_os_type_in_closure() {
    let items = func(
        "f(xs: &List<Int>) -> List<Int>",
        "none",
        "  return xs.map(|x| {\n    let p: Option<Path> = None;\n    return x;\n  });",
    );
    assert_eq!(core_kind(&items), ErrorKind::OsItemInCore);
}

#[test]
fn core_with_server_still_rejects() {
    let items = func("f() -> Int", "console", "  console.log(&1);\n  return 0;");
    assert_eq!(
        kind_of(&with("bare, server", "none", &items)),
        ErrorKind::OsItemInCore
    );
}

#[test]
fn core_trust_base_still_rejects() {
    let items = func("f() -> Int", "console", "  console.log(&1);\n  return 0;");
    assert_eq!(
        kind_of(&with("bare", "base", &items)),
        ErrorKind::OsItemInCore
    );
}

// ---- 通る例 ----

#[test]
fn core_allows_core_items() {
    let items = func(
        "f(xs: &List<Int>, s: &String) -> Int",
        "none",
        "  let ys = xs.map(|x| x + 1);\n  let n = ys.len() + s.len();\n  let o = Some(n);\n  let r = s.parse_int();\n  let t = s.trim();\n  let m = Int.min_value();\n  let c = ys.clone();\n  let k = to_int(to_float(n));\n  return o.unwrap_or(m) + r.unwrap_or(0) + t.len() + c.len() + k.unwrap_or(0);",
    );
    ok(&with("bare", "none", &items));
}

#[test]
fn example_01_is_core() {
    let src = std::fs::read_to_string("examples/01-average/c-brace.sigil").unwrap();
    assert!(src.contains("targets bare, browser, server"));
    ok(&src);
    let o = run(
        &src,
        "average",
        &[Arg::List(vec![
            Arg::Float(1.0),
            Arg::Float(2.0),
            Arg::Float(4.5),
        ])],
    )
    .unwrap();
    assert_eq!(o.value, "2.5");
}

#[test]
fn hosted_passes_console_fn_to_core_hof() {
    let items = func(
        "f(xs: &List<Int>) -> List<Int>",
        "console",
        "  return xs.map(|x| {\n    console.log(&x);\n    return x;\n  });",
    );
    ok(&with("server", "none", &items));
}

#[test]
fn core_generic_accepts_path_type_arg() {
    let items = func(
        "f() -> Int",
        "none",
        "  let s = \"a\";\n  let p = Path.new(&s);\n  let mut xs = List.new();\n  xs.push(p);\n  let o: Option<Path> = None;\n  let g = xs.get(0);\n  return xs.len();",
    );
    ok(&with("server", "none", &items));
}

#[test]
fn hosted_modules_unchanged() {
    let items = func(
        "f() -> Int",
        "console, fs.read",
        "  let s = \"a\";\n  let p = Path.new(&s);\n  console.log(&1);\n  let r = fs.read(&p);\n  return 0;",
    );
    ok(&with("server", "none", &items));
}

// ---- 複数のファイル（ADR 0014 PR2b） ----

fn hosted_file(targets: &str, trust: &str, name: &str) -> String {
    format!(
        "module {name}\n  targets {targets}\n  trust {trust}\n\nfn h() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects console\n{{\n  console.log(&1);\n  return 0;\n}}\n"
    )
}

fn core_file(targets: &str, trust: &str, name: &str) -> String {
    format!(
        "module {name}\n  targets {targets}\n  trust {trust}\n\nfn g() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 1;\n}}\n"
    )
}

fn mismatch(a: &str, b: &str) {
    for srcs in [[a, b], [b, a]] {
        match sigil::interp::run_sources(&srcs, "g", &[]) {
            Err(f) => match f.error {
                sigil::interp::RunError::Check(e) => {
                    assert_eq!(e.kind, ErrorKind::ModuleMismatch, "{e}")
                }
                other => panic!("RunError::Check のはず: {other}"),
            },
            Ok(_) => panic!("エラーになるはずが成功した"),
        }
    }
}

#[test]
fn run_sources_rejects_mismatched_modules() {
    mismatch(
        &hosted_file("server", "none", "m.a"),
        &core_file("bare", "none", "m.a"),
    );
}

#[test]
fn run_sources_rejects_mismatched_name_and_trust() {
    mismatch(
        &core_file("bare", "none", "m.a"),
        &core_file("bare", "none", "m.b"),
    );
    mismatch(
        &core_file("bare", "none", "m.a"),
        &core_file("bare", "base", "m.a"),
    );
}

#[test]
fn run_sources_accepts_same_module() {
    let a = core_file("bare", "none", "m.a");
    let b = "module m.a\n  targets bare\n  trust none\n\nfn k() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return g();\n}\n";
    let o = sigil::interp::run_sources(&[&a, b], "k", &[]).unwrap();
    assert_eq!(o.value, "1");
}

#[test]
fn run_sources_rejects_mismatched_targets_only() {
    let e = sigil::interp::run_sources(
        &[
            &core_file("bare", "none", "m.a"),
            &core_file("server", "none", "m.a"),
        ],
        "g",
        &[],
    )
    .unwrap_err();
    assert!(matches!(e.error, sigil::interp::RunError::Check(_)));
}

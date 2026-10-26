//! ADR 0021 の 2-2c: モジュールをまたぐ lowering・実行・ネイティブ出力。
//!
//! 複数のモジュールを 1 つのプログラムに結ぶ。入口でないモジュールの関数の記号は
//! `<モジュール>.<関数>` になるので、同じ名前の関数がモジュールごとにあっても取り違えない。
//! `common::run_sources` は SIR の検証器と評価器とネイティブの実行ファイルの結果を突き合わせる。
mod common;

use common::native::{exec, scratch};
use sigil::exec::{Arg, Options, RunError};
use std::process::Command;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n";

fn module(name: &str, uses: &str, items: &str) -> String {
    format!("module {name}\n  targets server\n  trust none\n{uses}\n{items}")
}

fn func(sig: &str, effects: &str, body: &str) -> String {
    format!("{sig}\n{TAIL}  effects {effects}\n{{\n{body}\n}}\n")
}

/// 3 つのモジュールが、同じ名前 `helper` の関数をそれぞれ持つ。
fn trio() -> [String; 3] {
    let main = module(
        "app.main",
        "\nuse lib.a.run_a;\nuse lib.b.run_b;\n",
        &format!(
            "{}\n{}",
            func("fn helper(n: Int) -> Int", "none", "  return n + 1000;"),
            func(
                "pub fn main(n: Int) -> Int",
                "console",
                "  console.log(&helper(n));\n  return run_a(n) + run_b(n) + helper(n);"
            )
        ),
    );
    let a = module(
        "lib.a",
        "",
        &format!(
            "{}\n{}",
            func("fn helper(n: Int) -> Int", "none", "  return n * 2;"),
            func(
                "pub fn run_a(n: Int) -> Int",
                "console",
                "  console.log(&helper(n));\n  return helper(n) + 1;"
            )
        ),
    );
    // `shadow` の局所変数 `helper` は、同じ名前の関数 `helper` とは別のもの。
    let b = module(
        "lib.b",
        "",
        &format!(
            "{}\n{}\n{}",
            func("fn helper(n: Int) -> Int", "none", "  return n * 3;"),
            func(
                "fn shadow(n: Int) -> Int",
                "none",
                "  let helper = 5;\n  return n + helper;"
            ),
            func(
                "pub fn run_b(n: Int) -> Int",
                "console",
                "  console.log(&helper(n));\n  return helper(n) + shadow(n);"
            )
        ),
    );
    [main, a, b]
}

#[test]
fn same_named_functions_in_three_modules_are_called_apart() {
    let [m, a, b] = trio();
    // 7: main=1007 を出し、run_a は 14 を出して 15、run_b は 21 を出して 21+12=33。合計 15+33+1007。
    let r = common::run_sources(&[&m, &a, &b], "main", &[Arg::Int(7)]).unwrap();
    assert_eq!(r.output, "1007\n14\n21\n");
    assert_eq!(r.value, "1055");
    // 並びの順（入口の次）を替えても同じ。
    let r = common::run_sources(&[&m, &b, &a], "main", &[Arg::Int(7)]).unwrap();
    assert_eq!(
        (r.output.as_str(), r.value.as_str()),
        ("1007\n14\n21\n", "1055")
    );
    let r = common::run_sources(&[&m, &a, &b], "main", &[Arg::Int(0)]).unwrap();
    assert_eq!(
        (r.output.as_str(), r.value.as_str()),
        ("1000\n0\n0\n", "1006")
    );
}

#[test]
fn symbols_carry_the_module_name() {
    let [m, a, b] = trio();
    let p = sigil::ir_eval::lower_sources(&[&m, &a, &b], "main").unwrap();
    let names: Vec<&str> = p.funcs.iter().map(|f| f.name.as_str()).collect();
    for want in [
        "main",
        "helper",
        "lib.a.helper",
        "lib.a.run_a",
        "lib.b.helper",
        "lib.b.shadow",
    ] {
        assert!(names.contains(&want), "{want} がない: {names:?}");
    }
    let mut uniq = names.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), names.len(), "関数名が重なっている: {names:?}");
    // 実行ファイルの記号にもモジュール名が出る（番号つきで一意）。
    let dir = scratch("sym");
    let exe = dir.join("prog");
    common::native::try_build(&[&m, &a, &b], "main", &exe, false).unwrap();
    let bytes = std::fs::read(&exe).unwrap();
    let has = |s: &str| bytes.windows(s.len()).any(|w| w == s.as_bytes());
    assert!(has("_lib.a.helper"), "lib.a.helper の記号がない");
    assert!(has("_lib.b.helper"), "lib.b.helper の記号がない");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_the_first_module_provides_entry_functions() {
    // 入口は第 1 のモジュールの関数。ほかのモジュールの関数は、入口にはならない。
    let [m, a, b] = trio();
    let e = sigil::ir_eval::run_sources(&[&m, &a, &b], "run_a", &[Arg::Int(1)]).unwrap_err();
    assert!(matches!(e.error, RunError::Entry(_)), "{}", e.error);
}

#[test]
fn same_named_types_in_two_modules_are_unsupported_at_link() {
    // どちらも非公開の `P`。型の名前は記号と違って平らなので、結ぶと衝突する（今は未対応として止める）。
    let s = "struct P {\n  a: Int,\n}\n";
    let m = module(
        "app.main",
        "\nuse lib.a.f;\n",
        &format!(
            "{s}\n{}",
            func(
                "pub fn main() -> Int",
                "none",
                "  let q = P { a: 1 };\n  return f(q.a);"
            )
        ),
    );
    let a = module(
        "lib.a",
        "",
        &format!(
            "{s}\n{}",
            func(
                "pub fn f(n: Int) -> Int",
                "none",
                "  let q = P { a: n };\n  return q.a;"
            )
        ),
    );
    let e = sigil::ir_eval::run_sources(&[&m, &a], "main", &[]).unwrap_err();
    let text = e.error.to_string();
    assert!(text.contains("`P`"), "{text}");
    assert!(
        matches!(&e.error, RunError::Unsupported(_))
            || matches!(&e.error, RunError::Check(c) if c.kind == sigil::ErrorKind::Unsupported),
        "{text}"
    );
}

#[test]
fn non_entry_trust_base_module_is_unsupported_at_link() {
    let m = module(
        "app.main",
        "\nuse lib.a.f;\n",
        &func("pub fn main() -> Int", "none", "  return f(1);"),
    );
    let a = "module lib.a\n  targets server\n  trust base\n\n".to_string()
        + &func("pub fn f(n: Int) -> Int", "none", "  return n;");
    let opts = Options {
        trust_base: vec!["lib.a".into()],
        ..Options::captured()
    };
    let e = sigil::ir_eval::run_sources_with(&[&m, &a], "main", &[], &opts).unwrap_err();
    let text = e.error.to_string();
    assert!(text.contains("trust base"), "{text}");
}

// ---- CLI: `--root`（既定は cwd）----

fn stage_trio(tag: &str) -> std::path::PathBuf {
    let [m, a, b] = trio();
    let root = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sigil-2c-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (p, t) in [("app/main", &m), ("lib/a", &a), ("lib/b", &b)] {
        let f = root.join(format!("{p}.sigil"));
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    root
}

#[test]
fn cli_run_and_build_link_modules_and_match() {
    let root = stage_trio("cli");
    let bin = env!("CARGO_BIN_EXE_sigil");
    // `--root` なし: 根は cwd。
    let o = Command::new(bin)
        .current_dir(&root)
        .args(["run", "app/main.sigil", "main", "7"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let ran = String::from_utf8_lossy(&o.stdout).into_owned();
    assert_eq!(ran, "1007\n14\n21\n1055\n");
    // `build` も同じ結び方で、実行ファイルの出力は `run` と一致する。
    let dir = scratch("cli2c");
    let exe = dir.join("prog");
    let o = Command::new(bin)
        .current_dir(&root)
        .args(["build", "app/main.sigil", "main", "-o"])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(exec(&exe, &[Arg::Int(7)]).stdout, ran);
    // `--root` を明示しても同じ。cwd と別の場所からでよい。
    let o = Command::new(bin)
        .arg("run")
        .arg("--root")
        .arg(&root)
        .arg(root.join("app/main.sigil"))
        .args(["main", "7"])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout), ran);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root);
}

/// 取り込んだ関数 `step` と同じ綴りの局所束縛（引数・クロージャの引数・match の腕）は、取り込みの名前を隠す。
/// 隠しが外れると `step` が `lib.a.step` に書き換わり、検査か結果で落ちる。
fn shadow_sources(body: &str, sig: &str) -> [String; 2] {
    let main = module(
        "app.main",
        "\nuse lib.a.step;\n",
        &format!(
            "{}\n{}",
            func(sig, "none", body),
            func(
                "pub fn main(n: Int) -> Int",
                "none",
                "  return step(n) + probe(n);"
            )
        ),
    );
    let a = module(
        "lib.a",
        "",
        &func("pub fn step(n: Int) -> Int", "none", "  return n * 7;"),
    );
    [main, a]
}

fn run_shadow(body: &str, sig: &str) -> String {
    let [m, a] = shadow_sources(body, sig);
    let r = common::run_sources(&[&m, &a], "main", &[Arg::Int(1)]).unwrap();
    r.value
}

#[test]
fn fn_param_shadows_an_imported_function_name() {
    // 引数 `step` は整数。取り込みの `step(n)` は 7、probe は 1 + 1。
    assert_eq!(
        run_shadow("  return step + 1;", "fn probe(step: Int) -> Int"),
        "9"
    );
}

#[test]
fn closure_param_shadows_an_imported_function_name() {
    assert_eq!(
        run_shadow(
            "  let f = move |step| step + 10;\n  return f(n);",
            "fn probe(n: Int) -> Int"
        ),
        "18"
    );
}

#[test]
fn match_expression_arm_binding_shadows_an_imported_function_name() {
    assert_eq!(
        run_shadow(
            "  return match Some(n) {\n    Some(step) => step + 1000,\n    None => 0,\n  };",
            "fn probe(n: Int) -> Int"
        ),
        "1008"
    );
}

#[test]
fn match_statement_arm_binding_shadows_an_imported_function_name() {
    assert_eq!(
        run_shadow(
            "  match Some(n) {\n    Some(step) => {\n      return step + 100;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 0;",
            "fn probe(n: Int) -> Int"
        ),
        "108"
    );
}

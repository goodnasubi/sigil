//! 標準ライブラリ（仮の範囲。src/stdlib.rs）の効果と、ADR 0006・ADR 0011 の M1 完了条件のうち
//! 標準ライブラリの関数を使って確かめるもののテスト。

use sigil::check::check;
use sigil::ErrorKind;

fn fx(target: &str, sig: &str, effects: &str, body: &str) -> String {
    format!(
        "module m.a\n  targets {target}\n  trust none\n\nfn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

fn kind(src: &str) -> ErrorKind {
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

// ---- 効果（ADR 0004）。04-effects のエラー例 3 件を標準ライブラリで確かめる ----

#[test]
fn console_log_needs_console() {
    ok(&fx("server", "f(n: Int)", "console", "  console.log(&n);"));
    // 宣言の不足
    assert_eq!(
        kind(&fx("server", "f(n: Int)", "none", "  console.log(&n);")),
        ErrorKind::EffectMissing
    );
    // 宣言の余分
    assert_eq!(
        kind(&fx("server", "f(n: Int)", "console", "  return;")),
        ErrorKind::EffectExtra
    );
}

#[test]
fn fs_read_is_rejected_in_browser_modules() {
    let body = "  let r = fs.read(&p);\n  return r.is_ok();";
    ok(&fx("server", "f(p: Path) -> Bool", "fs.read", body));
    assert_eq!(
        kind(&fx("browser", "f(p: Path) -> Bool", "fs.read", body)),
        ErrorKind::EffectTarget
    );
    // fs.write は別のカテゴリ
    assert_eq!(
        kind(&fx("server", "f(p: Path) -> Bool", "fs.write", body)),
        ErrorKind::EffectMissing
    );
    // 宣言と推論は一致させる（`fs` は fs.write も含むので、本体が起こさない分が余分になる）
    assert_eq!(
        kind(&fx("server", "f(p: Path) -> Bool", "fs", body)),
        ErrorKind::EffectExtra
    );
}

#[test]
fn shared_cell_get_and_set_have_state_effect() {
    let get = "  return c.get();";
    ok(&fx(
        "server",
        "f(c: &Shared<Cell<Int>>) -> Int",
        "state",
        get,
    ));
    assert_eq!(
        kind(&fx(
            "server",
            "f(c: &Shared<Cell<Int>>) -> Int",
            "none",
            get
        )),
        ErrorKind::EffectMissing
    );
    ok(&fx(
        "server",
        "g(c: &Shared<Cell<Int>>)",
        "state",
        "  c.set(1);",
    ));
}

#[test]
fn std_higher_order_methods_inherit_the_effect_of_the_argument() {
    // List.map は of(f) を引き継ぐ
    let pure = "  return xs.map(|x| x + 1);";
    ok(&fx(
        "server",
        "f(xs: &List<Int>) -> List<Int>",
        "none",
        pure,
    ));
    let noisy =
        "  let g = move |x| {\n    console.log(&x);\n    return x;\n  };\n  return xs.map(g);";
    assert_eq!(
        kind(&fx(
            "server",
            "f(xs: &List<Int>) -> List<Int>",
            "none",
            noisy
        )),
        ErrorKind::EffectMissing
    );
    ok(&fx(
        "server",
        "f(xs: &List<Int>) -> List<Int>",
        "console",
        noisy,
    ));
}

// ---- ADR 0011 の完了条件のうち、標準ライブラリの型を使うもの ----

#[test]
fn list_of_refinement_is_not_list_of_base() {
    // `List<Port>` を `List<Int>` の引数に渡すとエラー
    let src = "module m.a\n  targets server\n  trust none\n\ntype Port = Int where 1 <= self && self <= 65535;\n\nfn take(xs: &List<Int>)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\n\nfn f(ps: &List<Port>)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  take(ps);\n}\n";
    assert_eq!(kind(src), ErrorKind::TypeMismatch);
}

#[test]
fn ok_unit_and_try_operator_with_std_results() {
    ok(&fx(
        "server",
        "f() -> Result<Unit, IoError>",
        "none",
        "  return Ok(Unit);",
    ));
    // `?` で E が異なる Result はエラー
    assert_eq!(
        kind(&fx(
            "server",
            "f(p: Path) -> Result<String, ParseError>",
            "fs.read",
            "  let s = fs.read(&p)?;\n  return Ok(s);"
        )),
        ErrorKind::TryOperand
    );
    ok(&fx(
        "server",
        "f(p: Path) -> Result<String, IoError>",
        "fs.read",
        "  let s = fs.read(&p)?;\n  return Ok(s);",
    ));
}

#[test]
fn std_fns_with_comparable_bounds() {
    ok(&fx(
        "server",
        "f(xs: &List<Float>) -> Float",
        "none",
        "  return xs.min();",
    ));
}

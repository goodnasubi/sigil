//! ADR 0025 補足 A-1: 大域（`global`）の型規則・構文検査（PR 3-4c）。
//! 各違反のエラー種別と、正常系を固定する。

use sigil::check::check_with;
use sigil::effects::Options;
use sigil::error::ErrorKind;
use sigil::parser::parse;
use sigil::project::link_sources;

fn opts(mods: &[&str]) -> Options {
    Options {
        trust_base: mods.iter().map(|m| m.to_string()).collect(),
        ..Default::default()
    }
}

fn base(items: &str) -> String {
    format!("module m.test\n  targets server\n  trust base\n\n{items}")
}

fn kind(items: &str) -> ErrorKind {
    check_with(&base(items), &opts(&["m.test"]))
        .expect_err("エラーになるはず")
        .kind
}

fn ok(items: &str) {
    check_with(&base(items), &opts(&["m.test"])).expect("検査を通るはず");
}

const UNCHECKED_FN: &str = "fn touch() -> Int
  intent \"touch\"
  requires none
  ensures none
  effects none
  unchecked \"単一スレッド\"
{
  return C;
}
";

// ---- pub global ----

#[test]
fn pub_global_is_a_syntax_error() {
    for decl in ["pub global A: Int = 0", "pub global mut A: Int = 0"] {
        let src = base(&format!("{decl}\n"));
        let err = parse(&src).expect_err("構文エラーのはず");
        assert_eq!(err.kind, ErrorKind::PubGlobal, "{decl}");
        assert_eq!(err.span.line, 5, "{decl}");
    }
}

// ---- 型は Copyable に限る ----

#[test]
fn global_type_must_be_copyable() {
    for ty in ["String", "List<Int>", "(Int, String)", "Option<Int>"] {
        let k = kind(&format!("global A: {ty} = 0\n"));
        assert_eq!(k, ErrorKind::GlobalNotCopyable, "{ty}");
    }
    // 破棄の要る型を含む構造体
    let k = kind("struct S {\n  s: String,\n}\n\nglobal mut A: S = S { s: \"x\" }\n");
    assert_eq!(k, ErrorKind::GlobalNotCopyable);
}

#[test]
fn global_copyable_types_pass() {
    ok("struct P {\n  a: Int,\n  b: Bool,\n  c: Addr,\n}\n\n\
        global mut A: Int = 0\n\
        global B: Bool = true\n\
        global C: Addr = raw.addr_null()\n\
        global mut D: P = P { a: 1, b: false, c: raw.addr_null() }\n\
        global E: (Int, Bool) = (1, true)\n\
        global F: Int = -5\n");
}

// ---- 初期化式は定数のみ ----

#[test]
fn global_init_must_be_const() {
    let cases = [
        "global A: Int = 1 + 2",
        "global A: Int = -(1)\nglobal B: Int = 1 + 1",
        "global OTHER: Int = 0\nglobal A: Int = OTHER",
        "global A: Int = id(3)\n\nfn id(n: Int) -> Int\n  intent \"id\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}",
        "global A: Addr = raw.alloc(8, 8)",
        "global A: Addr = raw.offset(raw.addr_null(), 1)",
        "global A: Float = 1.5",
        "global A: Int = match 1 {\n  _ => 2,\n}",
    ];
    for c in cases {
        assert_eq!(
            kind(&format!("{c}\n")),
            ErrorKind::GlobalInitNotConst,
            "{c}"
        );
    }
    // 構造体の欄・組の要素の中の非定数も拒む
    let k = kind("struct P {\n  a: Int,\n}\n\nglobal A: P = P { a: 1 + 1 }\n");
    assert_eq!(k, ErrorKind::GlobalInitNotConst);
    assert_eq!(
        kind("global A: (Int, Int) = (1, 2 + 3)\n"),
        ErrorKind::GlobalInitNotConst
    );
}

#[test]
fn raw_addr_null_is_allowed_only_for_global_init() {
    // 特例は大域の初期化式に限る: 通常の関数の本体では従来どおり `RawOutsideUnchecked`
    let k = kind(
        "global A: Addr = raw.addr_null()\n\nfn f() -> Addr\n  intent \"f\"\n  requires none\n  ensures none\n  effects none\n{\n  return raw.addr_null();\n}\n",
    );
    assert_eq!(k, ErrorKind::RawOutsideUnchecked);
}

#[test]
fn global_init_does_not_inherit_previous_fn_state() {
    // 直前の関数の引数の名前・`unchecked` の状態を、後ろの大域の初期化式が引き継がない。
    let k = kind(
        "fn f(n: Int) -> Int\n  intent \"f\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"x\"\n{\n  return n;\n}\n\nglobal A: Int = n\n"
    );
    assert_eq!(k, ErrorKind::GlobalInitNotConst);
    // `unchecked` の関数の後でも、`raw.alloc` は大域の初期化式に書けない
    let k = kind(&format!(
        "global C: Int = 0\n\n{UNCHECKED_FN}\nglobal A: Addr = raw.alloc(8, 8)\n"
    ));
    assert_eq!(k, ErrorKind::GlobalInitNotConst);
    // 定数の初期化式の検査が、前の関数に影響されず通る
    ok(&format!(
        "global C: Int = 0\n\n{UNCHECKED_FN}\nglobal A: Addr = raw.addr_null()\n"
    ));
}

// ---- 中断しうる効果の禁止 ----

fn mut_fn(effects: &str) -> String {
    format!(
        "global mut COUNTER: Int = 0\n\nfn bump() -> Int\n  intent \"bump\"\n  requires none\n  ensures none\n  effects {effects}\n  unchecked \"単一スレッド\"\n{{\n  COUNTER = COUNTER + 1;\n  return COUNTER;\n}}\n"
    )
}

#[test]
fn global_mut_fn_cannot_declare_suspending_effects() {
    for eff in [
        "task.suspend",
        "task.spawn",
        "task",
        "task.share, task.suspend",
    ] {
        assert_eq!(kind(&mut_fn(eff)), ErrorKind::GlobalMutSuspend, "{eff}");
    }
    // 読むだけでも `global mut` に触る
    let k = kind(
        "global mut COUNTER: Int = 0\n\nfn peek() -> Int\n  intent \"peek\"\n  requires none\n  ensures none\n  effects task.suspend\n  unchecked \"x\"\n{\n  return COUNTER;\n}\n",
    );
    assert_eq!(k, ErrorKind::GlobalMutSuspend);
}

#[test]
fn global_mut_fn_with_other_effects_or_none_passes() {
    ok(&mut_fn("none"));
    // 中断しない効果は妨げない
    let src = "global mut COUNTER: Int = 0\n\nfn bump() -> Int\n  intent \"bump\"\n  requires none\n  ensures none\n  effects console\n  unchecked \"単一スレッド\"\n{\n  console.log(&\"x\");\n  COUNTER = COUNTER + 1;\n  return COUNTER;\n}\n";
    ok(src);
    // `global mut` に触らない関数は、中断する効果を宣言できる（この規則の対象外）
    let k = check_with(
        &base(
            "global mut COUNTER: Int = 0\n\nfn other() -> Int\n  intent \"other\"\n  requires none\n  ensures none\n  effects task.suspend\n{\n  return 1;\n}\n",
        ),
        &opts(&["m.test"]),
    )
    .expect_err("効果の宣言が余っているので別のエラー")
    .kind;
    assert_ne!(k, ErrorKind::GlobalMutSuspend);
}

#[test]
fn global_mut_touch_flag_does_not_leak_to_next_fn() {
    // 1 つ目が `global mut` に触り、2 つ目は触らない。2 つ目の効果は規則の対象外。
    let src = format!(
        "{}\nfn other() -> Int\n  intent \"other\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 1;\n}}\n",
        mut_fn("none")
    );
    ok(&src);
}

// ---- 不変の global は通常関数から読める ----

#[test]
fn immutable_global_is_readable_from_plain_fn() {
    ok("global LIMIT: Int = 42\n\nfn limit() -> Int\n  intent \"limit\"\n  requires none\n  ensures none\n  effects none\n{\n  return LIMIT;\n}\n");
    // 不変の global に触る関数は、中断しうる効果を宣言してもこの規則では拒まれない
    let k = check_with(
        &base("global LIMIT: Int = 42\n\nfn limit() -> Int\n  intent \"limit\"\n  requires none\n  ensures none\n  effects task.suspend\n{\n  return LIMIT;\n}\n"),
        &opts(&["m.test"]),
    )
    .expect_err("効果が余る")
    .kind;
    assert_ne!(k, ErrorKind::GlobalMutSuspend);
}

#[test]
fn global_mut_in_plain_fn_is_still_rejected() {
    let k = kind("global mut C: Int = 0\n\nfn f() -> Int\n  intent \"f\"\n  requires none\n  ensures none\n  effects none\n{\n  return C;\n}\n");
    assert_eq!(k, ErrorKind::GlobalMutOutsideUnchecked);
}

// ---- 複数モジュール（resolve が初期化式の名前を付け替える） ----

#[test]
fn global_init_names_are_resolved_in_project() {
    let lib = "module lib.cfg
  targets server
  trust none

pub fn one() -> Int
  intent \"1\"
  requires none
  ensures none
  effects none
{
  return 1;
}
";
    let app = "module app.main
  targets server
  trust base

use lib.cfg.one;

struct Conf {
  a: Int,
  b: Bool,
}

global DEFAULT: Conf = Conf { a: 7, b: true }

fn main() -> Int
  intent \"入口\"
  requires none
  ensures none
  effects none
{
  return DEFAULT.a + one();
}
";
    link_sources(&[app, lib], &opts(&["app.main"])).expect("結合できる");
}

#[test]
fn global_mut_write_only_fn_cannot_declare_suspending_effects() {
    // 書き込みだけ(読まない)でも `global mut` に触る
    let k = kind(
        "global mut COUNTER: Int = 0\n\nfn reset()\n  intent \"reset\"\n  requires none\n  ensures none\n  effects task.suspend\n  unchecked \"x\"\n{\n  COUNTER = 1;\n  return;\n}\n",
    );
    assert_eq!(k, ErrorKind::GlobalMutSuspend);
}

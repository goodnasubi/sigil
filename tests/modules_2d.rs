//! ADR 0022 決定 1: 型の同一性と名前の解決（明示・暗黙・曖昧な名前を書いた箇所での `UseConflict`）。
//! 期待値は決め打ち（エラー種別・ファイル・行）。
#![allow(dead_code)]

use sigil::effects::Options;
use sigil::project::{check_project, ProjectError};
use sigil::ErrorKind;
use std::path::PathBuf;

const TAIL: &str =
    "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n";

fn module_t(name: &str, targets: &str, uses: &str, items: &str) -> String {
    format!("module {name}\n  targets {targets}\n  trust none\n{uses}\n{items}")
}

fn module(name: &str, uses: &str, items: &str) -> String {
    module_t(name, "server", uses, items)
}

fn pub_fn(name: &str) -> String {
    format!("pub fn {name}(n: Int) -> Int\n{TAIL}")
}

fn priv_fn(name: &str) -> String {
    format!("fn {name}(n: Int) -> Int\n{TAIL}")
}

fn caller(name: &str, callee: &str) -> String {
    format!(
        "pub fn {name}(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return {callee}(n);\n}}\n"
    )
}

/// 根の下の一時ディレクトリ。
struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sigil-2b-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // macOS などの /tmp の別名を避ける
        Tmp(d.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, src: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, src).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// (種別, 診断のあるファイルの根からの相対パス, 行)
type Got = Result<(), (ErrorKind, String, u32)>;

fn check(t: &Tmp, entry: &str) -> Got {
    to_got(
        t,
        check_project(&t.0, &t.0.join(entry), &Options::default()),
    )
}

fn to_got(t: &Tmp, r: Result<(), ProjectError>) -> Got {
    match r {
        Ok(()) => Ok(()),
        Err(ProjectError::Diag { file, error }) => Err((
            error.kind,
            file.strip_prefix(&t.0).unwrap().display().to_string(),
            error.span.line,
        )),
        Err(ProjectError::Read { file, msg }) => panic!("読めない {}: {msg}", file.display()),
    }
}

/// `lib/util.sigil`: 公開の fn `f`・公開の struct `P`・公開の enum `E`、非公開の fn `g`・struct `Hidden`。
fn util(t: &Tmp) {
    let items = format!(
        "{}\n{}\npub struct P {{\n  a: Int,\n}}\n\nstruct Hidden {{\n  a: Int,\n}}\n\npub enum Shade {{\n  Light,\n  Dark,\n}}\n",
        pub_fn("f"),
        priv_fn("g"),
    );
    t.write("lib/util.sigil", &module("lib.util", "", &items));
}

// ---- 成功 ----

/// 公開の `Config` と、それを返す公開の関数を持つモジュール。
fn provider(t: &Tmp, m: &str, mk: &str) {
    let items =
        format!("pub struct Config {{\n  a: Int,\n}}\n\npub fn {mk}(n: Int) -> Config\n{TAIL_C}",);
    t.write(
        &format!("{}.sigil", m.replace('.', "/")),
        &module(m, "", &items),
    );
}

const TAIL_C: &str =
    "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Config { a: n };\n}\n";

/// `Config` を引数に取る `h` と、`g` の本体を持つ。
fn user(g_body: &str) -> String {
    let h = "pub fn h(c: Config) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n";
    let g = format!(
        "pub fn g(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{g_body}\n}}\n"
    );
    format!("{h}\n{g}")
}

const TWO: &str = "\nuse lib.p1.mk1;\nuse lib.p2.mk2;\n";

fn setup(tag: &str) -> Tmp {
    let t = Tmp::new(tag);
    provider(&t, "lib.p1", "mk1");
    provider(&t, "lib.p2", "mk2");
    t
}

#[test]
fn ambiguous_implicit_names_are_fine_until_written() {
    // 2 つの別の `Config` が暗黙の名前で現れるが、書かなければ通る（型を書かずに受け渡せる）。
    let t = setup("unwritten");
    let g = "pub fn g(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let c = mk1(n);\n  let d = mk2(n);\n  return n;\n}\n";
    t.write("app/main.sigil", &module("app.main", TWO, g));
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
}

#[test]
fn ambiguity_is_reported_where_the_name_is_written() {
    let t = setup("written");
    t.write(
        "app/main.sigil",
        &module(
            "app.main",
            TWO,
            &user("  let d = mk2(n);\n  return h(mk1(n));"),
        ),
    );
    // `use` の行（5・6 行）ではなく、`Config` を書いた `h` の宣言の行（8 行）
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UseConflict, "app/main.sigil".into(), 8))
    );
}

const SELECT: &str = "\nuse lib.p1.Config;\nuse lib.p1.mk1;\nuse lib.p2.mk2;\n";

#[test]
fn use_selects_among_implicit_candidates() {
    let t = setup("select");
    // 明示の `Config` が、暗黙の候補の 1 つ（lib.p1）を選ぶ
    let ok = user("  let d = mk2(n);\n  return h(mk1(n));");
    t.write("app/main.sigil", &module("app.main", SELECT, &ok));
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
    // 選ばなかった側の値は別の型（`g` の `return` の行）
    let bad = user("  let d = mk1(n);\n  return h(mk2(n));");
    t.write("app/main.sigil", &module("app.main", SELECT, &bad));
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::TypeMismatch, "app/main.sigil".into(), 25))
    );
}

#[test]
fn a_use_cannot_hijack_a_name_already_written() {
    // 暗黙の `Config`（lib.p1 だけ）を書いていたモジュールに、別の同一性の `Config` の `use` を足すと、
    // 書いてあった位置で `UseConflict`（黙って指す先が変わらない）。
    let t = setup("hijack");
    let body = user("  return h(mk1(n));");
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.p1.mk1;\n", &body),
    );
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.p1.mk1;\nuse lib.p2.Config;\n", &body),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UseConflict, "app/main.sigil".into(), 8))
    );
}

#[test]
fn same_identity_reached_twice_is_not_ambiguous() {
    // 同じ型が明示でも暗黙でも、同一性は 1 つ
    let t = setup("same");
    let uses = "\nuse lib.p1.Config;\nuse lib.p1.mk1;\n";
    t.write(
        "app/main.sigil",
        &module("app.main", uses, &user("  return h(mk1(n));")),
    );
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
}

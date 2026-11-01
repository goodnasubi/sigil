//! Issue #173（LSP の繰り越し low 指摘）の回帰試験。
//! - 型 `Config` のメソッド `m` と、モジュール `config.m` が名前でぶつからないこと（解消済みの再現試験）。
//! - 同じ文に同じ短い名前の別の同一性が出るときだけ、型の名前が修飾して示されること（ADR 0022 決定 1）。

use sigil::effects::Options;
use sigil::project::{check_project, ProjectError};
use std::path::PathBuf;
use std::process::Command;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

fn head(name: &str, uses: &str) -> String {
    format!("module {name}\n  targets server\n  trust none\n\n{uses}\n")
}

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sigil-173-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, src: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(t: &Tmp, entry: &str) -> Result<(), String> {
    match check_project(&t.0, &t.0.join(entry), &Options::default()) {
        Ok(()) => Ok(()),
        Err(ProjectError::Diag { error, .. }) => Err(error.message),
        Err(ProjectError::Read { file, msg }) => panic!("読めない {}: {msg}", file.display()),
    }
}

/// 型 `Config` のメソッド `m` を `Config.m(...)` で呼ぶ。同じ名前の並びのモジュール `config.m` も取り込む。
#[test]
fn type_config_m_and_module_config_m_do_not_collide() {
    let t = Tmp::new("config-m");
    t.write(
        "config/m.sigil",
        &format!(
            "{}pub fn f(n: Int) -> Int\n{TAIL}{{\n  return n;\n}}\n",
            head("config.m", "")
        ),
    );
    let main = format!(
        "{}pub struct Config {{\n  a: Int,\n}}\n\nimpl Config {{\n  pub fn m(n: Int) -> Int\n  {}  {{\n    return f(n);\n  }}\n}}\n\npub fn g(n: Int) -> Int\n{TAIL}{{\n  return Config.m(n);\n}}\n",
        head("app.main", "use config.m.f;\n"),
        TAIL.replace("\n  ", "\n    ").trim_start()
    );
    t.write("app/main.sigil", &main);
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
    // 実行しても、メソッドを呼んだ結果が返る（結合の名前もぶつからない）。
    let out = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .current_dir(&t.0)
        .args(["run", "app/main.sigil", "g", "3"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "3");
}

fn lib_config(t: &Tmp, m: &str) {
    t.write(
        &format!("lib/{m}.sigil"),
        &format!(
            "{}pub struct Config {{\n  a: Int,\n}}\n\npub fn mk(n: Int) -> Config\n{TAIL}{{\n  return Config {{ a: n }};\n}}\n\npub fn take(c: Config) -> Int\n{TAIL}{{\n  return 1;\n}}\n",
            head(&format!("lib.{m}"), "")
        ),
    );
}

fn app(body: &str) -> String {
    format!(
        "{}pub fn g(n: Int) -> Int\n{TAIL}{{\n  {body}\n}}\n",
        head("app.main", "use lib.p1.mk;\nuse lib.p2.take;\n")
    )
}

/// 同じ文に別の同一性の `Config` が 2 つ出るときは、どちらも修飾して示す。
#[test]
fn same_short_name_with_two_identities_is_qualified_in_one_message() {
    let t = Tmp::new("qualified");
    lib_config(&t, "p1");
    lib_config(&t, "p2");
    t.write("app/main.sigil", &app("return take(mk(n));"));
    let msg = check(&t, "app/main.sigil").unwrap_err();
    assert!(msg.contains("`lib.p2.Config`"), "{msg}");
    assert!(msg.contains("`lib.p1.Config`"), "{msg}");
}

/// プログラムに同じ短い名前の同一性が 1 つしかなければ、修飾しない名前で示す。
#[test]
fn single_identity_stays_unqualified() {
    let t = Tmp::new("unqualified");
    lib_config(&t, "p1");
    let src = app("return take(n);").replace("use lib.p2.take;", "use lib.p1.take;");
    t.write("app/main.sigil", &src);
    let msg = check(&t, "app/main.sigil").unwrap_err();
    assert!(
        msg.contains("`Config`") && !msg.contains("lib.p1.Config"),
        "{msg}"
    );
}

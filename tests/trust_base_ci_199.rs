//! Issue #199 / ADR 0018 判断 1（T3）・判断 2（S2）: `scripts/trust-base-diff.sh` のテスト。
//! 一時的な git リポジトリに base と head を作り、検出と終了コードを確かめる。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/trust-base-diff.sh");

fn func(name: &str, premise: &str, body: &str) -> String {
    format!(
        "  pub fn {name}(buf: &Bytes)\n    intent \"x\"\n    requires none\n    ensures none\n    effects console\n    unchecked \"{premise}\"\n  {{\n    {body}\n  }}\n"
    )
}

fn module(funcs: &[String]) -> String {
    format!(
        "pub const SRC: &str = r#\"module std.os\n  targets server\n  trust base\n\nimpl Console {{\n{}}}\n\"#;\n",
        funcs.concat()
    )
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Repo(PathBuf);

impl Repo {
    fn new(tag: &str, files: &[(&str, &str)]) -> Repo {
        let d = std::env::temp_dir().join(format!("sigil-tbci-199-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        git(&d, &["init", "-q"]);
        let r = Repo(d);
        r.write(files);
        r
    }
    fn write(&self, files: &[(&str, &str)]) {
        for (p, c) in files {
            let f = self.0.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, c).unwrap();
        }
    }
    fn commit(&self) -> String {
        git(&self.0, &["add", "-A"]);
        git(&self.0, &["commit", "-q", "--allow-empty", "-m", "c"]);
        git(&self.0, &["rev-parse", "HEAD"])
    }
    fn run(&self, base: &str, mode: &str) -> (i32, String) {
        let o = Command::new("bash")
            .current_dir(&self.0)
            .arg(SCRIPT)
            .arg(base)
            .env("SIGIL_TRUST_BASE_MODE", mode)
            .env_remove("GITHUB_STEP_SUMMARY")
            .env_remove("GITHUB_ACTIONS")
            .output()
            .unwrap();
        (
            o.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&o.stdout).into_owned(),
        )
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const F: &str = "src/stdlib/os_sigil.rs";

#[test]
fn no_change_is_quiet() {
    let r = Repo::new("none", &[(F, &module(&[func("log", "前提A", "raw.x();")]))]);
    let base = r.commit();
    r.write(&[("README.md", "other")]);
    r.commit();
    let (rc, out) = r.run(&base, "fail");
    assert_eq!(rc, 0, "{out}");
    assert!(out.contains("信頼基盤に関わる変更はない"), "{out}");
}

#[test]
fn premise_change_is_shown() {
    let r = Repo::new(
        "premise",
        &[(F, &module(&[func("log", "前提A", "raw.x();")]))],
    );
    let base = r.commit();
    r.write(&[(F, &module(&[func("log", "前提B", "raw.x();")]))]);
    r.commit();
    let (rc, out) = r.run(&base, "mark");
    assert_eq!(rc, 0, "印のみは失敗にしない: {out}");
    assert!(out.contains("前提の文の変更"), "{out}");
    assert!(
        out.contains("旧: 前提A") && out.contains("新: 前提B"),
        "{out}"
    );
    assert!(!out.contains("本体（宣言を含む）の変更"), "{out}");
    assert_eq!(r.run(&base, "fail").0, 1);
}

#[test]
fn body_only_change_is_detected() {
    let r = Repo::new("body", &[(F, &module(&[func("log", "前提A", "raw.x();")]))]);
    let base = r.commit();
    r.write(&[(F, &module(&[func("log", "前提A", "raw.y();")]))]);
    r.commit();
    let (rc, out) = r.run(&base, "fail");
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("本体（宣言を含む）の変更"), "{out}");
    assert!(out.contains("Console :: log"), "{out}");
    assert!(!out.contains("前提の文の変更"), "{out}");
}

#[test]
fn added_and_removed_functions() {
    let r = Repo::new(
        "addrm",
        &[(F, &module(&[func("old", "古い前提", "raw.x();")]))],
    );
    let base = r.commit();
    r.write(&[(F, &module(&[func("new", "新しい前提", "raw.x();")]))]);
    r.commit();
    let (rc, out) = r.run(&base, "mark");
    assert_eq!(rc, 0, "{out}");
    assert!(
        out.contains("追加: `src/stdlib/os_sigil.rs :: std.os :: Console :: new`"),
        "{out}"
    );
    assert!(
        out.contains("削除: `src/stdlib/os_sigil.rs :: std.os :: Console :: old`"),
        "{out}"
    );
    assert!(
        out.contains("前提の文: 新しい前提") && out.contains("前提の文: 古い前提"),
        "{out}"
    );
}

#[test]
fn non_unchecked_function_change_is_ignored() {
    let plain = |b: &str| {
        format!("pub const S: &str = r#\"module std.os\n  trust base\nfn f()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  {b}\n}}\n\"#;\n")
    };
    let r = Repo::new("plain", &[(F, &plain("return 1;"))]);
    let base = r.commit();
    r.write(&[(F, &plain("return 2;"))]);
    r.commit();
    let (rc, out) = r.run(&base, "fail");
    assert_eq!(rc, 0, "{out}");
}

#[test]
fn raw_ops_table_and_allow_list_changes() {
    let table = |p: &str| {
        format!("pub const RAW_OPS: &[RawRow] = &[\n    RawRow {{\n        layer: Layer::Core,\n        signature: \"offset(a: Addr, n: Int) -> Addr\",\n        effects: \"none\",\n        requires: \"none\",\n        premise: \"{p}\",\n    }},\n];\n")
    };
    let r = Repo::new(
        "table",
        &[
            ("src/stdlib.rs", &table("P1")),
            ("trust-base.list", "# c\nm.a\n"),
        ],
    );
    let base = r.commit();
    r.write(&[
        ("src/stdlib.rs", &table("P2")),
        ("trust-base.list", "m.a\nm.b\n"),
    ]);
    r.commit();
    let (rc, out) = r.run(&base, "fail");
    assert_eq!(rc, 1, "{out}");
    assert!(out.contains("変更: `raw.offset`"), "{out}");
    assert!(
        out.contains("旧の前提: P1") && out.contains("新の前提: P2"),
        "{out}"
    );
    assert!(out.contains("追加: `m.b`"), "{out}");
}

#[test]
fn missing_planned_files_are_ignored() {
    let r = Repo::new("missing", &[("a.txt", "1")]);
    let base = r.commit();
    r.write(&[("a.txt", "2")]);
    r.commit();
    let (rc, out) = r.run(&base, "fail");
    assert_eq!(rc, 0, "{out}");
}

//! ADR 0021 の PR 2-2b: 名前解決（`use`・可視性・輪・`targets`・非公開の型・構築）と
//! ファイルを開く際の差し替え対策。期待値は決め打ち（エラー種別・ファイル・位置）。

use sigil::effects::Options;
use sigil::project::{check_project, check_project_with, ProjectError};
use sigil::ErrorKind;
use std::path::{Path, PathBuf};

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

#[test]
fn import_pub_fn_and_call() {
    let t = Tmp::new("ok");
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.f;\n", &caller("run", "f")),
    );
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
}

#[test]
fn enum_variants_may_be_constructed_across_modules() {
    let t = Tmp::new("enum");
    util(&t);
    let body = "pub fn mk(n: Int) -> Shade\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Shade.Light;\n}\n";
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.Shade;\n", body),
    );
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
}

// ---- 名前の解決 ----

#[test]
fn private_import_is_rejected_at_the_use_line() {
    let t = Tmp::new("priv");
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.g;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::PrivateImport, "app/main.sigil".into(), 5))
    );
    // 非公開の型も同じ
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.Hidden;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::PrivateImport, "app/main.sigil".into(), 5))
    );
}

#[test]
fn unresolved_use_name_and_module() {
    let t = Tmp::new("unres");
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.nope;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UnresolvedUse, "app/main.sigil".into(), 5))
    );
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.missing.f;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UnresolvedUse, "app/main.sigil".into(), 5))
    );
}

#[test]
fn reexport_is_not_possible() {
    // `mid` は `util.f` を取り込んでいるが、`mid.f` としては公開されない
    let t = Tmp::new("reexp");
    util(&t);
    t.write(
        "lib/mid.sigil",
        &module("lib.mid", "\nuse lib.util.f;\n", &caller("viaf", "f")),
    );
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.mid.f;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UnresolvedUse, "app/main.sigil".into(), 5))
    );
}

#[test]
fn module_cycle_is_reported_at_the_closing_use() {
    let t = Tmp::new("cyc");
    t.write(
        "a/x.sigil",
        &module("a.x", "\nuse a.y.g;\n", &format!("{}{}", pub_fn("f"), "")),
    );
    t.write(
        "a/y.sigil",
        &module("a.y", "\nuse a.x.f;\n", &caller("g", "f")),
    );
    // 入口 a.x -> a.y -> a.x: 輪を閉じる `use`（a/y の 5 行目）
    assert_eq!(
        check(&t, "a/x.sigil"),
        Err((ErrorKind::ModuleCycle, "a/y.sigil".into(), 5))
    );
    // 自分自身を取り込む
    t.write(
        "a/me.sigil",
        &module("a.me", "\nuse a.me.f;\n", &pub_fn("f")),
    );
    assert_eq!(
        check(&t, "a/me.sigil"),
        Err((ErrorKind::ModuleCycle, "a/me.sigil".into(), 5))
    );
}

#[test]
fn target_mismatch() {
    let t = Tmp::new("tgt");
    util(&t); // targets server
    t.write(
        "app/web.sigil",
        &module_t("app.web", "browser", "\nuse lib.util.f;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/web.sigil"),
        Err((ErrorKind::TargetMismatch, "app/web.sigil".into(), 5))
    );
    // 取り込む側が取り込まれる側の部分集合ならよい
    t.write(
        "lib/both.sigil",
        &module_t("lib.both", "browser, server", "", &pub_fn("f")),
    );
    t.write(
        "app/ok.sigil",
        &module_t("app.ok", "server", "\nuse lib.both.f;\n", &caller("h", "f")),
    );
    assert_eq!(check(&t, "app/ok.sigil"), Ok(()));
    // 逆（取り込む側のほうが広い）は拒否
    t.write(
        "app/wide.sigil",
        &module_t(
            "app.wide",
            "browser, server",
            "\nuse lib.util.f;\n",
            &pub_fn("h"),
        ),
    );
    assert_eq!(
        check(&t, "app/wide.sigil"),
        Err((ErrorKind::TargetMismatch, "app/wide.sigil".into(), 5))
    );
}

#[test]
fn private_in_public_signature() {
    let t = Tmp::new("pip");
    let items = "struct Hidden {\n  a: Int,\n}\n\npub fn leak(h: Hidden) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n";
    t.write("app/a.sigil", &module("app.a", "", items));
    // 公開の fn `leak` は 9 行目
    assert_eq!(
        check(&t, "app/a.sigil"),
        Err((ErrorKind::PrivateInPublic, "app/a.sigil".into(), 9))
    );
    // 非公開の fn なら問題ない
    let ok = items.replace("pub fn leak", "fn leak");
    t.write("app/a.sigil", &module("app.a", "", &ok));
    assert_eq!(check(&t, "app/a.sigil"), Ok(()));
}

#[test]
fn private_construction_of_imported_struct() {
    let t = Tmp::new("pc");
    util(&t);
    let body = "pub fn mk(n: Int) -> P\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return P { a: n };\n}\n";
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.P;\n", body),
    );
    // 構築の式は 13 行目
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::PrivateConstruction, "app/main.sigil".into(), 13))
    );
}

#[test]
fn use_conflicts() {
    let t = Tmp::new("conf");
    util(&t);
    t.write("lib/other.sigil", &module("lib.other", "", &pub_fn("f")));
    // 別のモジュールの同じ名前（2 つ目の use が 6 行目）
    t.write(
        "app/a.sigil",
        &module(
            "app.a",
            "\nuse lib.other.f;\nuse lib.util.f;\n",
            &pub_fn("h"),
        ),
    );
    assert_eq!(
        check(&t, "app/a.sigil"),
        Err((ErrorKind::UseConflict, "app/a.sigil".into(), 6))
    );
    // 自分の宣言と同じ名前
    t.write(
        "app/b.sigil",
        &module("app.b", "\nuse lib.util.f;\n", &pub_fn("f")),
    );
    assert_eq!(
        check(&t, "app/b.sigil"),
        Err((ErrorKind::UseConflict, "app/b.sigil".into(), 5))
    );
}

#[test]
fn unused_use() {
    let t = Tmp::new("unused");
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.f;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UnusedUse, "app/main.sigil".into(), 5))
    );
}

#[test]
fn error_in_dependency_is_reported_in_its_own_file() {
    let t = Tmp::new("dep");
    t.write(
        "lib/bad.sigil",
        &module("lib.bad", "", "pub fn f(n: Int) -> Bool\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n"),
    );
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.bad.f;\n", &caller("h", "f")),
    );
    let (k, f, line) = check(&t, "app/main.sigil").unwrap_err();
    assert_eq!(
        (f.as_str(), line, k),
        ("lib/bad.sigil", 11, ErrorKind::TypeMismatch)
    );
}

#[test]
fn hidden_dependency_types_come_along_but_conflict_with_own_names() {
    // `mk` は公開の `P` を返す。`mk` だけを `use` しても、`P` は隠れた依存として見える。
    let t = Tmp::new("hidden");
    util(&t);
    let mk = "pub fn mk(n: Int) -> P\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return P { a: n };\n}\n";
    t.write("lib/mk.sigil", &module("lib.mk", "\nuse lib.util.P;\n", mk));
    let user = "pub fn h(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let p = mk(n);\n  return n;\n}\n";
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.mk.mk;\n", user),
    );
    // `lib.mk` の中の `P { .. }` は、取り込んだ型の構築なので拒否される（構築できるのは定義した側だけ）
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::PrivateConstruction, "lib/mk.sigil".into(), 13))
    );
    // 定義した側で作る関数にすれば通る
    let items = format!(
        "{}\npub fn mkp(n: Int) -> P\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return P {{ a: n }};\n}}\n",
        pub_fn("f")
    );
    t.write(
        "lib/util.sigil",
        &module(
            "lib.util",
            "",
            &format!("{items}\npub struct P {{\n  a: Int,\n}}\n"),
        ),
    );
    t.write(
        "app/main.sigil",
        &module(
            "app.main",
            "\nuse lib.util.mkp;\n",
            &user.replace("mk(n)", "mkp(n)"),
        ),
    );
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
    // 隠れた依存の `P` と同じ名前を自分で宣言すると衝突
    let own = format!(
        "pub struct P {{\n  a: Int,\n}}\n\n{}",
        user.replace("mk(n)", "mkp(n)")
    );
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.mkp;\n", &own),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::UseConflict, "app/main.sigil".into(), 5))
    );
}

// ---- 標準名との衝突 ----

#[test]
fn reserved_heads_cannot_be_module_names() {
    let t = Tmp::new("resv");
    for head in ["raw", "console", "fs", "std"] {
        let rel = format!("{head}/x.sigil");
        t.write(&rel, &module(&format!("{head}.x"), "", &pub_fn("f")));
        assert_eq!(
            check(&t, &rel),
            Err((ErrorKind::ReservedModuleName, rel.clone(), 1)),
            "{head}"
        );
    }
    // `use` の側でも
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse fs.x.f;\n", &pub_fn("h")),
    );
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::ReservedModuleName, "app/main.sigil".into(), 5))
    );
}

// ---- 根内の別名 ----

#[cfg(unix)]
#[test]
fn symlink_alias_inside_root_is_rejected() {
    let t = Tmp::new("alias");
    util(&t);
    // 同じ実体 lib/util.sigil に、もう 1 つの名前 lib/util2.sigil
    std::os::unix::fs::symlink(t.0.join("lib/util.sigil"), t.0.join("lib/util2.sigil")).unwrap();
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util2.f;\n", &caller("h", "f")),
    );
    // 別名の側のファイルは、宣言したモジュール名（lib.util）とパス（lib.util2）が合わない
    assert_eq!(
        check(&t, "app/main.sigil"),
        Err((ErrorKind::ModulePathMismatch, "lib/util2.sigil".into(), 1))
    );
    // 実体の側の名前では通る
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.f;\n", &caller("h", "f")),
    );
    assert_eq!(check(&t, "app/main.sigil"), Ok(()));
}

// ---- 差し替え（TOCTOU） ----

#[cfg(unix)]
#[test]
fn swap_to_symlink_outside_root_between_open_and_check() {
    let t = Tmp::new("swap-out");
    let outside = Tmp::new("swap-out-outside");
    util(&t);
    let evil = outside.write("evil.sigil", &module("lib.util", "", &pub_fn("f")));
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.f;\n", &caller("h", "f")),
    );
    let target = t.0.join("lib/util.sigil");
    let mut n = 0;
    let r = check_project_with(
        &t.0,
        &t.0.join("app/main.sigil"),
        &Options::default(),
        &mut |p| {
            // 依存を読んだ直後、確認の前に、パスを根の外への symlink に差し替える
            if p == target {
                n += 1;
                std::fs::remove_file(&target).unwrap();
                std::os::unix::fs::symlink(&evil, &target).unwrap();
            }
        },
    );
    assert_eq!(n, 1);
    assert_eq!(
        to_got(&t, r),
        Err((ErrorKind::ModulePathMismatch, "lib/util.sigil".into(), 1))
    );
}

#[cfg(unix)]
#[test]
fn swap_to_other_file_inside_root_between_open_and_check() {
    // 開いたものと、確認の時点のパスの指す先が別の実体（どちらも根の中）
    let t = Tmp::new("swap-in");
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.f;\n", &caller("h", "f")),
    );
    let target = t.0.join("lib/util.sigil");
    let replacement = module("lib.util", "", &pub_fn("f"));
    let other = t.write(
        "lib/other.sigil",
        &replacement.replace("lib.util", "lib.other"),
    );
    let mut n = 0;
    let r = check_project_with(
        &t.0,
        &t.0.join("app/main.sigil"),
        &Options::default(),
        &mut |p| {
            if p == target {
                n += 1;
                // パスを別の実体に差し替える（同じ根の中）
                std::fs::remove_file(&target).unwrap();
                std::fs::rename(&other, &target).unwrap();
            }
        },
    );
    assert_eq!(n, 1);
    let (k, f, _) = to_got(&t, r).unwrap_err();
    assert_eq!(
        (k, f.as_str()),
        (ErrorKind::ModulePathMismatch, "lib/util.sigil")
    );
}

// ---- CLI ----

fn bin(args: &[&str]) -> (i32, String, String) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(args)
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn cli_reports_dependency_file_and_json() {
    let t = Tmp::new("cli");
    util(&t);
    t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.g;\n", &pub_fn("h")),
    );
    let root = t.0.to_str().unwrap();
    let entry = t.0.join("app/main.sigil");
    let entry = entry.to_str().unwrap();
    let (code, _, err) = bin(&["check", "--root", root, entry]);
    assert_eq!(code, 1);
    assert!(err.contains("main.sigil:5:"), "{err}");
    let (code, out, _) = bin(&["check", "--json", "--root", root, entry]);
    assert_eq!(code, 1);
    assert!(out.contains("\"kind\":\"PrivateImport\""), "{out}");
    assert!(out.contains("main.sigil"), "{out}");
}

/// ADR 0021 2-2c: `--root` を省くと根は cwd。cwd が根でなければ `use` の先は見つからない。
#[test]
fn cli_use_without_root_defaults_to_cwd() {
    let t = Tmp::new("noroot");
    util(&t);
    let p = t.write(
        "app/main.sigil",
        &module("app.main", "\nuse lib.util.f;\n", &caller("h", "f")),
    );
    let run = |cwd: &Path| {
        std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
            .current_dir(cwd)
            .args(["check", p.to_str().unwrap()])
            .output()
            .unwrap()
    };
    // 根が cwd（リポジトリ）なら、ファイルは根の外。
    let o = run(Path::new(env!("CARGO_MANIFEST_DIR")));
    assert_eq!(o.status.code(), Some(1));
    // cwd を根の位置にすれば `use` を辿れる。
    let o = run(&t.0);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[allow(dead_code)]
fn _unused(_: &Path) {}

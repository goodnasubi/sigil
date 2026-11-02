//! ADR 0025 補足 A-6 / A-7 の 3-4h: 埋め込み標準 Sigil モジュールの組み込み。
//!
//! - 経路は暗黙のみ（`Options::embedded_roots`）。利用者の `module std.*` / `use std.*` は従来どおり拒否。
//! - 埋め込みモジュールの `trust base` は許可一覧を経ない。結合後の再検査では、名前が `std.` 始まりの項目だけが
//!   `unchecked`・大域を許される（出所の印）。`std.*` の名前の偽装でこれらを通せない。
//! - 入口でないモジュールの大域は `<モジュール>.<名前>` に改まり、モジュールごとに独立する。
//! - 評価器とネイティブの結果を突き合わせる。本番のレジストリは空なので、試験用の追加欄（`embedded_extra`）を使う。
mod common;

use common::native::{exec, scratch};
use sigil::effects::Options as COpts;
use sigil::error::ErrorKind;
use sigil::exec::{Options, RunError};
use sigil::project::{link_sources, ProjectError};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

fn unchecked_fn(sig: &str, body: &str) -> String {
    format!("{sig}\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n\n")
}

fn base_mod(name: &str, uses: &str, items: &str) -> String {
    format!("module {name}\n  targets server\n  trust base\n{uses}\n{items}")
}

fn user_entry() -> String {
    format!(
        "module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n"
    )
}

/// 同名の大域 `N` を持つ 2 つの埋め込みモジュール。
fn mod_a() -> String {
    base_mod(
        "std.os.a",
        "",
        &format!(
            "global mut N: Int = 10\n\n{}{}",
            unchecked_fn("pub fn bump() -> Int", "  N = N + 1;\n  return N;"),
            unchecked_fn("pub fn get() -> Int", "  return N;")
        ),
    )
}

fn mod_b() -> String {
    base_mod(
        "std.os.b",
        "\nuse std.os.a.bump;\nuse std.os.a.get;\n",
        &format!(
            "global mut N: Int = 100\n\n{}",
            unchecked_fn(
                "pub fn go() -> Int",
                "  N = N + 5;\n  bump();\n  bump();\n  return N * 1000 + get();"
            )
        ),
    )
}

fn copts(roots: &[&str]) -> COpts {
    COpts {
        embedded_extra: vec![
            ("std.os.a".to_string(), mod_a()),
            ("std.os.b".to_string(), mod_b()),
        ],
        embedded_roots: roots.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn eopts(roots: &[&str]) -> Options {
    let c = copts(roots);
    Options {
        embedded_extra: c.embedded_extra,
        embedded_roots: c.embedded_roots,
        ..Options::captured()
    }
}

fn kind(r: Result<sigil::ast::File, ProjectError>) -> ErrorKind {
    match r {
        Err(ProjectError::Diag { error, .. }) => error.kind,
        Err(ProjectError::Read { msg, .. }) => panic!("読めない: {msg}"),
        Ok(_) => panic!("通ってはいけない"),
    }
}

fn read_err(r: Result<sigil::ast::File, ProjectError>) -> String {
    match r {
        Err(ProjectError::Read { msg, .. }) => msg,
        other => panic!("Read エラーのはず: {:?}", other.map(|_| ())),
    }
}

/// 評価器で走らせ、同じプログラムをネイティブでも走らせて、値が一致することを確かめる。
fn both(entry: &str, roots: &[&str], func: &str) -> String {
    let want = sigil::ir_eval::run_sources_with(&[entry], func, &[], &eopts(roots))
        .unwrap_or_else(|f| panic!("評価器が失敗: {:?}", f.error))
        .value;
    let p = sigil::ir_eval::lower_sources_opts(&[entry], func, &copts(roots))
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("embedded34h");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = exec(&exe, &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(
        got.stdout.trim_end().rsplit('\n').next().unwrap(),
        want,
        "評価器とネイティブの値が違う"
    );
    want
}

// ---- 正の試験 ----

#[test]
fn embedded_root_runs_and_globals_are_isolated_per_module() {
    // a.N=10, b.N=100。go: b.N=105, bump 2 回で a.N=12。b.N*1000 + a.get() = 105012。
    // 大域の改名が効かず同じ `N` に潰れると別の値になる。
    assert_eq!(
        both(&user_entry(), &["std.os.a", "std.os.b"], "std.os.b.go"),
        "105012"
    );
}

#[test]
fn embedded_state_is_shared_within_module_across_calls() {
    let m = base_mod(
        "std.os.c",
        "",
        &format!(
            "global mut N: Int = 0\n\n{}{}",
            unchecked_fn("fn bump() -> Int", "  N = N + 1;\n  return N;"),
            unchecked_fn(
                "pub fn run() -> Int",
                "  bump();\n  bump();\n  bump();\n  return N;"
            )
        ),
    );
    let c = COpts {
        embedded_extra: vec![("std.os.c".into(), m)],
        embedded_roots: vec!["std.os.c".into()],
        ..Default::default()
    };
    let o = Options {
        embedded_extra: c.embedded_extra.clone(),
        embedded_roots: c.embedded_roots.clone(),
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(&[&user_entry()], "std.os.c.run", &[], &o).unwrap();
    assert_eq!(r.value, "3");
}

#[test]
fn user_entry_still_runs_with_roots_present() {
    assert_eq!(both(&user_entry(), &["std.os.a"], "main"), "7");
}

#[test]
fn embedded_module_uses_embedded_module_only_via_registry() {
    // b は a を `use` する。a だけを root にしても、b を root にすれば a は b の `use` で入る。
    assert_eq!(both(&user_entry(), &["std.os.b"], "std.os.b.go"), "105012");
}

#[test]
fn no_roots_means_no_extra_embedded_modules() {
    // 確保器 `std.os.alloc` は常に結ぶ（補足 C-2）ので、試験用の `std.os.a` が入らないことを見る。
    let f = link_sources(&[&user_entry()], &copts(&[])).unwrap();
    assert!(f
        .items
        .iter()
        .all(|i| !format!("{i:?}").contains("std.os.a.")));
}

// ---- 否定の試験: 利用者の std.* は従来どおり拒否 ----

#[test]
fn user_module_named_std_is_rejected_even_if_registered_and_listed() {
    let user = base_mod("std.os.a", "", "");
    for roots in [vec![], vec!["std.os.a"]] {
        let mut o = copts(&roots);
        o.trust_base = vec!["std.os.a".into()];
        assert_eq!(
            kind(link_sources(&[&user], &o)),
            ErrorKind::ReservedModuleName,
            "roots={roots:?}"
        );
    }
}

#[test]
fn user_use_of_std_module_is_rejected_even_if_registered() {
    let user = format!(
        "module app.main\n  targets server\n  trust none\n\nuse std.os.a.get;\n\npub fn main() -> Int\n{TAIL}{{\n  return get();\n}}\n"
    );
    for roots in [vec![], vec!["std.os.a"]] {
        assert_eq!(
            kind(link_sources(&[&user], &copts(&roots))),
            ErrorKind::ReservedModuleName,
            "roots={roots:?}"
        );
    }
}

#[test]
fn user_dependency_named_std_cannot_pose_as_embedded() {
    // 利用者が渡したソースで `std.os.z` を宣言し、別の利用者モジュールがそれを `use` する。
    let z = base_mod("std.os.z", "", "");
    let user = format!(
        "module app.main\n  targets server\n  trust none\n\nuse std.os.z.f;\n\npub fn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    assert_eq!(
        kind(link_sources(&[&user, &z], &copts(&[]))),
        ErrorKind::ReservedModuleName
    );
}

#[test]
fn std_name_cannot_smuggle_unchecked_or_global_into_user_module() {
    // 埋め込みの root があっても、利用者のモジュールの unchecked・大域は拒否のまま。
    let unchecked_user = format!(
        "module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}  unchecked \"x\"\n{{\n  return 1;\n}}\n"
    );
    assert_eq!(
        kind(link_sources(&[&unchecked_user], &copts(&["std.os.a"]))),
        ErrorKind::UncheckedOutsideTrustBase
    );
    let global_user = format!(
        "module app.main\n  targets server\n  trust none\n\nglobal mut G: Int = 0\n\npub fn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    assert_eq!(
        kind(link_sources(&[&global_user], &copts(&["std.os.a"]))),
        ErrorKind::GlobalOutsideTrustBase
    );
}

#[test]
fn user_impl_on_embedded_type_path_cannot_use_unchecked() {
    let ty = base_mod(
        "std.os.t",
        "",
        &format!(
            "pub struct T {{\n  v: Int,\n}}\n\n{}",
            unchecked_fn("pub fn mk() -> Int", "  return 1;")
        ),
    );
    let user = format!(
        "module app.main\n  targets server\n  trust none\n\nimpl std.os.t.T {{\n  pub fn f(self: &T) -> Int\n{TAIL}  unchecked \"x\"\n  {{\n    return 1;\n  }}\n}}\n\npub fn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    let mut o = copts(&["std.os.t"]);
    o.embedded_extra.push(("std.os.t".into(), ty));
    let r = link_sources(&[&user], &o);
    assert!(
        r.is_err(),
        "std.* の型パスへの impl で unchecked を通してはいけない"
    );
}

#[test]
fn user_impl_on_user_type_cannot_use_unchecked() {
    let user = format!(
        "module app.main\n  targets server\n  trust none\n\npub struct Acct {{\n  v: Int,\n}}\n\nimpl Acct {{\n  pub fn f(self: &Acct) -> Int\n{TAIL}  unchecked \"x\"\n  {{\n    return 1;\n  }}\n}}\n\npub fn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    let r = link_sources(&[&user], &copts(&["std.os.a"]));
    assert_eq!(
        kind(r),
        ErrorKind::UncheckedOutsideTrustBase,
        "利用者の impl で unchecked を通してはいけない"
    );
}

#[test]
fn user_trait_impl_cannot_use_unchecked() {
    let user = format!(
        "module app.main\n  targets server\n  trust none\n\ntrait Show {{\n  fn size(self: &Self) -> Int\n    intent \"x\"\n    requires none\n    ensures none\n    effects none;\n}}\n\nstruct Acct2 {{\n  v: Int,\n}}\n\nimpl Show for Acct2 {{\n  fn size(self: &Acct2) -> Int\n{TAIL}  unchecked \"x\"\n  {{\n    return 1;\n  }}\n}}\n\npub fn main() -> Int\n{TAIL}{{\n  return 1;\n}}\n"
    );
    let r = link_sources(&[&user], &copts(&["std.os.a"]));
    assert_eq!(
        kind(r),
        ErrorKind::UncheckedOutsideTrustBase,
        "利用者の trait impl で unchecked を通してはいけない"
    );
}

#[test]
fn non_entry_user_trust_base_module_is_still_rejected_by_link() {
    let lib = base_mod(
        "lib.t",
        "",
        &unchecked_fn("pub fn f() -> Int", "  return 1;"),
    );
    let main = format!(
        "module app.main\n  targets server\n  trust none\n\nuse lib.t.f;\n\npub fn main() -> Int\n{TAIL}{{\n  return f();\n}}\n"
    );
    let mut o = copts(&[]);
    o.trust_base = vec!["lib.t".into()];
    assert_eq!(
        kind(link_sources(&[&main, &lib], &o)),
        ErrorKind::Unsupported
    );
}

// ---- 埋め込みモジュール自身の検査 ----

#[test]
fn embedded_non_base_module_cannot_use_unchecked() {
    let m = format!(
        "module std.os.n\n  targets server\n  trust none\n\n{}",
        unchecked_fn("pub fn f() -> Int", "  return 1;")
    );
    let mut o = copts(&["std.os.n"]);
    o.embedded_extra.push(("std.os.n".into(), m));
    assert_eq!(
        kind(link_sources(&[&user_entry()], &o)),
        ErrorKind::UncheckedOutsideTrustBase
    );
}

#[test]
fn registry_name_must_match_declaration() {
    let mut o = copts(&["std.os.m"]);
    o.embedded_extra.push(("std.os.m".into(), mod_a()));
    assert_eq!(
        kind(link_sources(&[&user_entry()], &o)),
        ErrorKind::ModulePathMismatch
    );
}

#[test]
fn unknown_root_and_non_std_registration_are_rejected() {
    assert!(read_err(link_sources(&[&user_entry()], &copts(&["std.os.none"]))).contains("ない"));
    // `std.` で始まらない名前は、登録しても引けない。
    let mut o = copts(&["m.lib"]);
    o.embedded_extra
        .push(("m.lib".into(), base_mod("m.lib", "", "")));
    assert!(read_err(link_sources(&[&user_entry()], &o)).contains("ない"));
}

#[test]
fn embedded_cannot_use_unregistered_std_module() {
    let m = base_mod(
        "std.os.u",
        "\nuse std.os.nothing.f;\n",
        &unchecked_fn("pub fn g() -> Int", "  return 1;"),
    );
    let mut o = copts(&["std.os.u"]);
    o.embedded_extra.push(("std.os.u".into(), m));
    assert_eq!(
        kind(link_sources(&[&user_entry()], &o)),
        ErrorKind::ReservedModuleName
    );
}

#[test]
fn roots_require_entry_targets_to_be_supported() {
    let entry = "module app.main\n  targets browser, server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 7;\n}\n";
    assert_eq!(
        kind(link_sources(&[entry], &copts(&["std.os.a"]))),
        ErrorKind::TargetMismatch
    );
}

#[test]
fn run_error_surface_is_check_for_mem_sources() {
    let user = base_mod("std.os.a", "", "");
    let r = sigil::ir_eval::run_sources_with(&[&user], "f", &[], &eopts(&[]));
    assert!(matches!(r, Err(f) if matches!(f.error, RunError::Check(_))));
}

#[test]
fn embedded_impl_methods_and_properties_pass_the_merged_recheck() {
    // 結合後の再検査は入口（`trust none`）の見出しで行う。名前が `std.` 始まりの型の impl・性質の手付けは通る。
    let m = base_mod(
        "std.os.t",
        "",
        &format!(
            "pub struct Slot2 {{\n  v: Int,\n}}\n\nproperties Slot2: Sendable \"単純な整数\"\n\nimpl Slot2 {{\n  pub fn get(self: &Slot2) -> Int\n{TAIL}  unchecked \"読むだけ\"\n  {{\n    return self.v;\n  }}\n}}\n\npub fn run() -> Int\n{TAIL}{{\n  let c = Slot2 {{ v: 4 }};\n  return c.get();\n}}\n"
        ),
    );
    let c = COpts {
        embedded_extra: vec![("std.os.t".into(), m)],
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    };
    let o = Options {
        embedded_extra: c.embedded_extra,
        embedded_roots: c.embedded_roots,
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(&[&user_entry()], "std.os.t.run", &[], &o)
        .unwrap_or_else(|f| panic!("{:?}", f.error));
    assert_eq!(r.value, "4");
}

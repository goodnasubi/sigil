//! `sigil test --ct`（ADR 0061 決定 3・4 の段階 2）。`Sigil.toml` の `[testing.constant_time]` に
//! 書かれた外部モジュールの関数を、静的検査（R1〜R6）と CLIF 検査にかける。

use std::path::PathBuf;
use std::process::{Command, Output};

const LIB: &str = r#"module ctlib
  targets server
  trust none

pub fn xor_all(secret: Bytes, n: Int) -> Int
  intent "秘密の中身を長さ n だけ畳み込む"
  requires none
  ensures none
  effects none
{
  let mut acc = 0;
  let mut i = 0;
  while i < n {
    acc = acc ^ secret.get(i).unwrap_or(0);
    i = i + 1;
  }
  return acc;
}

pub fn leaky(secret: Bytes) -> Int
  intent "秘密で分岐する"
  requires none
  ensures none
  effects none
{
  if secret.get(0).unwrap_or(0) == 1 {
    return 1;
  }
  return 0;
}

pub fn outer(secret: Bytes, n: Int) -> Int
  intent "別の対象を呼ぶ"
  requires none
  ensures none
  effects none
{
  return xor_all(secret, n);
}

pub fn plain(a: Int) -> Int
  intent "秘密を渡す呼び出し先"
  requires none
  ensures none
  effects none
{
  return a;
}

pub fn calls_plain(secret: Int) -> Int
  intent "対象でない関数へ秘密を渡す"
  requires none
  ensures none
  effects none
{
  return plain(secret);
}
"#;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(tag: &str, manifest: &str) -> Fixture {
        let dir = PathBuf::from(format!("target/ct_cli_{}_{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ctlib.sigil"), LIB).unwrap();
        std::fs::write(dir.join("Sigil.toml"), manifest).unwrap();
        Fixture { dir }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_sigil"))
            .args(args)
            .arg("--root")
            .arg(&self.dir)
            .output()
            .expect("sigil を実行できない")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn manifest(targets: &str) -> String {
    format!("[testing.constant_time]\ntargets = [\n{targets}\n]\n")
}

#[test]
fn clean_target_passes_static_and_clif() {
    let f = Fixture::new(
        "clean",
        &manifest(r#"  { name = "ctlib.xor_all", public_args = ["n"] },"#),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("ctlib.xor_all: ok（CLIF:"), "{t}");
}

#[test]
fn public_args_decides_what_is_secret() {
    // `n` を公開と書かなければ秘密になり、`i < n` が R1 になる。
    let f = Fixture::new(
        "secret_n",
        &manifest(r#"  { name = "ctlib.xor_all", public_args = [] },"#),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("ctlib.xor_all: 失敗") && t.contains("R1(分岐)"),
        "{t}"
    );
}

#[test]
fn omitted_public_args_means_all_secret() {
    let f = Fixture::new("omitted", &manifest(r#"  { name = "ctlib.xor_all" },"#));
    let o = f.run(&["test", "--ct", "--no-clif"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
}

#[test]
fn secret_branch_is_reported_as_r1() {
    let f = Fixture::new(
        "leaky",
        &manifest(r#"  { name = "ctlib.leaky", public_args = [] },"#),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("R1(分岐)") && t.contains("1 件中 1 件が失敗"),
        "{t}"
    );
}

#[test]
fn failures_do_not_hide_other_targets() {
    let f = Fixture::new(
        "mixed",
        &manifest(
            "  { name = \"ctlib.xor_all\", public_args = [\"n\"] },\n  { name = \"ctlib.leaky\", public_args = [] },",
        ),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("ctlib.xor_all: ok") && t.contains("ctlib.leaky: 失敗"),
        "{t}"
    );
}

#[test]
fn missing_function_is_an_error() {
    let f = Fixture::new(
        "nofn",
        &manifest(r#"  { name = "ctlib.nothere", public_args = [] },"#),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("関数 `nothere` がモジュール `ctlib` にない"),
        "{t}"
    );
}

#[test]
fn missing_module_is_an_error() {
    let f = Fixture::new(
        "nomod",
        &manifest(r#"  { name = "nomod.f", public_args = [] },"#),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("モジュール `nomod` のファイル"), "{t}");
}

#[test]
fn unknown_public_arg_is_an_error() {
    let f = Fixture::new(
        "badarg",
        &manifest(r#"  { name = "ctlib.xor_all", public_args = ["len"] },"#),
    );
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("`public_args` の `len` は関数 `ctlib.xor_all` の仮引数にない"),
        "{t}"
    );
}

#[test]
fn calling_a_declared_target_with_secrets_is_allowed() {
    let both = Fixture::new(
        "both",
        &manifest(
            "  { name = \"ctlib.xor_all\", public_args = [\"n\"] },\n  { name = \"ctlib.outer\", public_args = [\"n\"] },",
        ),
    );
    let o = both.run(&["test", "--ct", "--no-clif"]);
    assert!(o.status.success(), "{}", text(&o));

    let only_outer = Fixture::new(
        "only_outer",
        &manifest(r#"  { name = "ctlib.outer", public_args = ["n"] },"#),
    );
    let o = only_outer.run(&["test", "--ct", "--no-clif"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("R4(呼び出し)"), "{t}");
}

#[test]
fn passing_a_secret_to_a_non_target_is_r4() {
    let f = Fixture::new(
        "r4",
        &manifest(r#"  { name = "ctlib.calls_plain", public_args = [] },"#),
    );
    let o = f.run(&["test", "--ct", "--no-clif"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("R4(呼び出し)") && t.contains("plain"), "{t}");
}

#[test]
fn verify_ct_is_an_alias() {
    let f = Fixture::new(
        "alias",
        &manifest(r#"  { name = "ctlib.xor_all", public_args = ["n"] },"#),
    );
    let o = f.run(&["verify", "--ct", "--no-clif"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("ctlib.xor_all: ok（静的検査のみ）"), "{t}");
}

#[test]
fn manifest_without_targets_or_missing_is_an_error() {
    let f = Fixture::new("empty", "[package]\nname = \"x\"\n");
    let o = f.run(&["test", "--ct"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(text(&o).contains("検査する対象がない"), "{}", text(&o));

    std::fs::remove_file(f.dir.join("Sigil.toml")).unwrap();
    let o = f.run(&["test", "--ct"]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
}

#[test]
fn bad_manifest_syntax_reports_the_line() {
    let f = Fixture::new("syntax", "[testing.constant_time]\ntargets = @\n");
    let o = f.run(&["test", "--ct"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("Sigil.toml:2:"), "{t}");
}

#[test]
fn usage_error_without_ct() {
    let f = Fixture::new("usage", "");
    let o = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["test", "--root"])
        .arg(&f.dir)
        .output()
        .unwrap();
    assert_ne!(o.status.code(), Some(0));
}

//! ADR 0018 判断 1 (T2) / PR 3-0: trust-base 許可一覧ファイル（`trust-base.list`）の読み込み。
//!
//! リポジトリまたはプロジェクトの根（`--root`、未指定時はカレントディレクトリ）に存在する
//! `trust-base.list` からモジュール一覧を読み込み、`check`, `run`, `build` の `trust_base` に反映する。
//! CLI オプション `--trust-base` との併用（和集合）もテストする。

use std::fs;
use std::process::Command;

fn sigil_bin_in(root: &str, args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigil"));
    cmd.current_dir(root);
    cmd.args(args);
    let o = cmd.output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn module(name: &str, targets: &str, trust: &str, items: &str) -> String {
    format!("module {name}\n  targets {targets}\n  trust {trust}\n\n{items}")
}

fn ufn(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n  unchecked \"前提\"\n{{\n{body}\n}}\n\n"
    )
}

fn base(items: &str) -> String {
    module("m.a", "server", "base", items)
}

fn staged(tag: &str, src: &str) -> (String, String) {
    let name = src
        .lines()
        .find_map(|l| l.strip_prefix("module "))
        .unwrap()
        .trim()
        .to_string();
    let root = std::env::temp_dir().join(format!(
        "sigil-trust-base-file-{}-{tag}",
        std::process::id()
    ));
    let file = root.join(format!("{}.sigil", name.replace('.', "/")));
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, src).unwrap();
    (
        root.to_string_lossy().into_owned(),
        file.to_string_lossy().into_owned(),
    )
}

fn temp_file(name: &str, src: &str) -> String {
    let dir = std::env::temp_dir().join(format!("sigil-trust-base-file-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    fs::write(&p, src).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn trust_base_list_allows_check_run_and_build() {
    let src = base(&ufn("f(n: Int) -> Int", "none", "  return n + 1;"));
    let (root, p) = staged("all", &src);

    // 最初は trust-base.list がないので check, run, build は失敗する
    let (code, _, err) = sigil_bin_in(&root, &["check", &p]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("trust base"), "{err}");

    let (code, _, _) = sigil_bin_in(&root, &["run", &p, "f", "10"]);
    assert_eq!(code, 1);

    let out_bin = temp_file("f_all.out", "");
    let (code, _, err) = sigil_bin_in(&root, &["build", &p, "f", "-o", &out_bin]);
    assert_eq!(code, 1, "{err}");

    // trust-base.list を配置（コメント行や空行を含める）
    let list_path = std::path::Path::new(&root).join("trust-base.list");
    fs::write(&list_path, "# 信頼基盤リスト\n\n  m.a  \n# other.module\n").unwrap();

    // check が通る
    let (code, out, err) = sigil_bin_in(&root, &["check", &p]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ok"), "{out}");

    // run が通る
    let (code, out, err) = sigil_bin_in(&root, &["run", &p, "f", "10"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "11");

    // build が通る
    let (code, _, err) = sigil_bin_in(&root, &["build", &p, "f", "-o", &out_bin]);
    assert_eq!(code, 0, "{err}");
    let o = Command::new(&out_bin).arg("10").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "11");
}

#[test]
fn trust_base_list_combined_with_cli_option() {
    // 2つの trust base モジュール m.a と m.b を用意
    let src_a = base(&ufn("f(n: Int) -> Int", "none", "  return n + 1;"));
    let src_b = module(
        "m.b",
        "server",
        "base",
        &ufn("g(n: Int) -> Int", "none", "  return n + 2;"),
    );

    let (root, p_a) = staged("union_a", &src_a);
    let p_b = std::path::Path::new(&root).join("m/b.sigil");
    fs::create_dir_all(p_b.parent().unwrap()).unwrap();
    fs::write(&p_b, src_b).unwrap();
    let p_b_str = p_b.to_str().unwrap();

    // trust-base.list には m.a のみ記載
    let list_path = std::path::Path::new(&root).join("trust-base.list");
    fs::write(&list_path, "m.a\n").unwrap();

    // m.a はオプションなしでも list から通る
    let (code, out, err) = sigil_bin_in(&root, &["check", &p_a]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ok"), "{out}");

    // m.b は list にないので単体では失敗する
    let (code, _, err) = sigil_bin_in(&root, &["check", p_b_str]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("trust base"), "{err}");

    // --trust-base m.b を指定すれば、list の m.a と CLI の m.b の両方が許可され、m.b も check が通る
    let (code, out, err) = sigil_bin_in(&root, &["check", "--trust-base", "m.b", p_b_str]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ok"), "{out}");

    // m.a も同時に検査できる
    let (code, _, err) = sigil_bin_in(&root, &["check", "--trust-base", "m.b", &p_a, p_b_str]);
    assert_eq!(code, 0, "{err}");

    // run でも list(m.a) と CLI(m.b) が両方有効であることを確認
    let (code, out, err) = sigil_bin_in(&root, &["run", "--trust-base", "m.b", &p_a, "f", "5"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "6");

    let (code, out, err) = sigil_bin_in(&root, &["run", "--trust-base", "m.b", p_b_str, "g", "5"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "7");
}

#[test]
fn trust_base_list_with_root_option() {
    let src = base(&ufn("f(n: Int) -> Int", "none", "  return n + 1;"));
    let (root, p) = staged("root_opt", &src);

    // root ディレクトリに trust-base.list を置く
    let list_path = std::path::Path::new(&root).join("trust-base.list");
    fs::write(&list_path, "m.a\n").unwrap();

    // 別のディレクトリ（例: temp_dir）をカレントディレクトリとして起動し、--root <root> を渡す
    let other_dir = std::env::temp_dir().to_string_lossy().into_owned();
    let (code, out, err) = sigil_bin_in(&other_dir, &["check", "--root", &root, &p]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ok"), "{out}");

    let (code, out, err) = sigil_bin_in(&other_dir, &["run", "--root", &root, &p, "f", "1"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "2");

    let out_bin = temp_file("f_root.out", "");
    let (code, _, err) = sigil_bin_in(
        &other_dir,
        &["build", "--root", &root, &p, "f", "-o", &out_bin],
    );
    assert_eq!(code, 0, "{err}");
}

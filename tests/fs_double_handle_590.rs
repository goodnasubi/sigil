//! Issue #590 / ADR 0049 決定 5・補足 B: 差し替え（`fs_files`／`--fs-double`）下の fs A のハンドル `open`。
//!
//! `common::run_with` は、評価器の結果を、同じ表を一時ディレクトリの実ファイルに置いたネイティブの実行ファイルの結果と突き合わせる。
//! したがって、ここで `ok` を返す事例は、差し替え下の評価器とネイティブで成功の結果まで一致している。
//! 拒否だけの確認では足りない（Issue #590 の完了条件）。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::collections::BTreeMap;

const HEAD: &str = "module app.fs_double_handle\n  targets server\n  trust none\n\n";

fn f(sig: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"fs double handle test\"\n  requires none\n  ensures none\n  effects fs\n{{\n{body}\n}}\n\n")
}

fn table(entries: &[(&str, &str)]) -> Options {
    let mut files = BTreeMap::new();
    for (k, v) in entries {
        files.insert((*k).to_string(), (*v).to_string());
    }
    Options {
        fs_files: Some(files),
        ..Options::default()
    }
}

fn run(entries: &[(&str, &str)], sig: &str, body: &str, args: &[Arg]) -> String {
    let src = format!("{HEAD}{}", f(sig, body));
    run_with(&src, "t", args, &table(entries))
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

#[test]
fn open_existing_virtual_file_and_lock_match_native() {
    let body = r#"
        let vp = Path.new(&data);
        let ro = fs.open_options().read(true);
        match ro.open(&vp) {
            Err(_) => { return "err_open_read"; }
            Ok(f) => {
                match f.try_lock_shared() {
                    Err(_) => { return "err_lock_shared"; }
                    Ok(b) => { if !b { return "err_shared_not_acquired"; } }
                }
                match f.unlock() {
                    Err(_) => { return "err_unlock"; }
                    Ok(_) => {}
                }
                match f.close() {
                    Err(_) => { return "err_close"; }
                    Ok(_) => {}
                }
            }
        }
        match fs.read(&vp) {
            Err(_) => { return "err_read_after_open"; }
            Ok(s) => { if s != "hello" { return "err_content_changed"; } }
        }
        return "ok";
    "#;
    let res = run(
        &[("/v/data.txt", "hello")],
        "t(data: String) -> String",
        body,
        &[Arg::Str("/v/data.txt".into())],
    );
    assert_eq!(res, "\"ok\"");
}

#[test]
fn truncate_through_handle_is_seen_by_fs_read_and_write_is_seen_by_handle_open() {
    let body = r#"
        let vp = Path.new(&data);
        let w = fs.open_options().write(true).truncate(true);
        match w.open(&vp) {
            Err(_) => { return "err_open_truncate"; }
            Ok(f) => { let _ = f.close(); }
        }
        match fs.read(&vp) {
            Err(_) => { return "err_read_after_truncate"; }
            Ok(s) => { if s != "" { return "err_truncate_not_visible"; } }
        }
        match fs.write(&vp, &"abc") {
            Err(_) => { return "err_write"; }
            Ok(_) => {}
        }
        match fs.read(&vp) {
            Err(_) => { return "err_read_after_write"; }
            Ok(s) => { if s != "abc" { return "err_write_not_visible"; } }
        }
        let ro = fs.open_options().read(true);
        match ro.open(&vp) {
            Err(_) => { return "err_reopen"; }
            Ok(f) => { let _ = f.close(); }
        }
        return "ok";
    "#;
    let res = run(
        &[("/v/data.txt", "hello")],
        "t(data: String) -> String",
        body,
        &[Arg::Str("/v/data.txt".into())],
    );
    assert_eq!(res, "\"ok\"");
}

#[test]
fn create_new_and_missing_parent_match_native() {
    let body = r#"
        let existing = Path.new(&data);
        let fresh = Path.new(&fresh_path);
        let cn = fs.open_options().create_new(true).write(true);

        match cn.open(&existing) {
            Ok(_) => { return "err_create_new_over_existing"; }
            Err(e) => {
                match e {
                    IoError.AlreadyExists(_) => {}
                    _ => { return "err_not_already_exists"; }
                }
            }
        }
        match cn.open(&fresh) {
            Err(_) => { return "err_create_new_fresh"; }
            Ok(f) => { let _ = f.close(); }
        }
        match fs.read(&fresh) {
            Err(_) => { return "err_read_created"; }
            Ok(s) => { if s != "" { return "err_created_not_empty"; } }
        }
        match cn.open(&fresh) {
            Ok(_) => { return "err_create_new_twice"; }
            Err(e) => {
                match e {
                    IoError.AlreadyExists(_) => {}
                    _ => { return "err_second_not_already_exists"; }
                }
            }
        }
        return "ok";
    "#;
    let res = run(
        &[("/v/data.txt", "hello")],
        "t(data: String, fresh_path: String) -> String",
        body,
        &[
            Arg::Str("/v/data.txt".into()),
            Arg::Str("/v/fresh.txt".into()),
        ],
    );
    assert_eq!(res, "\"ok\"");
}

#[test]
fn exclusive_locks_conflict_between_handles_match_native() {
    let body = r#"
        let vp = Path.new(&data);
        let rw = fs.open_options().read(true).write(true);
        match rw.open(&vp) {
            Err(_) => { return "err_open_f1"; }
            Ok(f1) => {
                match f1.try_lock_exclusive() {
                    Err(_) => { return "err_lock_f1"; }
                    Ok(b1) => { if !b1 { return "err_f1_not_acquired"; } }
                }
                match rw.open(&vp) {
                    Err(_) => { return "err_open_f2"; }
                    Ok(f2) => {
                        match f2.try_lock_exclusive() {
                            Err(_) => { return "err_lock_f2"; }
                            Ok(b2) => { if b2 { return "err_f2_unexpectedly_acquired"; } }
                        }
                        match f1.unlock() {
                            Err(_) => { return "err_unlock_f1"; }
                            Ok(_) => {}
                        }
                        match f2.try_lock_exclusive() {
                            Err(_) => { return "err_lock_f2_retry"; }
                            Ok(b3) => { if !b3 { return "err_f2_retry_failed"; } }
                        }
                        let _ = f1.close();
                        let _ = f2.close();
                        return "ok";
                    }
                }
            }
        }
    "#;
    let res = run(
        &[("/v/lock.txt", "x")],
        "t(data: String) -> String",
        body,
        &[Arg::Str("/v/lock.txt".into())],
    );
    assert_eq!(res, "\"ok\"");
}

#[test]
fn remove_file_after_handle_open_is_a_real_unlink() {
    let body = r#"
        let vp = Path.new(&data);
        let ro = fs.open_options().read(true);
        match ro.open(&vp) {
            Err(_) => { return "err_open"; }
            Ok(f) => { let _ = f.close(); }
        }
        match fs.remove_file(&vp) {
            Err(_) => { return "err_remove"; }
            Ok(_) => {}
        }
        match fs.read(&vp) {
            Ok(_) => { return "err_read_after_remove"; }
            Err(e) => {
                match e {
                    IoError.NotFound(_) => {}
                    _ => { return "err_read_not_not_found"; }
                }
            }
        }
        match ro.open(&vp) {
            Ok(_) => { return "err_open_after_remove"; }
            Err(e) => {
                match e {
                    IoError.NotFound(_) => {}
                    _ => { return "err_open_not_not_found"; }
                }
            }
        }
        return "ok";
    "#;
    let res = run(
        &[("/v/data.txt", "hello")],
        "t(data: String) -> String",
        body,
        &[Arg::Str("/v/data.txt".into())],
    );
    assert_eq!(res, "\"ok\"");
}

/// 親の仮ディレクトリが無い仮のパスへの作成は NotFound。評価器だけの確認（ネイティブの試験の枠組みは、表にない絶対パスの親を実際に作ってしまう）。
#[test]
fn create_without_virtual_parent_is_not_found_under_fs_double() {
    let body = r#"
        let orphan = Path.new(&"/nowhere/fresh.txt");
        let cn = fs.open_options().create_new(true).write(true);
        match cn.open(&orphan) {
            Ok(_) => { return "err_create_without_parent"; }
            Err(e) => {
                match e {
                    IoError.NotFound(_) => {}
                    _ => { return "err_not_not_found"; }
                }
            }
        }
        return "ok";
    "#;
    let src = format!("{HEAD}{}", f("t(x: String) -> String", body));
    let res = sigil::ir_eval::run_with(
        &src,
        "t",
        &[Arg::Str("x".into())],
        &table(&[("/v/data.txt", "x")]),
    )
    .unwrap()
    .value;
    assert_eq!(res, "\"ok\"");
}

/// 差し替えの外へ出うるパス（`..`）は、解決せずに拒否する（fail-closed）。評価器だけの確認（ネイティブは実ファイルシステムの規則で `..` を解く）。
#[test]
fn parent_escape_is_rejected_under_fs_double() {
    let body = r#"
        let esc = Path.new(&"/v/../etc/passwd");
        let ro = fs.open_options().read(true);
        match ro.open(&esc) {
            Ok(_) => { return "err_escape_opened"; }
            Err(_) => {}
        }
        return "ok";
    "#;
    let src = format!("{HEAD}{}", f("t(x: String) -> String", body));
    let res = sigil::ir_eval::run_with(
        &src,
        "t",
        &[Arg::Str("x".into())],
        &table(&[("/v/data.txt", "x")]),
    )
    .unwrap()
    .value;
    assert_eq!(res, "\"ok\"");
}

/// `--fs-double 仮のパス=実ファイル` の右辺の実ファイルは変更せず（隔離）、影の一時ディレクトリは実行の終了で消える。
#[test]
fn cli_fs_double_handle_open_never_touches_the_original_and_cleans_up() {
    let base = std::env::temp_dir().join(format!("sigil-fsd590-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let tmp = base.join("tmp");
    let root = base.join("root");
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::create_dir_all(root.join("app")).unwrap();
    let original = base.join("original.txt");
    std::fs::write(&original, "orig").unwrap();
    let body = r#"
        let vp = Path.new(&data);
        let w = fs.open_options().write(true).truncate(true);
        match w.open(&vp) {
            Err(_) => { return "err_open_truncate"; }
            Ok(f) => { let _ = f.close(); }
        }
        match fs.read(&vp) {
            Err(_) => { return "err_read"; }
            Ok(s) => { if s != "" { return "err_not_truncated"; } }
        }
        return "ok";
    "#;
    let src = format!(
        "module app.t\n  targets server\n  trust none\n\n{}",
        f("t(data: String) -> String", body)
    );
    let file = root.join("app").join("t.sigil");
    std::fs::write(&file, src).unwrap();
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .env("TMPDIR", &tmp)
        .args(["run", "--root"])
        .arg(&root)
        .arg("--fs-double")
        .arg(format!("/v/data.txt={}", original.display()))
        .arg(&file)
        .args(["t", "/v/data.txt"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "\"ok\"\n");
    assert_eq!(std::fs::read_to_string(&original).unwrap(), "orig");
    assert_eq!(
        std::fs::read_dir(&tmp).unwrap().count(),
        0,
        "影の一時ディレクトリが残っている"
    );
    let _ = std::fs::remove_dir_all(&base);
}

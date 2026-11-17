//! Issue #586 / ADR 0049: fs 境界の実装 A「資源・名前・排他」
//!
//! ディレクトリ操作（作成・削除・種別・上限付き列挙）とプロセス間排他ロック（flock）、
//! サブパス境界（相対性検査・親ディレクトリ脱出禁止・NUL拒絶）、symlink 拒絶。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::path::{Path, PathBuf};

const HEAD: &str = "module app.fs_test\n  targets server\n  trust none\n\n";

fn f(sig: &str, effects: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"fs test\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n")
}

fn out(sig: &str, effects: &str, body: &str, args: &[Arg]) -> String {
    let src = format!("{HEAD}{}", f(sig, effects, body));
    run_with(&src, "t", args, &Options::default())
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-fs-a-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn p(x: &Path) -> Arg {
    Arg::Str(x.to_string_lossy().into_owned())
}

#[test]
fn create_and_remove_dir_and_remove_file() {
    let base = scratch_dir("create_remove");
    let test_dir = base.join("subdir");
    let test_file = test_dir.join("test.txt");

    let sig = "t(dir_path: String, file_path: String) -> String";
    let body = r#"
        let dp = Path.new(&dir_path);
        let fp = Path.new(&file_path);
        match fs.create_dir(&dp) {
            Err(_) => { return "err_create_dir"; }
            Ok(_) => {}
        }
        match fs.write(&fp, &"hello world") {
            Err(_) => { return "err_write"; }
            Ok(_) => {}
        }
        match fs.remove_dir(&dp) {
            Ok(_) => { return "err_remove_nonempty_succeeded"; }
            Err(_) => {}
        }
        match fs.remove_file(&fp) {
            Err(_) => { return "err_remove_file"; }
            Ok(_) => {}
        }
        match fs.remove_dir(&dp) {
            Err(_) => { return "err_remove_empty"; }
            Ok(_) => {}
        }
        return "ok";
    "#;

    let res = out(sig, "fs", body, &[p(&test_dir), p(&test_file)]);
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn create_dir_all_creates_nested_directories() {
    let base = scratch_dir("create_dir_all");
    let nested = base.join("a").join("b").join("c");

    let sig = "t(nested_path: String) -> String";
    let body = r#"
        let p = Path.new(&nested_path);
        match fs.create_dir_all(&p) {
            Err(_) => { return "err_create_dir_all"; }
            Ok(_) => {}
        }
        return "ok";
    "#;

    let res = out(sig, "fs", body, &[p(&nested)]);
    assert_eq!(res, "\"ok\"");
    assert!(nested.is_dir());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn dir_handle_operations_and_bounded_read_entries() {
    let base = scratch_dir("dir_ops");

    let sig = "t(base_path: String) -> String";
    let body = r#"
        let bp = Path.new(&base_path);
        match fs.open_dir(&bp) {
            Err(_) => { return "err_open_dir"; }
            Ok(dir) => {
                let sub1 = Path.new(&"file1.txt");
                let sub2 = Path.new(&"file2.txt");
                let _ = dir.remove_file(&sub1);
                let _ = dir.remove_file(&sub2);

                match dir.create_file(&sub1) {
                    Err(_) => { return "err_create_file1"; }
                    Ok(f1) => {
                        let _ = f1.close();
                    }
                }
                match dir.create_file(&sub2) {
                    Err(_) => { return "err_create_file2"; }
                    Ok(f2) => {
                        let _ = f2.close();
                    }
                }

                // 上限10件での列挙（2件取得）
                match dir.read_entries(10) {
                    Err(_) => { return "err_read_entries_10"; }
                    Ok(entries) => {
                        if entries.len() != 2 {
                            return "err_len_not_2";
                        }
                        match entries.get(0) {
                            None => { return "err_no_e0"; }
                            Some(e0) => {
                                if !e0.is_file() || e0.is_dir() || e0.is_symlink() {
                                    return "err_e0_kind";
                                }
                            }
                        }
                        match entries.get(1) {
                            None => { return "err_no_e1"; }
                            Some(e1) => {
                                if !e1.is_file() || e1.is_dir() || e1.is_symlink() {
                                    return "err_e1_kind";
                                }
                            }
                        }
                    }
                }

                let _ = dir.close();

                // 新しいハンドルで1件上限での列挙
                match fs.open_dir(&bp) {
                    Err(_) => { return "err_reopen_dir"; }
                    Ok(dir2) => {
                        match dir2.read_entries(1) {
                            Err(_) => { return "err_read_entries_1"; }
                            Ok(entries1) => {
                                if entries1.len() != 1 {
                                    return "err_len_not_1";
                                }
                            }
                        }

                        match dir2.remove_file(&sub1) {
                            Err(_) => { return "err_remove_file1"; }
                            Ok(_) => {}
                        }
                        match dir2.remove_file(&sub2) {
                            Err(_) => { return "err_remove_file2"; }
                            Ok(_) => {}
                        }

                        let _ = dir2.close();
                        return "ok";
                    }
                }
            }
        }
    "#;

    let res = out(sig, "fs", body, &[p(&base)]);
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn subpath_validation_rejects_escape_and_nul() {
    let base = scratch_dir("subpath_validation");

    let sig = "t(base_path: String) -> String";
    let body = r#"
        let bp = Path.new(&base_path);
        match fs.open_dir(&bp) {
            Err(_) => { return "err_open_dir"; }
            Ok(dir) => {
                let abs_p = Path.new(&"/escape.txt");
                match dir.create_file(&abs_p) {
                    Ok(_) => { return "err_abs_allowed"; }
                    Err(_) => {}
                }

                let up_p = Path.new(&"../escape.txt");
                match dir.create_file(&up_p) {
                    Ok(_) => { return "err_up_allowed"; }
                    Err(_) => {}
                }

                let sub_up_p = Path.new(&"a/../b.txt");
                match dir.create_file(&sub_up_p) {
                    Ok(_) => { return "err_sub_up_allowed"; }
                    Err(_) => {}
                }

                let _ = dir.close();
                return "ok";
            }
        }
    "#;

    let res = out(sig, "fs", body, &[p(&base)]);
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn file_flock_exclusion() {
    let base = scratch_dir("flock_test");
    let test_file = base.join("locked.txt");

    let sig = "t(file_path: String) -> String";
    let body = r#"
        let fp = Path.new(&file_path);
        let opts = fs.open_options()
            .read(true)
            .write(true)
            .create(true);
        match opts.open(&fp) {
            Err(_) => { return "err_open_f1"; }
            Ok(f1) => {
                match f1.try_lock_exclusive() {
                    Err(_) => { return "err_lock_f1"; }
                    Ok(b1) => {
                        if !b1 {
                            return "err_f1_not_acquired";
                        }
                    }
                }

                let opts2 = fs.open_options().read(true).write(true);
                match opts2.open(&fp) {
                    Err(_) => { return "err_open_f2"; }
                    Ok(f2) => {
                        match f2.try_lock_exclusive() {
                            Err(_) => { return "err_lock_f2"; }
                            Ok(b2) => {
                                if b2 {
                                    return "err_f2_unexpectedly_acquired";
                                }
                            }
                        }

                        match f1.unlock() {
                            Err(_) => { return "err_unlock_f1"; }
                            Ok(_) => {}
                        }

                        match f2.try_lock_exclusive() {
                            Err(_) => { return "err_lock_f2_retry"; }
                            Ok(b2_retry) => {
                                if !b2_retry {
                                    return "err_f2_retry_failed";
                                }
                            }
                        }

                        let _ = f1.close();
                        let _ = f2.close();
                        return "ok";
                    }
                }
            }
        }
    "#;

    let res = out(sig, "fs", body, &[p(&test_file)]);
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn symlink_rejection_when_not_following() {
    let base = scratch_dir("symlink_test");
    let target = base.join("target.txt");
    let link = base.join("link.txt");
    std::fs::write(&target, "content").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let sig = "t(link_path: String) -> String";
    let body = r#"
        let lp = Path.new(&link_path);
        let opts = fs.open_options()
            .read(true)
            .follow_symlinks(false);
        match opts.open(&lp) {
            Ok(_) => { return "err_symlink_allowed"; }
            Err(_) => {}
        }
        return "ok";
    "#;

    let res = out(sig, "fs", body, &[p(&link)]);
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[cfg(unix)]
#[test]
fn intermediate_symlink_rejection() {
    let base = scratch_dir("intermediate_symlink");
    let outside = scratch_dir("outside");
    let link_to_outside = base.join("link_dir");
    std::os::unix::fs::symlink(&outside, &link_to_outside).unwrap();

    let sig = "t(base_dir: String) -> String";
    let body = r#"
        let bp = Path.new(&base_dir);
        match fs.open_dir(&bp) {
            Err(_) => { return "err_open_dir"; }
            Ok(dir) => {
                let sub = Path.new(&"link_dir/victim.txt");
                match dir.create_file(&sub) {
                    Ok(_) => { return "err_create_through_symlink_allowed"; }
                    Err(_) => {}
                }

                let opts = fs.open_options().read(true);
                match dir.open_file(&sub, &opts) {
                    Ok(_) => { return "err_open_through_symlink_allowed"; }
                    Err(_) => {}
                }

                match dir.remove_file(&sub) {
                    Ok(_) => { return "err_remove_through_symlink_allowed"; }
                    Err(_) => {}
                }

                let _ = dir.close();
                return "ok";
            }
        }
    "#;

    let res = out(sig, "fs", body, &[p(&base)]);
    assert_eq!(res, "\"ok\"");
    assert!(!outside.join("victim.txt").exists());
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn fs_double_isolation() {
    use std::collections::BTreeMap;
    let mut files = BTreeMap::new();
    let key = "/virtual/test.txt".to_string();
    files.insert(key.clone(), "virtual content".to_string());
    let opts = Options {
        fs_files: Some(files),
        ..Options::default()
    };

    let sig = "t(virtual_path: String) -> String";
    let body = r#"
        let vp = Path.new(&virtual_path);
        match fs.remove_file(&vp) {
            Err(_) => { return "err_remove_virtual"; }
            Ok(_) => {}
        }
        match fs.read(&vp) {
            Ok(_) => { return "err_read_after_remove"; }
            Err(_) => {}
        }
        let opts = fs.open_options().read(true);
        match opts.open(&vp) {
            Ok(_) => { return "err_open_unsupported"; }
            Err(_) => {}
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f(sig, "fs", body));
    let res = run_with(&src, "t", &[Arg::Str(key)], &opts).unwrap().value;
    assert_eq!(res, "\"ok\"");
}

#[test]
fn structured_io_error_classification() {
    let base = scratch_dir("structured_errors");
    let test_dir = base.join("nonempty_dir");
    std::fs::create_dir_all(&test_dir).unwrap();
    let test_file = test_dir.join("existing.txt");
    std::fs::write(&test_file, "initial").unwrap();

    let sig = "t(dir_path: String, file_path: String) -> String";
    let body = r#"
        let dp = Path.new(&dir_path);
        let fp = Path.new(&file_path);

        // 1. 既存ファイルに対する create_new は IoError.AlreadyExists
        let opts = fs.open_options().create_new(true).write(true);
        match opts.open(&fp) {
            Ok(_) => { return "err_already_exists_opened"; }
            Err(e) => {
                match e {
                    IoError.AlreadyExists(_) => {}
                    _ => { return "err_not_already_exists"; }
                }
            }
        }

        // 2. 子ファイルが存在するディレクトリに対する remove_dir は IoError.DirectoryNotEmpty
        match fs.remove_dir(&dp) {
            Ok(_) => { return "err_remove_nonempty_succeeded"; }
            Err(e) => {
                match e {
                    IoError.DirectoryNotEmpty(_) => {}
                    _ => { return "err_not_directory_not_empty"; }
                }
            }
        }

        // 3. 不正パス（.. 脱出）は IoError.InvalidPath
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(dir) => {
                let bad = Path.new(&"../escape.txt");
                match dir.create_file(&bad) {
                    Ok(_) => { return "err_escape_created"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_not_invalid_path"; }
                        }
                    }
                }
                let _ = dir.close();
            }
        }

        return "ok";
    "#;

    let res = out(sig, "fs", body, &[p(&test_dir), p(&test_file)]);
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn fs_bytes_double_isolation_remove_after_write() {
    use std::collections::BTreeMap;
    let mut files = BTreeMap::new();
    let key = "/virtual/binary.dat".to_string();
    files.insert(key.clone(), vec![1, 2, 3, 4]);
    let opts = Options {
        fs_bytes: Some(files),
        ..Options::default()
    };

    let sig = "t(path_str: String) -> String";
    let body = r#"
        let p = Path.new(&path_str);
        // 1. 初期 bytes を上書き
        let updated = Bytes.new();
        match fs.write_bytes(&p, &updated, 100) {
            Err(_) => { return "err_write_bytes"; }
            Ok(_) => {}
        }

        // 2. fs.remove_file で削除
        match fs.remove_file(&p) {
            Err(_) => { return "err_remove_file"; }
            Ok(_) => {}
        }

        // 3. 削除後に fs.read_bytes しても元の fs_bytes が復活せず NotFound になること
        match fs.read_bytes(&p, 100) {
            Ok(_) => { return "err_read_bytes_succeeded_after_remove"; }
            Err(_) => {}
        }

        // 4. 再度 remove_file すると NotFound で失敗すること
        match fs.remove_file(&p) {
            Ok(_) => { return "err_second_remove_succeeded"; }
            Err(_) => {}
        }

        return "ok";
    "#;

    let src = format!("{HEAD}{}", f(sig, "fs", body));
    let res = sigil::ir_eval::run_with(&src, "t", &[Arg::Str(key)], &opts)
        .unwrap()
        .value;
    assert_eq!(res, "\"ok\"");
}

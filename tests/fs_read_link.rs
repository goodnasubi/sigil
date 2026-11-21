//! Issue #632 / ADR 0049: ディレクトリ起点で symlink のリンク文字列を上限付きで読む API（`Dir::read_link`）。
//!
//! - リンク先をたどらず、最終 symlink 自体のリンク文字列を `Bytes` として返す（壊れたリンク先でも読める）。
//! - 上限（`max_len`）ちょうどは成功し、`max_len` を 1 バイトでも超えれば `IoError.LimitExceeded`。
//! - `max_len < 0` は `IoError.InvalidInput`、`max_len > 4096`（`MAX_PATH_LEN` 超）は `IoError.LimitExceeded`。
//! - 通常ファイルやディレクトリなど非 symlink は `IoError.InvalidInput`（`EINVAL`）、未存在は `IoError.NotFound`。
//! - 中間構成要素の symlink は追従せず `IoError.PermissionDenied`（`ELOOP`）で拒否。
//! - 空パス、絶対パス、`..` 脱出、末尾 `/` や `.`、NUL バイトは `IoError.InvalidPath` で拒否。
//! - 差し替え（`--fs-double`）下でもネイティブと SIR 評価器で挙動が一致する。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::collections::BTreeMap;
use std::path::PathBuf;

const HEAD: &str = "module app.fs_read_link_test\n  targets server\n  trust none\n\n";

fn f(sig: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"fs read_link test\"\n  requires none\n  ensures none\n  effects fs\n{{\n{body}\n}}\n\n")
}

fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-fs-readlink-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 通常の symlink、ネストしたディレクトリ内の symlink、壊れた（リンク先が存在しない）symlink、
/// および非 UTF-8 バイト列を含む symlink のリンク文字列をそのままバイト列として読めること。
#[cfg(unix)]
#[test]
fn read_link_returns_raw_target_bytes_without_following() {
    use std::os::unix::ffi::OsStrExt;

    let base = scratch_dir("basic");
    let sub = base.join("nested").join("dir");
    std::fs::create_dir_all(&sub).unwrap();

    // 1. 通常の相対リンク
    std::os::unix::fs::symlink("target/file.txt", base.join("rel_link")).unwrap();
    // 2. ネストしたディレクトリ内のリンク（リンク先は存在しない壊れたリンク + 外部を指す文字列でも追従しない）
    std::os::unix::fs::symlink("../../outside/nonexistent", sub.join("dangling_link")).unwrap();
    // 3. 非 UTF-8 バイト列のリンク先
    let raw_target = std::ffi::OsStr::from_bytes(&[0x61, 0xff, 0xfe, 0x62]);
    std::os::unix::fs::symlink(raw_target, base.join("raw_link")).unwrap();

    let body = r#"
        let dp = Path.new(&base_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                // 1. rel_link
                let p1 = Path.new(&"rel_link");
                match d.read_link(&p1, 4096) {
                    Err(_) => { return "err_read_rel"; }
                    Ok(b) => {
                        match b.to_string_utf8() {
                            Err(_) => { return "err_utf8_rel"; }
                            Ok(s) => {
                                if s != "target/file.txt" {
                                    return "err_val_rel";
                                }
                            }
                        }
                    }
                }

                // 2. nested/dir/dangling_link（リンク先が存在しなくても・.. を含んでいてもリンク自体の文字列を読む）
                let p2 = Path.new(&"nested/dir/dangling_link");
                match d.read_link(&p2, 4096) {
                    Err(_) => { return "err_read_dangling"; }
                    Ok(b) => {
                        match b.to_string_utf8() {
                            Err(_) => { return "err_utf8_dangling"; }
                            Ok(s) => {
                                if s != "../../outside/nonexistent" {
                                    return "err_val_dangling";
                                }
                            }
                        }
                    }
                }

                // 3. raw_link（非 UTF-8 バイト列も Bytes としてそのまま取得できる）
                let p3 = Path.new(&"raw_link");
                match d.read_link(&p3, 16) {
                    Err(_) => { return "err_read_raw"; }
                    Ok(b) => {
                        if b.byte_len() != 4 { return "err_raw_len"; }
                        if b.get(0).unwrap_or(0) != 97 { return "err_raw_0"; }
                        if b.get(1).unwrap_or(0) != 255 { return "err_raw_1"; }
                        if b.get(2).unwrap_or(0) != 254 { return "err_raw_2"; }
                        if b.get(3).unwrap_or(0) != 98 { return "err_raw_3"; }
                    }
                }

                let _ = d.close();
            }
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f("t(base_path: String) -> String", body));
    let res = run_with(
        &src,
        "t",
        &[Arg::Str(base.to_string_lossy().into_owned())],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

/// 上限（max_len）の境界検査:
/// - リンク文字列の長さ == max_len は成功
/// - リンク文字列の長さ > max_len は LimitExceeded
/// - max_len == 0 は空でない symlink に対して LimitExceeded
/// - max_len < 0 は InvalidInput
/// - max_len > 4096 (MAX_PATH_LEN) は LimitExceeded
#[cfg(unix)]
#[test]
fn read_link_enforces_max_len_boundaries() {
    let base = scratch_dir("limits");
    // 10 バイトのリンク先
    std::os::unix::fs::symlink("0123456789", base.join("ten_bytes")).unwrap();

    let body = r#"
        let dp = Path.new(&base_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                let p = Path.new(&"ten_bytes");

                // ちょうど 10 バイト -> Ok
                match d.read_link(&p, 10) {
                    Err(_) => { return "err_exact_10_failed"; }
                    Ok(b) => {
                        if b.byte_len() != 10 { return "err_exact_10_len"; }
                    }
                }

                // 9 バイト上限 -> LimitExceeded
                match d.read_link(&p, 9) {
                    Ok(_) => { return "err_limit_9_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.LimitExceeded(_) => {}
                            _ => { return "err_limit_9_kind"; }
                        }
                    }
                }

                // 0 バイト上限 -> LimitExceeded
                match d.read_link(&p, 0) {
                    Ok(_) => { return "err_limit_0_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.LimitExceeded(_) => {}
                            _ => { return "err_limit_0_kind"; }
                        }
                    }
                }

                // 負の上限 -> InvalidInput
                match d.read_link(&p, -1) {
                    Ok(_) => { return "err_neg_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidInput(_) => {}
                            _ => { return "err_neg_kind"; }
                        }
                    }
                }

                // MAX_PATH_LEN (4096) ちょうど -> Ok
                match d.read_link(&p, 4096) {
                    Err(_) => { return "err_4096_failed"; }
                    Ok(b) => {
                        if b.byte_len() != 10 { return "err_4096_len"; }
                    }
                }

                // MAX_PATH_LEN 超 (4097) -> LimitExceeded
                match d.read_link(&p, 4097) {
                    Ok(_) => { return "err_4097_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.LimitExceeded(_) => {}
                            _ => { return "err_4097_kind"; }
                        }
                    }
                }

                let _ = d.close();
            }
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f("t(base_path: String) -> String", body));
    let res = run_with(
        &src,
        "t",
        &[Arg::Str(base.to_string_lossy().into_owned())],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

/// 中間 symlink・作業木外脱出（`..`・絶対パス・不正パス）・非 symlink（通常ファイル・ディレクトリ・未存在）の拒否。
#[cfg(unix)]
#[test]
fn read_link_rejects_intermediate_symlink_escape_and_non_symlink() {
    let base = scratch_dir("rejections");
    let outside = scratch_dir("outside_target");

    // 作業木外の symlink
    std::os::unix::fs::symlink("secret_target", outside.join("outside_link")).unwrap();
    // 作業木内から作業木外ディレクトリへの symlink（中間 symlink）
    std::os::unix::fs::symlink(&outside, base.join("escape_dir")).unwrap();

    // 作業木内の通常ディレクトリ・通常ファイル・正当な symlink
    let real_sub = base.join("real_sub");
    std::fs::create_dir_all(&real_sub).unwrap();
    std::fs::write(base.join("regular.txt"), "hello").unwrap();
    std::os::unix::fs::symlink("regular.txt", base.join("valid_link")).unwrap();

    let body = r#"
        let dp = Path.new(&base_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                // 1. 中間構成要素が symlink -> PermissionDenied (ELOOP)
                let p_inter = Path.new(&"escape_dir/outside_link");
                match d.read_link(&p_inter, 4096) {
                    Ok(_) => { return "err_inter_symlink_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.PermissionDenied(_) => {}
                            _ => { return "err_inter_symlink_kind"; }
                        }
                    }
                }

                // 2. .. による親脱出 -> InvalidPath
                let p_dotdot = Path.new(&"../outside_link");
                match d.read_link(&p_dotdot, 4096) {
                    Ok(_) => { return "err_dotdot_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_dotdot_kind"; }
                        }
                    }
                }

                let p_sub_dotdot = Path.new(&"real_sub/../../outside_link");
                match d.read_link(&p_sub_dotdot, 4096) {
                    Ok(_) => { return "err_sub_dotdot_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_sub_dotdot_kind"; }
                        }
                    }
                }

                // 3. 絶対パス・空パス・末尾スラッシュ・カレントディレクトリ(.) -> InvalidPath
                let p_abs = Path.new(&"/etc/passwd");
                match d.read_link(&p_abs, 4096) {
                    Ok(_) => { return "err_abs_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_abs_kind"; }
                        }
                    }
                }

                let p_empty = Path.new(&"");
                match d.read_link(&p_empty, 4096) {
                    Ok(_) => { return "err_empty_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_empty_kind"; }
                        }
                    }
                }

                let p_trail = Path.new(&"valid_link/");
                match d.read_link(&p_trail, 4096) {
                    Ok(_) => { return "err_trail_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_trail_kind"; }
                        }
                    }
                }

                let p_dot = Path.new(&".");
                match d.read_link(&p_dot, 4096) {
                    Ok(_) => { return "err_dot_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_dot_kind"; }
                        }
                    }
                }

                // 4. 通常ファイル -> InvalidInput (EINVAL)
                let p_reg = Path.new(&"regular.txt");
                match d.read_link(&p_reg, 4096) {
                    Ok(_) => { return "err_reg_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidInput(_) => {}
                            _ => { return "err_reg_kind"; }
                        }
                    }
                }

                // 5. 通常ディレクトリ -> InvalidInput (EINVAL)
                let p_dir = Path.new(&"real_sub");
                match d.read_link(&p_dir, 4096) {
                    Ok(_) => { return "err_dir_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidInput(_) => {}
                            _ => { return "err_dir_kind"; }
                        }
                    }
                }

                // 6. 存在しないエントリ -> NotFound (ENOENT)
                let p_missing = Path.new(&"no_such_link");
                match d.read_link(&p_missing, 4096) {
                    Ok(_) => { return "err_missing_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.NotFound(_) => {}
                            _ => { return "err_missing_kind"; }
                        }
                    }
                }

                let _ = d.close();
            }
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f("t(base_path: String) -> String", body));
    let res = run_with(
        &src,
        "t",
        &[Arg::Str(base.to_string_lossy().into_owned())],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&outside);
}

/// 差し替え（`--fs-double`）下での `Dir::read_link` の振る舞いがネイティブと SIR 評価器で一致すること。
#[test]
fn fs_double_read_link_matches_native() {
    let body = r#"
        let dp = Path.new(&dir_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                // 1. 通常ファイルへの read_link -> InvalidInput
                let p_file = Path.new(&"data.txt");
                match d.read_link(&p_file, 256) {
                    Ok(_) => { return "err_file_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidInput(_) => {}
                            _ => { return "err_file_kind"; }
                        }
                    }
                }

                // 2. 未存在パスへの read_link -> NotFound
                let p_missing = Path.new(&"missing_link");
                match d.read_link(&p_missing, 256) {
                    Ok(_) => { return "err_missing_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.NotFound(_) => {}
                            _ => { return "err_missing_kind"; }
                        }
                    }
                }

                // 3. .. 脱出 -> InvalidPath
                let p_escape = Path.new(&"../escape");
                match d.read_link(&p_escape, 256) {
                    Ok(_) => { return "err_escape_succeeded"; }
                    Err(e) => {
                        match e {
                            IoError.InvalidPath(_) => {}
                            _ => { return "err_escape_kind"; }
                        }
                    }
                }

                let _ = d.close();
            }
        }
        return "ok";
    "#;

    let mut files = BTreeMap::new();
    files.insert("/v/data.txt".to_string(), "hello virtual".to_string());
    let opts = Options {
        fs_files: Some(files),
        ..Options::default()
    };
    let src = format!("{HEAD}{}", f("t(dir_path: String) -> String", body));
    let res = run_with(&src, "t", &[Arg::Str("/v".into())], &opts)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value;
    assert_eq!(res, "\"ok\"");
}

//! Issue #712 / ADR 0049 決定 8: `File::byte_len`（st_size）と `File::mtime`（st_mtime の秒）。
//!
//! 空・小・大きいファイルのサイズ、書き込み後の増加、固定した更新時刻、
//! ディレクトリの拒否（IsADirectory）、`Dir::open_file` 経由のハンドル、symlink 拒否、
//! 差し替え（`--fs-double`）下の振る舞い。
//! `common::run_with` は評価器の結果をネイティブの実行ファイルと突き合わせる。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

const HEAD: &str = "module app.fs_e_test\n  targets server\n  trust none\n\n";

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn f(sig: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"fs e test\"\n  requires none\n  ensures none\n  effects fs\n{{\n{body}\n}}\n\n")
}

fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-fs-e-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(src: &str, args: &[Arg]) -> String {
    run_with(src, "t", args, &Options::default())
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

fn set_mtime(path: &std::path::Path, secs: u64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

/// 空・小・大きい（複数ページ）ファイルのサイズが std と一致する。
#[test]
fn byte_len_matches_size() {
    let _serial = serial();
    let base = scratch_dir("size");
    let empty = base.join("empty");
    let small = base.join("small");
    let big = base.join("big");
    std::fs::write(&empty, "").unwrap();
    std::fs::write(&small, "hello").unwrap();
    std::fs::write(&big, vec![7u8; 1_000_003]).unwrap();

    let body = r#"
        let ro = fs.open_options().read(true);
        let p1 = Path.new(&empty_path);
        match ro.open(&p1) {
            Err(_) => { return "err_open_empty"; }
            Ok(f) => {
                match f.byte_len() {
                    Err(_) => { return "err_len_empty"; }
                    Ok(n) => { if n != 0 { return "bad_empty"; } }
                }
                let _ = f.close();
            }
        }
        let p2 = Path.new(&small_path);
        match ro.open(&p2) {
            Err(_) => { return "err_open_small"; }
            Ok(f) => {
                match f.byte_len() {
                    Err(_) => { return "err_len_small"; }
                    Ok(n) => { if n != 5 { return "bad_small"; } }
                }
                let _ = f.close();
            }
        }
        let p3 = Path.new(&big_path);
        match ro.open(&p3) {
            Err(_) => { return "err_open_big"; }
            Ok(f) => {
                match f.byte_len() {
                    Err(_) => { return "err_len_big"; }
                    Ok(n) => { if n != 1000003 { return "bad_big"; } }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;
    let src = format!(
        "{HEAD}{}",
        f(
            "t(empty_path: String, small_path: String, big_path: String) -> String",
            body
        )
    );
    let res = run(
        &src,
        &[
            Arg::Str(empty.to_string_lossy().into_owned()),
            Arg::Str(small.to_string_lossy().into_owned()),
            Arg::Str(big.to_string_lossy().into_owned()),
        ],
    );
    assert_eq!(res, "\"ok\"");
}

/// 同じハンドルで末尾へ書き込むと byte_len が書き込んだ分だけ増える。
/// 評価器とネイティブが同じファイルに続けて実行されるので、初期サイズに依らない相対検査にする。
#[test]
fn byte_len_tracks_writes_on_same_handle() {
    let _serial = serial();
    let base = scratch_dir("grow");
    let file = base.join("grow.txt");
    std::fs::write(&file, "abc").unwrap();

    let body = r#"
        let p = Path.new(&file_path);
        let rw = fs.open_options().read(true).write(true);
        match rw.open(&p) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                let mut before = 0;
                match f.byte_len() {
                    Err(_) => { return "err_len1"; }
                    Ok(n) => { before = n; }
                }
                let data = "defgh".as_bytes();
                match f.write_all_at(&data, before) {
                    Err(_) => { return "err_write"; }
                    Ok(_) => {}
                }
                match f.byte_len() {
                    Err(_) => { return "err_len2"; }
                    Ok(n) => { if n != before + 5 { return "bad_len2"; } }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;
    let src = format!("{HEAD}{}", f("t(file_path: String) -> String", body));
    let res = run(&src, &[Arg::Str(file.to_string_lossy().into_owned())]);
    assert_eq!(res, "\"ok\"");
    // 評価器とネイティブの 2 回分が末尾に追記されている。
    assert_eq!(std::fs::read(&file).unwrap(), b"abcdefghdefgh");
}

/// mtime は固定した更新時刻（Unix 秒）を返す。Dir::open_file 経由のハンドルでも同じ。
#[test]
fn mtime_returns_unix_seconds() {
    let _serial = serial();
    let base = scratch_dir("mtime");
    let file = base.join("m.txt");
    std::fs::write(&file, "x").unwrap();
    set_mtime(&file, 1_700_000_123);
    let old = base.join("old.txt");
    std::fs::write(&old, "x").unwrap();
    set_mtime(&old, 86_400); // 1970-01-02。0 に近い値

    let body = r#"
        let ro = fs.open_options().read(true);
        let p = Path.new(&file_path);
        match ro.open(&p) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                match f.mtime() {
                    Err(_) => { return "err_mtime"; }
                    Ok(t) => { if t != 1700000123 { return "bad_mtime"; } }
                }
                let _ = f.close();
            }
        }
        let dp = Path.new(&dir_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                let sub_name = "old.txt";
                let sub = Path.new(&sub_name);
                match d.open_file(&sub, &ro) {
                    Err(_) => { return "err_dir_open"; }
                    Ok(f) => {
                        match f.mtime() {
                            Err(_) => { return "err_dir_mtime"; }
                            Ok(t) => { if t != 86400 { return "bad_dir_mtime"; } }
                        }
                        let _ = f.close();
                    }
                }
                let _ = d.close();
            }
        }
        return "ok";
    "#;
    let src = format!(
        "{HEAD}{}",
        f("t(file_path: String, dir_path: String) -> String", body)
    );
    let res = run(
        &src,
        &[
            Arg::Str(file.to_string_lossy().into_owned()),
            Arg::Str(base.to_string_lossy().into_owned()),
        ],
    );
    assert_eq!(res, "\"ok\"");
}

/// ディレクトリを File として開いた場合は IsADirectory（byte_len・mtime の両方）。
#[test]
fn reject_directory() {
    let _serial = serial();
    let base = scratch_dir("dir");
    let sub_dir = base.join("subdir");
    std::fs::create_dir(&sub_dir).unwrap();

    let body = r#"
        let sdp = Path.new(&sub_path);
        let ro = fs.open_options().read(true);
        match ro.open(&sdp) {
            Err(_) => { return "err_open_dir_as_file"; }
            Ok(f) => {
                match f.byte_len() {
                    Ok(_) => { return "err_len_succeeded"; }
                    Err(e) => { match e { IoError.IsADirectory(_) => {} _ => { return "err_len_kind"; } } }
                }
                match f.mtime() {
                    Ok(_) => { return "err_mtime_succeeded"; }
                    Err(e) => { match e { IoError.IsADirectory(_) => {} _ => { return "err_mtime_kind"; } } }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;
    let src = format!("{HEAD}{}", f("t(sub_path: String) -> String", body));
    let res = run(&src, &[Arg::Str(sub_dir.to_string_lossy().into_owned())]);
    assert_eq!(res, "\"ok\"");
}

/// 通常ファイルでないもの（FIFO）は InvalidInput。
#[test]
fn reject_non_regular() {
    let _serial = serial();
    let base = scratch_dir("fifo");
    let fifo = base.join("pipe");
    let c = std::ffi::CString::new(fifo.to_string_lossy().as_bytes()).unwrap();
    extern "C" {
        fn mkfifo(path: *const std::os::raw::c_char, mode: u32) -> i32;
    }
    assert_eq!(unsafe { mkfifo(c.as_ptr(), 0o600) }, 0);

    let body = r#"
        let p = Path.new(&fifo_path);
        let rw = fs.open_options().read(true).write(true);
        match rw.open(&p) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                match f.byte_len() {
                    Ok(_) => { return "err_len_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_len_kind"; } } }
                }
                match f.mtime() {
                    Ok(_) => { return "err_mtime_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_mtime_kind"; } } }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;
    let src = format!("{HEAD}{}", f("t(fifo_path: String) -> String", body));
    let res = run(&src, &[Arg::Str(fifo.to_string_lossy().into_owned())]);
    assert_eq!(res, "\"ok\"");
}

/// symlink は Dir::open_file が拒否するので、リンク先のサイズ・時刻は読めない。
#[test]
fn symlink_is_not_followed() {
    let _serial = serial();
    let base = scratch_dir("symlink");
    std::fs::write(base.join("target.txt"), "secret").unwrap();
    std::os::unix::fs::symlink("target.txt", base.join("link.txt")).unwrap();

    let body = r#"
        let dp = Path.new(&dir_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                let sub_name = "link.txt";
                let sub = Path.new(&sub_name);
                let ro = fs.open_options().read(true).follow_symlinks(false);
                match d.open_file(&sub, &ro) {
                    Ok(_) => { return "err_symlink_opened"; }
                    Err(_) => {}
                }
                let _ = d.close();
            }
        }
        return "ok";
    "#;
    let src = format!("{HEAD}{}", f("t(dir_path: String) -> String", body));
    let res = run(&src, &[Arg::Str(base.to_string_lossy().into_owned())]);
    assert_eq!(res, "\"ok\"");
}

/// 差し替え（--fs-double）下でのサイズ・時刻の問い合わせがネイティブと SIR 評価器で一致すること。
#[test]
fn fs_double_metadata_matches_native() {
    let _serial = serial();
    let body = r#"
        let vp = Path.new(&data);
        let ro = fs.open_options().read(true);
        match ro.open(&vp) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                match f.byte_len() {
                    Err(_) => { return "err_len"; }
                    Ok(n) => { if n < 0 { return "err_negative"; } }
                }
                match f.mtime() {
                    Err(_) => { return "err_mtime"; }
                    Ok(t) => { if t <= 0 { return "err_mtime_value"; } }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;
    let mut files = std::collections::BTreeMap::new();
    files.insert("/v/data.txt".to_string(), "hello virtual".to_string());
    let opts = Options {
        fs_files: Some(files),
        ..Options::default()
    };
    let src = format!("{HEAD}{}", f("t(data: String) -> String", body));
    let res = run_with(&src, "t", &[Arg::Str("/v/data.txt".into())], &opts)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value;
    assert_eq!(res, "\"ok\"");
}

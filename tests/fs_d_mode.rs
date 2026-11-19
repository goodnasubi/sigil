//! Issue #625 / ADR 0049 決定 7: fs 境界の実装 D「既存ファイルのモードの取得と設定」。
//!
//! `File::mode`／`File::set_mode`、`Dir::file_mode`／`Dir::set_file_mode`、
//! 権限ビット（0o000〜0o777）の往復、実行ビットの設定と解除、umask 影響の有無、
//! ディレクトリ・範囲外モード・symlink・`..` の拒否、読み取り専用 FS の PermissionDenied、
//! 差し替え（`--fs-double`）下の振る舞い。
//! `common::run_with` は評価器の結果をネイティブの実行ファイルと突き合わせる。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const HEAD: &str = "module app.fs_d_test\n  targets server\n  trust none\n\n";

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn f(sig: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"fs d test\"\n  requires none\n  ensures none\n  effects fs\n{{\n{body}\n}}\n\n")
}

fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-fs-d-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

extern "C" {
    fn umask(mask: u32) -> u32;
}

struct UmaskGuard(u32);
impl Drop for UmaskGuard {
    fn drop(&mut self) {
        unsafe {
            umask(self.0);
        }
    }
}

fn set_umask(mask: u32) -> UmaskGuard {
    let old = unsafe { umask(mask) };
    UmaskGuard(old)
}

/// 0o644 と 0o755 の往復、実行ビットの設定と解除（File および Dir 起点）。
#[test]
fn mode_roundtrip_and_exec_bits() {
    let _serial = serial();
    let base = scratch_dir("roundtrip");
    let file = base.join("test.txt");
    std::fs::write(&file, "hello").unwrap();

    let body = r#"
        let fp = Path.new(&file_path);
        let rw = fs.open_options().read(true).write(true);
        match rw.open(&fp) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                // 1. 0o755 に設定して 0o755 が返る（実行ビット付与）
                match f.set_mode(493) { // 493 = 0o755
                    Err(_) => { return "err_set_755"; }
                    Ok(_) => {}
                }
                match f.mode() {
                    Err(_) => { return "err_mode_755"; }
                    Ok(m) => { if m != 493 { return "err_val_755"; } }
                }

                // 2. 0o644 に設定して 0o644 が返る（実行ビット解除・往復保証）
                match f.set_mode(420) { // 420 = 0o644
                    Err(_) => { return "err_set_644"; }
                    Ok(_) => {}
                }
                match f.mode() {
                    Err(_) => { return "err_mode_644"; }
                    Ok(m) => { if m != 420 { return "err_val_644"; } }
                }

                // 3. その他のビット（0o600 / 0o700 / 0o000）
                match f.set_mode(384) { // 384 = 0o600
                    Err(_) => { return "err_set_600"; }
                    Ok(_) => {}
                }
                match f.mode() {
                    Err(_) => { return "err_mode_600"; }
                    Ok(m) => { if m != 384 { return "err_val_600"; } }
                }

                let _ = f.close();
            }
        }

        // 4. Dir 起点の file_mode / set_file_mode
        let dp = Path.new(&dir_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                let sub_name = "test.txt";
                let sub = Path.new(&sub_name);
                match d.set_file_mode(&sub, 493) { // 0o755
                    Err(_) => { return "err_dir_set_755"; }
                    Ok(_) => {}
                }
                match d.file_mode(&sub) {
                    Err(_) => { return "err_dir_mode_755"; }
                    Ok(m) => { if m != 493 { return "err_dir_val_755"; } }
                }

                match d.set_file_mode(&sub, 420) { // 0o644
                    Err(_) => { return "err_dir_set_644"; }
                    Ok(_) => {}
                }
                match d.file_mode(&sub) {
                    Err(_) => { return "err_dir_mode_644"; }
                    Ok(m) => { if m != 420 { return "err_dir_val_644"; } }
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
    let res = run_with(
        &src,
        "t",
        &[
            Arg::Str(file.to_string_lossy().into_owned()),
            Arg::Str(base.to_string_lossy().into_owned()),
        ],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
}

/// umask 0o077 下でも set_mode(0o755) が 0o755 になること（ADR 0049 §7.3）。
#[test]
fn set_mode_ignores_umask() {
    let _serial = serial();
    let base = scratch_dir("umask");
    let file = base.join("umask_test.txt");
    std::fs::write(&file, "data").unwrap();
    let _guard = set_umask(0o077);

    let body = r#"
        let fp = Path.new(&file_path);
        let ro = fs.open_options().read(true);
        match ro.open(&fp) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                // umask 0o077 下でも set_mode(0o755) は正確に 0o755 になる
                match f.set_mode(493) { // 493 = 0o755
                    Err(_) => { return "err_set_mode"; }
                    Ok(_) => {}
                }
                match f.mode() {
                    Err(_) => { return "err_mode"; }
                    Ok(m) => { if m != 493 { return "err_val_not_755"; } }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f("t(file_path: String) -> String", body));
    let res = run_with(
        &src,
        "t",
        &[Arg::Str(file.to_string_lossy().into_owned())],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
}

/// 範囲外モード（負値、0o777 超、setuid/setgid/sticky ビット）の拒否（InvalidInput）。
#[test]
fn reject_out_of_range_modes() {
    let _serial = serial();
    let base = scratch_dir("range");
    let file = base.join("range.txt");
    std::fs::write(&file, "content").unwrap();

    let body = r#"
        let fp = Path.new(&file_path);
        let ro = fs.open_options().read(true);
        match ro.open(&fp) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                // 初期モード設定
                match f.set_mode(420) { Err(_) => { return "err_init"; } Ok(_) => {} } // 0o644

                // 負数 -> InvalidInput
                match f.set_mode(-1) {
                    Ok(_) => { return "err_neg_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_neg_kind"; } } }
                }

                // 512 (0o1000, 0o777超) -> InvalidInput
                match f.set_mode(512) {
                    Ok(_) => { return "err_512_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_512_kind"; } } }
                }

                // setuid (0o4755 = 2541) -> InvalidInput
                match f.set_mode(2541) {
                    Ok(_) => { return "err_setuid_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_setuid_kind"; } } }
                }

                // setgid (0o2755 = 1517) -> InvalidInput
                match f.set_mode(1517) {
                    Ok(_) => { return "err_setgid_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_setgid_kind"; } } }
                }

                // sticky (0o1755 = 1005) -> InvalidInput
                match f.set_mode(1005) {
                    Ok(_) => { return "err_sticky_succeeded"; }
                    Err(e) => { match e { IoError.InvalidInput(_) => {} _ => { return "err_sticky_kind"; } } }
                }

                // 失敗後にモードが変更されていないことを確認 (依然 0o644 = 420)
                match f.mode() {
                    Err(_) => { return "err_mode"; }
                    Ok(m) => { if m != 420 { return "err_mode_modified"; } }
                }

                let _ = f.close();
            }
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f("t(file_path: String) -> String", body));
    let res = run_with(
        &src,
        "t",
        &[Arg::Str(file.to_string_lossy().into_owned())],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
}

/// ディレクトリに対する mode / set_mode の拒否（IsADirectory）。
#[test]
fn reject_directory() {
    let _serial = serial();
    let base = scratch_dir("dir_mode");
    let sub_dir = base.join("subdir");
    std::fs::create_dir(&sub_dir).unwrap();

    let body = r#"
        let dp = Path.new(&base_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                let sub_name = "subdir";
                let sub = Path.new(&sub_name);
                // Dir::file_mode でディレクトリを指定 -> IsADirectory
                match d.file_mode(&sub) {
                    Ok(_) => { return "err_dir_mode_succeeded"; }
                    Err(e) => { match e { IoError.IsADirectory(_) => {} _ => { return "err_kind_file_mode"; } } }
                }

                // Dir::set_file_mode でディレクトリを指定 -> IsADirectory
                match d.set_file_mode(&sub, 493) { // 0o755
                    Ok(_) => { return "err_dir_set_mode_succeeded"; }
                    Err(e) => { match e { IoError.IsADirectory(_) => {} _ => { return "err_kind_set_file_mode"; } } }
                }

                let _ = d.close();
            }
        }

        // File ハンドルとしてディレクトリを開いた場合も IsADirectory
        let sdp = Path.new(&sub_path);
        let ro = fs.open_options().read(true);
        match ro.open(&sdp) {
            Err(_) => { return "err_open_dir_as_file"; }
            Ok(f) => {
                match f.mode() {
                    Ok(_) => { return "err_f_mode_succeeded"; }
                    Err(e) => { match e { IoError.IsADirectory(_) => {} _ => { return "err_f_mode_kind"; } } }
                }
                match f.set_mode(493) {
                    Ok(_) => { return "err_f_set_mode_succeeded"; }
                    Err(e) => { match e { IoError.IsADirectory(_) => {} _ => { return "err_f_set_mode_kind"; } } }
                }
                let _ = f.close();
            }
        }

        return "ok";
    "#;

    let src = format!(
        "{HEAD}{}",
        f("t(base_path: String, sub_path: String) -> String", body)
    );
    let res = run_with(
        &src,
        "t",
        &[
            Arg::Str(base.to_string_lossy().into_owned()),
            Arg::Str(sub_dir.to_string_lossy().into_owned()),
        ],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"))
    .value;
    assert_eq!(res, "\"ok\"");
}

/// symlink および .. 脱出の拒否（PermissionDenied / InvalidPath）。
#[test]
fn reject_symlinks_and_parent_traversal() {
    let _serial = serial();
    let base = scratch_dir("symlink");
    let target = base.join("target.txt");
    std::fs::write(&target, "data").unwrap();
    let link = base.join("link.txt");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let body = r#"
        let dp = Path.new(&base_path);
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                let sym_name = "link.txt";
                let sym = Path.new(&sym_name);
                // symlink の file_mode -> PermissionDenied (ELOOP)
                match d.file_mode(&sym) {
                    Ok(_) => { return "err_sym_mode_succeeded"; }
                    Err(e) => { match e { IoError.PermissionDenied(_) => {} _ => { return "err_sym_mode_kind"; } } }
                }

                // symlink の set_file_mode -> PermissionDenied (ELOOP)
                match d.set_file_mode(&sym, 493) {
                    Ok(_) => { return "err_sym_set_succeeded"; }
                    Err(e) => { match e { IoError.PermissionDenied(_) => {} _ => { return "err_sym_set_kind"; } } }
                }

                // .. の file_mode -> InvalidPath
                let dotdot_name = "../target.txt";
                let dotdot = Path.new(&dotdot_name);
                match d.file_mode(&dotdot) {
                    Ok(_) => { return "err_dotdot_mode_succeeded"; }
                    Err(e) => { match e { IoError.InvalidPath(_) => {} _ => { return "err_dotdot_mode_kind"; } } }
                }

                // .. の set_file_mode -> InvalidPath
                match d.set_file_mode(&dotdot, 493) {
                    Ok(_) => { return "err_dotdot_set_succeeded"; }
                    Err(e) => { match e { IoError.InvalidPath(_) => {} _ => { return "err_dotdot_set_kind"; } } }
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
}

/// 読み取り専用ファイルシステム上のファイルに対する set_mode は PermissionDenied（EROFS）。
#[test]
fn read_only_fs_permission_denied() {
    let _serial = serial();
    // /etc/hosts は本 VM 環境で squashfs ro にマウントされている
    if !Path::new("/etc/hosts").exists() {
        return;
    }

    let body = r#"
        let path_str = "/etc/hosts";
        let p = Path.new(&path_str);
        let ro = fs.open_options().read(true);
        match ro.open(&p) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                // read-only fs に対する set_mode は EROFS (30) -> PermissionDenied
                match f.set_mode(493) { // 0o755
                    Ok(_) => { return "err_set_mode_succeeded_on_ro"; }
                    Err(e) => {
                        match e {
                            IoError.PermissionDenied(_) => {}
                            _ => { return "err_not_permission_denied"; }
                        }
                    }
                }
                let _ = f.close();
            }
        }
        return "ok";
    "#;

    let src = format!("{HEAD}{}", f("t() -> String", body));
    let res = run_with(&src, "t", &[], &Options::default())
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value;
    assert_eq!(res, "\"ok\"");
}

/// 差し替え（--fs-double）下での mode / set_mode がネイティブと SIR 評価器で一致すること。
#[test]
fn fs_double_mode_matches_native() {
    let _serial = serial();
    let body = r#"
        let vp = Path.new(&data);
        let rw = fs.open_options().read(true).write(true);
        match rw.open(&vp) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                // 初期モード取得
                match f.mode() {
                    Err(_) => { return "err_mode_init"; }
                    Ok(_) => {}
                }

                // 0o755 設定
                match f.set_mode(493) { // 0o755
                    Err(_) => { return "err_set_755"; }
                    Ok(_) => {}
                }
                match f.mode() {
                    Err(_) => { return "err_mode_755"; }
                    Ok(m) => { if m != 493 { return "err_val_755"; } }
                }

                // 0o644 設定
                match f.set_mode(420) { // 0o644
                    Err(_) => { return "err_set_644"; }
                    Ok(_) => {}
                }
                match f.mode() {
                    Err(_) => { return "err_mode_644"; }
                    Ok(m) => { if m != 420 { return "err_val_644"; } }
                }

                let _ = f.close();
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
    let src = format!("{HEAD}{}", f("t(data: String) -> String", body));
    let res = run_with(&src, "t", &[Arg::Str("/v/data.txt".into())], &opts)
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value;
    assert_eq!(res, "\"ok\"");
}

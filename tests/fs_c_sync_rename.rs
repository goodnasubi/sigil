//! Issue #593 / ADR 0049 決定 4.2・4.3: fs 境界の実装 C「同期・原子的置換」。
//!
//! `File::sync_all`／`sync_data`、`Dir::sync`、`fs.rename`（同一 FS の原子的置換、異 FS は `NotSameFileSystem`）、
//! 差し替え（`--fs-double`）下の振る舞い。`common::run_with` は評価器の結果をネイティブの実行ファイルと突き合わせる。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::path::{Path, PathBuf};

const HEAD: &str = "module app.fs_c_test\n  targets server\n  trust none\n\n";

/// 評価器は同じプロセスの中でシステムコールを発行する。`File::close`／`Dir::close` の後に同じ fd が破棄フックでもう一度 close される
/// 既存の不具合（fs A。本 Issue の範囲外。PR 本文を参照）があり、並行する他の試験の fd を閉じうる。そこで、この試験ファイルの試験は直列に実行する。
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn f(sig: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"fs c test\"\n  requires none\n  ensures none\n  effects fs\n{{\n{body}\n}}\n\n")
}

fn out(sig: &str, body: &str, args: &[Arg]) -> String {
    let src = format!("{HEAD}{}", f(sig, body));
    run_with(&src, "t", args, &Options::default())
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

fn scratch_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-fs-c-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn p(x: &Path) -> Arg {
    Arg::Str(x.to_string_lossy().into_owned())
}

#[test]
fn sync_all_sync_data_and_dir_sync_succeed_after_update() {
    let _serial = serial();
    let base = scratch_dir("sync");
    let file = base.join("data.txt");
    let body = r#"
        let fp = Path.new(&file_path);
        let dp = Path.new(&dir_path);
        match fs.write(&fp, &"hello") {
            Err(_) => { return "err_write"; }
            Ok(_) => {}
        }
        let rw = fs.open_options().write(true);
        match rw.open(&fp) {
            Err(_) => { return "err_open"; }
            Ok(f) => {
                match f.sync_all() {
                    Err(_) => { return "err_sync_all"; }
                    Ok(_) => {}
                }
                match f.sync_data() {
                    Err(_) => { return "err_sync_data"; }
                    Ok(_) => {}
                }
                match f.close() {
                    Err(_) => { return "err_close"; }
                    Ok(_) => {}
                }
            }
        }
        // 新規作成のエントリは、親ディレクトリの sync が成功して初めて持続性が確定する。
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                match d.sync() {
                    Err(_) => { return "err_dir_sync"; }
                    Ok(_) => {}
                }
                let _ = d.close();
            }
        }
        match fs.read(&fp) {
            Err(_) => { return "err_read"; }
            Ok(s) => { if s != "hello" { return "err_content"; } }
        }
        return "ok";
    "#;
    let res = out(
        "t(file_path: String, dir_path: String) -> String",
        body,
        &[p(&file), p(&base)],
    );
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn rename_replaces_an_existing_file_and_reports_errors() {
    let _serial = serial();
    let base = scratch_dir("rename");
    let from = base.join("from.txt");
    let to = base.join("to.txt");
    let missing = base.join("missing.txt");
    let no_parent = base.join("no_such_dir").join("x.txt");
    let a_dir = base.join("a_dir");
    std::fs::create_dir_all(&a_dir).unwrap();
    let body = r#"
        let pf = Path.new(&from_path);
        let pt = Path.new(&to_path);
        let pm = Path.new(&missing_path);
        let pn = Path.new(&no_parent_path);
        let pd = Path.new(&dir_path);
        let _ = fs.write(&pf, &"new");
        let _ = fs.write(&pt, &"old");
        // 既存の to は from の内容で置き換わり、from は無くなる。
        match fs.rename(&pf, &pt) {
            Err(_) => { return "err_rename_replace"; }
            Ok(_) => {}
        }
        match fs.read(&pt) {
            Err(_) => { return "err_read_to"; }
            Ok(s) => { if s != "new" { return "err_to_not_replaced"; } }
        }
        match fs.read(&pf) {
            Ok(_) => { return "err_from_remains"; }
            Err(e) => {
                match e {
                    IoError.NotFound(_) => {}
                    _ => { return "err_from_not_not_found"; }
                }
            }
        }
        // 存在しない from は NotFound。
        match fs.rename(&pm, &pt) {
            Ok(_) => { return "err_rename_missing"; }
            Err(e) => {
                match e {
                    IoError.NotFound(_) => {}
                    _ => { return "err_missing_not_not_found"; }
                }
            }
        }
        match fs.read(&pt) {
            Err(_) => { return "err_to_lost_after_failed_rename"; }
            Ok(s) => { if s != "new" { return "err_to_changed_by_failed_rename"; } }
        }
        // 親が無い to は NotFound。
        match fs.rename(&pt, &pn) {
            Ok(_) => { return "err_rename_no_parent"; }
            Err(e) => {
                match e {
                    IoError.NotFound(_) => {}
                    _ => { return "err_no_parent_not_not_found"; }
                }
            }
        }
        // ファイルをディレクトリへは置き換えられない。
        match fs.rename(&pt, &pd) {
            Ok(_) => { return "err_rename_over_dir"; }
            Err(e) => {
                match e {
                    IoError.IsADirectory(_) => {}
                    _ => { return "err_over_dir_not_is_a_directory"; }
                }
            }
        }
        return "ok";
    "#;
    let res = out(
        "t(from_path: String, to_path: String, missing_path: String, no_parent_path: String, dir_path: String) -> String",
        body,
        &[p(&from), p(&to), p(&missing), p(&no_parent), p(&a_dir)],
    );
    assert_eq!(res, "\"ok\"");
    let _ = std::fs::remove_dir_all(&base);
}

/// 別のファイルシステムへの rename は、コピーして消す代替をせず、NotSameFileSystem で即座に失敗する。
#[cfg(unix)]
#[test]
fn rename_across_file_systems_fails_with_not_same_file_system() {
    let _serial = serial();
    use std::os::unix::fs::MetadataExt;
    let shm = Path::new("/dev/shm");
    let tmp = std::env::temp_dir();
    let same = match (std::fs::metadata(shm), std::fs::metadata(&tmp)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => true,
    };
    if same {
        eprintln!("飛ばす: /dev/shm と一時ディレクトリが同じファイルシステム（異 FS の rename を作れない環境）");
        return;
    }
    let src = shm.join(format!("sigil-fs-c-xdev-{}.txt", std::process::id()));
    std::fs::write(&src, "payload").unwrap();
    let dst_dir = scratch_dir("xdev");
    let dst = dst_dir.join("dst.txt");
    let body = r#"
        let ps = Path.new(&src_path);
        let pd = Path.new(&dst_path);
        match fs.rename(&ps, &pd) {
            Ok(_) => { return "err_cross_fs_rename_succeeded"; }
            Err(e) => {
                match e {
                    IoError.NotSameFileSystem(_) => {}
                    _ => { return "err_not_not_same_file_system"; }
                }
            }
        }
        // 暗黙のコピーはしない: 移動先は作られず、移動元は残る。
        match fs.read(&pd) {
            Ok(_) => { return "err_destination_created"; }
            Err(_) => {}
        }
        match fs.read(&ps) {
            Err(_) => { return "err_source_lost"; }
            Ok(s) => { if s != "payload" { return "err_source_changed"; } }
        }
        return "ok";
    "#;
    let res = out(
        "t(src_path: String, dst_path: String) -> String",
        body,
        &[p(&src), p(&dst)],
    );
    assert_eq!(res, "\"ok\"");
    assert!(!dst.exists());
    assert_eq!(std::fs::read_to_string(&src).unwrap(), "payload");
    let _ = std::fs::remove_file(&src);
    let _ = std::fs::remove_dir_all(&dst_dir);
}

/// 置換の瞬間に to が存在しない空白期間が無い: 別プロセスが to を読み続け、rename を繰り返す間、一度も読めない／空／途中の内容を見ない。
/// 読み手は別プロセスにする（評価器と同じプロセスだと、fd の表を共有して干渉しうるため）。
#[test]
fn replacing_rename_never_exposes_a_gap_to_a_concurrent_reader() {
    let _serial = serial();
    let base = scratch_dir("gap");
    let target = base.join("target.txt");
    let stop_file = base.join("stop");
    std::fs::write(&target, "AAAA").unwrap();
    let script = r#"t="$1"; stop="$2"; n=0
while [ ! -e "$stop" ]; do
  c=$(cat "$t" 2>&1) || { echo "FAIL:$c"; exit 1; }
  case "$c" in AAAA|BBBB) ;; *) echo "BAD:$c"; exit 1;; esac
  n=$((n+1))
done
echo "OK:$n""#;
    let reader = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(&target)
        .arg(&stop_file)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let body = r#"
        let pt = Path.new(&to_path);
        let pa = Path.new(&tmp_a);
        let pb = Path.new(&tmp_b);
        let mut i = 0;
        while i < 200 {
            match fs.write(&pa, &"AAAA") { Err(_) => { return "err_write_a"; } Ok(_) => {} }
            match fs.rename(&pa, &pt) { Err(_) => { return "err_rename_a"; } Ok(_) => {} }
            match fs.write(&pb, &"BBBB") { Err(_) => { return "err_write_b"; } Ok(_) => {} }
            match fs.rename(&pb, &pt) { Err(_) => { return "err_rename_b"; } Ok(_) => {} }
            i = i + 1;
        }
        return "ok";
    "#;
    let res = out(
        "t(to_path: String, tmp_a: String, tmp_b: String) -> String",
        body,
        &[p(&target), p(&base.join("a.tmp")), p(&base.join("b.tmp"))],
    );
    std::fs::write(&stop_file, "").unwrap();
    let outcome = reader.wait_with_output().unwrap();
    let reader_out = String::from_utf8_lossy(&outcome.stdout).trim().to_string();
    assert_eq!(res, "\"ok\"");
    assert!(
        reader_out.starts_with("OK:") && reader_out != "OK:0",
        "読み手が置換中の空白や途中の内容を見た、または一度も読めなかった: {reader_out}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// 公開の手順（一時ファイルに書く → sync_all → rename → 親の sync）を、各境界で止めたときの状態。
/// 公開点（rename）より前に止まれば元の内容のまま、後なら新しい内容で、途中の内容や欠落は見えない。
/// プロセスの電源断の再現ではない（ADR 0045 完了条件 3 のとおり、同期済み境界以降の喪失を再現する永続化モデルは DB の試験で別に行う）。
#[test]
fn publish_protocol_is_all_or_nothing_at_every_boundary() {
    let _serial = serial();
    let body = r#"
        let pt = Path.new(&target);
        let ptmp = Path.new(&tmp);
        let dp = Path.new(&dir_path);
        // 手順 1: 一時ファイルに新しい内容を書く。
        match fs.write(&ptmp, &"NEW-CONTENT") { Err(_) => { return "err_write_tmp"; } Ok(_) => {} }
        if stop_after == 1 { return "stopped"; }
        // 手順 2: 一時ファイルを sync_all する。
        let rw = fs.open_options().write(true);
        match rw.open(&ptmp) {
            Err(_) => { return "err_open_tmp"; }
            Ok(f) => {
                match f.sync_all() { Err(_) => { return "err_sync_all"; } Ok(_) => {} }
                let _ = f.close();
            }
        }
        if stop_after == 2 { return "stopped"; }
        // 手順 3: 公開点。同一 FS の原子的な置換。
        match fs.rename(&ptmp, &pt) { Err(_) => { return "err_rename"; } Ok(_) => {} }
        if stop_after == 3 { return "stopped"; }
        // 手順 4: 置換先の親ディレクトリを sync して、持続性を確定する。
        match fs.open_dir(&dp) {
            Err(_) => { return "err_open_dir"; }
            Ok(d) => {
                match d.sync() { Err(_) => { return "err_dir_sync"; } Ok(_) => {} }
                let _ = d.close();
            }
        }
        return "published";
    "#;
    for stop_after in 0..=4i64 {
        let base = scratch_dir(&format!("publish{stop_after}"));
        let target = base.join("current.txt");
        let tmp = base.join("current.txt.tmp");
        std::fs::write(&target, "OLD-CONTENT").unwrap();
        let res = out(
            "t(target: String, tmp: String, dir_path: String, stop_after: Int) -> String",
            body,
            &[p(&target), p(&tmp), p(&base), Arg::Int(stop_after)],
        );
        // stop_after == 0 は「止めない」（最後の手順 4 まで進む）。1〜3 はその境界で止める。
        let want_res = if stop_after == 0 || stop_after == 4 {
            "\"published\""
        } else {
            "\"stopped\""
        };
        // stop_after == 0 は「止めない」。
        assert_eq!(res, want_res, "stop_after={stop_after}");
        let got = std::fs::read_to_string(&target).unwrap();
        let published = stop_after == 0 || stop_after >= 3;
        assert_eq!(
            got,
            if published {
                "NEW-CONTENT"
            } else {
                "OLD-CONTENT"
            },
            "stop_after={stop_after}: 公開点より前は元の内容、後は新しい内容"
        );
        // 公開点より前で止まったときは、新しい内容は一時ファイルにだけあり、公開点の後は一時ファイルが無い。
        if (1..3).contains(&stop_after) {
            let want_tmp = "NEW-CONTENT";
            assert_eq!(std::fs::read_to_string(&tmp).unwrap(), want_tmp);
        } else {
            assert!(
                !tmp.exists(),
                "stop_after={stop_after}: 一時ファイルが残っている"
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}

/// 差し替え（`--fs-double`）下の rename は、実ファイルシステムも表も触らず、明示的に拒否される（IoError.Other）。
/// 仮のパスが偶然実在するパスと同じでも、本物の renameat を発行しない。
#[test]
fn rename_under_fs_double_fails_closed_and_never_touches_the_real_file_system() {
    let _serial = serial();
    use std::collections::BTreeMap;
    let base = scratch_dir("double");
    let real = base.join("real.txt");
    let other = base.join("other.txt");
    std::fs::write(&real, "REAL").unwrap();
    let mut files = BTreeMap::new();
    files.insert(real.to_string_lossy().into_owned(), "virtual".to_string());
    let opts = Options {
        fs_files: Some(files),
        ..Options::default()
    };
    let body = r#"
        let pr = Path.new(&real_path);
        let po = Path.new(&other_path);
        match fs.rename(&pr, &po) {
            Ok(_) => { return "err_rename_succeeded_under_double"; }
            Err(e) => {
                match e {
                    IoError.Other(_) => {}
                    _ => { return "err_not_other"; }
                }
            }
        }
        // 表の内容は変わらない。
        match fs.read(&pr) {
            Err(_) => { return "err_virtual_lost"; }
            Ok(s) => { if s != "virtual" { return "err_virtual_changed"; } }
        }
        return "ok";
    "#;
    let src = format!(
        "{HEAD}{}",
        f("t(real_path: String, other_path: String) -> String", body)
    );
    let res = sigil::ir_eval::run_with(&src, "t", &[p(&real), p(&other)], &opts)
        .unwrap()
        .value;
    assert_eq!(res, "\"ok\"");
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "REAL");
    assert!(!other.exists());
    let _ = std::fs::remove_dir_all(&base);
}

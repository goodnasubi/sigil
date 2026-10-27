//! CLI の入口の読み込み（parse/fmt/spec/check/run/build）も `modpath::load` と同じ安全な読み込み
//! （通常ファイルだけ・`O_NONBLOCK`・1MiB 上限）を使う。FIFO・`/dev/zero`・巨大ファイルで固まらず拒否される。
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-sr-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 5 秒で終わらなければ殺して失敗にする（固まりの検出）。(終了コードあり, stderr)
fn run(args: &[&str]) -> (bool, i32, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let t = Instant::now();
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            let mut err = String::new();
            use std::io::Read;
            c.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            return (true, st.code().unwrap_or(-1), err);
        }
        if t.elapsed() > Duration::from_secs(5) {
            let _ = c.kill();
            let _ = c.wait();
            return (false, -1, String::new());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn each_command(f: &Path, out: &Path, expect: &str) {
    let f = f.to_str().unwrap();
    let out = out.to_str().unwrap();
    let root = f.rsplit_once('/').unwrap().0;
    let cmds: Vec<Vec<&str>> = vec![
        vec!["parse", f],
        vec!["fmt", f],
        vec!["fmt", "--check", f],
        vec!["spec", f],
        vec!["check", f],
        vec!["run", "--root", root, f, "main"],
        vec!["build", "--root", root, f, "main", "-o", out],
    ];
    for a in cmds {
        let (done, code, err) = run(&a);
        assert!(done, "{a:?} が固まった");
        assert_ne!(code, 0, "{a:?} が成功した");
        assert!(err.contains(expect), "{a:?}: {err}");
    }
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_by_every_command() {
    let d = tmp("fifo");
    let p = d.join("pipe.sigil");
    if !matches!(Command::new("mkfifo").arg(&p).status(), Ok(s) if s.success()) {
        return;
    }
    each_command(&p, &d.join("o"), "通常のファイルではない");
}

#[cfg(unix)]
#[test]
fn dev_zero_symlink_is_rejected_by_every_command() {
    let d = tmp("zero");
    let p = d.join("z.sigil");
    std::os::unix::fs::symlink("/dev/zero", &p).unwrap();
    each_command(&p, &d.join("o"), "通常のファイルではない");
}

#[test]
fn huge_file_is_rejected_by_every_command() {
    let d = tmp("big");
    let p = d.join("big.sigil");
    std::fs::write(&p, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    each_command(&p, &d.join("o"), "大きすぎる");
}

#[test]
fn ordinary_file_still_reads() {
    let d = tmp("ok");
    let p = d.join("ok.sigil");
    std::fs::write(&p, "module ok\n  targets server\n  trust none\n").unwrap();
    let (done, code, _) = run(&["parse", p.to_str().unwrap()]);
    assert!(done);
    assert_eq!(code, 0);
}

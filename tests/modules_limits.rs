//! ADR 0021 2-2d 追補: `modpath::load`（CLI と LSP が共有）と `use` の連鎖の資源上限。
//! FIFO・デバイスは読まない、根の外は読む前に拒否、巨大ファイルは上限で打ち切る、
//! `use` の連鎖は深さとファイル数に上限がある。
use sigil::effects::Options;
use sigil::modpath::{self, LoadError, MAX_SOURCE_BYTES};
use sigil::project::{check_project, ProjectError, MAX_FILES, MAX_USE_DEPTH};
use sigil::ErrorKind;
use std::path::PathBuf;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sigil-lim-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }
    fn write(&self, rel: &str, src: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, src).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const TAIL: &str =
    "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n";

fn module(name: &str, uses: &str) -> String {
    module_fn(name, uses, "f")
}

fn module_fn(name: &str, uses: &str, f: &str) -> String {
    format!(
        "module {name}\n  targets server\n  trust none\n{uses}\npub fn {f}(n: Int) -> Int\n{TAIL}"
    )
}

fn io_msg(r: Result<(String, sigil::ast::File), LoadError>) -> String {
    match r {
        Err(LoadError::Io(m)) => m,
        Err(LoadError::Lang(e)) => panic!("Io のはずが Lang: {e:?}"),
        Ok(_) => panic!("拒否されるはずが成功した"),
    }
}

fn kind(root: &std::path::Path, entry: &str) -> Option<ErrorKind> {
    match check_project(root, &root.join(entry), &Options::default()) {
        Err(ProjectError::Diag { error, .. }) => Some(error.kind),
        Err(ProjectError::Read { msg, .. }) => panic!("Read: {msg}"),
        Ok(()) => None,
    }
}

#[cfg(unix)]
#[test]
fn fifo_is_rejected_without_blocking() {
    let t = Tmp::new("fifo");
    let p = t.0.join("pipe.sigil");
    let ok = std::process::Command::new("mkfifo").arg(&p).status();
    if !matches!(ok, Ok(s) if s.success()) {
        return; // mkfifo がない環境
    }
    let m = io_msg(modpath::load(&t.0, &p));
    assert!(m.contains("通常のファイルではない"), "{m}");
}

#[cfg(unix)]
#[test]
fn symlink_to_device_is_rejected() {
    let t = Tmp::new("dev");
    let p = t.0.join("z.sigil");
    std::os::unix::fs::symlink("/dev/zero", &p).unwrap();
    let m = io_msg(modpath::load(&t.0, &p));
    assert!(m.contains("通常のファイルではない"), "{m}");
}

#[cfg(unix)]
#[test]
fn outside_root_is_rejected_before_reading() {
    let t = Tmp::new("out-root");
    let o = Tmp::new("out-other");
    // UTF-8 でない中身。読んでいれば「UTF-8 でない」の Io になる。読む前に拒否するので根の外の診断になる。
    let target = o.0.join("t.sigil");
    std::fs::write(&target, [0xff, 0xfe, 0xfd]).unwrap();
    let link = t.0.join("esc.sigil");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    match modpath::load(&t.0, &link) {
        Err(LoadError::Lang(e)) => assert_eq!(e.kind, ErrorKind::ModulePathMismatch),
        other => panic!("{other:?}"),
    }
}

#[test]
fn huge_file_is_cut_off_with_an_explicit_error() {
    let t = Tmp::new("big");
    let mut src = module("big", "");
    src.push_str("\n//");
    src.push_str(&"x".repeat(MAX_SOURCE_BYTES as usize));
    let p = t.write("big.sigil", &src);
    let m = io_msg(modpath::load(&t.0, &p));
    assert!(m.contains("大きすぎる"), "{m}");
    // ちょうど上限なら読める。
    let mut ok = module("fit", "");
    ok.push_str("\n//");
    let pad = MAX_SOURCE_BYTES as usize - ok.len();
    ok.push_str(&"x".repeat(pad));
    assert_eq!(ok.len() as u64, MAX_SOURCE_BYTES);
    let p = t.write("fit.sigil", &ok);
    assert!(modpath::load(&t.0, &p).is_ok());
}

#[test]
fn huge_dependency_is_reported_on_the_use_line() {
    let t = Tmp::new("big-dep");
    let mut src = module("dep", "");
    src.push_str("\n//");
    src.push_str(&"x".repeat(MAX_SOURCE_BYTES as usize));
    t.write("dep.sigil", &src);
    t.write("main.sigil", &module("main", "\nuse dep.f;\n"));
    assert_eq!(kind(&t.0, "main.sigil"), Some(ErrorKind::UnresolvedUse));
}

fn chain(t: &Tmp, n: usize) {
    for i in 0..n {
        let uses = if i + 1 < n {
            format!("\nuse c{}.f;\n", i + 1)
        } else {
            String::new()
        };
        t.write(&format!("c{i}.sigil"), &module(&format!("c{i}"), &uses));
    }
}

#[test]
fn use_chain_depth_is_limited() {
    // 上限ちょうどの連鎖は ModuleLimit にならない（別の診断で止まってよい）。
    let t = Tmp::new("deep-ok");
    chain(&t, MAX_USE_DEPTH);
    assert_ne!(kind(&t.0, "c0.sigil"), Some(ErrorKind::ModuleLimit));
    // 1 つ超えると明示エラー（スタックを溢れさせない）。
    let t = Tmp::new("deep-ng");
    chain(&t, MAX_USE_DEPTH + 1);
    assert_eq!(kind(&t.0, "c0.sigil"), Some(ErrorKind::ModuleLimit));
}

#[test]
fn very_deep_chain_does_not_overflow_the_stack() {
    let t = Tmp::new("deep-huge");
    chain(&t, 5000);
    assert_eq!(kind(&t.0, "c0.sigil"), Some(ErrorKind::ModuleLimit));
}

#[test]
fn file_count_is_limited() {
    let t = Tmp::new("wide");
    let n = MAX_FILES + 44;
    let mut uses = String::new();
    for i in 0..n {
        uses.push_str(&format!("use x{i:04}.g{i};\n"));
        t.write(
            &format!("x{i:04}.sigil"),
            &module_fn(&format!("x{i:04}"), "", &format!("g{i}")),
        );
    }
    t.write("main.sigil", &module("main", &format!("\n{uses}")));
    assert_eq!(kind(&t.0, "main.sigil"), Some(ErrorKind::ModuleLimit));
}

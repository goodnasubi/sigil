//! ADR 0025 補足 C-7 の 3-6b: `fs.read`/`fs.write` と `Path` の Sigil 実装（`std.os.fs`・`std.core.path`）。
//!
//! `common::run_with` は同じプログラムを評価器（Rust の組み込み）とネイティブ（Sigil 実装）の両方で走らせて突き合わせるので、
//! ここでは Rust の `std::path::Path`（`shared_path`）を参照実装にして、ネイティブの Sigil 版が一致することを確かめる。
mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use sigil::shared_path as sp;
use std::path::PathBuf;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn f(sig: &str, effects: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n")
}

fn out(sig: &str, effects: &str, body: &str, args: &[Arg]) -> String {
    let src = format!("{HEAD}{}", f(sig, effects, body));
    run_with(&src, "t", args, &Options::default())
        .unwrap_or_else(|e| panic!("{e}\n{src}"))
        .value
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-36b-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn q(s: &str) -> String {
    format!("{s:?}")
}

const PATHS: &[&str] = &[
    "",
    "/",
    "//",
    "///",
    ".",
    "./",
    "..",
    "../",
    "a",
    "a/",
    "a//",
    "/a",
    "//a",
    "/a/",
    "a/b",
    "a//b",
    "a/b/",
    "/a/b",
    "/a/b/c.txt",
    "a/.",
    "a/./",
    "a/./b",
    "./a",
    "./a/b",
    "././a",
    "./.",
    "/.",
    "/./a",
    "a/..",
    "../a",
    "/a/..",
    "a/b/..",
    "a/b/../c",
    "..a",
    "a..",
    "...",
    ".hidden",
    "a b/c d",
    "こんにちは/世界.txt",
    "/日本語",
    "a/b/c/d/e/f/g",
    "x.tar.gz",
    "/etc/app/conf.toml",
    "a/.//./b//",
    "/..",
    "/../a",
];

fn s(x: &str) -> Arg {
    Arg::Str(x.to_string())
}

/// Sigil の文字列リテラルにする。
fn lit(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                o.push_str(&format!("\\u{{{:x}}}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// 全事例を 1 つのプログラムにまとめる（ネイティブの組み立ては 1 回だけにする）。
fn batch(ty: &str, pushes: &[String]) -> String {
    let body = format!("let mut r = List.new();\n{}\nreturn r;", pushes.join("\n"));
    out(&format!("t() -> List<{ty}>"), "none", &body, &[])
}

fn list_of(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

#[test]
fn path_file_name_matches_rust() {
    let pushes: Vec<String> = PATHS
        .iter()
        .map(|p| {
            let l = lit(p);
            format!("match Path.new(&{l}).file_name() {{\n  Some(n) => {{ r.push(n); }}\n  None => {{ r.push(\"<none>\"); }}\n}}")
        })
        .collect();
    let want: Vec<String> = PATHS
        .iter()
        .map(|p| q(&sp::file_name(p).unwrap_or("<none>".to_string())))
        .collect();
    assert_eq!(batch("String", &pushes), list_of(&want));
}

#[test]
fn path_parent_matches_rust() {
    let pushes: Vec<String> = PATHS
        .iter()
        .map(|p| {
            let l = lit(p);
            format!("match Path.new(&{l}).parent() {{\n  Some(n) => {{ r.push(n); }}\n  None => {{ r.push(Path.new(&\"<none>\")); }}\n}}")
        })
        .collect();
    let want: Vec<String> = PATHS
        .iter()
        .map(|p| q(&sp::parent(p).unwrap_or("<none>".to_string())))
        .collect();
    assert_eq!(batch("Path", &pushes), list_of(&want));
}

#[test]
fn path_join_matches_rust() {
    let others = ["", "b", "/abs", "x/y", "../c", "./d", "e/", "日本"];
    let mut pushes = vec![];
    let mut want = vec![];
    for a in PATHS {
        for b in others {
            pushes.push(format!("r.push(Path.new(&{}).join(&{}));", lit(a), lit(b)));
            want.push(q(&sp::join(a, b)));
        }
    }
    assert_eq!(batch("Path", &pushes), list_of(&want));
}

// ---- fs ----

fn p(x: &std::path::Path) -> Arg {
    Arg::Str(x.to_string_lossy().into_owned())
}

const READ: &str = "t(a: String) -> Result<String, IoError>";

#[test]
fn fs_read_sizes_around_the_growth_boundary() {
    let d = dir("sizes");
    // 実装の拡張境界はバイト数（最初の領域は 4096 バイト）。ASCII で境界の前後・倍・複数回の拡張を通す。
    for n in [0usize, 1, 4095, 4096, 4097, 8191, 8192, 8193, 70_000] {
        let f = d.join(format!("f{n}.txt"));
        let body: String = "abcdefg\n".chars().cycle().take(n).collect();
        std::fs::write(&f, &body).unwrap();
        let got = out(READ, "fs.read", "return fs.read(&Path.new(&a));", &[p(&f)]);
        assert_eq!(got, format!("Ok({body:?})"), "n={n}");
    }
    // 多バイト文字（読んだバイト列の途中で領域が満杯になる）は別事例にする
    for n in [1366usize, 1367, 1368, 2731, 20_000] {
        let f = d.join(format!("m{n}.txt"));
        let body: String = "あaé".chars().cycle().take(n).collect();
        std::fs::write(&f, &body).unwrap();
        let got = out(READ, "fs.read", "return fs.read(&Path.new(&a));", &[p(&f)]);
        assert_eq!(got, format!("Ok({body:?})"), "multibyte n={n}");
    }
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_read_invalid_utf8_is_other_and_missing_is_not_found() {
    let d = dir("bad");
    let f = d.join("bad.bin");
    std::fs::write(&f, [0x66, 0xff, 0xfe, 0x6f]).unwrap();
    let got = out(READ, "fs.read", "return fs.read(&Path.new(&a));", &[p(&f)]);
    assert!(got.starts_with("Err(IoError.Other"), "{got}");
    let got = out(
        READ,
        "fs.read",
        "return fs.read(&Path.new(&a));",
        &[p(&d.join("none"))],
    );
    assert!(got.starts_with("Err(IoError.NotFound"), "{got}");
    let got = out(READ, "fs.read", "return fs.read(&Path.new(&a));", &[p(&d)]);
    assert!(got.starts_with("Err(IoError.Other"), "{got}");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_read_and_write_path_with_nul_are_other_and_do_not_touch_the_prefix() {
    // NUL より前のパス（<dir>/a）が実在しても、切り詰めて開かないこと（K-4）。
    // コマンドライン引数に NUL は渡せないので、プログラムの中で作る。
    let d = dir("nul");
    let victim = d.join("a");
    std::fs::write(&victim, "orig").unwrap();
    let l = lit(&victim.to_string_lossy());
    let base = format!("{}\\u{{0}}b\"", &l[..l.len() - 1]);
    let prog = |call: &str, ty: &str, eff: &str| {
        let src = format!(
            "{HEAD}{}",
            f(
                &format!("t() -> {ty}"),
                eff,
                &format!("let p = Path.new(&{base});\nreturn {call};")
            )
        );
        run_with(&src, "t", &[], &Options::default())
            .unwrap_or_else(|e| panic!("{e}\n{src}"))
            .value
    };
    let r = prog("fs.read(&p)", "Result<String, IoError>", "fs.read");
    assert!(r.starts_with("Err(IoError.Other"), "{r}");
    let w = prog(
        "fs.write(&p, &\"changed\")",
        "Result<Unit, IoError>",
        "fs.write",
    );
    assert!(w.starts_with("Err(IoError.Other"), "{w}");
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "orig");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fs_write_creates_truncates_and_classifies_errors() {
    let d = dir("write");
    let f = d.join("w.txt");
    let sig = "t(a: String, c: String) -> Result<Unit, IoError>";
    let body = "return fs.write(&Path.new(&a), &c);";
    let big: String = "x".repeat(100_000);
    for c in ["hello\n", "", "short", big.as_str()] {
        let got = out(sig, "fs.write", body, &[p(&f), s(c)]);
        assert_eq!(got, "Ok(())", "{}", c.len());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), c);
    }
    let got = out(
        sig,
        "fs.write",
        body,
        &[p(&d.join("x").join("y.txt")), s("")],
    );
    assert!(got.starts_with("Err(IoError.NotFound"), "{got}");
    let got = out(sig, "fs.write", body, &[p(&d), s("")]);
    assert!(got.starts_with("Err(IoError.Other"), "{got}");
    let _ = std::fs::remove_dir_all(&d);
}

#[cfg(unix)]
#[test]
fn fs_permission_denied_maps_to_permission_denied() {
    use std::os::unix::fs::PermissionsExt;
    // root は権限検査を通り抜けるので、読めなくなったことを確かめてから比べる
    let d = dir("perm");
    let f = d.join("secret.txt");
    std::fs::write(&f, "x").unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_to_string(&f).is_ok() {
        let _ = std::fs::remove_dir_all(&d);
        return;
    }
    let got = out(READ, "fs.read", "return fs.read(&Path.new(&a));", &[p(&f)]);
    assert!(got.starts_with("Err(IoError.PermissionDenied"), "{got}");
    let _ = std::fs::remove_dir_all(&d);
}

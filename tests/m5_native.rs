//! ADR 0015 の決定 4・6・7（PR4）: `sigil build` が作るネイティブの実行ファイル。
//!
//! 構文がまだ全部は出せない（`BuildUnsupported`）間は、`SIGIL_NATIVE=1` のときだけ走る
//! （`cc` と glibc の静的ライブラリ `libc.a` が要る）。走らない旨は標準エラーに出す。
//! `SIGIL_NATIVE=1 cargo test` では、ほかの試験の `common::run*` も同じプログラムをネイティブで走らせて突き合わせる。
mod common;

use common::native::{arg_text, exec, scratch, try_build};
use sigil::codegen::Opts;
use sigil::interp::Arg;
use sigil::native::{self, BuildError};
use std::process::Command;

fn on() -> bool {
    if common::native::enabled() {
        return true;
    }
    eprintln!("SIGIL_NATIVE=1 でないので、このテストは飛ばす");
    false
}

fn example(path: &str) -> String {
    std::fs::read_to_string(format!("{}/examples/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn floats(xs: &[f64]) -> Arg {
    Arg::List(xs.iter().map(|x| Arg::Float(*x)).collect())
}

/// ビルドして走らせ、インタープリタ（`common::run*`）と同じ結果かを突き合わせる（`common` が行う）。
fn both(srcs: &[&str], func: &str, args: &[Arg]) -> String {
    let r = common::run_sources(srcs, func, args).unwrap();
    // `common::run_sources` が SIGIL_NATIVE=1 のとき突き合わせる。ここでは、飛ばされていないことを確かめる。
    let dir = scratch("both");
    let exe = dir.join("prog");
    assert!(
        try_build(srcs, func, &exe).unwrap().is_some(),
        "{func}: BuildUnsupported になった"
    );
    let got = exec(&exe, args);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{func}: {}", got.stderr);
    assert_eq!(got.stdout, format!("{}{}\n", r.output, r.value), "{func}");
    r.value
}

#[test]
fn samples_build_and_match() {
    if !on() {
        return;
    }
    let avg = example("01-average/c-brace.sigil");
    assert_eq!(
        both(&[&avg], "average", &[floats(&[1.0, 2.0, 3.0, 4.0])]),
        "2.5"
    );
    let own = example("03-ownership/ownership.sigil");
    both(&[&own], "main", &[]);
    let eff = example("04-effects/effects.sigil");
    let entry = example("04-effects/entry.sigil");
    assert_eq!(both(&[&eff, &entry], "bump_twice", &[]), "7");
    both(&[&eff, &entry], "tax", &[]);
    both(&[&eff], "add_tax", &[Arg::Int(100), Arg::Int(10)]);
    let ctl = example("05-control/control.sigil");
    both(&[&ctl], "sum_range", &[Arg::Int(1), Arg::Int(10)]);
    for x in [-5, 3, 50] {
        both(&[&ctl], "clamp", &[Arg::Int(x), Arg::Int(0), Arg::Int(10)]);
        both(&[&ctl], "sign", &[Arg::Int(x)]);
    }
    both(&[&ctl], "add_to", &[Arg::Int(3), Arg::Int(4)]);
}

#[test]
fn read_port_with_real_file() {
    if !on() {
        return;
    }
    let port = example("02-read-port/c-brace.sigil");
    let dir = scratch("port");
    let exe = dir.join("prog");
    assert!(try_build(&[&port], "read_port", &exe).unwrap().is_some());
    let file = dir.join("p.txt");
    let path = file.to_string_lossy().into_owned();
    for (content, want) in [("8080", "Ok(8080)"), ("abc", "Err(")] {
        std::fs::write(&file, content).unwrap();
        let got = exec(&exe, &[Arg::Str(path.clone())]);
        assert_eq!(got.code, Some(0), "{content}: {}", got.stderr);
        assert!(got.stdout.starts_with(want), "{content}: {}", got.stdout);
    }
    let _ = std::fs::remove_file(&file);
    let got = exec(&exe, &[Arg::Str(path)]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert!(got.stdout.contains("NotFound"), "{}", got.stdout);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn core_only_links_without_os_runtime() {
    if !on() {
        return;
    }
    let ctl = example("05-control/control.sigil");
    let p = sigil::ir_eval::lower_sources(&[&ctl], "sum_range").unwrap();
    assert!(p.core_only, "sum_range は OS 層を使わない");
    let dir = scratch("core");
    let exe = dir.join("prog");
    // core_only のときは os.a を渡さない。それでもリンクできて走る。
    native::build_program(&p, &Opts::default(), &exe).unwrap();
    let got = exec(&exe, &[Arg::Int(1), Arg::Int(10)]);
    assert_eq!(got.stdout, "55\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn core_only_rejects_os_link() {
    if !on() {
        return;
    }
    // console を使うプログラムを core_only と偽ると、os.a がなくてリンクに失敗する（失敗が黙って通らない）。
    let own = example("03-ownership/ownership.sigil");
    let mut p = sigil::ir_eval::lower_sources(&[&own], "main").unwrap();
    assert!(!p.core_only);
    p.core_only = true;
    let dir = scratch("reject");
    let r = native::build_program(&p, &Opts::default(), &dir.join("prog"));
    assert!(matches!(r, Err(BuildError::Toolchain(_))), "{r:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// ELF のプログラムヘッダに `PT_INTERP`・`PT_DYNAMIC` がない（動的リンクでない）。
fn assert_static(exe: &std::path::Path) {
    let b = std::fs::read(exe).unwrap();
    assert_eq!(&b[..4], b"\x7fELF");
    assert_eq!(b[4], 2, "ELF64");
    let u16at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let u32at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let u64at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) as usize;
    let (off, size, n) = (u64at(0x20), u16at(0x36), u16at(0x38));
    assert!(n > 0);
    for i in 0..n {
        let t = u32at(off + i * size);
        assert!(
            t != 2 && t != 3,
            "PT_DYNAMIC か PT_INTERP がある（type {t}）"
        );
    }
}

#[test]
fn executable_is_static() {
    if !on() {
        return;
    }
    let own = example("03-ownership/ownership.sigil");
    let ctl = example("05-control/control.sigil");
    let dir = scratch("static");
    for (src, f) in [(&own, "main"), (&ctl, "sign")] {
        let exe = dir.join(f);
        assert!(try_build(&[src], f, &exe).unwrap().is_some());
        assert_static(&exe);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn runs_without_toolchain() {
    if !on() {
        return;
    }
    let own = example("03-ownership/ownership.sigil");
    let dir = scratch("bare");
    let exe = dir.join("prog");
    assert!(try_build(&[&own], "main", &exe).unwrap().is_some());
    // 環境変数なし・PATH なし・別のディレクトリで走る（cc も Rust もない環境の代わり）。
    let o = Command::new(&exe)
        .env_clear()
        .current_dir(&dir)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "150\n150\n()\n");
    let _ = std::fs::remove_dir_all(&dir);
}

const HEAD: &str = "module t\n  targets server\n  trust none\n\n";

fn f(name: &str, sig: &str, body: &str) -> String {
    format!("fn {name}{sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n")
}

#[test]
fn abnormal_stop_exits_70_with_report() {
    if !on() {
        return;
    }
    let src = format!(
        "{HEAD}{}",
        f("d", "(x: Int, y: Int) -> Int", "return x / y;")
    );
    let want = common::run(&src, "d", &[Arg::Int(1), Arg::Int(0)]).unwrap_err();
    let dir = scratch("stop");
    let exe = dir.join("prog");
    assert!(try_build(&[&src], "d", &exe).unwrap().is_some());
    let got = exec(&exe, &[Arg::Int(1), Arg::Int(0)]);
    assert_eq!(got.code, Some(70));
    assert_eq!(got.stderr.trim_end(), want.error.to_string());
    assert_eq!(arg_text(&Arg::Int(7)), "7");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cli_build_and_trace_drops_unsupported() {
    if !on() {
        return;
    }
    let ctl = format!(
        "{}/examples/05-control/control.sigil",
        env!("CARGO_MANIFEST_DIR")
    );
    let dir = scratch("cli");
    let exe = dir.join("prog");
    let bin = env!("CARGO_BIN_EXE_sigil");
    let o = Command::new(bin)
        .args(["build", &ctl, "sum_range", "-o"])
        .arg(&exe)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(exec(&exe, &[Arg::Int(1), Arg::Int(4)]).stdout, "10\n");
    let o = Command::new(bin)
        .args(["build", "--trace-drops", &ctl, "sum_range", "-o"])
        .arg(dir.join("x"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("trace-drops"));
    assert!(!dir.join("x").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

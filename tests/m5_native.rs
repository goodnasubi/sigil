//! ADR 0015 の決定 4・6・7（PR4）: `sigil build` が作るネイティブの実行ファイル。
//!
//! `cc` と glibc の静的ライブラリ `libc.a` が要る（なければ失敗する。飛ばさない）。
//! ほかの試験の `common::run*` も、同じプログラムをネイティブで走らせて突き合わせる。
mod common;

use common::native::{arg_text, exec, scratch, try_build};
use sigil::codegen::Opts;
use sigil::exec::Arg;
use sigil::native::{self, BuildError};
use std::process::Command;

fn example(path: &str) -> String {
    std::fs::read_to_string(format!("{}/examples/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn floats(xs: &[f64]) -> Arg {
    Arg::List(xs.iter().map(|x| Arg::Float(*x)).collect())
}

/// ビルドして走らせ、SIR の評価器（`common::run*`）と同じ結果かを突き合わせる（`common` が行う）。
fn both(srcs: &[&str], func: &str, args: &[Arg]) -> String {
    let r = common::run_sources(srcs, func, args).unwrap();
    // `common::run_sources` も突き合わせる。ここでは、飛ばされずに作れて走ることを確かめる。
    let dir = scratch("both");
    let exe = dir.join("prog");
    try_build(srcs, func, &exe, false).unwrap();
    let got = exec(&exe, args);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{func}: {}", got.stderr);
    assert_eq!(got.stdout, format!("{}{}\n", r.output, r.value), "{func}");
    r.value
}

#[test]
fn samples_build_and_match() {
    let avg = example("01-average/c-brace.sigil");
    assert_eq!(
        both(&[&avg], "average", &[floats(&[1.0, 2.0, 3.0, 4.0])]),
        "2.5"
    );
    let own = example("03-ownership/ownership.sigil");
    both(&[&own], "main", &[]);
    let eff = example("04-effects/effects.sigil");
    let [entry, eff_pub] = common::effects_pair();
    assert_eq!(both(&[&entry, &eff_pub], "bump_twice", &[]), "7");
    both(&[&entry, &eff_pub], "tax", &[]);
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
    let port = example("02-read-port/c-brace.sigil");
    let dir = scratch("port");
    let exe = dir.join("prog");
    try_build(&[&port], "read_port", &exe, false).unwrap();
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
    let own = example("03-ownership/ownership.sigil");
    let ctl = example("05-control/control.sigil");
    let dir = scratch("static");
    for (src, f) in [(&own, "main"), (&ctl, "sign")] {
        let exe = dir.join(f);
        try_build(&[src], f, &exe, false).unwrap();
        assert_static(&exe);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn runs_without_toolchain() {
    let own = example("03-ownership/ownership.sigil");
    let dir = scratch("bare");
    let exe = dir.join("prog");
    try_build(&[&own], "main", &exe, false).unwrap();
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
    let src = format!(
        "{HEAD}{}",
        f("d", "(x: Int, y: Int) -> Int", "return x / y;")
    );
    let want = common::run(&src, "d", &[Arg::Int(1), Arg::Int(0)]).unwrap_err();
    let dir = scratch("stop");
    let exe = dir.join("prog");
    try_build(&[&src], "d", &exe, false).unwrap();
    let got = exec(&exe, &[Arg::Int(1), Arg::Int(0)]);
    assert_eq!(got.code, Some(70));
    assert_eq!(got.stderr.trim_end(), want.error.to_string());
    assert_eq!(arg_text(&Arg::Int(7)), "7");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cli_build_and_trace_drops() {
    let (root, ctl) = common::stage_file(
        "ctl",
        &format!(
            "{}/examples/05-control/control.sigil",
            env!("CARGO_MANIFEST_DIR")
        ),
    );
    let ctl = ctl.to_str().unwrap().to_string();
    let root = root.to_str().unwrap().to_string();
    let dir = scratch("cli");
    let exe = dir.join("prog");
    let bin = env!("CARGO_BIN_EXE_sigil");
    let o = Command::new(bin)
        .args(["build", "--root", &root, &ctl, "sum_range", "-o"])
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
        .args([
            "build",
            "--trace-drops",
            "--root",
            &root,
            &ctl,
            "sum_range",
            "-o",
        ])
        .arg(dir.join("x"))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(dir.join("x").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// ADR 0017: `sigil run --trace-drops` の破棄の記録は、`sigil build --trace-drops` の実行ファイルと同じ。
#[test]
fn cli_run_and_build_trace_match() {
    const HEAD: &str = "module m.a\n  targets server\n  trust none\n\nstruct Probe {\n  id: Int,\n}\n\nstruct Two {\n  a: Probe,\n  b: Probe,\n}\n\nfn mk(n: Int) -> Probe\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Probe { id: n };\n}\n\n";
    let bodies = [
        "let t = Two { a: mk(1), b: mk(2) };\nlet y = mk(9);\nlet x = t.a;\nreturn 0;",
        "let o = Some(mk(1));\nlet z = mk(5);\nmatch o {\n    Some(p) => {\n        let q = mk(7);\n    }\n    None => {}\n}\nreturn 0;",
        "let o = Some(mk(1));\nlet z = mk(5);\nlet m = o.map(|p| p.id);\nreturn 0;",
        "let p = mk(1);\nlet z = mk(5);\nlet c = move || p.id;\nlet w = mk(6);\nreturn c();",
        "let p = mk(1);\nlet c = move || p.id;\nlet w = mk(6);\nlet d = c;\nlet k = mk(7);\nreturn d();",
    ];
    let bin = env!("CARGO_BIN_EXE_sigil");
    let dir = scratch("run-trace");
    for (i, body) in bodies.iter().enumerate() {
        let src = format!(
            "{HEAD}fn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
        );
        let root = dir.join(format!("r{i}"));
        let file = root.join("m/a.sigil");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, src).unwrap();
        let exe = dir.join(format!("c{i}"));
        let o = Command::new(bin)
            .args(["build", "--trace-drops", "--root"])
            .arg(&root)
            .arg(&file)
            .args(["t", "-o"])
            .arg(&exe)
            .output()
            .unwrap();
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let built = exec(&exe, &[]);
        assert_eq!(built.code, Some(0), "{}", built.stderr);
        let ran = Command::new(bin)
            .args(["run", "--trace-drops", "--root"])
            .arg(&root)
            .arg(&file)
            .arg("t")
            .output()
            .unwrap();
        assert_eq!(ran.status.code(), Some(0));
        let ran_err = String::from_utf8_lossy(&ran.stderr);
        assert!(ran_err.contains("drop: "), "case {i}: {ran_err}");
        assert_eq!(ran_err, built.stderr, "case {i}: 破棄の記録");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 処理系のソースに `BuildUnsupported`（未実装の構文を build で止める印）を作る箇所が残っていない（M5 の完了の確かめ）。
#[test]
fn no_build_unsupported_left() {
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, hits);
            } else if p.extension().is_some_and(|x| x == "rs") {
                let text = std::fs::read_to_string(&p).unwrap();
                for (i, l) in text.lines().enumerate() {
                    if l.contains("BuildUnsupported") {
                        hits.push(format!("{}:{}", p.display(), i + 1));
                    }
                }
            }
        }
    }
    let mut hits = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut hits,
    );
    assert!(hits.is_empty(), "BuildUnsupported が残っている: {hits:?}");
}

/// `docker` で `FROM scratch` のイメージに実行ファイルだけを置いて走らせる（手で実行する。結果は ADR 0006 の M5 の記録）。
/// `cargo test --test m5_native -- --ignored runs_in_scratch_container`
#[test]
#[ignore]
fn runs_in_scratch_container() {
    let own = example("03-ownership/ownership.sigil");
    let dir = scratch("docker");
    let exe = dir.join("prog");
    try_build(&[&own], "main", &exe, false).unwrap();
    std::fs::write(
        dir.join("Dockerfile"),
        "FROM scratch\nCOPY prog /prog\nENTRYPOINT [\"/prog\"]\n",
    )
    .unwrap();
    let tag = format!("sigil-scratch-test-{}", std::process::id());
    let run = |args: &[&str]| {
        Command::new("docker")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap()
    };
    let b = run(&["build", "-t", &tag, "."]);
    assert!(b.status.success(), "{}", String::from_utf8_lossy(&b.stderr));
    let r = run(&["run", "--rm", &tag]);
    let _ = run(&["rmi", &tag]);
    assert_eq!(
        r.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&r.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&r.stdout), "150\n150\n()\n");
    let _ = std::fs::remove_dir_all(&dir);
}

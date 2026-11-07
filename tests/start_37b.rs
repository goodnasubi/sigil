//! ADR 0025 C-4・C-6: 起動のバイト列・直接 syscall・専用スタック。
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod common;
use common::native::{exec, exec_env, scratch, try_build};
use std::path::{Path, PathBuf};
use std::process::Command;

fn function(sig: &str, body: &str) -> String {
    format!("fn {sig}\n  intent \"起動の試験\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n")
}

fn build(items: &str) -> PathBuf {
    let src = format!("module m.start\n  targets server\n  trust none\n\n{items}");
    let exe = scratch("start37b").join("prog");
    try_build(&[&src], "main", &exe, false).unwrap();
    exe
}

fn disassemble(exe: &Path, symbol: &str) -> (String, Vec<u8>) {
    let out = Command::new("objdump")
        .arg("-d")
        .arg(format!("--disassemble={symbol}"))
        .arg(exe)
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    let bytes = text
        .lines()
        .filter_map(|line| {
            let (addr, rest) = line.trim().split_once(':')?;
            u64::from_str_radix(addr, 16).ok()?;
            Some(
                rest.split_whitespace()
                    .take_while(|s| s.len() == 2 && u8::from_str_radix(s, 16).is_ok())
                    .map(|s| u8::from_str_radix(s, 16).unwrap())
                    .collect::<Vec<_>>(),
            )
        })
        .flatten()
        .collect();
    (text, bytes)
}

#[test]
fn entry_bytes_syscall_write_and_live_environment() {
    let exe = build(&function("main() -> Int", "  return 42;"));
    let (start, bytes) = disassemble(&exe, "_start");
    assert_eq!(
        &bytes[..8],
        &[0x48, 0x89, 0xe7, 0x48, 0x83, 0xe4, 0xf0, 0xe8]
    );
    assert_eq!(&bytes[12..14], &[0x0f, 0x0b]);
    assert_eq!(bytes.len(), 14);
    let (syscall, bytes) = disassemble(&exe, "sigil_syscall6");
    assert_eq!(
        bytes,
        [
            0x48, 0x89, 0xf8, 0x48, 0x89, 0xf7, 0x48, 0x89, 0xd6, 0x48, 0x89, 0xca, 0x4d, 0x89,
            0xc2, 0x4d, 0x89, 0xc8, 0x4c, 0x8b, 0x4c, 0x24, 0x08, 0x0f, 0x05, 0xc3
        ]
    );
    println!("{start}\n{syscall}\nバイナリ: {}", exe.display());
    let limited = Command::new("prlimit")
        .arg("--as=201326592")
        .arg(&exe)
        .output()
        .unwrap();
    assert_eq!(
        limited.status.code(),
        Some(70),
        "mmap の失敗: {:?}",
        limited
    );
    let got = exec(&exe, &[]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout, "42\n");
    assert_eq!(got.stderr, "");
    for value in ["", "1", "0"] {
        let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", value)]);
        assert_eq!(got.code, Some(0));
        assert_eq!(got.stderr, "live: sigil_blocks=0\n");
    }
    {
        use std::os::unix::ffi::OsStrExt;
        let got = Command::new(&exe)
            .env("SIGIL_RT_LIVE", std::ffi::OsStr::from_bytes(&[0xff]))
            .output()
            .unwrap();
        assert_eq!(got.status.code(), Some(0));
        assert_eq!(got.stderr, b"live: sigil_blocks=0\n");
    }
    let got = exec_env(
        &exe,
        &[],
        &[("SIGIL_RT_LIVE_EXTRA", "1"), ("SIGIL_RT_LIV", "1")],
    );
    assert_eq!(got.stderr, "");
}

#[test]
fn stack_limit_reports_the_existing_stop() {
    let exe = build(&format!(
        "{}{}",
        function("down(n: Int) -> Int", "  return down(n + 1) + 1;"),
        function("main() -> Int", "  return down(0);")
    ));
    let got = exec(&exe, &[]);
    assert_eq!(got.code, Some(70), "{}", got.stderr);
    assert!(
        got.stderr
            .starts_with("異常停止: スタックの枯渇（関数 down、"),
        "{}",
        got.stderr
    );
}

#[test]
fn large_recursive_frame_stops_without_corruption() {
    // 局所は stack slot なので、各枠に十分な大きさを持たせる。
    let mut body = String::new();
    for i in 0..512 {
        body.push_str(&format!("  let x{i} = n + {i};\n"));
    }
    body.push_str("  let a0 = down(n + 1);\n");
    for i in 0..512 {
        body.push_str(&format!("  let a{} = a{i} + x{i};\n", i + 1));
    }
    body.push_str("  return a512;");
    let exe = build(&format!(
        "{}{}",
        function("down(n: Int) -> Int", &body),
        function("main() -> Int", "  return down(0);")
    ));
    let out = Command::new(&exe).output().unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert!(
        out.status.code() == Some(70) || out.status.signal() == Some(11),
        "{:?}",
        out.status
    );
    if out.status.code() == Some(70) {
        assert!(String::from_utf8_lossy(&out.stderr).contains("スタックの枯渇"));
    }
}

fn build_unchecked(body: &str) -> PathBuf {
    let src = format!("module m.rawstart\n  targets server\n  trust base\n\nfn main() -> Int\n  intent \"syscall の試験\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"Linux syscall に渡す領域を自分で管理する\"\n{{\n{body}\n}}\n");
    let exe = scratch("rawstart37b").join("prog");
    sigil::native::build(
        &[&src],
        "main",
        &sigil::codegen::Opts {
            trust_base: vec!["m.rawstart".into()],
            ..Default::default()
        },
        &exe,
    )
    .unwrap();
    exe
}

#[test]
fn raw_syscall_writes_and_exits_with_the_requested_code() {
    let exe = build_unchecked("  let p = raw.alloc(1, 1);\n  raw.store_u8(p, 65);\n  raw.syscall(1, 1, raw.addr_to_int(p), 1, 0, 0, 0);\n  return raw.syscall(231, 37, 0, 0, 0, 0, 0);");
    let got = exec(&exe, &[]);
    assert_eq!(got.stdout, "A");
    assert_eq!(got.code, Some(37));
    // mmap の第 6 引数（ページ境界でない offset）は EINVAL。
    let exe = build_unchecked("  return raw.syscall(9, 0, 4096, 3, 34, -1, 1);");
    let got = exec(&exe, &[]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout, "-22\n");
}

#[test]
fn envp_is_written_only_to_the_named_global() {
    let src = "module m.envstart\n  targets server\n  trust base\n\nglobal mut ENVP: Addr = raw.addr_null()\n\nfn main() -> Bool\n  intent \"envp の受け渡し\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"起動処理が書いた envp の番地を読む\"\n{\n  return raw.addr_to_int(ENVP) != 0;\n}\n";
    let trust_base = vec!["m.envstart".into()];
    let mut p = sigil::ir_eval::lower_sources_with(&[src], "main", &trust_base).unwrap();
    let idx = p
        .globals
        .iter()
        .position(|g| g.name.ends_with("ENVP"))
        .unwrap();
    for (name, expected) in [
        ("m.envstart.ENVP", "false\n"),
        ("std.os.env.ENVP", "true\n"),
    ] {
        p.globals[idx].name = name.into();
        let exe = scratch("envp37b").join("prog");
        sigil::native::build_program(
            &p,
            &sigil::codegen::Opts {
                trust_base: trust_base.clone(),
                ..Default::default()
            },
            &exe,
        )
        .unwrap();
        let got = exec(&exe, &[]);
        assert_eq!(got.code, Some(0), "{}", got.stderr);
        assert_eq!(got.stdout, expected);
    }
}

#[test]
fn generated_object_does_not_import_the_replaced_runtime_entries() {
    let src = format!(
        "module m.start\n  targets server\n  trust none\n\n{}",
        function("main() -> Int", "  return 0;")
    );
    let p = sigil::ir_eval::lower_sources(&[&src], "main").unwrap();
    let bytes = sigil::codegen::compile(&p, &Default::default()).unwrap();
    let path = scratch("symbols37b").join("prog.o");
    std::fs::write(&path, bytes).unwrap();
    let out = Command::new("nm").arg(&path).output().unwrap();
    assert!(out.status.success());
    let symbols = String::from_utf8(out.stdout).unwrap();
    for line in symbols.lines() {
        let name = line.split_whitespace().last().unwrap_or("");
        assert!(
            ![
                "main",
                "sigil_rt_enter",
                "sigil_rt_raw_syscall",
                "sigil_rt_stack_init",
                "sigil_rt_stack_limit"
            ]
            .contains(&name),
            "置き換えたシンボルが残っている: {name}"
        );
    }
}

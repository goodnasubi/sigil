//! ADR 0025 決定 1 / PR 3-2: 原始操作（`raw.*`）の検査と仕様表示のテスト。
//!
//! 標準ライブラリ `stdlib::RAW_OPS` に登録された 15 個の原始操作が、
//! `trust base` モジュールの `unchecked` 関数内から正しく型検査され、
//! 前提の文が検証されることを確認する。

use sigil::effects::Options;
use sigil::spec::spec_with;

fn module(items: &str) -> String {
    format!("module m.t\n  targets server\n  trust base\n\n{items}")
}

fn ufn(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n  unchecked \"領域の検証\"\n{{\n{body}\n}}\n\n"
    )
}

#[test]
fn raw_ops_check_all_core_and_os_operations() {
    let src = module(&format!(
        "{}{}{}",
        ufn(
            "test_core(a: Addr, v: Int) -> Addr",
            "none",
            r#"
  let n0 = raw.addr_null();
  let vi = raw.addr_to_int(a);
  let a2 = raw.int_to_addr(v);
  let a3 = raw.offset(a, 8);
  let b = raw.load_u8(a);
  raw.store_u8(a, 42);
  let w = raw.load_i64(a);
  raw.store_i64(a, 123456);
  let a4 = raw.load_addr(a);
  raw.store_addr(a, a2);
  raw.copy(a, a3, 16);
  raw.set(a, 0, 16);
  let p = raw.alloc(64, 16);
  raw.free(p, 64, 16);
  return n0;
"#
        ),
        ufn(
            "test_os(fd: Int, buf: Addr, count: Int) -> Int",
            "fs.read",
            r#"
  let b0 = raw.addr_to_int(buf);
  return raw.syscall(0, fd, b0, count, 0, 0, 0);
"#
        ),
        ufn("main() -> Int", "none", "  return 0;")
    ));

    let opts = Options {
        trust_base: vec!["m.t".to_string()],
        ..Default::default()
    };
    let res = sigil::check::check_with(&src, &opts);
    assert!(res.is_ok(), "型検査が通るはず: {:?}", res.err());

    let sp = spec_with(&src, &opts).expect("spec should succeed");
    assert!(sp.contains("raw.addr_null"), "{sp}");
    assert!(sp.contains("raw.alloc"), "{sp}");
    assert!(sp.contains("raw.free"), "{sp}");
    assert!(sp.contains("raw.syscall"), "{sp}");
    assert!(sp.contains("信頼基盤: はい（検査を外す関数 3 個）"), "{sp}");
}

#[test]
fn raw_ops_eval_execution() {
    let src = module(&format!(
        "{}{}",
        ufn(
            "run_test() -> Int",
            "none",
            r#"
  let p = raw.alloc(64, 16);
  raw.store_u8(p, 123);
  let b = raw.load_u8(p);
  raw.store_i64(p, 987654321);
  let w = raw.load_i64(p);
  let p2 = raw.offset(p, 16);
  raw.store_addr(p2, p);
  let loaded_p = raw.load_addr(p2);
  let vi = raw.addr_to_int(loaded_p);
  let a2 = raw.int_to_addr(vi);
  let null_a = raw.addr_null();
  let null_i = raw.addr_to_int(null_a);
  raw.free(p, 64, 16);
  return b + w + null_i;
"#
        ),
        ufn("main() -> Int", "none", "  return run_test();")
    ));

    let opts = sigil::exec::Options {
        trust_base: vec!["m.t".to_string()],
        ..Default::default()
    };
    let res = sigil::ir_eval::run_with(&src, "main", &[], &opts).expect("eval failed");
    assert_eq!(res.value, (123 + 987654321).to_string());
}

#[test]
fn raw_ops_native_build_and_run() {
    let src = module(&format!(
        "{}{}",
        ufn(
            "run_test() -> Int",
            "none",
            r#"
  let p = raw.alloc(64, 16);
  raw.store_u8(p, 123);
  let b = raw.load_u8(p);
  raw.store_i64(p, 987654321);
  let w = raw.load_i64(p);
  let p2 = raw.offset(p, 16);
  raw.store_addr(p2, p);
  let loaded_p = raw.load_addr(p2);
  let vi = raw.addr_to_int(loaded_p);
  let a2 = raw.int_to_addr(vi);
  let null_a = raw.addr_null();
  let null_i = raw.addr_to_int(null_a);
  raw.free(p, 64, 16);
  return b + w + null_i;
"#
        ),
        ufn("main() -> Int", "none", "  return run_test();")
    ));

    let root = std::env::temp_dir().join(format!("sigil-raw-native-{}", std::process::id()));
    let file = root.join("m/t.sigil");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, &src).unwrap();
    let list_path = root.join("trust-base.list");
    std::fs::write(&list_path, "m.t\n").unwrap();

    let out_bin = root.join("main.out");
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"));
    cmd.current_dir(&root);
    cmd.args([
        "build",
        file.to_str().unwrap(),
        "main",
        "-o",
        out_bin.to_str().unwrap(),
    ]);
    let output = cmd.output().expect("sigil build failed to execute");
    assert_eq!(
        output.status.code(),
        Some(0),
        "build failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run_output = std::process::Command::new(&out_bin)
        .output()
        .expect("binary execution failed");
    assert_eq!(run_output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&run_output.stdout).trim(),
        (123 + 987654321).to_string()
    );
}

//! ADR 0025 補足 A-3 / PR 3-4b: 評価器の `raw.syscall` 許可表と引数検証のテスト。

fn run(body: &str) -> Result<String, String> {
    let src = format!(
        "module m.t\n  targets server\n  trust base\n\nfn run_test() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"領域の検証\"\n{{\n{body}\n}}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"x\"\n{{\n  return run_test();\n}}\n"
    );
    let opts = sigil::exec::Options {
        trust_base: vec!["m.t".to_string()],
        ..Default::default()
    };
    match sigil::ir_eval::run_with(&src, "main", &[], &opts) {
        Ok(o) => Ok(o.value),
        Err(e) => Err(format!("{e:?}")),
    }
}

fn stops(body: &str, needle: &str) {
    let r = run(body);
    match r {
        Err(m) => assert!(m.contains(needle), "期待 {needle} / 実際 {m}"),
        Ok(v) => panic!("停止するはずが成功: {v}"),
    }
}

const MMAP: &str = "raw.syscall(9, 0, 4096, 3, 34, -1, 0)";

#[test]
fn normal_mmap_rw_munmap() {
    let r = run(&format!(
        "  let a = {MMAP};\n  let p = raw.int_to_addr(a);\n  raw.store_i64(p, 77);\n  let v = raw.load_i64(p);\n  let r = raw.syscall(11, a, 4096, 0, 0, 0, 0);\n  return v + r;"
    ));
    assert_eq!(r.unwrap(), "77");
}

#[test]
fn normal_write_inside_alloc() {
    let r = run("  let p = raw.alloc(16, 16);\n  raw.store_u8(p, 65);\n  let n = raw.syscall(1, 1, raw.addr_to_int(p), 1, 0, 0, 0);\n  raw.free(p, 16, 16);\n  return n;");
    assert_eq!(r.unwrap(), "1");
}

#[test]
fn disallowed_number_stops() {
    stops(
        "  return raw.syscall(39, 0, 0, 0, 0, 0, 0);",
        "許可表にない",
    );
}

#[test]
fn exit_group_is_simulated_stop() {
    stops(
        "  return raw.syscall(231, 3, 0, 0, 0, 0, 0);",
        "exit_group(3)",
    );
}

#[test]
fn out_of_range_write_stops() {
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(1, 1, raw.addr_to_int(p), 9, 0, 0, 0);",
        "範囲外",
    );
    stops("  return raw.syscall(1, 1, 4096, 1, 0, 0, 0);", "範囲外");
}

#[test]
fn mmap_restrictions() {
    stops(
        "  return raw.syscall(9, 0, 4096, 7, 34, -1, 0);",
        "PROT_EXEC",
    );
    stops(
        "  return raw.syscall(9, 0, 4096, 3, 50, -1, 0);",
        "MAP_FIXED",
    );
    stops("  return raw.syscall(9, 4096, 4096, 3, 34, -1, 0);", "addr");
    stops("  return raw.syscall(9, 0, 4096, 3, 34, 3, 0);", "fd");
    stops(
        "  return raw.syscall(9, 0, 4096, 3, 34, -1, 4096);",
        "offset",
    );
}

#[test]
fn partial_munmap_stops() {
    stops(
        "  let a = raw.syscall(9, 0, 8192, 3, 34, -1, 0);\n  return raw.syscall(11, a, 4096, 0, 0, 0, 0);",
        "完全一致",
    );
    stops(
        &format!("  let a = {MMAP};\n  return raw.syscall(11, a + 4096, 4096, 0, 0, 0, 0);"),
        "完全一致",
    );
}

#[test]
fn origin_mismatch_stops() {
    stops(
        &format!("  let a = {MMAP};\n  raw.free(raw.int_to_addr(a), 4096, 4096);\n  return 0;"),
        "mmap 由来",
    );
    stops(
        "  let p = raw.alloc(4096, 4096);\n  return raw.syscall(11, raw.addr_to_int(p), 4096, 0, 0, 0, 0);",
        "raw.alloc 由来",
    );
}

#[test]
fn double_munmap_stops() {
    stops(
        &format!("  let a = {MMAP};\n  raw.syscall(11, a, 4096, 0, 0, 0, 0);\n  return raw.syscall(11, a, 4096, 0, 0, 0, 0);"),
        "完全一致",
    );
}

#[test]
fn non_nul_terminated_path_stops() {
    stops(
        "  let p = raw.alloc(4, 1);\n  raw.set(p, 65, 4);\n  return raw.syscall(257, -100, raw.addr_to_int(p), 0, 0, 0, 0);",
        "NUL 終端",
    );
    stops(
        "  let p = raw.alloc(4, 1);\n  raw.set(p, 65, 4);\n  return raw.syscall(263, -100, raw.addr_to_int(p), 0, 0, 0, 0);",
        "NUL 終端",
    );
}

#[test]
fn statbuf_must_be_in_range() {
    stops(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(262, -100, raw.addr_to_int(p), raw.addr_to_int(p), 0, 0, 0);",
        "範囲外",
    );
}

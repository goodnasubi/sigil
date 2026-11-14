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

fn stat(path_alloc: usize, buf_alloc: usize, buf_off: usize) -> String {
    format!(
        "  let q = raw.alloc({path_alloc}, 16);\n  raw.set(q, 0, {path_alloc});\n  let s = raw.alloc({buf_alloc}, 16);\n  raw.set(s, 0, {buf_alloc});\n  return raw.syscall(262, -100, raw.addr_to_int(q), raw.addr_to_int(s) + {buf_off}, 0, 0, 0);"
    )
}

#[test]
fn munmap_larger_than_mmap_stops() {
    stops(
        &format!("  let a = {MMAP};\n  return raw.syscall(11, a, 8192, 0, 0, 0, 0);"),
        "完全一致",
    );
    stops(
        &format!("  let a = {MMAP};\n  return raw.syscall(11, a, 0, 0, 0, 0, 0);"),
        "完全一致",
    );
}

#[test]
fn statbuf_size_boundaries() {
    stops(&stat(16, 143, 0), "範囲外");
    assert!(run(&stat(16, 144, 0)).is_ok());
    // 末尾に 144B ちょうど / 1B はみ出し
    assert!(run(&stat(16, 160, 16)).is_ok());
    stops(&stat(16, 160, 17), "範囲外");
}

#[test]
fn newfstatat_path_checks() {
    stops(
        "  let s = raw.alloc(144, 16);\n  return raw.syscall(262, -100, 4096, raw.addr_to_int(s), 0, 0, 0);",
        "範囲外",
    );
    stops(
        "  let q = raw.alloc(4, 1);\n  raw.set(q, 65, 4);\n  let s = raw.alloc(144, 16);\n  return raw.syscall(262, -100, raw.addr_to_int(q), raw.addr_to_int(s), 0, 0, 0);",
        "NUL 終端",
    );
}

#[test]
fn mmap_len_bounds() {
    stops("  return raw.syscall(9, 0, 0, 3, 34, -1, 0);", "0 < len");
    stops("  return raw.syscall(9, 0, -1, 3, 34, -1, 0);", "0 < len");
    stops(
        "  return raw.syscall(9, 0, 70368744177665, 3, 34, -1, 0);",
        "0 < len",
    );
    // 2^46 ちょうどは検証を通る（実際の mmap は失敗してもよい）
    let r = run("  let a = raw.syscall(9, 0, 70368744177664, 3, 34, -1, 0);\n  if a >= 0 { raw.syscall(11, a, 70368744177664, 0, 0, 0, 0); }\n  return 0;");
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn read_write_negative_count_stops() {
    for n in [0, 1] {
        stops(
            &format!(
                "  let p = raw.alloc(8, 8);\n  return raw.syscall({n}, 1, raw.addr_to_int(p), -1, 0, 0, 0);"
            ),
            "count >= 0",
        );
    }
}

#[test]
fn mmap_prot_combinations_stop() {
    for prot in [0, 1, 2, 4, 5, 6, 7, 3 | 8, 3 | 0x1000000] {
        stops(
            &format!("  return raw.syscall(9, 0, 4096, {prot}, 34, -1, 0);"),
            "prot",
        );
    }
    stops(
        "  return raw.syscall(9, 0, 4096, 5, 34, -1, 0);",
        "PROT_EXEC",
    );
}

// ---------------------------------------------------------------------------
// ソケット系の行（ADR 0031 決定 3-4・判断点 D の案 B、PR 6-3e）。`std.os.net` が使う形だけを通す。

/// AF_INET・SOCK_STREAM|SOCK_NONBLOCK|SOCK_CLOEXEC のソケット。
const SOCK: &str = "raw.syscall(41, 2, 526337, 0, 0, 0, 0)";

/// 127.0.0.1:0 の sockaddr_in を `sa`（16 バイト）に組み立てる。
const SOCKADDR: &str = "  let sa = raw.alloc(16, 8);\n  raw.set(sa, 0, 16);\n  raw.store_u8(sa, 2);\n  raw.store_u8(raw.offset(sa, 4), 127);\n  raw.store_u8(raw.offset(sa, 7), 1);\n";

#[test]
fn normal_socket_bind_listen_getsockname_accept_close() {
    // 待ち受けを作り、番号を読み、誰も来ていない accept4 は EAGAIN（-11）。shutdown（SHUT_WR）は表を通る。
    let r = run(&format!(
        "  let fd = {SOCK};\n  if fd < 0 {{ return 0 - 1; }}\n  let one = raw.alloc(8, 8);\n  raw.store_i64(one, 1);\n  let r0 = raw.syscall(54, fd, 1, 2, raw.addr_to_int(one), 4, 0);\n  raw.free(one, 8, 8);\n{SOCKADDR}  let r1 = raw.syscall(49, fd, raw.addr_to_int(sa), 16, 0, 0, 0);\n  let r2 = raw.syscall(50, fd, 4, 0, 0, 0, 0);\n  let len = raw.alloc(8, 8);\n  raw.store_i64(len, 16);\n  let r3 = raw.syscall(51, fd, raw.addr_to_int(sa), raw.addr_to_int(len), 0, 0, 0);\n  let port = raw.load_u8(raw.offset(sa, 2)) * 256 + raw.load_u8(raw.offset(sa, 3));\n  let r4 = raw.syscall(288, fd, 0, 0, 526336, 0, 0);\n  let r5 = raw.syscall(48, fd, 1, 0, 0, 0, 0);\n  let r6 = raw.syscall(3, fd, 0, 0, 0, 0, 0);\n  raw.free(len, 8, 8);\n  raw.free(sa, 16, 8);\n  if port <= 0 {{ return 0 - 2; }}\n  if r0 != 0 || r1 != 0 || r2 != 0 || r3 != 0 || r6 != 0 {{ return 0 - 3; }}\n  if r5 > 0 {{ return 0 - 4; }}\n  return r4;"
    ));
    assert_eq!(r.unwrap(), "-11");
}

#[test]
fn socket_shape_is_restricted() {
    // ブロッキングのソケット、AF_UNIX、SOCK_DGRAM、protocol の指定は通さない。
    for call in [
        "raw.syscall(41, 2, 1, 0, 0, 0, 0)",
        "raw.syscall(41, 1, 526337, 0, 0, 0, 0)",
        "raw.syscall(41, 2, 526338, 0, 0, 0, 0)",
        "raw.syscall(41, 2, 526337, 6, 0, 0, 0)",
    ] {
        stops(&format!("  return {call};"), "socket");
    }
    // connect（42）は ADR 0034 決定 4 で追加。引数が不正なら拒否する。フラグのない accept（43）は表にない。
    stops("  return raw.syscall(42, 3, 0, 0, 0, 0, 0);", "addrlen");
    stops(
        "  return raw.syscall(43, 3, 0, 0, 0, 0, 0);",
        "許可表にない",
    );
}

#[test]
fn setsockopt_and_shutdown_shapes_are_restricted() {
    let opt = |level: i64, name: i64, len: i64| {
        format!(
            "  let one = raw.alloc(8, 8);\n  raw.store_i64(one, 1);\n  return raw.syscall(54, 3, {level}, {name}, raw.addr_to_int(one), {len}, 0);"
        )
    };
    stops(&opt(1, 9, 4), "setsockopt");
    stops(&opt(6, 2, 4), "setsockopt");
    stops(&opt(1, 2, 8), "setsockopt");
    stops(
        "  let one = raw.alloc(2, 1);\n  return raw.syscall(54, 3, 1, 2, raw.addr_to_int(one), 4, 0);",
        "範囲外",
    );
    stops("  return raw.syscall(48, 3, 2, 0, 0, 0, 0);", "shutdown");
    stops("  return raw.syscall(48, 3, 0, 0, 0, 0, 0);", "shutdown");
}

#[test]
fn bind_and_getsockname_addresses_must_be_in_range() {
    // bind: 長さは 16 だけ、領域は 16 バイトが生きていること、家族は AF_INET。
    stops(
        &format!("{SOCKADDR}  return raw.syscall(49, 3, raw.addr_to_int(sa), 8, 0, 0, 0);"),
        "bind",
    );
    stops(
        "  let sa = raw.alloc(8, 8);\n  raw.set(sa, 0, 8);\n  return raw.syscall(49, 3, raw.addr_to_int(sa), 16, 0, 0, 0);",
        "範囲外",
    );
    stops(
        "  let sa = raw.alloc(16, 8);\n  raw.set(sa, 0, 16);\n  raw.store_u8(sa, 1);\n  return raw.syscall(49, 3, raw.addr_to_int(sa), 16, 0, 0, 0);",
        "AF_INET",
    );
    // getsockname: カーネルが書く長さ（*addrlen）は、検査した 16 バイトを超えない。
    stops(
        &format!("{SOCKADDR}  let len = raw.alloc(8, 8);\n  raw.store_i64(len, 128);\n  return raw.syscall(51, 3, raw.addr_to_int(sa), raw.addr_to_int(len), 0, 0, 0);"),
        "getsockname",
    );
    stops(
        "  let sa = raw.alloc(8, 8);\n  let len = raw.alloc(8, 8);\n  raw.store_i64(len, 16);\n  return raw.syscall(51, 3, raw.addr_to_int(sa), raw.addr_to_int(len), 0, 0, 0);",
        "範囲外",
    );
    stops(
        &format!("{SOCKADDR}  return raw.syscall(51, 3, raw.addr_to_int(sa), 4096, 0, 0, 0);"),
        "範囲外",
    );
}

#[test]
fn accept4_takes_no_peer_address_and_requires_nonblocking() {
    stops(
        &format!("{SOCKADDR}  return raw.syscall(288, 3, raw.addr_to_int(sa), 0, 526336, 0, 0);"),
        "accept4",
    );
    stops(
        "  let len = raw.alloc(8, 8);\n  return raw.syscall(288, 3, 0, raw.addr_to_int(len), 526336, 0, 0);",
        "accept4",
    );
    stops("  return raw.syscall(288, 3, 0, 0, 0, 0, 0);", "accept4");
    stops("  return raw.syscall(288, 3, 0, 0, 2048, 0, 0);", "accept4");
}

#[test]
fn recvfrom_and_sendto_buffers_and_flags_are_checked() {
    // 領域の範囲。
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(45, 3, raw.addr_to_int(p), 9, 0, 0, 0);",
        "範囲外",
    );
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(44, 3, raw.addr_to_int(p), 9, 16384, 0, 0);",
        "範囲外",
    );
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(45, 3, raw.addr_to_int(p), 0 - 1, 0, 0, 0);",
        "recvfrom",
    );
    // フラグ: 読みは 0、書きは MSG_NOSIGNAL だけ（SIGPIPE でプロセスが落ちない形だけを通す）。
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(45, 3, raw.addr_to_int(p), 8, 2, 0, 0);",
        "recvfrom",
    );
    stops(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 3, raw.addr_to_int(p), 8, 0, 0, 0);",
        "sendto",
    );
    // 相手のアドレスは渡さない・受け取らない。
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(45, 3, raw.addr_to_int(p), 8, 0, raw.addr_to_int(p), 0);",
        "recvfrom",
    );
    stops(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 3, raw.addr_to_int(p), 8, 16384, raw.addr_to_int(p), 16);",
        "sendto",
    );
    // ソケットでない fd への正しい形の呼び出しは、カーネルの errno（ENOTSOCK = -88）がそのまま返る。
    let r = run(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  let r = raw.syscall(44, 1, raw.addr_to_int(p), 8, 16384, 0, 0);\n  raw.free(p, 8, 8);\n  return r;",
    );
    assert_eq!(r.unwrap(), "-88");
}

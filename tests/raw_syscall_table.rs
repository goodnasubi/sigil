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
    // ブロッキングのソケット、AF_UNIX、protocol の指定は通さない。非ブロッキングでない SOCK_DGRAM も通さない。
    // （AF_INET の SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC・protocol 0 は ADR 0057 で追加。下の UDP のテストで確かめる。）
    for call in [
        "raw.syscall(41, 2, 1, 0, 0, 0, 0)",
        "raw.syscall(41, 1, 526337, 0, 0, 0, 0)",
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
    // 宛先を渡す形は UDP の形（ADR 0057）。8 バイトしかない領域を 16 バイトの宛先として渡すと範囲外で止まる。
    stops(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 3, raw.addr_to_int(p), 8, 16384, raw.addr_to_int(p), 16);",
        "範囲外",
    );
    // ソケットでない fd への正しい形の呼び出しは、カーネルの errno（ENOTSOCK = -88）がそのまま返る。
    let r = run(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  let r = raw.syscall(44, 1, raw.addr_to_int(p), 8, 16384, 0, 0);\n  raw.free(p, 8, 8);\n  return r;",
    );
    assert_eq!(r.unwrap(), "-88");
}

#[test]
fn clock_gettime_realtime_restrictions_and_execution() {
    // CLOCK_REALTIME (0) 以外は拒否。
    stops(
        "  let ts = raw.alloc(16, 8);\n  return raw.syscall(228, 1, raw.addr_to_int(ts), 0, 0, 0, 0);",
        "clock_gettime",
    );
    // 領域不足は拒否。
    stops(
        "  let ts = raw.alloc(8, 8);\n  return raw.syscall(228, 0, raw.addr_to_int(ts), 0, 0, 0, 0);",
        "範囲外",
    );
    // 未使用引数非ゼロは拒否。
    stops(
        "  let ts = raw.alloc(16, 8);\n  return raw.syscall(228, 0, raw.addr_to_int(ts), 1, 0, 0, 0);",
        "clock_gettime",
    );
    // 正常実行: 0 が返り、秒が正の値。
    let r = run(
        "  let ts = raw.alloc(16, 8);\n  raw.set(ts, 0, 16);\n  let res = raw.syscall(228, 0, raw.addr_to_int(ts), 0, 0, 0, 0);\n  let sec = raw.load_i64(ts);\n  raw.free(ts, 16, 8);\n  if res == 0 && sec > 0 { return 1; } else { return 0; }",
    );
    assert_eq!(r.unwrap(), "1");
}

#[test]
fn flock_operation_and_unused_args_checked() {
    // 未使用引数非ゼロは拒否
    stops("  return raw.syscall(73, 0, 2, 1, 0, 0, 0);", "flock");
    // 不正な operation（16 など定義外のフラグ）
    stops("  return raw.syscall(73, 0, 16, 0, 0, 0, 0);", "flock");
    // 不正な operation（LOCK_NB だけ等、SH/EX/UN を含まない）
    stops("  return raw.syscall(73, 0, 4, 0, 0, 0, 0);", "flock");
    // 正常実行（無効 fd = -1 への LOCK_EX|LOCK_NB は EBADF = -9 を返す）
    let r = run("  return raw.syscall(73, -1, 6, 0, 0, 0, 0);");
    assert_eq!(r.unwrap(), "-9");
}

#[test]
fn mkdirat_path_and_unused_args_checked() {
    // 未使用引数非ゼロは拒否
    stops("  let p = raw.alloc(8, 1);\n  raw.store_u8(p, 0);\n  let r = raw.syscall(258, -100, raw.addr_to_int(p), 511, 1, 0, 0);\n  raw.free(p, 8, 1);\n  return r;", "mkdirat");
    // パス領域外アクセス・NUL終端なしは拒否
    stops("  let p = raw.alloc(4, 1);\n  raw.store_u8(p, 65);\n  raw.store_u8(raw.offset(p, 1), 66);\n  raw.store_u8(raw.offset(p, 2), 67);\n  raw.store_u8(raw.offset(p, 3), 68);\n  let r = raw.syscall(258, -100, raw.addr_to_int(p), 511, 0, 0, 0);\n  raw.free(p, 4, 1);\n  return r;", "パスが NUL 終端されていない");
}

#[test]
fn getdents64_buffer_and_args_checked() {
    // 未使用引数非ゼロは拒否
    stops("  let p = raw.alloc(64, 8);\n  let r = raw.syscall(217, 0, raw.addr_to_int(p), 64, 1, 0, 0);\n  raw.free(p, 64, 8);\n  return r;", "getdents64");
    // count 負数は拒否
    stops("  let p = raw.alloc(64, 8);\n  let r = raw.syscall(217, 0, raw.addr_to_int(p), -1, 0, 0, 0);\n  raw.free(p, 64, 8);\n  return r;", "getdents64");
    // count 範囲外（バッファサイズより大きい）
    stops("  let p = raw.alloc(32, 8);\n  let r = raw.syscall(217, 0, raw.addr_to_int(p), 64, 0, 0, 0);\n  raw.free(p, 32, 8);\n  return r;", "範囲外");
    // 正常実行（無効 fd = -1 への呼び出しは EBADF = -9 を返す）
    let r = run("  let p = raw.alloc(64, 8);\n  let r = raw.syscall(217, -1, raw.addr_to_int(p), 64, 0, 0, 0);\n  raw.free(p, 64, 8);\n  return r;");
    assert_eq!(r.unwrap(), "-9");
}

#[test]
fn fsync_and_fdatasync_take_only_an_fd() {
    // 不正な fd でも、許可表の検査を通れば EBADF（-9）で返る（停止ではない）。
    assert_eq!(
        run("  return raw.syscall(74, 999, 0, 0, 0, 0, 0);").unwrap(),
        "-9"
    );
    assert_eq!(
        run("  return raw.syscall(75, 999, 0, 0, 0, 0, 0);").unwrap(),
        "-9"
    );
    stops(
        "  return raw.syscall(74, 999, 1, 0, 0, 0, 0);",
        "未使用の引数",
    );
    stops(
        "  return raw.syscall(75, 999, 0, 0, 0, 0, 1);",
        "未使用の引数",
    );
}

#[test]
fn renameat_dirfds_unused_args_and_paths_checked() {
    let paths = "  let a = raw.alloc(3, 1);\n  raw.store_u8(a, 120);\n  raw.store_u8(raw.offset(a, 1), 49);\n  raw.store_u8(raw.offset(a, 2), 0);\n  let b = raw.alloc(3, 1);\n  raw.store_u8(b, 120);\n  raw.store_u8(raw.offset(b, 1), 50);\n  raw.store_u8(raw.offset(b, 2), 0);\n";
    // 存在しないパスの名前変更は、検査を通って ENOENT（-2）。
    let ok = format!(
        "{paths}  let r = raw.syscall(264, -100, raw.addr_to_int(a), -100, raw.addr_to_int(b), 0, 0);\n  raw.free(a, 3, 1);\n  raw.free(b, 3, 1);\n  return r;"
    );
    assert_eq!(run(&ok).unwrap(), "-2");
    for (args, needle) in [
        (
            "3, raw.addr_to_int(a), -100, raw.addr_to_int(b), 0, 0",
            "AT_FDCWD",
        ),
        (
            "-100, raw.addr_to_int(a), 3, raw.addr_to_int(b), 0, 0",
            "AT_FDCWD",
        ),
        (
            "-100, raw.addr_to_int(a), -100, raw.addr_to_int(b), 1, 0",
            "未使用の引数",
        ),
        (
            "-100, raw.addr_to_int(a), -100, raw.addr_to_int(b), 0, 1",
            "未使用の引数",
        ),
        ("-100, 4096, -100, raw.addr_to_int(b), 0, 0", "範囲外"),
        ("-100, raw.addr_to_int(a), -100, 4096, 0, 0", "範囲外"),
    ] {
        stops(
            &format!("{paths}  return raw.syscall(264, {args});"),
            needle,
        );
    }
}

#[test]
fn fstat_and_fchmod_checks() {
    // fstat(5)
    // 正常実行（無効 fd = 999 への呼び出しは EBADF = -9 を返す）
    let ok_stat = "  let p = raw.alloc(144, 8);\n  let r = raw.syscall(5, 999, raw.addr_to_int(p), 0, 0, 0, 0);\n  raw.free(p, 144, 8);\n  return r;";
    assert_eq!(run(ok_stat).unwrap(), "-9");
    // 未使用引数非ゼロ
    stops(
        "  let p = raw.alloc(144, 8);\n  let r = raw.syscall(5, 999, raw.addr_to_int(p), 1, 0, 0, 0);\n  raw.free(p, 144, 8);\n  return r;",
        "未使用の引数",
    );
    // バッファ範囲外
    stops(
        "  let p = raw.alloc(64, 8);\n  let r = raw.syscall(5, 999, raw.addr_to_int(p), 0, 0, 0, 0);\n  raw.free(p, 64, 8);\n  return r;",
        "範囲外",
    );

    // fchmod(91)
    // 正常実行（無効 fd = 999 への呼び出しは EBADF = -9 を返す）
    assert_eq!(
        run("  return raw.syscall(91, 999, 420, 0, 0, 0, 0);").unwrap(),
        "-9"
    );
    // 未使用引数非ゼロ
    stops(
        "  return raw.syscall(91, 999, 420, 1, 0, 0, 0);",
        "未使用の引数",
    );
    // mode 負数
    stops(
        "  return raw.syscall(91, 999, -1, 0, 0, 0, 0);",
        "mode は 0 以上 0o777 以下",
    );
    // mode 0o777 超 (512 = 0o1000)
    stops(
        "  return raw.syscall(91, 999, 512, 0, 0, 0, 0);",
        "mode は 0 以上 0o777 以下",
    );
}

#[test]
fn readlinkat_path_buffer_and_unused_args_checked() {
    let setup = "  let p = raw.alloc(4, 1);\n  raw.store_u8(p, 120);\n  raw.store_u8(raw.offset(p, 1), 0);\n  let b = raw.alloc(64, 1);\n";
    // 存在しないパスへの readlinkat(267) は検査を通って ENOENT (-2)
    let ok = format!(
        "{setup}  let r = raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 64, 0, 0);\n  raw.free(p, 4, 1);\n  raw.free(b, 64, 1);\n  return r;"
    );
    assert_eq!(run(&ok).unwrap(), "-2");

    // 未使用引数非ゼロ
    stops(
        &format!("{setup}  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 64, 1, 0);"),
        "未使用の引数",
    );
    stops(
        &format!("{setup}  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 64, 0, 1);"),
        "未使用の引数",
    );
    // bufsiz <= 0 または > 65536
    stops(
        &format!("{setup}  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 0, 0, 0);"),
        "bufsiz は 1 以上 65536 以下",
    );
    stops(
        &format!("{setup}  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), -1, 0, 0);"),
        "bufsiz は 1 以上 65536 以下",
    );
    stops(
        &format!("{setup}  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 65537, 0, 0);"),
        "bufsiz は 1 以上 65536 以下",
    );
    // バッファ範囲外
    stops(
        &format!("{setup}  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 65, 0, 0);"),
        "範囲外",
    );
    // パス NUL 終端なし
    stops(
        "  let p = raw.alloc(4, 1);\n  raw.set(p, 65, 4);\n  let b = raw.alloc(64, 1);\n  return raw.syscall(267, -100, raw.addr_to_int(p), raw.addr_to_int(b), 64, 0, 0);",
        "NUL 終端",
    );
}

// ---------------------------------------------------------------------------
// UDP（ADR 0057 決定 4、UDP 実装 PR 1）。TCP の形（相手のアドレスを渡さない）とは別の形だけを通す。
// ---------------------------------------------------------------------------

/// AF_INET・SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC・protocol 0。
const UDP_SOCK: &str = "raw.syscall(41, 2, 526338, 0, 0, 0, 0)";
const MSG_NOSIGNAL: i64 = 0x4000;
const MSG_TRUNC: i64 = 0x20;

/// AF_INET の宛先（16 バイト）を `dst` に作る（アドレスは 127.0.0.1、ポート 9）。
const DST: &str = "  let dst = raw.alloc(16, 8);\n  raw.set(dst, 0, 16);\n  raw.store_u8(dst, 2);\n  raw.store_u8(raw.offset(dst, 3), 9);\n  raw.store_u8(raw.offset(dst, 4), 127);\n  raw.store_u8(raw.offset(dst, 7), 1);\n";

/// 受信用の三領域（受信バッファ 64 バイト `buf`、送信元 16 バイト `src`、長さ 4 バイト `al` を 16 に初期化）。
const RECV_AREAS: &str = "  let buf = raw.alloc(64, 8);\n  raw.set(buf, 0, 64);\n  let src = raw.alloc(16, 8);\n  raw.set(src, 0, 16);\n  let al = raw.alloc(8, 8);\n  raw.store_i64(al, 16);\n";

/// 送信の呼び出し（`sendto(fd, buf, len, flags, dst, 16)`）。fd 1 は標準出力でソケットではなく、
/// 許可表を通った呼び出しはカーネルの errno（ENOTSOCK = -88）がそのまま返る。
fn udp_send(fd: i64, len: i64, flags: i64, dlen: i64) -> String {
    format!(
        "{DST}  let p = raw.alloc(16, 8);\n  raw.set(p, 0, 16);\n  return raw.syscall(44, {fd}, raw.addr_to_int(p), {len}, {flags}, raw.addr_to_int(dst), {dlen});"
    )
}

/// 受信の呼び出し（`recvfrom(fd, buf, len, flags, src, &al)`）。
fn udp_recv(fd: i64, len: i64, flags: i64) -> String {
    format!(
        "{RECV_AREAS}  return raw.syscall(45, {fd}, raw.addr_to_int(buf), {len}, {flags}, raw.addr_to_int(src), raw.addr_to_int(al));"
    )
}

#[test]
fn udp_socket_shape_is_allowed_and_others_stay_denied() {
    // 許す形: AF_INET の SOCK_DGRAM|SOCK_NONBLOCK|SOCK_CLOEXEC・protocol 0。fd が返る。
    let r = run(&format!(
        "  let fd = {UDP_SOCK};\n  if fd < 3 {{ return 0 - 1; }}\n  let c = raw.syscall(3, fd, 0, 0, 0, 0, 0);\n  return c;"
    ));
    assert_eq!(r.unwrap(), "0");
    // 拒否する形: AF_INET6 の DGRAM、SOCK_RAW、非ブロッキングでない DGRAM、AF_UNIX、protocol の指定、CLOEXEC なし。
    for call in [
        "raw.syscall(41, 10, 526338, 0, 0, 0, 0)",
        "raw.syscall(41, 2, 526339, 0, 0, 0, 0)",
        "raw.syscall(41, 2, 2, 0, 0, 0, 0)",
        "raw.syscall(41, 1, 526338, 0, 0, 0, 0)",
        "raw.syscall(41, 2, 526338, 17, 0, 0, 0)",
        "raw.syscall(41, 2, 2050, 0, 0, 0, 0)",
        "raw.syscall(41, 2, 526338 + 1048576, 0, 0, 0, 0)",
    ] {
        stops(&format!("  return {call};"), "socket");
    }
}

#[test]
fn udp_loopback_roundtrip_with_bind_getsockname_sendto_recvfrom() {
    // 実際に 127.0.0.1 へ送って受け取る。bind と getsockname は既存の IPv4 形のまま使える。
    // recvfrom は MSG_TRUNC で、返り値は元のデータグラムの長さ（5）。送信元のポートは bind したポート。
    let r = run(&format!(
        "  let fd = {UDP_SOCK};\n  if fd < 3 {{ return 0 - 1; }}\n{SOCKADDR}  let r1 = raw.syscall(49, fd, raw.addr_to_int(sa), 16, 0, 0, 0);\n  let len = raw.alloc(8, 8);\n  raw.store_i64(len, 16);\n  let r2 = raw.syscall(51, fd, raw.addr_to_int(sa), raw.addr_to_int(len), 0, 0, 0);\n  if r1 != 0 || r2 != 0 {{ return 0 - 2; }}\n  let port = raw.load_u8(raw.offset(sa, 2)) * 256 + raw.load_u8(raw.offset(sa, 3));\n  let pay = raw.alloc(8, 8);\n  raw.store_u8(pay, 7);\n  raw.store_u8(raw.offset(pay, 1), 8);\n  raw.store_u8(raw.offset(pay, 2), 9);\n  raw.store_u8(raw.offset(pay, 3), 10);\n  raw.store_u8(raw.offset(pay, 4), 11);\n  let s = raw.syscall(44, fd, raw.addr_to_int(pay), 5, 16384, raw.addr_to_int(sa), 16);\n{RECV_AREAS}  let n = raw.syscall(45, fd, raw.addr_to_int(buf), 64, 32, raw.addr_to_int(src), raw.addr_to_int(al));\n  let sport = raw.load_u8(raw.offset(src, 2)) * 256 + raw.load_u8(raw.offset(src, 3));\n  let a0 = raw.load_u8(buf);\n  let a4 = raw.load_u8(raw.offset(buf, 4));\n  let alv = raw.load_u8(al);\n  let c = raw.syscall(3, fd, 0, 0, 0, 0, 0);\n  if port <= 0 {{ return 0 - 3; }}\n  if s != 5 {{ return 0 - 4; }}\n  if sport != port {{ return 0 - 5; }}\n  if alv != 16 {{ return 0 - 6; }}\n  if c != 0 {{ return 0 - 7; }}\n  return n * 10000 + a0 * 100 + a4;"
    ));
    assert_eq!(r.unwrap(), "50711");
}

#[test]
fn udp_sendto_shape_is_checked() {
    // 許す形（ENOTSOCK = -88 が返れば、表と領域の検査を通っている）: 空、1 バイト、上限ちょうどの 65507 バイト。
    for len in [0, 1, 16] {
        assert_eq!(
            run(&udp_send(1, len, MSG_NOSIGNAL, 16)).unwrap(),
            "-88",
            "len={len}"
        );
    }
    let big = run(&format!(
        "{DST}  let p = raw.alloc(65507, 8);\n  raw.set(p, 0, 65507);\n  let r = raw.syscall(44, 1, raw.addr_to_int(p), 65507, 16384, raw.addr_to_int(dst), 16);\n  raw.free(p, 65507, 8);\n  return r;"
    ));
    assert_eq!(big.unwrap(), "-88");
    // 拒否: フラグ（0、MSG_NOSIGNAL 以外の組み合わせ）、宛先の長さ（0、8、28）、上限超過、負の長さ。
    for flags in [0, MSG_NOSIGNAL | 1, MSG_NOSIGNAL | MSG_TRUNC, MSG_TRUNC] {
        stops(&udp_send(1, 1, flags, 16), "sendto");
    }
    for dlen in [0, 8, 28] {
        stops(&udp_send(1, 1, MSG_NOSIGNAL, dlen), "sendto");
    }
    stops(&udp_send(1, 65508, MSG_NOSIGNAL, 16), "sendto");
    stops(&udp_send(1, -1, MSG_NOSIGNAL, 16), "sendto");
}

#[test]
fn udp_sendto_destination_family_and_ranges_are_checked() {
    // 家族が AF_INET でない宛先（AF_INET6 = 10、0）は拒否する。
    for fam in [10, 0, 1] {
        stops(
            &format!(
                "{DST}  raw.store_u8(dst, {fam});\n  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 1, raw.addr_to_int(p), 4, 16384, raw.addr_to_int(dst), 16);"
            ),
            "AF_INET",
        );
    }
    // 宛先が 16 バイトに満たない・解放済み・範囲外。
    stops(
        "  let dst = raw.alloc(8, 8);\n  raw.set(dst, 0, 8);\n  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 1, raw.addr_to_int(p), 4, 16384, raw.addr_to_int(dst), 16);",
        "範囲外",
    );
    stops(
        &format!(
            "{DST}  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  let d = raw.addr_to_int(dst);\n  raw.free(dst, 16, 8);\n  return raw.syscall(44, 1, raw.addr_to_int(p), 4, 16384, d, 16);"
        ),
        "範囲外",
    );
    stops(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 1, raw.addr_to_int(p), 4, 16384, 4096, 16);",
        "範囲外",
    );
    // 送信バッファが範囲外・解放済み。
    stops(
        &format!(
            "{DST}  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 1, raw.addr_to_int(p), 9, 16384, raw.addr_to_int(dst), 16);"
        ),
        "範囲外",
    );
    stops(
        &format!(
            "{DST}  let p = raw.alloc(8, 8);\n  let a = raw.addr_to_int(p);\n  raw.free(p, 8, 8);\n  return raw.syscall(44, 1, a, 4, 16384, raw.addr_to_int(dst), 16);"
        ),
        "範囲外",
    );
}

#[test]
fn udp_recvfrom_shape_is_checked() {
    // 許す形（ENOTSOCK = -88）: 1 バイト、上限ちょうどの 65507 バイト。
    assert_eq!(run(&udp_recv(1, 1, MSG_TRUNC)).unwrap(), "-88");
    let big = run(
        "  let buf = raw.alloc(65507, 8);\n  let src = raw.alloc(16, 8);\n  raw.set(src, 0, 16);\n  let al = raw.alloc(8, 8);\n  raw.store_i64(al, 16);\n  let r = raw.syscall(45, 1, raw.addr_to_int(buf), 65507, 32, raw.addr_to_int(src), raw.addr_to_int(al));\n  raw.free(buf, 65507, 8);\n  return r;",
    );
    assert_eq!(big.unwrap(), "-88");
    // 拒否: フラグ（0、MSG_TRUNC 以外、MSG_TRUNC との組み合わせ）、バッファの長さ（0、上限超過、負）。
    for flags in [0, 2, MSG_TRUNC | 2, MSG_NOSIGNAL] {
        stops(&udp_recv(1, 4, flags), "recvfrom");
    }
    for len in [0, 65508, -1] {
        stops(&udp_recv(1, len, MSG_TRUNC), "recvfrom");
    }
    // 長さ領域は 16 に初期化したものだけ（0、28、17）。
    for v in [0, 28, 17] {
        stops(
            &format!(
                "{RECV_AREAS}  raw.store_i64(al, {v});\n  return raw.syscall(45, 1, raw.addr_to_int(buf), 4, 32, raw.addr_to_int(src), raw.addr_to_int(al));"
            ),
            "addrlen",
        );
    }
    // 送信元の領域を渡さない形（NULL）は TCP の形で、フラグが MSG_TRUNC なら拒否する。
    stops(
        &format!(
            "{RECV_AREAS}  return raw.syscall(45, 1, raw.addr_to_int(buf), 4, 32, 0, raw.addr_to_int(al));"
        ),
        "recvfrom",
    );
}

#[test]
fn udp_recvfrom_regions_must_be_live_in_range_and_disjoint() {
    // 三領域それぞれの範囲外（小さすぎる）と解放済み。
    stops(
        "  let buf = raw.alloc(8, 8);\n  let src = raw.alloc(16, 8);\n  let al = raw.alloc(8, 8);\n  raw.store_i64(al, 16);\n  return raw.syscall(45, 1, raw.addr_to_int(buf), 9, 32, raw.addr_to_int(src), raw.addr_to_int(al));",
        "範囲外",
    );
    stops(
        "  let buf = raw.alloc(8, 8);\n  let src = raw.alloc(8, 8);\n  let al = raw.alloc(8, 8);\n  raw.store_i64(al, 16);\n  return raw.syscall(45, 1, raw.addr_to_int(buf), 8, 32, raw.addr_to_int(src), raw.addr_to_int(al));",
        "範囲外",
    );
    stops(
        "  let buf = raw.alloc(8, 8);\n  let src = raw.alloc(16, 8);\n  let al = raw.alloc(2, 1);\n  return raw.syscall(45, 1, raw.addr_to_int(buf), 8, 32, raw.addr_to_int(src), raw.addr_to_int(al));",
        "範囲外",
    );
    for freed in ["buf", "src", "al"] {
        stops(
            &format!(
                "{RECV_AREAS}  let b = raw.addr_to_int(buf);\n  let s = raw.addr_to_int(src);\n  let a = raw.addr_to_int(al);\n  raw.free({freed}, 64, 8);\n  return raw.syscall(45, 1, b, 4, 32, s, a);"
            )
            .replace("raw.free(src, 64, 8)", "raw.free(src, 16, 8)")
            .replace("raw.free(al, 64, 8)", "raw.free(al, 8, 8)"),
            "範囲外",
        );
    }
    // 範囲外の番地（小さい整数）。
    stops(
        &format!("{RECV_AREAS}  return raw.syscall(45, 1, raw.addr_to_int(buf), 4, 32, 4096, raw.addr_to_int(al));"),
        "範囲外",
    );
    // 非重複: 三領域が同じ塊の中で重なるときは、それぞれが範囲内で生きていても拒否する。
    // 各組は (受信バッファの位置, 長さ, 送信元の位置, 長さ領域の位置)。長さ領域には 16 を書いてから呼ぶ。
    let recv_in_big = |buf_off: i64, len: i64, src_off: i64, al_off: i64| {
        format!(
            "  let big = raw.alloc(256, 8);\n  raw.set(big, 0, 256);\n  raw.store_u8(raw.offset(big, {al_off}), 16);\n  return raw.syscall(45, 1, raw.addr_to_int(raw.offset(big, {buf_off})), {len}, 32, raw.addr_to_int(raw.offset(big, {src_off})), raw.addr_to_int(raw.offset(big, {al_off})));"
        )
    };
    for (buf_off, len, src_off, al_off) in [
        (0, 32, 0, 64),   // 送信元が受信バッファと同じ番地
        (0, 32, 16, 64),  // 送信元の先頭が受信バッファの内側
        (8, 4, 0, 64),    // 受信バッファが送信元の内側
        (0, 32, 64, 8),   // 長さ領域が受信バッファの内側
        (64, 8, 96, 100), // 長さ領域が送信元の内側
        (64, 8, 96, 64),  // 長さ領域が受信バッファの先頭と同じ
        (0, 33, 32, 100), // 受信バッファの末尾 1 バイトが送信元の先頭に入る
    ] {
        stops(&recv_in_big(buf_off, len, src_off, al_off), "重ならない");
    }
    // 隣り合うだけ（重ならない）なら通る。同じ塊に並べた 3 領域。
    assert_eq!(run(&recv_in_big(0, 32, 32, 48)).unwrap(), "-88");
    assert_eq!(run(&recv_in_big(100, 32, 0, 16)).unwrap(), "-88");
}

#[test]
fn tcp_forms_of_sendto_and_recvfrom_are_kept() {
    // 相手のアドレスを渡さない TCP の形は、UDP の追加後も同じ条件で通る（ENOTSOCK = -88）。
    let r = run(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  let a = raw.syscall(44, 1, raw.addr_to_int(p), 8, 16384, 0, 0);\n  let b = raw.syscall(45, 1, raw.addr_to_int(p), 8, 0, 0, 0);\n  raw.free(p, 8, 8);\n  return a * 1000 + b;",
    );
    assert_eq!(r.unwrap(), "-88088");
    // 宛先の長さだけを渡す、送信元の長さだけを渡す形（片方だけ）は TCP の形ではない。
    stops(
        "  let p = raw.alloc(8, 8);\n  raw.set(p, 0, 8);\n  return raw.syscall(44, 1, raw.addr_to_int(p), 8, 16384, 0, 16);",
        "sendto",
    );
    stops(
        "  let p = raw.alloc(8, 8);\n  return raw.syscall(45, 1, raw.addr_to_int(p), 8, 0, 0, 4096);",
        "recvfrom",
    );
}

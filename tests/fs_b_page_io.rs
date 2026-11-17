//! ADR 0049 / Issue #588: ページ I/O・WAL 追記。通常試験は SIR とネイティブを突き合わせる。
mod common;
use common::run_with;
use sigil::exec::{Arg, Options};

fn source(body: &str) -> String {
    format!("module app.fs_b\n  targets server\n  trust none\n\nfn t(path: String) -> Int\n  intent \"fs B test\"\n  requires none\n  ensures none\n  effects fs\n{{\n{body}\n}}\n")
}
fn check(tag: &str, body: &str) {
    let dir = common::native::scratch(tag);
    let path = dir.join("pages");
    let src = source(body);
    let got = run_with(
        &src,
        "t",
        &[Arg::Str(path.to_string_lossy().into())],
        &Options::default(),
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"));
    assert_eq!(got.value, "0");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn random_pages_do_not_move_stream_position() {
    check(
        "random_pages",
        r#"
    let p = Path.new(&path);
    let opts = fs.open_options().read(true).write(true).create(true).truncate(true);
    match opts.open(&p) {
      Err(_) => { return 1; }
      Ok(handle) => {
        let mut f = handle;
        let data = "abc".as_bytes();
        match f.write_all_at(&data, 16384) { Err(_) => { return 2; } Ok(_) => {} }
        match f.write_at(&data, 8192) { Err(_) => { return 3; } Ok(n) => { if n != 3 { return 4; } } }
        match f.write_all_at(&data, 0) { Err(_) => { return 5; } Ok(_) => {} }
        let mut buf = "xxx".as_bytes();
        match f.read_exact_at(&mut buf, 8192) { Err(_) => { return 6; } Ok(_) => {} }
        if buf != data { return 7; }
        match f.read_at(&mut buf, 16384) { Err(_) => { return 8; } Ok(n) => { if n != 3 { return 9; } } }
        if buf != data { return 10; }
        match f.read(&mut buf) { Err(_) => { return 11; } Ok(n) => { if n != 3 { return 12; } } }
        if buf != data { return 13; }
        match f.read_at(&mut buf, 4) { Err(_) => { return 14; } Ok(n) => { if n != 3 { return 15; } } }
        if buf.get(0).unwrap_or(99) != 0 { return 16; }
        let _ = f.close();
      }
    }
    return 0;
    "#,
    );
}

#[test]
fn append_handles_reject_positioned_writes_even_empty() {
    check(
        "append",
        r#"
    let p = Path.new(&path);
    let init = fs.open_options().write(true).create(true).truncate(true);
    match init.open(&p) { Err(_) => { return 1; } Ok(f) => { let _ = f.close(); } }
    let opts = fs.open_options().read(true).append(true);
    match opts.open(&p) {
      Err(_) => { return 2; }
      Ok(handle_a) => {
        let mut a = handle_a;
        match opts.open(&p) {
          Err(_) => { return 3; }
          Ok(handle_b) => {
            let mut b = handle_b;
            let one = "a".as_bytes();
            let two = "b".as_bytes();
            match a.write(&one) { Err(_) => { return 4; } Ok(n) => { if n != 1 { return 5; } } }
            match b.write_all(&two) { Err(_) => { return 6; } Ok(_) => {} }
            match a.write_all(&one) { Err(_) => { return 7; } Ok(_) => {} }
            let empty = Bytes.new();
            match a.write_at(&one, 0) { Err(IoError.InvalidInput(_)) => {} _ => { return 8; } }
            match a.write_all_at(&one, 0) { Err(IoError.InvalidInput(_)) => {} _ => { return 9; } }
            match a.write_at(&empty, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 10; } }
            match a.write_all_at(&empty, 0) { Err(IoError.InvalidInput(_)) => {} _ => { return 11; } }
            let mut buf = "xxx".as_bytes();
            match a.read_exact_at(&mut buf, 0) { Err(_) => { return 12; } Ok(_) => {} }
            if buf != "aba".as_bytes() { return 13; }
            match a.read_at(&mut buf, 3) { Err(_) => { return 14; } Ok(n) => { if n != 0 { return 15; } } }
            let _ = b.close();
          }
        }
        let _ = a.close();
      }
    }
    // append(true) だけでも書き込み権限を付ける。
    let only = fs.open_options().append(true);
    match only.open(&p) { Err(_) => { return 16; } Ok(handle) => {
        let mut f = handle;
      match f.write_all(&"c".as_bytes()) { Err(_) => { return 17; } Ok(_) => {} }
      let _ = f.close();
    } }
    match fs.read(&p) { Err(_) => { return 18; } Ok(s) => { if s != "abac" { return 19; } } }
    return 0;
    "#,
    );
}

#[test]
fn eof_short_read_and_bounds_preserve_buffer() {
    check(
        "eof_bounds",
        r#"
    let p = Path.new(&path);
    let opts = fs.open_options().read(true).write(true).create(true).truncate(true);
    match opts.open(&p) {
      Err(_) => { return 1; }
      Ok(f) => {
        let data = "ab".as_bytes();
        match f.write_all_at(&data, 0) { Err(_) => { return 2; } Ok(_) => {} }
        let mut buf = "xxxx".as_bytes();
        match f.read_at(&mut buf, 0) { Err(_) => { return 3; } Ok(n) => { if n != 2 { return 4; } } }
        if buf != "abxx".as_bytes() { return 5; }
        match f.read_exact_at(&mut buf, 0) { Err(IoError.Other(_)) => {} _ => { return 6; } }
        if buf != "abxx".as_bytes() { return 7; }
        match f.read_at(&mut buf, 2) { Err(_) => { return 8; } Ok(n) => { if n != 0 { return 9; } } }
        let mut empty = Bytes.new();
        match f.read_exact_at(&mut empty, 9223372036854775807) { Err(_) => { return 10; } Ok(_) => {} }
        match f.write_all_at(&empty, 9223372036854775807) { Err(_) => { return 11; } Ok(_) => {} }
        match f.read_at(&mut empty, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 12; } }
        match f.write_at(&data, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 13; } }
        match f.write_all_at(&data, 9223372036854775806) { Err(IoError.LimitExceeded(_)) => {} _ => { return 14; } }
        match f.read_exact_at(&mut buf, 9223372036854775804) { Err(IoError.LimitExceeded(_)) => {} _ => { return 15; } }
        match f.read_exact_at(&mut empty, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 16; } }
        match f.write_all_at(&empty, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 17; } }
        match f.read_at(&mut buf, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 18; } }
        match f.read_exact_at(&mut buf, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 19; } }
        match f.write_at(&empty, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 20; } }
        match f.write_all_at(&data, -1) { Err(IoError.InvalidInput(_)) => {} _ => { return 21; } }
        let _ = f.close();
      }
    }
    return 0;
    "#,
    );
}

#[test]
fn fs_double_handles_stay_fail_closed() {
    let src = source(
        r#"
      let p = Path.new(&path);
      let opts = fs.open_options().read(true).append(true);
      match opts.open(&p) { Err(IoError.Other(_)) => { return 0; } _ => { return 1; } }
    "#,
    );
    for bytes in [false, true] {
        let mut opts = Options::default();
        if bytes {
            opts.fs_bytes = Some(std::collections::BTreeMap::from([(
                "/virtual/pages".into(),
                vec![1],
            )]));
        } else {
            opts.fs_files = Some(std::collections::BTreeMap::from([(
                "/virtual/pages".into(),
                "a".into(),
            )]));
        }
        assert_eq!(
            sigil::ir_eval::run_with(&src, "t", &[Arg::Str("/virtual/pages".into())], &opts)
                .unwrap()
                .value,
            "0"
        );
    }
}

/// 本番 file_io の syscall 発行点だけを差し替える。新 raw や本番注入 API は追加しない。
#[test]
fn injected_eintr_short_io_zero_and_errors_use_production_loop() {
    let production = include_str!("../src/stdlib/os_fs.sigil");
    let needle =
        "raw.syscall(op, fd, raw.addr_to_int(raw.offset(buf, done)), len - done, at, 0, 0)";
    assert_eq!(production.matches(needle).count(), 1);
    let injected = production.replace(
        needle,
        "injected_call(op, fd, raw.addr_to_int(raw.offset(buf, done)), len - done, at)",
    );
    let mock = r#"
fn injected_call(op: Int, fd: Int, bp: Int, count: Int, offset: Int) -> Int
  intent "EINTR、2バイトshort、EOF、容量不足を順に注入し、残量とoffsetを検証する"
  requires none
  ensures none
  effects fs
  unchecked "fdは試験が所有する64バイト領域の番地、bpは生存する6バイトbuffer内。コピーは残量内の2バイト以下"
{
  // fs効果を本番と同じにする。無効fdへのゼロ長readは外部状態を変更しない。
  let _ = raw.syscall(0, -1, 0, 0, 0, 0, 0);
  let state = raw.int_to_addr(fd);
  let calls = raw.load_i64(state);
  raw.store_i64(state, calls + 1);
  let progress = raw.load_i64(raw.offset(state, 8));
  let mode = raw.load_i64(raw.offset(state, 16));
  if count != 6 - progress { return -22; }
  if offset != 8192 + progress { return -22; }
  if calls == 0 { return -4; }
  if mode == 1 && progress == 2 { return 0; }
  if mode == 2 && progress == 2 { return -28; }
  if mode == 3 { return count + 1; }
  let buffer = raw.int_to_addr(bp);
  if op == 17 {
    let mut i = 0;
    while i < 2 {
      raw.store_u8(raw.offset(buffer, i), raw.load_u8(raw.offset(state, 32 + progress + i)));
      i = i + 1;
    }
  } else {
    if op != 18 { return -22; }
    let mut i = 0;
    while i < 2 {
      raw.store_u8(raw.offset(state, 48 + progress + i), raw.load_u8(raw.offset(buffer, i)));
      i = i + 1;
    }
  }
  raw.store_i64(raw.offset(state, 8), progress + 2);
  return 2;
}

pub fn injected_test() -> Int
  intent "本番完全充足ループと境界検査を故障注入で確かめる"
  requires none
  ensures none
  effects fs
  unchecked "stateを64/8、bufferを6/1で確保し全試験後に同じサイズで解放する。state内フィールドは8バイト境界"
{
  if !check_io_range(67108864, 0) { return 1; }
  if check_io_range(67108865, 0) { return 2; }
  if check_io_range(-1, 0) { return 3; }
  if !check_io_range(0, 9223372036854775807) { return 4; }
  if check_io_range(1, 9223372036854775807) { return 5; }
  let state = raw.alloc(64, 8);
  let buffer = raw.alloc(6, 1);
  let mut bad = 0;
  let mut direction = 0;
  while direction < 2 {
    let mut mode = 0;
    while mode < 4 {
      raw.store_i64(state, 0);
      raw.store_i64(raw.offset(state, 8), 0);
      raw.store_i64(raw.offset(state, 16), mode);
      let mut i = 0;
      while i < 6 {
        raw.store_u8(raw.offset(buffer, i), 10 + i);
        raw.store_u8(raw.offset(state, 32 + i), 10 + i);
        raw.store_u8(raw.offset(state, 48 + i), 0);
        i = i + 1;
      }
      if direction == 0 { raw.set(buffer, 0, 6); }
      let n = file_io(raw.addr_to_int(state), buffer, 6, 8192, direction == 0, true, true);
      if mode == 0 {
        if n != 6 || raw.load_i64(state) != 4 { bad = bad + 1; }
        i = 0;
        while i < 6 {
          let mut byte = raw.load_u8(raw.offset(buffer, i));
          if direction == 1 { byte = raw.load_u8(raw.offset(state, 48 + i)); }
          if byte != 10 + i { bad = bad + 1; }
          i = i + 1;
        }
      }
      if mode == 1 && n != -5 { bad = bad + 1; }
      if mode == 2 && n != -28 { bad = bad + 1; }
      if mode == 3 && n != -5 { bad = bad + 1; }
      mode = mode + 1;
    }
    direction = direction + 1;
  }
  raw.free(buffer, 6, 1);
  raw.free(state, 64, 8);
  return bad;
}
"#;
    let c = sigil::effects::Options {
        embedded_extra: vec![("std.os.fs".into(), format!("{injected}\n{mock}"))],
        embedded_roots: vec!["std.os.fs".into()],
        ..Default::default()
    };
    let user = source("return 0;").replace("effects fs", "effects none");
    let opts = Options {
        embedded_extra: c.embedded_extra.clone(),
        embedded_roots: c.embedded_roots.clone(),
        ..Options::default()
    };
    let entry = "std.os.fs.injected_test";
    assert_eq!(
        sigil::ir_eval::run_with(&user, entry, &[], &opts)
            .unwrap()
            .value,
        "0"
    );
    let program = sigil::ir_eval::lower_sources_opts(&[&user], entry, &c).unwrap();
    sigil::verify::verify(&program).unwrap();
    let dir = common::native::scratch("fs-b-injected");
    let exe = dir.join("test");
    sigil::native::build_program(&program, &sigil::codegen::Opts::default(), &exe).unwrap();
    let got = common::native::exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout.trim(), "0", "{}", got.stderr);
    assert!(got.stderr.contains("sigil_blocks=0"), "{}", got.stderr);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn positioned_syscall_allowlist_checks_before_issuing() {
    let tail = "  intent \"raw test\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"検査拒否を試す\"\n";
    for n in [17, 18] {
        for (body, needle) in [
            (format!("return raw.syscall({n}, -1, 0, -1, 0, 0, 0);"), "count >= 0"),
            (format!("return raw.syscall({n}, -1, 0, 0, -1, 0, 0);"), "offset >= 0"),
            (format!("return raw.syscall({n}, -1, 4096, 1, 0, 0, 0);"), "範囲外"),
            (format!("let p = raw.alloc(8, 8); return raw.syscall({n}, -1, raw.addr_to_int(p), 9, 0, 0, 0);"), "範囲外"),
        ] {
            let src = format!("module app.raw_test\n  targets server\n  trust base\n\nfn t() -> Int\n{tail}{{ {body} }}\n");
            let opts = Options { trust_base: vec!["app.raw_test".into()], ..Options::default() };
            let error = sigil::ir_eval::run_with(&src, "t", &[], &opts).unwrap_err().to_string();
            assert!(error.contains(needle), "{error}");
        }
        // ゼロ長なら番地を参照しない。許可済み syscall が実際に EBADF を返す。
        let src = format!("module app.raw_test\n  targets server\n  trust base\n\nfn t() -> Int\n{tail}{{ return raw.syscall({n}, -1, 0, 0, 0, 0, 0); }}\n");
        let opts = Options {
            trust_base: vec!["app.raw_test".into()],
            ..Options::default()
        };
        assert_eq!(
            sigil::ir_eval::run_with(&src, "t", &[], &opts)
                .unwrap()
                .value,
            "-9"
        );
    }
}

#[test]
fn full_8192_byte_pages_in_shuffled_order() {
    check(
        "full_pages",
        r#"
    let p = Path.new(&path);
    let opts = fs.open_options().read(true).write(true).create(true).truncate(true);
    match opts.open(&p) {
      Err(_) => { return 1; }
      Ok(f) => {
        let mut page = Bytes.new();
        let mut i = 0;
        while i < 8192 { page.push(i % 256); i = i + 1; }
        let mut turn = 0;
        while turn < 4 {
          let slot = (turn * 3 + 2) % 4;
          let _ = page.set(0, slot);
          match f.write_all_at(&page, slot * 8192) { Err(_) => { return 2; } Ok(_) => {} }
          turn = turn + 1;
        }
        let mut buf = Bytes.new();
        i = 0;
        while i < 8192 { buf.push(0); i = i + 1; }
        turn = 0;
        while turn < 4 {
          let slot = (turn * 3 + 1) % 4;
          let _ = page.set(0, slot);
          match f.read_exact_at(&mut buf, slot * 8192) { Err(_) => { return 3; } Ok(_) => {} }
          if buf != page { return 4; }
          turn = turn + 1;
        }
        let _ = f.close();
      }
    }
    return 0;
    "#,
    );
}

//! ADR 0030 決定 3 / PR 5-2f: `epoll` と時間の待機（I/O スレッド、`EPOLLONESHOT`、期限の最小ヒープ、`clock_gettime`）。
//!
//! 評価器は I/O スレッドを動かせない（単一スレッド・`raw.syscall` の許可表が狭い）ので、ネイティブだけで試験する。
//! 駆動のモジュール（`std.os.t`）が `std.os.io` を呼び、`std.os.thread` の本体 `body` を `std.os.io.io_loop` に
//! 差し替えた版で I/O スレッドを作る（5-2c の `thread_5_2c.rs` と同じ作り）。待つ相手は `pipe2` の記述子と実時間の
//! `sleep`。駆動の `run` は、最初に失敗した確認の番号（成功は 0）を返す。
mod common;

use common::native::{scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::stdlib::embedded::{EMBEDDED, IO_MODULE, THREAD_MODULE};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// `std.os.thread` のソースの `body` を、`std.os.io.io_loop` を呼ぶ版に差し替えたもの。
fn thread_source() -> String {
    let src = EMBEDDED
        .iter()
        .find(|(n, _)| *n == THREAD_MODULE)
        .map(|(_, s)| *s)
        .expect("std.os.thread が登録されている");
    let a = src.find("fn body(arg: Addr)").expect("body がある");
    let end = src[a..].find("\n}\n").expect("body の終わり") + a + 3;
    let body = format!(
        "fn body(arg: Addr)\n{TAIL}  unchecked \"試験用の本体。引数は io_new が返した状態\"\n{{\n  io_loop(raw.addr_to_int(arg));\n}}\n"
    );
    let h = src.find("global mut ENTRY").expect("大域がある");
    format!(
        "{}use std.os.io.io_loop;\n\n{}{}{}",
        &src[..h],
        &src[h..a],
        body,
        &src[end..]
    )
}

/// 駆動のモジュール。補助の関数（パイプ・書き込み・読み出し・眠り・記述子の確認）と `run`。
fn driver(run_body: &str, extra: &[&str]) -> String {
    let mut f = String::new();
    let mut add = |sig: &str, body: &str| {
        f.push_str(&format!(
            "fn {sig}\n{TAIL}  unchecked \"試験の補助。自分で作った記述子と領域だけを触る\"\n{{\n{body}\n}}\n\n"
        ));
    };
    add(
        "pipe_new() -> Int",
        "  let b = raw.alloc(8, 8);\n  raw.store_i64(b, 0);\n  let r = raw.syscall(293, raw.addr_to_int(b), 526336, 0, 0, 0, 0);\n  let v = raw.load_i64(b);\n  raw.free(b, 8, 8);\n  if r < 0 { return 0 - 1; }\n  return (v & 4294967295) * 65536 + (v >> 32);",
    );
    add(
        "wr(fd: Int)",
        "  let b = raw.alloc(8, 8);\n  raw.store_i64(b, 90);\n  raw.syscall(1, fd, raw.addr_to_int(b), 1, 0, 0, 0);\n  raw.free(b, 8, 8);",
    );
    add(
        "rd(fd: Int)",
        "  let b = raw.alloc(8, 8);\n  raw.syscall(0, fd, raw.addr_to_int(b), 1, 0, 0, 0);\n  raw.free(b, 8, 8);",
    );
    add("fd_close(fd: Int)", "  raw.syscall(3, fd, 0, 0, 0, 0, 0);");
    add(
        "fd_open(fd: Int) -> Int",
        "  if raw.syscall(72, fd, 1, 0, 0, 0, 0) >= 0 { return 1; }\n  return 0;",
    );
    add(
        "sleep_ms(ms: Int)",
        "  let b = raw.alloc(16, 8);\n  raw.store_i64(b, ms / 1000);\n  raw.store_i64(raw.offset(b, 8), (ms % 1000) * 1000000);\n  raw.syscall(35, raw.addr_to_int(b), 0, 0, 0, 0, 0);\n  raw.free(b, 16, 8);",
    );
    add(
        "buf() -> Int",
        "  return raw.addr_to_int(raw.alloc(32, 8));",
    );
    add("buf_free(p: Int)", "  raw.free(raw.int_to_addr(p), 32, 8);");
    add(
        "word(p: Int, i: Int) -> Int",
        "  return raw.load_i64(raw.int_to_addr(p + 8 * i));",
    );
    add(
        "start(s: Int) -> Addr",
        "  return spawn(raw.int_to_addr(s));",
    );
    // 使わない `use` はエラーなので、本体に現れる名前だけを取り込む（`use` は文字の順）。
    let names = [
        "std.os.alloc.live_blocks",
        "std.os.io.io_cancel",
        "std.os.io.io_count",
        "std.os.io.io_forget_fd",
        "std.os.io.io_free",
        "std.os.io.io_new",
        "std.os.io.io_now",
        "std.os.io.io_sleep",
        "std.os.io.io_sleep_until",
        "std.os.io.io_stop",
        "std.os.io.io_take",
        "std.os.io.io_take_wait",
        "std.os.io.io_wait_fd",
        "std.os.thread.join",
        "std.os.thread.spawn",
    ];
    let mut all: Vec<&str> = names.iter().chain(extra.iter()).copied().collect();
    all.sort_unstable();
    let uses: String = all
        .iter()
        .filter(|n| {
            let short = n.rsplit('.').next().unwrap();
            let call = format!("{short}(");
            let has = |t: &str| {
                t.match_indices(&call)
                    .any(|(i, _)| !t[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_'))
            };
            has(run_body) || has(&f)
        })
        .map(|n| format!("use {n};\n"))
        .collect();
    format!(
        "module std.os.t\n  targets server\n  trust base\n\n{uses}\n{f}pub fn run() -> Int\n{TAIL}  unchecked \"試験\"\n{{\n{run_body}\n}}\n"
    )
}

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n")
}

/// 制限時間内に終わらなければ `None`（ハング）。
fn run_exe(exe: &std::path::Path, limit: std::time::Duration) -> Option<Ran> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .env("SIGIL_RT_LIVE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut v = vec![];
        let _ = out.read_to_end(&mut v);
        v
    });
    let t_err = std::thread::spawn(move || {
        let mut v = vec![];
        let _ = err.read_to_end(&mut v);
        v
    });
    let start = std::time::Instant::now();
    let status = loop {
        if let Some(st) = child.try_wait().unwrap() {
            break st;
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            let _ = t_out.join();
            let _ = t_err.join();
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap()).into_owned();
    use std::os::unix::process::ExitStatusExt;
    Some(Ran {
        stdout,
        stderr,
        code: status.code().or(status.signal().map(|s| 128 + s)),
    })
}

/// `std.os.io` のソースを `io_src` に差し替えて走らせる（ミューテーションの確認で使う）。
fn native_with_io(io_src: &str, run_body: &str) -> Option<Ran> {
    native_with_io_uses(io_src, run_body, &[])
}

fn native_with_io_uses(io_src: &str, run_body: &str, extra: &[&str]) -> Option<Ran> {
    let c = COpts {
        embedded_extra: vec![
            ("std.os.t".to_string(), driver(run_body, extra)),
            (THREAD_MODULE.to_string(), thread_source()),
            (IO_MODULE.to_string(), io_src.to_string()),
        ],
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    };
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("epoll52f");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = run_exe(&exe, std::time::Duration::from_secs(20));
    let _ = std::fs::remove_dir_all(&dir);
    got
}

fn io_source() -> String {
    EMBEDDED
        .iter()
        .find(|(n, _)| *n == IO_MODULE)
        .map(|(_, s)| s.to_string())
        .expect("std.os.io が登録されている")
}

fn native(run_body: &str) -> Ran {
    native_with_io(&io_source(), run_body).expect("制限時間内に終わらなかった（ハング）")
}

fn value(r: &Ran) -> String {
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    r.stdout.trim_end().rsplit('\n').next().unwrap().to_string()
}

/// 最初に失敗した確認の番号（成功は 0）。
fn step(r: &Ran) -> String {
    value(r)
}

const ONESHOT_BODY: &str = r#"
  let live0 = live_blocks();
  let s = io_new();
  if s == 0 { return 1; }
  let h = start(s);
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let out = buf();
  if io_wait_fd(s, r, 1, 7) != 0 { return 2; }
  sleep_ms(60);
  if io_take(s, out) != 0 { return 3; }
  wr(w);
  if io_take_wait(s, out) != 1 { return 4; }
  if word(out, 0) != 7 { return 5; }
  if word(out, 1) != 1 { return 6; }
  if word(out, 2) % 2 != 1 { return 7; }
  sleep_ms(150);
  if io_take(s, out) != 0 { return 8; }
  if io_wait_fd(s, r, 1, 8) != 0 { return 9; }
  if io_take_wait(s, out) != 1 { return 10; }
  if word(out, 0) != 8 { return 11; }
  rd(r);
  if io_wait_fd(s, r, 1, 9) != 0 { return 12; }
  sleep_ms(100);
  if io_take(s, out) != 0 { return 13; }
  if io_count(s, 1) != 1 { return 14; }
  if io_forget_fd(s, r) != 1 { return 15; }
  if io_count(s, 1) != 0 { return 16; }
  io_stop(s);
  join(h);
  io_free(s);
  fd_close(r);
  fd_close(w);
  buf_free(out);
  if live_blocks() != live0 { return 17; }
  return 0;
"#;

#[test]
fn a_ready_fd_wakes_once_and_is_rearmed_with_oneshot() {
    // 準備完了の通知でトークンが完了の列に積まれ、データを読まないままでも 2 度目は来ない（EPOLLONESHOT）。
    // 登録し直す（EPOLL_CTL_MOD）と、すでに読める記述子はすぐ知らせる。データが無ければ知らせない。
    assert_eq!(step(&native(ONESHOT_BODY)), "0");
}

const TIMER_BODY: &str = r#"
  let live0 = live_blocks();
  let s = io_new();
  if s == 0 { return 1; }
  let h = start(s);
  let out = buf();
  let d3 = io_sleep(s, 300, 3);
  let d1 = io_sleep(s, 100, 1);
  let d2 = io_sleep(s, 200, 2);
  if io_count(s, 0) != 3 { return 2; }
  if io_take_wait(s, out) != 1 { return 3; }
  if word(out, 0) != 1 { return 4; }
  if word(out, 1) != 2 { return 5; }
  if word(out, 3) < d1 { return 6; }
  if io_take_wait(s, out) != 1 { return 7; }
  if word(out, 0) != 2 { return 8; }
  if word(out, 3) < d2 { return 9; }
  if io_take_wait(s, out) != 1 { return 10; }
  if word(out, 0) != 3 { return 11; }
  if word(out, 3) < d3 { return 12; }
  if io_count(s, 0) != 0 { return 13; }
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  let d5 = io_sleep(s, 400, 5);
  if io_wait_fd(s, r, 1, 6) != 0 { return 14; }
  sleep_ms(50);
  wr(w);
  if io_take_wait(s, out) != 1 { return 15; }
  if word(out, 0) != 6 { return 16; }
  if io_now() >= d5 { return 17; }
  if io_take(s, out) != 0 { return 18; }
  if io_count(s, 0) != 1 { return 19; }
  if io_take_wait(s, out) != 1 { return 20; }
  if word(out, 0) != 5 { return 21; }
  if word(out, 1) != 2 { return 22; }
  if word(out, 3) < d5 { return 23; }
  io_stop(s);
  join(h);
  io_free(s);
  fd_close(r);
  fd_close(w);
  buf_free(out);
  if live_blocks() != live0 { return 24; }
  return 0;
"#;

#[test]
fn timers_fire_in_deadline_order_and_never_early() {
    // 300・100・200 ミリ秒の順に足した期限が、昇順に起きる。時刻は期限以上。別の記述子の通知でスレッドが期限の前に
    // 起きても、期限のものは起こさない（期限の一覧に残り、届いたときに起きる）。
    assert_eq!(step(&native(TIMER_BODY)), "0");
}

const HEAP_BODY: &str = r#"
  let s = io_new();
  if s == 0 { return 1; }
  let h = start(s);
  let out = buf();
  let base = io_now() + 150000000;
  let mut i = 1;
  while i <= 500 {
    io_sleep_until(s, base + ((i * 7919) % 997) * 100000, i);
    i = i + 1;
  }
  let mut prev = 0;
  let mut n = 0;
  while n < 500 {
    if io_take_wait(s, out) != 1 { return 2; }
    let t = word(out, 0);
    let d = base + ((t * 7919) % 997) * 100000;
    if d < prev { return 3; }
    if word(out, 3) < d { return 4; }
    prev = d;
    n = n + 1;
  }
  if io_count(s, 0) != 0 { return 5; }
  if io_take(s, out) != 0 { return 6; }
  io_stop(s);
  join(h);
  io_free(s);
  buf_free(out);
  return 0;
"#;

#[test]
fn five_hundred_timers_come_out_sorted() {
    // 500 個の期限を乱れた順に足す（ヒープの容量が 8 から増える）。取り出す順は期限の昇順で、時刻は期限以上。
    assert_eq!(step(&native(HEAP_BODY)), "0");
}

const CANCEL_BODY: &str = r#"
  let live0 = live_blocks();
  let s = io_new();
  if s == 0 { return 1; }
  let h = start(s);
  let out = buf();
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  if io_wait_fd(s, r, 1, 20) != 0 { return 2; }
  io_sleep(s, 20000, 21);
  let d22 = io_sleep(s, 150, 22);
  if io_count(s, 0) != 2 { return 3; }
  if io_count(s, 1) != 1 { return 4; }
  if io_cancel(s, 21) != 1 { return 5; }
  if io_count(s, 0) != 1 { return 6; }
  if io_cancel(s, 20) != 1 { return 7; }
  if io_count(s, 1) != 0 { return 8; }
  if io_cancel(s, 20) != 0 { return 9; }
  wr(w);
  if io_take_wait(s, out) != 1 { return 10; }
  if word(out, 0) != 22 { return 11; }
  if word(out, 3) < d22 { return 12; }
  sleep_ms(100);
  if io_take(s, out) != 0 { return 13; }
  rd(r);
  if io_wait_fd(s, r, 1, 23) != 0 { return 14; }
  wr(w);
  sleep_ms(100);
  if io_count(s, 2) != 1 { return 15; }
  if io_cancel(s, 23) != 2 { return 16; }
  if io_count(s, 2) != 0 { return 17; }
  if io_take(s, out) != 0 { return 18; }
  if io_count(s, 1) != 0 { return 19; }
  io_stop(s);
  join(h);
  io_free(s);
  fd_close(r);
  fd_close(w);
  buf_free(out);
  if live_blocks() != live0 { return 20; }
  return 0;
"#;

#[test]
fn a_cancelled_task_is_removed_from_the_registrations() {
    // 取り消し: 記述子の登録（epoll からも外れる）、期限、取り出していない完了が外れ、その後はデータが来ても起きない。
    assert_eq!(step(&native(CANCEL_BODY)), "0");
}

const SHUTDOWN_BODY: &str = r#"
  let live0 = live_blocks();
  let s = io_new();
  if s == 0 { return 1; }
  let ep = io_count(s, 3);
  let ev = io_count(s, 4);
  if fd_open(ep) != 1 { return 2; }
  if fd_open(ev) != 1 { return 3; }
  let h = start(s);
  let p = pipe_new();
  let r = p / 65536;
  let w = p % 65536;
  io_wait_fd(s, r, 1, 30);
  io_sleep(s, 60000, 31);
  io_sleep(s, 61000, 32);
  sleep_ms(50);
  let t0 = io_now();
  io_stop(s);
  join(h);
  if io_now() - t0 > 5000000000 { return 4; }
  io_free(s);
  if fd_open(ep) != 0 { return 5; }
  if fd_open(ev) != 0 { return 6; }
  if fd_open(r) != 1 { return 7; }
  fd_close(r);
  fd_close(w);
  if live_blocks() != live0 { return 8; }
  return 0;
"#;

#[test]
fn stopping_ends_the_io_thread_and_returns_every_resource() {
    // 長い期限と登録が残ったままでも、停止で I/O スレッドはすぐ終わる。epoll と eventfd は閉じ、ヒープ・表・状態の塊は
    // 確保器に返る。呼び出し側の記述子（パイプ）は閉じない。
    assert_eq!(step(&native(SHUTDOWN_BODY)), "0");
}

#[test]
fn a_waiter_blocked_in_take_wait_is_released_by_stop() {
    // 完了が来ない `io_take_wait` は、停止で 0 を返して抜ける（眠ったままにならない）。
    let body = r#"
  let s = io_new();
  if s == 0 { return 1; }
  let h = start(s);
  let out = buf();
  io_stop(s);
  if io_take_wait(s, out) != 0 { return 2; }
  join(h);
  io_free(s);
  buf_free(out);
  return 0;
"#;
    assert_eq!(step(&native(body)), "0");
}

// --- ミューテーション: ソースを壊すと、対応する試験が失敗する ---

fn mutate(from: &str, to: &str) -> String {
    let src = io_source();
    assert_eq!(src.matches(from).count(), 1, "置換元が 1 か所: {from}");
    src.replacen(from, to, 1)
}

/// 壊した版で走らせ、確認の番号が 0（成功）にならないことを確かめる（停止やハングも失敗とみなす）。
fn must_fail(mutated: &str, body: &str) {
    // ビルドの失敗は試験の失敗ではなく、壊し方の誤りなので、そのまま panic させる。
    let got = native_with_io(mutated, body);
    let mutated_outcome = got
        .as_ref()
        .map(|r| (r.code, r.stdout.trim_end().to_string()));
    let passed = match got {
        None => false, // ハングも失敗とみなす
        Some(r) => r.code == Some(0) && r.stdout.trim_end().rsplit('\n').next() == Some("0"),
    };
    assert!(!passed, "壊したのに試験が通った");
    eprintln!("壊した版の結果: {:?}", mutated_outcome);
}

#[test]
fn mutation_without_oneshot_is_caught() {
    must_fail(
        &mutate(
            "ep_ctl(ep, 1, fd, events + EPOLLONESHOT, token, gen);",
            "ep_ctl(ep, 1, fd, events, token, gen);",
        ),
        ONESHOT_BODY,
    );
}

#[test]
fn mutation_reversed_deadline_order_is_caught() {
    must_fail(
        &mutate(
            "if di != dj { return di < dj; }",
            "if di != dj { return di > dj; }",
        ),
        TIMER_BODY,
    );
}

#[test]
fn mutation_firing_before_the_deadline_is_caught() {
    must_fail(
        &mutate(
            "ld(ld(s, S_HEAP), 0) <= now {",
            "ld(ld(s, S_HEAP), 0) <= now + 1000000000 {",
        ),
        TIMER_BODY,
    );
}

#[test]
fn mutation_cancel_keeps_the_registration_is_caught() {
    must_fail(
        &mutate(
            "    ep_ctl(ld(s, S_EP), 2, ld(ld(s, S_REGS), 3 * i), 0, 0, 0);\n    reg_remove_at(s, i);\n    removed = removed + 1;\n    i = reg_find_token(s, token);",
            "    removed = removed + 1;\n    i = 0 - 1;",
        ),
        CANCEL_BODY,
    );
}

// --- Codex 指摘への再現（PR #358） ---

const CANCEL_SAME_TOKEN_BODY: &str = r#"
  let s = io_new();
  if s == 0 { return 1; }
  let base = io_now() + 60000000000;
  io_sleep_until(s, base + 1000000000, 1);
  io_sleep_until(s, base + 5000000000, 2);
  io_sleep_until(s, base + 2000000000, 3);
  io_sleep_until(s, base + 6000000000, 9);
  io_sleep_until(s, base + 7000000000, 4);
  io_sleep_until(s, base + 3000000000, 9);
  io_sleep_until(s, base + 4000000000, 9);
  if io_count(s, 0) != 7 { return 2; }
  if io_cancel(s, 9) != 3 { return 3; }
  if io_count(s, 0) != 4 { return 4; }
  if io_cancel(s, 1) != 1 { return 5; }
  if io_cancel(s, 2) != 1 { return 6; }
  if io_cancel(s, 3) != 1 { return 7; }
  if io_cancel(s, 4) != 1 { return 8; }
  if io_count(s, 0) != 0 { return 9; }
  io_free(s);
  return 0;
"#;

#[test]
fn cancel_removes_every_deadline_of_the_token_even_after_heap_moves() {
    // 指摘 A: 期限（相対）[1,5,2,6,7,3,4] のうち 6・3・4 が同じトークン。1 つ消すたびに末尾の要素が走査済みの位置へ
    // 上がっても、取り消しはすべての期限を外す。
    assert_eq!(step(&native(CANCEL_SAME_TOKEN_BODY)), "0");
}

/// 試験用: epoll_wait と dispatch の間に別スレッドが割り込む状況を、1 スレッドで順に再現する入口。
const POLL_HOOKS: &str = "
pub fn io_poll_wait(s: Int) -> Int
  intent \"試験用: 待たずに epoll_wait を 1 回呼び、件数を返す\"
  requires none
  ensures none
  effects none
{
  return ep_wait(ld(s, S_EP), ld(s, S_EVBUF), 0);
}

pub fn io_poll_dispatch(s: Int, n: Int) -> Bool
  intent \"試験用: 受け取った n 件を dispatch する\"
  requires none
  ensures none
  effects none
{
  return dispatch(s, n);
}
";

const STALE_EVENT_BODY: &str = r#"
  let s = io_new();
  if s == 0 { return 1; }
  let out = buf();
  let p1 = pipe_new();
  let p2 = pipe_new();
  if io_wait_fd(s, p1 / 65536, 1, 7) != 0 { return 2; }
  wr(p1 % 65536);
  let n = io_poll_wait(s);
  if n != 1 { return 3; }
  if io_cancel(s, 7) != 1 { return 4; }
  if io_wait_fd(s, p2 / 65536, 1, 7) != 0 { return 5; }
  io_poll_dispatch(s, n);
  if io_take(s, out) != 0 { return 6; }
  wr(p2 % 65536);
  let m = io_poll_wait(s);
  if m != 1 { return 7; }
  io_poll_dispatch(s, m);
  if io_take(s, out) != 1 { return 8; }
  if word(out, 0) != 7 { return 9; }
  if io_take(s, out) != 0 { return 10; }
  io_cancel(s, 7);
  io_free(s);
  fd_close(p1 / 65536);
  fd_close(p1 % 65536);
  fd_close(p2 / 65536);
  fd_close(p2 % 65536);
  buf_free(out);
  return 0;
"#;

fn native_hooks(io_src: &str, body: &str) -> Option<Ran> {
    // 駆動の `use` に試験用の入口を足すため、本体の先頭で名前を取り込む形にはできない。駆動の生成後に差し込む。
    let src = format!("{io_src}{POLL_HOOKS}");
    native_with_io_uses(
        &src,
        body,
        &["std.os.io.io_poll_dispatch", "std.os.io.io_poll_wait"],
    )
}

#[test]
fn a_stale_event_for_a_cancelled_registration_is_not_delivered() {
    // 指摘 B: epoll_wait が受け取った後に取り消され、同じトークンで別の記述子に登録し直されても、古い通知は積まない。
    // 新しい登録は、自分の準備完了で通知される。
    let r = native_hooks(&io_source(), STALE_EVENT_BODY).expect("ハング");
    assert_eq!(step(&r), "0");
}

#[test]
fn mutation_ignoring_the_generation_is_caught() {
    let m = mutate_gen();
    let got = native_hooks(&m, STALE_EVENT_BODY);
    let passed = got
        .map(|r| r.code == Some(0) && r.stdout.trim_end().rsplit('\n').next() == Some("0"))
        .unwrap_or(false);
    assert!(!passed, "壊したのに試験が通った");
}

fn mutate_gen() -> String {
    mutate(
        "ld(r, 3 * i + 1) == token && ld(r, 3 * i + 2) == gen { return i; }",
        "ld(r, 3 * i + 1) == token { return i; }",
    )
}

const SAME_TOKEN_BODY: &str = r#"
  let s = io_new();
  if s == 0 { return 1; }
  let out = buf();
  let p1 = pipe_new();
  let p2 = pipe_new();
  if io_wait_fd(s, p1 / 65536, 1, 7) != 0 { return 2; }
  if io_wait_fd(s, p2 / 65536, 1, 7) != 0 { return 3; }
  wr(p1 % 65536);
  wr(p2 % 65536);
  let n = io_poll_wait(s);
  if n != 2 { return 4; }
  io_poll_dispatch(s, n);
  if io_take(s, out) != 1 { return 5; }
  if io_take(s, out) != 1 { return 6; }
  if io_take(s, out) != 0 { return 7; }
  if io_cancel(s, 7) != 2 { return 8; }
  io_free(s);
  fd_close(p1 / 65536);
  fd_close(p1 % 65536);
  fd_close(p2 / 65536);
  fd_close(p2 % 65536);
  buf_free(out);
  return 0;
"#;

#[test]
fn two_fds_with_the_same_token_are_both_delivered() {
    // 指摘 C: 同じトークンで二つの記述子を登録しても、どちらの通知も（トークンと世代の両方で引いて）届く。
    let r = native_hooks(&io_source(), SAME_TOKEN_BODY).expect("ハング");
    assert_eq!(step(&r), "0");
}

#[test]
fn mutation_first_token_match_only_is_caught() {
    let m = mutate(
        "let k = reg_find_exact(s, token, u32_at(buf, 12 * i + 8));\n      if k >= 0 {",
        "let k = reg_find_token(s, token);\n      if k >= 0 && ld(ld(s, S_REGS), 3 * k + 2) == u32_at(buf, 12 * i + 8) {",
    );
    let got = native_hooks(&m, SAME_TOKEN_BODY);
    let passed = got
        .map(|r| r.code == Some(0) && r.stdout.trim_end().rsplit('\n').next() == Some("0"))
        .unwrap_or(false);
    assert!(!passed, "壊したのに試験が通った");
}

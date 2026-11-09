//! ADR 0030 決定 4・補足 A-4 / PR 5-2c: スレッドの作成と、スレッドごとの領域（ネイティブだけ）。
//!
//! 評価器は複数の OS スレッドを動かさないので、駆動モジュール（`std.os.t`）から `std.os.thread` の
//! `spawn`・`join` を呼ぶ試験はネイティブで走らせる。スレッドの本体 `body` は、試験ごとに `std.os.thread` の
//! ソースの `body` を差し替えて作る（`embedded_extra`）。
mod common;

use common::native::{scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::stdlib::embedded::{EMBEDDED, THREAD_MODULE};

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// `std.os.thread` のソースの `body` を、`new_body`（`fn body(...)` から `}` まで）に差し替えたもの。
fn thread_with_body(new_body: &str) -> String {
    let src = EMBEDDED
        .iter()
        .find(|(n, _)| *n == THREAD_MODULE)
        .map(|(_, s)| *s)
        .expect("std.os.thread が登録されている");
    let a = src.find("fn body(arg: Addr)").expect("body がある");
    let end = src[a..].find("\n}\n").expect("body の終わり") + a + 3;
    format!("{}{}{}", &src[..a], new_body, &src[end..])
}

/// `thread_main` と `body` の効果を `console` にした版（本体が `console.log` を呼ぶ試験用）。
fn with_console(src: &str) -> String {
    let a = src.find("pub fn thread_main").expect("thread_main がある");
    let (head, tail) = src.split_at(a);
    format!(
        "{head}{}",
        tail.replacen("effects none", "effects console", 1)
    )
}

fn body_unchecked(stmts: &str) -> String {
    format!("fn body(arg: Addr)\n{TAIL}  unchecked \"試験用の本体\"\n{{\n{stmts}\n}}\n")
}

fn driver(body: &str, uses: &[&str]) -> String {
    let mut names: Vec<&str> = uses.to_vec();
    names.sort_unstable(); // `use` は文字の順に並べる
    let u: String = names.iter().map(|f| format!("use {f};\n")).collect();
    format!(
        "module std.os.t\n  targets server\n  trust base\n\n{u}\npub fn run() -> Int\n{TAIL}  unchecked \"試験\"\n{{\n{body}\n}}\n"
    )
}

fn user_entry() -> String {
    format!("module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n{TAIL}{{\n  return 7;\n}}\n")
}

/// 実行ファイルを時間制限つきで走らせる（ハングを試験の失敗にする）。`vmem_kb` があれば `ulimit -v` で仮想メモリを絞る
/// （スタックの領域の漏れを、`mmap` の失敗として見つける）。
fn run_exe(exe: &std::path::Path, vmem_kb: Option<u64>, limit: std::time::Duration) -> Ran {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut cmd = match vmem_kb {
        Some(kb) => {
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(format!("ulimit -v {kb}; exec \"$0\""))
                .arg(exe);
            c
        }
        None => Command::new(exe),
    };
    let mut child = cmd
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
            panic!("制限時間内に終わらなかった（ハング）");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let stdout = String::from_utf8_lossy(&t_out.join().unwrap()).into_owned();
    let stderr = String::from_utf8_lossy(&t_err.join().unwrap()).into_owned();
    use std::os::unix::process::ExitStatusExt;
    Ran {
        stdout,
        stderr,
        code: status.code().or(status.signal().map(|s| 128 + s)),
    }
}

/// 駆動をネイティブで走らせる。
fn native_with(
    thread_body: &str,
    run_body: &str,
    uses: &[&str],
    vmem_kb: Option<u64>,
    limit: std::time::Duration,
) -> Ran {
    let c = COpts {
        embedded_extra: vec![
            ("std.os.t".to_string(), driver(run_body, uses)),
            (THREAD_MODULE.to_string(), thread_with_body(thread_body)),
        ],
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    };
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let dir = scratch("thread52c");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = run_exe(&exe, vmem_kb, limit);
    let _ = std::fs::remove_dir_all(&dir);
    got
}

fn native(thread_body: &str, run_body: &str, uses: &[&str]) -> Ran {
    native_with(
        thread_body,
        run_body,
        uses,
        None,
        std::time::Duration::from_secs(60),
    )
}

fn value(r: &Ran) -> String {
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    r.stdout.trim_end().rsplit('\n').next().unwrap().to_string()
}

#[test]
fn a_thread_runs_the_body_and_join_waits_for_it() {
    // 本体は引数の語に 1 を原子的に足す。join の後、親は 1 を読む。
    let r = native(
        &body_unchecked("  raw.atomic_add_i64(arg, 1);"),
        "  let c = raw.alloc(16, 8);\n  raw.atomic_store_i64(c, 0);\n  let h = spawn(c);\n  join(h);\n  let n = raw.atomic_load_i64(c);\n  raw.free(c, 16, 8);\n  return n;",
        &["std.os.thread.spawn", "std.os.thread.join"],
    );
    assert_eq!(value(&r), "1");
}

const SPAWN: &str = "std.os.thread.spawn";
const JOIN: &str = "std.os.thread.join";
const LIVE: &str = "std.os.alloc.live_blocks";

/// `n` 個のスレッドを作り（引数はどれも `c`）、全部を待つ断片。ハンドルの配列は `hs`。
fn fan_out(n: usize) -> String {
    format!(
        "  let hs = raw.alloc({}, 8);\n  let mut k = 0;\n  while k < {n} {{\n    raw.store_addr(raw.offset(hs, k * 8), spawn(c));\n    k = k + 1;\n  }}\n  let mut j = 0;\n  while j < {n} {{\n    join(raw.load_addr(raw.offset(hs, j * 8)));\n    j = j + 1;\n  }}\n  raw.free(hs, {}, 8);\n",
        n * 8,
        n * 8
    )
}

#[test]
fn many_threads_add_atomically_to_one_counter() {
    // 8 スレッドが各 20000 回、同じ語に原子的に 1 を足す。足し漏れがなければ 160000。
    let r = native(
        &body_unchecked("  let mut i = 0;\n  while i < 20000 {\n    raw.atomic_add_i64(arg, 1);\n    i = i + 1;\n  }"),
        &format!(
            "  let c = raw.alloc(16, 8);\n  raw.atomic_store_i64(c, 0);\n{}  let n = raw.atomic_load_i64(c);\n  raw.free(c, 16, 8);\n  return n;",
            fan_out(8)
        ),
        &[SPAWN, JOIN],
    );
    assert_eq!(value(&r), "160000");
}

#[test]
fn the_allocator_stays_consistent_under_contention() {
    // 6 スレッドが、大きさの違う確保・書き込み・読み戻し・解放を繰り返す。領域が重なれば読み戻しが食い違う
    // （食い違いは原子的に数える）。最後に、生きている塊の数は元に戻る。
    let body = "  let mut i = 0;\n  while i < 3000 {\n    let mut sz = 16 + (i % 9) * 200;\n    let mut al = 8;\n    if i % 50 == 0 {\n      sz = 6000;\n      al = 64;\n    }\n    let p = raw.alloc(sz, al);\n    raw.store_i64(p, i);\n    raw.store_i64(raw.offset(p, sz - 8), i);\n    if raw.load_i64(p) != i || raw.load_i64(raw.offset(p, sz - 8)) != i {\n      raw.atomic_add_i64(arg, 1);\n    }\n    raw.free(p, sz, al);\n    i = i + 1;\n  }";
    let r = native(
        &body_unchecked(body),
        &format!(
            "  let c = raw.alloc(16, 8);\n  raw.atomic_store_i64(c, 0);\n  let before = live_blocks();\n{}  let errors = raw.atomic_load_i64(c);\n  let after = live_blocks();\n  raw.free(c, 16, 8);\n  return errors * 1000 + (after - before);",
            fan_out(6)
        ),
        &[SPAWN, JOIN, LIVE],
    );
    // 食い違い 0、生きている塊の数の差 0
    assert_eq!(value(&r), "0");
}

#[test]
fn each_thread_has_its_own_work_stack_for_drops() {
    // 作業の列（破棄の積み）がスレッドごとなら、文字列の一覧を作っては捨てる繰り返しを 4 スレッドが並行にしても、
    // 数えた要素の総数が合う。共有なら壊れる。
    let body = "  let mut i = 0;\n  while i < 2000 {\n    let mut xs = List.new();\n    let mut j = 0;\n    while j < 20 {\n      xs.push(\"  ab  \".trim());\n      j = j + 1;\n    }\n    raw.atomic_add_i64(arg, xs.len());\n    i = i + 1;\n  }";
    let r = native(
        &body_unchecked(body),
        &format!(
            "  let c = raw.alloc(16, 8);\n  raw.atomic_store_i64(c, 0);\n{}  let n = raw.atomic_load_i64(c);\n  raw.free(c, 16, 8);\n  return n;",
            fan_out(4)
        ),
        &[SPAWN, JOIN],
    );
    assert_eq!(value(&r), (4 * 2000 * 20).to_string());
}

#[test]
fn a_thread_that_overflows_its_stack_stops_with_the_report() {
    // スタックの下限もスレッドごと。スレッドの無限の再帰は、スレッドの領域の下限で止まって、報告を出す
    // （下限が主スレッドのものなら、ガードページに当たって SIGSEGV になる）。
    let new_body = format!(
        "fn dive(n: Int) -> Int\n{TAIL}{{\n  return dive(n + 1) + 1;\n}}\n\nfn body(arg: Addr)\n{TAIL}{{\n  dive(0);\n}}\n"
    );
    let r = native(
        &new_body,
        "  let h = spawn(raw.addr_null());\n  join(h);\n  return 0;",
        &[SPAWN, JOIN],
    );
    assert_eq!(r.code, Some(70), "{}", r.stderr);
    assert!(r.stderr.contains("スタックの枯渇"), "{}", r.stderr);
}

#[test]
fn a_worker_that_stops_ends_the_whole_process_without_hanging() {
    // 判断点 1 の基準 3: スレッドが契約違反（確保器の異常停止）で止まると、プロセスは終了コード 70 で終わり、
    // join で待っている親もハングしない。
    let r = native(
        &body_unchecked("  raw.alloc(0, 8);"),
        "  let h = spawn(raw.addr_null());\n  join(h);\n  return 0;",
        &[SPAWN, JOIN],
    );
    assert_eq!(r.code, Some(70), "{}", r.stderr);
}

/// 作る・終える・待ち合わせるを `n` 回繰り返す。`vmem_kb` で仮想メモリを絞るので、スタックの領域が返らなければ
/// `mmap` が失敗して `spawn` が空番地を返し、`fails` に数えられる。
fn churn(n: u64, limit_secs: u64) -> String {
    let run_body = format!(
        "  let c = raw.alloc(16, 8);\n  raw.atomic_store_i64(c, 0);\n  let before = live_blocks();\n  let mut fails = 0;\n  let mut i = 0;\n  while i < {n} {{\n    let h = spawn(c);\n    if raw.addr_to_int(h) == 0 {{\n      fails = fails + 1;\n    }} else {{\n      join(h);\n    }}\n    i = i + 1;\n  }}\n  let counter = raw.atomic_load_i64(c);\n  let after = live_blocks();\n  raw.free(c, 16, 8);\n  return counter + fails * 100000000 + (after - before) * 10000000000;"
    );
    let r = native_with(
        &body_unchecked("  raw.atomic_add_i64(arg, 1);"),
        &run_body,
        &[SPAWN, JOIN, LIVE],
        Some(3 * 1024 * 1024),
        std::time::Duration::from_secs(limit_secs),
    );
    value(&r)
}

#[test]
fn create_and_exit_in_a_loop_returns_every_stack() {
    // 判断点 1 の基準 1（回数を絞った版）: 領域が漏れれば、仮想メモリの上限（約 3GiB）で `mmap` が失敗する。
    assert_eq!(churn(2000, 120), "2000");
}

#[test]
#[ignore = "100 万回。時間がかかるので、手で実行する: cargo test --test thread_5_2c -- --ignored million"]
fn million_create_and_exit_cycles() {
    assert_eq!(churn(1_000_000, 3600), "1000000");
}

#[test]
fn a_thread_can_write_to_the_console_with_its_own_show_buffer() {
    // 表示バッファがスレッドごとなら、4 スレッドが同じ桁の数（1111, 2222, 3333, 4444）を並行に 300 回ずつ表示しても、
    // 桁ごとの個数が合う（行の混ざりは許す。個々の書き込みは分割されない）。
    let thread = with_console(&thread_with_body(
        "fn body(arg: Addr)\n  intent \"x\"\n  requires none\n  ensures none\n  effects console\n  unchecked \"試験用の本体\"\n{\n  let v = raw.addr_to_int(arg);\n  let mut i = 0;\n  while i < 300 {\n    console.log(&v);\n    i = i + 1;\n  }\n}\n",
    ));
    // `with_console` は全体のソースを受け取るので、差し替え済みのソースをそのまま渡す。
    let c = COpts {
        embedded_extra: vec![
            (
                "std.os.t".to_string(),
                driver(
                    "  let h1 = spawn(raw.int_to_addr(1111));\n  let h2 = spawn(raw.int_to_addr(2222));\n  let h3 = spawn(raw.int_to_addr(3333));\n  let h4 = spawn(raw.int_to_addr(4444));\n  join(h1);\n  join(h2);\n  join(h3);\n  join(h4);\n  return 0;",
                    &[SPAWN, JOIN],
                ),
            ),
            (THREAD_MODULE.to_string(), thread),
        ],
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    };
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    let dir = scratch("thread52c-show");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let r = run_exe(&exe, None, std::time::Duration::from_secs(60));
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    for d in ['1', '2', '3', '4'] {
        let n = r.stdout.chars().filter(|&c| c == d).count();
        assert_eq!(n, 4 * 300, "桁 {d} の個数: {}", r.stdout.len());
    }
    // 数字と改行のほかは出ていない
    assert!(r
        .stdout
        .chars()
        .all(|c| c.is_ascii_digit() || c == '\n' || c == '0'));
}

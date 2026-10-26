//! Sigil の暫定ランタイムの核（ADR 0015 の決定 6）。
//!
//! 確保と解放、`String`・`List`・`Shared` の操作、値の表示、異常停止の報告、スタックの下限、入口の引数の読み取り。
//! OS の機能（`console.log`・`fs`・`Path`）は `runtime/os` にある。関数の名前は `sigil_rt_` で始め、`extern "C"` とする。
//! 境界は ADR 0014 の決定 2 の宣言で、Sigil のプログラムから名前で呼ぶ手段は作らない（ADR 0006 の「境界」）。
//!
//! 所有の約束: `*mut` で返された値は呼び出し側のもので、対の `..._free` で解放する。`*const` で受け取る値は借用。
//! 生成したコードは正しい番地だけを渡す前提で、関数は `unsafe` である。

use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "../../../src/shared/args.rs"]
mod args;
#[path = "../../../src/shared/text.rs"]
mod text;

/// 異常停止の終了コード（ADR 0006 の M3 の記録）。
pub const EXIT_ABNORMAL_STOP: i32 = 70;
/// 入口を実行するスレッドのスタックの大きさ（インタプリタと同じ）。
pub const STACK_BYTES: usize = 256 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 確保と解放（ADR 0014 の決定 2: 確保器は環境が供給する）

fn layout(size: usize, align: usize) -> Layout {
    match Layout::from_size_align(size.max(1), align.max(1)) {
        Ok(l) => l,
        Err(_) => std::process::abort(),
    }
}

/// # Safety
/// `align` は 2 の冪。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_alloc(size: usize, align: usize) -> *mut u8 {
    let l = layout(size, align);
    let p = alloc(l);
    if p.is_null() {
        handle_alloc_error(l);
    }
    p
}

/// # Safety
/// `p` は同じ `size`・`align` の `sigil_rt_alloc` の戻り値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_free(p: *mut u8, size: usize, align: usize) {
    if !p.is_null() {
        dealloc(p, layout(size, align));
    }
}

// ---------------------------------------------------------------------------
// String（UTF-8。不透明な入れ物）

pub struct RtStr(String);

fn new_str(s: String) -> *mut RtStr {
    Box::into_raw(Box::new(RtStr(s)))
}

unsafe fn s<'a>(p: *const RtStr) -> &'a str {
    &(*p).0
}

/// バイト列（UTF-8）から作る。不正な列は置換文字になる。
///
/// # Safety
/// `ptr` は `len` バイト読める。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_new(ptr: *const u8, len: usize) -> *mut RtStr {
    let bytes = if len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(ptr, len)
    };
    new_str(String::from_utf8_lossy(bytes).into_owned())
}

/// # Safety
/// `p` は `sigil_rt_str_*` が返したもの（または null）。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_free(p: *mut RtStr) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_clone(p: *const RtStr) -> *mut RtStr {
    new_str(s(p).to_string())
}

/// バイト列の先頭（長さは `sigil_rt_str_bytes`）。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_data(p: *const RtStr) -> *const u8 {
    s(p).as_ptr()
}

/// バイト数。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_bytes(p: *const RtStr) -> usize {
    s(p).len()
}

/// `String.len`: Unicode のスカラー値の数。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_len(p: *const RtStr) -> i64 {
    text::char_len(s(p))
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_is_empty(p: *const RtStr) -> i32 {
    s(p).is_empty() as i32
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_trim(p: *const RtStr) -> *mut RtStr {
    new_str(text::trim(s(p)).to_string())
}

/// `a + b`（連結）。
///
/// # Safety
/// `a`・`b` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_concat(a: *const RtStr, b: *const RtStr) -> *mut RtStr {
    let mut r = String::with_capacity(s(a).len() + s(b).len());
    r.push_str(s(a));
    r.push_str(s(b));
    new_str(r)
}

/// 構造的な比較（-1・0・1）。
///
/// # Safety
/// `a`・`b` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_cmp(a: *const RtStr, b: *const RtStr) -> i32 {
    s(a).cmp(s(b)) as i32
}

/// `String.parse_int`。成功なら 1 を返し `*out` に値を書く。失敗なら 0。
///
/// # Safety
/// `p` は有効。`out` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_parse_int(p: *const RtStr, out: *mut i64) -> i32 {
    match text::parse_int(s(p)) {
        Some(n) => {
            *out = n;
            1
        }
        None => 0,
    }
}

/// `String.lines`。要素は `*mut RtStr`（`sigil_rt_list_free_strs` で解放する）。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_lines(p: *const RtStr) -> *mut RtList {
    let l = sigil_rt_list_new(std::mem::size_of::<*mut RtStr>());
    for line in text::lines(s(p)) {
        let e = new_str(line.to_string());
        sigil_rt_list_push(l, &e as *const *mut RtStr as *const u8);
    }
    l
}

// ---------------------------------------------------------------------------
// List（要素の大きさは生成したコードが渡す。要素は 16 バイト境界に置く）

pub struct RtList {
    elem: usize,
    len: usize,
    cap: usize,
    buf: *mut u8,
}

const LIST_ALIGN: usize = 16;

#[no_mangle]
pub extern "C" fn sigil_rt_list_new(elem_size: usize) -> *mut RtList {
    Box::into_raw(Box::new(RtList {
        elem: elem_size,
        len: 0,
        cap: 0,
        buf: std::ptr::null_mut(),
    }))
}

/// 入れ物だけを解放する（要素の破棄は呼び出し側が先に行う）。
///
/// # Safety
/// `l` は `sigil_rt_list_new` の戻り値（または null）。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_free(l: *mut RtList) {
    if l.is_null() {
        return;
    }
    let b = Box::from_raw(l);
    if !b.buf.is_null() {
        sigil_rt_free(b.buf, b.cap * b.elem, LIST_ALIGN);
    }
}

/// 要素が `*mut RtStr` の一覧を、要素ごと解放する。
///
/// # Safety
/// `l` の要素は `RtStr`。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_free_strs(l: *mut RtList) {
    if l.is_null() {
        return;
    }
    for i in 0..(*l).len {
        let e = *((*l).buf as *mut *mut RtStr).add(i);
        sigil_rt_str_free(e);
    }
    sigil_rt_list_free(l);
}

/// # Safety
/// `l` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_len(l: *const RtList) -> i64 {
    (*l).len as i64
}

/// `elem` の指す `elem_size` バイトを末尾へ写す（移動。元の破棄は生成したコードが済ませる）。
///
/// # Safety
/// `l` は有効。`elem` は `elem_size` バイト読める。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_push(l: *mut RtList, elem: *const u8) {
    let r = &mut *l;
    if r.len == r.cap {
        let ncap = if r.cap == 0 { 4 } else { r.cap * 2 };
        let nbuf = sigil_rt_alloc(ncap * r.elem, LIST_ALIGN);
        if !r.buf.is_null() {
            std::ptr::copy_nonoverlapping(r.buf, nbuf, r.len * r.elem);
            sigil_rt_free(r.buf, r.cap * r.elem, LIST_ALIGN);
        }
        r.buf = nbuf;
        r.cap = ncap;
    }
    if r.elem > 0 {
        std::ptr::copy_nonoverlapping(elem, r.buf.add(r.len * r.elem), r.elem);
    }
    r.len += 1;
}

/// 位置 `i` の要素の番地。範囲外は null。
///
/// # Safety
/// `l` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_get(l: *const RtList, i: i64) -> *mut u8 {
    let r = &*l;
    if i < 0 || i as usize >= r.len {
        return std::ptr::null_mut();
    }
    if r.elem == 0 {
        return LIST_ALIGN as *mut u8;
    }
    r.buf.add(i as usize * r.elem)
}

// ---------------------------------------------------------------------------
// Shared<T>（参照の数は原子的でない。単一スレッド用。ADR 0006 の条件 2）
// 領域: 先頭 16 バイトが参照の数（i64）、続いて中身。

const SHARED_HEAD: usize = 16;

/// 参照の数 1 の領域を作る。戻り値は領域の先頭。
///
/// # Safety
/// `align` は 2 の冪。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_shared_new(size: usize, align: usize) -> *mut u8 {
    let p = sigil_rt_alloc(SHARED_HEAD + size, align.max(SHARED_HEAD));
    *(p as *mut i64) = 1;
    p
}

/// 中身の番地。
///
/// # Safety
/// `p` は `sigil_rt_shared_new` の戻り値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_shared_payload(p: *mut u8) -> *mut u8 {
    p.add(SHARED_HEAD)
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_shared_retain(p: *mut u8) {
    *(p as *mut i64) += 1;
}

/// 参照の数を 1 減らす。0 になったら 1 を返す（呼び出し側が中身を破棄して `sigil_rt_shared_free` を呼ぶ）。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_shared_release(p: *mut u8) -> i32 {
    let c = p as *mut i64;
    *c -= 1;
    (*c == 0) as i32
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_shared_count(p: *const u8) -> i64 {
    *(p as *const i64)
}

/// # Safety
/// `size`・`align` は `sigil_rt_shared_new` に渡したもの。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_shared_free(p: *mut u8, size: usize, align: usize) {
    sigil_rt_free(p, SHARED_HEAD + size, align.max(SHARED_HEAD));
}

// ---------------------------------------------------------------------------
// 値の表示（`sigil run` の戻り値の表示と同じ書き方。合成は生成したコードが行う）

#[no_mangle]
pub extern "C" fn sigil_rt_show_int(n: i64) -> *mut RtStr {
    new_str(n.to_string())
}

#[no_mangle]
pub extern "C" fn sigil_rt_show_float(f: f64) -> *mut RtStr {
    new_str(text::fmt_float(f))
}

#[no_mangle]
pub extern "C" fn sigil_rt_show_bool(b: i32) -> *mut RtStr {
    new_str((b != 0).to_string())
}

#[no_mangle]
pub extern "C" fn sigil_rt_show_unit() -> *mut RtStr {
    new_str("()".to_string())
}

/// 引用符つきの文字列（`{:?}` と同じ）。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_show_str(p: *const RtStr) -> *mut RtStr {
    new_str(text::quote(s(p)))
}

fn write_out(bytes: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(bytes);
    let _ = o.flush();
}

/// 入口の戻り値の表示を標準出力に 1 行出す。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_print_result(p: *const RtStr) {
    let mut line = s(p).to_string();
    line.push('\n');
    write_out(line.as_bytes());
}

// ---------------------------------------------------------------------------
// 異常停止の報告と終了

fn flush_out() {
    let _ = std::io::stdout().flush();
}

/// 標準出力を書き出し切ってから、停止の報告を標準エラーに 1 行出す。
///
/// # Safety
/// `ptr` は `len` バイト読める。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_report(ptr: *const u8, len: usize) {
    flush_out();
    let bytes = if len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(ptr, len)
    };
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(bytes);
    let _ = e.write_all(b"\n");
    let _ = e.flush();
}

/// 異常停止: 報告を出して終了コード 70 で終わる。
///
/// # Safety
/// `ptr` は `len` バイト読める。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_stop(ptr: *const u8, len: usize) -> ! {
    sigil_rt_report(ptr, len);
    std::process::exit(EXIT_ABNORMAL_STOP)
}

/// 標準出力を書き出し切って終わる。
#[no_mangle]
pub extern "C" fn sigil_rt_exit(code: i32) -> ! {
    flush_out();
    std::process::exit(code)
}

/// `--trace-drops`: `drop: <表示>` を標準エラーに 1 行出す。
///
/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_trace_drop(p: *const RtStr) {
    let mut e = std::io::stderr().lock();
    let _ = writeln!(e, "drop: {}", s(p));
}

// ---------------------------------------------------------------------------
// スタックの下限（決定 5）

static STACK_LIMIT: AtomicUsize = AtomicUsize::new(0);

#[inline(never)]
fn stack_address() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

/// 生成した関数の入口が、スタックの番地と比べる下限。0 は未設定（常に十分）。
#[no_mangle]
pub extern "C" fn sigil_rt_stack_limit() -> usize {
    STACK_LIMIT.load(Ordering::Relaxed)
}

/// 現在のスタックの番地から、`usable` バイト下に下限を置く。
#[no_mangle]
pub extern "C" fn sigil_rt_stack_init(usable: usize) {
    STACK_LIMIT.store(stack_address().saturating_sub(usable), Ordering::Relaxed);
}

/// `stack_bytes` のスタックを持つスレッドで `entry(arg)` を呼び、戻り値を返す。下限はスタックの 3/4。
/// スレッドを作れなければ 1 を返す。
///
/// # Safety
/// `entry` は `arg` を受け取って戻る関数。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_enter_with(
    entry: extern "C" fn(*mut u8) -> i32,
    arg: *mut u8,
    stack_bytes: usize,
) -> i32 {
    struct Send_(*mut u8);
    unsafe impl Send for Send_ {}
    let a = Send_(arg);
    let h = std::thread::Builder::new()
        .stack_size(stack_bytes)
        .spawn(move || {
            let a = a;
            sigil_rt_stack_init(stack_bytes / 4 * 3);
            entry(a.0)
        });
    match h {
        Ok(h) => h.join().unwrap_or(1),
        Err(_) => 1,
    }
}

/// 256 MiB のスタックで入口を呼ぶ（下限は上端から 192 MiB 下）。
///
/// # Safety
/// `sigil_rt_enter_with` と同じ。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_enter(entry: extern "C" fn(*mut u8) -> i32, arg: *mut u8) -> i32 {
    sigil_rt_enter_with(entry, arg, STACK_BYTES)
}

// ---------------------------------------------------------------------------
// 入口の引数の読み取り（`sigil run` と同じ書き方。宣言の型への合わせ込みは生成したコードが行う）

#[repr(transparent)]
pub struct RtArg(args::Arg);

/// 引数 1 つを読む。誤りなら null を返し、`*err` に説明（`sigil_rt_str_free` で解放）を書く。
///
/// # Safety
/// `ptr` は `len` バイト読める。`err` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_parse(
    ptr: *const u8,
    len: usize,
    err: *mut *mut RtStr,
) -> *mut RtArg {
    let src = sigil_rt_str_new(ptr, len);
    let r = args::parse_arg(s(src));
    sigil_rt_str_free(src);
    match r {
        Ok(a) => Box::into_raw(Box::new(RtArg(a))),
        Err(m) => {
            *err = new_str(m);
            std::ptr::null_mut()
        }
    }
}

/// # Safety
/// `a` は `sigil_rt_arg_parse` の戻り値（または null）。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_free(a: *mut RtArg) {
    if !a.is_null() {
        drop(Box::from_raw(a));
    }
}

/// 種類: 0 Bool・1 Int・2 Float・3 String・4 List。
///
/// # Safety
/// `a` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_kind(a: *const RtArg) -> i32 {
    match (*a).0 {
        args::Arg::Bool(_) => 0,
        args::Arg::Int(_) => 1,
        args::Arg::Float(_) => 2,
        args::Arg::Str(_) => 3,
        args::Arg::List(_) => 4,
    }
}

/// # Safety
/// `a` は有効で種類が Bool。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_bool(a: *const RtArg) -> i32 {
    matches!((*a).0, args::Arg::Bool(true)) as i32
}

/// # Safety
/// `a` は有効で種類が Int。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_int(a: *const RtArg) -> i64 {
    match (*a).0 {
        args::Arg::Int(n) => n,
        _ => 0,
    }
}

/// `Float` の位置の整数は `Float` として読める（`coerce` と同じ規則）。
///
/// # Safety
/// `a` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_float(a: *const RtArg) -> f64 {
    match (*a).0 {
        args::Arg::Float(x) => x,
        args::Arg::Int(n) => n as f64,
        _ => 0.0,
    }
}

/// # Safety
/// `a` は有効で種類が String。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_str(a: *const RtArg) -> *mut RtStr {
    match &(*a).0 {
        args::Arg::Str(x) => new_str(x.clone()),
        _ => new_str(String::new()),
    }
}

/// # Safety
/// `a` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_list_len(a: *const RtArg) -> i64 {
    match &(*a).0 {
        args::Arg::List(xs) => xs.len() as i64,
        _ => 0,
    }
}

/// 一覧の要素（借用。`a` が生きている間だけ有効）。範囲外は null。
///
/// # Safety
/// `a` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_list_get(a: *const RtArg, i: i64) -> *const RtArg {
    match &(*a).0 {
        // `RtArg` は `Arg` と同じ並びの 1 欄の構造体（repr(transparent)）
        args::Arg::List(xs) if i >= 0 && (i as usize) < xs.len() => {
            &xs[i as usize] as *const args::Arg as *const RtArg
        }
        _ => std::ptr::null(),
    }
}

/// 型の合わない引数の説明に出す名前（`整数`・`小数`・`真偽値`・`文字列`・`一覧`）。
///
/// # Safety
/// `a` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_name(a: *const RtArg) -> *mut RtStr {
    new_str(args::arg_name(&(*a).0).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn st(s: &str) -> *mut RtStr {
        sigil_rt_str_new(s.as_ptr(), s.len())
    }

    unsafe fn text(p: *mut RtStr) -> String {
        let t = s(p).to_string();
        sigil_rt_str_free(p);
        t
    }

    #[test]
    fn show_and_strings() {
        unsafe {
            assert_eq!(text(sigil_rt_show_int(-3)), "-3");
            assert_eq!(text(sigil_rt_show_float(1.5)), "1.5");
            assert_eq!(text(sigil_rt_show_float(3.0)), "3.0");
            assert_eq!(text(sigil_rt_show_float(f64::NAN)), "NaN");
            assert_eq!(text(sigil_rt_show_float(f64::NEG_INFINITY)), "-inf");
            assert_eq!(text(sigil_rt_show_bool(1)), "true");
            assert_eq!(text(sigil_rt_show_unit()), "()");
            let a = st("  a b\t\n");
            assert_eq!(text(sigil_rt_str_trim(a)), "a b");
            let q = st("a\"\n");
            assert_eq!(text(sigil_rt_show_str(q)), "\"a\\\"\\n\"");
            assert_eq!(sigil_rt_str_len(st("日本語")), 3);
            let (x, y) = (st("ab"), st("cd"));
            assert_eq!(text(sigil_rt_str_concat(x, y)), "abcd");
            assert!(sigil_rt_str_cmp(x, y) < 0);
            let mut n = 0i64;
            assert_eq!(sigil_rt_str_parse_int(st("-42"), &mut n), 1);
            assert_eq!(n, -42);
            assert_eq!(sigil_rt_str_parse_int(st("+1"), &mut n), 0);
        }
    }

    #[test]
    fn lines_list_and_shared() {
        unsafe {
            let l = sigil_rt_str_lines(st("a\r\nb\n\nc\n"));
            assert_eq!(sigil_rt_list_len(l), 4);
            let e = *(sigil_rt_list_get(l, 1) as *mut *mut RtStr);
            assert_eq!(s(e), "b");
            assert!(sigil_rt_list_get(l, 4).is_null());
            sigil_rt_list_free_strs(l);

            let p = sigil_rt_shared_new(8, 8);
            assert_eq!(sigil_rt_shared_count(p), 1);
            sigil_rt_shared_retain(p);
            assert_eq!(sigil_rt_shared_count(p), 2);
            assert_eq!(sigil_rt_shared_release(p), 0);
            assert_eq!(sigil_rt_shared_release(p), 1);
            sigil_rt_shared_free(p, 8, 8);

            let m = sigil_rt_alloc(32, 16);
            assert_eq!(m as usize % 16, 0);
            sigil_rt_free(m, 32, 16);
        }
    }

    #[test]
    fn entry_args() {
        unsafe {
            let mut err = std::ptr::null_mut();
            let a = sigil_rt_arg_parse(b"[1, 2]".as_ptr(), 6, &mut err);
            assert!(!a.is_null());
            assert_eq!(sigil_rt_arg_list_len(a), 2);
            sigil_rt_arg_free(a);
            let bad = sigil_rt_arg_parse(b"99999999999999999999".as_ptr(), 20, &mut err);
            assert!(bad.is_null());
            assert!(!s(err).is_empty());
        }
    }
}

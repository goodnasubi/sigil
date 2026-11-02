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
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

#[path = "../../../src/shared/args.rs"]
mod args;
#[path = "../../../src/shared/text.rs"]
mod text;
mod val;

/// 異常停止の終了コード（ADR 0006 の M3 の記録）。
pub const EXIT_ABNORMAL_STOP: i32 = 70;
/// 入口を実行するスレッドのスタックの大きさ（SIR の評価器と同じ）。
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
// String（UTF-8。不透明な入れ物。表現は Sigil 製の `std.core.string` が持つ。補足 D）

/// 24B の頭（ptr・len・cap）の塊。Rust 側は中身を知らず、登録された関数表を通してだけ触る。
#[repr(C)]
pub struct RtStr {
    _p: [u8; 0],
}

type StrNew = extern "C" fn(*const u8, i64) -> *mut RtStr;
type StrFree = extern "C" fn(*mut RtStr);
type StrClone = extern "C" fn(*const RtStr) -> *mut RtStr;
type StrData = extern "C" fn(*const RtStr) -> *const u8;
type StrBytes = extern "C" fn(*const RtStr) -> i64;

/// 文字列の関数表（new・free・clone・data・bytes の関数ポインタ。0 は未登録）。
static STR_OPS: [AtomicUsize; 5] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// 生成コードの `main` が、入口を呼ぶ前に文字列の関数表を登録する（補足 D-3）。
#[no_mangle]
pub extern "C" fn sigil_rt_set_str_ops(
    new: StrNew,
    free: StrFree,
    clone: StrClone,
    data: StrData,
    bytes: StrBytes,
) {
    STR_OPS[0].store(new as usize, Ordering::Relaxed);
    STR_OPS[1].store(free as usize, Ordering::Relaxed);
    STR_OPS[2].store(clone as usize, Ordering::Relaxed);
    STR_OPS[3].store(data as usize, Ordering::Relaxed);
    STR_OPS[4].store(bytes as usize, Ordering::Relaxed);
}

/// 関数表の `i` 番目。未登録なら異常停止（exit_group(70) と同じ終了コード）。
fn str_op(i: usize) -> usize {
    // 試験では自動で登録する。ただし未登録の試験（子プロセス）では登録しない。
    #[cfg(test)]
    if std::env::var("SIGIL_TEST_STR_MODE").as_deref() != Ok("unregistered") {
        tests::table::init();
    }
    let p = STR_OPS[i].load(Ordering::Relaxed);
    if p == 0 {
        eprintln!("sigil: 文字列の関数表が未登録のまま文字列を使った");
        std::process::exit(70);
    }
    p
}

// SAFETY（以下の 5 つ）: `str_op` が返す値は `sigil_rt_set_str_ops` が受け取った、対応する型の関数ポインタだけ。
fn str_new_raw(ptr: *const u8, len: usize) -> *mut RtStr {
    let f: StrNew = unsafe { std::mem::transmute(str_op(0)) };
    f(ptr, len as i64)
}

pub(crate) fn str_free(p: *mut RtStr) {
    if !p.is_null() {
        let f: StrFree = unsafe { std::mem::transmute(str_op(1)) };
        f(p)
    }
}

pub(crate) fn str_clone(p: *const RtStr) -> *mut RtStr {
    let f: StrClone = unsafe { std::mem::transmute(str_op(2)) };
    f(p)
}

fn str_data(p: *const RtStr) -> *const u8 {
    let f: StrData = unsafe { std::mem::transmute(str_op(3)) };
    f(p)
}

fn str_bytes(p: *const RtStr) -> usize {
    let f: StrBytes = unsafe { std::mem::transmute(str_op(4)) };
    f(p) as usize
}

// ---------------------------------------------------------------------------
// 生きている塊の数（ADR 0018 の決定 5。テストが解放の漏れと二重解放を見つけるための数え）
//
// 文字列の数 `strs=` は、文字列を Sigil 製の確保器が持つようになったので 0 に固定する（補足 D-5。書式は 3-5e まで保つ）。
// 文字列の漏れ・二重解放は `sigil_blocks=` の厳密一致で見る。

static LIVE_BLOCKS: AtomicIsize = AtomicIsize::new(0);

/// 見出しつきの塊の数を `d` だけ増やす。0 を下回ったら二重解放として止める。
pub(crate) fn live_blocks(d: isize) {
    if LIVE_BLOCKS.fetch_add(d, Ordering::Relaxed) + d < 0 {
        std::process::abort();
    }
}

/// Sigil 製の確保器の生存数を返す関数（生成コードの `sigil_rt_sigil_live_blocks`。0 は未登録。補足 B-5）。
static SIGIL_LIVE: AtomicUsize = AtomicUsize::new(0);

/// 確保器を結んだプログラムの `main` が、入口を呼ぶ前に生存数の関数を登録する。
#[no_mangle]
pub extern "C" fn sigil_rt_set_sigil_live(f: extern "C" fn() -> i64) {
    SIGIL_LIVE.store(f as usize, Ordering::Relaxed);
}

/// 登録された確保器の生存数（未登録なら `None`）。
fn sigil_live_blocks() -> Option<i64> {
    let p = SIGIL_LIVE.load(Ordering::Relaxed);
    if p == 0 {
        return None;
    }
    // SAFETY: 0 でない値は `sigil_rt_set_sigil_live` が受け取った関数ポインタだけ。
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(p) };
    Some(f())
}

/// 生きている文字列・パスの数。文字列の表現を Sigil が持つので常に 0（補足 D-5）。
#[no_mangle]
pub extern "C" fn sigil_rt_live_strs() -> isize {
    0
}

/// 生きている見出しつきの塊の数。
#[no_mangle]
pub extern "C" fn sigil_rt_live_blocks() -> isize {
    LIVE_BLOCKS.load(Ordering::Relaxed)
}

/// Rust の文字列から作る（正しい UTF-8 なので `new_lossy` は複写と同じ）。
fn new_str(s: String) -> *mut RtStr {
    str_new_raw(s.as_ptr(), s.len())
}

/// 中身を読む。正しい UTF-8 でなければ（表現が壊れている）異常停止する。
unsafe fn s<'a>(p: *const RtStr) -> &'a str {
    let n = str_bytes(p);
    if n == 0 {
        return "";
    }
    match std::str::from_utf8(std::slice::from_raw_parts(str_data(p), n)) {
        Ok(t) => t,
        Err(_) => {
            eprintln!("sigil: 文字列の中身が正しい UTF-8 でない");
            std::process::exit(70);
        }
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
// Bytes（バイト列。可変長、8 バイト境界）

pub struct RtBytes(Vec<u8>);

fn new_bytes(v: Vec<u8>) -> *mut RtBytes {
    live_blocks(1);
    Box::into_raw(Box::new(RtBytes(v)))
}

unsafe fn bytes_slice<'a>(p: *const RtBytes) -> &'a [u8] {
    &(*p).0
}

unsafe fn bytes_vec<'a>(p: *mut RtBytes) -> &'a mut Vec<u8> {
    &mut (*p).0
}

#[no_mangle]
pub extern "C" fn sigil_rt_bytes_new() -> *mut RtBytes {
    new_bytes(Vec::new())
}

#[no_mangle]
pub extern "C" fn sigil_rt_bytes_with_capacity(capacity: i64) -> *mut RtBytes {
    new_bytes(Vec::with_capacity(capacity.max(0) as usize))
}

/// # Safety
/// `p` は `sigil_rt_bytes_*` が返したもの（または null）。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_free(p: *mut RtBytes) {
    if !p.is_null() {
        live_blocks(-1);
        drop(Box::from_raw(p));
    }
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_clone(p: *const RtBytes) -> *mut RtBytes {
    new_bytes(bytes_slice(p).to_vec())
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_len(p: *const RtBytes) -> i64 {
    bytes_slice(p).len() as i64
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_is_empty(p: *const RtBytes) -> i32 {
    if bytes_slice(p).is_empty() {
        1
    } else {
        0
    }
}

/// # Safety
/// `p` は有効。`out` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_get(p: *const RtBytes, index: i64, out: *mut i64) -> i32 {
    if index >= 0 && (index as usize) < bytes_slice(p).len() {
        *out = bytes_slice(p)[index as usize] as i64;
        1
    } else {
        0
    }
}

/// # Safety
/// `p` は有効。`err` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_set(
    p: *mut RtBytes,
    index: i64,
    byte: i64,
    err: *mut *mut RtStr,
) -> i32 {
    if index < 0 || (index as usize) >= bytes_vec(p).len() {
        *err = new_str("インデックスが範囲外".to_string());
        0
    } else if !(0..=255).contains(&byte) {
        *err = new_str("バイト値が 0..=255 の範囲外".to_string());
        0
    } else {
        bytes_vec(p)[index as usize] = byte as u8;
        1
    }
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_push(p: *mut RtBytes, byte: i64) {
    bytes_vec(p).push(byte as u8);
}

/// # Safety
/// `p` は有効。`out`・`err` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_slice(
    p: *const RtBytes,
    start: i64,
    end: i64,
    out: *mut *mut RtBytes,
    err: *mut *mut RtStr,
) -> i32 {
    let len = bytes_slice(p).len() as i64;
    if start >= 0 && start <= end && end <= len {
        *out = new_bytes(bytes_slice(p)[start as usize..end as usize].to_vec());
        1
    } else {
        *err = new_str("スライス範囲が不正".to_string());
        0
    }
}

/// # Safety
/// `a`・`b_ptr` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_concat(
    a: *const RtBytes,
    b_ptr: *const RtBytes,
) -> *mut RtBytes {
    let mut v = Vec::with_capacity(bytes_slice(a).len() + bytes_slice(b_ptr).len());
    v.extend_from_slice(bytes_slice(a));
    v.extend_from_slice(bytes_slice(b_ptr));
    new_bytes(v)
}

/// # Safety
/// `a`・`b_ptr` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_extend(a: *mut RtBytes, b_ptr: *const RtBytes) {
    bytes_vec(a).extend_from_slice(bytes_slice(b_ptr));
}

/// # Safety
/// `p` は有効。`out`・`err` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_to_string_utf8(
    p: *const RtBytes,
    out: *mut *mut RtStr,
    err: *mut *mut RtStr,
) -> i32 {
    match String::from_utf8(bytes_slice(p).to_vec()) {
        Ok(s) => {
            *out = new_str(s);
            1
        }
        Err(e) => {
            *err = new_str(e.to_string());
            0
        }
    }
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_str_as_bytes(p: *const RtStr) -> *mut RtBytes {
    new_bytes(s(p).as_bytes().to_vec())
}

/// # Safety
/// `l` は有効。`out`・`err` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_from_list(
    l: *const RtList,
    out: *mut *mut RtBytes,
    err: *mut *mut RtStr,
) -> i32 {
    let n = sigil_rt_list_len(l).max(0) as usize;
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let val = *(sigil_rt_list_get(l, i as i64) as *const i64);
        if !(0..=255).contains(&val) {
            *err = new_str("0..=255 の範囲外".to_string());
            return 0;
        }
        v.push(val as u8);
    }
    *out = new_bytes(v);
    1
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_push_u16_be(p: *mut RtBytes, value: i64) {
    bytes_vec(p).extend_from_slice(&(value as u16).to_be_bytes());
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_push_u32_be(p: *mut RtBytes, value: i64) {
    bytes_vec(p).extend_from_slice(&(value as u32).to_be_bytes());
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_push_u64_be(p: *mut RtBytes, value: i64) {
    bytes_vec(p).extend_from_slice(&(value as u64).to_be_bytes());
}

/// # Safety
/// `p` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_push_u32_le(p: *mut RtBytes, value: i64) {
    bytes_vec(p).extend_from_slice(&(value as u32).to_le_bytes());
}

/// # Safety
/// `p` は有効。`out` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_read_u32_be(
    p: *const RtBytes,
    offset: i64,
    out: *mut i64,
) -> i32 {
    if offset >= 0 && (offset as usize + 4) <= bytes_slice(p).len() {
        let off = offset as usize;
        let bytes: [u8; 4] = bytes_slice(p)[off..off + 4].try_into().unwrap();
        *out = u32::from_be_bytes(bytes) as i64;
        1
    } else {
        0
    }
}

/// # Safety
/// `a`・`b_ptr` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_bytes_ct_eq(a: *const RtBytes, b_ptr: *const RtBytes) -> i32 {
    let ba = bytes_slice(a);
    let bb = bytes_slice(b_ptr);
    if ba.len() != bb.len() {
        return 0;
    }
    let mut diff = 0u8;
    for (&x, &y) in ba.iter().zip(bb.iter()) {
        diff |= x ^ y;
    }
    if diff == 0 {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn sigil_rt_int_rotl(x: i64, n: i64, width: i64) -> i64 {
    if width == 32 {
        (x as u32).rotate_left((n % 32) as u32) as i64
    } else {
        (x as u64).rotate_left((n % 64) as u32) as i64
    }
}

#[no_mangle]
pub extern "C" fn sigil_rt_int_rotr(x: i64, n: i64, width: i64) -> i64 {
    if width == 32 {
        (x as u32).rotate_right((n % 32) as u32) as i64
    } else {
        (x as u64).rotate_right((n % 64) as u32) as i64
    }
}

#[no_mangle]
pub extern "C" fn sigil_rt_int_rotr32(x: i64, n: i64) -> i64 {
    (x as u32).rotate_right((n % 32) as u32) as i64
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
    live_blocks(1);
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
    live_blocks(-1);
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
        str_free(e);
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
    let code = match h {
        Ok(h) => h.join().unwrap_or(1),
        Err(_) => 1,
    };
    // テスト用: 環境変数 `SIGIL_RT_LIVE` があれば、終わった時点で生きている塊の数を標準エラーに出す。
    if std::env::var_os("SIGIL_RT_LIVE").is_some() {
        // 確保器を結んだときだけ、既存の書式の末尾に ` sigil_blocks=` を足す（補足 B-5）。
        let extra = match sigil_live_blocks() {
            Some(n) => format!(" sigil_blocks={n}"),
            None => String::new(),
        };
        let _ = writeln!(
            std::io::stderr().lock(),
            "live: strs={} blocks={}{extra}",
            sigil_rt_live_strs(),
            sigil_rt_live_blocks()
        );
    }
    code
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

/// 引数 1 つを読む。誤りなら null を返し、`*err` に説明（`str_free` で解放）を書く。
///
/// # Safety
/// `ptr` は `len` バイト読める。`err` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_arg_parse(
    ptr: *const u8,
    len: usize,
    err: *mut *mut RtStr,
) -> *mut RtArg {
    let src = str_new_raw(ptr, len);
    let r = args::parse_arg(s(src));
    str_free(src);
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
pub(crate) mod tests {
    use super::*;

    /// テスト用の関数表。頭は Rust の確保（ptr・len・cap の 24B）で、本物（Sigil 製）と同じ配置にする。
    pub(crate) mod table {
        use super::super::*;
        use std::sync::Once;

        #[repr(C)]
        struct Head {
            ptr: *mut u8,
            len: i64,
            cap: i64,
        }

        pub(crate) extern "C" fn new(p: *const u8, n: i64) -> *mut RtStr {
            let v: Vec<u8> = if n == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec()
            };
            let mut b = v.into_boxed_slice();
            let ptr = if n == 0 {
                std::ptr::null_mut()
            } else {
                b.as_mut_ptr()
            };
            std::mem::forget(b);
            Box::into_raw(Box::new(Head {
                ptr,
                len: n,
                cap: n,
            })) as *mut RtStr
        }

        extern "C" fn free(p: *mut RtStr) {
            unsafe {
                let h = Box::from_raw(p as *mut Head);
                if h.cap > 0 {
                    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
                        h.ptr,
                        h.cap as usize,
                    )));
                }
            }
        }

        extern "C" fn clone(p: *const RtStr) -> *mut RtStr {
            unsafe {
                let h = &*(p as *const Head);
                new(h.ptr, h.len)
            }
        }

        extern "C" fn data(p: *const RtStr) -> *const u8 {
            unsafe { (*(p as *const Head)).ptr }
        }

        extern "C" fn bytes(p: *const RtStr) -> i64 {
            unsafe { (*(p as *const Head)).len }
        }

        pub fn init() {
            static ONCE: Once = Once::new();
            ONCE.call_once(|| sigil_rt_set_str_ops(new, free, clone, data, bytes));
        }
    }

    unsafe fn st(s: &str) -> *mut RtStr {
        str_new_raw(s.as_ptr(), s.len())
    }

    unsafe fn text(p: *mut RtStr) -> String {
        let t = s(p).to_string();
        str_free(p);
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
            let q = st("a\"\n");
            assert_eq!(text(sigil_rt_show_str(q)), "\"a\\\"\\n\"");
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

    #[test]
    fn bytes_operations() {
        unsafe {
            let b = sigil_rt_bytes_new();
            assert_eq!(sigil_rt_bytes_len(b), 0);
            assert_eq!(sigil_rt_bytes_is_empty(b), 1);
            sigil_rt_bytes_push(b, 0x12);
            sigil_rt_bytes_push_u16_be(b, 0x3456);
            assert_eq!(sigil_rt_bytes_len(b), 3);
            let mut val = 0i64;
            assert_eq!(sigil_rt_bytes_get(b, 0, &mut val), 1);
            assert_eq!(val, 0x12);
            let mut read_u32 = 0i64;
            assert_eq!(sigil_rt_bytes_read_u32_be(b, 0, &mut read_u32), 0);
            sigil_rt_bytes_push(b, 0x78);
            assert_eq!(sigil_rt_bytes_read_u32_be(b, 0, &mut read_u32), 1);
            assert_eq!(read_u32, 0x12345678);

            let b2 = sigil_rt_bytes_clone(b);
            assert_eq!(sigil_rt_bytes_ct_eq(b, b2), 1);

            let mut out = std::ptr::null_mut();
            let mut err = std::ptr::null_mut();
            assert_eq!(sigil_rt_bytes_slice(b, 1, 3, &mut out, &mut err), 1);
            assert_eq!(sigil_rt_bytes_len(out), 2);
            sigil_rt_bytes_free(out);

            sigil_rt_bytes_free(b);
            sigil_rt_bytes_free(b2);
        }
    }

    /// 子プロセスで同じテストを走らせ、環境変数の指示のときだけ異常な使い方をして、終了コード 70 を確かめる。
    fn exits_70_when(mode: &str, name: &str) {
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env("SIGIL_TEST_STR_MODE", mode)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(70), "{:?}", out.status);
    }

    /// 関数表が未登録のまま文字列を作ると、終了コード 70 で止まる。
    #[test]
    fn unregistered_table_exits_70() {
        match std::env::var("SIGIL_TEST_STR_MODE").as_deref() {
            Ok("unregistered") => {
                let _ = str_new_raw(b"a".as_ptr(), 1);
            }
            _ => exits_70_when("unregistered", "tests::unregistered_table_exits_70"),
        }
    }

    /// 中身が不正な UTF-8 の文字列を `s` が見ると、終了コード 70 で止まる。
    #[test]
    fn invalid_utf8_exits_70() {
        match std::env::var("SIGIL_TEST_STR_MODE").as_deref() {
            Ok("invalid") => unsafe {
                table::init();
                let p = table::new(b"\xFF\xFE".as_ptr(), 2);
                let _ = s(p);
            },
            _ => exits_70_when("invalid", "tests::invalid_utf8_exits_70"),
        }
    }
}

//! Sigil の暫定ランタイムの核（ADR 0015 の決定 6）。
//!
//! 確保と解放、`String`・`List`・`Shared` の操作、スタックの下限、入口の引数の読み取り。
//! OS の機能（`console.log`・`fs`・`Path`）は `runtime/os` にある。関数の名前は `sigil_rt_` で始め、`extern "C"` とする。
//! 境界は ADR 0014 の決定 2 の宣言で、Sigil のプログラムから名前で呼ぶ手段は作らない（ADR 0006 の「境界」）。
//!
//! 所有の約束: `*mut` で返された値は呼び出し側のもので、対の `..._free` で解放する。`*const` で受け取る値は借用。
//! 生成したコードは正しい番地だけを渡す前提で、関数は `unsafe` である。

use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

#[path = "../../../src/shared/args.rs"]
mod args;
mod val;

/// 入口を実行するスレッドのスタックの大きさ（SIR の評価器と同じ）。
pub const STACK_BYTES: usize = 256 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 確保と解放（ADR 0014 の決定 2: 確保器は環境が供給する）

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
// 文字列・一覧・バイト列・組・構造体などの塊は、すべて Sigil 製の確保器が持つ。漏れ・二重解放は、終了時の
// `live: sigil_blocks=<数>` の厳密一致で見る（補足 G-7。以前の `strs=`・`blocks=` は、塊が確保器へ移って常に 0 になったので外した）。

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

// ---------------------------------------------------------------------------
// Bytes（24B の頭の塊。表現は Sigil 製の `std.core.bytes` が持つ。補足 F。バイト列は 1 バイトずつの領域）

/// 24B の頭（buf・len・cap）の塊。Rust 側は中身を知らず、登録された関数表を通してだけ触る。
#[repr(C)]
pub struct RtBytes {
    _p: [u8; 0],
}

type BytesFromRaw = extern "C" fn(*const u8, i64) -> *mut RtBytes;
type BytesFree = extern "C" fn(*mut RtBytes);
type BytesClone = extern "C" fn(*const RtBytes) -> *mut RtBytes;
type BytesData = extern "C" fn(*const RtBytes) -> *const u8;
type BytesLen = extern "C" fn(*const RtBytes) -> i64;

/// バイト列の関数表（from_raw・free・clone・data・len の関数ポインタ。0 は未登録）。
static BYTES_OPS: [AtomicUsize; 5] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// 生成コードの `main` が、入口を呼ぶ前にバイト列の関数表を登録する（補足 F-4）。
#[no_mangle]
pub extern "C" fn sigil_rt_set_bytes_ops(
    from_raw: BytesFromRaw,
    free: BytesFree,
    clone: BytesClone,
    data: BytesData,
    len: BytesLen,
) {
    BYTES_OPS[0].store(from_raw as usize, Ordering::Relaxed);
    BYTES_OPS[1].store(free as usize, Ordering::Relaxed);
    BYTES_OPS[2].store(clone as usize, Ordering::Relaxed);
    BYTES_OPS[3].store(data as usize, Ordering::Relaxed);
    BYTES_OPS[4].store(len as usize, Ordering::Relaxed);
}

/// 関数表の `i` 番目。未登録なら異常停止（exit_group(70) と同じ終了コード）。
fn bytes_op(i: usize) -> usize {
    #[cfg(test)]
    if std::env::var("SIGIL_TEST_STR_MODE").as_deref() != Ok("unregistered") {
        tests::table::init();
    }
    let p = BYTES_OPS[i].load(Ordering::Relaxed);
    if p == 0 {
        eprintln!("sigil: バイト列の関数表が未登録のままバイト列を使った");
        std::process::exit(70);
    }
    p
}

// SAFETY（以下の 5 つ）: `bytes_op` が返す値は `sigil_rt_set_bytes_ops` が受け取った、対応する型の関数ポインタだけ。
/// `p` から `n` バイトを写した Bytes を作る。
#[cfg(test)]
pub(crate) fn bytes_from_raw(p: *const u8, n: usize) -> *mut RtBytes {
    let f: BytesFromRaw = unsafe { std::mem::transmute(bytes_op(0)) };
    f(p, n as i64)
}

fn bytes_data(b: *const RtBytes) -> *const u8 {
    let f: BytesData = unsafe { std::mem::transmute(bytes_op(3)) };
    f(b)
}

pub(crate) fn bytes_len(b: *const RtBytes) -> usize {
    let f: BytesLen = unsafe { std::mem::transmute(bytes_op(4)) };
    f(b) as usize
}

// ---------------------------------------------------------------------------
// List（24B の頭の塊。表現は Sigil 製の `std.core.list` が持つ。補足 E。要素は 8B の語）

/// 24B の頭（buf・len・cap）の塊。Rust 側は中身を知らず、登録された関数表を通してだけ触る。
#[repr(C)]
pub struct RtList {
    _p: [u8; 0],
}

type ListNew = extern "C" fn(usize) -> *mut RtList;
type ListFree = extern "C" fn(*mut RtList);
type ListPush = extern "C" fn(*mut RtList, *const u8);
type ListGet = extern "C" fn(*const RtList, i64) -> *mut u8;
type ListLen = extern "C" fn(*const RtList) -> i64;

/// 一覧の関数表（create・free・push・get・len の関数ポインタ。0 は未登録）。
static LIST_OPS: [AtomicUsize; 5] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

/// 生成コードの `main` が、入口を呼ぶ前に一覧の関数表を登録する（補足 E-3）。
#[no_mangle]
pub extern "C" fn sigil_rt_set_list_ops(
    new: ListNew,
    free: ListFree,
    push: ListPush,
    get: ListGet,
    len: ListLen,
) {
    LIST_OPS[0].store(new as usize, Ordering::Relaxed);
    LIST_OPS[1].store(free as usize, Ordering::Relaxed);
    LIST_OPS[2].store(push as usize, Ordering::Relaxed);
    LIST_OPS[3].store(get as usize, Ordering::Relaxed);
    LIST_OPS[4].store(len as usize, Ordering::Relaxed);
}

/// 関数表の `i` 番目。未登録なら異常停止（exit_group(70) と同じ終了コード）。
fn list_op(i: usize) -> usize {
    #[cfg(test)]
    if std::env::var("SIGIL_TEST_STR_MODE").as_deref() != Ok("unregistered") {
        tests::table::init();
    }
    let p = LIST_OPS[i].load(Ordering::Relaxed);
    if p == 0 {
        eprintln!("sigil: 一覧の関数表が未登録のまま一覧を使った");
        std::process::exit(70);
    }
    p
}

// SAFETY（以下の 5 つ）: `list_op` が返す値は `sigil_rt_set_list_ops` が受け取った、対応する型の関数ポインタだけ。
/// 空の一覧を作る（要素は 8B の語）。
pub(crate) fn list_new() -> *mut RtList {
    let f: ListNew = unsafe { std::mem::transmute(list_op(0)) };
    f(8)
}

/// 語 `w` を末尾へ写す。
pub(crate) fn list_push(l: *mut RtList, w: i64) {
    let f: ListPush = unsafe { std::mem::transmute(list_op(2)) };
    f(l, &w as *const i64 as *const u8)
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
        // 確保器の生存数が登録されているときだけ出す。未登録なら行を出さない（偽の 0 を出さない。補足 G-7。
        // 3-7a からすべてのネイティブのプログラムが確保器を結ぶので、通常は常に出る）。
        if let Some(n) = sigil_live_blocks() {
            let _ = writeln!(std::io::stderr().lock(), "live: sigil_blocks={n}");
        }
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

    /// 中身の読み取り（単体試験用）。長さ 0 では `data` を読まない（空番地のことがある）。
    unsafe fn bytes_slice<'a>(p: *const RtBytes) -> &'a [u8] {
        let n = bytes_len(p);
        if n == 0 {
            return &[];
        }
        std::slice::from_raw_parts(bytes_data(p), n)
    }

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

        /// 一覧の頭（buf・len・cap の 24B。本物と同じ配置）。要素の領域は 8 バイト境界の Rust の確保。
        #[repr(C)]
        struct LHead {
            buf: *mut u8,
            len: i64,
            cap: i64,
        }

        extern "C" fn l_new(elem: usize) -> *mut RtList {
            assert_eq!(elem, 8);
            Box::into_raw(Box::new(LHead {
                buf: std::ptr::null_mut(),
                len: 0,
                cap: 0,
            })) as *mut RtList
        }

        extern "C" fn l_free(l: *mut RtList) {
            unsafe {
                let h = Box::from_raw(l as *mut LHead);
                if h.cap > 0 {
                    drop(Vec::from_raw_parts(h.buf as *mut i64, 0, h.cap as usize));
                }
            }
        }

        extern "C" fn l_push(l: *mut RtList, e: *const u8) {
            unsafe {
                let h = &mut *(l as *mut LHead);
                let w = *(e as *const i64);
                let mut v = if h.cap > 0 {
                    Vec::from_raw_parts(h.buf as *mut i64, h.len as usize, h.cap as usize)
                } else {
                    Vec::new()
                };
                v.push(w);
                h.buf = v.as_mut_ptr() as *mut u8;
                h.len = v.len() as i64;
                h.cap = v.capacity() as i64;
                std::mem::forget(v);
            }
        }

        extern "C" fn l_get(l: *const RtList, i: i64) -> *mut u8 {
            unsafe {
                let h = &*(l as *const LHead);
                if i < 0 || i >= h.len {
                    return std::ptr::null_mut();
                }
                h.buf.add(i as usize * 8)
            }
        }

        extern "C" fn l_len(l: *const RtList) -> i64 {
            unsafe { (*(l as *const LHead)).len }
        }

        /// バイト列の頭（buf・len・cap の 24B。本物と同じ配置）。領域は Rust の確保（cap バイト）。
        #[repr(C)]
        struct BHead {
            buf: *mut u8,
            len: i64,
            cap: i64,
        }

        extern "C" fn b_from_raw(p: *const u8, n: i64) -> *mut RtBytes {
            let (buf, cap) = if n > 0 {
                let mut v = unsafe { std::slice::from_raw_parts(p, n as usize) }.to_vec();
                v.shrink_to_fit();
                let b = v.as_mut_ptr();
                let c = v.capacity() as i64;
                std::mem::forget(v);
                (b, c)
            } else {
                (std::ptr::null_mut(), 0)
            };
            Box::into_raw(Box::new(BHead {
                buf,
                len: n.max(0),
                cap,
            })) as *mut RtBytes
        }

        extern "C" fn b_free(b: *mut RtBytes) {
            unsafe {
                let h = Box::from_raw(b as *mut BHead);
                if h.cap > 0 {
                    drop(Vec::from_raw_parts(h.buf, 0, h.cap as usize));
                }
            }
        }

        extern "C" fn b_clone(b: *const RtBytes) -> *mut RtBytes {
            unsafe {
                let h = &*(b as *const BHead);
                b_from_raw(h.buf, h.len)
            }
        }

        extern "C" fn b_data(b: *const RtBytes) -> *const u8 {
            unsafe { (*(b as *const BHead)).buf }
        }

        extern "C" fn b_len(b: *const RtBytes) -> i64 {
            unsafe { (*(b as *const BHead)).len }
        }

        pub fn init() {
            static ONCE: Once = Once::new();
            ONCE.call_once(|| {
                sigil_rt_set_str_ops(new, free, clone, data, bytes);
                sigil_rt_set_list_ops(l_new, l_free, l_push, l_get, l_len);
                sigil_rt_set_bytes_ops(b_from_raw, b_free, b_clone, b_data, b_len);
            });
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
    fn bytes_through_table() {
        type BytesFree = extern "C" fn(*mut RtBytes);
        type BytesClone = extern "C" fn(*const RtBytes) -> *mut RtBytes;
        unsafe {
            // 複写と解放は関数表の Sigil の関数を直接呼ぶ（Rust の包みは持たない。補足 G-6）。
            let clone: BytesClone = std::mem::transmute(bytes_op(2));
            let free: BytesFree = std::mem::transmute(bytes_op(1));
            let e = bytes_from_raw(std::ptr::null(), 0);
            assert_eq!(bytes_len(e), 0);
            assert_eq!(bytes_slice(e), b"");
            let b = bytes_from_raw(b"abc".as_ptr(), 3);
            assert_eq!(bytes_slice(b), b"abc");
            let c = clone(b);
            assert_eq!(bytes_slice(c), b"abc");
            free(b);
            assert_eq!(bytes_slice(c), b"abc");
            for x in [e, c] {
                free(x);
            }
        }
    }

    /// バイト列の関数表が未登録のままバイト列を使うと、終了コード 70 で止まる。
    #[test]
    fn unregistered_bytes_table_exits_70() {
        match std::env::var("SIGIL_TEST_STR_MODE").as_deref() {
            Ok("unregistered") => {
                let _ = bytes_from_raw(std::ptr::null(), 0);
            }
            _ => {
                let err = exits_70_when("unregistered", "tests::unregistered_bytes_table_exits_70");
                assert!(err.contains("バイト列の関数表が未登録"), "{err}");
            }
        }
    }

    /// 子プロセスで同じテストを走らせ、環境変数の指示のときだけ異常な使い方をして、終了コード 70 を確かめる。
    fn exits_70_when(mode: &str, name: &str) -> String {
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env("SIGIL_TEST_STR_MODE", mode)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(70), "{:?}", out.status);
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    /// 関数表が未登録のまま文字列を作ると、終了コード 70 で止まる。
    #[test]
    fn unregistered_table_exits_70() {
        match std::env::var("SIGIL_TEST_STR_MODE").as_deref() {
            Ok("unregistered") => {
                let _ = str_new_raw(b"a".as_ptr(), 1);
            }
            _ => {
                exits_70_when("unregistered", "tests::unregistered_table_exits_70");
            }
        }
    }

    /// 一覧の関数表が未登録のまま一覧を作ると、終了コード 70 で止まる。
    #[test]
    fn unregistered_list_table_exits_70() {
        match std::env::var("SIGIL_TEST_STR_MODE").as_deref() {
            Ok("unregistered") => {
                let _ = list_new();
            }
            _ => {
                let err = exits_70_when("unregistered", "tests::unregistered_list_table_exits_70");
                // 別の理由の終了 70 と取り違えない（一覧の表の未登録の報告であること）。
                assert!(err.contains("一覧の関数表が未登録"), "{err}");
            }
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
            _ => {
                exits_70_when("invalid", "tests::invalid_utf8_exits_70");
            }
        }
    }
}

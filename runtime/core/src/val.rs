//! 型の記述子（descriptor）にもとづく値の表示・比較・複写（ADR 0015 の決定 4・6、PR4）。
//!
//! 生成したコードの値は、どれも 8 バイトの語である。`Int`・`Bool` はそのまま、`Float` は 64 ビットの並び、
//! `String`・`Path` は `RtStr*`、一覧は `RtList*`（要素は語）、組・構造体は語の並びの塊（フィールドは `8*i`）、
//! 列挙型は `[腕の番号][フィールド…]`（フィールドは `8+8*i`、塊は少なくとも 16 バイト）、
//! `Cell<T>`・`Shared<T>` は中身の語を持つ 8 バイトの塊への番地、参照は指す先の語の番地（参照の参照も 1 つの番地）、
//! 関数の値は `[関数の番地][環境（なければ 0）]` の塊への番地である。
//!
//! 型ごとの形は、生成したコードが読み取り専用のデータとして持つ記述子（語の並び）が表す:
//!
//! ```text
//! [0]=種類  0 Unit  1 Int  2 Float  3 Bool  4 String  5 Path  6 ParseError  14 Fn
//! 7  List    [1]=要素の記述子
//! 8  Tuple   [1]=n, [2..]=要素の記述子
//! 9  Struct  [1]=名前の番地 [2]=名前の長さ [3]=n, 続いてフィールドごとに (名前の番地, 名前の長さ, 記述子)
//! 10 Enum    [1]=名前の番地 [2]=名前の長さ [3]=接頭辞なし(Option・Result) [4]=腕の数,
//!            続いて腕ごとに (名前の番地, 名前の長さ, フィールド数, 記述子…)
//! 11 Ref  12 Shared  13 Cell   [1]=中身の記述子
//! ```
//!
//! 表示・等しさ・順序・複写の規則は SIR の評価器（`src/ir_eval.rs`）と同じ。

use super::{
    new_str, s, sigil_rt_alloc, sigil_rt_list_get, sigil_rt_list_len, sigil_rt_list_new,
    sigil_rt_list_push, text, RtList, RtStr,
};
use crate::args::{arg_name, parse_arg, Arg};
use std::cmp::Ordering;
use std::io::Write;

const K_UNIT: i64 = 0;
const K_INT: i64 = 1;
const K_FLOAT: i64 = 2;
const K_BOOL: i64 = 3;
const K_STR: i64 = 4;
const K_PATH: i64 = 5;
const K_PARSE_ERROR: i64 = 6;
const K_LIST: i64 = 7;
const K_TUPLE: i64 = 8;
const K_STRUCT: i64 = 9;
const K_ENUM: i64 = 10;
const K_REF: i64 = 11;
const K_SHARED: i64 = 12;
const K_CELL: i64 = 13;

type D = *const i64;

unsafe fn rd(d: D, i: usize) -> i64 {
    *d.add(i)
}

unsafe fn dp(d: D, i: usize) -> D {
    rd(d, i) as D
}

unsafe fn name<'a>(d: D, i: usize) -> &'a str {
    let p = rd(d, i) as *const u8;
    let n = rd(d, i + 1) as usize;
    if n == 0 || p.is_null() {
        return "";
    }
    std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, n))
}

unsafe fn word(p: *const u8) -> i64 {
    *(p as *const i64)
}

unsafe fn list_items(l: *const RtList) -> Vec<i64> {
    let n = sigil_rt_list_len(l);
    (0..n)
        .map(|i| word(sigil_rt_list_get(l, i) as *const u8))
        .collect()
}

/// 列挙型の腕の項（名前、フィールド数、フィールドの記述子の位置）と、次の腕の位置。
unsafe fn variant(d: D, tag: i64) -> (&'static str, usize, usize) {
    let mut pos = 5usize;
    let n = rd(d, 4);
    let mut i = 0;
    loop {
        let nf = rd(d, pos + 2) as usize;
        if i == tag || i + 1 >= n {
            return (name(d, pos), nf, pos + 3);
        }
        pos += 3 + nf;
        i += 1;
    }
}

unsafe fn show_into(d: D, w: i64, quote: bool, out: &mut String) {
    match rd(d, 0) {
        K_UNIT => out.push_str("()"),
        K_INT => out.push_str(&w.to_string()),
        K_FLOAT => out.push_str(&text::fmt_float(f64::from_bits(w as u64))),
        K_BOOL => out.push_str(if w != 0 { "true" } else { "false" }),
        K_STR | K_PATH => {
            let t = s(w as *const RtStr);
            if quote {
                out.push_str(&text::quote(t));
            } else {
                out.push_str(t);
            }
        }
        K_PARSE_ERROR => out.push_str("ParseError"),
        K_LIST => {
            out.push('[');
            for (i, x) in list_items(w as *const RtList).into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                show_into(dp(d, 1), x, true, out);
            }
            out.push(']');
        }
        K_TUPLE => {
            let n = rd(d, 1) as usize;
            let b = w as *const i64;
            out.push('(');
            for i in 0..n {
                if i > 0 {
                    out.push_str(", ");
                }
                show_into(dp(d, 2 + i), *b.add(i), true, out);
            }
            out.push(')');
        }
        K_STRUCT => {
            let n = rd(d, 3) as usize;
            let b = w as *const i64;
            out.push_str(name(d, 1));
            out.push_str(" { ");
            for i in 0..n {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(name(d, 4 + 3 * i));
                out.push_str(": ");
                show_into(dp(d, 4 + 3 * i + 2), *b.add(i), true, out);
            }
            out.push_str(" }");
        }
        K_ENUM => {
            let b = w as *const i64;
            let (vname, nf, fpos) = variant(d, *b);
            if rd(d, 3) == 0 {
                out.push_str(name(d, 1));
                out.push('.');
            }
            out.push_str(vname);
            if nf > 0 {
                out.push('(');
                for i in 0..nf {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    show_into(dp(d, fpos + i), *b.add(1 + i), true, out);
                }
                out.push(')');
            }
        }
        K_REF => show_into(dp(d, 1), *(w as *const i64), quote, out),
        K_SHARED => {
            out.push_str("Shared(");
            show_into(dp(d, 1), *(w as *const i64), true, out);
            out.push(')');
        }
        K_CELL => {
            out.push_str("Cell(");
            show_into(dp(d, 1), *(w as *const i64), true, out);
            out.push(')');
        }
        _ => out.push_str("<関数>"),
    }
}

/// 値の表示（`quote` が真なら文字列に引用符を付ける）。戻り値は `sigil_rt_str_free` で解放する。
///
/// # Safety
/// `d` は記述子、`w` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_val_show(d: D, w: i64, quote: i32) -> *mut RtStr {
    let mut o = String::new();
    show_into(d, w, quote != 0, &mut o);
    new_str(o)
}

unsafe fn eq(d: D, a: i64, b: i64) -> bool {
    match rd(d, 0) {
        K_UNIT | K_PARSE_ERROR => true,
        K_INT | K_BOOL => a == b,
        K_FLOAT => f64::from_bits(a as u64) == f64::from_bits(b as u64),
        K_STR | K_PATH => s(a as *const RtStr) == s(b as *const RtStr),
        K_LIST => {
            let (x, y) = (list_items(a as *const RtList), list_items(b as *const RtList));
            x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| eq(dp(d, 1), *p, *q))
        }
        K_TUPLE => {
            let n = rd(d, 1) as usize;
            let (x, y) = (a as *const i64, b as *const i64);
            (0..n).all(|i| eq(dp(d, 2 + i), *x.add(i), *y.add(i)))
        }
        K_STRUCT => {
            let n = rd(d, 3) as usize;
            let (x, y) = (a as *const i64, b as *const i64);
            (0..n).all(|i| eq(dp(d, 4 + 3 * i + 2), *x.add(i), *y.add(i)))
        }
        K_ENUM => {
            let (x, y) = (a as *const i64, b as *const i64);
            if *x != *y {
                return false;
            }
            let (_, nf, fpos) = variant(d, *x);
            (0..nf).all(|i| eq(dp(d, fpos + i), *x.add(1 + i), *y.add(1 + i)))
        }
        K_REF => eq(dp(d, 1), *(a as *const i64), *(b as *const i64)),
        K_SHARED | K_CELL => {
            a == b || eq(dp(d, 1), *(a as *const i64), *(b as *const i64))
        }
        _ => false,
    }
}

/// 構造的な等しさ（1 なら等しい）。
///
/// # Safety
/// `d` は記述子、`a`・`b` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_val_eq(d: D, a: i64, b: i64) -> i32 {
    eq(d, a, b) as i32
}

unsafe fn cmp(d: D, a: i64, b: i64) -> Option<Ordering> {
    match rd(d, 0) {
        K_INT | K_BOOL => Some(a.cmp(&b)),
        K_FLOAT => f64::from_bits(a as u64).partial_cmp(&f64::from_bits(b as u64)),
        K_STR => Some(s(a as *const RtStr).cmp(s(b as *const RtStr))),
        K_LIST => {
            let (x, y) = (list_items(a as *const RtList), list_items(b as *const RtList));
            for (p, q) in x.iter().zip(&y) {
                match cmp(dp(d, 1), *p, *q)? {
                    Ordering::Equal => {}
                    o => return Some(o),
                }
            }
            Some(x.len().cmp(&y.len()))
        }
        K_TUPLE => {
            let n = rd(d, 1) as usize;
            let (x, y) = (a as *const i64, b as *const i64);
            for i in 0..n {
                match cmp(dp(d, 2 + i), *x.add(i), *y.add(i))? {
                    Ordering::Equal => {}
                    o => return Some(o),
                }
            }
            Some(Ordering::Equal)
        }
        K_REF => cmp(dp(d, 1), *(a as *const i64), *(b as *const i64)),
        _ => None,
    }
}

/// 構造的な順序（-1・0・1。比べられない値は 2）。
///
/// # Safety
/// `d` は記述子、`a`・`b` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_val_cmp(d: D, a: i64, b: i64) -> i32 {
    match cmp(d, a, b) {
        Some(o) => o as i32,
        None => 2,
    }
}

unsafe fn block(n: usize) -> *mut i64 {
    sigil_rt_alloc(8 * n.max(1), 8) as *mut i64
}

/// `deep` が真なら `clone`（`Cell` は独立した複製）、偽なら `Copy`（共有するものは共有のまま）。
unsafe fn dup(d: D, w: i64, deep: bool) -> i64 {
    match rd(d, 0) {
        K_LIST => {
            let l = sigil_rt_list_new(8);
            for x in list_items(w as *const RtList) {
                let y = dup(dp(d, 1), x, deep);
                sigil_rt_list_push(l, &y as *const i64 as *const u8);
            }
            l as i64
        }
        K_TUPLE => {
            let n = rd(d, 1) as usize;
            let (x, o) = (w as *const i64, block(n));
            for i in 0..n {
                *o.add(i) = dup(dp(d, 2 + i), *x.add(i), deep);
            }
            o as i64
        }
        K_STRUCT => {
            let n = rd(d, 3) as usize;
            let (x, o) = (w as *const i64, block(n));
            for i in 0..n {
                *o.add(i) = dup(dp(d, 4 + 3 * i + 2), *x.add(i), deep);
            }
            o as i64
        }
        K_ENUM => {
            let x = w as *const i64;
            let (_, nf, fpos) = variant(d, *x);
            let o = block((1 + nf).max(2));
            *o = *x;
            for i in 0..nf {
                *o.add(1 + i) = dup(dp(d, fpos + i), *x.add(1 + i), deep);
            }
            o as i64
        }
        K_CELL if deep => {
            let o = block(1);
            *o = dup(dp(d, 1), *(w as *const i64), deep);
            o as i64
        }
        _ => w,
    }
}

/// `Copy`: 塊は新しく作り、参照・文字列・`Cell`・`Shared`・関数の値は共有する。
///
/// # Safety
/// `d` は記述子、`w` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_val_shadow(d: D, w: i64) -> i64 {
    dup(d, w, false)
}

/// `clone`: `Cell` も独立した複製にする。
///
/// # Safety
/// `d` は記述子、`w` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_val_clone(d: D, w: i64) -> i64 {
    dup(d, w, true)
}

/// `List<Int>.sum`。足し算があふれたら 1 を返す（空は 0）。
///
/// # Safety
/// `l` は有効。`out` は書ける。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_sum_int(l: *const RtList, out: *mut i64) -> i32 {
    let mut acc = 0i64;
    for x in list_items(l) {
        match acc.checked_add(x) {
            Some(n) => acc = n,
            None => return 1,
        }
    }
    *out = acc;
    0
}

/// `List<Float>.sum`（先頭から順に足す）。
///
/// # Safety
/// `l` は有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_sum_float(l: *const RtList) -> f64 {
    let mut it = list_items(l).into_iter().map(|x| f64::from_bits(x as u64));
    let Some(mut acc) = it.next() else {
        return 0.0;
    };
    for x in it {
        acc += x;
    }
    acc
}

/// `List.min`・`List.max` の位置（一覧は空でない）。同順位は先のもの、比べられない相手は取らない。
///
/// # Safety
/// `l` は有効。`d` は要素の記述子。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_list_best(l: *const RtList, d: D, want_max: i32) -> i64 {
    let xs = list_items(l);
    let mut best = 0usize;
    for i in 1..xs.len() {
        let o = cmp(d, xs[i], xs[best]);
        let better = if want_max != 0 {
            o == Some(Ordering::Greater)
        } else {
            o == Some(Ordering::Less)
        };
        if better {
            best = i;
        }
    }
    best as i64
}

// ---------------------------------------------------------------------------
// 入口の引数（`sigil run` と同じ書き方。宣言の型の記述子に合わせて語にする）

fn kind_name(k: i64) -> &'static str {
    match k {
        K_INT => "Int",
        K_FLOAT => "Float",
        K_BOOL => "Bool",
        K_STR => "String",
        K_PATH => "Path",
        _ => "List",
    }
}

/// 引数を記述子の型の語にする（`interp` の `coerce` と同じ規則）。
unsafe fn coerce(a: &Arg, mut d: D) -> Result<i64, String> {
    while rd(d, 0) == K_REF {
        d = dp(d, 1);
    }
    let k = rd(d, 0);
    let mismatch = || {
        Err(format!(
            "{} が要るが、{}が渡された",
            kind_name(k),
            arg_name(a)
        ))
    };
    match (k, a) {
        (K_INT, Arg::Int(n)) => Ok(*n),
        (K_FLOAT, Arg::Float(x)) => Ok(x.to_bits() as i64),
        (K_FLOAT, Arg::Int(n)) => Ok((*n as f64).to_bits() as i64),
        (K_BOOL, Arg::Bool(b)) => Ok(*b as i64),
        (K_STR | K_PATH, Arg::Str(x)) => Ok(new_str(x.clone()) as i64),
        (K_LIST, Arg::List(xs)) => {
            let l = sigil_rt_list_new(8);
            for x in xs {
                let w = coerce(x, dp(d, 1))?;
                sigil_rt_list_push(l, &w as *const i64 as *const u8);
            }
            Ok(l as i64)
        }
        _ => mismatch(),
    }
}

unsafe fn entry_args(
    argc: i32,
    argv: *const *const std::os::raw::c_char,
    func: &str,
    params: &[(D, &str)],
    out: *mut i64,
) -> Result<(), String> {
    let mut given = Vec::new();
    for i in 1..argc.max(0) as usize {
        let t = std::ffi::CStr::from_ptr(*argv.add(i)).to_string_lossy();
        given.push(parse_arg(&t)?);
    }
    if given.len() != params.len() {
        return Err(format!(
            "{func} は引数を {} 個とる（{} 個渡された）",
            params.len(),
            given.len()
        ));
    }
    for (i, (a, (d, pname))) in given.iter().zip(params).enumerate() {
        match coerce(a, *d) {
            Ok(w) => {
                *out.add(i) = w;
            }
            Err(m) => return Err(format!("引数 {pname} の型が合わない: {m}")),
        }
    }
    Ok(())
}

/// 入口の関数の引数を、コマンドラインから読んで `out[0..nparams]` に語で書く。誤りなら説明を標準エラーに出して 1 を返す。
///
/// `descs` は引数ごとの記述子、`names` は引数ごとの名前の `(番地, 長さ)`。
///
/// # Safety
/// 引数のポインタはすべて有効で、`argv` は `argc` 個の C 文字列。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_entry_args(
    argc: i32,
    argv: *const *const std::os::raw::c_char,
    func_ptr: *const u8,
    func_len: usize,
    nparams: usize,
    descs: *const D,
    names: *const i64,
    out: *mut i64,
) -> i32 {
    let func = if func_len == 0 {
        ""
    } else {
        std::str::from_utf8_unchecked(std::slice::from_raw_parts(func_ptr, func_len))
    };
    let params: Vec<(D, &str)> = (0..nparams)
        .map(|i| {
            let n = *names.add(2 * i + 1) as usize;
            let p = *names.add(2 * i) as *const u8;
            let nm = if n == 0 {
                ""
            } else {
                std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, n))
            };
            (*descs.add(i), nm)
        })
        .collect();
    match entry_args(argc, argv, func, &params, out) {
        Ok(()) => 0,
        Err(m) => {
            let _ = writeln!(std::io::stderr(), "入口の指定が誤り: {m}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_and_compare_primitives() {
        let int = [K_INT];
        let float = [K_FLOAT];
        let list = [K_LIST, int.as_ptr() as i64];
        unsafe {
            assert_eq!(s(sigil_rt_val_show(int.as_ptr(), -5, 1)), "-5");
            assert_eq!(
                s(sigil_rt_val_show(float.as_ptr(), 1.5f64.to_bits() as i64, 1)),
                "1.5"
            );
            let l = sigil_rt_list_new(8);
            for x in [3i64, 1, 2] {
                sigil_rt_list_push(l, &x as *const i64 as *const u8);
            }
            assert_eq!(s(sigil_rt_val_show(list.as_ptr(), l as i64, 1)), "[3, 1, 2]");
            assert_eq!(sigil_rt_list_best(l, int.as_ptr(), 1), 0);
            assert_eq!(sigil_rt_list_best(l, int.as_ptr(), 0), 1);
            let c = sigil_rt_val_shadow(list.as_ptr(), l as i64);
            assert_ne!(c, l as i64);
            assert_eq!(sigil_rt_val_eq(list.as_ptr(), c, l as i64), 1);
            assert_eq!(sigil_rt_val_cmp(list.as_ptr(), c, l as i64), 0);
            let mut out = 0i64;
            assert_eq!(sigil_rt_list_sum_int(l, &mut out), 0);
            assert_eq!(out, 6);
        }
    }
}

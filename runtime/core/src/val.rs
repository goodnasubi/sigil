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
    bytes_slice, list_get, list_len, list_new, list_push, new_str, s, text, RtBytes, RtList, RtStr,
};
use crate::args::{arg_name, parse_arg, Arg};
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
const K_FN: i64 = 14;
const K_BYTES: i64 = 15;

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
    let n = list_len(l);
    (0..n).map(|i| word(list_get(l, i) as *const u8)).collect()
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

/// 破棄の対象になる（見出しつきの塊や列を持つ）種類。
fn owning_kind(k: i64) -> bool {
    matches!(
        k,
        K_LIST | K_TUPLE | K_STRUCT | K_ENUM | K_SHARED | K_CELL | K_FN
    )
}

unsafe fn show_into(d: D, w: i64, quote: bool, out: &mut String) {
    // 移動済み（空）の値は、SIR の評価器と同じく `()` と表示する。
    if w == 0 && (owning_kind(rd(d, 0)) || matches!(rd(d, 0), K_STR | K_PATH | K_BYTES)) {
        out.push_str("()");
        return;
    }
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
        K_BYTES => {
            let bytes = bytes_slice(w as *const RtBytes);
            out.push_str(&format!("Bytes({:?})", bytes));
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

/// 値の表示（`quote` が真なら文字列に引用符を付ける）。戻り値は `str_free` で解放する。
///
/// # Safety
/// `d` は記述子、`w` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_val_show(d: D, w: i64, quote: i32) -> *mut RtStr {
    let mut o = String::new();
    show_into(d, w, quote != 0, &mut o);
    new_str(o)
}

/// `--trace-drops`（補足 G-3）: 構造体の値 `w` を `drop: <表示>` として標準エラーへ 1 行出す。
/// 表示は Rust の中で作る（Sigil 製の文字列を使わない。スタックの下限の付近でも、Sigil の関数を呼ばずに出せる）。
///
/// # Safety
/// `d` は記述子、`w` はその型の値。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_trace_drop(d: D, w: i64) {
    let mut t = String::new();
    show_into(d, w, true, &mut t);
    let _ = writeln!(std::io::stderr().lock(), "drop: {t}");
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

/// 引数を記述子の型の語にする（SIR の評価器の `coerce` と同じ規則）。
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
            let l = list_new();
            for x in xs {
                let w = coerce(x, dp(d, 1))?;
                list_push(l, w);
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
                s(sigil_rt_val_show(
                    float.as_ptr(),
                    1.5f64.to_bits() as i64,
                    1
                )),
                "1.5"
            );
            let l = list_new();
            for x in [3i64, 1, 2] {
                list_push(l, x);
            }
            assert_eq!(
                s(sigil_rt_val_show(list.as_ptr(), l as i64, 1)),
                "[3, 1, 2]"
            );
        }
    }
}

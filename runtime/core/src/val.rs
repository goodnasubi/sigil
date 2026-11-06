//! 型の記述子（descriptor）にもとづく入口の引数の解釈と、`Secret` の中身のゼロ化。
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
//! 記述子を読むのは入口の引数（3-5g まで）だけになった。表示は生成した `sigil_show_<k>` が担い、
//! 等しさ・順序・複写・破棄も生成コードへ移行済み（ADR 0025 補足 G・H）。

use super::{bytes_data, bytes_len, list_new, list_push, new_str, RtBytes};
use crate::args::{arg_name, parse_arg, Arg};
use std::io::Write;

const K_INT: i64 = 1;
const K_FLOAT: i64 = 2;
const K_BOOL: i64 = 3;
const K_STR: i64 = 4;
const K_PATH: i64 = 5;
const K_LIST: i64 = 7;
const K_REF: i64 = 11;

type D = *const i64;

unsafe fn rd(d: D, i: usize) -> i64 {
    *d.add(i)
}

unsafe fn dp(d: D, i: usize) -> D {
    rd(d, i) as D
}

/// `Secret` の中身のゼロ化（ADR 0024、PR 6-1b）: Bytes `b` のデータ領域を
/// ゼロで上書きする。生成された破棄の段（codegen の `sigil_dstep`）が、
/// `Secret` の中身の Bytes を解放する前に呼ぶ。0 は何もしない。
///
/// # Safety
/// `b` は生きている Bytes の値（頭の番地）。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_zeroize_bytes(b: i64) {
    if b == 0 {
        return;
    }
    let head = b as *const RtBytes;
    let data = bytes_data(head) as *mut u8;
    let len = bytes_len(head);
    if !data.is_null() && len > 0 {
        unsafe {
            std::ptr::write_bytes(data, 0, len);
        }
    }
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

    /// 破棄の段が呼ぶゼロ化で、`Secret` の中身のバイト列が消える（ADR 0024、PR 6-1b）。
    /// 表示の秘匿（`Secret(***)`）は生成した表示の試験（`tests/show_35f.rs`）で確かめる。
    #[test]
    fn secret_drop_zeroizes() {
        // テスト用のバイト列の頭（ptr・len・cap の 24B、本物と同じ配置）。
        // cap を 0 にすると、表側の free は領域を解放しないので、
        // 破棄のあとに中身を観察できる。
        #[repr(C)]
        struct Head {
            ptr: *mut u8,
            len: i64,
            cap: i64,
        }
        let mut buf = vec![9u8, 8, 7];
        let head = Box::into_raw(Box::new(Head {
            ptr: buf.as_mut_ptr(),
            len: buf.len() as i64,
            cap: 0,
        }));
        unsafe {
            // 破棄の段が解放前に呼ぶゼロ化（Bytes の頭を渡す）。
            sigil_rt_zeroize_bytes(head as i64);
            assert_eq!(buf, [0, 0, 0], "ゼロ化で中身が消えていない");
            // 0 は何もしない。
            sigil_rt_zeroize_bytes(0);
        }
    }
}

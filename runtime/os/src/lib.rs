//! Sigil の暫定ランタイムの OS 部（ADR 0015 の決定 6）。
//!
//! `console.log`・`fs.read`・`fs.write`・`Path` の操作と `IoError` の分類。`runtime/core` には依存せず、
//! 文字列は `(ptr, len)` の UTF-8 で受け渡す。返す文字列の領域はこの crate の `sigil_rt_os_buf_free` で解放する。
//! `Path` の意味と `IoError` の分類は `src/shared/path.rs` を SIR の評価器と共有する。

use std::io::Write;

#[path = "../../../src/shared/path.rs"]
mod path;

/// # Safety
/// `ptr` は `len` バイト読める（`len` が 0 なら何でもよい）。
unsafe fn text<'a>(ptr: *const u8, len: usize) -> std::borrow::Cow<'a, str> {
    if len == 0 {
        return "".into();
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(ptr, len))
}

/// 文字列を所有する領域として `(ptr, len)` に渡す。
///
/// # Safety
/// `out_ptr`・`out_len` は書ける。
unsafe fn give(s: String, out_ptr: *mut *mut u8, out_len: *mut usize) {
    let b = s.into_bytes().into_boxed_slice();
    *out_len = b.len();
    *out_ptr = Box::into_raw(b) as *mut u8;
}

/// `sigil_rt_os_*` が返した領域を解放する。
///
/// # Safety
/// `(ptr, len)` はこの crate が返したもの。一度だけ。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_os_buf_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }
}

/// `console.log`: 1 行を標準出力に書く（呼び出しごとに書き出す）。
///
/// # Safety
/// `ptr` は `len` バイト読める。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_console_log(ptr: *const u8, len: usize) {
    let bytes = if len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(ptr, len)
    };
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(bytes);
    let _ = o.write_all(b"\n");
    let _ = o.flush();
}

/// `fs.read`: 成功なら 0 を返し `(out_ptr, out_len)` に内容を渡す。失敗なら 1 を返し `out_kind` に
/// `IoError` の腕の番号（0 NotFound, 1 PermissionDenied, 2 Other）を入れる。
///
/// # Safety
/// 引数のポインタはすべて有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_fs_read(
    path_ptr: *const u8,
    path_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
    out_kind: *mut i32,
) -> i32 {
    match std::fs::read_to_string(&*text(path_ptr, path_len)) {
        Ok(s) => {
            give(s, out_ptr, out_len);
            0
        }
        Err(e) => {
            *out_kind = path::io_arm(e.kind()) as i32;
            1
        }
    }
}

/// `fs.write`: 成功なら 0、失敗なら 1 と `out_kind`。
///
/// # Safety
/// 引数のポインタはすべて有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_fs_write(
    path_ptr: *const u8,
    path_len: usize,
    data_ptr: *const u8,
    data_len: usize,
    out_kind: *mut i32,
) -> i32 {
    let data = if data_len == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(data_ptr, data_len)
    };
    match std::fs::write(&*text(path_ptr, path_len), data) {
        Ok(()) => 0,
        Err(e) => {
            *out_kind = path::io_arm(e.kind()) as i32;
            1
        }
    }
}

/// `Path.join`。
///
/// # Safety
/// 引数のポインタはすべて有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_path_join(
    a_ptr: *const u8,
    a_len: usize,
    b_ptr: *const u8,
    b_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) {
    give(
        path::join(&text(a_ptr, a_len), &text(b_ptr, b_len)),
        out_ptr,
        out_len,
    );
}

/// `Path.file_name`: あれば 1 を返して `(out_ptr, out_len)` に渡す。なければ 0。
///
/// # Safety
/// 引数のポインタはすべて有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_path_file_name(
    p_ptr: *const u8,
    p_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    match path::file_name(&text(p_ptr, p_len)) {
        Some(s) => {
            give(s, out_ptr, out_len);
            1
        }
        None => 0,
    }
}

/// `Path.parent`: あれば 1 を返して `(out_ptr, out_len)` に渡す。なければ 0。
///
/// # Safety
/// 引数のポインタはすべて有効。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_path_parent(
    p_ptr: *const u8,
    p_len: usize,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    match path::parent(&text(p_ptr, p_len)) {
        Some(s) => {
            give(s, out_ptr, out_len);
            1
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe fn take(p: *mut u8, n: usize) -> String {
        let t = String::from_utf8(std::slice::from_raw_parts(p, n).to_vec()).unwrap();
        sigil_rt_os_buf_free(p, n);
        t
    }

    #[test]
    fn fs_and_path() {
        unsafe {
            let dir = std::env::temp_dir().join(format!("sigil-rt-os-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let f = dir.join("a.txt").to_string_lossy().to_string();
            let mut kind = -1i32;
            assert_eq!(
                sigil_rt_fs_write(f.as_ptr(), f.len(), b"hi\n".as_ptr(), 3, &mut kind),
                0
            );
            let (mut p, mut n) = (std::ptr::null_mut(), 0usize);
            assert_eq!(
                sigil_rt_fs_read(f.as_ptr(), f.len(), &mut p, &mut n, &mut kind),
                0
            );
            assert_eq!(take(p, n), "hi\n");
            let missing = dir.join("none.txt").to_string_lossy().to_string();
            assert_eq!(
                sigil_rt_fs_read(missing.as_ptr(), missing.len(), &mut p, &mut n, &mut kind),
                1
            );
            assert_eq!(kind, 0);
            // 親が無い所への書き込みは NotFound
            let bad = dir.join("x").join("y.txt").to_string_lossy().to_string();
            assert_eq!(
                sigil_rt_fs_write(bad.as_ptr(), bad.len(), b"".as_ptr(), 0, &mut kind),
                1
            );
            assert_eq!(kind, 0);

            sigil_rt_path_join(
                b"/etc/app".as_ptr(),
                8,
                b"conf.toml".as_ptr(),
                9,
                &mut p,
                &mut n,
            );
            assert_eq!(take(p, n), "/etc/app/conf.toml");
            assert_eq!(sigil_rt_path_file_name(b"/".as_ptr(), 1, &mut p, &mut n), 0);
            assert_eq!(sigil_rt_path_parent(b"/".as_ptr(), 1, &mut p, &mut n), 0);
            assert_eq!(sigil_rt_path_parent(b"/a/b".as_ptr(), 4, &mut p, &mut n), 1);
            assert_eq!(take(p, n), "/a");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

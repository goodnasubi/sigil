//! `Secret` の中身のゼロ化（ADR 0024、PR 6-1b）。
//!
//! 入口の引数の読み取りと型への合わせ込みは生成したコードが担う（ADR 0025 補足 I）。
//! このモジュールに残るのは、生成した破棄の段が `Secret` の中身の Bytes を解放する前に呼ぶ
//! ゼロ化だけである。

use super::{bytes_data, bytes_len, RtBytes};

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

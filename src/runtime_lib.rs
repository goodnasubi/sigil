//! 暫定ランタイムの staticlib（ADR 0015 の決定 6）。`build.rs` が作った `.a` を埋め込む。
//!
//! リンクは `cc -static -no-pie <出力のオブジェクト> core.a [os.a] <native_libs>`（方式 A）。
//! `core.a` と `os.a` はそれぞれ std を持つが、アーカイブは必要なメンバだけ取り込むので、
//! 順序を問わず重複定義にならない（PR3 の計測。PR の記録を参照）。入口の型や標準出力の表示だけで
//! `console` や `fs` を使わない成果物は `core.a` だけでリンクできる。

static CORE_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/core.a"));
static OS_A: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/os.a"));
static NATIVE_LIBS: &str = include_str!(concat!(env!("OUT_DIR"), "/native_libs.txt"));

/// `libsigil_rt_core.a` の中身。
pub fn core_a() -> &'static [u8] {
    CORE_A
}

/// `libsigil_rt_os.a` の中身。
pub fn os_a() -> &'static [u8] {
    OS_A
}

/// `cc` に渡す `-l` の並び（`rustc --print native-static-libs` から `-lgcc_s`・`-lc` を除いたもの）。
pub fn native_libs() -> Vec<&'static str> {
    NATIVE_LIBS.split_whitespace().collect()
}

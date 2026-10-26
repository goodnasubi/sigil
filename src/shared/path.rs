//! `Path` の操作と `fs` の失敗の分類（インタプリタ・評価器・暫定ランタイム `runtime/os` が共有する。ADR 0015 の決定 6）。
//!
//! 意味は `std::path::Path` に従う（実行環境の OS の区切りを使う。ADR 0006 の M4 の記録）。

use std::path::Path;

fn text(x: &Path) -> String {
    x.to_string_lossy().as_ref().to_string()
}

pub fn join(base: &str, other: &str) -> String {
    text(&Path::new(base).join(other))
}

pub fn file_name(p: &str) -> Option<String> {
    Path::new(p).file_name().map(|n| text(n.as_ref()))
}

pub fn parent(p: &str) -> Option<String> {
    Path::new(p).parent().map(text)
}

/// `IoError` の腕の番号（宣言の順。`ir::IO_*` と同じ）。
pub const IO_NOT_FOUND: usize = 0;
pub const IO_PERMISSION_DENIED: usize = 1;
pub const IO_OTHER: usize = 2;

/// `fs` の失敗を `IoError` の腕の番号にする。
pub fn io_arm(kind: std::io::ErrorKind) -> usize {
    match kind {
        std::io::ErrorKind::NotFound => IO_NOT_FOUND,
        std::io::ErrorKind::PermissionDenied => IO_PERMISSION_DENIED,
        _ => IO_OTHER,
    }
}

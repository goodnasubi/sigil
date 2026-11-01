//! `sigil fmt` の書き戻し。読んだときのファイルの状態と、書き戻す直前の状態を照合する。
//! 変わっていれば書かずにエラーにする（読み込みと書き戻しの間の差し替え・追記を上書きで失わない）。

use std::fs::Metadata;
use std::io::{self, Write};
use std::path::Path;
use std::time::SystemTime;

/// 読み込み時のファイルの状態（長さ・更新時刻、Unix では加えて dev・inode）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    id: (u64, u64),
}

impl Stamp {
    fn of(md: &Metadata) -> Stamp {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Stamp {
            len: md.len(),
            modified: md.modified().ok(),
            #[cfg(unix)]
            id: (md.dev(), md.ino()),
        }
    }

    /// `path` の今の状態を控える。**読み込みの前に**呼ぶ（読んでいる間の変更も照合で検知できる）。
    pub fn take(path: &Path) -> io::Result<Stamp> {
        Ok(Stamp::of(&std::fs::metadata(path)?))
    }
}

fn changed() -> io::Error {
    io::Error::other("読み込んだあとにファイルが変わったので書かない（もう一度実行する）")
}

/// 同じディレクトリの一時ファイルに書き、fsync し、`stamp` と照合してから rename で置き換える。
/// パーミッションは元のファイルから引き継ぐ。失敗（照合の不一致を含む）しても元のファイルは変わらない。
pub fn write_atomic_if_unchanged(path: &Path, content: &str, stamp: &Stamp) -> io::Result<()> {
    write_atomic_with_hook(path, content, stamp, &mut || {})
}

/// `write_atomic_if_unchanged` の、一時ファイルを書いたあと・照合の前に `between` を呼ぶ版
/// （読み込みと書き戻しの間の差し替えを試験するための差し込み口）。
#[doc(hidden)]
pub fn write_atomic_with_hook(
    path: &Path,
    content: &str,
    stamp: &Stamp,
    between: &mut dyn FnMut(),
) -> io::Result<()> {
    let target = std::fs::canonicalize(path)?; // シンボリックリンクは先を置き換える
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let perms = std::fs::metadata(&target)?.permissions();
    let tmp = dir.join(format!(".{name}.sigil-fmt-{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(content.as_bytes())?;
        f.set_permissions(perms)?;
        f.sync_all()?;
        drop(f);
        between();
        // rename の直前に照合する（残る隙間は照合から rename までのごく短い間だけ）
        if Stamp::of(&std::fs::metadata(&target)?) != *stamp {
            return Err(changed());
        }
        std::fs::rename(&tmp, &target)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

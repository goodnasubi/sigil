//! テスト差し替え（`--fs-double`、`Options::fs_files`／`fs_bytes`）下の「影の実ファイル層」（ADR 0049 決定 5 と 5.1、Issue #590）。
//!
//! 差し替えの表（パス → 内容）は評価器の内部の状態で、実 OS のファイルディスクリプタを持たない。
//! fs A のハンドル（`OpenOptions.open`、`Dir.open_file`、`File` の排他ロック）は fd を要るので、
//! 差し替え下では**この実行専用の私的な一時ディレクトリ（影の根）**に、表の内容から作った実ファイルを置き、
//! その実ファイルに対して本物のシステムコールを発行する。元の実ファイル（`--fs-double 仮のパス=実ファイル` の右辺）は変更しない。
//!
//! - 影に置くのは、ハンドルから開かれた仮のパスだけ（遅延実体化）。置いた時点で、そのパスの真実は影の実ファイルになり、
//!   表を引く操作（`fs.read`／`fs.write`／バイナリの読み書き／`stat`／`unlink`）も影の実ファイルへ向ける。
//! - 影の根の外へ出る仮のパス（`..` を含むもの）は、解決せずに明示的に拒否する（fail-closed）。
//! - 影の根は実行の終了（`M` の破棄）で再帰的に削除する。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
pub(super) struct Shadow {
    root: Option<PathBuf>,
    /// 影の実ファイルに置いた仮のパス（正規化した構成要素を `/` でつないだもの）。
    files: BTreeSet<String>,
}

impl Shadow {
    /// 影の根（まだ作っていなければ作る）。シンボリックリンクを解決した絶対パス。
    pub(super) fn ensure_root(&mut self) -> std::io::Result<PathBuf> {
        if let Some(r) = &self.root {
            return Ok(r.clone());
        }
        use std::os::unix::fs::DirBuilderExt;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "sigil-fs-double-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let root = std::fs::canonicalize(&dir)?;
        self.root = Some(root.clone());
        Ok(root)
    }

    /// 作成済みの影の根。
    pub(super) fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// 仮のパスを影の実ファイルとして登録する。
    pub(super) fn mark(&mut self, key: String) {
        if !key.is_empty() {
            self.files.insert(key);
        }
    }

    /// `prefix`（末尾に `/` を付けたキー）の下に、実在する影の実ファイルがあるか。
    pub(super) fn has_file_under(&self, prefix: &str) -> bool {
        let Some(root) = &self.root else {
            return false;
        };
        self.files
            .iter()
            .any(|k| k.starts_with(prefix) && root.join(k).exists())
    }

    pub(super) fn is_marked(&self, key: &str) -> bool {
        self.files.contains(key)
    }

    /// 仮のパスが影の実ファイルに置かれていれば、その実際のパス。
    /// 末尾が `/` や `/.` のパス（ディレクトリでなければならない）は、通常ファイルを指すものとして扱わない。
    pub(super) fn path_of(&self, virt: &str) -> Option<PathBuf> {
        if requires_dir(virt) {
            return None;
        }
        let root = self.root.as_ref()?;
        let comps = split(virt)?;
        let key = comps.join("/");
        self.files.contains(&key).then(|| root.join(key))
    }
}

impl Drop for Shadow {
    fn drop(&mut self) {
        if let Some(r) = self.root.take() {
            let _ = std::fs::remove_dir_all(r);
        }
    }
}

/// パスの末尾が「ディレクトリでなければならない」形（`/` で終わる、`.` や `/.` で終わる）か。
/// 構成要素に分けると捨てられる情報なので、別に調べて保つ。
pub(super) fn requires_dir(path: &str) -> bool {
    path.ends_with('/') || path == "." || path.ends_with("/.")
}

/// 仮のパスを構成要素に分ける。空と `.` は捨てる。`..` と NUL を含むものは `None`（影の根の外へ出さない）。
pub(super) fn split(path: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => return None,
            c if c.contains('\0') => return None,
            c => out.push(c.to_string()),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_rejects_escape_and_normalizes() {
        assert_eq!(split("/a//b/./c").unwrap(), vec!["a", "b", "c"]);
        assert_eq!(split("/").unwrap(), Vec::<String>::new());
        assert!(split("/a/../b").is_none());
        assert!(split("..").is_none());
    }

    #[test]
    fn requires_dir_keeps_trailing_directory_syntax() {
        assert!(requires_dir("/v/data.txt/"));
        assert!(requires_dir("/v/data.txt/."));
        assert!(requires_dir("."));
        assert!(requires_dir("/"));
        assert!(!requires_dir("/v/data.txt"));
        assert!(!requires_dir("/v/.hidden"));
        assert!(!requires_dir("/v/a.b"));
    }

    #[test]
    fn shadow_root_is_private_and_removed_on_drop() {
        let root;
        {
            let mut s = Shadow::default();
            root = s.ensure_root().unwrap();
            assert!(root.is_dir());
            s.mark("a/b".into());
            assert_eq!(s.path_of("/a/b").unwrap(), root.join("a/b"));
            assert!(s.path_of("/a/c").is_none());
            assert!(s.path_of("/a/../b").is_none());
        }
        assert!(!root.exists());
    }
}

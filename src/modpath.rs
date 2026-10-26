//! ファイルのパスとモジュール名の対応（ADR 0021 の決定 1・2）。
//!
//! 1 ファイルが 1 モジュール。モジュール名は、根（`--root`）からの相対パスの `/` を `.` に
//! 替え、末尾の `.sigil` を除いたもの。`module` の宣言とこの名前が違えば `ModulePathMismatch`。
//! 根を指定しない呼び出しではこの検査をしない（`sigil` の CLI は `--root` があるときだけ呼ぶ）。

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, ErrorKind, Span};
use crate::lexer::{FUTURE_RESERVED, KEYWORDS};

/// `.` と `..` を字面で畳む（ファイルシステムは見ない。symlink の先に影響されない）。
/// 根の上へ出る `..` は残す（後で根の外として扱う）。
fn normalize(p: &Path) -> PathBuf {
    let mut out: Vec<Component> = vec![];
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) => {}
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    out.iter().collect()
}

fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        normalize(p)
    } else {
        let cwd = std::env::current_dir().unwrap_or_default();
        normalize(&cwd.join(p))
    }
}

fn segment_ok(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_lowercase())
        && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.ends_with('_')
        && !KEYWORDS.contains(&s)
        && !FUTURE_RESERVED.contains(&s)
}

/// 根からの相対パスで決まるモジュール名。モジュールにならないパスは理由を返す。
pub fn module_name_for(root: &Path, file: &Path) -> Result<Vec<String>, String> {
    let root = absolute(root);
    let file = absolute(file);
    let rel = file.strip_prefix(&root).map_err(|_| {
        format!(
            "ファイル `{}` は根 `{}` の外にある（`..` で根の外へ出る参照を含む）",
            file.display(),
            root.display()
        )
    })?;
    if file.extension().and_then(|e| e.to_str()) != Some("sigil") {
        return Err(format!(
            "ファイル `{}` の拡張子が `.sigil` でないので、モジュールではない",
            rel.display()
        ));
    }
    let mut segs: Vec<String> = vec![];
    for c in rel.components() {
        let Component::Normal(os) = c else {
            return Err(format!("パス `{}` に使えない部分がある", rel.display()));
        };
        let Some(s) = os.to_str() else {
            return Err(format!(
                "パス `{}` に文字として読めない部分がある",
                rel.display()
            ));
        };
        segs.push(s.to_string());
    }
    if let Some(last) = segs.last_mut() {
        *last = last.strip_suffix(".sigil").unwrap_or(last).to_string();
    }
    for s in &segs {
        if !segment_ok(s) {
            return Err(format!(
                "パスの部分 `{s}` は識別子（小文字・数字・`_`。語頭は小文字）として使えないので、モジュール名にならない"
            ));
        }
    }
    Ok(segs)
}

/// 実体の確認。名前は見かけの位置（字面で畳んだパス）で決めるが、実体は根の外であってはならない。
/// - 与えられたパスをそのまま OS に解決させた実体（途中のディレクトリの symlink を辿る）が、
///   根の実体の下にあること。
/// - 字面で畳んだパス（`d/link/../top.sigil` は `d/top.sigil`）が同じ実体を指すこと。
///   食い違うなら、`..` が symlink の先で解決されており、見かけの位置と実体が別になっている。
fn real_inside_root(root: &Path, file: &Path) -> Result<(), String> {
    let (Ok(root_real), Ok(raw_real)) = (std::fs::canonicalize(root), std::fs::canonicalize(file))
    else {
        return Err(format!("`{}` の実体を解決できない", file.display()));
    };
    if !raw_real.starts_with(&root_real) {
        return Err(format!(
            "`{}` の実体 `{}` が根 `{}` の外にある（symlink）",
            file.display(),
            raw_real.display(),
            root_real.display()
        ));
    }
    match std::fs::canonicalize(absolute(file)) {
        Ok(norm_real) if norm_real == raw_real => Ok(()),
        _ => Err(format!(
            "`{}` は symlink の先で `..` を辿っており、見かけの位置と実体が違う",
            file.display()
        )),
    }
}

/// 根からファイルまでの各部分で、大小だけが違う名前が同じ場所にないことを確かめる。
fn case_collision(root: &Path, file: &Path) -> Option<String> {
    let root = absolute(root);
    let file = absolute(file);
    let rel = file.strip_prefix(&root).ok()?;
    let mut dir = root;
    for c in rel.components() {
        let Component::Normal(name) = c else { continue };
        let name = name.to_string_lossy().to_string();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let other = e.file_name().to_string_lossy().to_string();
                if other != name && other.to_lowercase() == name.to_lowercase() {
                    return Some(format!(
                        "`{}` と `{}` は大小だけが違う（大小を区別しないファイルシステムで指す先が環境で変わるので、同じ場所に置けない）",
                        dir.join(&name).display(),
                        other
                    ));
                }
            }
        }
        dir.push(name);
    }
    None
}

/// `file`（内容 `src`）が、根 `root` の下で宣言どおりのモジュールか確かめる。
/// 構文エラーならそのエラー。`std`・`std.` で始まる宣言は、パスと一致していても `ReservedModuleName`。
pub fn verify(src: &str, root: &Path, file: &Path) -> Result<(), Error> {
    let f = crate::parse(src)?;
    let span: Span = f.module.span;
    let fail = |kind: ErrorKind, msg: String| Err(Error::new(kind, span, msg));
    // `std`・`std.…` は、宣言とパスが一致していても拒否する（検査器と同じ `ReservedModuleName`）。
    if f.module.name.first().map(String::as_str) == Some("std") {
        return fail(
            ErrorKind::ReservedModuleName,
            format!(
                "モジュール名 `{}` は標準ライブラリの名前（`std`・`std.` で始まる名前は利用者は使えない）",
                f.module.name.join(".")
            ),
        );
    }
    if let Err(m) = real_inside_root(root, file) {
        return fail(ErrorKind::ModulePathMismatch, m);
    }
    if let Some(m) = case_collision(root, file) {
        return fail(ErrorKind::ModulePathCase, m);
    }
    let want = match module_name_for(root, file) {
        Ok(n) => n,
        Err(m) => return fail(ErrorKind::ModulePathMismatch, m),
    };
    if want != f.module.name {
        return fail(
            ErrorKind::ModulePathMismatch,
            format!(
                "パスから決まるモジュール名 `{}` と、`module` の宣言 `{}` が違う",
                want.join("."),
                f.module.name.join(".")
            ),
        );
    }
    Ok(())
}

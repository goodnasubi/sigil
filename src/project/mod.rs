//! 複数モジュールの名前解決と検査（ADR 0021 の決定 3〜5、PR 2-2b）。
//!
//! 入口のファイルから `use` で辿れるモジュールを依存の順に検査する。他のモジュールの宣言は、
//! シグネチャだけを検査器に渡し（本体は見ない）、各モジュールを 1 度だけ検査する。
//! ファイルは `modpath::load` で読む（開いたファイルから読み、同じファイルで実体を確かめる）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::effects::Options;
use crate::error::{Error, ErrorKind};

mod decl;
mod link;
mod load;
mod resolve;

pub use decl::private_in_public;

/// 検査の失敗。診断は、どのファイルのものかを持つ。
#[derive(Debug)]
pub enum ProjectError {
    /// ファイルを開けない・読めない（入口のファイルだけ。`use` の先は `UnresolvedUse`）。
    Read {
        file: PathBuf,
        msg: String,
    },
    Diag {
        file: PathBuf,
        error: Error,
    },
}

type R<T> = Result<T, ProjectError>;

/// `entry` を入口にして、`use` で辿れるすべてのモジュールを検査する。`root` は `--root`。
pub fn check_project(root: &Path, entry: &Path, opts: &Options) -> R<()> {
    check_project_with(root, entry, opts, &mut |_| {})
}

/// `check_project` の、開いている文書の中身を優先する版（言語サーバー用）。`overlay` は、読もうとする
/// ファイルごとに呼ばれ、`Some(中身)` ならディスクの代わりにそれを使う（`None` ならディスクから読む）。
/// パスと名前の対応・根の外の確認は、ディスクのファイルと同じ。`on_text` は、検査が実際に読んだ本文
/// （構文解析に渡したもの）をファイルごとに受け取る（ADR 0022 決定 4: 言語サーバーはこの本文を使い、読み直さない）。
pub fn check_project_overlay(
    root: &Path,
    entry: &Path,
    opts: &Options,
    overlay: &mut dyn FnMut(&Path) -> Option<String>,
    on_text: &mut dyn FnMut(&Path, &str),
) -> R<()> {
    check_project_names(root, entry, opts, overlay, on_text).0
}

/// 入口のモジュールで、型（トレイトを含む）の名前が解決した先（ADR 0022 決定 1。hover が示す）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeName {
    /// 書いた名前（修飾しない）。
    pub name: String,
    /// 解決した先の型を宣言したモジュール。
    pub origin: String,
    /// 明示の名前（自分の宣言・`use`）なら `true`、暗黙の名前（隠れた依存の型）なら `false`。
    pub explicit: bool,
}

/// `check_project_overlay` に、入口のモジュールの型の名前の解決結果を足した版（言語サーバーの hover 用）。
/// 解決結果は、検査が通ったときだけ返す（失敗したときは空）。曖昧な名前は含めない。
pub fn check_project_names(
    root: &Path,
    entry: &Path,
    opts: &Options,
    overlay: &mut dyn FnMut(&Path) -> Option<String>,
    on_text: &mut dyn FnMut(&Path, &str),
) -> (R<()>, Vec<TypeName>) {
    let mut nop = |_: &Path| {};
    match run_fs_with(root, entry, opts, &mut nop, Some(overlay), Some(on_text)) {
        Ok((p, name)) => {
            let names = p.cands.get(&name).map_or_else(Vec::new, |c| c.type_names());
            (Ok(()), names)
        }
        Err(e) => (Err(e), vec![]),
    }
}

/// `check_project` の、各ファイルを読んだ直後（確認の前）に呼ぶ差し込み口つき（差し替えの試験用）。
#[doc(hidden)]
pub fn check_project_with(
    root: &Path,
    entry: &Path,
    opts: &Options,
    on_read: &mut dyn FnMut(&Path),
) -> R<()> {
    run_fs(root, entry, opts, on_read).map(|_| ())
}

fn run_fs<'a>(
    root: &'a Path,
    entry: &Path,
    opts: &'a Options,
    on_read: &'a mut dyn FnMut(&Path),
) -> R<(Proj<'a>, String)> {
    run_fs_with(root, entry, opts, on_read, None, None)
}

type Overlay<'a> = &'a mut dyn FnMut(&Path) -> Option<String>;
type OnText<'a> = &'a mut dyn FnMut(&Path, &str);

fn run_fs_with<'a>(
    root: &'a Path,
    entry: &Path,
    opts: &'a Options,
    on_read: &'a mut dyn FnMut(&Path),
    overlay: Option<Overlay<'a>>,
    on_text: Option<OnText<'a>>,
) -> R<(Proj<'a>, String)> {
    let mut p = Proj {
        src: Src::Fs {
            root,
            on_read,
            overlay,
            on_text,
        },
        opts,
        mods: HashMap::new(),
        order: vec![],
        stack: vec![],
        cands: HashMap::new(),
    };
    let (path, file) = p.load(entry, None)?;
    let name = file.module.name.join(".");
    p.visit(path, file)?;
    Ok((p, name))
}

/// `entry` を入口にして `use` で辿れるモジュールを検査し、1 つのプログラム（`File`）に結ぶ
/// （ADR 0021 の 2-2c。lowering・評価器・ネイティブ出力が受け取る形）。
pub fn link_project(root: &Path, entry: &Path, opts: &Options) -> R<File> {
    let mut nop = |_: &Path| {};
    let (p, name) = run_fs(root, entry, opts, &mut nop)?;
    p.link(&name)
}

/// ソースの並び（先頭が入口のモジュール、残りは入口から `use` で辿られるモジュール。
/// ファイルの位置は見ず、`module` の宣言の名前で引く）を検査して、1 つのプログラムに結ぶ。
/// 辿られないソース、同じモジュール名の宣言が 2 つ以上あるのはエラー。
pub fn link_sources(srcs: &[&str], opts: &Options) -> R<File> {
    let mut files: HashMap<String, (PathBuf, File)> = HashMap::new();
    let mut entry: Option<String> = None;
    for (i, s) in srcs.iter().enumerate() {
        let pseudo = PathBuf::from(format!("<ソース {}>", i + 1));
        let f = crate::parse(s).map_err(|error| ProjectError::Diag {
            file: pseudo.clone(),
            error,
        })?;
        let name = f.module.name.join(".");
        if files.contains_key(&name) {
            return Err(ProjectError::Diag {
                file: pseudo,
                error: Error::new(
                    ErrorKind::ModulePathMismatch,
                    f.module.span,
                    format!("モジュール `{name}` の宣言が 2 つ以上のソースにある（1 つのモジュールは 1 つのソース）"),
                ),
            });
        }
        if i == 0 {
            entry = Some(name.clone());
        }
        files.insert(name, (pseudo, f));
    }
    let Some(entry) = entry else {
        return Err(ProjectError::Read {
            file: PathBuf::new(),
            msg: "ソースがない".into(),
        });
    };
    let (path, file) = files.remove(&entry).expect("入口を入れた");
    let mut p = Proj {
        src: Src::Mem(files),
        opts,
        mods: HashMap::new(),
        order: vec![],
        stack: vec![],
        cands: HashMap::new(),
    };
    p.visit(path, file)?;
    if let Src::Mem(rest) = &p.src {
        if let Some((name, (path, f))) = rest.iter().min_by_key(|(n, _)| n.to_string()) {
            return Err(ProjectError::Diag {
                file: path.clone(),
                error: Error::new(
                    ErrorKind::UnresolvedUse,
                    f.module.span,
                    format!("モジュール `{name}` は入口のモジュールから `use` で辿られない（辿られないソースは渡せない）"),
                ),
            });
        }
    }
    p.link(&entry)
}

enum Src<'a> {
    Fs {
        root: &'a Path,
        on_read: &'a mut dyn FnMut(&Path),
        overlay: Option<Overlay<'a>>,
        on_text: Option<OnText<'a>>,
    },
    /// 渡されたソース（モジュール名 -> 仮の位置と構文木）。辿ったものは取り除く。
    Mem(HashMap<String, (PathBuf, File)>),
}

/// `use` の連鎖の深さの上限（入口を 1 とする。再帰の深さをこれで抑える。ADR 0021 2-2d 追補）。
pub const MAX_USE_DEPTH: usize = 64;
/// 1 回の検査で読むファイル数の上限。
pub const MAX_FILES: usize = 256;

struct Proj<'a> {
    src: Src<'a>,
    opts: &'a Options,
    /// 検査し終えたモジュール（名前 -> 位置と構文木）。
    mods: HashMap<String, (PathBuf, File)>,
    /// 検査し終えた順（依存が先。入口が最後）。
    order: Vec<String>,
    /// 検査中のモジュール（輪の検出）。
    stack: Vec<String>,
    /// 検査し終えたモジュールごとの、名前の候補（ADR 0022 決定 1。リンクが同じ解決を再び使う）。
    cands: HashMap<String, resolve::Cands>,
}

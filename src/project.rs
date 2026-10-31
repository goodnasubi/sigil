//! 複数モジュールの名前解決と検査（ADR 0021 の決定 3〜5、PR 2-2b）。
//!
//! 入口のファイルから `use` で辿れるモジュールを依存の順に検査する。他のモジュールの宣言は、
//! シグネチャだけを検査器に渡し（本体は見ない）、各モジュールを 1 度だけ検査する。
//! ファイルは `modpath::load` で読む（開いたファイルから読み、同じファイルで実体を確かめる）。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::effects::Options;
use crate::error::{Error, ErrorKind, Span};
use crate::modpath::{self, LoadError};

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
    cands: HashMap<String, Cands>,
}

impl Proj<'_> {
    /// ファイルを読む。`from` は取り込んだ側の `use`（ファイルがないとき、その行の `UnresolvedUse` にする）。
    fn load(&mut self, path: &Path, from: Option<(&Path, &UseDecl)>) -> R<(PathBuf, File)> {
        let Src::Fs {
            root,
            on_read,
            overlay,
            on_text,
        } = &mut self.src
        else {
            unreachable!("ファイルから読むのは Fs だけ");
        };
        let mut hook = || (on_read)(path);
        let loaded = match overlay.as_mut().and_then(|ov| ov(path)) {
            Some(text) => {
                if let Some(cb) = on_text.as_mut() {
                    cb(path, &text);
                }
                crate::parse(&text)
                    .and_then(|f| modpath::verify_parsed(&f, root, path, None).map(|()| (text, f)))
                    .map_err(LoadError::Lang)
            }
            None => {
                let mut sink = |t: &str| {
                    if let Some(cb) = on_text.as_mut() {
                        cb(path, t);
                    }
                };
                modpath::load_full(root, path, &mut hook, &mut sink)
            }
        };
        match loaded {
            Ok((_, f)) => Ok((path.to_path_buf(), f)),
            Err(LoadError::Lang(error)) => Err(ProjectError::Diag {
                file: path.to_path_buf(),
                error,
            }),
            Err(LoadError::Io(msg)) => match from {
                Some((ufile, u)) => Err(ProjectError::Diag {
                    file: ufile.to_path_buf(),
                    error: Error::new(
                        ErrorKind::UnresolvedUse,
                        u.span,
                        format!(
                            "`use {}` の先のモジュールのファイル `{}` を読めない: {msg}",
                            u.path.join("."),
                            path.display()
                        ),
                    ),
                }),
                None => Err(ProjectError::Read {
                    file: path.to_path_buf(),
                    msg,
                }),
            },
        }
    }

    fn visit(&mut self, path: PathBuf, file: File) -> R<()> {
        let name = file.module.name.join(".");
        self.stack.push(name.clone());
        let r = self.visit_in(&path, &file, &name);
        self.stack.pop();
        r?;
        self.order.push(name.clone());
        self.mods.insert(name, (path, file));
        Ok(())
    }

    /// `use` の先のモジュール `mpath` を読む（ファイル、または渡されたソース）。
    fn load_dep(&mut self, mpath: &[String], from: (&Path, &UseDecl)) -> R<(PathBuf, File)> {
        let mname = mpath.join(".");
        match &mut self.src {
            Src::Fs { root, .. } => {
                let tpath = root.join(format!("{}.sigil", mpath.join("/")));
                self.load(&tpath, Some(from))
            }
            Src::Mem(files) => match files.remove(&mname) {
                Some(x) => Ok(x),
                None => Err(ProjectError::Diag {
                    file: from.0.to_path_buf(),
                    error: Error::new(
                        ErrorKind::UnresolvedUse,
                        from.1.span,
                        format!(
                            "`use {}` の先のモジュール `{mname}` のソースが渡されていない",
                            from.1.path.join(".")
                        ),
                    ),
                }),
            },
        }
    }

    fn visit_in(&mut self, path: &Path, file: &File, name: &str) -> R<()> {
        let diag = |span: Span, kind: ErrorKind, msg: String| ProjectError::Diag {
            file: path.to_path_buf(),
            error: Error::new(kind, span, msg),
        };
        let mut cands = Cands::default();
        for i in &file.items {
            if let Some(n) = item_name(i) {
                cands.add(n, name, true);
            }
        }
        let mut found_all: Vec<(String, Item)> = vec![];
        let mut visible_traits: Vec<String> = vec![];
        let mut trait_uses: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        for u in &file.uses {
            let (item, mpath) = u.path.split_last().expect("use の path は 2 つ以上");
            let mname = mpath.join(".");
            if modpath::is_reserved_head(&mpath[0]) {
                return Err(diag(
                    u.span,
                    ErrorKind::ReservedModuleName,
                    format!("`use {}`: `{}` は標準ライブラリの名前で、利用者のモジュールの名前にならない（標準ライブラリは `use` なしで見える）", u.path.join("."), mpath[0]),
                ));
            }
            if crate::check::is_std_name(item) {
                return Err(diag(
                    u.span,
                    ErrorKind::UseConflict,
                    format!("取り込む名前 `{item}` は標準の名前と同じで使えない"),
                ));
            }
            if let Some(i) = self.stack.iter().position(|m| *m == mname) {
                let mut cyc: Vec<&str> = self.stack[i..].iter().map(String::as_str).collect();
                cyc.push(&mname);
                return Err(diag(
                    u.span,
                    ErrorKind::ModuleCycle,
                    format!("モジュールの参照が輪になっている: {}", cyc.join(" -> ")),
                ));
            }
            if !self.mods.contains_key(&mname) {
                if self.stack.len() >= MAX_USE_DEPTH {
                    return Err(diag(
                        u.span,
                        ErrorKind::ModuleLimit,
                        format!("`use` の連鎖が深すぎる（上限 {MAX_USE_DEPTH}）"),
                    ));
                }
                if self.mods.len() + self.stack.len() >= MAX_FILES {
                    return Err(diag(
                        u.span,
                        ErrorKind::ModuleLimit,
                        format!("辿るファイルが多すぎる（上限 {MAX_FILES}）"),
                    ));
                }
                let (tp, tf) = self.load_dep(mpath, (path, u))?;
                self.visit(tp, tf)?;
            }
            let (_, target) = &self.mods[&mname];
            let Some(decl) = find_decl(target, item) else {
                return Err(diag(
                    u.span,
                    ErrorKind::UnresolvedUse,
                    format!("モジュール `{mname}` に `{item}` という宣言がない（他のモジュールから取り込んだ名前の再公開はできない）"),
                ));
            };
            if !decl_is_pub(decl) {
                return Err(diag(
                    u.span,
                    ErrorKind::PrivateImport,
                    format!("`{mname}` の `{item}` は非公開（`pub` がない）ので取り込めない"),
                ));
            }
            if let Item::Trait(td) = decl {
                visible_traits.push(cands.view_name(name, &mname, item));
                trait_uses.insert(
                    item.to_string(),
                    td.fns.iter().map(|f| f.name.clone()).collect(),
                );
            }
            if let Some(t) = file
                .module
                .targets
                .iter()
                .find(|t| !target.module.targets.contains(t))
            {
                return Err(diag(
                    u.span,
                    ErrorKind::TargetMismatch,
                    format!(
                        "`{name}` の `targets` に `{t}` があるが、取り込む先 `{mname}` の `targets` にはない（取り込む側の出力先のすべてで、取り込まれる側が使えなければならない）"
                    ),
                ));
            }
            let mut found = vec![];
            self.exports(&mname, item, &mut found, &mut HashSet::new());
            for (k, (origin, it)) in found.into_iter().enumerate() {
                if let Some(n) = item_name(&it) {
                    // 先頭は `use` で名指しした宣言（明示）。ほかは隠れた依存の型（暗黙）。
                    cands.add(n, &origin, k == 0);
                    if found_all
                        .iter()
                        .any(|(o, x)| *o == origin && item_name(x) == Some(n))
                    {
                        continue;
                    }
                }
                found_all.push((origin, it));
            }
        }
        // 名前の解決。曖昧な名前は、書いた箇所（文・宣言）で `UseConflict`。
        let picks = cands.picks();
        let view = |origin: &str, n: &str| cands.view_name(name, origin, n);
        let map: HashMap<String, Result<String, String>> = picks
            .iter()
            .map(|(n, r)| (n.clone(), r.clone().map(|o| view(&o, n))))
            .collect();
        let mut externs: Vec<Item> = vec![];
        for (origin, mut it) in found_all {
            // 取り込んだ宣言は、元のモジュールでの解決に従って名前を付け替える。
            let omap: HashMap<String, Result<String, String>> = match self.cands.get(&origin) {
                Some(oc) => oc
                    .picks()
                    .into_iter()
                    .map(|(n, r)| {
                        let r = r.map(|o| cands.view_name(name, &o, &n));
                        (n, r)
                    })
                    .collect(),
                None => HashMap::new(),
            };
            let mut decl = HashMap::new();
            if let Some(n) = item_name(&it) {
                decl.insert(n.to_string(), cands.view_name(name, &origin, n));
            }
            Walk::new(&omap, decl).item(&mut it);
            externs.push(it);
        }
        let mut checked = file.clone();
        let mut w = Walk::new(&map, HashMap::new());
        for it in &mut checked.items {
            w.item(it);
        }
        if let Some((span, msg)) = w.err {
            return Err(diag(span, ErrorKind::UseConflict, msg));
        }
        for it in &file.items {
            if let Item::Impl(i) = it {
                if let Type::Path { path: tp, .. } = &i.ty {
                    if matches!(picks.get(&tp[0]), Some(Ok(o)) if o != name) {
                        return Err(diag(
                            i.span,
                            ErrorKind::TypeMismatch,
                            format!("`impl` は型を宣言したモジュールの中にだけ書ける（取り込んだ型 `{}` には付けられない）", tp[0]),
                        ));
                    }
                }
            }
        }
        crate::check::check_module(&checked, &externs, &visible_traits, self.opts).map_err(
            |error| ProjectError::Diag {
                file: path.to_path_buf(),
                error,
            },
        )?;
        self.cands.insert(name.to_string(), cands);
        let mut used = used_names(file);
        if !trait_uses.is_empty() {
            // トレイトの取り込みは、そのトレイトの関数をメソッドの形で呼んでいれば使っている
            // （呼び出しにトレイトの名前は現れない。ADR 0022 決定 2）。
            let fc = field_call_names(file);
            for (item, fns) in &trait_uses {
                if fns.iter().any(|f| fc.contains(f)) {
                    used.insert(item.clone());
                }
            }
        }
        for u in &file.uses {
            let item = u.path.last().expect("use の path は 2 つ以上");
            if !used.contains(item.as_str()) {
                return Err(diag(
                    u.span,
                    ErrorKind::UnusedUse,
                    format!("取り込んだ `{item}` を使っていない（使わない `use` は消す）"),
                ));
            }
        }
        Ok(())
    }

    /// モジュール `mname` の公開の宣言 `item` と、そのメソッド（`pub` のもの）、シグネチャに現れる型を集める。
    /// 型は取り込む側が名指しできなくても、値の型として現れるので、隠れた依存として一緒に渡す。
    fn exports(
        &self,
        mname: &str,
        item: &str,
        out: &mut Vec<(String, Item)>,
        seen: &mut HashSet<(String, String)>,
    ) {
        if !seen.insert((mname.to_string(), item.to_string())) {
            return;
        }
        let (_, target) = &self.mods[mname];
        let Some(decl) = find_decl(target, item) else {
            return;
        };
        let mut items = vec![decl.clone()];
        if !matches!(decl, Item::Fn(_)) {
            for it in &target.items {
                if let Item::Impl(i) = it {
                    if matches!(&i.ty, Type::Path { path, .. } if path[0] == item) {
                        let mut i = i.clone();
                        i.fns.retain(|f| f.is_pub);
                        items.push(Item::Impl(i));
                    }
                }
                // トレイトの実装は、トレイトか型を取り込む側へ一緒に渡す（ADR 0022 決定 2:
                // 実装を探す範囲は依存の閉包の全体。`impl` 自体に公開・非公開はない）。
                if let Item::ImplFor(im) = it {
                    let ty_head = matches!(&im.ty, Type::Path { path, .. } if path[0] == item);
                    if im.trait_name == item || ty_head {
                        items.push(Item::ImplFor(im.clone()));
                    }
                }
            }
        }
        let mut heads: Vec<String> = vec![];
        for it in &items {
            decl_type_heads(it, &mut heads);
        }
        for it in items {
            out.push((mname.to_string(), it));
        }
        for h in heads {
            if h == item {
                continue;
            }
            if target
                .items
                .iter()
                .any(|i| !matches!(i, Item::Fn(_)) && item_name(i) == Some(h.as_str()))
            {
                self.exports(mname, &h, out, seen);
            } else if let Some(u) = target.uses.iter().find(|u| u.path.last() == Some(&h)) {
                let m = u.path[..u.path.len() - 1].join(".");
                self.exports(&m, &h, out, seen);
            }
        }
    }
}

fn item_name(i: &Item) -> Option<&str> {
    match i {
        Item::Type(t) => Some(&t.name),
        Item::Struct(s) => Some(&s.name),
        Item::Enum(e) => Some(&e.name),
        Item::Fn(f) => Some(&f.name),
        Item::Trait(t) => Some(&t.name),
        Item::Impl(_) | Item::ImplFor(_) => None,
    }
}

fn find_decl<'a>(f: &'a File, name: &str) -> Option<&'a Item> {
    f.items.iter().find(|i| item_name(i) == Some(name))
}

fn decl_is_pub(i: &Item) -> bool {
    match i {
        Item::Type(t) => t.is_pub,
        Item::Struct(s) => s.is_pub,
        Item::Enum(e) => e.is_pub,
        Item::Fn(f) => f.is_pub,
        Item::Trait(t) => t.is_pub,
        Item::Impl(_) | Item::ImplFor(_) => false,
    }
}

// ---- 型の名前の収集 ----

fn type_heads(t: &Type, out: &mut Vec<String>) {
    match t {
        Type::Path { path, args } => {
            out.push(path[0].clone());
            args.iter().for_each(|a| type_heads(a, out));
        }
        Type::Ref { inner, .. } => type_heads(inner, out),
        Type::Fn { params, ret } => {
            params.iter().for_each(|p| type_heads(p, out));
            if let Some(r) = ret {
                type_heads(r, out);
            }
        }
        Type::Tuple(ts) => ts.iter().for_each(|t| type_heads(t, out)),
    }
}

fn fn_sig_heads(f: &FnDecl, outer: &[Generic], out: &mut Vec<String>) {
    let mut hs = vec![];
    f.params.iter().for_each(|p| type_heads(&p.ty, &mut hs));
    if let Some(r) = &f.ret {
        type_heads(r, &mut hs);
    }
    hs.retain(|h| !f.generics.iter().any(|g| g.name == *h) && !outer.iter().any(|g| g.name == *h));
    out.extend(hs);
}

/// 宣言のシグネチャ（本体を除く）に現れる型の名前。型引数の名前は除く。
fn decl_type_heads(i: &Item, out: &mut Vec<String>) {
    let mut hs = vec![];
    match i {
        Item::Fn(f) => fn_sig_heads(f, &[], &mut hs),
        Item::Struct(s) => {
            s.fields.iter().for_each(|f| type_heads(&f.ty, &mut hs));
            hs.retain(|h| !s.generics.iter().any(|g| g.name == *h));
        }
        Item::Enum(e) => {
            e.variants
                .iter()
                .for_each(|v| v.fields.iter().for_each(|t| type_heads(t, &mut hs)));
            hs.retain(|h| !e.generics.iter().any(|g| g.name == *h));
        }
        Item::Type(t) => type_heads(&t.base, &mut hs),
        Item::Impl(im) => {
            for f in &im.fns {
                fn_sig_heads(f, &im.generics, &mut hs);
            }
        }
        Item::Trait(t) => {
            for f in &t.fns {
                fn_sig_heads(f, &[], &mut hs);
            }
            // トレイトの関数に出る `Self` は宣言の名前ではないので除く。
            hs.retain(|h| h != "Self");
        }
        Item::ImplFor(im) => {
            hs.push(im.trait_name.clone());
            type_heads(&im.ty, &mut hs);
            for f in &im.fns {
                fn_sig_heads(f, &im.generics, &mut hs);
            }
        }
    }
    out.extend(hs);
}

/// 公開の宣言のシグネチャが、同じモジュールの非公開の型を名指ししていれば、その位置と説明
/// （ADR 0021 の決定 4。仕様に出るシグネチャが、取り込めない型を指さないようにする）。
pub fn private_in_public(file: &File) -> Option<(Span, String)> {
    let private: HashSet<&str> = file
        .items
        .iter()
        .filter(|i| !matches!(i, Item::Fn(_) | Item::Impl(_)) && !decl_is_pub(i))
        .filter_map(item_name)
        .collect();
    if private.is_empty() {
        return None;
    }
    let hit = |what: String, span: Span, heads: Vec<String>| {
        heads.iter().find(|h| private.contains(h.as_str())).map(|h| {
            (
                span,
                format!("公開の{what}のシグネチャが、非公開の型 `{h}` を名指ししている（`{h}` にも `pub` を付けるか、公開をやめる）"),
            )
        })
    };
    for i in &file.items {
        let r = match i {
            Item::Fn(f) if f.is_pub => {
                let mut hs = vec![];
                fn_sig_heads(f, &[], &mut hs);
                hit(format!("関数 `{}`", f.name), f.span, hs)
            }
            Item::Struct(s) if s.is_pub => {
                let mut hs = vec![];
                decl_type_heads(i, &mut hs);
                hit(format!("構造体 `{}`", s.name), s.span, hs)
            }
            Item::Enum(e) if e.is_pub => {
                let mut hs = vec![];
                decl_type_heads(i, &mut hs);
                hit(format!("列挙型 `{}`", e.name), e.span, hs)
            }
            Item::Type(t) if t.is_pub => {
                let mut hs = vec![];
                decl_type_heads(i, &mut hs);
                hit(format!("型 `{}`", t.name), t.span, hs)
            }
            Item::Impl(im) => {
                let head_private =
                    matches!(&im.ty, Type::Path { path, .. } if private.contains(path[0].as_str()));
                if head_private {
                    None
                } else {
                    im.fns.iter().filter(|f| f.is_pub).find_map(|f| {
                        let mut hs = vec![];
                        fn_sig_heads(f, &im.generics, &mut hs);
                        hit(format!("メソッド `{}`", f.name), f.span, hs)
                    })
                }
            }
            _ => None,
        };
        if r.is_some() {
            return r;
        }
    }
    None
}

impl Proj<'_> {
    /// 検査し終えたモジュールを 1 つのプログラムに結ぶ（ADR 0021 の 2-2c、ADR 0022 決定 1）。
    /// 入口でないモジュールの関数と型は `<モジュール名>.<名前>` に改め（`.` は利用者の名前に現れないので
    /// 衝突しない）、各モジュールの参照を検査時と同じ解決で、改めた名前に向ける。
    fn link(&self, entry: &str) -> R<File> {
        for m in &self.order {
            let (_, f) = &self.mods[m];
            if m != entry && f.module.trust == Trust::Base {
                return Err(ProjectError::Diag {
                    file: self.mods[m].0.clone(),
                    error: Error::new(
                        ErrorKind::Unsupported,
                        f.module.span,
                        format!("入口でないモジュール `{m}` は `trust base` にできない（信頼基盤は入口のモジュールだけ。結合の対象外）"),
                    ),
                });
            }
        }
        let mut items: Vec<Item> = vec![];
        for m in &self.order {
            let (_, f) = &self.mods[m];
            let map: HashMap<String, Result<String, String>> = self.cands[m]
                .picks()
                .into_iter()
                .map(|(n, r)| {
                    let r = r.map(|o| {
                        if o == entry {
                            n.clone()
                        } else {
                            format!("{o}.{n}")
                        }
                    });
                    (n, r)
                })
                .collect();
            let mut decl = HashMap::new();
            if m != entry {
                for it in &f.items {
                    if let Some(n) = item_name(it) {
                        decl.insert(n.to_string(), format!("{m}.{n}"));
                    }
                }
            }
            let mut w = Walk::new(&map, decl);
            for it in &f.items {
                let mut it = it.clone();
                w.item(&mut it);
                items.push(it);
            }
        }
        let (_, ef) = &self.mods[entry];
        Ok(File {
            module: ef.module.clone(),
            uses: vec![],
            items,
        })
    }
}

/// 名前の候補（ADR 0022 決定 1）。短い名前 -> (明示の出どころ, 暗黙の出どころ)。
/// 明示は自分の宣言と `use` で名指しした宣言、暗黙は取り込んだ宣言のシグネチャに現れる隠れた依存の型。
#[derive(Default)]
struct Cands {
    names: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>,
}

impl Cands {
    fn add(&mut self, n: &str, origin: &str, explicit: bool) {
        let e = self.names.entry(n.to_string()).or_default();
        if explicit {
            e.0.insert(origin.to_string());
        } else {
            e.1.insert(origin.to_string());
        }
    }

    /// 型・トレイトの名前（大文字で始まるもの）の、解決した先と明示か暗黙か。曖昧な名前は含めない。
    fn type_names(&self) -> Vec<TypeName> {
        let picks = self.picks();
        self.names
            .iter()
            .filter(|(n, _)| n.starts_with(|c: char| c.is_uppercase()))
            .filter_map(|(n, (e, _))| match picks.get(n) {
                Some(Ok(o)) => Some(TypeName {
                    name: n.clone(),
                    origin: o.clone(),
                    explicit: e.contains(o),
                }),
                _ => None,
            })
            .collect()
    }

    /// 書かれた名前 -> 出どころのモジュール、または曖昧の説明。
    /// 出どころが 1 つならそれ。明示が 2 つ以上なら曖昧。明示が 1 つ X のとき、暗黙に別の出どころがあれば、
    /// 暗黙にも X があるときだけ X（宣言が隠れた依存の型に乗っ取られない）。明示がなく暗黙が 2 つ以上なら曖昧。
    fn picks(&self) -> HashMap<String, Result<String, String>> {
        self.names
            .iter()
            .map(|(n, (e, i))| {
                let all: BTreeSet<&String> = e.iter().chain(i.iter()).collect();
                let list = all.iter().map(|o| format!("`{o}`")).collect::<Vec<_>>().join("・");
                let r = if all.len() == 1 {
                    Ok(all.iter().next().expect("1 つ").to_string())
                } else if e.len() >= 2 {
                    Err(format!("名前 `{n}` がどのモジュールの宣言か決まらない（{list} から取り込んだ同じ名前。別名で逃がす手段はない。名前を変えるのは書き手の側）"))
                } else if e.len() == 1 {
                    let x = e.iter().next().expect("1 つ");
                    if i.contains(x) {
                        Ok(x.clone())
                    } else {
                        Err(format!("名前 `{n}` がどのモジュールの宣言か決まらない（{list}。取り込んだ宣言のシグネチャに現れる別の `{n}` がある）"))
                    }
                } else {
                    Err(format!("名前 `{n}` がどのモジュールの宣言か決まらない（{list} の型が、取り込んだ宣言のシグネチャに現れる）"))
                };
                (n.clone(), r)
            })
            .collect()
    }

    /// モジュール `m` の検査で、`origin` の宣言 `n` を指す名前。自分の宣言と、候補が 1 つだけの名前は
    /// そのまま。同じ名前が複数の出どころにあるときは `<出どころ>.<名前>`。
    fn view_name(&self, m: &str, origin: &str, n: &str) -> String {
        if origin == m {
            return n.to_string();
        }
        let mut all: BTreeSet<&str> = BTreeSet::new();
        all.insert(origin);
        if let Some((e, i)) = self.names.get(n) {
            all.extend(e.iter().chain(i.iter()).map(String::as_str));
        }
        if all.len() <= 1 {
            n.to_string()
        } else {
            format!("{origin}.{n}")
        }
    }
}

/// 名前の付け替え（局所の束縛・型引数が同じ名前を隠すところは触らない）。
/// 曖昧な名前を書いた最初の箇所（文・宣言の位置）を `err` に残す。
struct Walk<'a> {
    map: &'a HashMap<String, Result<String, String>>,
    /// 宣言の名前の付け替え。
    decl: HashMap<String, String>,
    scopes: Vec<Vec<String>>,
    generics: Vec<String>,
    span: Span,
    err: Option<(Span, String)>,
}

impl<'a> Walk<'a> {
    fn new(
        map: &'a HashMap<String, Result<String, String>>,
        decl: HashMap<String, String>,
    ) -> Self {
        Walk {
            map,
            decl,
            scopes: vec![],
            generics: vec![],
            span: Span::default(),
            err: None,
        }
    }

    fn rename(&mut self, n: &mut String) {
        if self.generics.iter().any(|g| g == n) {
            return;
        }
        match self.map.get(n.as_str()) {
            Some(Ok(m)) => *n = m.clone(),
            Some(Err(msg)) if self.err.is_none() => {
                self.err = Some((self.span, msg.clone()));
            }
            Some(Err(_)) | None => {}
        }
    }

    fn decl_name(&self, n: &mut String) {
        if let Some(m) = self.decl.get(n.as_str()) {
            *n = m.clone();
        }
    }

    /// 型引数の制約に書かれた名前（トレイトの名前）の付け替え。性質の名前は候補にないので触らない。
    fn bounds(&mut self, gs: &mut [Generic]) {
        for g in gs {
            for b in &mut g.bounds {
                if let Some(first) = b.first_mut() {
                    self.rename(first);
                }
            }
        }
    }

    fn with_generics(&mut self, gs: &[Generic], f: impl FnOnce(&mut Self)) {
        let n = self.generics.len();
        self.generics.extend(gs.iter().map(|g| g.name.clone()));
        f(self);
        self.generics.truncate(n);
    }

    fn ty(&mut self, t: &mut Type) {
        match t {
            Type::Path { path, args } => {
                self.rename(&mut path[0]);
                args.iter_mut().for_each(|a| self.ty(a));
            }
            Type::Ref { inner, .. } => self.ty(inner),
            Type::Fn { params, ret } => {
                params.iter_mut().for_each(|p| self.ty(p));
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
            Type::Tuple(ts) => ts.iter_mut().for_each(|t| self.ty(t)),
        }
    }

    fn item(&mut self, it: &mut Item) {
        match it {
            Item::Type(t) => {
                self.span = t.span;
                self.decl_name(&mut t.name);
                self.ty(&mut t.base);
                self.scopes.push(vec![]);
                self.expr(&mut t.cond);
                self.scopes.pop();
            }
            Item::Struct(s) => {
                self.span = s.span;
                self.decl_name(&mut s.name);
                self.bounds(&mut s.generics);
                let gs = s.generics.clone();
                self.with_generics(&gs, |w| s.fields.iter_mut().for_each(|f| w.ty(&mut f.ty)));
            }
            Item::Enum(e) => {
                self.span = e.span;
                self.decl_name(&mut e.name);
                self.bounds(&mut e.generics);
                let gs = e.generics.clone();
                self.with_generics(&gs, |w| {
                    for v in &mut e.variants {
                        v.fields.iter_mut().for_each(|t| w.ty(t));
                    }
                });
            }
            Item::Fn(f) => self.func(f, true),
            Item::Impl(im) => {
                self.span = im.span;
                self.bounds(&mut im.generics);
                let gs = im.generics.clone();
                self.with_generics(&gs, |w| {
                    w.ty(&mut im.ty);
                    im.fns.iter_mut().for_each(|f| w.func(f, false));
                });
            }
            Item::Trait(t) => {
                self.span = t.span;
                self.decl_name(&mut t.name);
                t.fns.iter_mut().for_each(|f| self.func(f, false));
            }
            Item::ImplFor(im) => {
                self.span = im.span;
                // トレイトの名前は、自分の宣言なら宣言の付け替え、取り込んだものなら候補からの解決。
                if self.decl.contains_key(&im.trait_name) {
                    self.decl_name(&mut im.trait_name);
                } else {
                    self.rename(&mut im.trait_name);
                }
                self.bounds(&mut im.generics);
                let gs = im.generics.clone();
                self.with_generics(&gs, |w| {
                    w.ty(&mut im.ty);
                    im.fns.iter_mut().for_each(|f| w.func(f, false));
                });
            }
        }
    }

    fn func(&mut self, f: &mut FnDecl, is_free: bool) {
        self.span = f.span;
        if is_free {
            self.decl_name(&mut f.name);
        }
        self.bounds(&mut f.generics);
        let gs = f.generics.clone();
        self.with_generics(&gs, |w| {
            for p in &mut f.params {
                w.ty(&mut p.ty);
            }
            if let Some(r) = &mut f.ret {
                w.ty(r);
            }
            w.scopes
                .push(f.params.iter().map(|p| p.name.clone()).collect());
            if let Some(e) = &mut f.requires {
                w.expr(e);
            }
            if let Some(e) = &mut f.ensures {
                w.expr(e);
            }
            w.block(&mut f.body);
            w.scopes.pop();
        });
    }

    fn bound(&self, n: &str) -> bool {
        self.scopes.iter().any(|s| s.iter().any(|x| x == n))
    }

    fn pat(&mut self, p: &mut Pattern) {
        match p {
            Pattern::Bind(n) => self.scopes.last_mut().expect("スコープ").push(n.clone()),
            Pattern::Variant { path, args } => {
                self.rename(&mut path[0]);
                args.iter_mut().for_each(|a| self.pat(a));
            }
            Pattern::Tuple(ps) => ps.iter_mut().for_each(|a| self.pat(a)),
            _ => {}
        }
    }

    fn block(&mut self, b: &mut Block) {
        self.scopes.push(vec![]);
        b.stmts.iter_mut().for_each(|s| self.stmt(s));
        self.scopes.pop();
    }

    fn stmt(&mut self, s: &mut Stmt) {
        self.span = s.span;
        match &mut s.kind {
            StmtKind::Let {
                pattern, ty, value, ..
            } => {
                if let Some(t) = ty {
                    self.ty(t);
                }
                self.expr(value);
                self.pat(pattern);
            }
            StmtKind::Assign { target, value } => {
                self.expr(target);
                self.expr(value);
            }
            StmtKind::Expr(e) => self.expr(e),
            StmtKind::Return(e) => {
                if let Some(e) = e {
                    self.expr(e);
                }
            }
            StmtKind::If(i) => self.if_stmt(i),
            StmtKind::While { cond, body } => {
                self.expr(cond);
                self.block(body);
            }
            StmtKind::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for a in arms {
                    self.scopes.push(vec![]);
                    self.pat(&mut a.pattern);
                    self.block(&mut a.body);
                    self.scopes.pop();
                }
            }
        }
    }

    fn if_stmt(&mut self, i: &mut IfStmt) {
        self.expr(&mut i.cond);
        self.block(&mut i.then);
        match i.otherwise.as_deref_mut() {
            Some(Else::If(n)) => self.if_stmt(n),
            Some(Else::Block(b)) => self.block(b),
            None => {}
        }
    }

    fn expr(&mut self, e: &mut Expr) {
        match e {
            Expr::Name(n) => {
                if !self.bound(n) {
                    self.rename(n);
                }
            }
            Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => self.expr(x),
            Expr::Tuple(xs) => xs.iter_mut().for_each(|x| self.expr(x)),
            Expr::Unary { expr: x, .. } => self.expr(x),
            Expr::Binary { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            Expr::Field { expr: x, .. } => self.expr(x),
            Expr::Call { callee, args } => {
                self.expr(callee);
                args.iter_mut().for_each(|a| self.expr(a));
            }
            Expr::StructLit { path, fields } => {
                self.rename(&mut path[0]);
                fields.iter_mut().for_each(|(_, x)| self.expr(x));
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                self.expr(scrutinee);
                for a in arms {
                    self.scopes.push(vec![]);
                    self.pat(&mut a.pattern);
                    self.expr(&mut a.body);
                    self.scopes.pop();
                }
            }
            Expr::Closure { params, body, .. } => {
                self.scopes.push(params.clone());
                match body {
                    ClosureBody::Expr(x) => self.expr(x),
                    ClosureBody::Block(b) => self.block(b),
                }
                self.scopes.pop();
            }
            Expr::Int(_)
            | Expr::Float(_)
            | Expr::Str(_)
            | Expr::Bool(_)
            | Expr::Result
            | Expr::SelfValue => {}
        }
    }
}

// ---- 名前の使用の収集（未使用の `use` の検出。字面で見る。同じ名前の局所変数があれば使用に数える）----

/// 呼び出しの形（`受け手.名前(...)`・`型.名前(...)`）で現れる名前の集合。
/// トレイトの取り込みが「使われている」かの判定に使う（メソッドの呼び出しはトレイトの名前を書かない）。
fn field_call_names(file: &File) -> HashSet<String> {
    fn ex(e: &Expr, out: &mut HashSet<String>) {
        match e {
            Expr::Call { callee, args } => {
                if let Expr::Field { name, .. } = callee.as_ref() {
                    out.insert(name.clone());
                }
                ex(callee, out);
                args.iter().for_each(|a| ex(a, out));
            }
            Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => ex(x, out),
            Expr::Tuple(xs) => xs.iter().for_each(|x| ex(x, out)),
            Expr::Unary { expr: x, .. } => ex(x, out),
            Expr::Binary { lhs, rhs, .. } => {
                ex(lhs, out);
                ex(rhs, out);
            }
            Expr::Field { expr: x, .. } => ex(x, out),
            Expr::StructLit { fields, .. } => fields.iter().for_each(|(_, x)| ex(x, out)),
            Expr::Match {
                scrutinee, arms, ..
            } => {
                ex(scrutinee, out);
                for a in arms {
                    ex(&a.body, out);
                }
            }
            Expr::Closure { body, .. } => match body {
                ClosureBody::Expr(x) => ex(x, out),
                ClosureBody::Block(b) => bl(b, out),
            },
            _ => {}
        }
    }
    fn bl(b: &Block, out: &mut HashSet<String>) {
        for s in &b.stmts {
            match &s.kind {
                StmtKind::Let { value, .. } => ex(value, out),
                StmtKind::Assign { target, value } => {
                    ex(target, out);
                    ex(value, out);
                }
                StmtKind::Expr(e) | StmtKind::Return(Some(e)) => ex(e, out),
                StmtKind::Return(None) => {}
                StmtKind::If(i) => {
                    ex(&i.cond, out);
                    bl(&i.then, out);
                    match i.otherwise.as_deref() {
                        Some(Else::If(n)) => {
                            ex(&n.cond, out);
                            bl(&n.then, out);
                        }
                        Some(Else::Block(b2)) => bl(b2, out),
                        None => {}
                    }
                }
                StmtKind::While { cond, body } => {
                    ex(cond, out);
                    bl(body, out);
                }
                StmtKind::Match {
                    scrutinee, arms, ..
                } => {
                    ex(scrutinee, out);
                    for a in arms {
                        bl(&a.body, out);
                    }
                }
            }
        }
    }
    let mut out = HashSet::new();
    for i in &file.items {
        let fns: Vec<&FnDecl> = match i {
            Item::Fn(f) => vec![f],
            Item::Impl(im) => im.fns.iter().collect(),
            Item::ImplFor(im) => im.fns.iter().collect(),
            _ => vec![],
        };
        for f in fns {
            if let Some(e) = &f.requires {
                ex(e, &mut out);
            }
            if let Some(e) = &f.ensures {
                ex(e, &mut out);
            }
            bl(&f.body, &mut out);
        }
    }
    out
}

fn used_names(file: &File) -> HashSet<String> {
    let mut u = HashSet::new();
    for i in &file.items {
        match i {
            Item::Type(t) => {
                ty(&t.base, &mut u);
                expr(&t.cond, &mut u);
            }
            Item::Struct(s) => s.fields.iter().for_each(|f| ty(&f.ty, &mut u)),
            Item::Enum(e) => e
                .variants
                .iter()
                .for_each(|v| v.fields.iter().for_each(|t| ty(t, &mut u))),
            Item::Fn(f) => func(f, &mut u),
            Item::Impl(im) => {
                ty(&im.ty, &mut u);
                im.fns.iter().for_each(|f| func(f, &mut u));
            }
            Item::Trait(t) => t.fns.iter().for_each(|f| func(f, &mut u)),
            Item::ImplFor(im) => {
                u.insert(im.trait_name.clone());
                ty(&im.ty, &mut u);
                im.fns.iter().for_each(|f| func(f, &mut u));
            }
        }
    }
    u
}

fn ty(t: &Type, u: &mut HashSet<String>) {
    let mut hs = vec![];
    type_heads(t, &mut hs);
    u.extend(hs);
}

fn func(f: &FnDecl, u: &mut HashSet<String>) {
    f.params.iter().for_each(|p| ty(&p.ty, u));
    if let Some(r) = &f.ret {
        ty(r, u);
    }
    if let Some(e) = &f.requires {
        expr(e, u);
    }
    if let Some(e) = &f.ensures {
        expr(e, u);
    }
    block(&f.body, u);
}

fn block(b: &Block, u: &mut HashSet<String>) {
    b.stmts.iter().for_each(|s| stmt(s, u));
}

fn stmt(s: &Stmt, u: &mut HashSet<String>) {
    match &s.kind {
        StmtKind::Let {
            pattern,
            ty: t,
            value,
            ..
        } => {
            pat(pattern, u);
            if let Some(t) = t {
                ty(t, u);
            }
            expr(value, u);
        }
        StmtKind::Assign { target, value } => {
            expr(target, u);
            expr(value, u);
        }
        StmtKind::Expr(e) => expr(e, u),
        StmtKind::Return(e) => {
            if let Some(e) = e {
                expr(e, u);
            }
        }
        StmtKind::If(i) => if_stmt(i, u),
        StmtKind::While { cond, body } => {
            expr(cond, u);
            block(body, u);
        }
        StmtKind::Match {
            scrutinee, arms, ..
        } => {
            expr(scrutinee, u);
            for a in arms {
                pat(&a.pattern, u);
                block(&a.body, u);
            }
        }
    }
}

fn if_stmt(i: &IfStmt, u: &mut HashSet<String>) {
    expr(&i.cond, u);
    block(&i.then, u);
    match i.otherwise.as_deref() {
        Some(Else::If(n)) => if_stmt(n, u),
        Some(Else::Block(b)) => block(b, u),
        None => {}
    }
}

fn pat(p: &Pattern, u: &mut HashSet<String>) {
    match p {
        Pattern::Variant { path, args } => {
            u.insert(path[0].clone());
            args.iter().for_each(|a| pat(a, u));
        }
        Pattern::Tuple(ps) => ps.iter().for_each(|p| pat(p, u)),
        _ => {}
    }
}

fn expr(e: &Expr, u: &mut HashSet<String>) {
    match e {
        Expr::Name(n) => {
            u.insert(n.clone());
        }
        Expr::Old(x) | Expr::Paren(x) | Expr::Try(x) => expr(x, u),
        Expr::Tuple(xs) => xs.iter().for_each(|x| expr(x, u)),
        Expr::Unary { expr: x, .. } => expr(x, u),
        Expr::Binary { lhs, rhs, .. } => {
            expr(lhs, u);
            expr(rhs, u);
        }
        Expr::Field { expr: x, .. } => expr(x, u),
        Expr::Call { callee, args } => {
            expr(callee, u);
            args.iter().for_each(|a| expr(a, u));
        }
        Expr::StructLit { path, fields } => {
            u.insert(path[0].clone());
            fields.iter().for_each(|(_, x)| expr(x, u));
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            expr(scrutinee, u);
            for a in arms {
                pat(&a.pattern, u);
                expr(&a.body, u);
            }
        }
        Expr::Closure { body, .. } => match body {
            ClosureBody::Expr(x) => expr(x, u),
            ClosureBody::Block(b) => block(b, u),
        },
        Expr::Int(_)
        | Expr::Float(_)
        | Expr::Str(_)
        | Expr::Bool(_)
        | Expr::Result
        | Expr::SelfValue => {}
    }
}

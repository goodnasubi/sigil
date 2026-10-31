//! モジュールの探索・依存の読み込み・検査の実行。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::ast::*;
use crate::error::{Error, ErrorKind, Span};
use crate::modpath::{self, LoadError};
use crate::project::decl::{
    decl_is_pub, decl_type_heads, field_call_names, find_decl, item_name, used_names,
};
use crate::project::resolve::{Cands, Walk};
use crate::project::{Proj, ProjectError, Src, MAX_FILES, MAX_USE_DEPTH, R};

impl Proj<'_> {
    /// ファイルを読む。`from` は取り込んだ側の `use`（ファイルがないとき、その行の `UnresolvedUse` にする）。
    pub(super) fn load(
        &mut self,
        path: &Path,
        from: Option<(&Path, &UseDecl)>,
    ) -> R<(PathBuf, File)> {
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

    pub(super) fn visit(&mut self, path: PathBuf, file: File) -> R<()> {
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

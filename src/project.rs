//! 複数モジュールの名前解決と検査（ADR 0021 の決定 3〜5、PR 2-2b）。
//!
//! 入口のファイルから `use` で辿れるモジュールを依存の順に検査する。他のモジュールの宣言は、
//! シグネチャだけを検査器に渡し（本体は見ない）、各モジュールを 1 度だけ検査する。
//! ファイルは `modpath::load` で読む（開いたファイルから読み、同じファイルで実体を確かめる）。

use std::collections::{HashMap, HashSet};
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

/// `check_project` の、各ファイルを読んだ直後（確認の前）に呼ぶ差し込み口つき（差し替えの試験用）。
#[doc(hidden)]
pub fn check_project_with(
    root: &Path,
    entry: &Path,
    opts: &Options,
    on_read: &mut dyn FnMut(&Path),
) -> R<()> {
    let mut p = Proj {
        root,
        opts,
        mods: HashMap::new(),
        stack: vec![],
        on_read,
    };
    let (path, file) = p.load(entry, None)?;
    p.visit(path, file)
}

struct Proj<'a> {
    root: &'a Path,
    opts: &'a Options,
    /// 検査し終えたモジュール（名前 -> 位置と構文木）。
    mods: HashMap<String, (PathBuf, File)>,
    /// 検査中のモジュール（輪の検出）。
    stack: Vec<String>,
    on_read: &'a mut dyn FnMut(&Path),
}

impl Proj<'_> {
    /// ファイルを読む。`from` は取り込んだ側の `use`（ファイルがないとき、その行の `UnresolvedUse` にする）。
    fn load(&mut self, path: &Path, from: Option<(&Path, &UseDecl)>) -> R<(PathBuf, File)> {
        let mut hook = || (self.on_read)(path);
        match modpath::load_with_hook(self.root, path, &mut hook) {
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
        self.mods.insert(name, (path, file));
        Ok(())
    }

    fn visit_in(&mut self, path: &Path, file: &File, name: &str) -> R<()> {
        let diag = |span: Span, kind: ErrorKind, msg: String| ProjectError::Diag {
            file: path.to_path_buf(),
            error: Error::new(kind, span, msg),
        };
        let own: HashSet<String> = file
            .items
            .iter()
            .filter_map(|i| item_name(i).map(str::to_string))
            .collect();
        let mut externs: Vec<Item> = vec![];
        // 取り込んだ名前（隠れた依存の型を含む）-> 元のモジュール
        let mut provided: HashMap<String, String> = HashMap::new();
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
                let tpath = self.root.join(format!("{}.sigil", mpath.join("/")));
                let (tp, tf) = self.load(&tpath, Some((path, u)))?;
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
            for (origin, it) in found {
                if let Some(n) = item_name(&it) {
                    if own.contains(n) {
                        return Err(diag(
                            u.span,
                            ErrorKind::UseConflict,
                            format!(
                                "`use {}` が持ち込む名前 `{n}` が、このモジュールの宣言と同じ",
                                u.path.join(".")
                            ),
                        ));
                    }
                    match provided.get(n) {
                        Some(o) if *o == origin => continue,
                        Some(o) => {
                            return Err(diag(
                                u.span,
                                ErrorKind::UseConflict,
                                format!("`use {}` が持ち込む名前 `{n}` が、モジュール `{o}` から取り込んだ同じ名前と衝突する（別名で逃がす手段はない。名前を変えるのは書き手の側）", u.path.join(".")),
                            ));
                        }
                        None => {
                            provided.insert(n.to_string(), origin);
                        }
                    }
                }
                externs.push(it);
            }
        }
        for it in &file.items {
            if let Item::Impl(i) = it {
                if let Type::Path { path: tp, .. } = &i.ty {
                    if provided.contains_key(&tp[0]) {
                        return Err(diag(
                            i.span,
                            ErrorKind::TypeMismatch,
                            format!("`impl` は型を宣言したモジュールの中にだけ書ける（取り込んだ型 `{}` には付けられない）", tp[0]),
                        ));
                    }
                }
            }
        }
        crate::check::check_module(file, &externs, self.opts).map_err(|error| {
            ProjectError::Diag {
                file: path.to_path_buf(),
                error,
            }
        })?;
        let used = used_names(file);
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
        Item::Impl(_) => None,
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
        Item::Impl(_) => false,
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

// ---- 名前の使用の収集（未使用の `use` の検出。字面で見る。同じ名前の局所変数があれば使用に数える）----

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

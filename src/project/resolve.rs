//! 名前の解決・候補・AST の名前付け替え。

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ast::*;
use crate::error::Span;
use crate::project::TypeName;

/// 名前の候補（ADR 0022 決定 1）。短い名前 -> (明示の出どころ, 暗黙の出どころ)。
/// 明示は自分の宣言と `use` で名指しした宣言、暗黙は取り込んだ宣言のシグネチャに現れる隠れた依存の型。
#[derive(Default)]
pub(super) struct Cands {
    names: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)>,
}

impl Cands {
    pub(super) fn add(&mut self, n: &str, origin: &str, explicit: bool) {
        let e = self.names.entry(n.to_string()).or_default();
        if explicit {
            e.0.insert(origin.to_string());
        } else {
            e.1.insert(origin.to_string());
        }
    }

    /// 型・トレイトの名前（大文字で始まるもの）の、解決した先と明示か暗黙か。曖昧な名前は含めない。
    pub(super) fn type_names(&self) -> Vec<TypeName> {
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
    pub(super) fn picks(&self) -> HashMap<String, Result<String, String>> {
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
    pub(super) fn view_name(&self, m: &str, origin: &str, n: &str) -> String {
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
pub(super) struct Walk<'a> {
    map: &'a HashMap<String, Result<String, String>>,
    /// 宣言の名前の付け替え。
    decl: HashMap<String, String>,
    scopes: Vec<Vec<String>>,
    generics: Vec<String>,
    span: Span,
    pub(super) err: Option<(Span, String)>,
}

impl<'a> Walk<'a> {
    pub(super) fn new(
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
            Type::Fn { params, ret, .. } => {
                params.iter_mut().for_each(|p| self.ty(p));
                if let Some(r) = ret {
                    self.ty(r);
                }
            }
            Type::Tuple(ts) => ts.iter_mut().for_each(|t| self.ty(t)),
        }
    }

    pub(super) fn item(&mut self, it: &mut Item) {
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
            Item::Global(g) => {
                self.span = g.span;
                self.decl_name(&mut g.name);
                self.ty(&mut g.ty);
            }
            Item::Properties(p) => {
                self.span = p.span;
                self.rename(&mut p.type_name);
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

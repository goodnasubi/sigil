//! 型検査（M1 の最初の一区切り。ADR 0011）。
//!
//! 対象は総称を含まないコード: 基本型（`Int`・`Float`・`Bool`・`String`・`Unit`）・組・参照・
//! 構造体・列挙型・篩型・`impl` のメソッド・関数の型の引数。式を下から上へ型づけ、最初の
//! 型エラーで打ち切る（M0 と同じ）。総称・標準ライブラリの型（`List`・`Option`・`Result`）・
//! `match`・クロージャ・`?` は後続の PR で扱うので、いまは `Unsupported` のエラーにする
//! （黙って通さない）。効果の検査と所有権の検査は含まない。
//!
//! 位置: 式は位置を持たないので、エラーの位置は式を含む文（または宣言）の位置になる。

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::error::{Error, ErrorKind, Span};
use crate::parser::parse;

type R<T> = Result<T, Error>;

/// ソースを構文解析してから型検査する。最初のエラーを返す。
pub fn check(src: &str) -> R<()> {
    check_file(&parse(src)?)
}

#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    Tuple(Vec<Ty>),
    Ref(bool, Box<Ty>),
    /// 構造体・列挙型・篩型の名前。
    Named(String),
    Fn(Vec<Ty>, Box<Ty>),
}

fn show(t: &Ty) -> String {
    match t {
        Ty::Int => "Int".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "Bool".into(),
        Ty::Str => "String".into(),
        Ty::Unit => "Unit".into(),
        Ty::Tuple(ts) => format!("({})", ts.iter().map(show).collect::<Vec<_>>().join(", ")),
        Ty::Ref(m, i) => format!("&{}{}", if *m { "mut " } else { "" }, show(i)),
        Ty::Named(n) => n.clone(),
        Ty::Fn(ps, r) => {
            let ps = ps.iter().map(show).collect::<Vec<_>>().join(", ");
            if **r == Ty::Unit {
                format!("fn({ps})")
            } else {
                format!("fn({ps}) -> {}", show(r))
            }
        }
    }
}

#[derive(Debug, Clone)]
enum TypeDef {
    Alias(Ty),
    Struct(Vec<(String, Ty)>),
    Enum(Vec<(String, Vec<Ty>)>),
}

#[derive(Debug, Clone)]
struct Sig {
    params: Vec<(String, Ty)>,
    ret: Ty,
    effects_none: bool,
    has_self: bool,
}

#[derive(Debug, Clone)]
struct Var {
    ty: Ty,
    /// `let mut` で宣言した変数。
    mutable: bool,
    /// 引数（メッセージの出し分け用）。
    param: bool,
}

/// 標準の前置きの型・値のうち、いまの範囲では扱わないもの。
const PRELUDE_TYPES: &[&str] = &["List", "Option", "Result", "Map", "Set", "Path", "IoError"];
const PRELUDE_VALUES: &[&str] = &["Some", "None", "Ok", "Err"];

#[derive(Clone, Copy, PartialEq)]
enum Pos {
    /// 関数・メソッドの引数の型（関数の型を書ける唯一の位置）。
    Param,
    Other,
}

struct Ck {
    types: HashMap<String, TypeDef>,
    fns: HashMap<String, Sig>,
    methods: HashMap<(String, String), Sig>,
    scopes: Vec<HashMap<String, Var>>,
    span: Span,
    ret: Ty,
    result: Option<Ty>,
    refine: bool,
}

pub fn check_file(file: &File) -> R<()> {
    let mut ck = Ck {
        types: HashMap::new(),
        fns: HashMap::new(),
        methods: HashMap::new(),
        scopes: vec![],
        span: file.module.span,
        ret: Ty::Unit,
        result: None,
        refine: false,
    };
    ck.run(file)
}

impl Ck {
    fn err<T>(&self, kind: ErrorKind, msg: impl Into<String>) -> R<T> {
        Err(Error::new(kind, self.span, msg))
    }

    fn unsupported<T>(&self, what: &str) -> R<T> {
        self.err(
            ErrorKind::Unsupported,
            format!("{what}は、この版の型検査ではまだ扱えない（M1 の後続の PR で対応する）"),
        )
    }

    // ---------- 宣言の収集 ----------

    fn run(&mut self, file: &File) -> R<()> {
        // 1. 型の名前
        let mut names: HashSet<String> = HashSet::new();
        for item in &file.items {
            let (name, span, generics) = match item {
                Item::Type(t) => (Some(&t.name), t.span, &[][..]),
                Item::Struct(s) => (Some(&s.name), s.span, &s.generics[..]),
                Item::Enum(e) => (Some(&e.name), e.span, &e.generics[..]),
                Item::Fn(f) => (None, f.span, &f.generics[..]),
                Item::Impl(i) => (None, i.span, &i.generics[..]),
            };
            self.span = span;
            if !generics.is_empty() {
                return self.unsupported("総称の宣言");
            }
            if let Some(n) = name {
                if !names.insert(n.clone()) {
                    return self.err(
                        ErrorKind::DuplicateDefinition,
                        format!("型 `{n}` が二重に宣言されている"),
                    );
                }
            }
        }
        // 型の名前を先に仮登録（互いに参照できるように）
        for n in &names {
            self.types.insert(n.clone(), TypeDef::Alias(Ty::Unit));
        }
        // 2. 型の中身
        for item in &file.items {
            match item {
                Item::Type(t) => {
                    self.span = t.span;
                    let base = self.resolve(&t.base, Pos::Other)?;
                    if self.contains_bad_base(&base) {
                        return self.err(
                            ErrorKind::RefinementBase,
                            format!(
                                "篩型 `{}` の基の型 `{}` は、篩型・参照・関数の型を含まない型でなければならない",
                                t.name,
                                show(&base)
                            ),
                        );
                    }
                    self.types.insert(t.name.clone(), TypeDef::Alias(base));
                }
                Item::Struct(s) => {
                    self.span = s.span;
                    let mut fields = vec![];
                    let mut seen = HashSet::new();
                    for f in &s.fields {
                        if !seen.insert(f.name.clone()) {
                            return self.err(
                                ErrorKind::DuplicateDefinition,
                                format!("フィールド `{}` が二重にある", f.name),
                            );
                        }
                        fields.push((f.name.clone(), self.resolve(&f.ty, Pos::Other)?));
                    }
                    self.types.insert(s.name.clone(), TypeDef::Struct(fields));
                }
                Item::Enum(e) => {
                    self.span = e.span;
                    let mut vs = vec![];
                    let mut seen = HashSet::new();
                    for v in &e.variants {
                        if !seen.insert(v.name.clone()) {
                            return self.err(
                                ErrorKind::DuplicateDefinition,
                                format!("値 `{}` が二重にある", v.name),
                            );
                        }
                        let mut ts = vec![];
                        for t in &v.fields {
                            ts.push(self.resolve(t, Pos::Other)?);
                        }
                        vs.push((v.name.clone(), ts));
                    }
                    self.types.insert(e.name.clone(), TypeDef::Enum(vs));
                }
                _ => {}
            }
        }
        // 3. 直接の再帰（間接化の型が決まるまでエラー。ADR 0011 の繰り延べの表）
        for item in &file.items {
            let (name, span) = match item {
                Item::Struct(s) => (&s.name, s.span),
                Item::Enum(e) => (&e.name, e.span),
                _ => continue,
            };
            self.span = span;
            if self.reaches(name, name, &mut HashSet::new()) {
                return self.err(
                    ErrorKind::RecursiveType,
                    format!(
                        "型 `{name}` が自分自身を直接含んでいる（間接化の型が決まるまで書けない）"
                    ),
                );
            }
        }
        // 4. 関数とメソッドのシグネチャ
        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    self.span = f.span;
                    if self.fns.contains_key(&f.name) {
                        return self.err(
                            ErrorKind::DuplicateDefinition,
                            format!("関数 `{}` が二重に宣言されている", f.name),
                        );
                    }
                    let sig = self.signature(f, None)?;
                    self.fns.insert(f.name.clone(), sig);
                }
                Item::Impl(i) => self.register_impl(i)?,
                _ => {}
            }
        }
        // 5. 篩型の条件と関数の本体
        for item in &file.items {
            match item {
                Item::Type(t) => self.check_refinement(t)?,
                Item::Fn(f) => {
                    let sig = self.fns[&f.name].clone();
                    self.check_fn(f, &sig)?;
                }
                Item::Impl(i) => {
                    let Type::Path { path, .. } = &i.ty else {
                        continue;
                    };
                    for f in &i.fns {
                        let sig = self.methods[&(path[0].clone(), f.name.clone())].clone();
                        self.check_fn(f, &sig)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn register_impl(&mut self, i: &ImplDecl) -> R<()> {
        self.span = i.span;
        let Ty::Named(tn) = self.resolve(&i.ty, Pos::Other)? else {
            return self.err(
                ErrorKind::TypeMismatch,
                "`impl` は構造体・列挙型・篩型に付ける",
            );
        };
        for f in &i.fns {
            self.span = f.span;
            if !f.generics.is_empty() {
                return self.unsupported("総称のメソッド");
            }
            let key = (tn.clone(), f.name.clone());
            if self.methods.contains_key(&key) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("`{tn}` のメソッド `{}` が二重に宣言されている", f.name),
                );
            }
            if let Some(TypeDef::Struct(fs)) = self.types.get(&tn) {
                if fs.iter().any(|(n, _)| *n == f.name) {
                    return self.err(
                        ErrorKind::MemberConflict,
                        format!(
                            "`{tn}` のフィールドとメソッドに同じ名前 `{}` は付けられない",
                            f.name
                        ),
                    );
                }
            }
            let sig = self.signature(f, Some(&tn))?;
            self.methods.insert(key, sig);
        }
        Ok(())
    }

    fn signature(&mut self, f: &FnDecl, recv: Option<&str>) -> R<Sig> {
        if !f.generics.is_empty() {
            return self.unsupported("総称の関数");
        }
        let mut params = vec![];
        let mut seen = HashSet::new();
        for p in &f.params {
            if !seen.insert(p.name.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("引数 `{}` が二重にある", p.name),
                );
            }
            params.push((p.name.clone(), self.resolve(&p.ty, Pos::Param)?));
        }
        let has_self = params.first().is_some_and(|(n, _)| n == "self");
        if has_self {
            let Some(tn) = recv else {
                return self.err(
                    ErrorKind::TypeMismatch,
                    "`self` を受け手に書けるのは `impl` の中のメソッドだけ",
                );
            };
            let inner = match &params[0].1 {
                Ty::Ref(_, i) => (**i).clone(),
                t => t.clone(),
            };
            if inner != Ty::Named(tn.to_string()) {
                return self.err(
                    ErrorKind::TypeMismatch,
                    format!(
                        "受け手 `self` の型は `{tn}`・`&{tn}`・`&mut {tn}` のどれかでなければならない（`{}` は違う）",
                        show(&params[0].1)
                    ),
                );
            }
        }
        let ret = match &f.ret {
            None => Ty::Unit,
            Some(t) => self.resolve(t, Pos::Other)?,
        };
        Ok(Sig {
            params,
            ret,
            effects_none: f.effects.is_empty(),
            has_self,
        })
    }

    fn resolve(&self, t: &Type, pos: Pos) -> R<Ty> {
        match t {
            Type::Path { path, args } => {
                if !args.is_empty() {
                    return self.unsupported("型引数のある型");
                }
                if path.len() != 1 {
                    return self.err(
                        ErrorKind::UnknownType,
                        format!("型 `{}` が見つからない", path.join(".")),
                    );
                }
                let n = path[0].as_str();
                Ok(match n {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "Bool" => Ty::Bool,
                    "String" => Ty::Str,
                    "Unit" => Ty::Unit,
                    _ if self.types.contains_key(n) => Ty::Named(n.to_string()),
                    _ if PRELUDE_TYPES.contains(&n) => {
                        return self.unsupported(&format!("標準ライブラリの型 `{n}`"))
                    }
                    _ => {
                        return self.err(ErrorKind::UnknownType, format!("型 `{n}` が見つからない"))
                    }
                })
            }
            Type::Ref { mutable, inner } => Ok(Ty::Ref(
                *mutable,
                Box::new(self.resolve(inner, Pos::Other)?),
            )),
            Type::Tuple(ts) => Ok(Ty::Tuple(
                ts.iter()
                    .map(|t| self.resolve(t, Pos::Other))
                    .collect::<R<_>>()?,
            )),
            Type::Fn { params, ret } => {
                if pos != Pos::Param {
                    return self.err(
                        ErrorKind::FnTypePosition,
                        "関数の型 `fn(...)` は、関数・メソッドの引数の型の一番外にだけ書ける",
                    );
                }
                let ps = params
                    .iter()
                    .map(|t| self.resolve(t, Pos::Other))
                    .collect::<R<_>>()?;
                let r = match ret {
                    None => Ty::Unit,
                    Some(r) => self.resolve(r, Pos::Other)?,
                };
                Ok(Ty::Fn(ps, Box::new(r)))
            }
        }
    }

    fn contains_bad_base(&self, t: &Ty) -> bool {
        match t {
            Ty::Ref(..) | Ty::Fn(..) => true,
            Ty::Named(n) => matches!(self.types.get(n), Some(TypeDef::Alias(_))),
            Ty::Tuple(ts) => ts.iter().any(|t| self.contains_bad_base(t)),
            _ => false,
        }
    }

    /// `from` の定義が（組・構造体・列挙型・篩型をたどって）`target` を直接含むか。
    fn reaches(&self, from: &str, target: &str, seen: &mut HashSet<String>) -> bool {
        let tys: Vec<&Ty> = match self.types.get(from) {
            Some(TypeDef::Alias(b)) => vec![b],
            Some(TypeDef::Struct(fs)) => fs.iter().map(|(_, t)| t).collect(),
            Some(TypeDef::Enum(vs)) => vs.iter().flat_map(|(_, ts)| ts.iter()).collect(),
            None => return false,
        };
        fn names<'a>(t: &'a Ty, out: &mut Vec<&'a str>) {
            match t {
                Ty::Named(n) => out.push(n),
                Ty::Tuple(ts) => ts.iter().for_each(|t| names(t, out)),
                _ => {}
            }
        }
        let mut ns = vec![];
        tys.iter().for_each(|t| names(t, &mut ns));
        for n in ns {
            if n == target {
                return true;
            }
            if seen.insert(n.to_string()) && self.reaches(n, target, seen) {
                return true;
            }
        }
        false
    }

    // ---------- 型の性質 ----------

    /// 値の位置での型: 参照を外し、篩型は基の型にする（ADR 0011 の決定 4・8）。
    fn val(&self, t: &Ty) -> Ty {
        match t {
            Ty::Ref(_, i) => self.val(i),
            Ty::Named(n) => match self.types.get(n) {
                Some(TypeDef::Alias(b)) => b.clone(),
                _ => t.clone(),
            },
            _ => t.clone(),
        }
    }

    fn copyable(&self, t: &Ty) -> bool {
        match t {
            Ty::Int | Ty::Float | Ty::Bool | Ty::Unit => true,
            Ty::Tuple(ts) => ts.iter().all(|t| self.copyable(t)),
            Ty::Named(_) => {
                let b = self.val(t);
                b != *t && self.copyable(&b)
            }
            Ty::Ref(_, i) => self.copyable(i),
            Ty::Str | Ty::Fn(..) => false,
        }
    }

    fn equatable(&self, t: &Ty, depth: u32) -> bool {
        if depth > 32 {
            return false;
        }
        match t {
            Ty::Int | Ty::Float | Ty::Bool | Ty::Unit | Ty::Str => true,
            Ty::Tuple(ts) => ts.iter().all(|t| self.equatable(t, depth + 1)),
            Ty::Ref(_, i) => self.equatable(i, depth + 1),
            Ty::Fn(..) => false,
            Ty::Named(n) => match self.types.get(n) {
                Some(TypeDef::Alias(b)) => self.equatable(b, depth + 1),
                Some(TypeDef::Struct(fs)) => fs.iter().all(|(_, t)| self.equatable(t, depth + 1)),
                Some(TypeDef::Enum(vs)) => vs
                    .iter()
                    .all(|(_, ts)| ts.iter().all(|t| self.equatable(t, depth + 1))),
                None => false,
            },
        }
    }

    /// 値を `expected` が求められる位置に渡せるか（暗黙の移しは篩型 -> 基の型と、リテラル -> 篩型だけ）。
    fn assignable(&self, expected: &Ty, actual: &Ty, e: &Expr) -> bool {
        if expected == actual {
            return true;
        }
        if matches!(actual, Ty::Named(_)) {
            let b = self.val(actual);
            if b != *actual && b == *expected {
                return true;
            }
        }
        if matches!(expected, Ty::Named(_)) {
            let b = self.val(expected);
            if b != *expected && b == *actual && is_literal(e) {
                return true;
            }
        }
        false
    }

    // ---------- 関数 ----------

    fn check_refinement(&mut self, t: &TypeDecl) -> R<()> {
        self.span = t.span;
        let Some(TypeDef::Alias(base)) = self.types.get(&t.name).cloned() else {
            return Ok(());
        };
        self.scopes = vec![HashMap::from([(
            "self".to_string(),
            Var {
                ty: base,
                mutable: false,
                param: true,
            },
        )])];
        self.refine = true;
        self.result = None;
        let r = self.expect(&Ty::Bool, &t.cond, "篩型の条件");
        self.refine = false;
        r
    }

    fn check_fn(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.span = f.span;
        let mut scope = HashMap::new();
        for (n, t) in &sig.params {
            scope.insert(
                n.clone(),
                Var {
                    ty: t.clone(),
                    mutable: false,
                    param: true,
                },
            );
        }
        self.scopes = vec![scope];
        self.ret = sig.ret.clone();
        self.refine = false;
        self.result = None;
        if let Some(r) = &f.requires {
            self.expect(&Ty::Bool, r, "`requires`")?;
        }
        if let Some(e) = &f.ensures {
            self.result = Some(sig.ret.clone());
            let r = self.expect(&Ty::Bool, e, "`ensures`");
            self.result = None;
            r?;
        }
        self.block(&f.body)?;
        self.span = f.span;
        if sig.ret != Ty::Unit && !block_returns(&f.body) {
            return self.err(
                ErrorKind::MissingReturn,
                format!(
                    "関数 `{}` は `{}` を返すので、すべての道が `return` で終わらなければならない",
                    f.name,
                    show(&sig.ret)
                ),
            );
        }
        Ok(())
    }

    // ---------- 文 ----------

    fn block(&mut self, b: &Block) -> R<()> {
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn lookup(&self, n: &str) -> Option<&Var> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }

    fn expect(&mut self, expected: &Ty, e: &Expr, what: &str) -> R<()> {
        let t = self.expr(e)?;
        if !self.assignable(expected, &t, e) {
            return self.err(
                ErrorKind::TypeMismatch,
                format!(
                    "{what}は `{}` でなければならないが、`{}` である",
                    show(expected),
                    show(&t)
                ),
            );
        }
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        self.span = s.span;
        match &s.kind {
            StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
            } => {
                let vt = self.expr(value)?;
                self.span = s.span;
                let fin = match ty {
                    None => vt,
                    Some(t) => {
                        let d = self.resolve(t, Pos::Other)?;
                        if d == vt {
                            return self.err(
                                ErrorKind::RedundantAnnotation,
                                format!(
                                    "型注釈 `{}` を外しても同じ型に決まるので、型注釈を取り除く",
                                    show(&d)
                                ),
                            );
                        }
                        if !self.assignable(&d, &vt, value) {
                            return self.err(
                                ErrorKind::TypeMismatch,
                                format!(
                                    "型注釈は `{}` だが、式の型は `{}` である{}",
                                    show(&d),
                                    show(&vt),
                                    self.check_hint(&d, &vt)
                                ),
                            );
                        }
                        d
                    }
                };
                if *mutable && matches!(fin, Ty::Ref(..)) {
                    return self.err(
                        ErrorKind::LetMutRef,
                        "型が参照の変数は `let mut` で宣言できない（参照を別の参照で束縛し直す操作はない）",
                    );
                }
                self.bind(pattern, &fin, *mutable)
            }
            StmtKind::Assign { target, value } => {
                let root = place_root(target).map(str::to_string);
                let tt = self.expr(target)?;
                self.span = s.span;
                let Some(root) = root else {
                    return self.err(
                        ErrorKind::AssignToImmutable,
                        "代入できるのは変数とそのフィールドだけ",
                    );
                };
                self.assignable_place(&root, target, true)?;
                self.span = s.span;
                let vt = self.expr(value)?;
                self.span = s.span;
                if !self.assignable(&tt, &vt, value) {
                    return self.err(
                        ErrorKind::TypeMismatch,
                        format!(
                            "代入先は `{}` だが、右辺は `{}` である{}",
                            show(&tt),
                            show(&vt),
                            self.check_hint(&tt, &vt)
                        ),
                    );
                }
                Ok(())
            }
            StmtKind::Expr(e) => self.expr(e).map(|_| ()),
            StmtKind::Return(v) => {
                let ret = self.ret.clone();
                match v {
                    None => {
                        if ret != Ty::Unit {
                            return self.err(
                                ErrorKind::TypeMismatch,
                                format!(
                                    "`{}` を返す関数で、値のない `return;` は書けない",
                                    show(&ret)
                                ),
                            );
                        }
                        Ok(())
                    }
                    Some(e) => self.expect(&ret, e, "戻り値"),
                }
            }
            StmtKind::If(i) => self.if_stmt(i),
            StmtKind::While { cond, body } => {
                self.expect(&Ty::Bool, cond, "`while` の条件")?;
                self.block(body)
            }
            StmtKind::Match { .. } => self.unsupported("`match`"),
        }
    }

    fn check_hint(&self, expected: &Ty, actual: &Ty) -> &'static str {
        if matches!(expected, Ty::Named(_))
            && self.val(expected) != *expected
            && self.val(expected) == *actual
        {
            "（基の型から篩型への変換は、リテラルの定数か `T.check` に限る）"
        } else if matches!(expected, Ty::Int) && *actual == Ty::Float
            || matches!(expected, Ty::Float) && *actual == Ty::Int
        {
            "（`Int` と `Float` の暗黙の変換はない）"
        } else {
            ""
        }
    }

    fn if_stmt(&mut self, i: &IfStmt) -> R<()> {
        self.expect(&Ty::Bool, &i.cond, "`if` の条件")?;
        self.block(&i.then)?;
        match i.otherwise.as_deref() {
            None => Ok(()),
            Some(Else::If(n)) => self.if_stmt(n),
            Some(Else::Block(b)) => self.block(b),
        }
    }

    fn bind(&mut self, p: &Pattern, t: &Ty, mutable: bool) -> R<()> {
        match p {
            Pattern::Wild => Ok(()),
            Pattern::Bind(n) => {
                self.scopes.last_mut().expect("scope").insert(
                    n.clone(),
                    Var {
                        ty: t.clone(),
                        mutable,
                        param: false,
                    },
                );
                Ok(())
            }
            Pattern::Tuple(ps) => {
                let Ty::Tuple(ts) = t else {
                    return self.err(
                        ErrorKind::DestructureMismatch,
                        format!("組として分解しているが、式の型は `{}` である", show(t)),
                    );
                };
                if ps.len() != ts.len() {
                    return self.err(
                        ErrorKind::DestructureMismatch,
                        format!(
                            "{} 個に分解しているが、組の要素は {} 個ある",
                            ps.len(),
                            ts.len()
                        ),
                    );
                }
                for (p, t) in ps.iter().zip(ts) {
                    self.bind(p, t, mutable)?;
                }
                Ok(())
            }
            _ => self.unsupported("この形のパターン"),
        }
    }

    /// 代入先（または `&mut` で借りる場所）の根の変数が書き換えられるか。
    fn assignable_place(&self, root: &str, target: &Expr, assign: bool) -> R<()> {
        let Some(v) = self.lookup(root) else {
            return self.err(
                ErrorKind::UnknownName,
                format!("名前 `{root}` が見つからない"),
            );
        };
        let direct = matches!(target, Expr::Name(_) | Expr::SelfValue);
        match &v.ty {
            Ty::Ref(true, _) => Ok(()),
            Ty::Ref(false, _) => {
                if assign {
                    self.err(
                        ErrorKind::AssignThroughSharedRef,
                        format!(
                            "`{root}` は `{}` なので、{}書き換えられない（`&mut` で受け取る）",
                            show(&v.ty),
                            if direct { "" } else { "フィールドも" }
                        ),
                    )
                } else {
                    self.err(
                        ErrorKind::BorrowMutOfImmutable,
                        format!(
                            "`{root}` は `{}` なので、`&mut` では借りられない",
                            show(&v.ty)
                        ),
                    )
                }
            }
            _ if v.mutable => Ok(()),
            _ => {
                let why = if v.param {
                    "値で受け取った引数は書き換えられない（`&mut` で受け取る）"
                } else {
                    "`mut` のない `let` の変数は書き換えられない"
                };
                self.err(
                    if assign {
                        ErrorKind::AssignToImmutable
                    } else {
                        ErrorKind::BorrowMutOfImmutable
                    },
                    format!("`{root}`: {why}"),
                )
            }
        }
    }

    // ---------- 式 ----------

    fn expr(&mut self, e: &Expr) -> R<Ty> {
        match e {
            Expr::Int(s) => {
                if s.parse::<i64>().is_err() {
                    return self.err(
                        ErrorKind::IntLiteralRange,
                        format!("整数のリテラル `{s}` が `Int`（64 ビットの符号付き整数）の範囲を超えている"),
                    );
                }
                Ok(Ty::Int)
            }
            Expr::Float(_) => Ok(Ty::Float),
            Expr::Str(_) => Ok(Ty::Str),
            Expr::Bool(_) => Ok(Ty::Bool),
            Expr::Name(n) => self.name(n),
            Expr::SelfValue => self.name("self"),
            Expr::Result => match self.result.clone() {
                Some(t) => Ok(t),
                None => self.err(
                    ErrorKind::UnknownName,
                    "`result` は `ensures` の中だけで使える",
                ),
            },
            Expr::Old(inner) => {
                let t = self.expr(inner)?;
                if !self.copyable(&t) {
                    return self.err(
                        ErrorKind::OldNotCopyable,
                        format!(
                            "`old(...)` の引数は暗黙に複製される型（`Copyable`）でなければならないが、`{}` である",
                            show(&t)
                        ),
                    );
                }
                Ok(t)
            }
            Expr::Paren(i) => self.expr(i),
            Expr::Tuple(es) => Ok(Ty::Tuple(
                es.iter().map(|e| self.expr(e)).collect::<R<_>>()?,
            )),
            Expr::Unary { op, expr } => self.unary(*op, expr),
            Expr::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            Expr::Field { expr, name } => self.field(expr, name),
            Expr::Call { callee, args } => self.call(callee, args),
            Expr::StructLit { path, fields } => self.struct_lit(path, fields),
            Expr::Try(_) => self.unsupported("`?`"),
            Expr::Match { .. } => self.unsupported("`match`"),
            Expr::Closure { .. } => self.unsupported("クロージャ"),
        }
    }

    fn name(&mut self, n: &str) -> R<Ty> {
        if self.refine && n != "self" {
            return self.err(
                ErrorKind::RefinementName,
                format!(
                    "篩型の条件で値として参照できるのは `self` とリテラルだけ（`{n}` は書けない）"
                ),
            );
        }
        if let Some(v) = self.lookup(n) {
            // 参照の型の名前は、式の中では参照先の値を表す（ADR 0011 の決定 4）。
            return Ok(match &v.ty {
                Ty::Ref(_, i) => (**i).clone(),
                t => t.clone(),
            });
        }
        if n == "Unit" {
            return Ok(Ty::Unit);
        }
        if let Some(sig) = self.fns.get(n) {
            return Ok(Ty::Fn(
                sig.params.iter().map(|(_, t)| t.clone()).collect(),
                Box::new(sig.ret.clone()),
            ));
        }
        if PRELUDE_VALUES.contains(&n) {
            return self.unsupported(&format!("標準ライブラリの値 `{n}`"));
        }
        self.err(ErrorKind::UnknownName, format!("名前 `{n}` が見つからない"))
    }

    fn unary(&mut self, op: UnOp, inner: &Expr) -> R<Ty> {
        match op {
            UnOp::Not => {
                let t = self.expr(inner)?;
                if t != Ty::Bool {
                    return self.err(
                        ErrorKind::OperandTypes,
                        format!("`!` は `Bool` に使うが、`{}` である", show(&t)),
                    );
                }
                Ok(Ty::Bool)
            }
            UnOp::Neg => {
                let t = self.expr(inner)?;
                let v = self.val(&t);
                if v != Ty::Int && v != Ty::Float {
                    return self.err(
                        ErrorKind::OperandTypes,
                        format!(
                            "前置の `-` は `Int`・`Float` に使うが、`{}` である",
                            show(&t)
                        ),
                    );
                }
                Ok(v)
            }
            UnOp::Ref | UnOp::RefMut => {
                let mutable = op == UnOp::RefMut;
                let t = self.expr(inner)?;
                if mutable {
                    let Some(root) = place_root(inner).map(str::to_string) else {
                        return self.err(
                            ErrorKind::BorrowOfNonPlace,
                            "`&mut` で借りられるのは変数とそのフィールドだけ",
                        );
                    };
                    self.assignable_place(&root, inner, false)?;
                }
                Ok(Ty::Ref(mutable, Box::new(t)))
            }
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr) -> R<Ty> {
        let lt = self.expr(l)?;
        let rt = self.expr(r)?;
        let (lv, rv) = (self.val(&lt), self.val(&rt));
        let sym = op_text(op);
        let mismatch = |ck: &Ck| -> R<Ty> {
            ck.err(
                ErrorKind::OperandTypes,
                format!(
                    "`{sym}` の両辺の型が合わない: `{}` と `{}`{}",
                    show(&lt),
                    show(&rt),
                    if (lv == Ty::Int && rv == Ty::Float) || (lv == Ty::Float && rv == Ty::Int) {
                        "（`Int` と `Float` の暗黙の変換はない）"
                    } else {
                        ""
                    }
                ),
            )
        };
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                if lv == rv && (lv == Ty::Int || lv == Ty::Float) {
                    Ok(lv)
                } else {
                    mismatch(self)
                }
            }
            BinOp::Rem => {
                if lv == Ty::Int && rv == Ty::Int {
                    Ok(Ty::Int)
                } else if lv == Ty::Float && rv == Ty::Float {
                    self.err(ErrorKind::OperandTypes, "`%` は `Float` に使えない")
                } else {
                    mismatch(self)
                }
            }
            BinOp::Eq | BinOp::Ne => {
                if lv != rv {
                    return mismatch(self);
                }
                if !self.equatable(&lv, 0) {
                    return self.err(
                        ErrorKind::NotEquatable,
                        format!(
                            "`{}` は `{sym}` で比べられない（`Equatable` でない）",
                            show(&lv)
                        ),
                    );
                }
                Ok(Ty::Bool)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                if lv != rv {
                    return mismatch(self);
                }
                if !matches!(lv, Ty::Int | Ty::Float | Ty::Str) {
                    return self.err(
                        ErrorKind::NotComparable,
                        format!(
                            "`{}` は `{sym}` で大小を比べられない（`Comparable` でない）",
                            show(&lv)
                        ),
                    );
                }
                Ok(Ty::Bool)
            }
            BinOp::And | BinOp::Or => {
                if lt == Ty::Bool && rt == Ty::Bool {
                    Ok(Ty::Bool)
                } else {
                    mismatch(self)
                }
            }
        }
    }

    /// `T.名前`（`T` が型）のとき、その型の名前。
    fn type_path(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Name(n) if self.lookup(n).is_none() && self.types.contains_key(n) => {
                Some(n.clone())
            }
            _ => None,
        }
    }

    fn field(&mut self, recv: &Expr, name: &str) -> R<Ty> {
        if let Some(tn) = self.type_path(recv) {
            return match self.types.get(&tn).cloned() {
                Some(TypeDef::Enum(vs)) => match vs.iter().find(|(v, _)| v == name) {
                    Some((_, fs)) if fs.is_empty() => Ok(Ty::Named(tn)),
                    Some((_, fs)) => Ok(Ty::Fn(fs.clone(), Box::new(Ty::Named(tn)))),
                    None => self.err(
                        ErrorKind::NoSuchMember,
                        format!("列挙型 `{tn}` に値 `{name}` がない"),
                    ),
                },
                _ if name == "check" => self.unsupported("`T.check`（`Option` を返す）"),
                _ => self.err(
                    ErrorKind::NoSuchMember,
                    format!("`{tn}.{name}` は値として使えない"),
                ),
            };
        }
        let t = self.expr(recv)?;
        match self.val(&t) {
            Ty::Named(n) => match self.types.get(&n) {
                Some(TypeDef::Struct(fs)) => match fs.iter().find(|(f, _)| f == name) {
                    Some((_, t)) => Ok(t.clone()),
                    None => self.err(
                        ErrorKind::NoSuchMember,
                        format!("構造体 `{n}` にフィールド `{name}` がない"),
                    ),
                },
                _ => self.err(
                    ErrorKind::NoSuchMember,
                    format!("型 `{n}` にフィールド `{name}` はない"),
                ),
            },
            Ty::Tuple(_) => self.err(
                ErrorKind::NoSuchMember,
                format!("組の要素は `.{name}` で取り出せない（`let (a, b) = t;` で分解する）"),
            ),
            other => self.err(
                ErrorKind::NoSuchMember,
                format!("`{}` にフィールド `{name}` はない", show(&other)),
            ),
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Expr]) -> R<Ty> {
        match callee {
            Expr::Name(n) => {
                if self.refine {
                    return match self.fns.get(n) {
                        Some(sig) if sig.effects_none => {
                            let sig = sig.clone();
                            let ps: Vec<Ty> = sig.params.iter().map(|(_, t)| t.clone()).collect();
                            self.args(&ps, args, &format!("関数 `{n}`"))?;
                            Ok(sig.ret)
                        }
                        Some(_) => self.err(
                            ErrorKind::RefinementEffect,
                            format!(
                                "篩型の条件で呼べるのは `effects none` の関数だけ（`{n}` は違う）"
                            ),
                        ),
                        None => {
                            self.err(ErrorKind::UnknownName, format!("関数 `{n}` が見つからない"))
                        }
                    };
                }
                let ft = if self.lookup(n).is_some() || self.fns.contains_key(n) {
                    self.name(n)?
                } else if PRELUDE_VALUES.contains(&n.as_str()) {
                    return self.unsupported(&format!("標準ライブラリの値 `{n}`"));
                } else {
                    return self.err(ErrorKind::UnknownName, format!("関数 `{n}` が見つからない"));
                };
                match ft {
                    Ty::Fn(ps, r) => {
                        self.args(&ps, args, &format!("`{n}`"))?;
                        Ok(*r)
                    }
                    other => self.err(
                        ErrorKind::NotCallable,
                        format!("`{n}` は `{}` で、呼び出せない", show(&other)),
                    ),
                }
            }
            Expr::Field { expr, name } => {
                if let Some(tn) = self.type_path(expr) {
                    match self.types.get(&tn).cloned() {
                        Some(TypeDef::Enum(vs)) if vs.iter().any(|(v, _)| v == name) => {
                            let (_, fs) = vs.iter().find(|(v, _)| v == name).expect("variant");
                            self.args(fs, args, &format!("`{tn}.{name}`"))?;
                            return Ok(Ty::Named(tn));
                        }
                        _ => {}
                    }
                    if name == "check" {
                        return self.unsupported("`T.check`（`Option` を返す）");
                    }
                    let Some(sig) = self.methods.get(&(tn.clone(), name.clone())).cloned() else {
                        return self.err(
                            ErrorKind::NoSuchMember,
                            format!("`{tn}` に `{name}` という関数・値がない"),
                        );
                    };
                    if sig.has_self {
                        return self.err(
                            ErrorKind::NotCallable,
                            format!("`{tn}.{name}` は受け手のあるメソッドなので、値に対して `x.{name}(...)` と呼ぶ"),
                        );
                    }
                    let ps: Vec<Ty> = sig.params.iter().map(|(_, t)| t.clone()).collect();
                    self.args(&ps, args, &format!("`{tn}.{name}`"))?;
                    return Ok(sig.ret);
                }
                // メソッド呼び出し x.m(...)
                let rt = strip_ref(&self.expr(expr)?);
                let Ty::Named(tn) = rt.clone() else {
                    // 篩型は基の型として働く。基の型のメソッドは標準ライブラリの範囲。
                    return self.unsupported(&format!(
                        "標準ライブラリの型 `{}` のメソッド `{name}`",
                        show(&rt)
                    ));
                };
                let Some(sig) = self.methods.get(&(tn.clone(), name.clone())).cloned() else {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!("型 `{tn}` にメソッド `{name}` がない"),
                    );
                };
                if !sig.has_self {
                    return self.err(
                        ErrorKind::NotCallable,
                        format!("`{name}` は受け手のない関数なので、`{tn}.{name}(...)` と呼ぶ"),
                    );
                }
                if let Ty::Ref(true, _) = sig.params[0].1 {
                    let Some(root) = place_root(expr).map(str::to_string) else {
                        return self.err(
                            ErrorKind::BorrowOfNonPlace,
                            format!("`{name}` は `&mut` の受け手を取るので、変数に対して呼ぶ"),
                        );
                    };
                    self.assignable_place(&root, expr, false)?;
                }
                let ps: Vec<Ty> = sig.params[1..].iter().map(|(_, t)| t.clone()).collect();
                self.args(&ps, args, &format!("メソッド `{tn}.{name}`"))?;
                Ok(sig.ret)
            }
            _ => self.err(
                ErrorKind::NotCallable,
                "呼び出せるのは関数・メソッド・値の構築子だけ",
            ),
        }
    }

    fn args(&mut self, params: &[Ty], args: &[Expr], what: &str) -> R<()> {
        if params.len() != args.len() {
            return self.err(
                ErrorKind::ArgCount,
                format!(
                    "{what}の引数は {} 個だが、{} 個渡している",
                    params.len(),
                    args.len()
                ),
            );
        }
        for (i, (p, a)) in params.iter().zip(args).enumerate() {
            let t = self.expr(a)?;
            if !self.assignable(p, &t, a) {
                let hint = if matches!(p, Ty::Ref(..)) && !matches!(t, Ty::Ref(..)) {
                    "（借用は呼び出しの位置で `&x`・`&mut x` と書く）"
                } else {
                    self.check_hint(p, &t)
                };
                return self.err(
                    ErrorKind::TypeMismatch,
                    format!(
                        "{what}の第 {} 引数は `{}` でなければならないが、`{}` である{hint}",
                        i + 1,
                        show(p),
                        show(&t)
                    ),
                );
            }
        }
        Ok(())
    }

    fn struct_lit(&mut self, path: &[String], fields: &[(String, Expr)]) -> R<Ty> {
        let name = path.join(".");
        let Some(TypeDef::Struct(defs)) = (if path.len() == 1 {
            self.types.get(&path[0]).cloned()
        } else {
            None
        }) else {
            if path.len() == 1 && PRELUDE_TYPES.contains(&path[0].as_str()) {
                return self.unsupported(&format!("標準ライブラリの型 `{name}`"));
            }
            return self.err(
                ErrorKind::UnknownType,
                format!("構造体 `{name}` が見つからない"),
            );
        };
        let mut seen = HashSet::new();
        for (f, e) in fields {
            let Some((_, ft)) = defs.iter().find(|(n, _)| n == f) else {
                return self.err(
                    ErrorKind::FieldSet,
                    format!("構造体 `{name}` にフィールド `{f}` がない"),
                );
            };
            if !seen.insert(f.clone()) {
                return self.err(
                    ErrorKind::FieldSet,
                    format!("フィールド `{f}` が二重にある"),
                );
            }
            self.expect(ft, e, &format!("フィールド `{f}`"))?;
        }
        if let Some((m, _)) = defs.iter().find(|(n, _)| !seen.contains(n)) {
            return self.err(
                ErrorKind::FieldSet,
                format!("構造体 `{name}` のフィールド `{m}` がない"),
            );
        }
        Ok(Ty::Named(name))
    }
}

fn strip_ref(t: &Ty) -> Ty {
    match t {
        Ty::Ref(_, i) => strip_ref(i),
        _ => t.clone(),
    }
}

fn place_root(e: &Expr) -> Option<&str> {
    match e {
        Expr::Name(n) => Some(n),
        Expr::SelfValue => Some("self"),
        Expr::Field { expr, .. } => place_root(expr),
        Expr::Paren(i) => place_root(i),
        _ => None,
    }
}

/// 基の型のリテラルの定数か（`-` を前置した数も含む）。
fn is_literal(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => true,
        Expr::Paren(i) => is_literal(i),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
        } => matches!(**expr, Expr::Int(_) | Expr::Float(_)),
        _ => false,
    }
}

fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn block_returns(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::If(i) => if_returns(i),
        _ => false,
    })
}

fn if_returns(i: &IfStmt) -> bool {
    block_returns(&i.then)
        && match i.otherwise.as_deref() {
            None => false,
            Some(Else::If(n)) => if_returns(n),
            Some(Else::Block(b)) => block_returns(b),
        }
}

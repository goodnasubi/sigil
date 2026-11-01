//! lowering: 検査済みの構文木（`check::Typed`）から SIR（`ir`）を作る（ADR 0015 の決定 2・5）。
//!
//! - 総称関数・総称型は、入口から届く具体的な型ごとに単相化する。
//! - 局所の値の破棄は、登録簿（`Fb::reg`）で追う。停止しうる操作の `cleanup` は、その時点の登録簿を逆順に破棄して
//!   `Resume` する連鎖で、`return`・`?` の早期の抜けも同じ破棄の並び（登録簿の逆順）を通る。
//! - 値の移動は検査器の表（`Typed::moves`）に従う。表にない読みは複製（`Copy`）か借用。

mod closure;
mod expr;
mod items;
mod pattern;
mod stmt;

use crate::ast as a;
use crate::check::{Ty as CTy, Typed};
use crate::exec::{Clause, StopKind};
use crate::ir::{self, Const, Operand, Place, Rvalue, Stmt, Term};
use std::collections::HashMap;

pub struct LowerError(pub String);

type R<T> = Result<T, LowerError>;

fn bail<T>(m: impl Into<String>) -> R<T> {
    Err(LowerError(m.into()))
}

type Mono = HashMap<String, ir::Ty>;

fn addr(e: &a::Expr) -> usize {
    e as *const a::Expr as usize
}

fn strip(e: &a::Expr) -> &a::Expr {
    match e {
        a::Expr::Paren(i) => strip(i),
        e => e,
    }
}

fn is_ref(t: &ir::Ty) -> bool {
    matches!(t, ir::Ty::Ref(_) | ir::Ty::MutRef(_))
}

fn peel(t: &ir::Ty) -> &ir::Ty {
    match t {
        ir::Ty::Ref(i) | ir::Ty::MutRef(i) => peel(i),
        t => t,
    }
}

/// 破棄の記録を残す値の型。
pub(crate) fn needs_drop(t: &ir::Ty) -> bool {
    !matches!(
        t,
        ir::Ty::Int
            | ir::Ty::Float
            | ir::Ty::Bool
            | ir::Ty::Unit
            | ir::Ty::Addr
            | ir::Ty::ParseError
            | ir::Ty::Ref(_)
            | ir::Ty::MutRef(_)
    )
}

/// 読んでも元を空にしない型（破棄の記録がない値の組を含む）。
fn trivial(t: &ir::Ty) -> bool {
    match t {
        ir::Ty::Tuple(ts) => ts.iter().all(trivial),
        t => !needs_drop(t),
    }
}

fn int_lit(s: &str) -> R<i64> {
    match s.replace('_', "").parse::<i64>() {
        Ok(n) => Ok(n),
        Err(_) => bail(format!("Int リテラル {s}")),
    }
}

#[derive(Clone)]
struct Bind {
    name: String,
    place: Place,
    ty: ir::Ty,
}

struct Bb {
    stmts: Vec<Stmt>,
    term: Option<Term>,
}

/// 関数 1 つを作っている途中の状態。
struct Fb {
    name: String,
    locals: Vec<ir::Local>,
    blocks: Vec<Bb>,
    cur: usize,
    params: Vec<usize>,
    scopes: Vec<Vec<Bind>>,
    /// 破棄する局所の登録簿（宣言の順）。2 つ目は文の終わりに破棄する一時の値。
    reg: Vec<(usize, bool)>,
    reg_blocks: Vec<Option<usize>>,
    resume: Option<usize>,
    line: u32,
    during: Option<(String, Clause)>,
    mono: Mono,
    ret_ty: ir::Ty,
    epilogue: usize,
    body_base: usize,
    self_local: Option<usize>,
    olds: HashMap<usize, usize>,
}

impl Fb {
    fn new(name: &str, ret_ty: ir::Ty, mono: Mono, line: u32) -> Fb {
        Fb {
            name: name.to_string(),
            locals: vec![],
            blocks: vec![],
            cur: 0,
            params: vec![],
            scopes: vec![],
            reg: vec![],
            reg_blocks: vec![],
            resume: None,
            line,
            during: None,
            mono,
            ret_ty,
            epilogue: 0,
            body_base: 0,
            self_local: None,
            olds: HashMap::new(),
        }
    }
}

struct Job<'a> {
    fid: usize,
    decl: &'a a::FnDecl,
    mono: Mono,
}

struct L<'a> {
    t: &'a Typed<'a>,
    fns: HashMap<&'a str, &'a a::FnDecl>,
    methods: HashMap<(String, String), &'a a::FnDecl>,
    /// トレイトの実装の関数（トレイトの名前, 型の頭, 関数の名前, 宣言, 関数。ADR 0022 決定 2）。
    trait_fns: Vec<(String, String, String, &'a a::ImplForDecl, &'a a::FnDecl)>,
    /// 宣言の番地 -> 実体名（トレイトの実装の関数は `<型>/<トレイト>.<関数>`。ADR 0022 決定 1）。
    names: HashMap<usize, String>,
    /// ADT の ID -> 型引数（入れ子の深さの計算用）。
    adt_args: HashMap<usize, Vec<ir::Ty>>,
    structs: HashMap<&'a str, &'a a::StructDecl>,
    enums: HashMap<&'a str, &'a a::EnumDecl>,
    refined: HashMap<&'a str, &'a a::TypeDecl>,
    types: Vec<ir::TypeDef>,
    type_ids: HashMap<(String, Vec<ir::Ty>), usize>,
    strings: Vec<String>,
    string_ids: HashMap<String, usize>,
    stops: Vec<ir::StopSite>,
    stop_ids: HashMap<ir::StopSite, usize>,
    funcs: Vec<Option<ir::Func>>,
    instances: HashMap<(usize, Vec<ir::Ty>), usize>,
    queue: Vec<Job<'a>>,
    checks: HashMap<String, usize>,
    core_only: bool,
    fb: Fb,
}

/// 単相化の実体（関数と型の合計）の数の上限（ADR 0022 決定 2）。
const MAX_INSTANCES: usize = 10_000;
/// 実体の鍵の型引数の入れ子の深さの上限（ADR 0022 決定 2）。
const MAX_TYPE_DEPTH: usize = 32;

pub fn lower(typed: &Typed, entry: &str) -> Result<ir::Program, LowerError> {
    let mut l = L::new(typed);
    l.run(entry)
}

fn last_name(t: &a::Type) -> Option<&str> {
    match t {
        a::Type::Path { path, .. } => path.last().map(|s| s.as_str()),
        _ => None,
    }
}

fn mentions(t: &a::Type, name: &str) -> bool {
    match t {
        a::Type::Path { path, args } => {
            path.last().is_some_and(|n| n == name) || args.iter().any(|x| mentions(x, name))
        }
        a::Type::Ref { inner, .. } => mentions(inner, name),
        a::Type::Fn { params, ret, .. } => {
            params.iter().any(|x| mentions(x, name))
                || ret.as_deref().is_some_and(|x| mentions(x, name))
        }
        a::Type::Tuple(ts) => ts.iter().any(|x| mentions(x, name)),
    }
}

impl<'a> L<'a> {
    fn new(t: &'a Typed<'a>) -> L<'a> {
        let mut l = L {
            t,
            fns: HashMap::new(),
            methods: HashMap::new(),
            trait_fns: vec![],
            names: HashMap::new(),
            adt_args: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            refined: HashMap::new(),
            types: vec![],
            type_ids: HashMap::new(),
            strings: vec![],
            string_ids: HashMap::new(),
            stops: vec![],
            stop_ids: HashMap::new(),
            funcs: vec![],
            instances: HashMap::new(),
            queue: vec![],
            checks: HashMap::new(),
            core_only: true,
            fb: Fb::new("", ir::Ty::Unit, HashMap::new(), 0),
        };
        for it in &t.file.items {
            match it {
                a::Item::Fn(d) => {
                    l.fns.insert(d.name.as_str(), d);
                }
                a::Item::Struct(s) => {
                    l.structs.insert(s.name.as_str(), s);
                }
                a::Item::Enum(e) => {
                    l.enums.insert(e.name.as_str(), e);
                }
                a::Item::Type(td) => {
                    l.refined.insert(td.name.as_str(), td);
                }
                a::Item::Impl(im) => {
                    if let Some(tn) = last_name(&im.ty) {
                        for f in &im.fns {
                            l.methods.insert((tn.to_string(), f.name.clone()), f);
                        }
                    }
                }
                a::Item::ImplFor(im) => {
                    if let Some(tn) = last_name(&im.ty) {
                        for f in &im.fns {
                            l.trait_fns.push((
                                im.trait_name.clone(),
                                tn.to_string(),
                                f.name.clone(),
                                im,
                                f,
                            ));
                            // 実体名は `<型のモジュール>.<型>/<トレイトのモジュール>.<トレイト>.<関数>`
                            // （ADR 0022 決定 1。固有のメソッドや、別のトレイトの同じ名前の関数と別の名前になる）。
                            l.names.insert(
                                f as *const a::FnDecl as usize,
                                format!("{tn}/{}.{}", im.trait_name, f.name),
                            );
                        }
                    }
                }
                a::Item::Trait(_) => {}
                a::Item::Global(_) => {}
                a::Item::Properties(_) => {}
            }
        }
        l
    }

    fn run(&mut self, entry: &str) -> R<ir::Program> {
        let Some(&decl) = self.fns.get(entry) else {
            return bail(format!("関数 {entry} がない"));
        };
        if !decl.generics.is_empty() {
            return bail(format!("総称関数 {entry} は入口にできない"));
        }
        // 標準の列挙型を先に登録する（評価器が名前で探す）
        self.adt("Option", vec![ir::Ty::Unit])?;
        self.adt("Result", vec![ir::Ty::Unit, ir::Ty::Unit])?;
        self.adt("IoError", vec![])?;
        let s0 = self.stop(StopKind::StackExhausted, entry, decl.span.line, None, "");
        debug_assert_eq!(s0, 0);
        let fid = self.instance(decl, HashMap::new(), vec![]);
        while let Some(job) = self.queue.pop() {
            let fid = job.fid;
            let f = self.lower_fn(job)?;
            self.funcs[fid] = Some(f);
        }
        let funcs = std::mem::take(&mut self.funcs)
            .into_iter()
            .map(|f| f.ok_or_else(|| LowerError("未生成の関数".into())))
            .collect::<R<Vec<_>>>()?;
        Ok(ir::Program {
            funcs,
            types: std::mem::take(&mut self.types),
            strings: std::mem::take(&mut self.strings),
            stop_sites: std::mem::take(&mut self.stops),
            entry: fid,
            core_only: self.core_only,
            exit_line: decl.span.line,
        })
    }

    // --- 表 ---

    fn stop(
        &mut self,
        kind: StopKind,
        func: &str,
        line: u32,
        during: Option<(String, Clause)>,
        detail: &str,
    ) -> usize {
        let s = ir::StopSite {
            kind,
            func: func.to_string(),
            line,
            during,
            detail: detail.to_string(),
        };
        if let Some(&i) = self.stop_ids.get(&s) {
            return i;
        }
        let i = self.stops.len();
        self.stops.push(s.clone());
        self.stop_ids.insert(s, i);
        i
    }

    /// 現在の地点での停止の地点。
    fn site(&mut self, kind: StopKind, detail: &str) -> usize {
        let (n, l, d) = (self.fb.name.clone(), self.fb.line, self.fb.during.clone());
        self.stop(kind, &n, l, d, detail)
    }

    fn string(&mut self, s: &str) -> usize {
        if let Some(&i) = self.string_ids.get(s) {
            return i;
        }
        let i = self.strings.len();
        self.strings.push(s.to_string());
        self.string_ids.insert(s.to_string(), i);
        i
    }

    fn instance(&mut self, decl: &'a a::FnDecl, mono: Mono, key: Vec<ir::Ty>) -> usize {
        let k = (decl as *const a::FnDecl as usize, key);
        if let Some(&f) = self.instances.get(&k) {
            return f;
        }
        // 単相化の上限（数の上限）は、エラーを返せる `user_call` と `adt` で見る（ADR 0022 決定 2）。
        let fid = self.funcs.len();
        self.funcs.push(None);
        self.instances.insert(k, fid);
        self.queue.push(Job { fid, decl, mono });
        fid
    }

    // --- 型 ---

    fn adt(&mut self, name: &str, args: Vec<ir::Ty>) -> R<usize> {
        let key = (name.to_string(), args.clone());
        if let Some(&i) = self.type_ids.get(&key) {
            return Ok(i);
        }
        if self.funcs.len() + self.types.len() >= MAX_INSTANCES {
            return bail(format!(
                "単相化の上限を超えた（InstanceLimit: 実体（関数と型）の数が上限 {MAX_INSTANCES} を超えた）"
            ));
        }
        let id = self.types.len();
        self.adt_args.insert(id, args.clone());
        self.type_ids.insert(key, id);
        self.types.push(ir::TypeDef {
            name: name.to_string(),
            kind: ir::TypeKind::Struct { fields: vec![] },
        });
        let arg = |i: usize| args.get(i).cloned().unwrap_or(ir::Ty::Unit);
        let v = |n: &str, f: Vec<ir::Ty>| ir::VariantDef {
            name: n.to_string(),
            fields: f,
            indirect: false,
        };
        let kind = match name {
            "Option" => ir::TypeKind::Enum {
                variants: vec![v("Some", vec![arg(0)]), v("None", vec![])],
            },
            "Result" => ir::TypeKind::Enum {
                variants: vec![v("Ok", vec![arg(0)]), v("Err", vec![arg(1)])],
            },
            "IoError" => ir::TypeKind::Enum {
                variants: vec![
                    v("NotFound", vec![ir::Ty::Path]),
                    v("PermissionDenied", vec![ir::Ty::Path]),
                    v("Other", vec![ir::Ty::Path]),
                ],
            },
            "LockBroken" => ir::TypeKind::Struct { fields: vec![] },
            _ => {
                if let Some(&s) = self.structs.get(name) {
                    let m: Mono = s
                        .generics
                        .iter()
                        .map(|g| g.name.clone())
                        .zip(args.iter().cloned())
                        .collect();
                    let mut fields = vec![];
                    for f in &s.fields {
                        fields.push((f.name.clone(), self.conv_ast(&f.ty, &m)?));
                    }
                    ir::TypeKind::Struct { fields }
                } else if let Some(&e) = self.enums.get(name) {
                    let m: Mono = e
                        .generics
                        .iter()
                        .map(|g| g.name.clone())
                        .zip(args.iter().cloned())
                        .collect();
                    let mut variants = vec![];
                    for vr in &e.variants {
                        let mut fs = vec![];
                        for t in &vr.fields {
                            fs.push(self.conv_ast(t, &m)?);
                        }
                        variants.push(ir::VariantDef {
                            name: vr.name.clone(),
                            fields: fs,
                            indirect: vr.fields.iter().any(|t| mentions(t, name)),
                        });
                    }
                    ir::TypeKind::Enum { variants }
                } else {
                    return bail(format!("未知の型 {name}"));
                }
            }
        };
        self.types[id].kind = kind;
        Ok(id)
    }

    fn named(&mut self, n: &str, args: Vec<ir::Ty>) -> R<ir::Ty> {
        let one = |args: &[ir::Ty]| Box::new(args.first().cloned().unwrap_or(ir::Ty::Unit));
        Ok(match n {
            "List" => ir::Ty::List(one(&args)),
            "Path" => ir::Ty::Path,
            "ParseError" => ir::Ty::ParseError,
            "Shared" => ir::Ty::Shared(one(&args)),
            "Cell" => ir::Ty::Cell(one(&args)),
            // 並行処理の取っ手は評価器では番号（ADR 0026）。共有の部品は `Cell` と同じ入れ物。
            "Scope" | "TaskHandle" | "Sender" | "Receiver" => ir::Ty::Int,
            // `clone` が同じ実体を指す（ADR 0026 決定 3）ので、`Shared<Cell<T>>` の形にする。
            "SyncShared" | "Atomic" => ir::Ty::Shared(Box::new(ir::Ty::Cell(one(&args)))),
            // `Lock` は評価器では番号。`LockGuard` は破棄で解放する持ち手なので、破棄される型にする。
            "Lock" => ir::Ty::Int,
            "LockGuard" => ir::Ty::Cell(one(&args)),
            "Int" => ir::Ty::Int,
            "Float" => ir::Ty::Float,
            "Bool" => ir::Ty::Bool,
            "String" => ir::Ty::Str,
            "Bytes" => ir::Ty::Bytes,
            "Addr" => ir::Ty::Addr,
            "Unit" => ir::Ty::Unit,
            "Map" | "Set" => return bail(format!("{n} は SIR の対象外")),
            _ => {
                if let Some(&td) = self.refined.get(n) {
                    return self.conv_ast(&td.base, &HashMap::new());
                }
                ir::Ty::Adt(self.adt(n, args)?)
            }
        })
    }

    fn conv(&mut self, t: &CTy, m: &Mono) -> R<ir::Ty> {
        Ok(match t {
            CTy::Int => ir::Ty::Int,
            CTy::Float => ir::Ty::Float,
            CTy::Bool => ir::Ty::Bool,
            CTy::Str => ir::Ty::Str,
            CTy::Unit => ir::Ty::Unit,
            CTy::Tuple(ts) => {
                if ts.is_empty() {
                    ir::Ty::Unit
                } else {
                    let mut v = vec![];
                    for x in ts {
                        v.push(self.conv(x, m)?);
                    }
                    ir::Ty::Tuple(v)
                }
            }
            CTy::Ref(mu, i) => {
                let i = Box::new(self.conv(i, m)?);
                if *mu {
                    ir::Ty::MutRef(i)
                } else {
                    ir::Ty::Ref(i)
                }
            }
            CTy::Named(n, args) => {
                let mut v = vec![];
                for x in args {
                    v.push(self.conv(x, m)?);
                }
                self.named(n, v)?
            }
            CTy::Param(p) => match m.get(p) {
                Some(t) => t.clone(),
                None => return bail(format!("型引数 {p} が具体化されていない")),
            },
            CTy::Var(_) => ir::Ty::Unit,
            CTy::Fn(ps, r, _) => {
                let mut v = vec![];
                for x in ps {
                    v.push(self.conv(x, m)?);
                }
                ir::Ty::FnRef(v, Box::new(self.conv(r, m)?))
            }
        })
    }

    fn conv_ast(&mut self, t: &a::Type, m: &Mono) -> R<ir::Ty> {
        Ok(match t {
            a::Type::Path { path, args } => {
                let n = path.last().map(|s| s.as_str()).unwrap_or("");
                if path.len() == 1 {
                    if let Some(t) = m.get(n) {
                        return Ok(t.clone());
                    }
                }
                let mut v = vec![];
                for x in args {
                    v.push(self.conv_ast(x, m)?);
                }
                self.named(n, v)?
            }
            a::Type::Ref { mutable, inner } => {
                let i = Box::new(self.conv_ast(inner, m)?);
                if *mutable {
                    ir::Ty::MutRef(i)
                } else {
                    ir::Ty::Ref(i)
                }
            }
            a::Type::Fn { params, ret, .. } => {
                let mut v = vec![];
                for x in params {
                    v.push(self.conv_ast(x, m)?);
                }
                let r = match ret {
                    Some(r) => self.conv_ast(r, m)?,
                    None => ir::Ty::Unit,
                };
                ir::Ty::FnRef(v, Box::new(r))
            }
            a::Type::Tuple(ts) => {
                if ts.is_empty() {
                    ir::Ty::Unit
                } else {
                    let mut v = vec![];
                    for x in ts {
                        v.push(self.conv_ast(x, m)?);
                    }
                    ir::Ty::Tuple(v)
                }
            }
        })
    }

    /// 式の型（現在の単相化の下で）。
    fn ety(&mut self, e: &a::Expr) -> R<ir::Ty> {
        let t = self.t;
        let ct = t
            .expr_ty
            .get(&addr(e))
            .or_else(|| t.expr_ty.get(&addr(strip(e))));
        match ct {
            Some(ct) => {
                let m = self.fb.mono.clone();
                self.conv(ct, &m)
            }
            None => bail("式の型の記録がない"),
        }
    }

    fn field_info(&self, ty: &ir::Ty, name: &str) -> R<(usize, ir::Ty)> {
        match peel(ty) {
            ir::Ty::Adt(tid) => match &self.types[*tid].kind {
                ir::TypeKind::Struct { fields } => match fields.iter().position(|(n, _)| n == name)
                {
                    Some(i) => Ok((i, fields[i].1.clone())),
                    None => bail(format!("フィールド {name} がない")),
                },
                _ => bail(format!("フィールド {name} を持たない型")),
            },
            ir::Ty::Tuple(ts) => match name.parse::<usize>() {
                Ok(i) if i < ts.len() => Ok((i, ts[i].clone())),
                _ => bail(format!("組の添字 {name}")),
            },
            _ => bail(format!("フィールド {name} を持たない型")),
        }
    }
}

enum Arm<'a> {
    B(&'a a::Block),
    E(&'a a::Expr),
}

impl<'a> L<'a> {
    // ---- 関数の組み立て ----

    fn new_local(&mut self, name: &str, ty: ir::Ty) -> usize {
        self.fb.locals.push(ir::Local {
            name: name.to_string(),
            ty,
        });
        self.fb.locals.len() - 1
    }

    fn tmp(&mut self, ty: ir::Ty) -> usize {
        self.new_local("_t", ty)
    }

    fn new_block(&mut self) -> usize {
        self.fb.blocks.push(Bb {
            stmts: vec![],
            term: None,
        });
        self.fb.blocks.len() - 1
    }

    fn emit(&mut self, s: Stmt) {
        let c = self.fb.cur;
        self.fb.blocks[c].stmts.push(s);
    }

    fn assign(&mut self, p: Place, r: Rvalue) {
        self.emit(Stmt::Assign(p, r));
    }

    fn term(&mut self, t: Term) {
        let c = self.fb.cur;
        let b = &mut self.fb.blocks[c];
        if b.term.is_none() {
            b.term = Some(t);
        }
    }

    fn goto(&mut self, b: usize) {
        self.term(Term::Goto(b));
    }

    /// 以降を、どこからも来ない（死んだ）基本ブロックに続ける。
    fn dead(&mut self) {
        let b = self.new_block();
        self.fb.cur = b;
    }

    fn register(&mut self, l: usize, temp: bool) {
        self.fb.reg.push((l, temp));
        self.fb.reg_blocks.push(None);
    }

    fn resume_block(&mut self) -> usize {
        if let Some(b) = self.fb.resume {
            return b;
        }
        let b = self.new_block();
        self.fb.blocks[b].term = Some(Term::Resume);
        self.fb.resume = Some(b);
        b
    }

    /// 登録簿の先頭 `n` 個を逆順に破棄して `Resume` する連鎖の入口。
    fn cleanup_at(&mut self, n: usize) -> usize {
        if n == 0 {
            return self.resume_block();
        }
        if let Some(b) = self.fb.reg_blocks[n - 1] {
            return b;
        }
        let next = self.cleanup_at(n - 1);
        let b = self.new_block();
        let l = self.fb.reg[n - 1].0;
        self.fb.blocks[b].stmts.push(Stmt::Drop(Place::local(l)));
        self.fb.blocks[b].term = Some(Term::Goto(next));
        self.fb.reg_blocks[n - 1] = Some(b);
        b
    }

    /// `mark` 以降に登録した一時の値を逆順に破棄し、登録簿から外す。
    fn flush_temps(&mut self, mark: usize) {
        let idxs: Vec<usize> = (mark..self.fb.reg.len())
            .filter(|&i| self.fb.reg[i].1)
            .collect();
        let Some(&first) = idxs.first() else {
            return;
        };
        for &i in idxs.iter().rev() {
            let l = self.fb.reg[i].0;
            self.emit(Stmt::Drop(Place::local(l)));
        }
        let tail: Vec<(usize, bool)> = self.fb.reg.drain(mark..).filter(|x| !x.1).collect();
        self.fb.reg.extend(tail);
        let n = self.fb.reg.len();
        self.fb.reg_blocks.truncate(first);
        self.fb.reg_blocks.resize(n, None);
    }

    /// `mark` 以降の登録を逆順に破棄する（登録簿は変えない）。
    fn drop_from(&mut self, mark: usize) {
        for i in (mark..self.fb.reg.len()).rev() {
            let l = self.fb.reg[i].0;
            self.emit(Stmt::Drop(Place::local(l)));
        }
    }

    fn enter_scope(&mut self) -> usize {
        self.fb.scopes.push(vec![]);
        self.fb.reg.len()
    }

    fn exit_scope(&mut self, mark: usize) {
        self.drop_from(mark);
        self.fb.reg.truncate(mark);
        self.fb.reg_blocks.truncate(mark);
        self.fb.scopes.pop();
    }

    fn lookup(&self, n: &str) -> Option<Bind> {
        for sc in self.fb.scopes.iter().rev() {
            for b in sc.iter().rev() {
                if b.name == n {
                    return Some(b.clone());
                }
            }
        }
        None
    }

    fn push_bind(&mut self, name: &str, place: Place, ty: ir::Ty) {
        if let Some(sc) = self.fb.scopes.last_mut() {
            sc.push(Bind {
                name: name.to_string(),
                place,
                ty,
            });
        }
    }

    fn finish(&mut self, fb: Fb) -> ir::Func {
        // 終わりのないブロックは、入口から辿れなければ `Unreachable`、辿れるなら（lowering の誤りだが）従来どおり `Return`。
        let mut reach = vec![false; fb.blocks.len()];
        let mut work = vec![0usize];
        while let Some(b) = work.pop() {
            if b < reach.len() && !reach[b] {
                reach[b] = true;
                if let Some(t) = &fb.blocks[b].term {
                    work.extend(t.succs());
                }
            }
        }
        let blocks = fb
            .blocks
            .into_iter()
            .enumerate()
            .map(|(i, b)| ir::Block {
                stmts: b.stmts,
                term: b.term.unwrap_or(if reach[i] {
                    Term::Return
                } else {
                    Term::Unreachable
                }),
            })
            .collect();
        ir::Func {
            name: fb.name,
            params: fb.params,
            ret: fb.ret_ty,
            locals: fb.locals,
            blocks,
        }
    }

    /// `_ret`・入口・終わりの基本ブロックを作る。
    fn start(&mut self) {
        let ret = self.fb.ret_ty.clone();
        let r = self.new_local("_ret", ret.clone());
        debug_assert_eq!(r, 0);
        let entry = self.new_block();
        self.fb.cur = entry;
        self.fb.epilogue = self.new_block();
        if ret == ir::Ty::Unit {
            self.assign(Place::local(0), Rvalue::Use(Operand::Const(Const::Unit)));
        }
    }

    fn mv(&self, p: &Place, ty: &ir::Ty) -> Operand {
        if trivial(ty) {
            Operand::Copy(p.clone())
        } else {
            Operand::Move(p.clone())
        }
    }

    /// 値を新しい一時に入れ、読む操作を返す。
    fn fresh(&mut self, rv: Rvalue, ty: ir::Ty) -> Operand {
        let t = self.tmp(ty.clone());
        self.assign(Place::local(t), rv);
        if needs_drop(&ty) {
            self.register(t, true);
        }
        self.mv(&Place::local(t), &ty)
    }

    fn assert(&mut self, cond: Operand, stop: usize) {
        let cleanup = self.cleanup_at(self.fb.reg.len());
        let next = self.new_block();
        self.term(Term::Assert {
            cond,
            stop,
            next,
            cleanup,
        });
        self.fb.cur = next;
    }
}

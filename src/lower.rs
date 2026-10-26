//! lowering: 検査済みの構文木（`check::Typed`）から SIR（`ir`）を作る（ADR 0015 の決定 2・5）。
//!
//! - 総称関数・総称型は、入口から届く具体的な型ごとに単相化する。
//! - 局所の値の破棄は、登録簿（`Fb::reg`）で追う。停止しうる操作の `cleanup` は、その時点の登録簿を逆順に破棄して
//!   `Resume` する連鎖で、`return`・`?` の早期の抜けも同じ破棄の並び（登録簿の逆順）を通る。
//! - 値の移動は検査器の表（`Typed::moves`）に従う。表にない読みは複製（`Copy`）か借用。

use crate::ast as a;
use crate::check::{ResKind, Resolved, Ty as CTy, Typed};
use crate::exec::{collect_olds, Clause, StopKind};
use crate::ir::{
    self, AggKind, ArithOp, Builtin, Callee, Const, Operand, Place, Rvalue, Stmt, Term,
};
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
        a::Type::Fn { params, ret } => {
            params.iter().any(|x| mentions(x, name))
                || ret.as_deref().is_some_and(|x| mentions(x, name))
        }
        a::Type::Tuple(ts) => ts.iter().any(|x| mentions(x, name)),
    }
}

fn pat_names(p: &a::Pattern, out: &mut Vec<String>) {
    match p {
        a::Pattern::Bind(n) => out.push(n.clone()),
        a::Pattern::Variant { args, .. } => args.iter().for_each(|x| pat_names(x, out)),
        a::Pattern::Tuple(ps) => ps.iter().for_each(|x| pat_names(x, out)),
        _ => {}
    }
}

impl<'a> L<'a> {
    fn new(t: &'a Typed<'a>) -> L<'a> {
        let mut l = L {
            t,
            fns: HashMap::new(),
            methods: HashMap::new(),
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
        let id = self.types.len();
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
            "Int" => ir::Ty::Int,
            "Float" => ir::Ty::Float,
            "Bool" => ir::Ty::Bool,
            "String" => ir::Ty::Str,
            "Unit" => ir::Ty::Unit,
            "Map" | "Set" | "Addr" => return bail(format!("{n} は SIR の対象外")),
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
            CTy::Fn(ps, r) => {
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
            a::Type::Fn { params, ret } => {
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

fn cmp_op(op: a::BinOp) -> ir::BinOp {
    match op {
        a::BinOp::Add => ir::BinOp::Add,
        a::BinOp::Sub => ir::BinOp::Sub,
        a::BinOp::Mul => ir::BinOp::Mul,
        a::BinOp::Div => ir::BinOp::Div,
        a::BinOp::Rem => ir::BinOp::Rem,
        a::BinOp::Eq => ir::BinOp::Eq,
        a::BinOp::Ne => ir::BinOp::Ne,
        a::BinOp::Lt => ir::BinOp::Lt,
        a::BinOp::Le => ir::BinOp::Le,
        a::BinOp::Gt => ir::BinOp::Gt,
        a::BinOp::Ge => ir::BinOp::Ge,
        a::BinOp::And | a::BinOp::Or => ir::BinOp::Eq,
    }
}

// ---- 自由変数（クロージャの捕捉） ----

fn free_expr(e: &a::Expr, bound: &mut Vec<String>, out: &mut Vec<String>) {
    use a::Expr::*;
    match e {
        Name(n) => {
            if !bound.contains(n) && !out.contains(n) {
                out.push(n.clone());
            }
        }
        Old(i) | Paren(i) | Try(i) => free_expr(i, bound, out),
        Tuple(es) => es.iter().for_each(|x| free_expr(x, bound, out)),
        Unary { expr, .. } => free_expr(expr, bound, out),
        Binary { lhs, rhs, .. } => {
            free_expr(lhs, bound, out);
            free_expr(rhs, bound, out);
        }
        Field { expr, .. } => free_expr(expr, bound, out),
        Call { callee, args } => {
            free_expr(callee, bound, out);
            args.iter().for_each(|x| free_expr(x, bound, out));
        }
        StructLit { fields, .. } => fields.iter().for_each(|(_, x)| free_expr(x, bound, out)),
        Match {
            scrutinee, arms, ..
        } => {
            free_expr(scrutinee, bound, out);
            for arm in arms {
                let n = bound.len();
                pat_names(&arm.pattern, bound);
                free_expr(&arm.body, bound, out);
                bound.truncate(n);
            }
        }
        Closure { params, body, .. } => {
            let n = bound.len();
            bound.extend(params.iter().cloned());
            match body {
                a::ClosureBody::Expr(x) => free_expr(x, bound, out),
                a::ClosureBody::Block(b) => free_block(b, bound, out),
            }
            bound.truncate(n);
        }
        _ => {}
    }
}

fn free_block(b: &a::Block, bound: &mut Vec<String>, out: &mut Vec<String>) {
    let n = bound.len();
    for s in &b.stmts {
        free_stmt(s, bound, out);
    }
    bound.truncate(n);
}

fn free_if(i: &a::IfStmt, bound: &mut Vec<String>, out: &mut Vec<String>) {
    free_expr(&i.cond, bound, out);
    free_block(&i.then, bound, out);
    match i.otherwise.as_deref() {
        Some(a::Else::If(j)) => free_if(j, bound, out),
        Some(a::Else::Block(b)) => free_block(b, bound, out),
        None => {}
    }
}

fn free_stmt(s: &a::Stmt, bound: &mut Vec<String>, out: &mut Vec<String>) {
    match &s.kind {
        a::StmtKind::Let { pattern, value, .. } => {
            free_expr(value, bound, out);
            pat_names(pattern, bound);
        }
        a::StmtKind::Assign { target, value } => {
            free_expr(target, bound, out);
            free_expr(value, bound, out);
        }
        a::StmtKind::Expr(e) => free_expr(e, bound, out),
        a::StmtKind::Return(e) => {
            if let Some(e) = e {
                free_expr(e, bound, out)
            }
        }
        a::StmtKind::If(i) => free_if(i, bound, out),
        a::StmtKind::While { cond, body } => {
            free_expr(cond, bound, out);
            free_block(body, bound, out);
        }
        a::StmtKind::Match {
            scrutinee, arms, ..
        } => {
            free_expr(scrutinee, bound, out);
            for arm in arms {
                let n = bound.len();
                pat_names(&arm.pattern, bound);
                free_block(&arm.body, bound, out);
                bound.truncate(n);
            }
        }
    }
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

    fn lower_fn(&mut self, job: Job<'a>) -> R<ir::Func> {
        let decl = job.decl;
        let ret_ty = match &decl.ret {
            Some(t) => self.conv_ast(t, &job.mono)?,
            None => ir::Ty::Unit,
        };
        let fb = Fb::new(&decl.name, ret_ty, job.mono, decl.span.line);
        let saved = std::mem::replace(&mut self.fb, fb);
        let r = self.fn_body(decl);
        let fb = std::mem::replace(&mut self.fb, saved);
        r?;
        Ok(self.finish(fb))
    }

    fn fn_body(&mut self, decl: &'a a::FnDecl) -> R<()> {
        self.start();
        let mono = self.fb.mono.clone();
        let mut pscope = vec![];
        for p in &decl.params {
            let ty = self.conv_ast(&p.ty, &mono)?;
            let l = self.new_local(&p.name, ty.clone());
            self.fb.params.push(l);
            if needs_drop(&ty) {
                self.register(l, false);
            }
            pscope.push(Bind {
                name: p.name.clone(),
                place: Place::local(l),
                ty,
            });
        }
        self.fb.scopes.push(pscope);
        if let Some(r) = &decl.requires {
            self.fb.line = decl.clause_lines[1];
            self.fb.during = Some((decl.name.clone(), Clause::Requires));
            let mark = self.fb.reg.len();
            let op = self.expr(r)?;
            self.flush_temps(mark);
            self.fb.during = None;
            let site = self.site(StopKind::RequiresViolated, "");
            self.assert(op, site);
        }
        let mut olds_exprs = vec![];
        if let Some(en) = &decl.ensures {
            collect_olds(en, &mut olds_exprs);
        }
        if !olds_exprs.is_empty() {
            self.fb.line = decl.clause_lines[2];
            self.fb.during = Some((decl.name.clone(), Clause::Ensures));
            for inner in olds_exprs {
                let mark = self.fb.reg.len();
                let op = self.expr(inner)?;
                let ty = self.ety(inner)?;
                let l = self.new_local("_old", ty.clone());
                self.assign(Place::local(l), Rvalue::Use(op));
                self.flush_temps(mark);
                if needs_drop(&ty) {
                    self.register(l, false);
                }
                self.fb.olds.insert(addr(inner), l);
            }
            self.fb.during = None;
        }
        self.fb.body_base = self.fb.reg.len();
        self.fb.line = decl.body.open_line;
        self.block(&decl.body)?;
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.fb.cur = epi;
        if let Some(en) = &decl.ensures {
            self.fb.line = decl.clause_lines[2];
            self.fb.during = Some((decl.name.clone(), Clause::Ensures));
            self.register(0, false);
            let mark = self.fb.reg.len();
            let op = self.expr(en)?;
            self.flush_temps(mark);
            self.fb.during = None;
            let site = self.site(StopKind::EnsuresViolated, "");
            self.assert(op, site);
            self.fb.reg.pop();
            self.fb.reg_blocks.pop();
        }
        self.drop_from(0);
        self.term(Term::Return);
        Ok(())
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

    // ---- 文 ----

    fn block(&mut self, b: &'a a::Block) -> R<()> {
        let mark = self.enter_scope();
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.exit_scope(mark);
        Ok(())
    }

    fn stmt(&mut self, s: &'a a::Stmt) -> R<()> {
        self.fb.line = s.span.line;
        let mark = self.fb.reg.len();
        match &s.kind {
            a::StmtKind::Let { pattern, value, .. } => {
                let op = self.expr(value)?;
                let ty = match self.t.let_ty.get(&(s as *const a::Stmt as usize)) {
                    Some(ct) => {
                        let m = self.fb.mono.clone();
                        self.conv(ct, &m)?
                    }
                    None => self.ety(value)?,
                };
                match pattern {
                    a::Pattern::Bind(n) => {
                        let l = self.new_local(n, ty.clone());
                        self.assign(Place::local(l), Rvalue::Use(op));
                        if needs_drop(&ty) {
                            self.register(l, false);
                        }
                        self.push_bind(n, Place::local(l), ty);
                    }
                    a::Pattern::Wild => {
                        let t = self.tmp(ty.clone());
                        self.assign(Place::local(t), Rvalue::Use(op));
                        if needs_drop(&ty) {
                            self.register(t, true);
                        }
                    }
                    p => {
                        let t = self.tmp(ty.clone());
                        self.assign(Place::local(t), Rvalue::Use(op));
                        if needs_drop(&ty) {
                            self.register(t, true);
                        }
                        self.pat_bind(p, &Place::local(t), &ty, None)?;
                    }
                }
                self.flush_temps(mark);
            }
            a::StmtKind::Assign { target, value } => {
                let op = self.expr(value)?;
                let Some((mut p, ty)) = self.place_expr(target)? else {
                    return bail("代入先が場所でない");
                };
                if is_ref(&ty) {
                    p = p.deref();
                }
                self.assign(p, Rvalue::Use(op));
                self.flush_temps(mark);
            }
            a::StmtKind::Expr(e) => {
                self.expr(e)?;
                self.flush_temps(mark);
            }
            a::StmtKind::Return(e) => {
                if let Some(e) = e {
                    let op = self.expr(e)?;
                    self.assign(Place::local(0), Rvalue::Use(op));
                }
                self.early_return();
            }
            a::StmtKind::If(i) => {
                let line = s.span.line;
                self.if_stmt(i, line)?;
            }
            a::StmtKind::While { cond, body } => {
                let head = self.new_block();
                let bb = self.new_block();
                let exit = self.new_block();
                self.goto(head);
                self.fb.cur = head;
                self.fb.line = s.span.line;
                let op = self.expr(cond)?;
                self.flush_temps(mark);
                self.term(Term::Branch(op, bb, exit));
                self.fb.cur = bb;
                self.block(body)?;
                self.goto(head);
                self.fb.cur = exit;
            }
            a::StmtKind::Match {
                scrutinee, arms, ..
            } => {
                let arms: Vec<(&a::Pattern, Arm)> =
                    arms.iter().map(|x| (&x.pattern, Arm::B(&x.body))).collect();
                self.match_(scrutinee, arms, None)?;
                self.flush_temps(mark);
            }
        }
        Ok(())
    }

    /// `return`・失敗する `?`: 本体の局所を逆順に破棄して終わりの基本ブロックへ。
    fn early_return(&mut self) {
        let base = self.fb.body_base;
        self.drop_from(base);
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.dead();
    }

    fn if_stmt(&mut self, i: &'a a::IfStmt, line: u32) -> R<()> {
        let mark = self.fb.reg.len();
        let op = self.expr(&i.cond)?;
        self.flush_temps(mark);
        let tb = self.new_block();
        let eb = self.new_block();
        let join = self.new_block();
        self.term(Term::Branch(op, tb, eb));
        self.fb.cur = tb;
        self.block(&i.then)?;
        self.goto(join);
        self.fb.cur = eb;
        match i.otherwise.as_deref() {
            None => {}
            Some(a::Else::Block(b)) => self.block(b)?,
            Some(a::Else::If(j)) => {
                self.fb.line = line;
                self.if_stmt(j, line)?;
            }
        }
        self.goto(join);
        self.fb.cur = join;
        Ok(())
    }

    // ---- match とパターン ----

    fn root_ref(&self, e: &a::Expr) -> Option<bool> {
        match e {
            a::Expr::Paren(i) => self.root_ref(i),
            a::Expr::Field { expr, .. } => self.root_ref(expr),
            a::Expr::Name(n) => match self.lookup(n)?.ty {
                ir::Ty::MutRef(_) => Some(true),
                ir::Ty::Ref(_) => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    fn match_(
        &mut self,
        scrut: &'a a::Expr,
        arms: Vec<(&'a a::Pattern, Arm<'a>)>,
        res: Option<usize>,
    ) -> R<()> {
        let s = strip(scrut);
        let (place, sty, by_ref) = if let a::Expr::Unary {
            op: op @ (a::UnOp::Ref | a::UnOp::RefMut),
            expr: inner,
        } = s
        {
            let (p, ty) = self.expr_place(inner)?;
            (p, ty, Some(*op == a::UnOp::RefMut))
        } else if let Some((p, ty)) = self.place_expr(s)? {
            let br = match &ty {
                ir::Ty::MutRef(_) => Some(true),
                ir::Ty::Ref(_) => Some(false),
                _ => self.root_ref(s),
            };
            (p, ty, br)
        } else {
            let (p, ty) = self.expr_place(s)?;
            let br = match &ty {
                ir::Ty::MutRef(_) => Some(true),
                ir::Ty::Ref(_) => Some(false),
                _ => None,
            };
            (p, ty, br)
        };
        let join = self.new_block();
        for (pat, arm) in arms {
            let fail = self.new_block();
            self.pat_test(pat, &place, &sty, fail)?;
            let mark = self.enter_scope();
            self.pat_bind(pat, &place, &sty, by_ref)?;
            match arm {
                Arm::B(b) => self.block(b)?,
                Arm::E(e) => {
                    let m2 = self.fb.reg.len();
                    let op = self.expr(e)?;
                    if let Some(r) = res {
                        self.assign(Place::local(r), Rvalue::Use(op));
                    }
                    self.flush_temps(m2);
                }
            }
            self.exit_scope(mark);
            self.goto(join);
            self.fb.cur = fail;
        }
        // どの腕にも合わなかった場合（腕がすべて並んでいれば来ない）。
        self.term(Term::Unreachable);
        self.fb.cur = join;
        Ok(())
    }

    fn variant_of(&self, ty: &ir::Ty, name: &str) -> R<(usize, usize, Vec<ir::Ty>)> {
        if let ir::Ty::Adt(tid) = peel(ty) {
            if let ir::TypeKind::Enum { variants } = &self.types[*tid].kind {
                if let Some(i) = variants.iter().position(|v| v.name == name) {
                    return Ok((*tid, i, variants[i].fields.clone()));
                }
            }
        }
        bail(format!("列挙型の腕 {name} がない"))
    }

    fn pat_test(&mut self, p: &a::Pattern, place: &Place, ty: &ir::Ty, fail: usize) -> R<()> {
        match p {
            a::Pattern::Wild | a::Pattern::Bind(_) => {}
            a::Pattern::Int(_)
            | a::Pattern::NegInt(_)
            | a::Pattern::Str(_)
            | a::Pattern::Bool(_) => {
                let k = match p {
                    a::Pattern::Int(s) => Const::Int(int_lit(s)?),
                    a::Pattern::NegInt(s) => {
                        let t = s.replace('_', "");
                        let t = if t.starts_with('-') {
                            t
                        } else {
                            format!("-{t}")
                        };
                        Const::Int(int_lit(&t)?)
                    }
                    a::Pattern::Str(s) => Const::Str(self.string(s)),
                    a::Pattern::Bool(b) => Const::Bool(*b),
                    _ => unreachable!(),
                };
                let c = self.tmp(ir::Ty::Bool);
                self.assign(
                    Place::local(c),
                    Rvalue::Binary(
                        ir::BinOp::Eq,
                        Operand::Copy(place.clone()),
                        Operand::Const(k),
                    ),
                );
                let ok = self.new_block();
                self.term(Term::Branch(Operand::Copy(Place::local(c)), ok, fail));
                self.fb.cur = ok;
            }
            a::Pattern::Tuple(ps) => {
                let ts = match peel(ty) {
                    ir::Ty::Tuple(ts) => ts.clone(),
                    _ => return bail("組のパターンの型"),
                };
                for (i, sp) in ps.iter().enumerate() {
                    let t = ts.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_test(sp, &place.field(i), &t, fail)?;
                }
            }
            a::Pattern::Variant { path, args } => {
                let name = path.last().map(|s| s.as_str()).unwrap_or("");
                let (_, idx, fields) = self.variant_of(ty, name)?;
                let d = self.tmp(ir::Ty::Int);
                self.assign(
                    Place::local(d),
                    Rvalue::Discriminant(Operand::Copy(place.clone())),
                );
                let ok = self.new_block();
                self.term(Term::Switch(
                    Operand::Copy(Place::local(d)),
                    vec![(idx as i64, ok)],
                    fail,
                ));
                self.fb.cur = ok;
                for (i, sp) in args.iter().enumerate() {
                    let t = fields.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_test(sp, &place.downcast(idx).field(i), &t, fail)?;
                }
            }
        }
        Ok(())
    }

    fn pat_bind(
        &mut self,
        p: &a::Pattern,
        place: &Place,
        ty: &ir::Ty,
        by_ref: Option<bool>,
    ) -> R<()> {
        match p {
            a::Pattern::Bind(n) => {
                if let Some(m) = by_ref {
                    let inner = peel(ty).clone();
                    let (bty, rv) = if is_ref(ty) {
                        (ty.clone(), Rvalue::Use(Operand::Copy(place.clone())))
                    } else if m {
                        (
                            ir::Ty::MutRef(Box::new(inner)),
                            Rvalue::RefMut(place.clone()),
                        )
                    } else {
                        (ir::Ty::Ref(Box::new(inner)), Rvalue::Ref(place.clone()))
                    };
                    let l = self.new_local(n, bty.clone());
                    self.assign(Place::local(l), rv);
                    self.push_bind(n, Place::local(l), bty);
                } else {
                    let l = self.new_local(n, ty.clone());
                    let op = self.mv(place, ty);
                    self.assign(Place::local(l), Rvalue::Use(op));
                    if needs_drop(ty) {
                        self.register(l, false);
                    }
                    self.push_bind(n, Place::local(l), ty.clone());
                }
            }
            a::Pattern::Tuple(ps) => {
                let ts = match peel(ty) {
                    ir::Ty::Tuple(ts) => ts.clone(),
                    _ => return bail("組のパターンの型"),
                };
                for (i, sp) in ps.iter().enumerate() {
                    let t = ts.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_bind(sp, &place.field(i), &t, by_ref)?;
                }
            }
            a::Pattern::Variant { path, args } => {
                let name = path.last().map(|s| s.as_str()).unwrap_or("");
                let (_, idx, fields) = self.variant_of(ty, name)?;
                for (i, sp) in args.iter().enumerate() {
                    let t = fields.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_bind(sp, &place.downcast(idx).field(i), &t, by_ref)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

// ---- 式 ----

impl<'a> L<'a> {
    fn moved(&self, e: &a::Expr) -> bool {
        self.t.moves.contains(&addr(e)) || self.t.moves.contains(&addr(strip(e)))
    }

    fn place_op(&self, e: &a::Expr, p: &Place, ty: &ir::Ty) -> Operand {
        if self.moved(e) && !trivial(ty) {
            Operand::Move(p.clone())
        } else {
            Operand::Copy(p.clone())
        }
    }

    fn variant_unit(&mut self, ty: &ir::Ty, name: &str) -> R<Operand> {
        let (tid, idx, fs) = self.variant_of(ty, name)?;
        if !fs.is_empty() {
            return bail(format!("腕 {name} は値を持つ"));
        }
        Ok(self.fresh(
            Rvalue::Aggregate(AggKind::Variant(tid, idx), vec![]),
            ty.clone(),
        ))
    }

    /// 値を持つ腕の構築子を関数の値として使うときの小さな関数（腕ごとに 1 つ）。
    fn ctor_fn(&mut self, ps: &[ir::Ty], ret: &ir::Ty, name: &str) -> R<usize> {
        let (tid, idx, _) = self.variant_of(ret, name)?;
        let key = format!("ctor:{tid}:{idx}");
        if let Some(&f) = self.checks.get(&key) {
            return Ok(f);
        }
        let fid = self.funcs.len();
        self.funcs.push(None);
        self.checks.insert(key, fid);
        let line = self.fb.line;
        let nfb = Fb::new(name, ret.clone(), HashMap::new(), line);
        let outer = std::mem::replace(&mut self.fb, nfb);
        self.start();
        let mut ops = vec![];
        for (i, t) in ps.iter().enumerate() {
            let l = self.new_local(&format!("a{i}"), t.clone());
            self.fb.params.push(l);
            if needs_drop(t) {
                self.register(l, false);
            }
            ops.push((l, t.clone()));
        }
        self.fb.scopes.push(vec![]);
        self.fb.body_base = self.fb.reg.len();
        let mut mvs = vec![];
        for (l, t) in &ops {
            mvs.push(self.mv(&Place::local(*l), t));
        }
        self.assign(
            Place::local(0),
            Rvalue::Aggregate(AggKind::Variant(tid, idx), mvs),
        );
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.fb.cur = epi;
        self.drop_from(0);
        self.term(Term::Return);
        let cfb = std::mem::replace(&mut self.fb, outer);
        let f = self.finish(cfb);
        self.funcs[fid] = Some(f);
        Ok(fid)
    }

    fn expr(&mut self, e: &'a a::Expr) -> R<Operand> {
        match e {
            a::Expr::Int(s) => Ok(Operand::Const(Const::Int(int_lit(s)?))),
            a::Expr::Float(s) => match s.replace('_', "").parse::<f64>() {
                Ok(x) => Ok(Operand::Const(Const::Float(x))),
                Err(_) => bail(format!("Float リテラル {s}")),
            },
            a::Expr::Str(s) => Ok(Operand::Const(Const::Str(self.string(s)))),
            a::Expr::Bool(b) => Ok(Operand::Const(Const::Bool(*b))),
            a::Expr::Name(n) => {
                if let Some(b) = self.lookup(n) {
                    return Ok(self.place_op(e, &b.place, &b.ty));
                }
                if let Some(&decl) = self.fns.get(n.as_str()) {
                    if !decl.generics.is_empty() {
                        return bail(format!("総称関数 {n} を値として使うのは未対応"));
                    }
                    let fid = self.instance(decl, HashMap::new(), vec![]);
                    return Ok(Operand::Const(Const::Fn(fid)));
                }
                let ty = self.ety(e)?;
                self.variant_unit(&ty, n)
            }
            a::Expr::Result => Ok(Operand::Copy(Place::local(0))),
            a::Expr::SelfValue => match self.fb.self_local {
                Some(l) => Ok(Operand::Copy(Place::local(l))),
                None => bail("self の場所がない"),
            },
            a::Expr::Old(i) => match self.fb.olds.get(&addr(i)) {
                Some(&l) => Ok(Operand::Copy(Place::local(l))),
                None => bail("old の記録がない"),
            },
            a::Expr::Paren(i) => self.expr(i),
            a::Expr::Tuple(es) => {
                if es.is_empty() {
                    return Ok(Operand::Const(Const::Unit));
                }
                let ty = self.ety(e)?;
                let mut ops = vec![];
                for x in es {
                    ops.push(self.expr(x)?);
                }
                Ok(self.fresh(Rvalue::Aggregate(AggKind::Tuple, ops), ty))
            }
            a::Expr::Unary { op, expr } => self.unary(e, *op, expr),
            a::Expr::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            a::Expr::Field { expr, name } => {
                if let a::Expr::Name(tn) = strip(expr) {
                    if self.lookup(tn).is_none() && self.enums.contains_key(tn.as_str()) {
                        let ty = self.ety(e)?;
                        if let ir::Ty::FnRef(ps, ret) = &ty {
                            let fid = self.ctor_fn(ps, ret, name)?;
                            return Ok(Operand::Const(Const::Fn(fid)));
                        }
                        return self.variant_unit(&ty, name);
                    }
                }
                match self.place_expr(e)? {
                    Some((p, ty)) => Ok(self.place_op(e, &p, &ty)),
                    None => bail("フィールドの読み"),
                }
            }
            a::Expr::Call { callee, args } => self.call(e, callee, args),
            a::Expr::Try(i) => self.try_expr(i),
            a::Expr::StructLit { fields, .. } => {
                let ty = self.ety(e)?;
                let ir::Ty::Adt(tid) = ty else {
                    return bail("構造体リテラルの型");
                };
                let names: Vec<String> = match &self.types[tid].kind {
                    ir::TypeKind::Struct { fields } => fields.iter().map(|f| f.0.clone()).collect(),
                    _ => return bail("構造体でない"),
                };
                let mut vals: Vec<(String, Operand)> = vec![];
                for (n, x) in fields {
                    let op = self.expr(x)?;
                    vals.push((n.clone(), op));
                }
                let mut ops = vec![];
                for n in &names {
                    match vals.iter().position(|(m, _)| m == n) {
                        Some(i) => ops.push(vals.remove(i).1),
                        None => return bail(format!("フィールド {n} がない")),
                    }
                }
                Ok(self.fresh(Rvalue::Aggregate(AggKind::Struct(tid), ops), ty))
            }
            a::Expr::Match {
                scrutinee, arms, ..
            } => {
                let ty = self.ety(e)?;
                let r = self.tmp(ty.clone());
                if needs_drop(&ty) {
                    self.register(r, true);
                }
                let arms: Vec<(&a::Pattern, Arm)> =
                    arms.iter().map(|x| (&x.pattern, Arm::E(&x.body))).collect();
                self.match_(scrutinee, arms, Some(r))?;
                Ok(self.mv(&Place::local(r), &ty))
            }
            a::Expr::Closure {
                is_move,
                params,
                body,
            } => self.closure(e, *is_move, params, body),
        }
    }

    fn unary(&mut self, e: &'a a::Expr, op: a::UnOp, inner: &'a a::Expr) -> R<Operand> {
        match op {
            a::UnOp::Not => {
                let a = self.expr(inner)?;
                Ok(self.fresh(Rvalue::Unary(ir::UnOp::Not, a), ir::Ty::Bool))
            }
            a::UnOp::Neg => {
                let a = self.expr(inner)?;
                let ty = self.ety(e)?;
                if peel(&ty) == &ir::Ty::Float {
                    Ok(self.fresh(Rvalue::Unary(ir::UnOp::NegFloat, a), ir::Ty::Float))
                } else {
                    Ok(self.checked(ArithOp::Sub, Operand::Const(Const::Int(0)), a, "-"))
                }
            }
            a::UnOp::Ref | a::UnOp::RefMut => {
                let (p, ty) = self.expr_place(inner)?;
                let inner_ty = peel(&ty).clone();
                if op == a::UnOp::Ref {
                    Ok(self.fresh(Rvalue::Ref(p), ir::Ty::Ref(Box::new(inner_ty))))
                } else {
                    Ok(self.fresh(Rvalue::RefMut(p), ir::Ty::MutRef(Box::new(inner_ty))))
                }
            }
        }
    }

    fn checked(&mut self, op: ArithOp, a: Operand, b: Operand, detail: &str) -> Operand {
        if matches!(op, ArithOp::Div | ArithOp::Rem) {
            let c = self.tmp(ir::Ty::Bool);
            self.assign(
                Place::local(c),
                Rvalue::Binary(ir::BinOp::Ne, b.clone(), Operand::Const(Const::Int(0))),
            );
            let site = self.site(StopKind::DivisionByZero, "");
            self.assert(Operand::Copy(Place::local(c)), site);
        }
        let dest = self.tmp(ir::Ty::Int);
        let cleanup = self.cleanup_at(self.fb.reg.len());
        let stop = self.site(StopKind::Overflow, detail);
        let next = self.new_block();
        self.term(Term::CheckedOp {
            op,
            a,
            b,
            dest: Place::local(dest),
            next,
            stop,
            cleanup,
        });
        self.fb.cur = next;
        Operand::Copy(Place::local(dest))
    }

    /// 左辺の値を、右辺の評価で変わらないよう一時に写す。
    fn snapshot(&mut self, op: Operand, ty: &ir::Ty) -> Operand {
        match op {
            Operand::Copy(_) => {
                let t = self.tmp(ty.clone());
                self.assign(Place::local(t), Rvalue::Use(op));
                Operand::Copy(Place::local(t))
            }
            o => o,
        }
    }

    fn binary(&mut self, op: a::BinOp, lhs: &'a a::Expr, rhs: &'a a::Expr) -> R<Operand> {
        if matches!(op, a::BinOp::And | a::BinOp::Or) {
            let r = self.tmp(ir::Ty::Bool);
            let l = self.expr(lhs)?;
            self.assign(Place::local(r), Rvalue::Use(l));
            let more = self.new_block();
            let join = self.new_block();
            let c = Operand::Copy(Place::local(r));
            if op == a::BinOp::And {
                self.term(Term::Branch(c, more, join));
            } else {
                self.term(Term::Branch(c, join, more));
            }
            self.fb.cur = more;
            let rr = self.expr(rhs)?;
            self.assign(Place::local(r), Rvalue::Use(rr));
            self.goto(join);
            self.fb.cur = join;
            return Ok(Operand::Copy(Place::local(r)));
        }
        let lty = self.ety(lhs)?;
        let lty = peel(&lty).clone();
        let simple = matches!(
            strip(rhs),
            a::Expr::Int(_)
                | a::Expr::Float(_)
                | a::Expr::Str(_)
                | a::Expr::Bool(_)
                | a::Expr::Name(_)
        );
        let mut a_ = self.expr(lhs)?;
        if trivial(&lty) && !simple {
            a_ = self.snapshot(a_, &lty);
        }
        let b_ = self.expr(rhs)?;
        let is_cmp = matches!(
            op,
            a::BinOp::Eq | a::BinOp::Ne | a::BinOp::Lt | a::BinOp::Le | a::BinOp::Gt | a::BinOp::Ge
        );
        if is_cmp {
            let scalar = matches!(
                lty,
                ir::Ty::Int | ir::Ty::Float | ir::Ty::Bool | ir::Ty::Str | ir::Ty::Unit
            );
            if scalar {
                return Ok(self.fresh(Rvalue::Binary(cmp_op(op), a_, b_), ir::Ty::Bool));
            }
            let va = self.view_op(a_, &lty);
            let vb = self.view_op(b_, &lty);
            return Ok(self.emit_call(
                Callee::Builtin(Builtin::Compare(cmp_op(op))),
                vec![va, vb],
                ir::Ty::Bool,
            ));
        }
        match lty {
            ir::Ty::Int => {
                let aop = match op {
                    a::BinOp::Add => ArithOp::Add,
                    a::BinOp::Sub => ArithOp::Sub,
                    a::BinOp::Mul => ArithOp::Mul,
                    a::BinOp::Div => ArithOp::Div,
                    _ => ArithOp::Rem,
                };
                Ok(self.checked(aop, a_, b_, ""))
            }
            ir::Ty::Float => Ok(self.fresh(Rvalue::Binary(cmp_op(op), a_, b_), ir::Ty::Float)),
            ir::Ty::Str => Ok(self.fresh(Rvalue::Binary(ir::BinOp::Add, a_, b_), ir::Ty::Str)),
            t => bail(format!("二項演算の型 {t:?}")),
        }
    }

    /// 値の演算子を、読むだけの見え方（`Copy`）にする（`Move` の一時は登録済みなので写しで足りる）。
    fn view_op(&self, op: Operand, _ty: &ir::Ty) -> Operand {
        match op {
            Operand::Move(p) => Operand::Copy(p),
            o => o,
        }
    }

    // ---- 場所 ----

    /// 場所として読める式の場所と型。場所でない式は `None`。
    fn place_expr(&mut self, e: &'a a::Expr) -> R<Option<(Place, ir::Ty)>> {
        match e {
            a::Expr::Name(n) => Ok(self.lookup(n).map(|b| (b.place, b.ty))),
            a::Expr::SelfValue => match self.fb.self_local {
                Some(l) => {
                    let ty = self.fb.locals[l].ty.clone();
                    Ok(Some((Place::local(l), ty)))
                }
                None => Ok(None),
            },
            a::Expr::Result => Ok(Some((Place::local(0), self.fb.ret_ty.clone()))),
            a::Expr::Old(i) => match self.fb.olds.get(&addr(i)) {
                Some(&l) => {
                    let ty = self.fb.locals[l].ty.clone();
                    Ok(Some((Place::local(l), ty)))
                }
                None => Ok(None),
            },
            a::Expr::Paren(i) => self.place_expr(i),
            a::Expr::Field { expr, name } => {
                if let a::Expr::Name(tn) = strip(expr) {
                    if self.lookup(tn).is_none() && self.enums.contains_key(tn.as_str()) {
                        return Ok(None);
                    }
                }
                let (bp, bt) = self.expr_place(expr)?;
                let (i, ft) = self.field_info(&bt, name)?;
                Ok(Some((bp.field(i), ft)))
            }
            _ => Ok(None),
        }
    }

    /// 式を場所にする（場所でなければ評価して一時に入れる）。
    fn expr_place(&mut self, e: &'a a::Expr) -> R<(Place, ir::Ty)> {
        if let Some(r) = self.place_expr(e)? {
            return Ok(r);
        }
        let op = self.expr(e)?;
        let ty = self.ety(e)?;
        if let Operand::Move(p) | Operand::Copy(p) = &op {
            if p.proj.is_empty() && self.fb.locals[p.base].name == "_t" {
                return Ok((p.clone(), ty));
            }
        }
        let t = self.tmp(ty.clone());
        self.assign(Place::local(t), Rvalue::Use(op));
        if needs_drop(&ty) {
            self.register(t, true);
        }
        Ok((Place::local(t), ty))
    }

    // ---- 呼び出し ----

    fn emit_call(&mut self, func: Callee, args: Vec<Operand>, ret: ir::Ty) -> Operand {
        let dest = self.tmp(ret.clone());
        let cleanup = self.cleanup_at(self.fb.reg.len());
        let stop = self.site(StopKind::StackExhausted, "");
        let next = self.new_block();
        self.term(Term::Call {
            func,
            args,
            dest: Place::local(dest),
            next,
            cleanup,
            stop,
        });
        self.fb.cur = next;
        if needs_drop(&ret) {
            self.register(dest, true);
        }
        self.mv(&Place::local(dest), &ret)
    }

    fn user_call(
        &mut self,
        decl: &'a a::FnDecl,
        targs: &[(String, CTy)],
        args: Vec<Operand>,
        ret: ir::Ty,
    ) -> R<Operand> {
        let cm = self.fb.mono.clone();
        let mut mono = Mono::new();
        let mut key = vec![];
        for (n, t) in targs {
            let ty = self.conv(t, &cm)?;
            key.push(ty.clone());
            mono.insert(n.clone(), ty);
        }
        let fid = self.instance(decl, mono, key);
        Ok(self.emit_call(Callee::Func(fid), args, ret))
    }

    fn args_of(&mut self, args: &'a [a::Expr]) -> R<Vec<Operand>> {
        let mut v = vec![];
        for x in args {
            v.push(self.expr(x)?);
        }
        Ok(v)
    }

    fn call(&mut self, e: &'a a::Expr, callee: &'a a::Expr, args: &'a [a::Expr]) -> R<Operand> {
        let Some(r) = self.t.calls.get(&addr(e)).cloned() else {
            return bail("呼び出しの解決の記録がない");
        };
        let ret = self.ety(e)?;
        match r.kind {
            ResKind::Fn => match r.name.as_str() {
                "Some" | "Ok" | "Err" => {
                    let ir::Ty::Adt(tid) = ret else {
                        return bail("構築子の型");
                    };
                    let idx = match r.name.as_str() {
                        "Some" => ir::OPTION_SOME,
                        "Ok" => ir::RESULT_OK,
                        _ => ir::RESULT_ERR,
                    };
                    let ops = self.args_of(args)?;
                    Ok(self.fresh(
                        Rvalue::Aggregate(AggKind::Variant(tid, idx), ops),
                        ir::Ty::Adt(tid),
                    ))
                }
                "to_float" | "to_int" if !self.fns.contains_key(r.name.as_str()) => {
                    let ops = self.args_of(args)?;
                    let b = if r.name == "to_float" {
                        Builtin::ToFloat
                    } else {
                        Builtin::ToInt
                    };
                    Ok(self.emit_call(Callee::Builtin(b), ops, ret))
                }
                n => {
                    let Some(&decl) = self.fns.get(n) else {
                        return bail(format!("関数 {n} がない"));
                    };
                    let ops = self.args_of(args)?;
                    self.user_call(decl, &r.targs, ops, ret)
                }
            },
            ResKind::Local => {
                let a::Expr::Name(n) = strip(callee) else {
                    return bail("局所の関数の呼び出し");
                };
                let Some(b) = self.lookup(n) else {
                    return bail(format!("束縛 {n} がない"));
                };
                let ops = self.args_of(args)?;
                Ok(self.emit_call(Callee::Indirect(Operand::Copy(b.place)), ops, ret))
            }
            ResKind::Namespaced => {
                let ops = self.args_of(args)?;
                self.core_only = false;
                let b = match r.name.as_str() {
                    "console.log" => Builtin::ConsoleLog,
                    "fs.read" => Builtin::FsRead,
                    "fs.write" => Builtin::FsWrite,
                    n => return bail(format!("名前空間の関数 {n}")),
                };
                Ok(self.emit_call(Callee::Builtin(b), ops, ret))
            }
            ResKind::Variant => {
                let ir::Ty::Adt(tid) = ret else {
                    return bail("腕の構築子の型");
                };
                let (_, idx, _) = self.variant_of(&ret, &r.name)?;
                let ops = self.args_of(args)?;
                Ok(self.fresh(
                    Rvalue::Aggregate(AggKind::Variant(tid, idx), ops),
                    ir::Ty::Adt(tid),
                ))
            }
            ResKind::Check => {
                let tn = r.owner.clone().unwrap_or_default();
                let fid = self.check_fn(&tn)?;
                let ops = self.args_of(args)?;
                Ok(self.emit_call(Callee::Func(fid), ops, ret))
            }
            ResKind::TypeFn => {
                let owner = r.owner.clone().unwrap_or_default();
                if let Some(&decl) = self.methods.get(&(owner.clone(), r.name.clone())) {
                    let ops = self.args_of(args)?;
                    return self.user_call(decl, &r.targs, ops, ret);
                }
                let b = match (owner.as_str(), r.name.as_str()) {
                    ("List", "new") => Builtin::ListNew,
                    ("Path", "new") => {
                        self.core_only = false;
                        Builtin::PathNew
                    }
                    ("Cell", "new") => Builtin::CellNew,
                    ("Shared", "new") => Builtin::SharedNew,
                    ("Int", "min_value") => Builtin::IntMinValue,
                    (o, n) => return bail(format!("型の関数 {o}.{n}")),
                };
                let ops = self.args_of(args)?;
                Ok(self.emit_call(Callee::Builtin(b), ops, ret))
            }
            ResKind::Method => {
                let a::Expr::Field { expr: recv, name } = strip(callee) else {
                    return bail("メソッド呼び出しの形");
                };
                self.method_call(recv, name, args, &r, ret)
            }
        }
    }

    fn method_call(
        &mut self,
        recv: &'a a::Expr,
        name: &str,
        args: &'a [a::Expr],
        r: &Resolved,
        ret: ir::Ty,
    ) -> R<Operand> {
        let owner = r.owner.clone().unwrap_or_default();
        if let Some(&decl) = self.methods.get(&(owner.clone(), name.to_string())) {
            let mut ops = vec![];
            match decl.params.first().map(|p| &p.ty) {
                Some(a::Type::Ref { mutable, .. }) => {
                    let (p, ty) = self.expr_place(recv)?;
                    let inner = Box::new(peel(&ty).clone());
                    if *mutable {
                        ops.push(self.fresh(Rvalue::RefMut(p), ir::Ty::MutRef(inner)));
                    } else {
                        ops.push(self.fresh(Rvalue::Ref(p), ir::Ty::Ref(inner)));
                    }
                }
                _ => ops.push(self.expr(recv)?),
            }
            for x in args {
                ops.push(self.expr(x)?);
            }
            return self.user_call(decl, &r.targs, ops, ret);
        }
        if name == "clone" {
            let (p, ty) = self.expr_place(recv)?;
            let ty = peel(&ty).clone();
            return Ok(self.fresh(Rvalue::Clone(Operand::Copy(p), ty.clone()), ty));
        }
        use Builtin::*;
        // (組み込み, 受け手を値で渡すか)
        let (b, by_value) = match (owner.as_str(), name) {
            ("List", "len") => (ListLen, false),
            ("List", "is_empty") => (ListIsEmpty, false),
            ("List", "get") => (ListGet, false),
            ("List", "get_mut") => (ListGetMut, false),
            ("List", "push") => (ListPush, false),
            ("List", "map") => (ListMap, false),
            ("List", "find") => (ListFind, false),
            ("List", "max_by") => (ListMaxBy, false),
            ("List", "min_by") => (ListMinBy, false),
            ("List", "min") => (ListMin, false),
            ("List", "max") => (ListMax, false),
            ("List", "sum") => {
                if matches!(ret, ir::Ty::Float) {
                    (ListSumFloat, false)
                } else {
                    (ListSumInt, false)
                }
            }
            ("Option", "is_some") => (OptIsSome, false),
            ("Option", "is_none") => (OptIsNone, false),
            ("Option", "unwrap_or") => (OptUnwrapOr, true),
            ("Option", "map") => (OptMap, true),
            ("Option", "ok_or") => (OptOkOr, true),
            ("Result", "is_ok") => (ResIsOk, false),
            ("Result", "is_err") => (ResIsErr, false),
            ("Result", "unwrap_or") => (ResUnwrapOr, true),
            ("Result", "ok") => (ResOk, true),
            ("Result", "map") => (ResMap, true),
            ("Result", "map_err") => (ResMapErr, true),
            ("String", "len") => (StrLen, false),
            ("String", "is_empty") => (StrIsEmpty, false),
            ("String", "trim") => (StrTrim, false),
            ("String", "lines") => (StrLines, false),
            ("String", "parse_int") => (StrParseInt, false),
            ("Path", "is_empty") => (PathIsEmpty, false),
            ("Path", "join") => (PathJoin, false),
            ("Path", "file_name") => (PathFileName, false),
            ("Path", "parent") => (PathParent, false),
            ("Shared", "get") => (SharedGet, false),
            ("Shared", "set") => (SharedSet, false),
            (o, n) => return bail(format!("メソッド {o}.{n}")),
        };
        if owner == "Path" {
            self.core_only = false;
        }
        let mut ops = vec![];
        if by_value {
            ops.push(self.expr(recv)?);
        } else {
            let (p, _) = self.expr_place(recv)?;
            ops.push(Operand::Copy(p));
        }
        for x in args {
            ops.push(self.expr(x)?);
        }
        Ok(self.emit_call(Callee::Builtin(b), ops, ret))
    }

    // ---- `?` ----

    fn try_expr(&mut self, inner: &'a a::Expr) -> R<Operand> {
        let op = self.expr(inner)?;
        let ity = self.ety(inner)?;
        let t = self.tmp(ity.clone());
        self.assign(Place::local(t), Rvalue::Use(op));
        self.register(t, true);
        let tp = Place::local(t);
        let d = self.tmp(ir::Ty::Int);
        self.assign(
            Place::local(d),
            Rvalue::Discriminant(Operand::Copy(tp.clone())),
        );
        let ok = self.new_block();
        let fail = self.new_block();
        self.term(Term::Switch(
            Operand::Copy(Place::local(d)),
            vec![(0, ok)],
            fail,
        ));
        self.fb.cur = fail;
        let rty = self.fb.ret_ty.clone();
        let ir::Ty::Adt(rt) = rty else {
            return bail("? の戻り値の型");
        };
        let is_option = self.types[rt].name == "Option";
        if is_option {
            self.assign(
                Place::local(0),
                Rvalue::Aggregate(AggKind::Variant(rt, ir::OPTION_NONE), vec![]),
            );
        } else {
            let (_, _, fs) = self.variant_of(&ity, "Err")?;
            let et = fs.first().cloned().unwrap_or(ir::Ty::Unit);
            let mvd = self.mv(&tp.downcast(ir::RESULT_ERR).field(0), &et);
            self.assign(
                Place::local(0),
                Rvalue::Aggregate(AggKind::Variant(rt, ir::RESULT_ERR), vec![mvd]),
            );
        }
        self.early_return();
        self.fb.cur = ok;
        let (_, _, fs) = self.variant_of(&ity, if is_option { "Some" } else { "Ok" })?;
        let pt = fs.first().cloned().unwrap_or(ir::Ty::Unit);
        Ok(self.mv(&tp.downcast(0).field(0), &pt))
    }

    // ---- クロージャ ----

    fn closure(
        &mut self,
        e: &'a a::Expr,
        is_move: bool,
        params: &'a [String],
        body: &'a a::ClosureBody,
    ) -> R<Operand> {
        let fty = self.ety(e)?;
        let ir::Ty::FnRef(pts, rty) = fty.clone() else {
            return bail("クロージャの型");
        };
        let mut bound: Vec<String> = params.to_vec();
        let mut out = vec![];
        match body {
            a::ClosureBody::Expr(x) => free_expr(x, &mut bound, &mut out),
            a::ClosureBody::Block(b) => free_block(b, &mut bound, &mut out),
        }
        let caps: Vec<Bind> = out.iter().filter_map(|n| self.lookup(n)).collect();
        // 捕捉の演算子と、環境の中での型
        let mut ops = vec![];
        let mut cap_tys = vec![];
        let mut by_ref = vec![];
        for c in &caps {
            if is_move {
                ops.push(self.mv(&c.place, &c.ty));
                cap_tys.push(c.ty.clone());
                by_ref.push(false);
            } else if is_ref(&c.ty) {
                ops.push(Operand::Copy(c.place.clone()));
                cap_tys.push(c.ty.clone());
                by_ref.push(false);
            } else {
                let rt = ir::Ty::Ref(Box::new(c.ty.clone()));
                ops.push(self.fresh(Rvalue::Ref(c.place.clone()), rt.clone()));
                cap_tys.push(rt);
                by_ref.push(true);
            }
        }
        let fid = self.funcs.len();
        self.funcs.push(None);
        let nfb = Fb::new(
            &self.fb.name.clone(),
            (*rty).clone(),
            self.fb.mono.clone(),
            self.fb.line,
        );
        let mut nfb = nfb;
        nfb.during = self.fb.during.clone();
        let outer = std::mem::replace(&mut self.fb, nfb);
        let r = self.closure_body(params, &pts, &caps, &cap_tys, &by_ref, body);
        let cfb = std::mem::replace(&mut self.fb, outer);
        r?;
        let f = self.finish(cfb);
        self.funcs[fid] = Some(f);
        Ok(self.fresh(Rvalue::MakeClosure(fid, ops), fty))
    }

    fn closure_body(
        &mut self,
        params: &'a [String],
        pts: &[ir::Ty],
        caps: &[Bind],
        cap_tys: &[ir::Ty],
        by_ref: &[bool],
        body: &'a a::ClosureBody,
    ) -> R<()> {
        self.start();
        let env_ty = if cap_tys.is_empty() {
            ir::Ty::Unit
        } else {
            ir::Ty::Tuple(cap_tys.to_vec())
        };
        let env = self.new_local("_env", ir::Ty::Ref(Box::new(env_ty)));
        self.fb.params.push(env);
        let mut cscope = vec![];
        for (i, c) in caps.iter().enumerate() {
            let mut p = Place::local(env).deref().field(i);
            if by_ref[i] {
                p = p.deref();
            }
            cscope.push(Bind {
                name: c.name.clone(),
                place: p,
                ty: c.ty.clone(),
            });
        }
        let mut pscope = vec![];
        for (i, n) in params.iter().enumerate() {
            let ty = pts.get(i).cloned().unwrap_or(ir::Ty::Unit);
            let l = self.new_local(n, ty.clone());
            self.fb.params.push(l);
            if needs_drop(&ty) {
                self.register(l, false);
            }
            pscope.push(Bind {
                name: n.clone(),
                place: Place::local(l),
                ty,
            });
        }
        self.fb.scopes.push(cscope);
        self.fb.scopes.push(pscope);
        self.fb.body_base = self.fb.reg.len();
        match body {
            a::ClosureBody::Expr(x) => {
                let mark = self.fb.reg.len();
                let op = self.expr(x)?;
                self.assign(Place::local(0), Rvalue::Use(op));
                self.flush_temps(mark);
            }
            a::ClosureBody::Block(b) => self.block(b)?,
        }
        let epi = self.fb.epilogue;
        self.goto(epi);
        self.fb.cur = epi;
        self.drop_from(0);
        self.term(Term::Return);
        Ok(())
    }

    // ---- 篩型の check ----

    fn check_fn(&mut self, tname: &str) -> R<usize> {
        if let Some(&f) = self.checks.get(tname) {
            return Ok(f);
        }
        let Some(&td) = self.refined.get(tname) else {
            return bail(format!("篩型 {tname} がない"));
        };
        let base = self.conv_ast(&td.base, &HashMap::new())?;
        let opt = self.adt("Option", vec![base.clone()])?;
        let fid = self.funcs.len();
        self.funcs.push(None);
        self.checks.insert(tname.to_string(), fid);
        let nfb = Fb::new(tname, ir::Ty::Adt(opt), HashMap::new(), td.span.line);
        let outer = std::mem::replace(&mut self.fb, nfb);
        let r = self.check_body(td, &base, opt);
        let cfb = std::mem::replace(&mut self.fb, outer);
        r?;
        let f = self.finish(cfb);
        self.funcs[fid] = Some(f);
        Ok(fid)
    }

    fn check_body(&mut self, td: &'a a::TypeDecl, base: &ir::Ty, opt: usize) -> R<()> {
        self.start();
        let p = self.new_local("self", base.clone());
        self.fb.params.push(p);
        if needs_drop(base) {
            self.register(p, false);
        }
        self.fb.self_local = Some(p);
        self.fb.scopes.push(vec![]);
        self.fb.body_base = self.fb.reg.len();
        let mark = self.fb.reg.len();
        let op = self.expr(&td.cond)?;
        self.flush_temps(mark);
        let yes = self.new_block();
        let no = self.new_block();
        self.term(Term::Branch(op, yes, no));
        let epi = self.fb.epilogue;
        self.fb.cur = yes;
        let mvd = self.mv(&Place::local(p), base);
        self.assign(
            Place::local(0),
            Rvalue::Aggregate(AggKind::Variant(opt, ir::OPTION_SOME), vec![mvd]),
        );
        self.goto(epi);
        self.fb.cur = no;
        self.assign(
            Place::local(0),
            Rvalue::Aggregate(AggKind::Variant(opt, ir::OPTION_NONE), vec![]),
        );
        self.goto(epi);
        self.fb.cur = epi;
        self.drop_from(0);
        self.term(Term::Return);
        Ok(())
    }
}

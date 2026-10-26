//! SIR の検証器（ADR 0018 の決定 4、PR 1-4）。
//!
//! lowering の出力（`ir::Program`）を、実行せずに静的に確かめる。確かめるのは次の 3 つである。
//! 1. 型の一致: 代入先と右辺、呼び出しの引数と仮引数・戻り値、組・構造体・列挙型の腕の欄、
//!    条件の式（`Bool`）が、SIR の型の上で食い違っていない。番号（基本ブロック・局所・関数・型・停止の地点・欄）が範囲内にある。
//! 2. 移動した値を後で使っていない: 制御流れ図の上で、移動した場所（局所の全体、または欄の経路）を読んでいない。
//!    合流では「移動したかもしれない」を残す。
//! 3. 破棄が各経路でちょうど 1 回: 戻り（`Return`）と巻き戻しの終わり（`Resume`）で、破棄の対象の局所
//!    （戻り値の `_0` を除く）が生きたまま残っていない（破棄の落とし）。また、破棄済みかもしれない局所（合流の後を含む）をもう一度破棄していない
//!    （移動して空になった場所の `Drop` は、旗の規則で何もしないので数えない）。到達しない腕は `Term::Unreachable` で表し、何も検査しない。
//!
//! 解析は局所ごとの状態（未初期化・生きている・移動して空・破棄済み）の集合を、基本ブロックの入口で合流させる前向きの
//! データフローで行う。`cleanup` の辺は、停止しうる操作の引数を移動した後・結果を書く前の状態から出る。
//! 検査器が付けた式の型との照合は、この版では行わない（SIR は式の型を持たないので、SIR の局所と値の型の整合に置き換えた。
//! ADR の記録に明記）。

use crate::ir::*;
use std::collections::HashMap;

const UNINIT: u8 = 1;
const INIT: u8 = 2;
const MOVED: u8 = 4;
const DROPPED: u8 = 8;

#[derive(Clone, PartialEq, Eq, Default)]
struct State {
    /// 局所ごとの状態の集合（ビット）。
    local: Vec<u8>,
    /// 局所ごとの、移動した欄の経路（局所の全体が移動した場合は `local` の `MOVED` で持つ）。
    paths: Vec<Vec<Vec<Proj>>>,
}

impl State {
    fn join(&mut self, o: &State) -> bool {
        let mut ch = false;
        for (a, b) in self.local.iter_mut().zip(&o.local) {
            if *a | *b != *a {
                *a |= *b;
                ch = true;
            }
        }
        for (a, b) in self.paths.iter_mut().zip(&o.paths) {
            for p in b {
                if !a.contains(p) {
                    a.push(p.clone());
                    ch = true;
                }
            }
        }
        ch
    }
}

/// 検証して、見つけた誤りの一覧を返す（空なら正しい）。
pub fn verify(prog: &Program) -> Result<(), Vec<String>> {
    let mut errs = vec![];
    if prog.entry >= prog.funcs.len() {
        errs.push(format!("入口の関数の番号 {} が範囲外", prog.entry));
    }
    for (fi, f) in prog.funcs.iter().enumerate() {
        let mut v = Fn {
            prog,
            f,
            tag: format!("fn #{fi} {}", f.name),
            errs: vec![],
        };
        v.run();
        errs.extend(v.errs);
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// ソースを SIR にして検証する。SIR にできないときは誤りとして返す（呼び出し側は、SIR にできるソースだけを渡す）。
pub fn verify_sources(srcs: &[&str], func: &str) -> Result<(), Vec<String>> {
    match crate::ir_eval::lower_sources(srcs, func) {
        Ok(p) => verify(&p),
        Err(e) => Err(vec![format!("SIR にできない: {e}")]),
    }
}

struct Fn<'a> {
    prog: &'a Program,
    f: &'a Func,
    tag: String,
    errs: Vec<String>,
}

/// 場所をたどる途中の型。
enum Cur {
    Ty(Ty),
    Variant(TypeId, usize),
}

impl<'a> Fn<'a> {
    fn err(&mut self, bb: usize, msg: String) {
        self.errs.push(format!("{} bb{bb}: {msg}", self.tag));
    }

    // ---- 型 ----

    fn place_ty(&self, p: &Place) -> Option<Ty> {
        let mut cur = Cur::Ty(self.f.locals.get(p.base)?.ty.clone());
        for pr in &p.proj {
            // 参照の指す先の欄は、`Deref` を省いても読める。
            if let (Cur::Ty(Ty::Ref(t) | Ty::MutRef(t)), Proj::Field(_) | Proj::Downcast(_)) =
                (&cur, pr)
            {
                cur = Cur::Ty((**t).clone());
            }
            cur = match (cur, pr) {
                (
                    Cur::Ty(Ty::Ref(t) | Ty::MutRef(t) | Ty::Shared(t) | Ty::Cell(t)),
                    Proj::Deref,
                ) => Cur::Ty(*t),
                (Cur::Ty(Ty::Tuple(ts)), Proj::Field(i)) => Cur::Ty(ts.get(*i)?.clone()),
                (Cur::Ty(Ty::List(t)), Proj::Field(_)) => Cur::Ty(*t),
                (Cur::Ty(Ty::Adt(id)), Proj::Field(i)) => match &self.prog.types.get(id)?.kind {
                    TypeKind::Struct { fields } => Cur::Ty(fields.get(*i)?.1.clone()),
                    TypeKind::Enum { .. } => return None,
                },
                (Cur::Ty(Ty::Adt(id)), Proj::Downcast(v)) => Cur::Variant(id, *v),
                (Cur::Variant(id, v), Proj::Field(i)) => match &self.prog.types.get(id)?.kind {
                    TypeKind::Enum { variants } => {
                        Cur::Ty(variants.get(v)?.fields.get(*i)?.clone())
                    }
                    TypeKind::Struct { .. } => return None,
                },
                _ => return None,
            };
        }
        match cur {
            Cur::Ty(t) => Some(t),
            Cur::Variant(..) => None,
        }
    }

    fn const_ty(&self, c: &Const) -> Option<Ty> {
        Some(match c {
            Const::Unit => Ty::Unit,
            Const::Int(_) => Ty::Int,
            Const::Float(_) => Ty::Float,
            Const::Bool(_) => Ty::Bool,
            Const::Str(_) => Ty::Str,
            Const::Fn(_) => return None,
        })
    }

    fn op_ty(&self, o: &Operand) -> Option<Ty> {
        match o {
            Operand::Copy(p) | Operand::Move(p) => self.place_ty(p),
            Operand::Const(c) => self.const_ty(c),
        }
    }

    fn rv_ty(&self, r: &Rvalue) -> Option<Ty> {
        Some(match r {
            Rvalue::Use(o) => self.op_ty(o)?,
            Rvalue::Unary(UnOp::Not, _) => Ty::Bool,
            Rvalue::Unary(UnOp::NegFloat, _) => Ty::Float,
            Rvalue::Binary(op, a, _) => match op {
                BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => Ty::Bool,
                _ => self.op_ty(a)?,
            },
            Rvalue::Aggregate(AggKind::Tuple, ops) => Ty::Tuple(
                ops.iter()
                    .map(|o| self.op_ty(o))
                    .collect::<Option<Vec<_>>>()?,
            ),
            Rvalue::Aggregate(AggKind::Struct(t) | AggKind::Variant(t, _), _) => Ty::Adt(*t),
            Rvalue::Ref(p) => Ty::Ref(Box::new(self.place_ty(p)?)),
            Rvalue::RefMut(p) => Ty::MutRef(Box::new(self.place_ty(p)?)),
            Rvalue::Clone(_, t) => t.clone(),
            Rvalue::Discriminant(_) => Ty::Int,
            Rvalue::MakeClosure(..) => return None,
        })
    }

    fn expect(&mut self, bb: usize, what: &str, want: Option<Ty>, got: Option<Ty>) {
        // 参照は読むときに指す先として扱われるので、先頭の参照の層は区別しない。
        fn bare(t: Ty) -> Ty {
            match t {
                Ty::Ref(x) | Ty::MutRef(x) => bare(*x),
                t => t,
            }
        }
        if let (Some(w), Some(g)) = (want, got) {
            let (w, g) = (bare(w), bare(g));
            if w != g {
                self.err(bb, format!("{what}の型が合わない: 期待 {w:?}、実際 {g:?}"));
            }
        }
    }

    // ---- 番号の範囲 ----

    fn check_place(&mut self, bb: usize, p: &Place) -> bool {
        if p.base >= self.f.locals.len() {
            self.err(bb, format!("局所の番号 {} が範囲外", p.base));
            return false;
        }
        true
    }

    fn check_block(&mut self, bb: usize, t: BlockId) {
        if t >= self.f.blocks.len() {
            self.err(bb, format!("基本ブロックの番号 {t} が範囲外"));
        }
    }

    // ---- 型の検査 ----

    fn check_types(&mut self, bb: usize, b: &Block) {
        for s in &b.stmts {
            match s {
                Stmt::Assign(d, r) => {
                    if !self.check_place(bb, d) {
                        continue;
                    }
                    self.operands_in_range(bb, &rv_ops(r));
                    let (dt, rt) = (self.place_ty(d), self.rv_ty(r));
                    self.expect(bb, "代入", dt, rt);
                    self.check_agg(bb, r);
                    if let Rvalue::Clone(o, ty) = r {
                        self.expect(bb, "複製の元", Some(ty.clone()), self.op_ty(o));
                    }
                    if let Rvalue::MakeClosure(fid, _) = r {
                        if *fid >= self.prog.funcs.len() {
                            self.err(bb, format!("関数の番号 {fid} が範囲外"));
                        }
                    }
                }
                Stmt::Drop(p) => {
                    self.check_place(bb, p);
                }
            }
        }
        match &b.term {
            Term::Goto(t) => self.check_block(bb, *t),
            Term::Branch(c, a, e) => {
                self.operands_in_range(bb, &[c]);
                self.expect(bb, "分岐の条件", Some(Ty::Bool), self.op_ty(c));
                self.check_block(bb, *a);
                self.check_block(bb, *e);
            }
            Term::Switch(c, arms, d) => {
                self.operands_in_range(bb, &[c]);
                for (_, t) in arms {
                    self.check_block(bb, *t);
                }
                self.check_block(bb, *d);
            }
            Term::Call {
                func,
                args,
                dest,
                next,
                cleanup,
                stop,
            } => {
                let refs: Vec<&Operand> = args.iter().collect();
                self.operands_in_range(bb, &refs);
                self.check_place(bb, dest);
                self.check_block(bb, *next);
                self.check_block(bb, *cleanup);
                if *stop >= self.prog.stop_sites.len() {
                    self.err(bb, format!("停止の地点 {stop} が範囲外"));
                }
                match func {
                    Callee::Func(fid) => match self.prog.funcs.get(*fid) {
                        None => self.err(bb, format!("関数の番号 {fid} が範囲外")),
                        Some(g) => {
                            if g.params.len() != args.len() {
                                self.err(
                                    bb,
                                    format!(
                                        "{} の引数の数が合わない: 期待 {}、実際 {}",
                                        g.name,
                                        g.params.len(),
                                        args.len()
                                    ),
                                );
                            } else {
                                for (a, pl) in args.iter().zip(&g.params) {
                                    let want = g.locals.get(*pl).map(|l| l.ty.clone());
                                    self.expect(
                                        bb,
                                        &format!("{} の引数", g.name),
                                        want,
                                        self.op_ty(a),
                                    );
                                }
                            }
                            self.expect(
                                bb,
                                &format!("{} の戻り値", g.name),
                                Some(g.ret.clone()),
                                self.place_ty(dest),
                            );
                        }
                    },
                    Callee::Indirect(o) => self.operands_in_range(bb, &[o]),
                    Callee::Builtin(_) => {}
                }
            }
            Term::CheckedOp {
                a,
                b: bo,
                dest,
                next,
                stop,
                cleanup,
                ..
            } => {
                self.operands_in_range(bb, &[a, bo]);
                self.check_place(bb, dest);
                self.check_block(bb, *next);
                self.check_block(bb, *cleanup);
                if *stop >= self.prog.stop_sites.len() {
                    self.err(bb, format!("停止の地点 {stop} が範囲外"));
                }
                self.expect(bb, "演算の左辺", Some(Ty::Int), self.op_ty(a));
                self.expect(bb, "演算の右辺", Some(Ty::Int), self.op_ty(bo));
                self.expect(bb, "演算の結果", Some(Ty::Int), self.place_ty(dest));
            }
            Term::Assert {
                cond,
                stop,
                next,
                cleanup,
            } => {
                self.operands_in_range(bb, &[cond]);
                self.check_block(bb, *next);
                self.check_block(bb, *cleanup);
                if *stop >= self.prog.stop_sites.len() {
                    self.err(bb, format!("停止の地点 {stop} が範囲外"));
                }
                self.expect(bb, "表明の条件", Some(Ty::Bool), self.op_ty(cond));
            }
            Term::Return | Term::Resume | Term::Unreachable => {}
        }
    }

    fn operands_in_range(&mut self, bb: usize, ops: &[&Operand]) {
        for o in ops {
            match o {
                Operand::Copy(p) | Operand::Move(p) => {
                    self.check_place(bb, p);
                }
                Operand::Const(Const::Str(s)) if *s >= self.prog.strings.len() => {
                    self.err(bb, format!("文字列の番号 {s} が範囲外"));
                }
                Operand::Const(Const::Fn(f)) if *f >= self.prog.funcs.len() => {
                    self.err(bb, format!("関数の番号 {f} が範囲外"));
                }
                _ => {}
            }
        }
    }

    /// 構造体・列挙型の腕の欄の数と型。
    fn check_agg(&mut self, bb: usize, r: &Rvalue) {
        let Rvalue::Aggregate(k, ops) = r else { return };
        let (tid, fields): (TypeId, Vec<Ty>) = match k {
            AggKind::Tuple => return,
            AggKind::Struct(t) => match self.prog.types.get(*t).map(|d| &d.kind) {
                Some(TypeKind::Struct { fields }) => {
                    (*t, fields.iter().map(|f| f.1.clone()).collect())
                }
                _ => {
                    self.err(bb, format!("型 {t} は構造体でない"));
                    return;
                }
            },
            AggKind::Variant(t, v) => match self.prog.types.get(*t).map(|d| &d.kind) {
                Some(TypeKind::Enum { variants }) if *v < variants.len() => {
                    (*t, variants[*v].fields.clone())
                }
                _ => {
                    self.err(bb, format!("型 {t} に腕 {v} がない"));
                    return;
                }
            },
        };
        if fields.len() != ops.len() {
            self.err(
                bb,
                format!(
                    "型 {tid} の欄の数が合わない: 期待 {}、実際 {}",
                    fields.len(),
                    ops.len()
                ),
            );
            return;
        }
        for (i, (w, o)) in fields.iter().zip(ops).enumerate() {
            self.expect(
                bb,
                &format!("型 {tid} の欄 {i}"),
                Some(w.clone()),
                self.op_ty(o),
            );
        }
    }

    // ---- 移動と破棄のデータフロー ----

    fn read(&mut self, bb: usize, st: &State, p: &Place) {
        if p.base >= st.local.len() {
            return;
        }
        let s = st.local[p.base];
        if s & (MOVED | DROPPED) != 0 {
            self.err(bb, format!("移動または破棄した値 {p} を使っている"));
            return;
        }
        for q in &st.paths[p.base] {
            if q.starts_with(&p.proj) || p.proj.starts_with(q) {
                self.err(bb, format!("移動した欄 {p} を使っている"));
                return;
            }
        }
    }

    fn mv(&mut self, bb: usize, st: &mut State, o: &Operand) {
        match o {
            Operand::Copy(p) => self.read(bb, st, p),
            Operand::Move(p) => {
                self.read(bb, st, p);
                if p.base >= st.local.len() {
                    return;
                }
                if p.proj.is_empty() {
                    st.local[p.base] = MOVED;
                } else if !p.proj.contains(&Proj::Deref) && !st.paths[p.base].contains(&p.proj) {
                    st.paths[p.base].push(p.proj.clone());
                }
            }
            Operand::Const(_) => {}
        }
    }

    fn write(&mut self, st: &mut State, d: &Place) {
        if d.base >= st.local.len() {
            return;
        }
        if d.proj.is_empty() {
            st.local[d.base] = INIT;
            st.paths[d.base].clear();
        } else {
            st.paths[d.base].retain(|q| !q.starts_with(&d.proj));
        }
    }

    fn stmt(&mut self, bb: usize, st: &mut State, s: &Stmt) {
        match s {
            Stmt::Assign(d, r) => {
                for o in rv_ops(r) {
                    self.mv(bb, st, o);
                }
                match r {
                    Rvalue::Ref(p) | Rvalue::RefMut(p) => self.read(bb, st, p),
                    _ => {}
                }
                self.write(st, d);
            }
            Stmt::Drop(p) => {
                if p.base >= st.local.len() {
                    return;
                }
                if p.proj.is_empty() {
                    // 移動済み（`MOVED` だけ）の破棄は何もしない。破棄済みの可能性がある（`DROPPED` を含む）ものは、合流の後でも誤り。
                    if st.local[p.base] & DROPPED != 0 {
                        self.err(
                            bb,
                            format!("すでに破棄したかもしれない {p} をもう一度破棄している"),
                        );
                    }
                    st.local[p.base] = DROPPED;
                    st.paths[p.base].clear();
                } else if !st.paths[p.base].contains(&p.proj) {
                    st.paths[p.base].push(p.proj.clone());
                }
            }
        }
    }

    fn leaks(&mut self, bb: usize, st: &State, what: &str) {
        for (i, l) in self.f.locals.iter().enumerate() {
            if i == 0 {
                continue;
            }
            if crate::lower::needs_drop(&l.ty) && st.local[i] & INIT != 0 {
                self.err(
                    bb,
                    format!("{what}で局所 _{i} ({}) が破棄されていない", l.name),
                );
            }
        }
    }

    fn run(&mut self) {
        let n = self.f.blocks.len();
        if n == 0 {
            self.errs.push(format!("{}: 基本ブロックがない", self.tag));
            return;
        }
        for (bb, b) in self.f.blocks.iter().enumerate() {
            self.check_types(bb, b);
            // `Unreachable` は `Branch`・`Switch` の腕としてだけ指される。
            if !matches!(b.term, Term::Branch(..) | Term::Switch(..)) {
                for t in b.term.succs() {
                    if self
                        .f
                        .blocks
                        .get(t)
                        .is_some_and(|x| x.term == Term::Unreachable)
                    {
                        self.err(
                            bb,
                            format!("到達しない基本ブロック bb{t} へ分岐以外の辺で続いている"),
                        );
                    }
                }
            }
        }
        if !self.errs.is_empty() {
            return;
        }
        let nl = self.f.locals.len();
        let mut init = State {
            local: vec![UNINIT; nl],
            paths: vec![vec![]; nl],
        };
        for p in &self.f.params {
            if *p < nl {
                init.local[*p] = INIT;
            }
        }
        let mut ins: HashMap<usize, State> = HashMap::new();
        ins.insert(0, init);
        let mut work = vec![0usize];
        // 誤りは最後の（不動点の）状態で一度だけ出す。
        while let Some(bb) = work.pop() {
            let mut st = ins[&bb].clone();
            let saved = std::mem::take(&mut self.errs);
            let succ = self.block(bb, &mut st, false);
            self.errs = saved;
            for (t, s) in succ {
                match ins.get_mut(&t) {
                    Some(old) => {
                        if old.join(&s) {
                            work.push(t);
                        }
                    }
                    None => {
                        ins.insert(t, s);
                        work.push(t);
                    }
                }
            }
        }
        let mut order: Vec<usize> = ins.keys().copied().collect();
        order.sort();
        for bb in order {
            let mut st = ins[&bb].clone();
            self.block(bb, &mut st, true);
        }
    }

    /// 基本ブロックを実行して、後続の（ブロック, 状態）を返す。`report` が偽なら誤りは捨てる（呼び出し側で）。
    fn block(&mut self, bb: usize, st: &mut State, _report: bool) -> Vec<(usize, State)> {
        let b = &self.f.blocks[bb];
        for s in &b.stmts {
            self.stmt(bb, st, s);
        }
        let mut out = vec![];
        match &b.term {
            Term::Goto(t) => out.push((*t, st.clone())),
            Term::Branch(c, a, e) => {
                self.mv(bb, st, c);
                for t in [a, e] {
                    out.push((*t, st.clone()));
                }
            }
            Term::Switch(c, arms, d) => {
                self.mv(bb, st, c);
                for (_, t) in arms {
                    out.push((*t, st.clone()));
                }
                out.push((*d, st.clone()));
            }
            Term::Call {
                func,
                args,
                dest,
                next,
                cleanup,
                ..
            } => {
                if let Callee::Indirect(o) = func {
                    self.mv(bb, st, o);
                }
                for a in args {
                    self.mv(bb, st, a);
                }
                out.push((*cleanup, st.clone()));
                self.write(st, dest);
                out.push((*next, st.clone()));
            }
            Term::CheckedOp {
                a,
                b: bo,
                dest,
                next,
                cleanup,
                ..
            } => {
                self.mv(bb, st, a);
                self.mv(bb, st, bo);
                out.push((*cleanup, st.clone()));
                self.write(st, dest);
                out.push((*next, st.clone()));
            }
            Term::Assert {
                cond,
                next,
                cleanup,
                ..
            } => {
                self.mv(bb, st, cond);
                out.push((*cleanup, st.clone()));
                out.push((*next, st.clone()));
            }
            Term::Return => {
                self.leaks(bb, st, "戻り");
                if self.f.ret != Ty::Unit && st.local[0] & INIT == 0 {
                    self.err(bb, "戻り値 _0 が書かれていない".to_string());
                }
            }
            Term::Resume => self.leaks(bb, st, "巻き戻しの終わり"),
            // 来ない基本ブロック。何も検査しない（ここへの辺は、戻りへの辺と違って漏れを隠さない）。
            Term::Unreachable => {}
        }
        out
    }
}

fn rv_ops(r: &Rvalue) -> Vec<&Operand> {
    match r {
        Rvalue::Use(o) | Rvalue::Unary(_, o) | Rvalue::Clone(o, _) | Rvalue::Discriminant(o) => {
            vec![o]
        }
        Rvalue::Binary(_, a, b) => vec![a, b],
        Rvalue::Aggregate(_, ops) | Rvalue::MakeClosure(_, ops) => ops.iter().collect(),
        Rvalue::Ref(_) | Rvalue::RefMut(_) => vec![],
    }
}

use crate::ast as a;
use crate::check::{ResKind, Resolved, Ty as CTy};
use crate::exec::StopKind;
use crate::ir::{self, AggKind, ArithOp, Builtin, Callee, Const, Operand, Place, Rvalue, Term};
use std::collections::HashMap;

use super::{
    addr, bail, int_lit, needs_drop, peel, strip, trivial, Arm, Fb, Mono, L, MAX_INSTANCES,
    MAX_TYPE_DEPTH, R,
};

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
        a::BinOp::BitAnd => ir::BinOp::BitAnd,
        a::BinOp::BitOr => ir::BinOp::BitOr,
        a::BinOp::BitXor => ir::BinOp::BitXor,
        a::BinOp::Shl | a::BinOp::Shr => unreachable!("shift is arith"),
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

    pub(super) fn expr(&mut self, e: &'a a::Expr) -> R<Operand> {
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
            a::UnOp::BitNot => {
                let a = self.expr(inner)?;
                Ok(self.fresh(Rvalue::Unary(ir::UnOp::BitNot, a), ir::Ty::Int))
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
            ir::Ty::Int => match op {
                a::BinOp::BitAnd | a::BinOp::BitOr | a::BinOp::BitXor => {
                    Ok(self.fresh(Rvalue::Binary(cmp_op(op), a_, b_), ir::Ty::Int))
                }
                a::BinOp::Shl => Ok(self.checked(ArithOp::Shl, a_, b_, "<<")),
                a::BinOp::Shr => Ok(self.checked(ArithOp::Shr, a_, b_, ">>")),
                _ => {
                    let aop = match op {
                        a::BinOp::Add => ArithOp::Add,
                        a::BinOp::Sub => ArithOp::Sub,
                        a::BinOp::Mul => ArithOp::Mul,
                        a::BinOp::Div => ArithOp::Div,
                        _ => ArithOp::Rem,
                    };
                    Ok(self.checked(aop, a_, b_, ""))
                }
            },
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
    pub(super) fn place_expr(&mut self, e: &'a a::Expr) -> R<Option<(Place, ir::Ty)>> {
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
    pub(super) fn expr_place(&mut self, e: &'a a::Expr) -> R<(Place, ir::Ty)> {
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
        // 単相化の上限（ADR 0022 決定 2）: 鍵の型引数の入れ子の深さ。
        fn cty_depth(t: &CTy) -> usize {
            match t {
                CTy::Named(_, a) => 1 + a.iter().map(cty_depth).max().unwrap_or(0),
                CTy::Ref(_, i) => 1 + cty_depth(i),
                CTy::Tuple(ts) => 1 + ts.iter().map(cty_depth).max().unwrap_or(0),
                CTy::Fn(ps, r, _) => {
                    1 + ps
                        .iter()
                        .map(cty_depth)
                        .max()
                        .unwrap_or(0)
                        .max(cty_depth(r))
                }
                _ => 0,
            }
        }
        if targs.iter().any(|(_, t)| cty_depth(t) > MAX_TYPE_DEPTH) {
            return bail(format!(
                "単相化の上限を超えた（InstanceLimit: 型引数の入れ子の深さが上限 {MAX_TYPE_DEPTH} を超えた）"
            ));
        }
        let cm = self.fb.mono.clone();
        let mut mono = Mono::new();
        let mut key = vec![];
        for (n, t) in targs {
            let ty = self.conv(t, &cm)?;
            key.push(ty.clone());
            mono.insert(n.clone(), ty);
        }
        self.user_call_ir(decl, mono, key, args, ret)
    }

    fn user_call_ir(
        &mut self,
        decl: &'a a::FnDecl,
        mono: Mono,
        key: Vec<ir::Ty>,
        args: Vec<Operand>,
        ret: ir::Ty,
    ) -> R<Operand> {
        if !self
            .instances
            .contains_key(&(decl as *const a::FnDecl as usize, key.clone()))
            && self.funcs.len() + self.types.len() >= MAX_INSTANCES
        {
            return bail(format!(
                "単相化の上限を超えた（InstanceLimit: 実体（関数と型）の数が上限 {MAX_INSTANCES} を超えた）"
            ));
        }
        let fid = self.instance(decl, mono, key);
        Ok(self.emit_call(Callee::Func(fid), args, ret))
    }

    /// トレイトの実装の関数を探す（トレイトの名前・型の頭・関数の名前。ADR 0022 決定 2）。
    fn find_trait_fn(
        &self,
        trait_name: &str,
        head: &str,
        name: &str,
    ) -> Option<(&'a a::ImplForDecl, &'a a::FnDecl)> {
        self.trait_fns
            .iter()
            .find(|(t, h, n, _, _)| t == trait_name && h == head && n == name)
            .map(|(_, _, _, im, f)| (*im, *f))
    }

    /// SIR の型の頭の名前（トレイトの実装を探す鍵。ADR 0022 決定 2）。
    fn ir_head(&self, t: &ir::Ty) -> String {
        match t {
            ir::Ty::Adt(id) => self.types[*id].name.clone(),
            ir::Ty::Int => "Int".into(),
            ir::Ty::Float => "Float".into(),
            ir::Ty::Bool => "Bool".into(),
            ir::Ty::Str => "String".into(),
            ir::Ty::List(_) => "List".into(),
            ir::Ty::Path => "Path".into(),
            ir::Ty::ParseError => "ParseError".into(),
            ir::Ty::Shared(_) => "Shared".into(),
            ir::Ty::Cell(_) => "Cell".into(),
            ir::Ty::Ref(i) | ir::Ty::MutRef(i) => self.ir_head(i),
            _ => String::new(),
        }
    }

    /// SIR の型の型引数（`Adt` は登録時のもの、それ以外は中身）。
    fn ir_args(&self, t: &ir::Ty) -> Vec<ir::Ty> {
        match t {
            ir::Ty::Adt(id) => self.adt_args.get(id).cloned().unwrap_or_default(),
            ir::Ty::List(x) | ir::Ty::Shared(x) | ir::Ty::Cell(x) => vec![(**x).clone()],
            ir::Ty::Tuple(ts) => ts.clone(),
            _ => vec![],
        }
    }

    /// 実装の型のパターン（型引数を含む AST の型）を具体的な型に対応させ、単相化の対応を作る。
    fn impl_mono(&mut self, im: &a::ImplForDecl, concrete: &ir::Ty) -> R<(Mono, Vec<ir::Ty>)> {
        fn go(l: &mut L, t: &a::Type, c: &ir::Ty, gens: &[String], mono: &mut Mono) -> R<()> {
            match t {
                a::Type::Path { path, args } => {
                    let name = path.last().map(String::as_str).unwrap_or("");
                    if args.is_empty() && gens.iter().any(|g| g == name) {
                        mono.insert(name.to_string(), c.clone());
                        return Ok(());
                    }
                    if l.ir_head(c) != name {
                        return bail(format!(
                            "トレイトの実装の型 `{name}` と具体的な型が合わない"
                        ));
                    }
                    let ca = l.ir_args(c);
                    if args.len() != ca.len() {
                        return bail("トレイトの実装の型引数の数が合わない");
                    }
                    for (a, cc) in args.iter().zip(ca.iter()) {
                        go(l, a, cc, gens, mono)?;
                    }
                    Ok(())
                }
                a::Type::Ref { inner, .. } => {
                    let inner_c = match c {
                        ir::Ty::Ref(i) | ir::Ty::MutRef(i) => (**i).clone(),
                        other => other.clone(),
                    };
                    go(l, inner, &inner_c, gens, mono)
                }
                a::Type::Tuple(ts) => {
                    let ca = l.ir_args(c);
                    for (a, cc) in ts.iter().zip(ca.iter()) {
                        go(l, a, cc, gens, mono)?;
                    }
                    Ok(())
                }
                a::Type::Fn { .. } => bail("トレイトの実装の型に関数の型は書けない"),
            }
        }
        let gens: Vec<String> = im.generics.iter().map(|g| g.name.clone()).collect();
        let mut mono = Mono::new();
        go(self, &im.ty, concrete, &gens, &mut mono)?;
        let mut key = vec![];
        for g in &gens {
            let Some(t) = mono.get(g) else {
                return bail(format!(
                    "トレイトの実装の型引数 `{g}` が具体的な型から決まらない"
                ));
            };
            key.push(t.clone());
        }
        Ok((mono, key))
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
            ResKind::TraitMethod => {
                let a::Expr::Field { expr: recv, name } = strip(callee) else {
                    return bail("メソッド呼び出しの形");
                };
                // `トレイト.m(x, ...)` の修飾形は、受け手が最初の引数（ADR 0022 決定 2）。
                if let a::Expr::Name(n) = strip(recv) {
                    if r.owner.as_deref() == Some(n.as_str()) {
                        let Some((first, rest)) = args.split_first() else {
                            return bail("トレイトで選ぶ呼び出しに受け手がない");
                        };
                        return self.method_call(first, name, rest, &r, ret);
                    }
                }
                self.method_call(recv, name, args, &r, ret)
            }
            ResKind::TraitTypeFn => {
                let a::Expr::Field {
                    expr: head_expr,
                    name,
                } = strip(callee)
                else {
                    return bail("型に属する関数の呼び出しの形");
                };
                let trait_name = r.owner.clone().unwrap_or_default();
                let ops = self.args_of(args)?;
                // 型引数 `T.f()`（検査側が単一の（型引数, Param）対応を 1 つ残す形）か、
                // 具体的な型 `型.f()` かで、実装の探し方が違う（ADR 0022 決定 2）。
                if r.targs.len() == 1
                    && matches!(&r.targs[0].1, CTy::Param(q) if q == &r.targs[0].0)
                {
                    let pname = r.targs[0].0.clone();
                    let Some(cty) = self.fb.mono.get(&pname).cloned() else {
                        return bail(format!("型引数 `{pname}` の具体的な型が決まらない"));
                    };
                    let head = self.ir_head(&cty);
                    let Some((im, decl)) = self.find_trait_fn(&trait_name, &head, name) else {
                        return bail(format!(
                            "型 `{head}` のトレイト `{trait_name}` の実装に関数 `{name}` がない"
                        ));
                    };
                    let (mono, key) = self.impl_mono(im, &cty)?;
                    return self.user_call_ir(decl, mono, key, ops, ret);
                }
                let a::Expr::Name(head) = strip(head_expr) else {
                    return bail("型に属する関数の呼び出しの形");
                };
                let Some((_, decl)) = self.find_trait_fn(&trait_name, head, name) else {
                    return bail(format!(
                        "型 `{head}` のトレイト `{trait_name}` の実装に関数 `{name}` がない"
                    ));
                };
                self.user_call(decl, &r.targs, ops, ret)
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
        // トレイトの実装のメソッド（ADR 0022 決定 2）: 受け手の具体的な型の頭で実装を探す。
        // `トレイト.m(x)` の修飾形も、検査側が同じ解決（TraitMethod）を残すのでここへ来る。
        if r.kind == ResKind::TraitMethod {
            let rty = self.ety(recv)?;
            let peeled = peel(&rty).clone();
            let head = self.ir_head(&peeled);
            let trait_name = r.owner.clone().unwrap_or_default();
            let Some((_, decl)) = self.find_trait_fn(&trait_name, &head, name) else {
                return bail(format!(
                    "型 `{head}` のトレイト `{trait_name}` の実装にメソッド `{name}` がない"
                ));
            };
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
}

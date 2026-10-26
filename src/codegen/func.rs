//! 関数 1 つ分の出力（SIR の基本ブロック → Cranelift）。値の持ち方は親モジュールの説明を見よ。

use super::{norm, rt_spec, Cg, CgError, W};
use crate::interp::StopKind;
use crate::ir::{
    AggKind, ArithOp, BinOp, Builtin, Callee, Const, Func, Operand, Place, Proj, Rvalue, Stmt,
    Term, Ty, TypeKind, UnOp,
};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::Block as CBlock;
use cranelift_codegen::ir::{
    types, BlockArg, FuncRef, InstBuilder, MemFlagsData, SigRef, StackSlot, StackSlotData,
    StackSlotKind, Value,
};
use cranelift_frontend::FunctionBuilder;
use cranelift_module::{DataId, Module};
use std::collections::HashMap;

const NOARGS: &[BlockArg] = &[];

/// 場所をたどった途中の姿。`T` は型つきの語、`V` は列挙型の値を腕として見ているところ。
enum Cur {
    T(Ty),
    V(usize, usize),
}

/// 値の複写（影）と破棄が必要な型。文字列・パス（`RtStr`）も含む。スカラー・参照は含まない。
fn aggregate(t: &Ty) -> bool {
    matches!(
        t,
        Ty::Tuple(_)
            | Ty::Adt(_)
            | Ty::List(_)
            | Ty::Cell(_)
            | Ty::Shared(_)
            | Ty::FnRef(..)
            | Ty::Str
            | Ty::Path
    )
}

fn strip(t: &Ty) -> Ty {
    match t {
        Ty::Ref(i) | Ty::MutRef(i) => strip(i),
        other => other.clone(),
    }
}

pub(super) struct Fx<'a, 'p> {
    pub cg: &'a mut Cg<'p>,
    pub b: FunctionBuilder<'a>,
    lt: Vec<Ty>,
    slots: Vec<StackSlot>,
    pending: Option<StackSlot>,
    blocks: Vec<CBlock>,
    rtref: HashMap<&'static str, FuncRef>,
    sigs: HashMap<usize, SigRef>,
    pub err: Option<CgError>,
    line: u32,
    cleanup: Option<CBlock>,
    stack_code: i64,
}

impl<'a, 'p> Fx<'a, 'p> {
    pub fn new(cg: &'a mut Cg<'p>, b: FunctionBuilder<'a>, lt: Vec<Ty>) -> Self {
        Fx {
            cg,
            b,
            lt,
            slots: vec![],
            pending: None,
            blocks: vec![],
            rtref: HashMap::new(),
            sigs: HashMap::new(),
            err: None,
            line: 0,
            cleanup: None,
            stack_code: 0,
        }
    }

    // --- 小さな部品 ---

    fn fail(&mut self, e: CgError) -> Value {
        if self.err.is_none() {
            self.err = Some(e);
        }
        self.ic(0)
    }

    fn unsupported(&mut self, msg: &str) -> Value {
        let line = self.line;
        self.fail(CgError::Internal(format!("{msg}（{line} 行付近）")))
    }

    fn ic(&mut self, n: i64) -> Value {
        self.b.ins().iconst(types::I64, n)
    }

    fn ld(&mut self, addr: Value, off: i32) -> Value {
        self.b
            .ins()
            .load(types::I64, MemFlagsData::new(), addr, off)
    }

    fn st(&mut self, v: Value, addr: Value, off: i32) {
        self.b.ins().store(MemFlagsData::new(), v, addr, off);
    }

    fn tmp(&mut self, bytes: u32) -> StackSlot {
        self.b
            .create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, bytes, 3))
    }

    fn saddr(&mut self, s: StackSlot) -> Value {
        self.b.ins().stack_addr(types::I64, s, 0)
    }

    fn zero_tmp(&mut self, bytes: u32) -> Value {
        let s = self.tmp(bytes);
        let a = self.saddr(s);
        let z = self.ic(0);
        for i in 0..bytes / 8 {
            self.st(z, a, (i * 8) as i32);
        }
        a
    }

    fn b2i(&mut self, c: Value) -> Value {
        self.b.ins().uextend(types::I64, c)
    }

    fn data_addr(&mut self, d: DataId) -> Value {
        let gv = self.cg.m.declare_data_in_func(d, self.b.func);
        self.b.ins().symbol_value(types::I64, gv)
    }

    fn desc_addr(&mut self, t: &Ty) -> Value {
        match self.cg.desc(t) {
            Ok(d) => self.data_addr(d),
            Err(e) => self.fail(e),
        }
    }

    fn rt(&mut self, name: &'static str, args: &[Value]) -> Value {
        let Some((ps, r)) = rt_spec(name) else {
            return self.fail(CgError::Internal(format!("ランタイム関数 {name}")));
        };
        let fr = if let Some(f) = self.rtref.get(name) {
            *f
        } else {
            match self.cg.rt_id(name) {
                Ok(id) => {
                    let f = self.cg.m.declare_func_in_func(id, self.b.func);
                    self.rtref.insert(name, f);
                    f
                }
                Err(e) => return self.fail(e),
            }
        };
        let mut av = Vec::with_capacity(args.len());
        for (c, a) in ps.chars().zip(args) {
            av.push(match c {
                'i' => self.b.ins().ireduce(types::I32, *a),
                'f' => self.b.ins().bitcast(types::F64, MemFlagsData::new(), *a),
                _ => *a,
            });
        }
        let inst = self.b.ins().call(fr, &av);
        match r {
            'v' => self.ic(0),
            'p' => self.b.inst_results(inst)[0],
            'i' => {
                let v = self.b.inst_results(inst)[0];
                self.b.ins().sextend(types::I64, v)
            }
            _ => {
                let v = self.b.inst_results(inst)[0];
                self.b.ins().bitcast(types::I64, MemFlagsData::new(), v)
            }
        }
    }

    fn sigref(&mut self, n: usize) -> SigRef {
        if let Some(s) = self.sigs.get(&n) {
            return *s;
        }
        let sig = self.cg.sig(n);
        let s = self.b.import_signature(sig);
        self.sigs.insert(n, s);
        s
    }

    fn shadow(&mut self, t: &Ty, w: Value) -> Value {
        let d = self.desc_addr(t);
        self.rt("val_shadow", &[d, w])
    }

    /// 見出しつきの塊（`sigil_rt_val_drop` が解放する）。
    fn blk(&mut self, bytes: i64) -> Value {
        let sz = self.ic(bytes);
        self.rt("blk_new", &[sz])
    }

    /// 型 `t` の値 `w` を破棄する（0 は移動済みで何もしない）。
    fn drop_val(&mut self, t: &Ty, w: Value) {
        if !aggregate(t) {
            return;
        }
        let d = self.desc_addr(&strip(t));
        self.rt("val_drop", &[d, w]);
    }

    fn mk_enum(&mut self, tag: Value, fields: &[Value]) -> Value {
        let size = std::cmp::max(16, 8 + 8 * fields.len() as i64);
        let p = self.blk(size);
        self.st(tag, p, 0);
        for (i, f) in fields.iter().enumerate() {
            self.st(*f, p, 8 + 8 * i as i32);
        }
        p
    }

    fn mk_enum_c(&mut self, tag: i64, fields: &[Value]) -> Value {
        let t = self.ic(tag);
        self.mk_enum(t, fields)
    }

    // --- 制御の部品 ---

    fn jump_cleanup(&mut self, code: Value) {
        let Some(c) = self.cleanup else {
            self.fail(CgError::Internal("cleanup がない".into()));
            return;
        };
        let p = self.pending.expect("pending");
        let a = self.saddr(p);
        self.st(code, a, 0);
        self.b.ins().jump(c, NOARGS);
    }

    /// `cond` が真なら符号 `code` で巻き戻しへ進む。
    fn fail_if(&mut self, cond: Value, code: i64) {
        let bad = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(cond, bad, NOARGS, ok, NOARGS);
        self.b.switch_to_block(bad);
        let c = self.ic(code);
        self.jump_cleanup(c);
        self.b.switch_to_block(ok);
    }

    /// 呼び出しの符号が 0 でなければ巻き戻しへ進む。
    fn after_call(&mut self, code: Value) {
        let bad = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(code, bad, NOARGS, ok, NOARGS);
        self.b.switch_to_block(bad);
        self.jump_cleanup(code);
        self.b.switch_to_block(ok);
    }

    fn guard_stack(&mut self) {
        let sp = self.b.ins().get_stack_pointer(types::I64);
        let lim = self.rt("stack_limit", &[]);
        let c = self.b.ins().icmp(IntCC::UnsignedLessThan, sp, lim);
        let code = self.stack_code;
        self.fail_if(c, code);
    }

    /// 条件で 2 つの経路に分け、値を stack slot で合流させる。
    fn branch(
        &mut self,
        cond: Value,
        t: impl FnOnce(&mut Self) -> Value,
        e: impl FnOnce(&mut Self) -> Value,
    ) -> Value {
        let slot = self.tmp(8);
        let (tb, eb, mb) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().brif(cond, tb, NOARGS, eb, NOARGS);
        self.b.switch_to_block(tb);
        let v = t(self);
        let a = self.saddr(slot);
        self.st(v, a, 0);
        self.b.ins().jump(mb, NOARGS);
        self.b.switch_to_block(eb);
        let v = e(self);
        let a = self.saddr(slot);
        self.st(v, a, 0);
        self.b.ins().jump(mb, NOARGS);
        self.b.switch_to_block(mb);
        let a = self.saddr(slot);
        self.ld(a, 0)
    }

    /// `0..n` を回す。`body` は現在のブロックで続きを書く（終わりに添字を進めて先頭へ戻る）。
    fn loop_range(&mut self, n: Value, body: impl FnOnce(&mut Self, Value)) {
        let is = self.tmp(8);
        let a = self.saddr(is);
        let z = self.ic(0);
        self.st(z, a, 0);
        let (head, bodyb, exit) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(head);
        let a = self.saddr(is);
        let i = self.ld(a, 0);
        let c = self.b.ins().icmp(IntCC::SignedLessThan, i, n);
        self.b.ins().brif(c, bodyb, NOARGS, exit, NOARGS);
        self.b.switch_to_block(bodyb);
        let a = self.saddr(is);
        let i = self.ld(a, 0);
        body(self, i);
        let a = self.saddr(is);
        let i = self.ld(a, 0);
        let i2 = self.b.ins().iadd_imm_s(i, 1);
        self.st(i2, a, 0);
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(exit);
    }

    fn call_direct(&mut self, g: usize, args: &[Value]) -> Value {
        self.guard_stack();
        let fr = self.cg.m.declare_func_in_func(self.cg.fids[g], self.b.func);
        let inst = self.b.ins().call(fr, args);
        let r = self.b.inst_results(inst).to_vec();
        self.after_call(r[1]);
        r[0]
    }

    /// 関数の値（`[関数の番地][環境の語]` の塊）を呼ぶ。
    fn call_fnref(&mut self, fv: Value, args: &[Value]) -> Value {
        self.guard_stack();
        let fp = self.ld(fv, 0);
        let env = self.ld(fv, 8);
        let res = self.tmp(16);
        let (plain, clos, merge) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        let isplain = self.b.ins().icmp_imm_s(IntCC::Equal, env, 0);
        self.b.ins().brif(isplain, plain, NOARGS, clos, NOARGS);
        self.b.switch_to_block(plain);
        let sig = self.sigref(args.len());
        let inst = self.b.ins().call_indirect(sig, fp, args);
        let r = self.b.inst_results(inst).to_vec();
        let a = self.saddr(res);
        self.st(r[0], a, 0);
        self.st(r[1], a, 8);
        self.b.ins().jump(merge, NOARGS);
        self.b.switch_to_block(clos);
        let envp = self.b.ins().iadd_imm_s(fv, 8);
        let mut a2 = vec![envp];
        a2.extend_from_slice(args);
        let sig = self.sigref(a2.len());
        let inst = self.b.ins().call_indirect(sig, fp, &a2);
        let r = self.b.inst_results(inst).to_vec();
        let a = self.saddr(res);
        self.st(r[0], a, 0);
        self.st(r[1], a, 8);
        self.b.ins().jump(merge, NOARGS);
        self.b.switch_to_block(merge);
        let a = self.saddr(res);
        let v = self.ld(a, 0);
        let code = self.ld(a, 8);
        self.after_call(code);
        v
    }

    // --- 場所と被演算子 ---

    fn autoload(&mut self, addr: &mut Value, cur: &mut Cur) {
        if let Cur::T(Ty::Ref(inner)) = cur {
            *addr = self.ld(*addr, 0);
            let t = (**inner).clone();
            *cur = Cur::T(t);
        }
    }

    fn place(&mut self, pl: &Place) -> (Value, Cur) {
        let mut addr = self.saddr(self.slots[pl.base]);
        let mut cur = Cur::T(self.lt[pl.base].clone());
        for pj in &pl.proj {
            match *pj {
                Proj::Field(i) => {
                    self.autoload(&mut addr, &mut cur);
                    let bp = self.ld(addr, 0);
                    let p = self.cg.p;
                    let (off, ty) = match &cur {
                        Cur::T(Ty::Tuple(ts)) if i < ts.len() => (8 * i as i32, norm(&ts[i])),
                        Cur::T(Ty::Adt(t)) => match &p.types[*t].kind {
                            TypeKind::Struct { fields } if i < fields.len() => {
                                (8 * i as i32, norm(&fields[i].1))
                            }
                            _ => {
                                self.unsupported("場所の形が想定と違う（構造体でない）");
                                (0, Ty::Unit)
                            }
                        },
                        Cur::V(t, v) => match &p.types[*t].kind {
                            TypeKind::Enum { variants } if i < variants[*v].fields.len() => {
                                (8 + 8 * i as i32, norm(&variants[*v].fields[i]))
                            }
                            _ => {
                                self.unsupported("場所の形が想定と違う（腕のフィールド）");
                                (8, Ty::Unit)
                            }
                        },
                        _ => {
                            self.unsupported("この場所の射影はまだ対応していない");
                            (0, Ty::Unit)
                        }
                    };
                    addr = self.b.ins().iadd_imm_s(bp, off as i64);
                    cur = Cur::T(ty);
                }
                Proj::Downcast(v) => {
                    self.autoload(&mut addr, &mut cur);
                    match &cur {
                        Cur::T(Ty::Adt(t)) => cur = Cur::V(*t, v),
                        _ => {
                            self.unsupported("腕への射影が列挙型でない");
                        }
                    }
                }
                Proj::Deref => {
                    if let Cur::T(Ty::Ref(inner)) = &cur {
                        let t = (**inner).clone();
                        addr = self.ld(addr, 0);
                        cur = Cur::T(t);
                    }
                }
            }
        }
        (addr, cur)
    }

    fn cur_ty(c: &Cur) -> Ty {
        match c {
            Cur::T(t) => t.clone(),
            Cur::V(..) => Ty::Unit,
        }
    }

    /// 被演算子を語として読む（型・複製かどうかつき）。
    fn raw(&mut self, op: &Operand) -> (Value, Ty, bool) {
        let (v, t, c, _) = self.raw_a(op);
        (v, t, c)
    }

    /// `raw` に加えて、移動の元の番地（所有する型の場合だけ）を返す。
    fn raw_a(&mut self, op: &Operand) -> (Value, Ty, bool, Option<Value>) {
        if let Operand::Copy(p) | Operand::Move(p) = op {
            let (a, c) = self.place(p);
            let v = self.ld(a, 0);
            let t = Self::cur_ty(&c);
            let mv = matches!(op, Operand::Move(_)) && aggregate(&t);
            return (v, t, matches!(op, Operand::Copy(_)), mv.then_some(a));
        }
        let (v, t, c) = self.raw_const(op);
        (v, t, c, None)
    }

    fn raw_const(&mut self, op: &Operand) -> (Value, Ty, bool) {
        match op {
            Operand::Copy(_) | Operand::Move(_) => unreachable!(),
            Operand::Const(c) => match c {
                Const::Unit => (self.ic(0), Ty::Unit, false),
                Const::Int(n) => (self.ic(*n), Ty::Int, false),
                Const::Float(f) => (self.ic(f.to_bits() as i64), Ty::Float, false),
                Const::Bool(b) => (self.ic(*b as i64), Ty::Bool, false),
                Const::Str(s) => {
                    let text = self.cg.p.strings[*s].clone();
                    let d = match self.cg.bytes_data(text.as_bytes()) {
                        Ok(d) => d,
                        Err(e) => return (self.fail(e), Ty::Str, false),
                    };
                    let a = self.data_addr(d);
                    let n = self.ic(text.len() as i64);
                    (self.rt("str_new", &[a, n]), Ty::Str, false)
                }
                Const::Fn(f) => {
                    let func = &self.cg.p.funcs[*f];
                    let ps: Vec<Ty> = func
                        .params
                        .iter()
                        .map(|&l| norm(&func.locals[l].ty))
                        .collect();
                    let r = norm(&func.ret);
                    let d = match self.cg.fnblock(*f) {
                        Ok(d) => d,
                        Err(e) => return (self.fail(e), Ty::Unit, false),
                    };
                    (self.data_addr(d), Ty::FnRef(ps, Box::new(r)), false)
                }
            },
        }
    }

    /// 型の食い違いを直し、複製なら影（黙った深い複写）を作る。
    fn finish(&mut self, w: Value, ty: &Ty, is_copy: bool, dest: Option<&Ty>) -> Value {
        if let (Ty::Ref(inner), Some(d)) = (ty, dest) {
            if !matches!(d, Ty::Ref(_)) {
                let v = self.ld(w, 0);
                return if aggregate(inner) {
                    self.shadow(inner, v)
                } else {
                    v
                };
            }
        }
        if is_copy && aggregate(ty) {
            return self.shadow(ty, w);
        }
        w
    }

    fn argf(&mut self, op: &Operand, hint: Option<&Ty>) -> (Value, Ty) {
        let (w, ty, c, moved) = self.raw_a(op);
        let v = self.finish(w, &ty, c, hint);
        if let Some(a) = moved {
            // 移動した元は空にする（インタープリタの `Empty` と同じ）。
            let z = self.ic(0);
            self.st(z, a, 0);
        }
        let t = match (&ty, hint) {
            (Ty::Ref(i), Some(h)) if !matches!(h, Ty::Ref(_)) => (**i).clone(),
            _ => ty,
        };
        (v, t)
    }

    /// 所有する値として読む（参照ならたどって影を作る）。
    fn own(&mut self, op: &Operand) -> (Value, Ty) {
        self.argf(op, Some(&Ty::Unit))
    }

    /// 値として見る（参照を 1 段たどる。影は作らない）。
    fn val(&mut self, op: &Operand) -> (Value, Ty) {
        let (w, ty, _) = self.raw(op);
        if let Ty::Ref(i) = ty {
            (self.ld(w, 0), *i)
        } else {
            (w, ty)
        }
    }

    // --- 関数 ---

    pub fn run(&mut self, f: &Func) {
        let p = self.cg.p;
        self.line = f
            .blocks
            .iter()
            .find_map(|b| match &b.term {
                Term::Call { stop, .. }
                | Term::CheckedOp { stop, .. }
                | Term::Assert { stop, .. } => Some(p.stop_sites[*stop].line),
                _ => None,
            })
            .unwrap_or(p.exit_line);
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let params = self.b.block_params(entry).to_vec();
        for _ in &f.locals {
            let s = self.tmp(8);
            self.slots.push(s);
        }
        let z = self.ic(0);
        for i in 0..self.slots.len() {
            let a = self.saddr(self.slots[i]);
            self.st(z, a, 0);
        }
        let ps = self.tmp(8);
        let a = self.saddr(ps);
        self.st(z, a, 0);
        self.pending = Some(ps);
        for (i, &pl) in f.params.iter().enumerate() {
            let a = self.saddr(self.slots[pl]);
            self.st(params[i], a, 0);
        }
        for _ in &f.blocks {
            let b = self.b.create_block();
            self.blocks.push(b);
        }
        self.b.ins().jump(self.blocks[0], NOARGS);
        for (i, bb) in f.blocks.iter().enumerate() {
            self.b.switch_to_block(self.blocks[i]);
            for s in &bb.stmts {
                self.stmt(s);
            }
            self.term(&bb.term);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Drop(pl) => {
                let (a, c) = self.place(pl);
                let t = match c {
                    Cur::T(t) => t,
                    Cur::V(t, _) => Ty::Adt(t),
                };
                if aggregate(&t) {
                    let w = self.ld(a, 0);
                    let z = self.ic(0);
                    self.st(z, a, 0);
                    self.drop_val(&t, w);
                }
            }
            Stmt::Assign(dest, rv) => {
                let (da, dc) = self.place(dest);
                let dty = Self::cur_ty(&dc);
                let v = self.rvalue(rv, &dty);
                self.store_drop(da, v, &dty);
            }
        }
    }

    /// 場所へ書き、古い値を破棄する（新しい値を作った後に古い値を読む）。
    fn store_drop(&mut self, addr: Value, v: Value, ty: &Ty) {
        if aggregate(ty) {
            let old = self.ld(addr, 0);
            self.st(v, addr, 0);
            self.drop_val(ty, old);
        } else {
            self.st(v, addr, 0);
        }
    }

    fn rvalue(&mut self, rv: &Rvalue, dty: &Ty) -> Value {
        match rv {
            Rvalue::Use(op) => self.argf(op, Some(dty)).0,
            Rvalue::Unary(UnOp::Not, op) => {
                let (v, _) = self.val(op);
                self.b.ins().bxor_imm_s(v, 1)
            }
            Rvalue::Unary(UnOp::NegFloat, op) => {
                let (v, _) = self.val(op);
                let f = self.b.ins().bitcast(types::F64, MemFlagsData::new(), v);
                let n = self.b.ins().fneg(f);
                self.b.ins().bitcast(types::I64, MemFlagsData::new(), n)
            }
            Rvalue::Binary(op, a, b) => {
                let (x, tx) = self.val(a);
                let (y, _) = self.val(b);
                self.binary(*op, &tx, x, y)
            }
            Rvalue::Aggregate(kind, ops) => {
                let p = self.cg.p;
                let hints: Vec<Ty> = match kind {
                    AggKind::Tuple => match dty {
                        Ty::Tuple(ts) => ts.clone(),
                        _ => vec![],
                    },
                    AggKind::Struct(t) => match &p.types[*t].kind {
                        TypeKind::Struct { fields } => fields.iter().map(|f| norm(&f.1)).collect(),
                        _ => vec![],
                    },
                    AggKind::Variant(t, v) => match &p.types[*t].kind {
                        TypeKind::Enum { variants } => {
                            variants[*v].fields.iter().map(norm).collect()
                        }
                        _ => vec![],
                    },
                };
                let mut vals = vec![];
                for (i, o) in ops.iter().enumerate() {
                    vals.push(self.argf(o, hints.get(i)).0);
                }
                match kind {
                    AggKind::Variant(_, v) => self.mk_enum_c(*v as i64, &vals),
                    _ => {
                        let size = std::cmp::max(8, 8 * vals.len() as i64);
                        let blk = self.blk(size);
                        for (i, x) in vals.iter().enumerate() {
                            self.st(*x, blk, 8 * i as i32);
                        }
                        blk
                    }
                }
            }
            Rvalue::Ref(pl) | Rvalue::RefMut(pl) => {
                let (a, c) = self.place(pl);
                if matches!(c, Cur::T(Ty::Ref(_))) {
                    self.ld(a, 0)
                } else {
                    a
                }
            }
            Rvalue::Clone(op, ty) => {
                let (v, _) = self.val(op);
                let t = strip(&norm(ty));
                match t {
                    Ty::Int | Ty::Float | Ty::Bool | Ty::Unit | Ty::ParseError => v,
                    Ty::Str | Ty::Path => self.rt("str_clone", &[v]),
                    _ => {
                        let d = self.desc_addr(&t);
                        self.rt("val_clone", &[d, v])
                    }
                }
            }
            Rvalue::Discriminant(op) => {
                let (v, _) = self.val(op);
                self.ld(v, 0)
            }
            Rvalue::MakeClosure(f, ops) => {
                let p = self.cg.p;
                let func = &p.funcs[*f];
                let hints: Vec<Ty> = match func.params.first().map(|&l| norm(&func.locals[l].ty)) {
                    Some(Ty::Ref(i)) => match *i {
                        Ty::Tuple(ts) => ts,
                        _ => vec![],
                    },
                    _ => vec![],
                };
                let mut vals = vec![];
                for (i, o) in ops.iter().enumerate() {
                    vals.push(self.argf(o, hints.get(i)).0);
                }
                let size = std::cmp::max(8, 8 * vals.len() as i64);
                let env = self.blk(size);
                for (i, x) in vals.iter().enumerate() {
                    self.st(*x, env, 8 * i as i32);
                }
                // 環境の型（クロージャを破棄するときに中身を破棄するため）。
                let envty = if hints.len() == vals.len() {
                    Ty::Tuple(hints.clone())
                } else {
                    Ty::Tuple(vec![Ty::Unit; vals.len()])
                };
                let envd = self.desc_addr(&envty);
                let fr = self
                    .cg
                    .m
                    .declare_func_in_func(self.cg.fids[*f], self.b.func);
                let fa = self.b.ins().func_addr(types::I64, fr);
                let blk = self.blk(24);
                self.st(fa, blk, 0);
                self.st(env, blk, 8);
                self.st(envd, blk, 16);
                blk
            }
        }
    }

    /// 構造的な比較（記述子を通す）。結果は 0/1 の語。
    fn compare(&mut self, op: BinOp, t: &Ty, x: Value, y: Value) -> Value {
        let d = self.desc_addr(&strip(t));
        match op {
            BinOp::Eq | BinOp::Ne => {
                let e = self.rt("val_eq", &[d, x, y]);
                if op == BinOp::Ne {
                    self.b.ins().bxor_imm_s(e, 1)
                } else {
                    e
                }
            }
            _ => {
                let c = self.rt("val_cmp", &[d, x, y]);
                let r = match op {
                    BinOp::Lt => self.b.ins().icmp_imm_s(IntCC::Equal, c, -1),
                    BinOp::Gt => self.b.ins().icmp_imm_s(IntCC::Equal, c, 1),
                    BinOp::Le => {
                        let c1 = self.b.ins().iadd_imm_s(c, 1);
                        self.b
                            .ins()
                            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, c1, 1)
                    }
                    _ => self
                        .b
                        .ins()
                        .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, c, 1),
                };
                self.b2i(r)
            }
        }
    }

    fn binary(&mut self, op: BinOp, t: &Ty, x: Value, y: Value) -> Value {
        match t {
            Ty::Int | Ty::Bool | Ty::Unit => {
                let cc = match op {
                    BinOp::Eq => IntCC::Equal,
                    BinOp::Ne => IntCC::NotEqual,
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
                    _ => return self.unsupported("Int の算術が CheckedOp でない"),
                };
                let c = self.b.ins().icmp(cc, x, y);
                self.b2i(c)
            }
            Ty::Float => {
                let fx = self.b.ins().bitcast(types::F64, MemFlagsData::new(), x);
                let fy = self.b.ins().bitcast(types::F64, MemFlagsData::new(), y);
                let cc = match op {
                    BinOp::Eq => Some(FloatCC::Equal),
                    BinOp::Ne => Some(FloatCC::NotEqual),
                    BinOp::Lt => Some(FloatCC::LessThan),
                    BinOp::Le => Some(FloatCC::LessThanOrEqual),
                    BinOp::Gt => Some(FloatCC::GreaterThan),
                    BinOp::Ge => Some(FloatCC::GreaterThanOrEqual),
                    _ => None,
                };
                if let Some(cc) = cc {
                    let c = self.b.ins().fcmp(cc, fx, fy);
                    return self.b2i(c);
                }
                let r = match op {
                    BinOp::Add => self.b.ins().fadd(fx, fy),
                    BinOp::Sub => self.b.ins().fsub(fx, fy),
                    BinOp::Mul => self.b.ins().fmul(fx, fy),
                    BinOp::Div => self.b.ins().fdiv(fx, fy),
                    _ => return self.rt("fmod", &[x, y]),
                };
                self.b.ins().bitcast(types::I64, MemFlagsData::new(), r)
            }
            Ty::Str if op == BinOp::Add => self.rt("str_concat", &[x, y]),
            _ => self.compare(op, t, x, y),
        }
    }

    // --- 終端 ---

    fn term(&mut self, t: &Term) {
        let p = self.cg.p;
        match t {
            Term::Goto(b) => {
                self.b.ins().jump(self.blocks[*b], NOARGS);
            }
            Term::Branch(c, t, e) => {
                let (v, _) = self.val(c);
                self.b
                    .ins()
                    .brif(v, self.blocks[*t], NOARGS, self.blocks[*e], NOARGS);
            }
            Term::Switch(c, arms, d) => {
                let (v, _) = self.val(c);
                for (k, b) in arms {
                    let eq = self.b.ins().icmp_imm_s(IntCC::Equal, v, *k);
                    let next = self.b.create_block();
                    self.b.ins().brif(eq, self.blocks[*b], NOARGS, next, NOARGS);
                    self.b.switch_to_block(next);
                }
                self.b.ins().jump(self.blocks[*d], NOARGS);
            }
            Term::Return => {
                let a = self.saddr(self.slots[0]);
                let v = self.ld(a, 0);
                let z = self.ic(0);
                self.b.ins().return_(&[v, z]);
            }
            // 来ない基本ブロック。形を保つために戻りと同じ命令にする。
            Term::Unreachable => {
                let a = self.saddr(self.slots[0]);
                let v = self.ld(a, 0);
                let z = self.ic(0);
                self.b.ins().return_(&[v, z]);
            }
            Term::Resume => {
                let ps = self.pending.expect("pending");
                let a = self.saddr(ps);
                let c = self.ld(a, 0);
                let z = self.ic(0);
                self.b.ins().return_(&[z, c]);
            }
            Term::Assert {
                cond,
                stop,
                next,
                cleanup,
            } => {
                self.line = p.stop_sites[*stop].line;
                self.cleanup = Some(self.blocks[*cleanup]);
                let (v, _) = self.val(cond);
                let bad = self.b.create_block();
                self.b
                    .ins()
                    .brif(v, self.blocks[*next], NOARGS, bad, NOARGS);
                self.b.switch_to_block(bad);
                let code = self.cg.code_for(*stop, None, None);
                let c = self.ic(code);
                self.jump_cleanup(c);
            }
            Term::CheckedOp {
                op,
                a,
                b,
                dest,
                next,
                stop,
                cleanup,
            } => {
                self.line = p.stop_sites[*stop].line;
                self.cleanup = Some(self.blocks[*cleanup]);
                let code = self.cg.code_for(*stop, None, None);
                let (x, _) = self.val(a);
                let (y, _) = self.val(b);
                let r = match op {
                    ArithOp::Add | ArithOp::Sub | ArithOp::Mul => {
                        let (r, ovf) = match op {
                            ArithOp::Add => {
                                let r = self.b.ins().iadd(x, y);
                                let a1 = self.b.ins().bxor(x, r);
                                let a2 = self.b.ins().bxor(y, r);
                                let m = self.b.ins().band(a1, a2);
                                (r, self.b.ins().icmp_imm_s(IntCC::SignedLessThan, m, 0))
                            }
                            ArithOp::Sub => {
                                let r = self.b.ins().isub(x, y);
                                let a1 = self.b.ins().bxor(x, y);
                                let a2 = self.b.ins().bxor(x, r);
                                let m = self.b.ins().band(a1, a2);
                                (r, self.b.ins().icmp_imm_s(IntCC::SignedLessThan, m, 0))
                            }
                            _ => {
                                let r = self.b.ins().imul(x, y);
                                let hi = self.b.ins().smulhi(x, y);
                                let sh = self.b.ins().sshr_imm_s(r, 63);
                                (r, self.b.ins().icmp(IntCC::NotEqual, hi, sh))
                            }
                        };
                        self.fail_if(ovf, code);
                        r
                    }
                    ArithOp::Div | ArithOp::Rem => {
                        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, y, 0);
                        let xm = self.b.ins().icmp_imm_s(IntCC::Equal, x, i64::MIN);
                        let ym = self.b.ins().icmp_imm_s(IntCC::Equal, y, -1);
                        let both = self.b.ins().band(xm, ym);
                        let bad = self.b.ins().bor(zero, both);
                        self.fail_if(bad, code);
                        if *op == ArithOp::Div {
                            self.b.ins().sdiv(x, y)
                        } else {
                            self.b.ins().srem(x, y)
                        }
                    }
                };
                let (da, _) = self.place(dest);
                self.st(r, da, 0);
                self.b.ins().jump(self.blocks[*next], NOARGS);
            }
            Term::Call {
                func,
                args,
                dest,
                next,
                cleanup,
                stop,
            } => {
                self.line = p.stop_sites[*stop].line;
                self.cleanup = Some(self.blocks[*cleanup]);
                self.stack_code = self
                    .cg
                    .code_for(*stop, Some(StopKind::StackExhausted), Some(""));
                let (_, dc) = self.place(dest);
                let dty = Self::cur_ty(&dc);
                let (v, rty) = match func {
                    Callee::Func(g) => {
                        let callee = &p.funcs[*g];
                        let mut vs = vec![];
                        for (i, a) in args.iter().enumerate() {
                            let h = callee.params.get(i).map(|&l| norm(&callee.locals[l].ty));
                            vs.push(self.argf(a, h.as_ref()).0);
                        }
                        (self.call_direct(*g, &vs), norm(&callee.ret))
                    }
                    Callee::Indirect(op) => {
                        let (fv, fty) = self.val(op);
                        let (ps, r) = match strip(&fty) {
                            Ty::FnRef(ps, r) => (ps, *r),
                            _ => {
                                let v = self.unsupported("関数の値でないものを呼んでいる");
                                self.finish_call(v, &Ty::Unit, &dty, dest, *next);
                                return;
                            }
                        };
                        let mut vs = vec![];
                        for (i, a) in args.iter().enumerate() {
                            vs.push(self.argf(a, ps.get(i)).0);
                        }
                        (self.call_fnref(fv, &vs), r)
                    }
                    Callee::Builtin(b) => self.builtin(*b, args, *stop),
                };
                self.finish_call(v, &rty, &dty, dest, *next);
            }
        }
    }

    fn finish_call(&mut self, v: Value, rty: &Ty, dty: &Ty, dest: &Place, next: usize) {
        let w = self.finish(v, rty, false, Some(dty));
        let (da, _) = self.place(dest);
        self.store_drop(da, w, dty);
        self.b.ins().jump(self.blocks[next], NOARGS);
    }

    // --- 組み込み ---

    fn tag_is(&mut self, v: Value, tag: i64) -> Value {
        let t = self.ld(v, 0);
        let c = self.b.ins().icmp_imm_s(IntCC::Equal, t, tag);
        self.b2i(c)
    }

    fn builtin(&mut self, b: Builtin, args: &[Operand], stop: usize) -> (Value, Ty) {
        use Builtin::*;
        let u = Ty::Unit;
        match b {
            ListNew => {
                let e = self.ic(8);
                (self.rt("list_new", &[e]), u)
            }
            ListLen => {
                let (l, _) = self.val(&args[0]);
                (self.rt("list_len", &[l]), u)
            }
            ListIsEmpty => {
                let (l, _) = self.val(&args[0]);
                let n = self.rt("list_len", &[l]);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, n, 0);
                (self.b2i(c), u)
            }
            ListGet | ListGetMut => {
                let (l, _) = self.val(&args[0]);
                let (i, _) = self.val(&args[1]);
                let p = self.rt("list_get", &[l, i]);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, p, 0);
                let tag = self.b2i(c);
                (self.mk_enum(tag, &[p]), u)
            }
            ListPush => {
                let (l, lty) = self.val(&args[0]);
                let hint = match &lty {
                    Ty::List(e) => Some((**e).clone()),
                    _ => None,
                };
                let (x, _) = self.argf(&args[1], hint.as_ref());
                let a = self.zero_tmp(8);
                self.st(x, a, 0);
                self.rt("list_push", &[l, a]);
                (self.ic(0), u)
            }
            ListSumInt => {
                let (l, _) = self.val(&args[0]);
                let out = self.zero_tmp(8);
                let rc = self.rt("list_sum_int", &[l, out]);
                let code = self.cg.code_for(stop, Some(StopKind::Overflow), Some(""));
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, rc, 0);
                self.fail_if(c, code);
                (self.ld(out, 0), u)
            }
            ListSumFloat => {
                let (l, _) = self.val(&args[0]);
                (self.rt("list_sum_float", &[l]), u)
            }
            ListMin | ListMax => {
                let (l, lty) = self.val(&args[0]);
                let elem = match strip(&lty) {
                    Ty::List(e) => *e,
                    _ => Ty::Int,
                };
                let n = self.rt("list_len", &[l]);
                let name = if b == ListMin { "min" } else { "max" };
                let code = self.cg.code_for(
                    stop,
                    Some(StopKind::StdRequiresViolated),
                    Some(&format!("List.{name} は self.len() > 0 を要求する")),
                );
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, n, 0);
                self.fail_if(c, code);
                let d = self.desc_addr(&elem);
                let w = self.ic((b == ListMax) as i64);
                let idx = self.rt("list_best", &[l, d, w]);
                let p = self.rt("list_get", &[l, idx]);
                (p, Ty::Ref(Box::new(elem)))
            }
            ListMap => {
                let (l, lty) = self.val(&args[0]);
                let (f, ft) = self.own(&args[1]);
                let elem = match strip(&lty) {
                    Ty::List(e) => *e,
                    _ => Ty::Int,
                };
                let n = self.rt("list_len", &[l]);
                let e8 = self.ic(8);
                let outl = self.rt("list_new", &[e8]);
                let os = self.tmp(8);
                let oa = self.saddr(os);
                self.st(outl, oa, 0);
                self.loop_range(n, |s, i| {
                    let p = s.rt("list_get", &[l, i]);
                    let mut x = s.ld(p, 0);
                    if aggregate(&elem) {
                        x = s.shadow(&elem, x);
                    }
                    let r = s.call_fnref(f, &[x]);
                    let a = s.zero_tmp(8);
                    s.st(r, a, 0);
                    let oa = s.saddr(os);
                    let ol = s.ld(oa, 0);
                    s.rt("list_push", &[ol, a]);
                });
                let oa = self.saddr(os);
                let out = self.ld(oa, 0);
                self.drop_val(&ft, f);
                (out, u)
            }
            ListFind => {
                let (l, _) = self.val(&args[0]);
                let (f, ft) = self.own(&args[1]);
                let n = self.rt("list_len", &[l]);
                let res = self.tmp(8);
                let done = self.b.create_block();
                self.loop_range(n, |s, i| {
                    let p = s.rt("list_get", &[l, i]);
                    let r = s.call_fnref(f, &[p]);
                    let c = s.b.ins().icmp_imm_s(IntCC::NotEqual, r, 0);
                    let (found, cont) = (s.b.create_block(), s.b.create_block());
                    s.b.ins().brif(c, found, NOARGS, cont, NOARGS);
                    s.b.switch_to_block(found);
                    let e = s.mk_enum_c(0, &[p]);
                    let ra = s.saddr(res);
                    s.st(e, ra, 0);
                    s.b.ins().jump(done, NOARGS);
                    s.b.switch_to_block(cont);
                });
                let z = self.ic(0);
                let e = self.mk_enum_c(1, &[z]);
                let ra = self.saddr(res);
                self.st(e, ra, 0);
                self.b.ins().jump(done, NOARGS);
                self.b.switch_to_block(done);
                let ra = self.saddr(res);
                let out = self.ld(ra, 0);
                self.drop_val(&ft, f);
                (out, u)
            }
            ListMaxBy | ListMinBy => {
                let (l, _) = self.val(&args[0]);
                let (f, ft) = self.own(&args[1]);
                let kt = match strip(&ft) {
                    Ty::FnRef(_, r) => *r,
                    _ => Ty::Int,
                };
                let kd = self.desc_addr(&strip(&kt));
                let want = if b == ListMaxBy { 1 } else { -1 };
                let n = self.rt("list_len", &[l]);
                // 最良の鍵と、その要素の添字（-1 は未定）。
                let (ks, is) = (self.tmp(8), self.tmp(8));
                let (ka, ia) = (self.saddr(ks), self.saddr(is));
                let z = self.ic(0);
                self.st(z, ka, 0);
                let m1 = self.ic(-1);
                self.st(m1, ia, 0);
                self.loop_range(n, |s, i| {
                    let p = s.rt("list_get", &[l, i]);
                    let k = s.call_fnref(f, &[p]);
                    let (ka, ia) = (s.saddr(ks), s.saddr(is));
                    let (bk, bi) = (s.ld(ka, 0), s.ld(ia, 0));
                    let first = s.b.ins().icmp_imm_s(IntCC::Equal, bi, -1);
                    let better = s.branch(
                        first,
                        |s| s.ic(1),
                        |s| {
                            let c = s.rt("val_cmp", &[kd, k, bk]);
                            let e = s.b.ins().icmp_imm_s(IntCC::Equal, c, want);
                            s.b2i(e)
                        },
                    );
                    s.branch(
                        better,
                        |s| {
                            s.drop_val(&kt, bk);
                            let (ka, ia) = (s.saddr(ks), s.saddr(is));
                            s.st(k, ka, 0);
                            s.st(i, ia, 0);
                            s.ic(0)
                        },
                        |s| {
                            s.drop_val(&kt, k);
                            s.ic(0)
                        },
                    );
                });
                let (ka, ia) = (self.saddr(ks), self.saddr(is));
                let (bk, bi) = (self.ld(ka, 0), self.ld(ia, 0));
                self.drop_val(&kt, bk);
                self.drop_val(&ft, f);
                let none = self.b.ins().icmp_imm_s(IntCC::Equal, bi, -1);
                let r = self.branch(
                    none,
                    |s| {
                        let z = s.ic(0);
                        s.mk_enum_c(1, &[z])
                    },
                    |s| {
                        let p = s.rt("list_get", &[l, bi]);
                        s.mk_enum_c(0, &[p])
                    },
                );
                (r, u)
            }
            OptIsSome | ResIsOk => {
                let (v, _) = self.val(&args[0]);
                (self.tag_is(v, 0), u)
            }
            OptIsNone | ResIsErr => {
                let (v, _) = self.val(&args[0]);
                (self.tag_is(v, 1), u)
            }
            OptUnwrapOr | ResUnwrapOr => {
                let (v, vt) = self.own(&args[0]);
                let (d, dt) = self.own(&args[1]);
                let tag = self.ld(v, 0);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let pay = s.ld(v, 8);
                        let z = s.ic(0);
                        s.st(z, v, 8);
                        s.drop_val(&vt, v);
                        s.drop_val(&dt, d);
                        pay
                    },
                    |s| {
                        s.drop_val(&vt, v);
                        d
                    },
                );
                (r, dt)
            }
            OptMap => {
                let (v, vt) = self.own(&args[0]);
                let (f, ft) = self.own(&args[1]);
                let tag = self.ld(v, 0);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let x = s.ld(v, 8);
                        let z = s.ic(0);
                        s.st(z, v, 8);
                        s.drop_val(&vt, v);
                        let r = s.call_fnref(f, &[x]);
                        s.mk_enum_c(0, &[r])
                    },
                    |s| {
                        s.drop_val(&vt, v);
                        let z = s.ic(0);
                        s.mk_enum_c(1, &[z])
                    },
                );
                self.drop_val(&ft, f);
                (r, u)
            }
            OptOkOr => {
                let (v, vt) = self.own(&args[0]);
                let (e, et) = self.own(&args[1]);
                let tag = self.ld(v, 0);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0);
                let pay = self.branch(
                    c,
                    |s| {
                        let x = s.ld(v, 8);
                        let z = s.ic(0);
                        s.st(z, v, 8);
                        s.drop_val(&vt, v);
                        s.drop_val(&et, e);
                        x
                    },
                    |s| {
                        s.drop_val(&vt, v);
                        e
                    },
                );
                (self.mk_enum(tag, &[pay]), u)
            }
            ResOk => {
                let (v, vt) = self.own(&args[0]);
                let tag = self.ld(v, 0);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let x = s.ld(v, 8);
                        let z = s.ic(0);
                        s.st(z, v, 8);
                        s.drop_val(&vt, v);
                        s.mk_enum_c(0, &[x])
                    },
                    |s| {
                        s.drop_val(&vt, v);
                        let z = s.ic(0);
                        s.mk_enum_c(1, &[z])
                    },
                );
                (r, u)
            }
            ResMap | ResMapErr => {
                let (v, vt) = self.own(&args[0]);
                let (f, ft) = self.own(&args[1]);
                let tag = self.ld(v, 0);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, tag, 0);
                let map_ok = b == ResMap;
                let r = self.branch(
                    c,
                    |s| {
                        let x = s.ld(v, 8);
                        let z = s.ic(0);
                        s.st(z, v, 8);
                        s.drop_val(&vt, v);
                        let y = if map_ok { s.call_fnref(f, &[x]) } else { x };
                        s.mk_enum_c(0, &[y])
                    },
                    |s| {
                        let x = s.ld(v, 8);
                        let z = s.ic(0);
                        s.st(z, v, 8);
                        s.drop_val(&vt, v);
                        let y = if map_ok { x } else { s.call_fnref(f, &[x]) };
                        s.mk_enum_c(1, &[y])
                    },
                );
                self.drop_val(&ft, f);
                (r, u)
            }
            StrLen => {
                let (s, _) = self.val(&args[0]);
                (self.rt("str_len", &[s]), u)
            }
            StrIsEmpty | PathIsEmpty => {
                let (s, _) = self.val(&args[0]);
                (self.rt("str_is_empty", &[s]), u)
            }
            StrTrim => {
                let (s, _) = self.val(&args[0]);
                (self.rt("str_trim", &[s]), u)
            }
            StrLines => {
                let (s, _) = self.val(&args[0]);
                (self.rt("str_lines", &[s]), u)
            }
            StrParseInt => {
                let (s, _) = self.val(&args[0]);
                let out = self.zero_tmp(8);
                let ok = self.rt("str_parse_int", &[s, out]);
                let isbad = self.b.ins().icmp_imm_s(IntCC::Equal, ok, 0);
                let tag = self.b2i(isbad);
                let good = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let n = self.ld(out, 0);
                let z = self.ic(0);
                let pay = self.b.ins().select(good, n, z);
                (self.mk_enum(tag, &[pay]), u)
            }
            PathNew => {
                let (s, _) = self.val(&args[0]);
                (self.rt("str_clone", &[s]), u)
            }
            PathJoin => {
                let (a, _) = self.val(&args[0]);
                let (c, _) = self.val(&args[1]);
                let (ap, al) = (self.rt("str_data", &[a]), self.rt("str_bytes", &[a]));
                let (cp, cl) = (self.rt("str_data", &[c]), self.rt("str_bytes", &[c]));
                let op = self.zero_tmp(8);
                let ol = self.zero_tmp(8);
                self.rt("path_join", &[ap, al, cp, cl, op, ol]);
                let (p, n) = (self.ld(op, 0), self.ld(ol, 0));
                let s = self.rt("str_new", &[p, n]);
                self.rt("os_buf_free", &[p, n]);
                (s, u)
            }
            PathFileName | PathParent => {
                let (a, _) = self.val(&args[0]);
                let (ap, al) = (self.rt("str_data", &[a]), self.rt("str_bytes", &[a]));
                let op = self.zero_tmp(8);
                let ol = self.zero_tmp(8);
                let name = if b == PathFileName {
                    "path_file_name"
                } else {
                    "path_parent"
                };
                let ok = self.rt(name, &[ap, al, op, ol]);
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let (p, n) = (s.ld(op, 0), s.ld(ol, 0));
                        let st = s.rt("str_new", &[p, n]);
                        s.rt("os_buf_free", &[p, n]);
                        s.mk_enum_c(0, &[st])
                    },
                    |s| {
                        let z = s.ic(0);
                        s.mk_enum_c(1, &[z])
                    },
                );
                (r, u)
            }
            CellNew | SharedNew => {
                let (x, _) = self.own(&args[0]);
                let blk = self.blk(8);
                self.st(x, blk, 0);
                (blk, u)
            }
            SharedGet => {
                let (v, ty) = self.val(&args[0]);
                let (w, inner) = match strip(&ty) {
                    Ty::Shared(i) => match *i {
                        Ty::Cell(t) => {
                            let c = self.ld(v, 0);
                            (self.ld(c, 0), *t)
                        }
                        t => (self.ld(v, 0), t),
                    },
                    Ty::Cell(t) => (self.ld(v, 0), *t),
                    _ => {
                        return (self.unsupported("get の対象が Shared/Cell でない"), u);
                    }
                };
                let w = if aggregate(&inner) {
                    self.shadow(&inner, w)
                } else {
                    w
                };
                (w, inner)
            }
            SharedSet => {
                let (v, ty) = self.val(&args[0]);
                let (n, _) = self.own(&args[1]);
                let (cell, inner) = match strip(&ty) {
                    Ty::Shared(i) => match *i {
                        Ty::Cell(t) => (self.ld(v, 0), *t),
                        _ => return (self.unsupported("set の対象が Shared<Cell> でない"), u),
                    },
                    Ty::Cell(t) => (v, *t),
                    _ => return (self.unsupported("set の対象が Shared<Cell> でない"), u),
                };
                let old = self.ld(cell, 0);
                self.st(n, cell, 0);
                self.drop_val(&inner, old);
                (self.ic(0), u)
            }
            IntMinValue => (self.ic(i64::MIN), u),
            ToFloat => {
                let (x, _) = self.val(&args[0]);
                let f = self.b.ins().fcvt_from_sint(types::F64, x);
                (self.b.ins().bitcast(types::I64, MemFlagsData::new(), f), u)
            }
            ToInt => {
                let (x, _) = self.val(&args[0]);
                let f = self.b.ins().bitcast(types::F64, MemFlagsData::new(), x);
                let lo = self.b.ins().f64const(-9_223_372_036_854_775_808.0_f64);
                let hi = self.b.ins().f64const(9_223_372_036_854_775_808.0_f64);
                let c1 = self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, f, lo);
                let c2 = self.b.ins().fcmp(FloatCC::LessThan, f, hi);
                let ok = self.b.ins().band(c1, c2);
                let okw = self.b2i(ok);
                let tag = self.b.ins().bxor_imm_s(okw, 1);
                let n = self.b.ins().fcvt_to_sint_sat(types::I64, f);
                (self.mk_enum(tag, &[n]), u)
            }
            ConsoleLog => {
                let (v, ty) = self.val(&args[0]);
                let d = self.desc_addr(&strip(&ty));
                let q = self.ic(0);
                let s = self.rt("val_show", &[d, v, q]);
                let (p, n) = (self.rt("str_data", &[s]), self.rt("str_bytes", &[s]));
                self.rt("console_log", &[p, n]);
                self.rt("str_free", &[s]);
                (self.ic(0), u)
            }
            FsRead => {
                let (path, _) = self.val(&args[0]);
                let (pp, pl) = (self.rt("str_data", &[path]), self.rt("str_bytes", &[path]));
                let op = self.zero_tmp(8);
                let ol = self.zero_tmp(8);
                let ok = self.zero_tmp(8);
                let rc = self.rt("fs_read", &[pp, pl, op, ol, ok]);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, rc, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let (p, n) = (s.ld(op, 0), s.ld(ol, 0));
                        let st = s.rt("str_new", &[p, n]);
                        s.rt("os_buf_free", &[p, n]);
                        s.mk_enum_c(0, &[st])
                    },
                    |s| {
                        let io = s.io_error(ok, path);
                        s.mk_enum_c(1, &[io])
                    },
                );
                (r, u)
            }
            FsWrite => {
                let (path, _) = self.val(&args[0]);
                let (text, _) = self.val(&args[1]);
                let (pp, pl) = (self.rt("str_data", &[path]), self.rt("str_bytes", &[path]));
                let (tp, tl) = (self.rt("str_data", &[text]), self.rt("str_bytes", &[text]));
                let ok = self.zero_tmp(8);
                let rc = self.rt("fs_write", &[pp, pl, tp, tl, ok]);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, rc, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let z = s.ic(0);
                        let unit = s.mk_enum_c(0, &[z]);
                        let _ = unit;
                        let z = s.ic(0);
                        s.mk_enum_c(0, &[z])
                    },
                    |s| {
                        let io = s.io_error(ok, path);
                        s.mk_enum_c(1, &[io])
                    },
                );
                (r, u)
            }
            Compare(op) => {
                let (x, tx) = self.val(&args[0]);
                let (y, _) = self.val(&args[1]);
                (self.compare(op, &tx, x, y), u)
            }
        }
    }

    /// `IoError(kind, path)` の塊（`kind_slot` に 32 ビットの種別が入っている）。
    fn io_error(&mut self, kind_slot: Value, path: Value) -> Value {
        let k = self
            .b
            .ins()
            .load(types::I32, MemFlagsData::new(), kind_slot, 0);
        let kw = self.b.ins().uextend(types::I64, k);
        // 借りた経路は呼び出し側が解放するので、エラーの値は自分の複製を持つ。
        let own = self.rt("str_clone", &[path]);
        self.mk_enum(kw, &[own])
    }

    // --- 入口 ---

    pub fn c_main(&mut self, thunk: cranelift_module::FuncId) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let argc = self.b.ins().sextend(types::I64, ps[0]);
        let a = self.zero_tmp(16);
        self.st(argc, a, 0);
        self.st(ps[1], a, 8);
        let fr = self.cg.m.declare_func_in_func(thunk, self.b.func);
        let fa = self.b.ins().func_addr(types::I64, fr);
        let r = self.rt("enter", &[fa, a]);
        let r32 = self.b.ins().ireduce(types::I32, r);
        self.b.ins().return_(&[r32]);
    }

    pub fn thunk(&mut self) {
        let p = self.cg.p;
        let f = &p.funcs[p.entry];
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let arg = self.b.block_params(entry)[0];
        let argc = self.ld(arg, 0);
        let argv = self.ld(arg, 8);
        let n = f.params.len();
        let out = self.zero_tmp(8 * std::cmp::max(n, 1) as u32);

        // 引数の記述子と名前の表。
        let mut dws: Vec<W> = vec![];
        let mut nws: Vec<W> = vec![];
        for &pl in &f.params {
            let t = strip(&norm(&f.locals[pl].ty));
            match self.cg.desc(&t) {
                Ok(d) => dws.push(W::D(d)),
                Err(e) => {
                    self.fail(e);
                }
            }
            match self.cg.name_words(&f.locals[pl].name) {
                Ok(w) => nws.extend(w),
                Err(e) => {
                    self.fail(e);
                }
            }
        }
        if dws.is_empty() {
            dws.push(W::I(0));
            nws.push(W::I(0));
            nws.push(W::I(0));
        }
        let table = |this: &mut Self, ws: &[W]| -> Value {
            match this.cg.m.declare_anonymous_data(false, false) {
                Ok(id) => match this.cg.write_words(id, ws) {
                    Ok(()) => this.data_addr(id),
                    Err(e) => this.fail(e),
                },
                Err(e) => this.fail(CgError::Internal(e.to_string())),
            }
        };
        let dtab = table(self, &dws);
        let ntab = table(self, &nws);
        if self.cg.trace {
            self.rt("trace_on", &[]);
        }
        let (fp, fl) = match self.cg.bytes_data(f.name.as_bytes()) {
            Ok(d) => (self.data_addr(d), self.ic(f.name.len() as i64)),
            Err(e) => (self.fail(e), self.ic(0)),
        };
        let np = self.ic(n as i64);
        let rc = self.rt("entry_args", &[argc, argv, fp, fl, np, dtab, ntab, out]);
        let (bad, ok) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(rc, bad, NOARGS, ok, NOARGS);
        self.b.switch_to_block(bad);
        let one = self.b.ins().iconst(types::I32, 1);
        self.b.ins().return_(&[one]);
        self.b.switch_to_block(ok);

        let mut vs = vec![];
        for (i, &pl) in f.params.iter().enumerate() {
            if matches!(norm(&f.locals[pl].ty), Ty::Ref(_)) {
                vs.push(self.b.ins().iadd_imm_s(out, 8 * i as i64));
            } else {
                vs.push(self.ld(out, 8 * i as i32));
            }
        }
        let fr = self
            .cg
            .m
            .declare_func_in_func(self.cg.fids[p.entry], self.b.func);
        let inst = self.b.ins().call(fr, &vs);
        let r = self.b.inst_results(inst).to_vec();
        let (stopb, showb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(r[1], stopb, NOARGS, showb, NOARGS);

        self.b.switch_to_block(stopb);
        let base = self.data_addr(self.cg.msg_table);
        let idx = self.b.ins().iadd_imm_s(r[1], -1);
        let off = self.b.ins().imul_imm_s(idx, 16);
        let ent = self.b.ins().iadd(base, off);
        let (mp, ml) = (self.ld(ent, 0), self.ld(ent, 8));
        self.rt("stop", &[mp, ml]);
        let c70 = self.b.ins().iconst(types::I32, 70);
        self.b.ins().return_(&[c70]);

        self.b.switch_to_block(showb);
        let rt = strip(&norm(&f.ret));
        let d = self.desc_addr(&rt);
        let q = self.ic(1);
        let s = self.rt("val_show", &[d, r[0], q]);
        self.rt("print_result", &[s]);
        self.rt("str_free", &[s]);
        self.drop_val(&rt, r[0]);
        let z = self.b.ins().iconst(types::I32, 0);
        self.b.ins().return_(&[z]);
    }
}

//! 関数 1 つ分の出力（SIR の基本ブロック → Cranelift）。値の持ち方は親モジュールの説明を見よ。

mod call;
mod drop;
mod expr;
mod stmt;

use super::{norm, rt_spec, Cg, CgError, W};
use crate::ir::{Func, Term, Ty};
use cranelift_codegen::ir::condcodes::IntCC;
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
            | Ty::Bytes
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

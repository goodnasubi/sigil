//! 関数 1 つ分の出力（SIR の基本ブロック → Cranelift）。値の持ち方は親モジュールの説明を見よ。

mod call;
mod drop;
mod eqcmp;
mod expr;
mod stmt;

use super::{norm, rt_spec, shim_spec, Cg, CgError, W};
use crate::ir::{Func, GlobalInit, Term, Ty};
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
        let Some((ps, r)) = rt_spec(name).or_else(|| shim_spec(name)) else {
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
        // 確保器を結んだときは、生存数のシムをランタイムに登録する（`SIGIL_RT_LIVE` の `sigil_blocks=`。補足 B-5）。
        if let Some(l) = self.cg.live_shim {
            let lr = self.cg.m.declare_func_in_func(l, self.b.func);
            let la = self.b.ins().func_addr(types::I64, lr);
            self.rt("set_sigil_live", &[la]);
        }
        // 文字列の関数表を Rust ランタイムに登録する（補足 D-3。入口を呼ぶ前に文字列を作らせない）。
        if let Some(ids) = self.cg.str_ops {
            let addrs: Vec<Value> = ids
                .iter()
                .map(|&id| {
                    let r = self.cg.m.declare_func_in_func(id, self.b.func);
                    self.b.ins().func_addr(types::I64, r)
                })
                .collect();
            self.rt("set_str_ops", &addrs);
        }
        // 一覧の関数表も同じ（入口の引数の `List` を `coerce` が作るので、`enter` より前。補足 E-3）。
        if let Some(ids) = self.cg.list_ops {
            let addrs: Vec<Value> = ids
                .iter()
                .map(|&id| {
                    let r = self.cg.m.declare_func_in_func(id, self.b.func);
                    self.b.ins().func_addr(types::I64, r)
                })
                .collect();
            self.rt("set_list_ops", &addrs);
        }
        // バイト列の関数表は一覧の次（`set_str_ops` → `set_list_ops` → `set_bytes_ops` → `enter`。補足 F-4）。
        if let Some(ids) = self.cg.bytes_ops {
            let addrs: Vec<Value> = ids
                .iter()
                .map(|&id| {
                    let r = self.cg.m.declare_func_in_func(id, self.b.func);
                    self.b.ins().func_addr(types::I64, r)
                })
                .collect();
            self.rt("set_bytes_ops", &addrs);
        }
        let r = self.rt("enter", &[fa, a]);
        let r32 = self.b.ins().ireduce(types::I32, r);
        self.b.ins().return_(&[r32]);
    }

    /// 組・構造体の大域の塊を作って、静的データの語に入れる（スカラーは静的データがすでに初期値を持つ）。
    fn init_globals(&mut self) {
        let p = self.cg.p;
        for (g, gl) in p.globals.iter().enumerate() {
            if matches!(gl.init, GlobalInit::Tuple(_) | GlobalInit::Struct(_)) {
                let v = self.global_block(&gl.init);
                let d = self.cg.gids[g];
                let a = self.data_addr(d);
                self.st(v, a, 0);
            }
        }
    }

    fn global_block(&mut self, init: &GlobalInit) -> Value {
        match init {
            GlobalInit::Int(n) => self.ic(*n),
            GlobalInit::Bool(b) => self.ic(*b as i64),
            GlobalInit::AddrNull => self.ic(0),
            GlobalInit::Tuple(xs) | GlobalInit::Struct(xs) => {
                let vals: Vec<Value> = xs.iter().map(|x| self.global_block(x)).collect();
                let blk = self.blk(std::cmp::max(8, 8 * vals.len() as i64));
                for (i, v) in vals.iter().enumerate() {
                    self.st(*v, blk, 8 * i as i32);
                }
                blk
            }
        }
    }

    /// 終了時に組・構造体の大域の塊を解放する。
    ///
    /// 既知の差: 正常終了（`showb`）でだけ呼ぶ。異常停止（終了コード 70）ではこの解放をしない
    /// （評価器は異常停止でも大域を破棄する）。プロセスはすぐ終わるので害はないが、
    /// 停止時の `SIGIL_RT_LIVE` や `--trace-drops` の記録は評価器と一致しない。
    fn drop_globals(&mut self) {
        let p = self.cg.p;
        for (g, gl) in p.globals.iter().enumerate() {
            if matches!(gl.init, GlobalInit::Tuple(_) | GlobalInit::Struct(_)) {
                let d = self.cg.gids[g];
                let a = self.data_addr(d);
                let w = self.ld(a, 0);
                let t = norm(&gl.ty);
                self.drop_val(&t, w);
            }
        }
    }

    /// 停止の状態（1 始まりの番号）に対応する停止の報告を出す。
    fn report_stop(&mut self, st: Value) {
        let base = self.data_addr(self.cg.msg_table);
        let idx = self.b.ins().iadd_imm_s(st, -1);
        let off = self.b.ins().imul_imm_s(idx, 16);
        let ent = self.b.ins().iadd(base, off);
        let (mp, ml) = (self.ld(ent, 0), self.ld(ent, 8));
        self.rt("stop", &[mp, ml]);
    }

    /// 確保器のシムの本体（ADR 0025 補足 B-2）: Sigil の関数 `target` を呼び、値を返す。
    /// 確保器の中の停止（算術のあふれなど）は、報告を出して `exit_group(70)` で止める（補足 A-4）。
    pub fn alloc_shim(&mut self, target: usize, ret: char) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let fr = self
            .cg
            .m
            .declare_func_in_func(self.cg.fids[target], self.b.func);
        let inst = self.b.ins().call(fr, &ps);
        let r = self.b.inst_results(inst).to_vec();
        let (stopb, okb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(r[1], stopb, NOARGS, okb, NOARGS);

        self.b.switch_to_block(stopb);
        self.report_stop(r[1]);
        let n = self.ic(231);
        let code = self.ic(70);
        let z = self.ic(0);
        self.rt("raw_syscall", &[n, code, z, z, z, z, z]);
        match ret {
            'p' => {
                self.b.ins().return_(&[z]);
            }
            'i' => {
                let z32 = self.b.ins().ireduce(types::I32, z);
                self.b.ins().return_(&[z32]);
            }
            _ => {
                self.b.ins().return_(&[]);
            }
        }

        self.b.switch_to_block(okb);
        match ret {
            'p' => {
                self.b.ins().return_(&[r[0]]);
            }
            'i' => {
                // Sigil の戻り（0/1 の Bool か Int）の下位 32 ビットを返す（補足 D-3）。
                let v = self.b.ins().ireduce(types::I32, r[0]);
                self.b.ins().return_(&[v]);
            }
            _ => {
                self.b.ins().return_(&[]);
            }
        }
    }

    /// `memmove(dst, src, n)` の本体（ADR 0025 補足 C-3）: バイト単位のループ。`dst` が `src` より小さい
    /// （符号なし）なら前から、でなければ後ろから写すので、重なりがあっても正しい。`n` は符号なしとして読む。
    pub fn mem_move(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let (dst, src, n) = (ps[0], ps[1], ps[2]);
        let (fh, fb, bh, bb, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        for blk in [fh, fb, bh, bb] {
            self.b.append_block_param(blk, types::I64);
        }
        let zero = self.ic(0);
        let fwd = self.b.ins().icmp(IntCC::UnsignedLessThan, dst, src);
        self.b
            .ins()
            .brif(fwd, fh, &[BlockArg::Value(zero)], bh, &[BlockArg::Value(n)]);

        // 前から: i = 0; while i < n { dst[i] = src[i]; i += 1 }
        self.b.switch_to_block(fh);
        let i = self.b.block_params(fh)[0];
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, n);
        self.b
            .ins()
            .brif(more, fb, &[BlockArg::Value(i)], done, NOARGS);
        self.b.switch_to_block(fb);
        let i = self.b.block_params(fb)[0];
        self.copy_byte(dst, src, i);
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(fh, &[BlockArg::Value(next)]);

        // 後ろから: i = n; while i != 0 { i -= 1; dst[i] = src[i] }
        self.b.switch_to_block(bh);
        let i = self.b.block_params(bh)[0];
        let more = self.b.ins().icmp_imm_s(IntCC::NotEqual, i, 0);
        self.b
            .ins()
            .brif(more, bb, &[BlockArg::Value(i)], done, NOARGS);
        self.b.switch_to_block(bb);
        let i = self.b.block_params(bb)[0];
        let j = self.b.ins().iadd_imm_s(i, -1);
        self.copy_byte(dst, src, j);
        self.b.ins().jump(bh, &[BlockArg::Value(j)]);

        self.b.switch_to_block(done);
        self.b.ins().return_(&[]);
    }

    fn copy_byte(&mut self, dst: Value, src: Value, i: Value) {
        let s = self.b.ins().iadd(src, i);
        let d = self.b.ins().iadd(dst, i);
        let v = self
            .b
            .ins()
            .uload8(types::I64, MemFlagsData::trusted(), s, 0);
        self.b.ins().istore8(MemFlagsData::trusted(), v, d, 0);
    }

    /// `memset(dst, byte, n)` の本体（ADR 0025 補足 C-3）: `byte` の下位 8 ビットで `n` バイトを埋める。
    pub fn mem_set(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let (dst, byte, n) = (ps[0], ps[1], ps[2]);
        let (head, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        let zero = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(zero)]);

        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, i, n);
        self.b
            .ins()
            .brif(more, body, &[BlockArg::Value(i)], done, NOARGS);
        self.b.switch_to_block(body);
        let i = self.b.block_params(body)[0];
        let d = self.b.ins().iadd(dst, i);
        self.b.ins().istore8(MemFlagsData::trusted(), byte, d, 0);
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(head, &[BlockArg::Value(next)]);

        self.b.switch_to_block(done);
        self.b.ins().return_(&[]);
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

        self.init_globals();
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
        self.report_stop(r[1]);
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
        self.drop_globals();
        let z = self.b.ins().iconst(types::I32, 0);
        self.b.ins().return_(&[z]);
    }
}

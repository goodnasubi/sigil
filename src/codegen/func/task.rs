//! 中断しうる関数の再開可能な入口と、並行処理の操作の生成（ADR 0030 決定 1、PR 5-2d）。計画は `codegen/task.rs`。
use super::{aggregate, norm, strip, Fx, NOARGS};
use crate::codegen::shim_sig;
use crate::codegen::task::{self, SiteKind, CANCEL_CODE, HDR, SITE_W, SUSPEND};
use crate::exec::StopKind;
use crate::ir::{Builtin, Callee, Func, Operand, Place, Term, Ty};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, BlockArg, InstBuilder, Value};
use cranelift_module::Module;
use std::collections::HashMap;

/// 中断点 1 つの生成の情報。
#[derive(Clone)]
pub(super) struct SiteInfo {
    pub k: usize,
    pub kind: SiteKind,
    /// 再開の入口（状態 `1 + 2k`）。呼び出しの中断点では子を呼び直す所、ブロックする操作では値を取る所。
    pub call_blk: cranelift_codegen::ir::Block,
    /// `task.scope` の範囲の終わりの待ち（状態 `2 + 2k`）。
    pub wait_blk: Option<cranelift_codegen::ir::Block>,
}

/// 再開可能な入口の生成の状態。
pub(super) struct Rs {
    pub frame: Value,
    pub nloc: usize,
    pub sites: HashMap<usize, SiteInfo>,
}

impl<'a, 'p> Fx<'a, 'p> {
    pub(super) fn frame(&self) -> Value {
        self.rs.as_ref().expect("再開可能な入口").frame
    }

    /// 局所 `i` の番地。再開可能な入口では枠の中、そうでなければ stack slot。
    pub(super) fn local_addr(&mut self, i: usize) -> Value {
        if let Some(rs) = &self.rs {
            let f = rs.frame;
            self.b.ins().iadd_imm_s(f, HDR as i64 + 8 * i as i64)
        } else {
            self.saddr(self.slots[i])
        }
    }

    /// 巻き戻しの停止の符号を置く語の番地。
    pub(super) fn pending_addr(&mut self) -> Value {
        if let Some(rs) = &self.rs {
            let f = rs.frame;
            self.b.ins().iadd_imm_s(f, 24)
        } else {
            let ps = self.pending.expect("pending");
            self.saddr(ps)
        }
    }

    /// 中断点 `k` の枠の中の語 `w`（0 子の枠、1・2 補助）の位置。
    fn site_off(&self, k: usize, w: usize) -> i32 {
        let nloc = self.rs.as_ref().expect("再開可能な入口").nloc;
        HDR + 8 * (nloc + SITE_W * k + w) as i32
    }

    fn plan_fn(&mut self, id: cranelift_module::FuncId, args: &[Value]) -> Vec<Value> {
        let fr = self.cg.m.declare_func_in_func(id, self.b.func);
        let inst = self.b.ins().call(fr, args);
        self.b.inst_results(inst).to_vec()
    }

    fn plan_ids(&self) -> (cranelift_module::FuncId, cranelift_module::FuncId) {
        let t = self.cg.task.as_ref().expect("変換の計画");
        (t.frame_done, t.drain)
    }

    fn cancelling(&mut self) -> Value {
        let d = self.cg.task.as_ref().expect("変換の計画").cancel_flag;
        let a = self.data_addr(d);
        self.ld(a, 0)
    }

    fn suspend_ret(&mut self) {
        let z = self.ic(0);
        let s = self.ic(SUSPEND);
        self.b.ins().return_(&[z, s]);
    }

    /// 子の入口を呼ぶ（中断したら自分も中断を返す）。完了したら子の枠を片付けて（値, 符号）を返す。
    fn child_step(&mut self, k: usize) -> (Value, Value) {
        let off = self.site_off(k, 0);
        let frame = self.frame();
        let child = self.ld(frame, off);
        self.guard_stack();
        let stepf = self.ld(child, 0);
        let sig = self.sigref(1);
        let inst = self.b.ins().call_indirect(sig, stepf, &[child]);
        let r = self.b.inst_results(inst).to_vec();
        let susp = self.b.ins().icmp_imm_s(IntCC::Equal, r[1], SUSPEND);
        let (sb, cont) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(susp, sb, NOARGS, cont, NOARGS);
        self.b.switch_to_block(sb);
        self.suspend_ret();
        self.b.switch_to_block(cont);
        let (done, _) = self.plan_ids();
        self.plan_fn(done, &[child]);
        let z = self.ic(0);
        let frame = self.frame();
        self.st(z, frame, off);
        (r[0], r[1])
    }

    // --- 入口 ---

    /// 再開可能な入口 `sigil_step_<f>(枠) -> (値, 符号)` の本体。
    pub fn run_step(&mut self, f: &Func, _fid: usize) {
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
        let frame = self.b.block_params(entry)[0];
        let (susp, ind) = {
            let t = self.cg.task.as_ref().expect("変換の計画");
            (t.susp.clone(), t.indirect_susp)
        };
        let mut sites = HashMap::new();
        let mut order = vec![];
        for (bi, bb) in f.blocks.iter().enumerate() {
            if let Some(kind) = task::classify(&bb.term, &susp, ind) {
                let call_blk = self.b.create_block();
                let wait_blk = matches!(kind, SiteKind::Block(Builtin::TaskScope))
                    .then(|| self.b.create_block());
                sites.insert(
                    bi,
                    SiteInfo {
                        k: order.len(),
                        kind,
                        call_blk,
                        wait_blk,
                    },
                );
                order.push(bi);
            }
        }
        self.rs = Some(Rs {
            frame,
            nloc: f.locals.len(),
            sites,
        });
        for _ in &f.blocks {
            let b = self.b.create_block();
            self.blocks.push(b);
        }
        // 状態番号で飛ぶ。0 は関数の先頭（取り消されていれば、引数だけ破棄して巻き戻す）。
        let state = self.ld(frame, 16);
        let (start, bad) = (self.b.create_block(), self.b.create_block());
        let is0 = self.b.ins().icmp_imm_s(IntCC::Equal, state, 0);
        let mut next = self.b.create_block();
        self.b.ins().brif(is0, start, NOARGS, next, NOARGS);
        for (k, bi) in order.iter().enumerate() {
            let si = self.rs.as_ref().expect("rs").sites[bi].clone();
            let mut targets = vec![(1 + 2 * k as i64, si.call_blk)];
            if let Some(w) = si.wait_blk {
                targets.push((2 + 2 * k as i64, w));
            }
            for (s, blk) in targets {
                self.b.switch_to_block(next);
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, state, s);
                next = self.b.create_block();
                self.b.ins().brif(eq, blk, NOARGS, next, NOARGS);
            }
        }
        self.b.switch_to_block(next);
        self.b.ins().jump(bad, NOARGS);
        self.b.switch_to_block(bad);
        let z = self.ic(0);
        let c = self.ic(CANCEL_CODE);
        self.b.ins().return_(&[z, c]);

        self.b.switch_to_block(start);
        let can = self.cancelling();
        let (cstart, go) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cstart, NOARGS, go, NOARGS);
        self.b.switch_to_block(cstart);
        for &pl in f.params.iter().rev() {
            let t = norm(&f.locals[pl].ty);
            if aggregate(&t) {
                let a = self.local_addr(pl);
                let w = self.ld(a, 0);
                let z = self.ic(0);
                self.st(z, a, 0);
                self.drop_val(&t, w);
            }
        }
        let z = self.ic(0);
        let c = self.ic(CANCEL_CODE);
        self.b.ins().return_(&[z, c]);
        self.b.switch_to_block(go);
        self.b.ins().jump(self.blocks[0], NOARGS);

        for (i, bb) in f.blocks.iter().enumerate() {
            self.b.switch_to_block(self.blocks[i]);
            self.cur_bb = i;
            for s in &bb.stmts {
                self.stmt(s);
            }
            self.term(&bb.term);
        }
    }

    /// 作成関数 `sigil_mk_<f>(引数の並び) -> 枠`。枠を確保して 0 で埋め、頭と引数を書く。
    pub fn mk_body(&mut self, f: &Func, fid: usize) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let argv = self.b.block_params(entry)[0];
        let (size, step) = {
            let t = self.cg.task.as_ref().expect("変換の計画");
            (task::frame_bytes(f, &t.susp, t.indirect_susp), t.step[&fid])
        };
        let sz = self.ic(size);
        let eight = self.ic(8);
        let fr = self.rt("raw_alloc", &[sz, eight]);
        let z = self.ic(0);
        self.rt("memset", &[fr, z, sz]);
        let sr = self.cg.m.declare_func_in_func(step, self.b.func);
        let sa = self.b.ins().func_addr(types::I64, sr);
        self.st(sa, fr, 0);
        self.st(sz, fr, 32);
        let rd = self.drop_addr(&norm(&f.ret));
        self.st(rd, fr, 48);
        for (i, &pl) in f.params.iter().enumerate() {
            let v = self.ld(argv, 8 * i as i32);
            self.st(v, fr, HDR + 8 * pl as i32);
        }
        self.b.ins().return_(&[fr]);
    }

    // --- 中断点 ---

    /// 呼び出しの終端が中断点のときの生成。
    pub(super) fn site_call(
        &mut self,
        si: SiteInfo,
        func: &Callee,
        args: &[Operand],
        dest: &Place,
        next: usize,
        stop: usize,
    ) {
        let p = self.cg.p;
        let (_, dc) = self.place(dest);
        let dty = Self::cur_ty(&dc);
        let frame = self.frame();
        let k = si.k;
        match si.kind {
            SiteKind::Direct(g) => {
                let callee = &p.funcs[g];
                let mut vs = vec![];
                for (i, a) in args.iter().enumerate() {
                    let h = callee.params.get(i).map(|&l| norm(&callee.locals[l].ty));
                    vs.push(self.argf(a, h.as_ref()).0);
                }
                let argv = self.zero_tmp(8 * std::cmp::max(vs.len(), 1) as u32);
                for (i, v) in vs.iter().enumerate() {
                    self.st(*v, argv, 8 * i as i32);
                }
                let mk = self.cg.task.as_ref().expect("計画").mk[&g];
                let child = self.plan_fn(mk, &[argv])[0];
                self.st(child, frame, self.site_off(k, 0));
                let s = self.ic(1 + 2 * k as i64);
                self.st(s, frame, 16);
                self.b.ins().jump(si.call_blk, NOARGS);
                self.b.switch_to_block(si.call_blk);
                let (v, code) = self.child_step(k);
                self.after_call(code);
                self.finish_call(v, &norm(&callee.ret), &dty, dest, next);
            }
            SiteKind::Indirect => {
                let Callee::Indirect(op) = func else {
                    unreachable!()
                };
                let (fv, fty) = self.val(op);
                let (ps, r) = match strip(&fty) {
                    Ty::FnRef(ps, r) => (ps, *r),
                    _ => {
                        let v = self.unsupported("関数の値でないものを呼んでいる");
                        self.finish_call(v, &Ty::Unit, &dty, dest, next);
                        return;
                    }
                };
                let mut vs = vec![];
                for (i, a) in args.iter().enumerate() {
                    vs.push(self.argf(a, ps.get(i)).0);
                }
                let fp = self.ld(fv, 0);
                let info = self.zero_tmp(8);
                let lookup = self.cg.task.as_ref().expect("計画").fn_lookup;
                let mk = self.plan_fn(lookup, &[fp, info])[0];
                let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, mk, 0);
                let (rb, pb) = (self.b.create_block(), self.b.create_block());
                self.b.ins().brif(has, rb, NOARGS, pb, NOARGS);
                // 中断しない関数: その場で呼ぶ。
                self.b.switch_to_block(pb);
                let (v, code) = self.call_fnref_raw(fv, &vs);
                self.after_call(code);
                self.finish_call(v, &r, &dty, dest, next);
                // 中断しうる関数: 子の枠を作る（クロージャなら環境の番地を先頭に足す）。
                self.b.switch_to_block(rb);
                let env = self.ld(fv, 8);
                let buf = self.zero_tmp(8 * (vs.len() as u32 + 1));
                let envp = self.b.ins().iadd_imm_s(fv, 8);
                self.st(envp, buf, 0);
                for (i, v) in vs.iter().enumerate() {
                    self.st(*v, buf, 8 + 8 * i as i32);
                }
                let plain = self.b.ins().icmp_imm_s(IntCC::Equal, env, 0);
                let eight = self.ic(8);
                let z = self.ic(0);
                let off = self.b.ins().select(plain, eight, z);
                let ptr = self.b.ins().iadd(buf, off);
                let sig = self.b.import_signature(shim_sig(self.cg.cc, 1, 'p'));
                let inst = self.b.ins().call_indirect(sig, mk, &[ptr]);
                let child = self.b.inst_results(inst)[0];
                let frame = self.frame();
                self.st(child, frame, self.site_off(k, 0));
                let s = self.ic(1 + 2 * k as i64);
                self.st(s, frame, 16);
                self.b.ins().jump(si.call_blk, NOARGS);
                self.b.switch_to_block(si.call_blk);
                let (v, code) = self.child_step(k);
                self.after_call(code);
                self.finish_call(v, &r, &dty, dest, next);
            }
            SiteKind::Block(b) => self.block_site(&si, b, args, dest, &dty, next, stop),
        }
    }

    /// ブロックしうる操作の中断点。`pre` で操作を起こし、ブロックしなければその場の値で続け、ブロックしたら状態を
    /// 保存して中断を返す。起こされたら `call_blk`（状態 `1 + 2k`）から入り、起床の値で続ける。
    #[allow(clippy::too_many_arguments)]
    fn block_site(
        &mut self,
        si: &SiteInfo,
        b: Builtin,
        args: &[Operand],
        dest: &Place,
        dty: &Ty,
        next: usize,
        stop: usize,
    ) {
        use Builtin::*;
        let _ = stop;
        let k = si.k;
        if b == TaskScope {
            self.scope_site(si, args, dest, dty, next);
            return;
        }
        // 操作を起こして、ブロックしなかったときの値（ブロックしたときは意味がない）を作る。
        let (r, force) = match b {
            TaskYield => {
                let z = self.ic(0);
                self.rt("sched_yield_val", &[z]);
                (self.ic(0), true)
            }
            TaskSleep => {
                let (ms, _) = self.val(&args[0]);
                self.rt("sched_sleep", &[ms]);
                (self.ic(0), true)
            }
            AtomicLoad => {
                let w = self.shared_get(&args[0]);
                self.rt("sched_yield_val", &[w]);
                (w, true)
            }
            TaskJoin => {
                let (t, _) = self.val(&args[0]);
                (self.rt("sched_join", &[t]), false)
            }
            ChanRecv => {
                let (c, _) = self.val(&args[0]);
                (self.rt("sched_chan_recv", &[c]), false)
            }
            LockAcquire => {
                let (l, _) = self.val(&args[0]);
                (self.rt("sched_lock_acquire", &[l]), false)
            }
            _ => unreachable!("中断点でない操作"),
        };
        let frame = self.frame();
        let s = self.ic(1 + 2 * k as i64);
        let suspend = self.b.create_block();
        let okb = self.b.create_block();
        if force {
            self.b.ins().jump(suspend, NOARGS);
        } else {
            let bl = self.rt("sched_blocked", &[]);
            self.b.ins().brif(bl, suspend, NOARGS, okb, NOARGS);
        }
        self.b.switch_to_block(suspend);
        self.st(s, frame, 16);
        self.suspend_ret();
        if !force {
            self.b.switch_to_block(okb);
            let v = self.post_block(b, r);
            self.finish_call(v, dty, dty, dest, next);
        }
        // 起こされた。取り消しならそのまま巻き戻す。
        self.b.switch_to_block(si.call_blk);
        let can = self.cancelling();
        let (cb, rb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cb, NOARGS, rb, NOARGS);
        self.b.switch_to_block(cb);
        let c = self.ic(CANCEL_CODE);
        self.jump_cleanup(c);
        self.b.switch_to_block(rb);
        let r2 = self.rt("sched_take_resume", &[]);
        let v = self.post_block(b, r2);
        self.finish_call(v, dty, dty, dest, next);
    }

    /// 操作の結果の語を、型の値にする（`Lock` の取得は `Ok(LockGuard)` を作る）。
    fn post_block(&mut self, b: Builtin, r: Value) -> Value {
        if b == Builtin::LockAcquire {
            // 守りの塊: [0 = 空（破棄で何も破棄しない）][8 = Lock のレコード]。破棄の知らせの印を頭に立てる。
            let g = self.blk(16);
            self.st(r, g, 8);
            self.set_guard(g);
            self.mk_enum_c(0, &[g])
        } else {
            r
        }
    }

    /// `task.scope(f)`: 範囲を作り、本体を子として呼び、戻ったら子のタスクの完了を待つ。
    fn scope_site(&mut self, si: &SiteInfo, args: &[Operand], dest: &Place, dty: &Ty, next: usize) {
        let k = si.k;
        let frame = self.frame();
        let (fv, _) = self.own(&args[0]);
        let sid = self.rt("sched_scope_begin", &[]);
        let aux1 = self.site_off(k, 1);
        let aux2 = self.site_off(k, 2);
        self.st(sid, frame, aux1);
        let sref = self.b.ins().iadd_imm_s(frame, aux1 as i64);
        let argv = self.zero_tmp(8);
        self.st(sref, argv, 0);
        let one = self.ic(1);
        let start = self.cg.task.as_ref().expect("計画").fn_start;
        let child = self.plan_fn(start, &[fv, argv, one, one])[0];
        self.st(child, frame, self.site_off(k, 0));
        let s = self.ic(1 + 2 * k as i64);
        self.st(s, frame, 16);
        self.b.ins().jump(si.call_blk, NOARGS);
        self.b.switch_to_block(si.call_blk);
        let (v, code) = self.child_step(k);
        let (fail, okb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(code, fail, NOARGS, okb, NOARGS);
        self.b.switch_to_block(fail);
        self.jump_cleanup(code);
        self.b.switch_to_block(okb);
        let frame = self.frame();
        let sid2 = self.ld(frame, aux1);
        let wait = self.rt("sched_scope_end", &[sid2]);
        let (wb, done) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(wait, wb, NOARGS, done, NOARGS);
        self.b.switch_to_block(wb);
        self.st(v, frame, aux2);
        let s2 = self.ic(2 + 2 * k as i64);
        self.st(s2, frame, 16);
        self.suspend_ret();
        self.b.switch_to_block(done);
        let (_, drain) = self.plan_ids();
        self.plan_fn(drain, &[]);
        self.finish_call(v, dty, dty, dest, next);
        // 子のタスクがすべて終わって起こされた。
        let wblk = si.wait_blk.expect("scope の待ち");
        self.b.switch_to_block(wblk);
        let frame = self.frame();
        let can = self.cancelling();
        let (cb, rb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cb, NOARGS, rb, NOARGS);
        self.b.switch_to_block(cb);
        let w = self.ld(frame, aux2);
        let z = self.ic(0);
        self.st(z, frame, aux2);
        self.drop_val(dty, w);
        let c = self.ic(CANCEL_CODE);
        self.jump_cleanup(c);
        self.b.switch_to_block(rb);
        let v = self.ld(frame, aux2);
        self.finish_call(v, dty, dty, dest, next);
    }

    /// 中断しない並行処理の操作と、入れ子の呼び出しの中での中断点（評価器と同じ停止）。
    pub(super) fn task_builtin(
        &mut self,
        b: Builtin,
        args: &[Operand],
        stop: usize,
    ) -> Option<(Value, Ty)> {
        use Builtin::*;
        let u = Ty::Unit;
        Some(match b {
            TaskChannel => {
                let id = self.rt("sched_chan_new", &[]);
                let t = self.blk(16);
                self.st(id, t, 0);
                self.st(id, t, 8);
                (t, u)
            }
            ChanSend => {
                let (id, _) = self.val(&args[0]);
                let (x, xt) = self.own(&args[1]);
                let df = self.drop_addr(&xt);
                self.rt("sched_chan_send", &[id, x, df]);
                (self.ic(0), u)
            }
            LockNew => {
                let (x, xt) = self.own(&args[0]);
                let df = self.drop_addr(&xt);
                (self.rt("sched_lock_new", &[x, df]), u)
            }
            LockGuardGet | LockGuardGetMut => {
                let (g, _) = self.val(&args[0]);
                let l = self.ld(g, 8);
                (l, Ty::Ref(Box::new(Ty::Unit)))
            }
            SyncSharedNew | AtomicNew => {
                let (x, _) = self.own(&args[0]);
                let inner = self.blk(8);
                self.st(x, inner, 0);
                let outer = self.blk(8);
                self.st(inner, outer, 0);
                (outer, u)
            }
            ScopeSpawn => {
                let Some(start) = self.cg.task.as_ref().map(|t| t.fn_start) else {
                    return Some((self.unsupported("タスクを起こす関数が変換されていない"), u));
                };
                let (sid, _) = self.val(&args[0]);
                let (fv, _) = self.own(&args[1]);
                let argv = self.zero_tmp(8);
                let z = self.ic(0);
                let one = self.ic(1);
                let child = self.plan_fn(start, &[fv, argv, z, one])[0];
                let rd = self.ld(child, 48);
                (self.rt("sched_spawn", &[sid, child, rd]), u)
            }
            TaskAllOk | TaskRace | TaskTimeout => (
                self.unsupported(
                    "task.all_ok・task.race・task.timeout はネイティブ出力未対応（取り消しの意味を要する。ADR 0030 の 5-2g）",
                ),
                u,
            ),
            // 入れ子の呼び出し（関数値の呼び出しを経る組み込みの中）では、タスクを中断できない（評価器と同じ停止）。
            b if task::blocking(b) => {
                if self.cg.rt.contains_key("sched_start") {
                    let code = self.cg.code_for(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("関数値の呼び出しの中ではタスクを中断できない"),
                    );
                    let one = self.ic(1);
                    self.fail_if(one, code);
                    (self.ic(0), u)
                } else if b == TaskYield {
                    (self.ic(0), u)
                } else {
                    (self.unsupported("並行処理の操作はネイティブ出力未対応"), u)
                }
            }
            _ => return None,
        })
    }

    /// `Atomic`・`SyncShared` の中身の影（`SharedGet` と同じ）。
    fn shared_get(&mut self, op: &Operand) -> Value {
        let (v, ty) = self.val(op);
        let (w, inner) = match strip(&ty) {
            Ty::Shared(i) => match *i {
                Ty::Cell(t) => {
                    let c = self.ld(v, 0);
                    (self.ld(c, 0), *t)
                }
                t => (self.ld(v, 0), t),
            },
            Ty::Cell(t) => (self.ld(v, 0), *t),
            _ => return self.unsupported("load の対象が Atomic でない"),
        };
        if aggregate(&inner) {
            self.shadow(&inner, w)
        } else {
            w
        }
    }

    // --- 補助関数 ---

    /// 入れ子の呼び出しの本体 `sigil_plain_step(枠)`: 中断しない関数を、枠の持つ関数の値でそのまま呼ぶ。
    pub fn plain_step_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let frame = self.b.block_params(entry)[0];
        let can = self.cancelling();
        let (cb, go) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cb, NOARGS, go, NOARGS);
        self.b.switch_to_block(cb);
        let z = self.ic(0);
        let c = self.ic(CANCEL_CODE);
        self.b.ins().return_(&[z, c]);
        self.b.switch_to_block(go);
        let fv = self.ld(frame, HDR);
        let n = self.ld(frame, HDR + 8);
        let a0 = self.ld(frame, HDR + 16);
        let (b0, b1) = (self.b.create_block(), self.b.create_block());
        let is0 = self.b.ins().icmp_imm_s(IntCC::Equal, n, 0);
        self.b.ins().brif(is0, b0, NOARGS, b1, NOARGS);
        self.b.switch_to_block(b0);
        let (v, c) = self.call_fnref_raw(fv, &[]);
        self.b.ins().return_(&[v, c]);
        self.b.switch_to_block(b1);
        let (v, c) = self.call_fnref_raw(fv, &[a0]);
        self.b.ins().return_(&[v, c]);
    }

    /// `sigil_fn_lookup(関数の番地, 情報の番地) -> 作成関数の番地`。関数の値として使われる関数を引く。見つかれば結果の
    /// 破棄の関数の番地を情報の先頭に書き、中断しうる関数なら作成関数の番地を返す（そうでなければ 0）。
    pub fn fn_lookup_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let (fp, info) = (ps[0], ps[1]);
        let (taken, susp, mk) = {
            let t = self.cg.task.as_ref().expect("計画");
            (t.addr_taken.clone(), t.susp.clone(), t.mk.clone())
        };
        let p = self.cg.p;
        for f in taken {
            let fr = self.cg.m.declare_func_in_func(self.cg.fids[f], self.b.func);
            let fa = self.b.ins().func_addr(types::I64, fr);
            let eq = self.b.ins().icmp(IntCC::Equal, fp, fa);
            let (hit, nx) = (self.b.create_block(), self.b.create_block());
            self.b.ins().brif(eq, hit, NOARGS, nx, NOARGS);
            self.b.switch_to_block(hit);
            let rd = self.drop_addr(&norm(&p.funcs[f].ret));
            self.st(rd, info, 0);
            let mv = if susp[f] {
                let mr = self.cg.m.declare_func_in_func(mk[&f], self.b.func);
                self.b.ins().func_addr(types::I64, mr)
            } else {
                self.ic(0)
            };
            self.b.ins().return_(&[mv]);
            self.b.switch_to_block(nx);
        }
        let z = self.ic(0);
        self.st(z, info, 0);
        self.b.ins().return_(&[z]);
    }

    /// `sigil_fn_start(関数の値, 引数の並び, 引数の数(0|1), 持ち主か) -> 枠`。関数の値からタスクの枠を作る。中断しない関数は
    /// `sigil_plain_step` の枠に包む。持ち主なら枠が完了するときに関数の値を破棄する。
    pub fn fn_start_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let (fv, argv, n, own) = (ps[0], ps[1], ps[2], ps[3]);
        let (lookup, plain_step) = {
            let t = self.cg.task.as_ref().expect("計画");
            (t.fn_lookup, t.plain_step)
        };
        let fp = self.ld(fv, 0);
        let info = self.zero_tmp(8);
        let mk = self.plan_fn(lookup, &[fp, info])[0];
        let rd = self.ld(info, 0);
        let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, mk, 0);
        let (rb, pb, merge) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(merge, types::I64);
        self.b.ins().brif(has, rb, NOARGS, pb, NOARGS);

        self.b.switch_to_block(rb);
        let env = self.ld(fv, 8);
        let buf = self.zero_tmp(16);
        let envp = self.b.ins().iadd_imm_s(fv, 8);
        self.st(envp, buf, 0);
        let a0 = self.ld(argv, 0);
        self.st(a0, buf, 8);
        let plain = self.b.ins().icmp_imm_s(IntCC::Equal, env, 0);
        let eight = self.ic(8);
        let z = self.ic(0);
        let off = self.b.ins().select(plain, eight, z);
        let ptr = self.b.ins().iadd(buf, off);
        let sig = self.b.import_signature(shim_sig(self.cg.cc, 1, 'p'));
        let inst = self.b.ins().call_indirect(sig, mk, &[ptr]);
        let child = self.b.inst_results(inst)[0];
        self.b.ins().jump(merge, &[BlockArg::Value(child)]);

        self.b.switch_to_block(pb);
        let sz = self.ic(HDR as i64 + 24);
        let eight = self.ic(8);
        let fr = self.rt("raw_alloc", &[sz, eight]);
        let z = self.ic(0);
        self.rt("memset", &[fr, z, sz]);
        let sr = self.cg.m.declare_func_in_func(plain_step, self.b.func);
        let sa = self.b.ins().func_addr(types::I64, sr);
        self.st(sa, fr, 0);
        self.st(sz, fr, 32);
        self.st(fv, fr, HDR);
        self.st(n, fr, HDR + 8);
        let a0 = self.ld(argv, 0);
        self.st(a0, fr, HDR + 16);
        self.b.ins().jump(merge, &[BlockArg::Value(fr)]);

        self.b.switch_to_block(merge);
        let frame = self.b.block_params(merge)[0];
        self.st(rd, frame, 48);
        let z = self.ic(0);
        let has_own = self.b.ins().icmp_imm_s(IntCC::NotEqual, own, 0);
        let owner = self.b.ins().select(has_own, fv, z);
        self.st(owner, frame, 40);
        self.b.ins().return_(&[frame]);
    }

    /// `sigil_frame_done(枠)`: 完了（または停止）した枠を片付ける。持ち主の関数の値を破棄し、枠を返す。
    pub fn frame_done_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let frame = self.b.block_params(entry)[0];
        let owner = self.ld(frame, 40);
        let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, owner, 0);
        let (d, e) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(has, d, NOARGS, e, NOARGS);
        self.b.switch_to_block(d);
        self.drop_val(&Ty::FnRef(vec![], Box::new(Ty::Unit)), owner);
        self.b.ins().jump(e, NOARGS);
        self.b.switch_to_block(e);
        let size = self.ld(frame, 32);
        let eight = self.ic(8);
        self.rt("raw_free", &[frame, size, eight]);
        self.b.ins().return_(&[]);
    }

    /// `sigil_drain()`: スケジューラが積んだ「捨てる値」を取り出して破棄する。
    pub fn drain_body(&mut self) {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        let (head, body, end) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(head);
        let n = self.rt("sched_garbage_pop", &[]);
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, n, 0);
        self.b.ins().brif(zero, end, NOARGS, body, NOARGS);
        self.b.switch_to_block(body);
        let v = self.ld(n, 8);
        let f = self.ld(n, 16);
        self.rt("sched_garbage_free", &[n]);
        let sig = self.b.import_signature(shim_sig(self.cg.cc, 1, 'v'));
        self.b.ins().call_indirect(sig, f, &[v]);
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(end);
        self.b.ins().return_(&[]);
    }

    /// `sigil_sched_run(ルートの枠) -> (値, 符号)`: 決定的な単一ワーカーの駆動の輪。実行可能なタスクを 1 つ取り、
    /// 入口を呼ぶ。中断なら次へ、完了ならスケジューラに知らせ、停止なら残りのタスクを新しいものから取り消す。
    pub fn sched_run_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let root = self.b.block_params(entry)[0];
        let (done, drain) = self.plan_ids();
        let rd = self.ld(root, 48);
        self.rt("sched_start", &[root, rd]);
        let (head, idle, run, deadlock) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);

        self.b.switch_to_block(head);
        let f = self.rt("sched_next_task", &[]);
        let none = self.b.ins().icmp_imm_s(IntCC::Equal, f, 0);
        self.b.ins().brif(none, idle, NOARGS, run, NOARGS);

        self.b.switch_to_block(idle);
        let r = self.rt("sched_idle", &[]);
        self.b.ins().brif(r, head, NOARGS, deadlock, NOARGS);

        // 進行できるタスクがない（デッドロック。評価器と同じ停止）。
        self.b.switch_to_block(deadlock);
        let code = self.cg.code_for(
            0,
            Some(StopKind::StdRequiresViolated),
            Some("デッドロック（進行できるタスクがない）"),
        );
        let c = self.ic(code);
        self.cancel_all_and_return(c);

        self.b.switch_to_block(run);
        let sig = self.sigref(1);
        let stepf = self.ld(f, 0);
        let inst = self.b.ins().call_indirect(sig, stepf, &[f]);
        let r = self.b.inst_results(inst).to_vec();
        let (susp, fin) = (self.b.create_block(), self.b.create_block());
        let is_susp = self.b.ins().icmp_imm_s(IntCC::Equal, r[1], SUSPEND);
        self.b.ins().brif(is_susp, susp, NOARGS, fin, NOARGS);

        self.b.switch_to_block(susp);
        self.plan_fn(drain, &[]);
        self.b.ins().jump(head, NOARGS);

        self.b.switch_to_block(fin);
        let (fail, okb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(r[1], fail, NOARGS, okb, NOARGS);

        self.b.switch_to_block(okb);
        self.plan_fn(done, &[f]);
        self.rt("sched_finish_task", &[r[0]]);
        self.plan_fn(drain, &[]);
        let rdone = self.rt("sched_root_done", &[]);
        let (fin_ok, again) = (self.b.create_block(), head);
        self.b.ins().brif(rdone, fin_ok, NOARGS, again, NOARGS);
        self.b.switch_to_block(fin_ok);
        let res = self.rt("sched_root_result", &[]);
        self.rt("sched_collect_garbage", &[]);
        self.plan_fn(drain, &[]);
        self.rt("sched_release", &[]);
        let z = self.ic(0);
        self.b.ins().return_(&[res, z]);

        self.b.switch_to_block(fail);
        self.plan_fn(done, &[f]);
        self.cancel_all_and_return(r[1]);
    }

    /// 残りのタスクを新しいものから順に取り消し、後始末をして、符号 `code` の停止を返す。
    fn cancel_all_and_return(&mut self, code: Value) {
        let (_, drain) = self.plan_ids();
        let flag = self.cg.task.as_ref().expect("計画").cancel_flag;
        let fa = self.data_addr(flag);
        let one = self.ic(1);
        self.st(one, fa, 0);
        let (head, body, end) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(head);
        let g = self.rt("sched_cancel_next", &[]);
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, g, 0);
        self.b.ins().brif(zero, end, NOARGS, body, NOARGS);
        self.b.switch_to_block(body);
        let sig = self.sigref(1);
        let stepf = self.ld(g, 0);
        self.b.ins().call_indirect(sig, stepf, &[g]);
        let (done, _) = self.plan_ids();
        self.plan_fn(done, &[g]);
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(end);
        let fa = self.data_addr(flag);
        let z = self.ic(0);
        self.st(z, fa, 0);
        self.rt("sched_collect_garbage", &[]);
        self.plan_fn(drain, &[]);
        self.rt("sched_release", &[]);
        let z = self.ic(0);
        self.b.ins().return_(&[z, code]);
    }

    /// 入口の `thunk` から: 入口の関数が中断しうるとき、枠を作って駆動の輪で動かし、（値, 符号）を返す。
    pub(super) fn run_entry_task(&mut self, entry: usize, vs: &[Value]) -> Vec<Value> {
        let (mk, run) = {
            let t = self.cg.task.as_ref().expect("計画");
            (t.mk[&entry], t.sched_run)
        };
        let argv = self.zero_tmp(8 * std::cmp::max(vs.len(), 1) as u32);
        for (i, v) in vs.iter().enumerate() {
            self.st(*v, argv, 8 * i as i32);
        }
        let frame = self.plan_fn(mk, &[argv])[0];
        self.plan_fn(run, &[frame])
    }
}

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

/// 局所の語がそのまま値を表すか（空転の指紋に使える型か）。参照は指す先が連鎖の枠かクロージャの環境なので許す。
fn fp_checkable(t: &Ty) -> bool {
    match t {
        Ty::Int
        | Ty::Float
        | Ty::Bool
        | Ty::Unit
        | Ty::Addr
        | Ty::ParseError
        | Ty::Ref(_)
        | Ty::MutRef(_) => true,
        Ty::Shared(i) => {
            matches!(&**i, Ty::Cell(c) if fp_checkable(c) && !matches!(**c, Ty::Ref(_) | Ty::MutRef(_)))
        }
        _ => false,
    }
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
        let base = self.region_base();
        let a = self
            .b
            .ins()
            .iadd_imm_s(base, crate::codegen::TCB_CANCEL_OFF);
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
                let wait_blk = matches!(
                    kind,
                    SiteKind::Block(Builtin::TaskScope | Builtin::RootsScope)
                )
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
        // 子の枠の位置の始まり（空転の検出の指紋が、局所と子を分けて辿る）。
        let cb = self.ic(HDR as i64 + 8 * f.locals.len() as i64);
        // 値を語として比べるだけでは変化が分からない局所（一覧・文字列・構造体など、中身が別の塊にあるもの）を持つ関数は、
        // 空転の検出の対象外にする印（高位ビット）を立てる。評価器は中身まで指紋に入れるので、誤って時計を進めない側に倒す。
        let opaque = f.locals.iter().any(|l| !fp_checkable(&norm(&l.ty)));
        let cb = if opaque {
            self.b.ins().bor_imm_s(cb, 1 << 40)
        } else {
            cb
        };
        self.st(cb, fr, 8);
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
        if b == TaskTimeout {
            self.timeout_site(si, args, dest, dty, next);
            return;
        }
        if b == RootsScope {
            self.roots_site(si, args, dest, dty, next, stop);
            return;
        }
        let roots = self.cg.task.as_ref().is_some_and(|t| t.roots);
        // 操作を起こして、ブロックしなかったときの値（ブロックしたときは意味がない）を作る。
        let (r, force) = match b {
            TaskYield => {
                self.rt("sched_yield_plain", &[]);
                (self.ic(0), true)
            }
            TaskSleep => {
                let (ms, _) = self.val(&args[0]);
                self.rt("sched_sleep", &[ms]);
                (self.ic(0), true)
            }
            IoWaitFd => {
                let (fd, _) = self.val(&args[0]);
                let (ev, _) = self.val(&args[1]);
                (self.rt("sched_io_wait", &[fd, ev]), false)
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
                if roots {
                    self.dead_check("sched_chan_dead", c, "通信路", stop);
                }
                (self.rt("sched_chan_recv", &[c]), false)
            }
            LockAcquire => {
                let (l, _) = self.val(&args[0]);
                if roots {
                    self.dead_check("sched_lock_dead", l, "Lock", stop);
                }
                (self.rt("sched_lock_acquire", &[l]), false)
            }
            RootReserve => {
                let (g, _) = self.val(&args[0]);
                let frame = self.frame();
                self.st(g, frame, self.site_off(k, 1));
                let r = self.rt("sched_reserve", &[g]);
                let closed = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
                let code = self.cg.code_for(
                    stop,
                    Some(StopKind::StdRequiresViolated),
                    Some("RootGroup.reserve: task.roots の範囲が終わっている"),
                );
                self.fail_if(closed, code);
                (r, false)
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
            let v = self.post_block(b, r, k);
            self.finish_call(v, dty, dty, dest, next);
        }
        // 起こされた。取り消しならそのまま巻き戻す。
        self.b.switch_to_block(si.call_blk);
        let can = self.cancelling();
        let (cb, rb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cb, NOARGS, rb, NOARGS);
        self.b.switch_to_block(cb);
        // 起こされたが再開前に取り消された: 渡された所有値を破棄する（評価器の cancel_task が resume を捨てるのと同じ）。
        if b == RootReserve {
            // 渡されたスロットを受け取る前に取り消された: スロットを返す（評価器では起床の値の破棄で返る）。
            let has = self.rt("sched_has_resume", &[]);
            let (rel, after) = (self.b.create_block(), self.b.create_block());
            self.b.ins().brif(has, rel, NOARGS, after, NOARGS);
            self.b.switch_to_block(rel);
            self.rt("sched_take_resume", &[]);
            let g = self.ld(self.frame(), self.site_off(k, 1));
            self.rt("sched_slot_release", &[g]);
            self.b.ins().jump(after, NOARGS);
            self.b.switch_to_block(after);
        }
        if matches!(b, TaskJoin | ChanRecv | AtomicLoad) {
            let has = self.rt("sched_has_resume", &[]);
            let (dropb, after) = (self.b.create_block(), self.b.create_block());
            self.b.ins().brif(has, dropb, NOARGS, after, NOARGS);
            self.b.switch_to_block(dropb);
            let w = self.rt("sched_take_resume", &[]);
            // すぐには破棄せず、ごみの列に積む（局所・持ち主の後に、駆動の輪が破棄する）。
            let df = self.drop_addr(dty);
            self.rt("sched_defer_drop", &[w, df]);
            self.b.ins().jump(after, NOARGS);
            self.b.switch_to_block(after);
        }
        let c = self.ic(CANCEL_CODE);
        self.jump_cleanup(c);
        self.b.switch_to_block(rb);
        let r2 = self.rt("sched_take_resume", &[]);
        let v = self.post_block(b, r2, k);
        self.finish_call(v, dty, dty, dest, next);
    }

    /// `task.timeout(ms, g)`: 新しい範囲に `g` を子のタスクとして起こし、子の完了と期限のどちらか先までブロックする
    /// （評価器の `TaskTimeout`）。子が先なら `Some(値)`。期限が先なら範囲の子孫を取り消して（評価器の `cancel_scope`
    /// と同じく、新しい子から）`None`。
    fn timeout_site(
        &mut self,
        si: &SiteInfo,
        args: &[Operand],
        dest: &Place,
        dty: &Ty,
        next: usize,
    ) {
        let k = si.k;
        let Some(start) = self.cg.task.as_ref().map(|t| t.fn_start) else {
            let v = self.unsupported("タスクを起こす関数が変換されていない");
            self.finish_call(v, dty, dty, dest, next);
            return;
        };
        let aux1 = self.site_off(k, 1);
        let (ms, _) = self.val(&args[0]);
        let (fv, _) = self.own(&args[1]);
        let sid = self.rt("sched_scope_begin", &[]);
        let argv = self.zero_tmp(8);
        let z = self.ic(0);
        let one = self.ic(1);
        let child = self.plan_fn(start, &[fv, argv, z, one])[0];
        let rd = self.ld(child, 48);
        let tid = self.rt("sched_spawn", &[sid, child, rd]);
        let frame = self.frame();
        self.st(sid, frame, aux1);
        let r = self.rt("sched_timeout_wait", &[tid, ms]);
        let s = self.ic(1 + 2 * k as i64);
        let suspend = self.b.create_block();
        let okb = self.b.create_block();
        let bl = self.rt("sched_blocked", &[]);
        self.b.ins().brif(bl, suspend, NOARGS, okb, NOARGS);
        self.b.switch_to_block(suspend);
        self.st(s, self.frame(), 16);
        self.suspend_ret();
        // 子がすでに終わっていた: Some(結果)。
        self.b.switch_to_block(okb);
        let v = self.mk_enum_c(0, &[r]);
        self.finish_call(v, dty, dty, dest, next);
        // 起こされた。取り消しならそのまま巻き戻す。
        self.b.switch_to_block(si.call_blk);
        let can = self.cancelling();
        let (cb, rb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cb, NOARGS, rb, NOARGS);
        self.b.switch_to_block(cb);
        // 起こされたが再開前に取り消された: 子の結果（`Some` に包む前の `T`）を受け取っていれば、ごみの列に積む。
        let has = self.rt("sched_has_resume", &[]);
        let (dropb, after) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(has, dropb, NOARGS, after, NOARGS);
        self.b.switch_to_block(dropb);
        let w = self.rt("sched_take_resume", &[]);
        let inner = self.option_inner(dty);
        let df = self.drop_addr(&inner);
        self.rt("sched_defer_drop", &[w, df]);
        self.b.ins().jump(after, NOARGS);
        self.b.switch_to_block(after);
        let c = self.ic(CANCEL_CODE);
        self.jump_cleanup(c);
        self.b.switch_to_block(rb);
        let fired = self.rt("sched_tmo_fired", &[]);
        let (fb, vb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(fired, fb, NOARGS, vb, NOARGS);
        // 時間切れ: 範囲の子孫を取り消してから None。
        self.b.switch_to_block(fb);
        let frame = self.frame();
        let sid2 = self.ld(frame, aux1);
        self.cancel_loop_by(Some(sid2), "sched_tmo_cancel_next");
        let zz = self.ic(0);
        let none = self.mk_enum_c(1, &[zz]);
        self.finish_call(none, dty, dty, dest, next);
        // 子が先に終わった: Some(値)。
        self.b.switch_to_block(vb);
        let r2 = self.rt("sched_take_resume", &[]);
        let some = self.mk_enum_c(0, &[r2]);
        self.finish_call(some, dty, dty, dest, next);
    }

    /// `Option<T>` の `T`。
    fn option_inner(&self, dty: &Ty) -> Ty {
        let Ty::Adt(id) = dty else {
            unreachable!("task.timeout の戻りは Option")
        };
        match &self.cg.p.types[*id].kind {
            crate::ir::TypeKind::Enum { variants } => variants[0].fields[0].clone(),
            _ => unreachable!("Option は列挙型"),
        }
    }

    /// 回収済みの通信路・`Lock`（作ったルートが終わった。ADR 0031 決定 4-2）の使用を停止にする。
    fn dead_check(&mut self, f: &'static str, id: Value, what: &str, stop: usize) {
        let dead = self.rt(f, &[id]);
        let code = self.cg.code_for(
            stop,
            Some(StopKind::StdRequiresViolated),
            Some(&format!(
                "回収済みの{what}を使った（作ったルートが終わっている。ADR 0031 決定 4-2）"
            )),
        );
        self.fail_if(dead, code);
    }

    /// 操作の結果の語を、型の値にする（`Lock` の取得は `Ok(LockGuard)`、壊れていれば `Err(LockBroken)`、
    /// `reserve` は `RootSlot` を作る）。
    fn post_block(&mut self, b: Builtin, r: Value, k: usize) -> Value {
        if b == Builtin::RootReserve {
            // スロットの塊: [0 集まりの番号][8 使用済み]。頭にスロットの印を立てる（破棄で上限を返す）。
            let g = self.ld(self.frame(), self.site_off(k, 1));
            let sl = self.blk(16);
            self.st(g, sl, 0);
            let z = self.ic(0);
            self.st(z, sl, 8);
            self.set_root_slot(sl);
            return sl;
        }
        if b == Builtin::LockAcquire && self.cg.task.as_ref().is_some_and(|t| t.roots) {
            // 壊れた Lock（-1）: Err(LockBroken)。
            let broken = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
            return self.branch(
                broken,
                |s| {
                    let lb = s.blk(8);
                    s.mk_enum_c(1, &[lb])
                },
                |s| s.guard_ok(r),
            );
        }
        if b == Builtin::LockAcquire {
            self.guard_ok(r)
        } else {
            r
        }
    }

    /// `Ok(LockGuard)`。守りの塊: [0 = 空（破棄で何も破棄しない）][8 = Lock のレコード]。破棄の知らせの印を頭に立てる。
    fn guard_ok(&mut self, r: Value) -> Value {
        let g = self.blk(16);
        // 先頭の語は明示的に 0 にする（再利用された塊の古い値を、破棄が T の値として解放しないため）。
        let z = self.ic(0);
        self.st(z, g, 0);
        self.st(r, g, 8);
        self.set_guard(g);
        self.mk_enum_c(0, &[g])
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
        // 範囲の本体が失敗した: 外側を巻き戻す前に、この範囲の子（子孫）を取り消す（評価器の Act::Resume の cancel_scope）。
        let sid_f = self.ld(self.frame(), aux1);
        self.cancel_loop(Some(sid_f));
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

    /// `task.roots(limit, f)`: ルートの集まりを作り、本体を子として呼び、戻ったら起動したルートの終わりを待つ
    /// （ADR 0031 決定 4-1）。本体が失敗・取り消されたら、起動したルートを起動の逆順に取り消す（決定 4-2 の (d)）。
    fn roots_site(
        &mut self,
        si: &SiteInfo,
        args: &[Operand],
        dest: &Place,
        dty: &Ty,
        next: usize,
        stop: usize,
    ) {
        let k = si.k;
        let (limit, _) = self.val(&args[0]);
        let lo = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, limit, 1);
        let hi = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThan, limit, 4096);
        let bad = self.b.ins().bor(lo, hi);
        let code = self.cg.code_for(
            stop,
            Some(StopKind::StdRequiresViolated),
            Some("task.roots: limit は 1 以上 4096 以下"),
        );
        self.fail_if(bad, code);
        let frame = self.frame();
        let (fv, _) = self.own(&args[1]);
        let gid = self.rt("sched_roots_begin", &[limit]);
        let aux1 = self.site_off(k, 1);
        let aux2 = self.site_off(k, 2);
        self.st(gid, frame, aux1);
        let gref = self.b.ins().iadd_imm_s(frame, aux1 as i64);
        let argv = self.zero_tmp(8);
        self.st(gref, argv, 0);
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
        let g = self.ld(self.frame(), aux1);
        self.cancel_loop_by(Some(g), "sched_group_cancel_next");
        self.jump_cleanup(code);
        self.b.switch_to_block(okb);
        let frame = self.frame();
        let g = self.ld(frame, aux1);
        let wait = self.rt("sched_roots_end", &[g]);
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
        // 起動したルートがすべて終わって起こされた。取り消しなら、残っているルートを取り消してから巻き戻す。
        let wblk = si.wait_blk.expect("roots の待ち");
        self.b.switch_to_block(wblk);
        let frame = self.frame();
        let can = self.cancelling();
        let (cb, rb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(can, cb, NOARGS, rb, NOARGS);
        self.b.switch_to_block(cb);
        let g = self.ld(frame, aux1);
        self.cancel_loop_by(Some(g), "sched_group_cancel_next");
        let frame = self.frame();
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
                if self.cg.task.as_ref().is_some_and(|t| t.roots) {
                    self.dead_check("sched_chan_dead", id, "通信路", stop);
                }
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
            RootStart => {
                let Some(start) = self.cg.task.as_ref().map(|t| t.fn_start) else {
                    return Some((self.unsupported("タスクを起こす関数が変換されていない"), u));
                };
                // スロット: 使用済みなら停止。集まりが閉じていれば停止。
                let (sl, slt) = self.own(&args[0]);
                let used = self.ld(sl, 8);
                let code = self.cg.code_for(
                    stop,
                    Some(StopKind::StdRequiresViolated),
                    Some("RootSlot.start: 使えないスロット"),
                );
                self.fail_if(used, code);
                let gid = self.ld(sl, 0);
                let closed = self.rt("sched_group_closed", &[gid]);
                let code = self.cg.code_for(
                    stop,
                    Some(StopKind::StdRequiresViolated),
                    Some("RootSlot.start: task.roots の範囲が終わっている"),
                );
                self.fail_if(closed, code);
                // スロットを消費する（以後の破棄では上限を返さない。上限はルートの終わりで返す）。
                let one = self.ic(1);
                self.st(one, sl, 8);
                self.drop_val(&slt, sl);
                let (conn, ct) = self.own(&args[1]);
                let (gv, _) = self.own(&args[2]);
                let (oa, oat) = self.own(&args[3]);
                let cd = self.drop_addr(&ct);
                let od = self.drop_addr(&oat);
                // 接続はルートの外（起動の記録の語 2）が持ち、g へはその語の番地（借用）を渡す。
                let rr = self.rt("sched_root_new", &[gid, conn, cd, oa, od]);
                let cref = self.b.ins().iadd_imm_s(rr, 16);
                let argv = self.zero_tmp(8);
                self.st(cref, argv, 0);
                let child = self.plan_fn(start, &[gv, argv, one, one])[0];
                let rd = self.ld(child, 48);
                self.rt("sched_root_launch", &[rr, child, rd]);
                (self.ic(0), u)
            }
            TaskAllOk | TaskRace => (
                self.unsupported(
                    "task.all_ok・task.race はネイティブ出力未対応（取り消しの意味を要する。ADR 0030 の 5-2g）",
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

    /// `sigil_frame_fp(枠) -> 指紋`: 枠の連鎖（状態番号と局所の語）の FNV 風の指紋。枠が指す先の中身は見ない。
    pub fn frame_fp_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let frame = self.b.block_params(entry)[0];
        let (outer, lhead, lbody, sbody, shead, step_done, fin) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        // outer(cur, h) / lhead(cur, h, i) / lbody(cur, h, i) / shead(cur, h, j, child) / sbody(cur, h, j, child) / step_done(h, child)
        self.b.append_block_param(outer, types::I64);
        self.b.append_block_param(outer, types::I64);
        for b in [lhead, lbody] {
            for _ in 0..3 {
                self.b.append_block_param(b, types::I64);
            }
        }
        for b in [shead, sbody] {
            for _ in 0..4 {
                self.b.append_block_param(b, types::I64);
            }
        }
        for _ in 0..2 {
            self.b.append_block_param(step_done, types::I64);
        }
        self.b.append_block_param(fin, types::I64);
        let h0 = self.ic(0xcbf29ce484222325u64 as i64);
        self.b
            .ins()
            .jump(outer, &[BlockArg::Value(frame), BlockArg::Value(h0)]);
        let mix = |s: &mut Self, h: Value, w: Value| {
            let x = s.b.ins().bxor(h, w);
            s.b.ins().imul_imm_s(x, 0x100000001b3)
        };

        self.b.switch_to_block(outer);
        let (cur, h) = (self.b.block_params(outer)[0], self.b.block_params(outer)[1]);
        let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, cur, 0);
        let go = self.b.create_block();
        self.b
            .ins()
            .brif(nz, go, NOARGS, fin, &[BlockArg::Value(h)]);
        self.b.switch_to_block(go);
        let cbw = self.ld(cur, 8);
        let opaque = self.b.ins().ushr_imm_s(cbw, 40);
        let (unk, known) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(opaque, unk, NOARGS, known, NOARGS);
        self.b.switch_to_block(unk);
        let zero = self.ic(0);
        self.b.ins().return_(&[zero]);
        self.b.switch_to_block(known);
        let state = self.ld(cur, 16);
        let h = mix(self, h, state);
        let i0 = self.ic(HDR as i64);
        self.b.ins().jump(
            lhead,
            &[
                BlockArg::Value(cur),
                BlockArg::Value(h),
                BlockArg::Value(i0),
            ],
        );

        self.b.switch_to_block(lhead);
        let ps = self.b.block_params(lhead).to_vec();
        let (cur, h, i) = (ps[0], ps[1], ps[2]);
        let cbv = self.ld(cur, 8);
        let c = self.b.ins().icmp(IntCC::SignedLessThan, i, cbv);
        let sstart = self.b.create_block();
        self.b.ins().brif(
            c,
            lbody,
            &[BlockArg::Value(cur), BlockArg::Value(h), BlockArg::Value(i)],
            sstart,
            NOARGS,
        );
        self.b.switch_to_block(lbody);
        let ps = self.b.block_params(lbody).to_vec();
        let (cur, h, i) = (ps[0], ps[1], ps[2]);
        let a = self.b.ins().iadd(cur, i);
        let w = self.ld(a, 0);
        let h = mix(self, h, w);
        let i2 = self.b.ins().iadd_imm_s(i, 8);
        self.b.ins().jump(
            lhead,
            &[
                BlockArg::Value(cur),
                BlockArg::Value(h),
                BlockArg::Value(i2),
            ],
        );

        // 子の枠の位置（局所の後ろ）を 3 語ずつ見て、0 でない最後の語を子とする。
        self.b.switch_to_block(sstart);
        // lhead から来るので cur・h は直前の値を引き直せない: 値は lhead のブロック引数。
        let ps = self.b.block_params(lhead).to_vec();
        let (cur, h) = (ps[0], ps[1]);
        let cbw = self.ld(cur, 8);
        let szw = self.ld(cur, 32);
        let nocb = self.b.ins().icmp_imm_s(IntCC::Equal, cbw, 0);
        let j0 = self.b.ins().select(nocb, szw, cbw);
        let z = self.ic(0);
        self.b.ins().jump(
            shead,
            &[
                BlockArg::Value(cur),
                BlockArg::Value(h),
                BlockArg::Value(j0),
                BlockArg::Value(z),
            ],
        );
        self.b.switch_to_block(shead);
        let ps = self.b.block_params(shead).to_vec();
        let (cur, h, j, child) = (ps[0], ps[1], ps[2], ps[3]);
        let sz = self.ld(cur, 32);
        let c = self.b.ins().icmp(IntCC::SignedLessThan, j, sz);
        self.b.ins().brif(
            c,
            sbody,
            &[
                BlockArg::Value(cur),
                BlockArg::Value(h),
                BlockArg::Value(j),
                BlockArg::Value(child),
            ],
            step_done,
            &[BlockArg::Value(h), BlockArg::Value(child)],
        );
        self.b.switch_to_block(sbody);
        let ps = self.b.block_params(sbody).to_vec();
        let (cur, h, j, child) = (ps[0], ps[1], ps[2], ps[3]);
        let a = self.b.ins().iadd(cur, j);
        let w = self.ld(a, 0);
        let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, w, 0);
        let child2 = self.b.ins().select(has, w, child);
        let j2 = self.b.ins().iadd_imm_s(j, 8 * SITE_W as i64);
        self.b.ins().jump(
            shead,
            &[
                BlockArg::Value(cur),
                BlockArg::Value(h),
                BlockArg::Value(j2),
                BlockArg::Value(child2),
            ],
        );
        self.b.switch_to_block(step_done);
        let ps = self.b.block_params(step_done).to_vec();
        self.b
            .ins()
            .jump(outer, &[BlockArg::Value(ps[1]), BlockArg::Value(ps[0])]);
        self.b.switch_to_block(fin);
        let h = self.b.block_params(fin)[0];
        self.b.ins().return_(&[h]);
    }

    /// 実行の割り当てに残った値（チャネルの項・`Lock` の値）を破棄し、記録を返す。
    pub(super) fn finish_sched(&mut self) {
        let drain = self.cg.task.as_ref().expect("計画").drain;
        self.rt("sched_collect_garbage", &[]);
        self.plan_fn(drain, &[]);
        self.rt("sched_release", &[]);
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

    /// 主スレッドが起動・合流・後始末を所有する。共有状態の解放前に全ワーカーを待つ。
    pub fn sched_run_body(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let root = self.b.block_params(entry)[0];
        let rd = self.ld(root, 48);
        self.rt("sched_start", &[root, rd]);
        self.rt("sched_workers_start", &[]);
        let worker = self.cg.task.as_ref().expect("計画").worker_run;
        self.plan_fn(worker, &[]);
        self.rt("sched_workers_join", &[]);
        let stopped = self.rt("sched_stopped", &[]);
        let (ok, fail) = (self.b.create_block(), self.b.create_block());
        let normal = self.b.ins().icmp_imm_s(IntCC::Equal, stopped, -1);
        self.b.ins().brif(normal, ok, NOARGS, fail, NOARGS);
        self.b.switch_to_block(ok);
        let res = self.rt("sched_root_result", &[]);
        self.finish_sched();
        let z = self.ic(0);
        self.b.ins().return_(&[res, z]);
        self.b.switch_to_block(fail);
        self.cancel_all_and_return(stopped);
    }

    /// 各ワーカーは同じ FIFO から取り出し、中断点まで枠を専有して実行する。
    pub fn worker_run_body(&mut self) {
        let entry = self.b.create_block();
        self.b.switch_to_block(entry);
        let (done, drain) = self.plan_ids();
        let (head, idle, run, deadlock) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);

        self.b.switch_to_block(head);
        let stopped = self.rt("sched_stopped", &[]);
        let (take, leave) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(stopped, leave, NOARGS, take, NOARGS);
        self.b.switch_to_block(leave);
        let z = self.ic(0);
        self.b.ins().return_(&[z, stopped]);
        self.b.switch_to_block(take);
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
        if self.cg.task.as_ref().is_some_and(|t| t.roots) {
            // 起動したルートが生きていれば、親ではなくそのルートを止める（ADR 0031 決定 4-2）。
            let rr = self.rt("sched_deadlock_root", &[]);
            let (iso, glob) = (self.b.create_block(), self.b.create_block());
            self.b.ins().brif(rr, iso, NOARGS, glob, NOARGS);
            self.b.switch_to_block(iso);
            self.isolate_root(rr, c);
            self.rt("sched_step_end", &[]);
            self.b.ins().jump(head, NOARGS);
            self.b.switch_to_block(glob);
        }
        self.rt("sched_fail", &[c]);
        let z = self.ic(0);
        self.b.ins().return_(&[z, c]);

        self.b.switch_to_block(run);
        let sig = self.sigref(1);
        let stepf = self.ld(f, 0);
        let inst = self.b.ins().call_indirect(sig, stepf, &[f]);
        let r = self.b.inst_results(inst).to_vec();
        let (susp, fin) = (self.b.create_block(), self.b.create_block());
        let is_susp = self.b.ins().icmp_imm_s(IntCC::Equal, r[1], SUSPEND);
        self.b.ins().brif(is_susp, susp, NOARGS, fin, NOARGS);

        self.b.switch_to_block(susp);
        // `task.yield` で中断したら、枠の指紋を記録して空転を調べる（眠りがあれば時計を進める）。
        let y = self.rt("sched_yielded", &[]);
        let (chk, cont) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(y, chk, NOARGS, cont, NOARGS);
        self.b.switch_to_block(chk);
        let fpf = self.cg.task.as_ref().expect("計画").frame_fp;
        let fp = self.plan_fn(fpf, &[f])[0];
        self.rt("sched_note_yield", &[fp]);
        self.b.ins().jump(cont, NOARGS);
        self.b.switch_to_block(cont);
        self.plan_fn(drain, &[]);
        self.rt("sched_step_end", &[]);
        self.b.ins().jump(head, NOARGS);

        self.b.switch_to_block(fin);
        let (fail, okb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(r[1], fail, NOARGS, okb, NOARGS);

        self.b.switch_to_block(okb);
        self.plan_fn(done, &[f]);
        self.rt("sched_finish_task", &[r[0]]);
        self.plan_fn(drain, &[]);
        self.rt("sched_step_end", &[]);
        self.b.ins().jump(head, NOARGS);

        self.b.switch_to_block(fail);
        self.plan_fn(done, &[f]);
        if self.cg.task.as_ref().is_some_and(|t| t.roots) {
            // 起動したルートの異常停止は、そのルートだけを止める（隔離。ADR 0031 決定 4-2）。
            let rr = self.rt("sched_root_abort", &[r[1]]);
            let (iso, glob) = (self.b.create_block(), self.b.create_block());
            self.b.ins().brif(rr, iso, NOARGS, glob, NOARGS);
            self.b.switch_to_block(iso);
            self.isolate_root(rr, r[1]);
            self.rt("sched_step_end", &[]);
            self.b.ins().jump(head, NOARGS);
            self.b.switch_to_block(glob);
        }
        self.rt("sched_fail", &[r[1]]);
        self.rt("sched_step_end", &[]);
        let z = self.ic(0);
        self.b.ins().return_(&[z, r[1]]);
    }

    /// 起動したルート（記録 `rr`）を止める: 停止を報告し、そのルートのタスクを作った順の逆に巻き戻し、作った通信路と
    /// `Lock` を捨てる。`g` のルートなら、接続を破棄せずに新しいルートで `on_abort(conn)` を起動する。
    fn isolate_root(&mut self, rr: Value, code: Value) {
        self.report_stop_soft(code);
        self.cancel_loop_by(Some(rr), "sched_root_cancel_next");
        let again = self.rt("sched_root_abort_end", &[rr]);
        let (re, end) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(again, re, NOARGS, end, NOARGS);
        self.b.switch_to_block(re);
        let start = self.cg.task.as_ref().expect("計画").fn_start;
        let oa = self.ld(again, 32);
        let conn = self.ld(again, 16);
        let argv = self.zero_tmp(8);
        self.st(conn, argv, 0);
        let one = self.ic(1);
        let child = self.plan_fn(start, &[oa, argv, one, one])[0];
        let rd = self.ld(child, 48);
        self.rt("sched_root_launch", &[again, child, rd]);
        self.b.ins().jump(end, NOARGS);
        self.b.switch_to_block(end);
        let (_, drain) = self.plan_ids();
        self.plan_fn(drain, &[]);
    }

    /// 残りのタスクを新しいものから順に取り消し、後始末をして、符号 `code` の停止を返す。
    fn cancel_all_and_return(&mut self, code: Value) {
        let (_, drain) = self.plan_ids();
        self.cancel_loop(None);
        self.rt("sched_collect_garbage", &[]);
        self.plan_fn(drain, &[]);
        self.rt("sched_release", &[]);
        let z = self.ic(0);
        self.b.ins().return_(&[z, code]);
    }

    /// 取り消しの旗を立てて、タスクを 1 つずつ取り消す（`scope` が `Some` ならその範囲の子孫だけ、なければ全体）。
    /// 取り消したタスクごとに、枠を片付けてから（持ち主の関数の値の破棄）ごみの列を空にする（未受領の起床の値は
    /// 局所・持ち主の後に破棄する。評価器の cancel_task の順序）。旗は元の値に戻す。
    fn cancel_loop(&mut self, scope: Option<Value>) {
        self.cancel_loop_by(scope, "sched_scope_cancel_next");
    }

    /// `cancel_loop` の取り消し順の選び方を指定する版（`task.timeout` は評価器と同じ部分木単位の `sched_tmo_cancel_next`）。
    fn cancel_loop_by(&mut self, scope: Option<Value>, next_fn: &'static str) {
        let (done, drain) = self.plan_ids();
        let base = self.region_base();
        let fa = self
            .b
            .ins()
            .iadd_imm_s(base, crate::codegen::TCB_CANCEL_OFF);
        let old = self.ld(fa, 0);
        let one = self.ic(1);
        self.st(one, fa, 0);
        let (head, body, end) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(head);
        let g = match scope {
            Some(sid) => self.rt(next_fn, &[sid]),
            None => self.rt("sched_cancel_next", &[]),
        };
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, g, 0);
        let (chk, only_drain) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(zero, end, NOARGS, chk, NOARGS);
        self.b.switch_to_block(chk);
        let neg = self.b.ins().icmp_imm_s(IntCC::Equal, g, -1);
        self.b.ins().brif(neg, only_drain, NOARGS, body, NOARGS);
        self.b.switch_to_block(only_drain);
        self.plan_fn(drain, &[]);
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(body);
        let sig = self.sigref(1);
        let stepf = self.ld(g, 0);
        self.b.ins().call_indirect(sig, stepf, &[g]);
        self.plan_fn(done, &[g]);
        self.plan_fn(drain, &[]);
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(end);
        let base = self.region_base();
        let fa = self
            .b
            .ins()
            .iadd_imm_s(base, crate::codegen::TCB_CANCEL_OFF);
        self.st(old, fa, 0);
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

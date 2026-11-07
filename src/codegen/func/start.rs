//! 初期スタック・専用スタックと終了処理（ADR 0025 C-4〜C-6）。
use super::*;
use cranelift_codegen::ir::TrapCode;
use cranelift_module::FuncId;

impl Fx<'_, '_> {
    fn start_head(&mut self) -> Vec<Value> {
        let b = self.b.create_block();
        self.b.append_block_params_for_function_params(b);
        self.b.switch_to_block(b);
        self.b.block_params(b).to_vec()
    }

    pub(crate) fn start_noop(&mut self) {
        self.start_head();
        self.b.ins().return_(&[]);
    }

    pub(crate) fn start_limit(&mut self, limit: DataId) {
        self.start_head();
        let p = self.data_addr(limit);
        let v = self.ld(p, 0);
        self.b.ins().return_(&[v]);
    }

    pub(crate) fn start_boot(&mut self, entry: FuncId, libc: FuncId) {
        let ps = self.start_head();
        let sp = ps[0];
        let argc = self.ld(sp, 0);
        let argv = self.b.ins().iadd_imm_s(sp, 8);
        let fr = self.cg.m.declare_func_in_func(entry, self.b.func);
        let addr = self.b.ins().func_addr(types::I64, fr);
        let libc = self.cg.m.declare_func_in_func(libc, self.b.func);
        let z = self.ic(0);
        self.b.ins().call(libc, &[addr, argc, argv, z, z, z, sp]);
        self.b.ins().trap(TrapCode::unwrap_user(1));
    }

    pub(crate) fn start_exit(&mut self, code: Value) {
        let n = self.ic(231);
        let z = self.ic(0);
        self.rt("syscall6", &[n, code, z, z, z, z, z]);
        self.b.ins().trap(TrapCode::unwrap_user(1));
    }

    pub(crate) fn start_stack(&mut self, limit: DataId, run: FuncId, switch: FuncId) {
        let ps = self.start_head();
        let argc = self.b.ins().uextend(types::I64, ps[0]);
        let argv = ps[1];
        let count = self.b.ins().iadd_imm_s(argc, 1);
        let bytes = self.b.ins().ishl_imm_s(count, 3);
        let envp = self.b.ins().iadd(argv, bytes);
        if let Some(g) = self
            .cg
            .p
            .globals
            .iter()
            .position(|g| g.name == "std.os.env.ENVP")
        {
            let p = self.data_addr(self.cg.gids[g]);
            self.st(envp, p, 0);
        }
        let a = self.zero_tmp(24);
        self.st(argc, a, 0);
        self.st(argv, a, 8);
        self.st(envp, a, 16);
        let z = self.ic(0);
        let n = self.ic(9);
        let size = self.ic(256 * 1024 * 1024 + 4096);
        let prot = self.ic(3);
        let flags = self.ic(0x4022);
        let minus = self.ic(-1);
        let base = self.rt("syscall6", &[n, z, size, prot, flags, minus, z]);
        let bad = self.b.create_block();
        let protect = self.b.create_block();
        let failed = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, base, -4095);
        self.b.ins().brif(failed, bad, NOARGS, protect, NOARGS);
        self.b.switch_to_block(bad);
        let code = self.ic(70);
        self.start_exit(code);
        self.b.switch_to_block(protect);
        let n = self.ic(10);
        let page = self.ic(4096);
        let r = self.rt("syscall6", &[n, base, page, z, z, z, z]);
        let failed = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, r, 0);
        let ready = self.b.create_block();
        self.b.ins().brif(failed, bad, NOARGS, ready, NOARGS);
        self.b.switch_to_block(ready);
        let top = self.b.ins().iadd(base, size);
        let low = self.b.ins().iadd_imm_s(top, -192 * 1024 * 1024);
        let lp = self.data_addr(limit);
        self.st(low, lp, 0);
        let fr = self.cg.m.declare_func_in_func(run, self.b.func);
        let addr = self.b.ins().func_addr(types::I64, fr);
        let fr = self.cg.m.declare_func_in_func(switch, self.b.func);
        self.b.ins().call(fr, &[top, addr, a]);
        self.b.ins().trap(TrapCode::unwrap_user(1));
    }

    pub(super) fn live_at_exit(&mut self, a: Value) {
        let Some(live) = self.cg.live_shim else {
            return;
        };
        let envp = self.ld(a, 16);
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        let check = self.b.create_block();
        let next = self.b.create_block();
        let found = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().jump(head, &[BlockArg::Value(envp)]);
        self.b.switch_to_block(head);
        let cursor = self.b.block_params(head)[0];
        let p = self.ld(cursor, 0);
        self.b.ins().brif(p, check, NOARGS, done, NOARGS);
        self.b.switch_to_block(check);
        // 完全な名前と '=' を比較する。値は空でもよく、UTF-8 でなくてもよい。
        for (offset, &byte) in b"SIGIL_RT_LIVE=".iter().enumerate() {
            let ch = self
                .b
                .ins()
                .load(types::I8, MemFlagsData::new(), p, offset as i32);
            let eq = self.b.ins().icmp_imm_s(IntCC::Equal, ch, byte as i64);
            let matched = self.b.create_block();
            self.b.ins().brif(eq, matched, NOARGS, next, NOARGS);
            self.b.switch_to_block(matched);
        }
        self.b.ins().jump(found, NOARGS);
        self.b.switch_to_block(next);
        let cursor2 = self.b.ins().iadd_imm_s(cursor, 8);
        self.b.ins().jump(head, &[BlockArg::Value(cursor2)]);
        self.b.switch_to_block(found);
        let fr = self.cg.m.declare_func_in_func(live, self.b.func);
        let call = self.b.ins().call(fr, &[]);
        let n = self.b.inst_results(call)[0];
        self.live_write(n);
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(done);
    }

    // 生存数の出力自体で確保しない。最大 20 桁の符号なし整数と改行をスタックで組み立てる。
    fn live_write(&mut self, n: Value) {
        let two = self.ic(2);
        let (prefix, len) = self.lit_ref(b"live: sigil_blocks=");
        self.scall(SFn::WriteAll, &[two, prefix, len]);
        let slot = self.tmp(32);
        let base = self.saddr(slot);
        let end = self.b.ins().iadd_imm_s(base, 31);
        let nl = self.ic(10);
        self.st8(nl, end);
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(end), BlockArg::Value(n)]);
        self.b.switch_to_block(head);
        let p = self.b.block_params(head)[0];
        let n = self.b.block_params(head)[1];
        let ten = self.ic(10);
        let d = self.b.ins().urem(n, ten);
        let rest = self.b.ins().udiv(n, ten);
        let p = self.b.ins().iadd_imm_s(p, -1);
        let d = self.b.ins().iadd_imm_s(d, 48);
        self.st8(d, p);
        self.b.ins().brif(
            rest,
            head,
            &[BlockArg::Value(p), BlockArg::Value(rest)],
            done,
            &[BlockArg::Value(p)],
        );
        self.b.switch_to_block(done);
        let p = self.b.block_params(done)[0];
        let end = self.b.ins().iadd_imm_s(end, 1);
        let len = self.b.ins().isub(end, p);
        self.scall(SFn::WriteAll, &[two, p, len]);
    }
}

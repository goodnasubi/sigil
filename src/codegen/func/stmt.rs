use super::{aggregate, norm, strip, Cur, Fx, NOARGS};
use crate::exec::StopKind;
use crate::ir::{ArithOp, Callee, Stmt, Term, Ty};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::InstBuilder;

impl<'a, 'p> Fx<'a, 'p> {
    pub(super) fn stmt(&mut self, s: &Stmt) {
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

    // --- 終端 ---

    pub(super) fn term(&mut self, t: &Term) {
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
                    ArithOp::Shl | ArithOp::Shr => {
                        let neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, y, 0);
                        let ge64 = self
                            .b
                            .ins()
                            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, y, 64);
                        let bad = self.b.ins().bor(neg, ge64);
                        self.fail_if(bad, code);
                        if *op == ArithOp::Shl {
                            self.b.ins().ishl(x, y)
                        } else {
                            // 論理右シフト (ushr)
                            self.b.ins().ushr(x, y)
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
}

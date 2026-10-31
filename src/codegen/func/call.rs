use super::{aggregate, strip, Fx, NOARGS};
use crate::exec::StopKind;
use crate::ir::{Builtin, Operand, Place, Ty};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;

impl<'a, 'p> Fx<'a, 'p> {
    pub(super) fn call_direct(&mut self, g: usize, args: &[Value]) -> Value {
        self.guard_stack();
        let fr = self.cg.m.declare_func_in_func(self.cg.fids[g], self.b.func);
        let inst = self.b.ins().call(fr, args);
        let r = self.b.inst_results(inst).to_vec();
        self.after_call(r[1]);
        r[0]
    }

    /// 関数の値（`[関数の番地][環境の語]` の塊）を呼ぶ。
    pub(super) fn call_fnref(&mut self, fv: Value, args: &[Value]) -> Value {
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

    pub(super) fn finish_call(&mut self, v: Value, rty: &Ty, dty: &Ty, dest: &Place, next: usize) {
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

    pub(super) fn builtin(&mut self, b: Builtin, args: &[Operand], stop: usize) -> (Value, Ty) {
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
            CtEq => {
                let (a, _) = self.val(&args[0]);
                let (b, _) = self.val(&args[1]);
                let eq = self.rt("bytes_ct_eq", &[a, b]);
                (self.b.ins().uextend(types::I64, eq), u)
            }
            StrAsBytes => {
                let (s, _) = self.val(&args[0]);
                (self.rt("str_as_bytes", &[s]), u)
            }
            IntRotl => {
                let (x, _) = self.val(&args[0]);
                let (n, _) = self.val(&args[1]);
                let (w, _) = self.val(&args[2]);
                (self.rt("int_rotl", &[x, n, w]), u)
            }
            IntRotr => {
                let (x, _) = self.val(&args[0]);
                let (n, _) = self.val(&args[1]);
                let (w, _) = self.val(&args[2]);
                (self.rt("int_rotr", &[x, n, w]), u)
            }
            IntRotr32 => {
                let (x, _) = self.val(&args[0]);
                let (n, _) = self.val(&args[1]);
                (self.rt("int_rotr32", &[x, n]), u)
            }
            IntAshr => {
                let (x, _) = self.val(&args[0]);
                let (n, _) = self.val(&args[1]);
                (self.b.ins().sshr(x, n), u)
            }
            BytesNew => (self.rt("bytes_new", &[]), u),
            BytesWithCapacity => {
                let (c, _) = self.val(&args[0]);
                (self.rt("bytes_with_capacity", &[c]), u)
            }
            BytesFromList => {
                let (l, _) = self.val(&args[0]);
                let out = self.zero_tmp(8);
                let err = self.zero_tmp(8);
                let ok = self.rt("bytes_from_list", &[l, out, err]);
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let res = s.ld(out, 0);
                        s.mk_enum_c(0, &[res])
                    },
                    |s| {
                        let e = s.ld(err, 0);
                        s.mk_enum_c(1, &[e])
                    },
                );
                (r, u)
            }
            BytesLen => {
                let (b, _) = self.val(&args[0]);
                (self.rt("bytes_len", &[b]), u)
            }
            BytesIsEmpty => {
                let (b, _) = self.val(&args[0]);
                let res = self.rt("bytes_is_empty", &[b]);
                (self.b.ins().uextend(types::I64, res), u)
            }
            BytesGet => {
                let (b, _) = self.val(&args[0]);
                let (i, _) = self.val(&args[1]);
                let out = self.zero_tmp(8);
                let ok = self.rt("bytes_get", &[b, i, out]);
                let isbad = self.b.ins().icmp_imm_s(IntCC::Equal, ok, 0);
                let tag = self.b2i(isbad);
                let good = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let n = self.ld(out, 0);
                let z = self.ic(0);
                let pay = self.b.ins().select(good, n, z);
                (self.mk_enum(tag, &[pay]), u)
            }
            BytesSet => {
                let (b, _) = self.val(&args[0]);
                let (i, _) = self.val(&args[1]);
                let (val, _) = self.val(&args[2]);
                let err = self.zero_tmp(8);
                let ok = self.rt("bytes_set", &[b, i, val, err]);
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let z = s.ic(0);
                        s.mk_enum_c(0, &[z])
                    },
                    |s| {
                        let e = s.ld(err, 0);
                        s.mk_enum_c(1, &[e])
                    },
                );
                (r, u)
            }
            BytesPush => {
                let (b, _) = self.val(&args[0]);
                let (val, _) = self.val(&args[1]);
                self.rt("bytes_push", &[b, val]);
                (self.ic(0), u)
            }
            BytesSlice => {
                let (b, _) = self.val(&args[0]);
                let (st, _) = self.val(&args[1]);
                let (en, _) = self.val(&args[2]);
                let out = self.zero_tmp(8);
                let err = self.zero_tmp(8);
                let ok = self.rt("bytes_slice", &[b, st, en, out, err]);
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let res = s.ld(out, 0);
                        s.mk_enum_c(0, &[res])
                    },
                    |s| {
                        let e = s.ld(err, 0);
                        s.mk_enum_c(1, &[e])
                    },
                );
                (r, u)
            }
            BytesConcat => {
                let (b1, _) = self.val(&args[0]);
                let (b2, _) = self.val(&args[1]);
                (self.rt("bytes_concat", &[b1, b2]), u)
            }
            BytesExtend => {
                let (b1, _) = self.val(&args[0]);
                let (b2, _) = self.val(&args[1]);
                self.rt("bytes_extend", &[b1, b2]);
                (self.ic(0), u)
            }
            BytesToStringUtf8 => {
                let (b, _) = self.val(&args[0]);
                let out = self.zero_tmp(8);
                let err = self.zero_tmp(8);
                let ok = self.rt("bytes_to_string_utf8", &[b, out, err]);
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let r = self.branch(
                    c,
                    |s| {
                        let res = s.ld(out, 0);
                        s.mk_enum_c(0, &[res])
                    },
                    |s| {
                        let e = s.ld(err, 0);
                        s.mk_enum_c(1, &[e])
                    },
                );
                (r, u)
            }
            BytesPushU16Be => {
                let (b, _) = self.val(&args[0]);
                let (v, _) = self.val(&args[1]);
                self.rt("bytes_push_u16_be", &[b, v]);
                (self.ic(0), u)
            }
            BytesPushU32Be => {
                let (b, _) = self.val(&args[0]);
                let (v, _) = self.val(&args[1]);
                self.rt("bytes_push_u32_be", &[b, v]);
                (self.ic(0), u)
            }
            BytesPushU64Be => {
                let (b, _) = self.val(&args[0]);
                let (v, _) = self.val(&args[1]);
                self.rt("bytes_push_u64_be", &[b, v]);
                (self.ic(0), u)
            }
            BytesPushU32Le => {
                let (b, _) = self.val(&args[0]);
                let (v, _) = self.val(&args[1]);
                self.rt("bytes_push_u32_le", &[b, v]);
                (self.ic(0), u)
            }
            BytesReadU32Be => {
                let (b, _) = self.val(&args[0]);
                let (o, _) = self.val(&args[1]);
                let out = self.zero_tmp(8);
                let ok = self.rt("bytes_read_u32_be", &[b, o, out]);
                let isbad = self.b.ins().icmp_imm_s(IntCC::Equal, ok, 0);
                let tag = self.b2i(isbad);
                let good = self.b.ins().icmp_imm_s(IntCC::NotEqual, ok, 0);
                let n = self.ld(out, 0);
                let z = self.ic(0);
                let pay = self.b.ins().select(good, n, z);
                (self.mk_enum(tag, &[pay]), u)
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
}

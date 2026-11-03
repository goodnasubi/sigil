//! 構造的な等しさ・順序の生成（ADR 0025 補足 G-1）。
//!
//! 型 `T` ごとに `sigil_eq_<k>(a, b) -> Int`（1 なら等しい）と `sigil_cmp_<k>(a, b) -> Int`（-1・0・1。比べられなければ 2）を
//! 生成する。C の呼び出し規約で、Sigil の `(値, 停止の符号)` の形ではない。規則は評価器と同じ（Rust ランタイムの `eq`・`cmp` を
//! 置き換えたもの。ただし `Addr` は番地の値で比べる。G-10）。スカラー・文字列・バイト列は関数を作らずその場で出す。

use super::{Fx, NOARGS};
use crate::codegen::norm_with;
use crate::ir::{Ty, TypeKind};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{types, Block as CBlock, BlockArg, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;

impl<'a, 'p> Fx<'a, 'p> {
    /// 型 `x`・`y` の値の等しさ（0/1 の語）。
    pub(crate) fn eq_val(&mut self, ty: &Ty, x: Value, y: Value) -> Value {
        let ty = norm_with(ty, true);
        match &ty {
            Ty::Unit | Ty::ParseError => self.ic(1),
            Ty::Int | Ty::Bool | Ty::Addr => {
                let c = self.b.ins().icmp(IntCC::Equal, x, y);
                self.b2i(c)
            }
            Ty::Float => {
                let fx = self.b.ins().bitcast(types::F64, MemFlagsData::new(), x);
                let fy = self.b.ins().bitcast(types::F64, MemFlagsData::new(), y);
                let c = self.b.ins().fcmp(FloatCC::Equal, fx, fy);
                self.b2i(c)
            }
            Ty::Str | Ty::Path => self.rt("str_eq", &[x, y]),
            Ty::Bytes => self.rt("bytes_eq", &[x, y]),
            Ty::Ref(i) | Ty::MutRef(i) => {
                let (x2, y2) = (self.ld(x, 0), self.ld(y, 0));
                self.eq_val(i, x2, y2)
            }
            Ty::FnRef(..) => self.ic(0),
            Ty::List(_) | Ty::Tuple(_) | Ty::Adt(_) | Ty::Shared(_) | Ty::Cell(_) => {
                self.call_eqcmp(false, &ty, x, y)
            }
        }
    }

    /// 型 `x`・`y` の値の順序（-1・0・1。比べられなければ 2）。
    pub(crate) fn cmp_val(&mut self, ty: &Ty, x: Value, y: Value) -> Value {
        let ty = norm_with(ty, true);
        match &ty {
            Ty::Int | Ty::Bool | Ty::Addr => {
                let lt = self.b.ins().icmp(IntCC::SignedLessThan, x, y);
                let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, x, y);
                let (lt, gt) = (self.b2i(lt), self.b2i(gt));
                self.b.ins().isub(gt, lt)
            }
            Ty::Float => {
                let fx = self.b.ins().bitcast(types::F64, MemFlagsData::new(), x);
                let fy = self.b.ins().bitcast(types::F64, MemFlagsData::new(), y);
                let lt = self.b.ins().fcmp(FloatCC::LessThan, fx, fy);
                let gt = self.b.ins().fcmp(FloatCC::GreaterThan, fx, fy);
                let uno = self.b.ins().fcmp(FloatCC::Unordered, fx, fy);
                let (lt, gt, uno) = (self.b2i(lt), self.b2i(gt), self.b2i(uno));
                // 非数なら lt = gt = 0 なので 0 + 2。
                let r = self.b.ins().isub(gt, lt);
                let u2 = self.b.ins().iadd(uno, uno);
                self.b.ins().iadd(r, u2)
            }
            Ty::Str => self.rt("str_cmp", &[x, y]),
            Ty::Bytes => self.rt("bytes_cmp", &[x, y]),
            Ty::Ref(i) | Ty::MutRef(i) => {
                let (x2, y2) = (self.ld(x, 0), self.ld(y, 0));
                self.cmp_val(i, x2, y2)
            }
            Ty::List(_) | Ty::Tuple(_) => self.call_eqcmp(true, &ty, x, y),
            // 順序を持たない型（Path・構造体・列挙型・Shared・Cell・関数・Unit・ParseError）
            _ => self.ic(2),
        }
    }

    fn call_eqcmp(&mut self, is_cmp: bool, ty: &Ty, x: Value, y: Value) -> Value {
        let id = match self.cg.eqcmp_fn(is_cmp, ty) {
            Ok(id) => id,
            Err(e) => return self.fail(e),
        };
        let fr = self.cg.m.declare_func_in_func(id, self.b.func);
        let inst = self.b.ins().call(fr, &[x, y]);
        self.b.inst_results(inst)[0]
    }

    /// `done`（引数 1 つの合流点）へ `v` を渡して抜ける。
    fn leave(&mut self, done: CBlock, v: Value) {
        self.b.ins().jump(done, &[BlockArg::Value(v)]);
    }

    /// `c` が真なら `done` へ `v` を渡して抜け、偽なら続きの新しいブロックへ進む。
    fn leave_if(&mut self, c: Value, done: CBlock, v: Value) {
        let next = self.b.create_block();
        self.b
            .ins()
            .brif(c, done, &[BlockArg::Value(v)], next, NOARGS);
        self.b.switch_to_block(next);
    }

    /// 語の並び（フィールド `i` は `base + 8*i`）を順に比べる。等しさは食い違いで 0、順序は 0 でない結果（2 を含む）で抜ける。
    fn eqcmp_fields(
        &mut self,
        is_cmp: bool,
        tys: &[Ty],
        (a, b): (Value, Value),
        base: i32,
        done: CBlock,
    ) {
        for (i, t) in tys.iter().enumerate() {
            let off = base + 8 * i as i32;
            let (x, y) = (self.ld(a, off), self.ld(b, off));
            if is_cmp {
                let r = self.cmp_val(t, x, y);
                let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, r, 0);
                self.leave_if(c, done, r);
            } else {
                let r = self.eq_val(t, x, y);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, r, 0);
                let z = self.ic(0);
                self.leave_if(c, done, z);
            }
        }
    }

    /// `sigil_eq_<k>`・`sigil_cmp_<k>` の本体。`ty` は `norm_with(.., true)` した型。
    pub(crate) fn eqcmp_body(&mut self, is_cmp: bool, ty: &Ty) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        let (a, b) = (ps[0], ps[1]);
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        // 全部の道が `done` へ値を渡して終わる。通り抜けたときの値（等しさは 1、順序は 0）。
        let fall = if is_cmp { 0 } else { 1 };
        let p = self.cg.p;
        match ty {
            Ty::Tuple(ts) => {
                let ts: Vec<Ty> = ts.iter().map(|t| norm_with(t, true)).collect();
                self.eqcmp_fields(is_cmp, &ts, (a, b), 0, done);
                let v = self.ic(fall);
                self.leave(done, v);
            }
            Ty::Adt(t) => match &p.types[*t].kind {
                TypeKind::Struct { fields } if !is_cmp => {
                    let ts: Vec<Ty> = fields.iter().map(|(_, t)| norm_with(t, true)).collect();
                    self.eqcmp_fields(false, &ts, (a, b), 0, done);
                    let v = self.ic(1);
                    self.leave(done, v);
                }
                TypeKind::Enum { variants } if !is_cmp => {
                    let (ta, tb) = (self.ld(a, 0), self.ld(b, 0));
                    let ne = self.b.ins().icmp(IntCC::NotEqual, ta, tb);
                    let z = self.ic(0);
                    self.leave_if(ne, done, z);
                    for (vi, v) in variants.iter().enumerate() {
                        if v.fields.is_empty() {
                            continue;
                        }
                        let (arm, nextv) = (self.b.create_block(), self.b.create_block());
                        let is = self.b.ins().icmp_imm_s(IntCC::Equal, ta, vi as i64);
                        self.b.ins().brif(is, arm, NOARGS, nextv, NOARGS);
                        self.b.switch_to_block(arm);
                        let ts: Vec<Ty> = v.fields.iter().map(|t| norm_with(t, true)).collect();
                        self.eqcmp_fields(false, &ts, (a, b), 8, done);
                        let one = self.ic(1);
                        self.leave(done, one);
                        self.b.switch_to_block(nextv);
                    }
                    let v = self.ic(1);
                    self.leave(done, v);
                }
                _ => {
                    let v = self.ic(if is_cmp { 2 } else { 0 });
                    self.leave(done, v);
                }
            },
            Ty::List(e) => {
                let e = norm_with(e, true);
                self.eqcmp_list(is_cmp, &e, (a, b), done);
            }
            Ty::Shared(i) | Ty::Cell(i) if !is_cmp => {
                let same = self.b.ins().icmp(IntCC::Equal, a, b);
                let one = self.ic(1);
                self.leave_if(same, done, one);
                let (x, y) = (self.ld(a, 0), self.ld(b, 0));
                let r = self.eq_val(i, x, y);
                self.leave(done, r);
            }
            _ => {
                let v = self.ic(if is_cmp { 2 } else { 0 });
                self.leave(done, v);
            }
        }
        self.b.switch_to_block(done);
        let r = self.b.block_params(done)[0];
        self.b.ins().return_(&[r]);
    }

    /// 一覧: 等しさは長さが同じで要素が全部等しい。順序は短いほうの長さまで要素を順に比べ（0 でない結果で抜ける）、
    /// 全部 0 なら長さを比べる。
    fn eqcmp_list(&mut self, is_cmp: bool, e: &Ty, (a, b): (Value, Value), done: CBlock) {
        let (la, lb) = (self.rt("list_len", &[a]), self.rt("list_len", &[b]));
        let (head, body, cont) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        for blk in [head, body, cont] {
            self.b.append_block_param(blk, types::I64);
        }
        let zero = self.ic(0);
        let (limit, exit, exit_arg) = if is_cmp {
            let tail = self.b.create_block();
            let lt = self.b.ins().icmp(IntCC::SignedLessThan, lb, la);
            let m = self.b.ins().select(lt, lb, la);
            (m, tail, None)
        } else {
            let ne = self.b.ins().icmp(IntCC::NotEqual, la, lb);
            self.leave_if(ne, done, zero);
            let one = self.ic(1);
            (la, done, Some(one))
        };
        self.b.ins().jump(head, &[BlockArg::Value(zero)]);

        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, limit);
        match exit_arg {
            Some(one) => self.b.ins().brif(
                more,
                body,
                &[BlockArg::Value(i)],
                exit,
                &[BlockArg::Value(one)],
            ),
            None => self
                .b
                .ins()
                .brif(more, body, &[BlockArg::Value(i)], exit, NOARGS),
        };

        self.b.switch_to_block(body);
        let i = self.b.block_params(body)[0];
        let (pa, pb) = (self.rt("list_get", &[a, i]), self.rt("list_get", &[b, i]));
        let (x, y) = (self.ld(pa, 0), self.ld(pb, 0));
        let (r, stop) = if is_cmp {
            let r = self.cmp_val(e, x, y);
            let c = self.b.ins().icmp_imm_s(IntCC::NotEqual, r, 0);
            (r, c)
        } else {
            let r = self.eq_val(e, x, y);
            let c = self.b.ins().icmp_imm_s(IntCC::Equal, r, 0);
            (zero, c)
        };
        self.b.ins().brif(
            stop,
            done,
            &[BlockArg::Value(r)],
            cont,
            &[BlockArg::Value(i)],
        );

        self.b.switch_to_block(cont);
        let i = self.b.block_params(cont)[0];
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(head, &[BlockArg::Value(next)]);

        if is_cmp {
            self.b.switch_to_block(exit);
            let lt = self.b.ins().icmp(IntCC::SignedLessThan, la, lb);
            let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, la, lb);
            let (lt, gt) = (self.b2i(lt), self.b2i(gt));
            let r = self.b.ins().isub(gt, lt);
            self.leave(done, r);
        }
    }
}

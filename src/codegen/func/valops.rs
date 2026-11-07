//! 値の複写・破棄・塊の確保の生成（ADR 0025 補足 G-1・G-2・G-3・G-4）。
//!
//! 型 `T` ごとに `sigil_clone_<k>`・`sigil_shadow_<k>`（複写）、`sigil_drop_<k>`（破棄の入口）、`sigil_dstep_<k>`
//! （破棄の 1 段）を生成する。規則は、3-5e-ii より前の Rust ランタイム（`val.rs` の `dup`・`sigil_rt_val_drop`）と同じ。
//! 破棄は作業の列（項 `(段の関数の番地, 語)` の積み）で反復し、再帰しない。作業の列は静的な領域から始まり、
//! 足りなければ `raw_alloc`（Sigil 製の確保器）で 2 倍の領域に替える。
//!
//! 塊は `raw_alloc(16 + n, 8)` で取る。頭は `[n | SILENT][参照の数]` で、中身の番地（頭 + 16）が値の語になる。

use super::{aggregate, strip, Fx, NOARGS};
use crate::codegen::{norm_with, shim_sig, Gk, SFn, WS_INLINE_ITEMS};
use crate::ir::{Ty, TypeKind};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, Block as CBlock, BlockArg, InstBuilder, Value};
use cranelift_module::Module;

/// 塊の頭の第 1 語に立つ「黙った」印（補足 G-4。`val.rs` の `SILENT` と同じ）。
const SILENT: i64 = 1 << 62;
/// 解放した塊の参照の数の場所に書く印（`val.rs` の `FREED` と同じ）。
const FREED: i64 = 0x5afe_dead_0000_0001;
/// 二重解放の停止の報告（補足 G-4）。
const STOP_FREED: &str = "解放済みの塊をもう一度解放した";

/// 型の鍵（補足 G-1）: 参照を外し、関数の型の中身を消して正規化する。
fn key(t: &Ty) -> Ty {
    norm_with(&strip(t), true)
}

impl<'a, 'p> Fx<'a, 'p> {
    /// 関数 `sigil_<種類>_<k>` の本体の入口。ブロックを作って切り替え、先頭の引数の語を返す。
    fn body_entry(&mut self) -> (Value, Vec<Value>) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        let ps = self.b.block_params(entry).to_vec();
        (ps[0], ps)
    }

    /// 生成の関数の本体（`Gk` ごと）。
    pub(crate) fn gen_body(&mut self, k: Gk, ty: &Ty) {
        match k {
            Gk::Eq => self.eqcmp_body(false, ty),
            Gk::Cmp => self.eqcmp_body(true, ty),
            Gk::Clone => self.copy_body(true, ty),
            Gk::Shadow => self.copy_body(false, ty),
            Gk::Drop => self.drop_body(ty),
            Gk::Dstep => self.dstep_body(ty),
            Gk::Show => self.show_typed_body(ty),
            Gk::Coerce => self.coerce_body(ty),
        }
    }

    // -----------------------------------------------------------------------
    // 呼び出し側の口（`shadow`・`clone`・`drop`・`blk` は `drop.rs` から使う）

    /// 型 `t` の値 `x` の複写（`deep` なら `clone`、偽なら影）。スカラー・参照は値のまま。
    pub(super) fn copy_val(&mut self, t: &Ty, x: Value, deep: bool) -> Value {
        if !aggregate(t) {
            return x;
        }
        self.gen_call(if deep { Gk::Clone } else { Gk::Shadow }, t, &[x])
    }

    /// 型 `t` の生成の関数を呼ぶ。戻りがなければ 0。
    fn gen_call(&mut self, k: Gk, t: &Ty, args: &[Value]) -> Value {
        let id = match self.cg.gen_fn(k, &key(t)) {
            Ok(id) => id,
            Err(e) => return self.fail(e),
        };
        let fr = self.cg.m.declare_func_in_func(id, self.b.func);
        let inst = self.b.ins().call(fr, args);
        match self.b.inst_results(inst).first().copied() {
            Some(v) => v,
            None => self.ic(0),
        }
    }

    /// 型 `t` の破棄の段の関数の番地（作業の列に積むため）。
    pub(super) fn dstep_addr(&mut self, t: &Ty) -> Value {
        match self.cg.gen_fn(Gk::Dstep, &key(t)) {
            Ok(id) => {
                let fr = self.cg.m.declare_func_in_func(id, self.b.func);
                self.b.ins().func_addr(types::I64, fr)
            }
            Err(e) => self.fail(e),
        }
    }

    /// 見出しつきの塊を作る（補足 G-4）。`bytes` は 8 の倍数に切り上げ、頭に `[n][参照の数 1]` を書く。戻りは中身の番地。
    pub(super) fn blk(&mut self, bytes: i64) -> Value {
        if !self.cg.rt.contains_key("raw_alloc") {
            return self.unsupported("確保器 std.os.alloc が結ばれていない");
        }
        let n = (bytes.max(8) + 7) / 8 * 8;
        let total = self.ic(16 + n);
        let eight = self.ic(8);
        let h = self.rt("raw_alloc", &[total, eight]);
        let nv = self.ic(n);
        self.st(nv, h, 0);
        let one = self.ic(1);
        self.st(one, h, 8);
        self.b.ins().iadd_imm_s(h, 16)
    }

    /// 参照の数を 1 増やす（中身の番地 `w` の前の語）。
    fn rc_inc(&mut self, w: Value) {
        let r = self.b.ins().iadd_imm_s(w, -8);
        let c = self.ld(r, 0);
        let c1 = self.b.ins().iadd_imm_s(c, 1);
        self.st(c1, r, 0);
    }

    /// 参照の数を 1 減らし、新しい値を返す。
    fn rc_dec(&mut self, w: Value) -> Value {
        let r = self.b.ins().iadd_imm_s(w, -8);
        let c = self.ld(r, 0);
        let c1 = self.b.ins().iadd_imm_s(c, -1);
        self.st(c1, r, 0);
        c1
    }

    /// 塊 `w` の頭に黙った印を立てる。
    fn set_silent(&mut self, w: Value) {
        let h = self.b.ins().iadd_imm_s(w, -16);
        let hw = self.ld(h, 0);
        let s = self.b.ins().bor_imm_s(hw, SILENT);
        self.st(s, h, 0);
    }

    // -----------------------------------------------------------------------
    // 複写（`sigil_clone_<k>`・`sigil_shadow_<k>`）。今の `dup` と同じ規則（補足 G-1 の表）。

    fn copy_body(&mut self, deep: bool, t: &Ty) {
        let (w, _) = self.body_entry();
        let (body, done) = (self.b.create_block(), self.b.create_block());
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, w, 0);
        self.b.ins().brif(zero, done, NOARGS, body, NOARGS);

        // 0 は移動済みなので、そのまま返す。
        self.b.switch_to_block(done);
        self.b.ins().return_(&[w]);

        self.b.switch_to_block(body);
        let p = self.cg.p;
        match t {
            Ty::Str | Ty::Path => {
                let r = self.rt("str_clone", &[w]);
                self.b.ins().return_(&[r]);
            }
            Ty::Bytes => {
                let r = self.rt("bytes_clone", &[w]);
                self.b.ins().return_(&[r]);
            }
            Ty::FnRef(..) => {
                // 環境を持つ関数の値（塊）だけが参照の数を持つ。環境のない値は静的な 16 バイトの並びで、数を持たない。
                let env = self.ld(w, 8);
                let (inc, out) = (self.b.create_block(), self.b.create_block());
                let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, env, 0);
                self.b.ins().brif(has, inc, NOARGS, out, NOARGS);
                self.b.switch_to_block(inc);
                self.rc_inc(w);
                self.b.ins().jump(out, NOARGS);
                self.b.switch_to_block(out);
                self.b.ins().return_(&[w]);
            }
            Ty::Shared(_) => {
                self.rc_inc(w);
                self.b.ins().return_(&[w]);
            }
            Ty::Cell(i) => {
                if deep {
                    let nb = self.blk(8);
                    let x = self.ld(w, 0);
                    let c = self.copy_val(i, x, true);
                    self.st(c, nb, 0);
                    self.b.ins().return_(&[nb]);
                } else {
                    self.rc_inc(w);
                    self.b.ins().return_(&[w]);
                }
            }
            Ty::Tuple(ts) => {
                let nb = self.blk(8 * ts.len() as i64);
                for (i, ct) in ts.iter().enumerate() {
                    let x = self.ld(w, 8 * i as i32);
                    let c = self.copy_val(ct, x, deep);
                    self.st(c, nb, 8 * i as i32);
                }
                self.b.ins().return_(&[nb]);
            }
            Ty::Adt(id) => match &p.types[*id].kind {
                TypeKind::Struct { fields } => {
                    let nb = self.blk(8 * fields.len() as i64);
                    for (i, (_, ft)) in fields.iter().enumerate() {
                        let x = self.ld(w, 8 * i as i32);
                        let c = self.copy_val(ft, x, deep);
                        self.st(c, nb, 8 * i as i32);
                    }
                    if !deep {
                        self.set_silent(nb);
                    }
                    self.b.ins().return_(&[nb]);
                }
                TypeKind::Enum { variants } => {
                    if variants.is_empty() {
                        self.b.ins().return_(&[w]);
                        return;
                    }
                    let tag = self.ld(w, 0);
                    // 腕ごとに、タグが一致すれば複写する。最後の腕は残り全部。
                    let n = variants.len();
                    let arms: Vec<CBlock> = (0..n).map(|_| self.b.create_block()).collect();
                    for (vi, &arm) in arms.iter().enumerate().take(n - 1) {
                        let next = self.b.create_block();
                        let is = self.b.ins().icmp_imm_s(IntCC::Equal, tag, vi as i64);
                        self.b.ins().brif(is, arm, NOARGS, next, NOARGS);
                        self.b.switch_to_block(next);
                    }
                    self.b.ins().jump(arms[n - 1], NOARGS);
                    for (vi, v) in variants.iter().enumerate() {
                        self.b.switch_to_block(arms[vi]);
                        let nf = v.fields.len();
                        let nb = self.blk(8 * (1 + nf).max(2) as i64);
                        self.st(tag, nb, 0);
                        for (i, ft) in v.fields.iter().enumerate() {
                            let x = self.ld(w, 8 + 8 * i as i32);
                            let c = self.copy_val(ft, x, deep);
                            self.st(c, nb, 8 + 8 * i as i32);
                        }
                        if !deep {
                            self.set_silent(nb);
                        }
                        self.b.ins().return_(&[nb]);
                    }
                }
            },
            Ty::List(e) => {
                let eight = self.ic(8);
                let nl = self.rt("list_new", &[eight]);
                let len = self.rt("list_len", &[w]);
                let (head, bodyb, exit) = (
                    self.b.create_block(),
                    self.b.create_block(),
                    self.b.create_block(),
                );
                for blk in [head, bodyb] {
                    self.b.append_block_param(blk, types::I64);
                }
                let zero = self.ic(0);
                self.b.ins().jump(head, &[BlockArg::Value(zero)]);
                self.b.switch_to_block(head);
                let i = self.b.block_params(head)[0];
                let more = self.b.ins().icmp(IntCC::SignedLessThan, i, len);
                self.b
                    .ins()
                    .brif(more, bodyb, &[BlockArg::Value(i)], exit, NOARGS);
                self.b.switch_to_block(bodyb);
                let i = self.b.block_params(bodyb)[0];
                let slot = self.rt("list_get", &[w, i]);
                let x = self.ld(slot, 0);
                let c = self.copy_val(e, x, deep);
                // `list_push` は要素の語を指す番地を受け取る（補足 E-3）。一時の領域に書いて渡す。
                let slot = self.zero_tmp(8);
                self.st(c, slot, 0);
                self.rt("list_push", &[nl, slot]);
                let next = self.b.ins().iadd_imm_s(i, 1);
                self.b.ins().jump(head, &[BlockArg::Value(next)]);
                self.b.switch_to_block(exit);
                self.b.ins().return_(&[nl]);
            }
            _ => {
                self.b.ins().return_(&[w]);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 破棄（`sigil_drop_<k>`・`sigil_dstep_<k>`）。補足 G-2。

    /// 作業の列の状態の番地。語: `[0]` 項の数、`[8]` 確保した領域（0 なら静的な領域）、`[16]` その項の数、`[24..]` 静的な領域。
    fn ws_base(&mut self) -> Value {
        match self.cg.ws_data() {
            Ok(d) => self.data_addr(d),
            Err(e) => self.fail(e),
        }
    }

    /// 今の領域の番地と項の数（確保した領域がなければ静的な領域。項の数は `WS_INLINE_ITEMS`）。
    fn ws_area(&mut self, ws: Value) -> (Value, Value) {
        let raw = self.ld(ws, 8);
        let inl = self.b.ins().iadd_imm_s(ws, 24);
        let rawcap = self.ld(ws, 16);
        let none = self.b.ins().icmp_imm_s(IntCC::Equal, raw, 0);
        let buf = self.b.ins().select(none, inl, raw);
        let inline_cap = self.ic(WS_INLINE_ITEMS as i64);
        let cap = self.b.ins().select(none, inline_cap, rawcap);
        (buf, cap)
    }

    /// 項 `(f, x)` を作業の列に積む。一杯なら 2 倍の領域を確保して写す（確保の失敗は確保器が止める）。
    fn ws_push(&mut self, f: Value, x: Value) {
        let ws = self.ws_base();
        let top = self.ld(ws, 0);
        let (buf, cap) = self.ws_area(ws);
        let full = self
            .b
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, top, cap);
        let (grow, store) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(full, grow, NOARGS, store, NOARGS);

        self.b.switch_to_block(grow);
        let ncap = self.b.ins().ishl_imm_s(cap, 1);
        let nbytes = self.b.ins().imul_imm_s(ncap, 16);
        let eight = self.ic(8);
        let nbuf = self.rt("raw_alloc", &[nbytes, eight]);
        let used = self.b.ins().imul_imm_s(top, 16);
        self.rt("memmove", &[nbuf, buf, used]);
        let raw = self.ld(ws, 8);
        let (free_old, keep) = (self.b.create_block(), self.b.create_block());
        let has_raw = self.b.ins().icmp_imm_s(IntCC::NotEqual, raw, 0);
        self.b.ins().brif(has_raw, free_old, NOARGS, keep, NOARGS);
        self.b.switch_to_block(free_old);
        let oldbytes = self.b.ins().imul_imm_s(cap, 16);
        self.rt("raw_free", &[raw, oldbytes, eight]);
        self.b.ins().jump(keep, NOARGS);
        self.b.switch_to_block(keep);
        self.st(nbuf, ws, 8);
        self.st(ncap, ws, 16);
        self.b.ins().jump(store, NOARGS);

        self.b.switch_to_block(store);
        let (buf2, _) = self.ws_area(ws);
        let off = self.b.ins().imul_imm_s(top, 16);
        let slot = self.b.ins().iadd(buf2, off);
        self.st(f, slot, 0);
        self.st(x, slot, 8);
        let t1 = self.b.ins().iadd_imm_s(top, 1);
        self.st(t1, ws, 0);
    }

    /// 項を 1 つ取り出す（空でないことは呼ぶ側が確かめる）。戻りは `(段の関数の番地, 語)`。
    fn ws_pop(&mut self) -> (Value, Value) {
        let ws = self.ws_base();
        let top = self.ld(ws, 0);
        let t1 = self.b.ins().iadd_imm_s(top, -1);
        self.st(t1, ws, 0);
        let (buf, _) = self.ws_area(ws);
        let off = self.b.ins().imul_imm_s(t1, 16);
        let slot = self.b.ins().iadd(buf, off);
        let f = self.ld(slot, 0);
        let x = self.ld(slot, 8);
        (f, x)
    }

    /// 子 `x`（型 `t`）を作業の列に積む。0（移動済み）と、破棄の対象でない型は積まない。
    fn push_child(&mut self, t: &Ty, x: Value) {
        if !aggregate(t) {
            return;
        }
        let (go, skip) = (self.b.create_block(), self.b.create_block());
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, x, 0);
        self.b.ins().brif(zero, skip, NOARGS, go, NOARGS);
        self.b.switch_to_block(go);
        let f = self.dstep_addr(t);
        self.ws_push(f, x);
        self.b.ins().jump(skip, NOARGS);
        self.b.switch_to_block(skip);
    }

    /// 塊 `w` を解放する（補足 G-4）。参照の数の場所が解放済みの印なら停止し（終了コード 70）、
    /// 違えば印を書いて、確保器へ返す。
    fn blk_free(&mut self, w: Value) {
        let h = self.b.ins().iadd_imm_s(w, -16);
        let flag = self.ld(h, 8);
        let (bad, ok) = (self.b.create_block(), self.b.create_block());
        let twice = self.b.ins().icmp_imm_s(IntCC::Equal, flag, FREED);
        self.b.ins().brif(twice, bad, NOARGS, ok, NOARGS);
        self.b.switch_to_block(bad);
        match self.cg.bytes_data(STOP_FREED.as_bytes()) {
            Ok(d) => {
                let mp = self.data_addr(d);
                let ml = self.ic(STOP_FREED.len() as i64);
                self.scall(SFn::ReportStop, &[mp, ml]);
            }
            Err(e) => {
                self.fail(e);
            }
        }
        self.b.ins().jump(ok, NOARGS);
        self.b.switch_to_block(ok);
        let fv = self.ic(FREED);
        self.st(fv, h, 8);
        let hd = self.ld(h, 0);
        let n = self.b.ins().band_imm_s(hd, !SILENT);
        let total = self.b.ins().iadd_imm_s(n, 16);
        let eight = self.ic(8);
        self.rt("raw_free", &[h, total, eight]);
    }

    /// `--trace-drops`（補足 G-3）: 黙っておらず移動済みのフィールドがない構造体は、破棄の前に `drop: <表示>` を出す。
    fn trace_struct(&mut self, id: usize, w: Value, tys: &[Ty]) {
        let h = self.b.ins().iadd_imm_s(w, -16);
        let hw = self.ld(h, 0);
        let silent = self.b.ins().band_imm_s(hw, SILENT);
        let (chk, end) = (self.b.create_block(), self.b.create_block());
        let not_silent = self.b.ins().icmp_imm_s(IntCC::Equal, silent, 0);
        self.b.ins().brif(not_silent, chk, NOARGS, end, NOARGS);
        self.b.switch_to_block(chk);
        let mut moved = self.ic(0);
        for (i, ft) in tys.iter().enumerate() {
            if aggregate(ft) {
                let x = self.ld(w, 8 * i as i32);
                let z = self.b.ins().icmp_imm_s(IntCC::Equal, x, 0);
                let zi = self.b2i(z);
                moved = self.b.ins().bor(moved, zi);
            }
        }
        let print = self.b.create_block();
        self.b.ins().brif(moved, end, NOARGS, print, NOARGS);
        self.b.switch_to_block(print);
        self.show_trace_drop(&Ty::Adt(id), w);
        self.b.ins().jump(end, NOARGS);
        self.b.switch_to_block(end);
    }

    /// `sigil_drop_<k>(w)`: 0 なら何もしない。そうでなければ、段の関数を作業の列に積み、列が空になるまで取り出して呼ぶ。
    /// 列の領域を確保していたら、空になった時点で返して静的な領域に戻す。
    fn drop_body(&mut self, t: &Ty) {
        let (w, _) = self.body_entry();
        let (go, end) = (self.b.create_block(), self.b.create_block());
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, w, 0);
        self.b.ins().brif(zero, end, NOARGS, go, NOARGS);

        self.b.switch_to_block(go);
        let ws = self.ws_base();
        let base = self.ld(ws, 0);
        let f0 = self.dstep_addr(t);
        self.ws_push(f0, w);
        let (head, body, exit) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);

        self.b.switch_to_block(head);
        let top = self.ld(ws, 0);
        let more = self.b.ins().icmp(IntCC::UnsignedGreaterThan, top, base);
        self.b.ins().brif(more, body, NOARGS, exit, NOARGS);

        self.b.switch_to_block(body);
        let (f, x) = self.ws_pop();
        let sig = self.b.import_signature(shim_sig(self.cg.cc, 1, 'v'));
        self.b.ins().call_indirect(sig, f, &[x]);
        self.b.ins().jump(head, NOARGS);

        // 列が空になったら、確保していた領域を返す。
        self.b.switch_to_block(exit);
        let top2 = self.ld(ws, 0);
        let empty = self.b.ins().icmp_imm_s(IntCC::Equal, top2, 0);
        let (release, done) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(empty, release, NOARGS, end, NOARGS);
        self.b.switch_to_block(release);
        let raw = self.ld(ws, 8);
        let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, raw, 0);
        self.b.ins().brif(has, done, NOARGS, end, NOARGS);
        self.b.switch_to_block(done);
        let rcap = self.ld(ws, 16);
        let nbytes = self.b.ins().imul_imm_s(rcap, 16);
        let eight = self.ic(8);
        self.rt("raw_free", &[raw, nbytes, eight]);
        let z = self.ic(0);
        self.st(z, ws, 8);
        self.st(z, ws, 16);
        self.b.ins().jump(end, NOARGS);

        self.b.switch_to_block(end);
        self.b.ins().return_(&[]);
    }

    /// `sigil_dstep_<k>(w)`: 値 `w`（0 でない）の 1 段。子を宣言の逆順に積んでから、自分の塊を解放する
    /// （子の語は解放の前に読む）。文字列・バイト列は解放だけをする。
    fn dstep_body(&mut self, t: &Ty) {
        let (w, _) = self.body_entry();
        let p = self.cg.p;
        match t {
            Ty::Str | Ty::Path => {
                self.rt("str_free", &[w]);
            }
            Ty::Bytes => {
                self.rt("bytes_free", &[w]);
            }
            Ty::Tuple(ts) => {
                for (i, ct) in ts.iter().enumerate().rev() {
                    let x = self.ld(w, 8 * i as i32);
                    self.push_child(ct, x);
                }
                self.blk_free(w);
            }
            Ty::Adt(id) => match &p.types[*id].kind {
                TypeKind::Struct { fields } => {
                    // `Secret`（ADR 0024、PR 6-1b）は、中身の Bytes を解放する前に
                    // データ領域をゼロ化する。判定は記述子の種類と同じく、
                    // lower が付けた同一性の印（`secret`）だけを見る。
                    if p.types[*id].secret {
                        let b = self.ld(w, 0);
                        self.rt("zeroize_bytes", &[b]);
                    }
                    if self.cg.trace {
                        let tys: Vec<Ty> = fields.iter().map(|(_, t)| t.clone()).collect();
                        self.trace_struct(*id, w, &tys);
                    }
                    for (i, (_, ft)) in fields.iter().enumerate().rev() {
                        let x = self.ld(w, 8 * i as i32);
                        self.push_child(ft, x);
                    }
                    self.blk_free(w);
                }
                TypeKind::Enum { variants } => {
                    let tag = self.ld(w, 0);
                    let free_b = self.b.create_block();
                    for (vi, v) in variants.iter().enumerate() {
                        if !v.fields.iter().any(aggregate) {
                            continue;
                        }
                        let (arm, next) = (self.b.create_block(), self.b.create_block());
                        let is = self.b.ins().icmp_imm_s(IntCC::Equal, tag, vi as i64);
                        self.b.ins().brif(is, arm, NOARGS, next, NOARGS);
                        self.b.switch_to_block(arm);
                        for (i, ft) in v.fields.iter().enumerate().rev() {
                            let x = self.ld(w, 8 + 8 * i as i32);
                            self.push_child(ft, x);
                        }
                        self.b.ins().jump(free_b, NOARGS);
                        self.b.switch_to_block(next);
                    }
                    self.b.ins().jump(free_b, NOARGS);
                    self.b.switch_to_block(free_b);
                    self.blk_free(w);
                }
            },
            Ty::List(e) => {
                if aggregate(e) {
                    // 要素を後ろから積む（取り出しは前から）。
                    let n = self.rt("list_len", &[w]);
                    let (head, body, exit) = (
                        self.b.create_block(),
                        self.b.create_block(),
                        self.b.create_block(),
                    );
                    for blk in [head, body] {
                        self.b.append_block_param(blk, types::I64);
                    }
                    let start = self.b.ins().iadd_imm_s(n, -1);
                    self.b.ins().jump(head, &[BlockArg::Value(start)]);
                    self.b.switch_to_block(head);
                    let i = self.b.block_params(head)[0];
                    let more = self
                        .b
                        .ins()
                        .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, i, 0);
                    self.b
                        .ins()
                        .brif(more, body, &[BlockArg::Value(i)], exit, NOARGS);
                    self.b.switch_to_block(body);
                    let i = self.b.block_params(body)[0];
                    let slot = self.rt("list_get", &[w, i]);
                    let x = self.ld(slot, 0);
                    self.push_child(e, x);
                    let prev = self.b.ins().iadd_imm_s(i, -1);
                    self.b.ins().jump(head, &[BlockArg::Value(prev)]);
                    self.b.switch_to_block(exit);
                }
                self.rt("list_free", &[w]);
            }
            Ty::Shared(i) | Ty::Cell(i) => {
                // 参照の数を 1 減らし、0 になったら中身を積んで塊を解放する。
                let cnt = self.rc_dec(w);
                let (dead, end) = (self.b.create_block(), self.b.create_block());
                let z = self.b.ins().icmp_imm_s(IntCC::Equal, cnt, 0);
                self.b.ins().brif(z, dead, NOARGS, end, NOARGS);
                self.b.switch_to_block(dead);
                let c = self.ld(w, 0);
                self.push_child(i, c);
                self.blk_free(w);
                self.b.ins().jump(end, NOARGS);
                self.b.switch_to_block(end);
            }
            Ty::FnRef(..) => {
                // 環境のある関数の値（塊）: 参照の数が 0 になったら、環境を積んで本体を解放する。
                // 環境の段の関数の番地は本体の +16 にある（`MakeClosure`）。
                let env = self.ld(w, 8);
                let (chk, end) = (self.b.create_block(), self.b.create_block());
                let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, env, 0);
                self.b.ins().brif(has, chk, NOARGS, end, NOARGS);
                self.b.switch_to_block(chk);
                let cnt = self.rc_dec(w);
                let dead = self.b.create_block();
                let z = self.b.ins().icmp_imm_s(IntCC::Equal, cnt, 0);
                self.b.ins().brif(z, dead, NOARGS, end, NOARGS);
                self.b.switch_to_block(dead);
                let fa = self.ld(w, 16);
                self.ws_push(fa, env);
                self.blk_free(w);
                self.b.ins().jump(end, NOARGS);
                self.b.switch_to_block(end);
            }
            // スカラー・参照は積まれない。
            _ => {}
        }
        self.b.ins().return_(&[]);
    }

    /// `sigil_blkfree_nz(w)`（補足 G-2）: 0 でなければ塊を解放する。どのプログラムにも置く共通の関数。
    pub(crate) fn blkfree_nz_body(&mut self) {
        let (w, _) = self.body_entry();
        let (go, end) = (self.b.create_block(), self.b.create_block());
        let zero = self.b.ins().icmp_imm_s(IntCC::Equal, w, 0);
        self.b.ins().brif(zero, end, NOARGS, go, NOARGS);
        self.b.switch_to_block(go);
        self.blk_free(w);
        self.b.ins().jump(end, NOARGS);
        self.b.switch_to_block(end);
        self.b.ins().return_(&[]);
    }

    /// 子を持たない値の破棄（組・列挙型・構造体で、子が破棄の対象の型でなく、`--trace-drops` でないもの）は、
    /// 型ごとの関数を作らず、共通の `sigil_blkfree_nz` を呼ぶ。呼んだら真。
    pub(super) fn drop_flat(&mut self, t: &Ty, w: Value) -> bool {
        let p = self.cg.p;
        let flat = match t {
            Ty::Tuple(ts) => !ts.iter().any(aggregate),
            Ty::Adt(id) => match &p.types[*id].kind {
                TypeKind::Struct { fields } => {
                    !self.cg.trace && !fields.iter().any(|(_, ft)| aggregate(ft))
                }
                TypeKind::Enum { variants } => {
                    variants.iter().all(|v| !v.fields.iter().any(aggregate))
                }
            },
            _ => false,
        };
        let Some(f) = self.cg.blkfree_nz else {
            return false;
        };
        if !flat {
            return false;
        }
        let fr = self.cg.m.declare_func_in_func(f, self.b.func);
        self.b.ins().call(fr, &[w]);
        true
    }

    /// 値 `w`（型 `t`）を破棄する（0 は移動済みで何もしない）。`drop.rs` の `drop_val` から使う。
    pub(super) fn drop_call(&mut self, t: &Ty, w: Value) {
        if !aggregate(t) {
            return;
        }
        let _ = self.gen_call(Gk::Drop, t, &[w]);
    }
}

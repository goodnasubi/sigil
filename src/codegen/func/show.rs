//! 表示の土台（ADR 0025 補足 H-1・H-2・H-3・H-4・H-5・H-6）。
//!
//! 型ごとの表示（3-5f-ii の `sigil_show_<k>`）が使う共通の補助を、プログラムに 1 組だけ生成する。
//! バッファは静的な 4096 バイトから始まり、足りなければ Sigil 製の確保器で 2 倍に伸ばし、
//! 使い終わったら返して静的な領域に戻す（補足 G-2 の破棄の作業の列と同じ作り）。
//! `Float` の最短表現は正確な多倍長演算による区間方式（Steele & White）で作り、
//! 評価器（共有の `fmt_float`、Rust の `{}`）と一致させる。引用は Rust の `char::escape_debug` と同じ規則。

use super::{Fx, NOARGS};
use crate::codegen::{SFn, SB_INLINE_BYTES};
use crate::ir::Ty;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, BlockArg, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;

/// 多倍長演算の語の数（64 ビット × 24 = 1536 ビット。補足 H-3）。
/// 必要なのは、10 のべきで拡縮した後でも高々約 1200 ビット（最小の非正規化数で見積もった）。
const BI_WORDS: usize = 24;
/// 多倍長演算の数 1 つ分の大きさ（バイト）。
const BI_SIZE: usize = BI_WORDS * 8;

/// 桁の並びの置き場の大きさ（最短表現は 17 桁まで。余裕を持たせる）。
const DIGITS_MAX: u32 = 800;

impl<'a, 'p> Fx<'a, 'p> {
    /// 表示の補助の本体（`SFn` ごと）。
    pub(crate) fn show_body(&mut self, k: SFn) {
        match k {
            SFn::Begin => {
                self.body_head();
                let sb = self.sb_base();
                let z = self.ic(0);
                self.st(z, sb, 0);
                self.b.ins().return_(&[]);
            }
            SFn::End => {
                self.body_head();
                let sb = self.sb_base();
                self.sb_cleanup(sb);
                self.b.ins().return_(&[]);
            }
            SFn::EndStr => {
                self.body_head();
                let sb = self.sb_base();
                let (buf, _) = self.sb_area(sb);
                let len = self.ld(sb, 0);
                let s = self.rt("str_new", &[buf, len]);
                self.sb_cleanup(sb);
                self.b.ins().return_(&[s]);
            }
            SFn::Push => self.show_push_body(),
            SFn::Int => self.show_int_body(),
            SFn::Float => self.show_float_body(),
            SFn::Quote => self.show_quote_body(),
            SFn::WriteAll => self.show_write_all_body(),
        }
    }

    /// 生成した関数の本体の入口。ブロックを作って切り替え、引数の語を返す。
    fn body_head(&mut self) -> Vec<Value> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.block_params(entry).to_vec()
    }

    // -----------------------------------------------------------------------
    // 呼び出し側の口

    /// 表示の補助 `k` を呼ぶ。`Float` の引数は語（ビット並び）から `f64` に直す。
    pub(super) fn scall(&mut self, k: SFn, args: &[Value]) -> Value {
        let id = match self.cg.show_fn(k) {
            Ok(i) => i,
            Err(e) => return self.fail(e),
        };
        let fr = self.cg.m.declare_func_in_func(id, self.b.func);
        let mut av: Vec<Value> = args.to_vec();
        if k == SFn::Float {
            av[0] = self.b.ins().bitcast(types::F64, MemFlagsData::new(), av[0]);
        }
        let inst = self.b.ins().call(fr, &av);
        let rs = self.b.inst_results(inst);
        if rs.is_empty() {
            self.ic(0)
        } else {
            rs[0]
        }
    }

    /// バイト列のリテラルをバッファへ足す。
    pub(super) fn sb_lit(&mut self, s: &[u8]) {
        if s.is_empty() {
            return;
        }
        let ptr = match self.cg.bytes_data(s) {
            Ok(d) => self.data_addr(d),
            Err(e) => {
                self.fail(e);
                return;
            }
        };
        let len = self.ic(s.len() as i64);
        self.scall(SFn::Push, &[ptr, len]);
    }

    /// 改行を標準出力へ書く（入口の表示の締め）。
    fn sb_newline_out(&mut self) {
        let ptr = match self.cg.bytes_data(b"\n") {
            Ok(d) => self.data_addr(d),
            Err(e) => {
                self.fail(e);
                return;
            }
        };
        let fd = self.ic(1);
        let one = self.ic(1);
        self.scall(SFn::WriteAll, &[fd, ptr, one]);
    }

    /// 入口の戻り値の表示（補足 H-6）。スカラーは生成した補助で組み立てて書き出し、
    /// それ以外は `val_show` の文字列を書き出す（付け替えは 3-5f-ii）。
    pub(super) fn show_entry_result(&mut self, ty: &Ty, w: Value) {
        match ty {
            Ty::Unit | Ty::Int | Ty::Float | Ty::Bool => {
                self.scall(SFn::Begin, &[]);
                match ty {
                    Ty::Unit => self.sb_lit(b"()"),
                    Ty::Int => {
                        self.scall(SFn::Int, &[w]);
                    }
                    Ty::Float => {
                        self.scall(SFn::Float, &[w]);
                    }
                    _ => {
                        // Bool: 0 でなければ `true`。
                        let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, w, 0);
                        let (tp, tl) = self.lit_ref(b"true");
                        let (fp, fl) = self.lit_ref(b"false");
                        let ptr = self.b.ins().select(nz, tp, fp);
                        let len = self.b.ins().select(nz, tl, fl);
                        self.scall(SFn::Push, &[ptr, len]);
                    }
                }
                let sb = self.sb_base();
                let (buf, _) = self.sb_area(sb);
                let len = self.ld(sb, 0);
                let fd = self.ic(1);
                self.scall(SFn::WriteAll, &[fd, buf, len]);
                self.sb_newline_out();
                self.scall(SFn::End, &[]);
            }
            _ => {
                let d = self.desc_addr(ty);
                let q = self.ic(1);
                let s = self.rt("val_show", &[d, w, q]);
                let p = self.rt("str_data", &[s]);
                let n = self.rt("str_bytes", &[s]);
                let fd = self.ic(1);
                self.scall(SFn::WriteAll, &[fd, p, n]);
                self.sb_newline_out();
                self.rt("str_free", &[s]);
            }
        }
    }

    /// リテラルの（番地, 長さ）。
    fn lit_ref(&mut self, s: &[u8]) -> (Value, Value) {
        let ptr = match self.cg.bytes_data(s) {
            Ok(d) => self.data_addr(d),
            Err(e) => self.fail(e),
        };
        (ptr, self.ic(s.len() as i64))
    }

    // -----------------------------------------------------------------------
    // バッファ（補足 H-2）

    /// バッファの状態の番地。語: `[0]` 長さ、`[8]` 確保した領域（0 なら静的な領域）、`[16]` その容量、
    /// `[24..]` 静的な領域。
    fn sb_base(&mut self) -> Value {
        match self.cg.show_buf_data() {
            Ok(d) => self.data_addr(d),
            Err(e) => self.fail(e),
        }
    }

    /// 今の領域の番地と容量（確保した領域がなければ静的な領域。容量は `SB_INLINE_BYTES`）。
    fn sb_area(&mut self, sb: Value) -> (Value, Value) {
        let raw = self.ld(sb, 8);
        let inl = self.b.ins().iadd_imm_s(sb, 24);
        let rawcap = self.ld(sb, 16);
        let none = self.b.ins().icmp_imm_s(IntCC::Equal, raw, 0);
        let buf = self.b.ins().select(none, inl, raw);
        let inl_cap = self.ic(SB_INLINE_BYTES as i64);
        let cap = self.b.ins().select(none, inl_cap, rawcap);
        (buf, cap)
    }

    /// バッファの片付け（伸ばした領域を返し、長さを 0 に戻す）。`End` と `EndStr` が使う。
    fn sb_cleanup(&mut self, sb: Value) {
        let raw = self.ld(sb, 8);
        let (freeb, keep) = (self.b.create_block(), self.b.create_block());
        let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, raw, 0);
        self.b.ins().brif(has, freeb, NOARGS, keep, NOARGS);
        self.b.switch_to_block(freeb);
        let cap = self.ld(sb, 16);
        let eight = self.ic(8);
        self.rt("raw_free", &[raw, cap, eight]);
        self.b.ins().jump(keep, NOARGS);
        self.b.switch_to_block(keep);
        let z = self.ic(0);
        self.st(z, sb, 8);
        self.st(z, sb, 16);
        self.st(z, sb, 0);
    }

    /// `sigil_show_push(ptr, len)`: バッファへバイト列を足す。足りなければ 2 倍に伸ばす。
    fn show_push_body(&mut self) {
        let ps = self.body_head();
        let (ptr, len) = (ps[0], ps[1]);
        let (go, done) = (self.b.create_block(), self.b.create_block());
        let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, len, 0);
        self.b.ins().brif(nz, go, NOARGS, done, NOARGS);
        self.b.switch_to_block(done);
        self.b.ins().return_(&[]);
        self.b.switch_to_block(go);
        let sb = self.sb_base();
        let used = self.ld(sb, 0);
        let (buf, cap) = self.sb_area(sb);
        let need = self.b.ins().iadd(used, len);
        let fits = self.b.ins().icmp(IntCC::UnsignedLessThanOrEqual, need, cap);
        let (store, grow) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(fits, store, NOARGS, grow, NOARGS);

        // 伸ばす（容量を 2 倍ずつ、必要分まで）。
        self.b.switch_to_block(grow);
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        self.b.ins().jump(head, &[BlockArg::Value(cap)]);
        self.b.switch_to_block(head);
        let nc = self.b.block_params(head)[0];
        let nc2 = self.b.ins().ishl_imm_s(nc, 1);
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, nc2, need);
        let (again, alloc) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(more, again, NOARGS, alloc, NOARGS);
        self.b.switch_to_block(again);
        self.b.ins().jump(head, &[BlockArg::Value(nc2)]);
        self.b.switch_to_block(alloc);
        let eight = self.ic(8);
        let nbuf = self.rt("raw_alloc", &[nc2, eight]);
        self.rt("memmove", &[nbuf, buf, used]);
        let raw = self.ld(sb, 8);
        let (free_old, keep) = (self.b.create_block(), self.b.create_block());
        let has = self.b.ins().icmp_imm_s(IntCC::NotEqual, raw, 0);
        self.b.ins().brif(has, free_old, NOARGS, keep, NOARGS);
        self.b.switch_to_block(free_old);
        self.rt("raw_free", &[raw, cap, eight]);
        self.b.ins().jump(keep, NOARGS);
        self.b.switch_to_block(keep);
        self.st(nbuf, sb, 8);
        self.st(nc2, sb, 16);
        self.b.ins().jump(store, NOARGS);

        // 足す。
        self.b.switch_to_block(store);
        let (buf2, _) = self.sb_area(sb);
        let dest = self.b.ins().iadd(buf2, used);
        self.rt("memmove", &[dest, ptr, len]);
        let nu = self.b.ins().iadd(used, len);
        self.st(nu, sb, 0);
        self.b.ins().return_(&[]);
    }

    // -----------------------------------------------------------------------
    // 書き出し（補足 H-6）

    /// `sigil_write_all(fd, ptr, len)`: 全部書くまで繰り返す（部分書き込みと `EINTR`）。
    /// 書けなければ黙ってやめる（報告の経路そのものなので、ここで止まらない）。
    fn show_write_all_body(&mut self) {
        let ps = self.body_head();
        let (fd, ptr, len) = (ps[0], ps[1], ps[2]);
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(ptr), BlockArg::Value(len)]);
        self.b.switch_to_block(head);
        let p = self.b.block_params(head)[0];
        let n = self.b.block_params(head)[1];
        let pos = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, n, 0);
        let body = self.b.create_block();
        self.b.ins().brif(pos, body, NOARGS, done, NOARGS);
        self.b.switch_to_block(done);
        self.b.ins().return_(&[]);
        self.b.switch_to_block(body);
        let one = self.ic(1);
        let z = self.ic(0);
        let r = self.rt("raw_syscall", &[one, fd, p, n, z, z, z]);
        let eintr = self.b.ins().icmp_imm_s(IntCC::Equal, r, -4);
        let (retry, check) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(eintr, retry, NOARGS, check, NOARGS);
        self.b.switch_to_block(retry);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(p), BlockArg::Value(n)]);
        self.b.switch_to_block(check);
        let ok = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, r, 0);
        let next = self.b.create_block();
        self.b.ins().brif(ok, next, NOARGS, done, NOARGS);
        self.b.switch_to_block(next);
        let p2 = self.b.ins().iadd(p, r);
        let n2 = self.b.ins().isub(n, r);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(p2), BlockArg::Value(n2)]);
    }

    // -----------------------------------------------------------------------
    // Int（補足 H-5）

    /// `sigil_show_int(n)`: 10 進の並びをバッファへ足す（符号つき。最小値でも溢れない）。
    fn show_int_body(&mut self) {
        let ps = self.body_head();
        let n = ps[0];
        let slot = self.tmp(32);
        let base = self.saddr(slot);
        let endp = self.b.ins().iadd_imm_s(base, 32);
        let neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, n, 0);
        let z = self.ic(0);
        let mag = self.b.ins().isub(z, n);
        let u0 = self.b.ins().select(neg, mag, n);
        // 後ろから 1 桁ずつ置く（(置き場, 残り) の繰り返し）。
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(endp), BlockArg::Value(u0)]);
        self.b.switch_to_block(head);
        let pos = self.b.block_params(head)[0];
        let u = self.b.block_params(head)[1];
        let ten = self.ic(10);
        let d = self.b.ins().urem(u, ten);
        let u2 = self.b.ins().udiv(u, ten);
        let pos2 = self.b.ins().iadd_imm_s(pos, -1);
        let ch = self.b.ins().iadd_imm_s(d, 48);
        self.st8(ch, pos2);
        let more = self.b.ins().icmp_imm_s(IntCC::NotEqual, u2, 0);
        self.b.ins().brif(
            more,
            head,
            &[BlockArg::Value(pos2), BlockArg::Value(u2)],
            done,
            &[BlockArg::Value(pos2)],
        );
        self.b.switch_to_block(done);
        let pos3 = self.b.block_params(done)[0];
        let (with_sign, fin) = (self.b.create_block(), self.b.create_block());
        self.b.append_block_param(fin, types::I64);
        self.b
            .ins()
            .brif(neg, with_sign, NOARGS, fin, &[BlockArg::Value(pos3)]);
        self.b.switch_to_block(with_sign);
        let pos4 = self.b.ins().iadd_imm_s(pos3, -1);
        let dash = self.ic(45);
        self.st8(dash, pos4);
        self.b.ins().jump(fin, &[BlockArg::Value(pos4)]);
        self.b.switch_to_block(fin);
        let start = self.b.block_params(fin)[0];
        let len = self.b.ins().isub(endp, start);
        self.scall(SFn::Push, &[start, len]);
        self.b.ins().return_(&[]);
    }
}

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 引用（補足 H-4）

    /// 1 バイトを読む（符号なしで広げる）。
    fn ld8(&mut self, addr: Value) -> Value {
        let b = self.b.ins().load(types::I8, MemFlagsData::new(), addr, 0);
        self.b.ins().uextend(types::I64, b)
    }

    /// 1 バイトを書く（下位 8 ビット）。
    fn st8(&mut self, v: Value, addr: Value) {
        self.b.ins().istore8(MemFlagsData::trusted(), v, addr, 0);
    }

    /// 符号点 `cp` が `\u{...}` に逃がす対象か（範囲の表を二分探索する）。
    fn quote_escaped(&mut self, cp: Value) -> Value {
        let (data, count) = match self.cg.quote_data() {
            Ok(v) => v,
            Err(e) => return self.fail(e),
        };
        let base = self.data_addr(data);
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        let z = self.ic(0);
        let cnt = self.ic(count as i64);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(z), BlockArg::Value(cnt)]);
        self.b.switch_to_block(head);
        let lo = self.b.block_params(head)[0];
        let hi = self.b.block_params(head)[1];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, lo, hi);
        let (body, miss) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(more, body, NOARGS, miss, NOARGS);
        self.b.switch_to_block(miss);
        self.b.ins().jump(done, &[BlockArg::Value(z)]);
        self.b.switch_to_block(body);
        let sum = self.b.ins().iadd(lo, hi);
        let one_sh = self.ic(1);
        let mid = self.b.ins().ushr(sum, one_sh);
        let off = self.b.ins().imul_imm_s(mid, 16);
        let at = self.b.ins().iadd(base, off);
        let rlo = self.ld(at, 0);
        let rhi = self.ld(at, 8);
        let below = self.b.ins().icmp(IntCC::SignedLessThan, cp, rlo);
        let (left, rest) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(below, left, NOARGS, rest, NOARGS);
        self.b.switch_to_block(left);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(lo), BlockArg::Value(mid)]);
        self.b.switch_to_block(rest);
        let above = self.b.ins().icmp(IntCC::SignedGreaterThan, cp, rhi);
        let (right, hit) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(above, right, NOARGS, hit, NOARGS);
        self.b.switch_to_block(right);
        let mid1 = self.b.ins().iadd_imm_s(mid, 1);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(mid1), BlockArg::Value(hi)]);
        self.b.switch_to_block(hit);
        let one = self.ic(1);
        self.b.ins().jump(done, &[BlockArg::Value(one)]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }

    /// 符号点の 16 進（小文字、余分な 0 なし）をバッファへ足す。
    fn sb_hex(&mut self, cp: Value) {
        let slot = self.tmp(8);
        let base = self.saddr(slot);
        let endp = self.b.ins().iadd_imm_s(base, 8);
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(endp), BlockArg::Value(cp)]);
        self.b.switch_to_block(head);
        let pos = self.b.block_params(head)[0];
        let v = self.b.block_params(head)[1];
        let d = self.b.ins().band_imm_s(v, 15);
        let four = self.ic(4);
        let v2 = self.b.ins().ushr(v, four);
        let digit = self.b.ins().iadd_imm_s(d, 48);
        let letter = self.b.ins().iadd_imm_s(d, 87);
        let is_digit = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, d, 10);
        let ch = self.b.ins().select(is_digit, digit, letter);
        let pos2 = self.b.ins().iadd_imm_s(pos, -1);
        self.st8(ch, pos2);
        let more = self.b.ins().icmp_imm_s(IntCC::NotEqual, v2, 0);
        self.b.ins().brif(
            more,
            head,
            &[BlockArg::Value(pos2), BlockArg::Value(v2)],
            done,
            &[BlockArg::Value(pos2)],
        );
        self.b.switch_to_block(done);
        let start = self.b.block_params(done)[0];
        let len = self.b.ins().isub(endp, start);
        self.scall(SFn::Push, &[start, len]);
    }

    /// `sigil_show_quote(ptr, len)`: 文字列の中身の引用をバッファへ足す（`"` で囲む）。
    /// 文字の切り分けは UTF-8 の先行バイトで決める（文字列は正しい UTF-8 であることが保証される）。
    fn show_quote_body(&mut self) {
        let ps = self.body_head();
        let (ptr, len) = (ps[0], ps[1]);
        self.sb_lit(b"\"");
        // 繰り返し: (済んだバイト数)。
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        let z = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let left = self.b.ins().icmp(IntCC::SignedLessThan, i, len);
        let body = self.b.create_block();
        self.b.ins().brif(left, body, NOARGS, done, NOARGS);
        self.b.switch_to_block(done);
        self.sb_lit(b"\"");
        self.b.ins().return_(&[]);

        // 1 文字を取り出す: （符号点, バイト数）を決める。
        self.b.switch_to_block(body);
        let at = self.b.ins().iadd(ptr, i);
        let b0 = self.ld8(at);
        let one = self.b.create_block();
        self.b.append_block_param(one, types::I64);
        self.b.append_block_param(one, types::I64);
        let ascii = self.b.ins().icmp_imm_s(IntCC::UnsignedLessThan, b0, 0x80);
        let (multi, ascii_go) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(ascii, ascii_go, NOARGS, multi, NOARGS);
        self.b.switch_to_block(ascii_go);
        let one_i = self.ic(1);
        self.b
            .ins()
            .jump(one, &[BlockArg::Value(b0), BlockArg::Value(one_i)]);
        self.b.switch_to_block(multi);
        let lead2 = self.b.ins().band_imm_s(b0, 0xE0);
        let is2 = self.b.ins().icmp_imm_s(IntCC::Equal, lead2, 0xC0);
        let (d2, rest) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is2, d2, NOARGS, rest, NOARGS);
        self.b.switch_to_block(d2);
        let cp2 = self.b.ins().band_imm_s(b0, 0x1F);
        let cp2 = self.decode_tail(at, cp2, 1);
        let two = self.ic(2);
        self.b
            .ins()
            .jump(one, &[BlockArg::Value(cp2), BlockArg::Value(two)]);
        self.b.switch_to_block(rest);
        let lead3 = self.b.ins().band_imm_s(b0, 0xF0);
        let is3 = self.b.ins().icmp_imm_s(IntCC::Equal, lead3, 0xE0);
        let (d3, d4) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is3, d3, NOARGS, d4, NOARGS);
        self.b.switch_to_block(d3);
        let cp3 = self.b.ins().band_imm_s(b0, 0x0F);
        let cp3 = self.decode_tail(at, cp3, 2);
        let three = self.ic(3);
        self.b
            .ins()
            .jump(one, &[BlockArg::Value(cp3), BlockArg::Value(three)]);
        self.b.switch_to_block(d4);
        let cp4 = self.b.ins().band_imm_s(b0, 0x07);
        let cp4 = self.decode_tail(at, cp4, 3);
        let four = self.ic(4);
        self.b
            .ins()
            .jump(one, &[BlockArg::Value(cp4), BlockArg::Value(four)]);

        // 1 文字分: （符号点, バイト数）→ 逃がすか、そのまま足すか。
        self.b.switch_to_block(one);
        let cp = self.b.block_params(one)[0];
        let cplen = self.b.block_params(one)[1];
        let next = self.b.create_block();
        // 名前つきの逃がし。
        let names: [(&[u8], i64); 5] = [
            (b"\\t", 9),
            (b"\\n", 10),
            (b"\\r", 13),
            (b"\\\\", 92),
            (b"\\\"", 34),
        ];
        let mut plain = one;
        for (lit, code) in names {
            let hit = self.b.create_block();
            let eq = self.b.ins().icmp_imm_s(IntCC::Equal, cp, code);
            let miss = self.b.create_block();
            self.b.ins().brif(eq, hit, NOARGS, miss, NOARGS);
            self.b.switch_to_block(hit);
            self.sb_lit(lit);
            self.b.ins().jump(next, NOARGS);
            self.b.switch_to_block(miss);
            plain = miss;
        }
        self.b.switch_to_block(plain);
        let esc = self.quote_escaped(cp);
        let esc_nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, esc, 0);
        let (esc_go, raw_go) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(esc_nz, esc_go, NOARGS, raw_go, NOARGS);
        self.b.switch_to_block(esc_go);
        self.sb_lit(b"\\u{");
        self.sb_hex(cp);
        self.sb_lit(b"}");
        self.b.ins().jump(next, NOARGS);
        self.b.switch_to_block(raw_go);
        self.scall(SFn::Push, &[at, cplen]);
        self.b.ins().jump(next, NOARGS);
        self.b.switch_to_block(next);
        let i2 = self.b.ins().iadd(i, cplen);
        self.b.ins().jump(head, &[BlockArg::Value(i2)]);
    }

    /// UTF-8 の続きのバイトを `n` 個読んで符号点にする（`at` が先行バイトの番地）。
    fn decode_tail(&mut self, at: Value, cp0: Value, n: i64) -> Value {
        let mut cp = cp0;
        for j in 1..=n {
            let a = self.b.ins().iadd_imm_s(at, j);
            let b = self.ld8(a);
            let low = self.b.ins().band_imm_s(b, 0x3F);
            let sh = self.b.ins().ishl_imm_s(cp, 6);
            cp = self.b.ins().bor(sh, low);
        }
        cp
    }
}

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 多倍長演算（補足 H-3）。数は語の並び（下位が先）で、番地はその先頭。

    fn bi_ld(&mut self, a: Value, i: usize) -> Value {
        self.ld(a, (i * 8) as i32)
    }

    fn bi_st(&mut self, a: Value, i: usize, v: Value) {
        self.st(v, a, (i * 8) as i32)
    }

    fn bi_zero(&mut self, a: Value) {
        let z = self.ic(0);
        for i in 0..BI_WORDS {
            self.bi_st(a, i, z);
        }
    }

    /// `a = m << shift`（`m` は 53 ビットまで、`shift` は 1075 まで）。
    fn bi_set(&mut self, a: Value, m: Value, shift: Value) {
        self.bi_zero(a);
        let six = self.ic(6);
        let ws = self.b.ins().ushr(shift, six);
        let bs = self.b.ins().band_imm_s(shift, 63);
        let three = self.ic(3);
        let off = self.b.ins().ishl(ws, three);
        let addr = self.b.ins().iadd(a, off);
        let lo = self.b.ins().ishl(m, bs);
        self.st(lo, addr, 0);
        let (hi_go, done) = (self.b.create_block(), self.b.create_block());
        let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, bs, 0);
        self.b.ins().brif(nz, hi_go, NOARGS, done, NOARGS);
        self.b.switch_to_block(hi_go);
        let sixtyfour = self.ic(64);
        let rest = self.b.ins().isub(sixtyfour, bs);
        let hi = self.b.ins().ushr(m, rest);
        self.st(hi, addr, 8);
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(done);
    }

    fn bi_copy(&mut self, dst: Value, src: Value) {
        let n = self.ic(BI_SIZE as i64);
        self.rt("memmove", &[dst, src, n]);
    }

    /// `dst += src`（桁上がりは捨てる。呼び出し側が溢れないことを保証する）。
    fn bi_add(&mut self, dst: Value, src: Value) {
        let mut carry = self.ic(0);
        for i in 0..BI_WORDS {
            let x = self.bi_ld(dst, i);
            let y = self.bi_ld(src, i);
            let s1 = self.b.ins().iadd(x, carry);
            let c1 = self.b.ins().icmp(IntCC::UnsignedLessThan, s1, x);
            let s = self.b.ins().iadd(s1, y);
            let c2 = self.b.ins().icmp(IntCC::UnsignedLessThan, s, s1);
            self.bi_st(dst, i, s);
            let cor = self.b.ins().bor(c1, c2);
            carry = self.b2i(cor);
        }
    }

    /// `dst -= src`（`dst >= src` のときだけ呼ぶ）。
    fn bi_sub(&mut self, dst: Value, src: Value) {
        let mut borrow = self.ic(0);
        for i in 0..BI_WORDS {
            let x = self.bi_ld(dst, i);
            let y = self.bi_ld(src, i);
            let b1 = self.b.ins().icmp(IntCC::UnsignedLessThan, x, borrow);
            let d1 = self.b.ins().isub(x, borrow);
            let b2 = self.b.ins().icmp(IntCC::UnsignedLessThan, d1, y);
            let d = self.b.ins().isub(d1, y);
            self.bi_st(dst, i, d);
            let bor2 = self.b.ins().bor(b1, b2);
            borrow = self.b2i(bor2);
        }
    }

    /// `a *= 10`。
    fn bi_mul10(&mut self, a: Value) {
        let ten = self.ic(10);
        let mut carry = self.ic(0);
        for i in 0..BI_WORDS {
            let x = self.bi_ld(a, i);
            let hi = self.b.ins().umulhi(x, ten);
            let lo = self.b.ins().imul(x, ten);
            let s = self.b.ins().iadd(lo, carry);
            let c = self.b.ins().icmp(IntCC::UnsignedLessThan, s, lo);
            let cb = self.b2i(c);
            carry = self.b.ins().iadd(hi, cb);
            self.bi_st(a, i, s);
        }
    }

    /// `a` と `b` の比較（−1・0・1）。
    fn bi_cmp(&mut self, a: Value, b: Value) -> Value {
        let mut res = self.ic(0);
        for i in (0..BI_WORDS).rev() {
            let x = self.bi_ld(a, i);
            let y = self.bi_ld(b, i);
            let gt = self.b.ins().icmp(IntCC::UnsignedGreaterThan, x, y);
            let lt = self.b.ins().icmp(IntCC::UnsignedLessThan, x, y);
            let zero = self.b.ins().icmp_imm_s(IntCC::Equal, res, 0);
            let tg = self.b.ins().band(zero, gt);
            let one = self.ic(1);
            res = self.b.ins().select(tg, one, res);
            let tl = self.b.ins().band(zero, lt);
            let mone = self.ic(-1);
            res = self.b.ins().select(tl, mone, res);
        }
        res
    }

    /// `a *= 10^cnt`（`cnt >= 0`）。
    fn bi_pow10(&mut self, a: Value, cnt: Value) {
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        let z = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, cnt);
        let body = self.b.create_block();
        self.b.ins().brif(more, body, NOARGS, done, NOARGS);
        self.b.switch_to_block(body);
        self.bi_mul10(a);
        let i2 = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(head, &[BlockArg::Value(i2)]);
        self.b.switch_to_block(done);
    }

    /// `0` を `cnt` 個バッファへ足す。
    fn sb_zeros(&mut self, cnt: Value) {
        let (zp, _) = self.lit_ref(b"0");
        let head = self.b.create_block();
        self.b.append_block_param(head, types::I64);
        let done = self.b.create_block();
        let z = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, cnt);
        let body = self.b.create_block();
        self.b.ins().brif(more, body, NOARGS, done, NOARGS);
        self.b.switch_to_block(body);
        let one = self.ic(1);
        self.scall(SFn::Push, &[zp, one]);
        let i2 = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(head, &[BlockArg::Value(i2)]);
        self.b.switch_to_block(done);
    }

    // -----------------------------------------------------------------------
    // Float（補足 H-3）

    /// `sigil_show_float(f)`: 最短表現をバッファへ足す（区間方式）。
    /// 値 `f = m・2^e` に対し、丸めの区間 `(f − Mm, f + Mp)` を整数の比 `R/S` で保ち、
    /// 区間の中に入る最も短い 10 進の並びを作る。端を含むかどうかは `m` の偶奇で決まる。
    fn show_float_body(&mut self) {
        let ps = self.body_head();
        let f = ps[0];
        let bits = self.b.ins().bitcast(types::I64, MemFlagsData::new(), f);
        let neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, bits, 0);
        let fiftytwo = self.ic(52);
        let exp_sh = self.b.ins().ushr(bits, fiftytwo);
        let exp = self.b.ins().band_imm_s(exp_sh, 0x7FF);
        let frac = self.b.ins().band_imm_s(bits, (1i64 << 52) - 1);
        let z = self.ic(0);
        let one = self.ic(1);
        // 特殊な値（NaN・無限大）。
        let (sp, norm) = (self.b.create_block(), self.b.create_block());
        let is_sp = self.b.ins().icmp_imm_s(IntCC::Equal, exp, 0x7FF);
        self.b.ins().brif(is_sp, sp, NOARGS, norm, NOARGS);
        self.b.switch_to_block(sp);
        let (nan_b, inf_b) = (self.b.create_block(), self.b.create_block());
        let isnan = self.b.ins().icmp_imm_s(IntCC::NotEqual, frac, 0);
        self.b.ins().brif(isnan, nan_b, NOARGS, inf_b, NOARGS);
        self.b.switch_to_block(nan_b);
        self.sb_lit(b"NaN");
        self.b.ins().return_(&[]);
        self.b.switch_to_block(inf_b);
        let (ip, il) = self.lit_ref(b"inf");
        let (np, nl) = self.lit_ref(b"-inf");
        let ptr = self.b.ins().select(neg, np, ip);
        let len = self.b.ins().select(neg, nl, il);
        self.scall(SFn::Push, &[ptr, len]);
        self.b.ins().return_(&[]);
        // 0（符号つき）。
        self.b.switch_to_block(norm);
        let (zero_b, main) = (self.b.create_block(), self.b.create_block());
        let ez = self.b.ins().icmp_imm_s(IntCC::Equal, exp, 0);
        let fz = self.b.ins().icmp_imm_s(IntCC::Equal, frac, 0);
        let is_zero = self.b.ins().band(ez, fz);
        self.b.ins().brif(is_zero, zero_b, NOARGS, main, NOARGS);
        self.b.switch_to_block(zero_b);
        let (zs, zdone) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(neg, zs, NOARGS, zdone, NOARGS);
        self.b.switch_to_block(zs);
        self.sb_lit(b"-");
        self.b.ins().jump(zdone, NOARGS);
        self.b.switch_to_block(zdone);
        self.sb_lit(b"0.0");
        self.b.ins().return_(&[]);
        // 符号。
        self.b.switch_to_block(main);
        let (ms, mdone) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(neg, ms, NOARGS, mdone, NOARGS);
        self.b.switch_to_block(ms);
        self.sb_lit(b"-");
        self.b.ins().jump(mdone, NOARGS);
        self.b.switch_to_block(mdone);

        // 分解: f = m・2^e。
        let sub = self.b.ins().icmp_imm_s(IntCC::Equal, exp, 0);
        let norm_m = self.b.ins().bor_imm_s(frac, 1i64 << 52);
        let m = self.b.ins().select(sub, frac, norm_m);
        let emin = self.ic(-1074);
        let e_norm = self.b.ins().iadd_imm_s(exp, -1075);
        let e = self.b.ins().select(sub, emin, e_norm);
        // 作業場（R・S・Mp・Mm・T・U）。
        let frame = self.zero_tmp((BI_SIZE * 6) as u32);
        let ar = frame;
        let as_ = self.b.ins().iadd_imm_s(frame, BI_SIZE as i64);
        let amp = self.b.ins().iadd_imm_s(frame, (BI_SIZE * 2) as i64);
        let amm = self.b.ins().iadd_imm_s(frame, (BI_SIZE * 3) as i64);
        let at = self.b.ins().iadd_imm_s(frame, (BI_SIZE * 4) as i64);
        // 区間の組み立て（場合分けは select で値を選ぶ）。
        let two52 = self.ic(1i64 << 52);
        let big = self.b.ins().icmp(IntCC::Equal, m, two52);
        let epos = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, e, 0);
        let e_ne_min = self.b.ins().icmp_imm_s(IntCC::NotEqual, e, -1074);
        let asym = self.b.ins().band(big, e_ne_min);
        let two = self.ic(2);
        let e_p2 = self.b.ins().iadd_imm_s(e, 2);
        let e_p1 = self.b.ins().iadd_imm_s(e, 1);
        let r_pos = self.b.ins().select(big, e_p2, e_p1);
        let r_neg = self.b.ins().select(asym, two, one);
        let r_sh = self.b.ins().select(epos, r_pos, r_neg);
        let s_pos = self.b.ins().select(big, two, one);
        let two_m_e = self.b.ins().isub(two, e);
        let one_m_e = self.b.ins().isub(one, e);
        let s_neg = self.b.ins().select(asym, two_m_e, one_m_e);
        let s_sh = self.b.ins().select(epos, s_pos, s_neg);
        let mp_pos = self.b.ins().select(big, e_p1, e);
        let mp_neg = self.b.ins().select(asym, one, z);
        let mp_sh = self.b.ins().select(epos, mp_pos, mp_neg);
        let mm_sh = self.b.ins().select(epos, e, z);
        self.bi_set(ar, m, r_sh);
        self.bi_set(as_, one, s_sh);
        self.bi_set(amp, one, mp_sh);
        self.bi_set(amm, one, mm_sh);
        let lsb = self.b.ins().band_imm_s(m, 1);
        let incl = self.b.ins().icmp_imm_s(IntCC::Equal, lsb, 0);

        // 小数の指数 K の見当と置き場。
        let kslot = self.tmp(8);
        let kaddr = self.saddr(kslot);
        let dslot = self.tmp(8);
        let dcaddr = self.saddr(dslot);
        // ビット長（m を右へずらしながら数える）。
        let bhead = self.b.create_block();
        self.b.append_block_param(bhead, types::I64);
        self.b.append_block_param(bhead, types::I64);
        let bdone = self.b.create_block();
        self.b.append_block_param(bdone, types::I64);
        self.b
            .ins()
            .jump(bhead, &[BlockArg::Value(m), BlockArg::Value(z)]);
        self.b.switch_to_block(bhead);
        let bv = self.b.block_params(bhead)[0];
        let bc = self.b.block_params(bhead)[1];
        let bmore = self.b.ins().icmp_imm_s(IntCC::NotEqual, bv, 0);
        let bbody = self.b.create_block();
        self.b
            .ins()
            .brif(bmore, bbody, NOARGS, bdone, &[BlockArg::Value(bc)]);
        self.b.switch_to_block(bbody);
        let bv2 = self.b.ins().ushr(bv, one);
        let bc2 = self.b.ins().iadd_imm_s(bc, 1);
        self.b
            .ins()
            .jump(bhead, &[BlockArg::Value(bv2), BlockArg::Value(bc2)]);
        self.b.switch_to_block(bdone);
        let bl = self.b.block_params(bdone)[0];
        // K = floor((t − 1)・log10(2)) + 1（t = e + ビット長。log10(2) ≒ 30103/100000）。
        let t = self.b.ins().iadd(e, bl);
        let tm1 = self.b.ins().iadd_imm_s(t, -1);
        let num = self.b.ins().imul_imm_s(tm1, 30103);
        let den = self.ic(100000);
        let q0 = self.b.ins().sdiv(num, den);
        let r0 = self.b.ins().srem(num, den);
        let r_nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, r0, 0);
        let num_neg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, num, 0);
        let adj = self.b.ins().band(r_nz, num_neg);
        let q_dec = self.b.ins().iadd_imm_s(q0, -1);
        let q = self.b.ins().select(adj, q_dec, q0);
        let k0 = self.b.ins().iadd_imm_s(q, 1);
        self.st(k0, kaddr, 0);
        // 拡縮（K >= 0 なら S を、K < 0 なら R・Mp・Mm を 10^(∓K) 倍する）。
        let k = self.ld(kaddr, 0);
        let (spos, sneg) = (self.b.create_block(), self.b.create_block());
        let sdone = self.b.create_block();
        let kpos = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, k, 0);
        self.b.ins().brif(kpos, spos, NOARGS, sneg, NOARGS);
        self.b.switch_to_block(spos);
        self.bi_pow10(as_, k);
        self.b.ins().jump(sdone, NOARGS);
        self.b.switch_to_block(sneg);
        let nk = self.b.ins().isub(z, k);
        self.bi_pow10(ar, nk);
        self.bi_pow10(amp, nk);
        self.bi_pow10(amm, nk);
        self.b.ins().jump(sdone, NOARGS);
        self.b.switch_to_block(sdone);
        // 直し: 区間が [1/10, 1) に収まるよう K を直す（上下どちらのずれも直る）。
        let fhead = self.b.create_block();
        self.b.ins().jump(fhead, NOARGS);
        self.b.switch_to_block(fhead);
        self.bi_copy(at, ar);
        self.bi_add(at, amp);
        let c = self.bi_cmp(at, as_);
        let (fup, fcheck) = (self.b.create_block(), self.b.create_block());
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, c, 0);
        self.b.ins().brif(ge, fup, NOARGS, fcheck, NOARGS);
        self.b.switch_to_block(fup);
        self.bi_mul10(as_);
        let kk = self.ld(kaddr, 0);
        let kk1 = self.b.ins().iadd_imm_s(kk, 1);
        self.st(kk1, kaddr, 0);
        self.b.ins().jump(fhead, NOARGS);
        self.b.switch_to_block(fcheck);
        self.bi_mul10(at);
        let c2 = self.bi_cmp(at, as_);
        let (fdown, fexit) = (self.b.create_block(), self.b.create_block());
        let lt = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, c2, 0);
        self.b.ins().brif(lt, fdown, NOARGS, fexit, NOARGS);
        self.b.switch_to_block(fdown);
        self.bi_mul10(ar);
        self.bi_mul10(amp);
        self.bi_mul10(amm);
        let kk = self.ld(kaddr, 0);
        let kk1 = self.b.ins().iadd_imm_s(kk, -1);
        self.st(kk1, kaddr, 0);
        self.b.ins().jump(fhead, NOARGS);
        self.b.switch_to_block(fexit);

        // 桁を作る。
        let digits_slot = self.tmp(DIGITS_MAX);
        let dbase = self.saddr(digits_slot);
        self.st(z, dcaddr, 0);
        let carry = self.b.create_block();
        let dhead = self.b.create_block();
        self.b.ins().jump(dhead, NOARGS);
        self.b.switch_to_block(dhead);
        self.bi_mul10(ar);
        self.bi_mul10(amp);
        self.bi_mul10(amm);
        // 抜き出し（R から S を引ける回数が桁）。
        let xhead = self.b.create_block();
        self.b.append_block_param(xhead, types::I64);
        let xdone = self.b.create_block();
        self.b.append_block_param(xdone, types::I64);
        self.b.ins().jump(xhead, &[BlockArg::Value(z)]);
        self.b.switch_to_block(xhead);
        let dv = self.b.block_params(xhead)[0];
        let c = self.bi_cmp(ar, as_);
        let (xbody, xend) = (self.b.create_block(), self.b.create_block());
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, c, 0);
        self.b.ins().brif(ge, xbody, NOARGS, xend, NOARGS);
        self.b.switch_to_block(xbody);
        self.bi_sub(ar, as_);
        let dv2 = self.b.ins().iadd_imm_s(dv, 1);
        self.b.ins().jump(xhead, &[BlockArg::Value(dv2)]);
        self.b.switch_to_block(xend);
        self.b.ins().jump(xdone, &[BlockArg::Value(dv)]);
        self.b.switch_to_block(xdone);
        let d = self.b.block_params(xdone)[0];
        // 区間の端: 下（余りが Mm 以下）・上（余り + Mp が S 以上）。
        let clow = self.bi_cmp(ar, amm);
        let low_lt = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, clow, 0);
        let low_eq = self.b.ins().icmp_imm_s(IntCC::Equal, clow, 0);
        let low_in = self.b.ins().band(incl, low_eq);
        let low = self.b.ins().bor(low_lt, low_in);
        self.bi_copy(at, ar);
        self.bi_add(at, amp);
        let chi = self.bi_cmp(at, as_);
        let high_gt = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, chi, 0);
        let high_eq = self.b.ins().icmp_imm_s(IntCC::Equal, chi, 0);
        let high_in = self.b.ins().band(incl, high_eq);
        let high = self.b.ins().bor(high_gt, high_in);
        let (lh, l_only) = (self.b.create_block(), self.b.create_block());
        let (h_only, neither) = (self.b.create_block(), self.b.create_block());
        let (lh_t, lh_f) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(low, lh_t, NOARGS, lh_f, NOARGS);
        self.b.switch_to_block(lh_t);
        self.b.ins().brif(high, lh, NOARGS, l_only, NOARGS);
        self.b.switch_to_block(lh_f);
        self.b.ins().brif(high, h_only, NOARGS, neither, NOARGS);
        // 両方: 余りの 2 倍と S で近い方（ちょうど半分なら偶数へ）。
        self.b.switch_to_block(lh);
        self.bi_copy(at, ar);
        self.bi_add(at, ar);
        let c3 = self.bi_cmp(at, as_);
        let d_lsb = self.b.ins().band_imm_s(d, 1);
        let odd = self.b.ins().icmp_imm_s(IntCC::Equal, d_lsb, 1);
        let up_gt = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, c3, 0);
        let up_eq = self.b.ins().icmp_imm_s(IntCC::Equal, c3, 0);
        let up_tie = self.b.ins().band(up_eq, odd);
        let up = self.b.ins().bor(up_gt, up_tie);
        let d_p1 = self.b.ins().iadd_imm_s(d, 1);
        let dfin = self.b.ins().select(up, d_p1, d);
        self.emit_digit(dbase, dcaddr, dfin);
        self.b.ins().jump(carry, NOARGS);
        self.b.switch_to_block(l_only);
        self.emit_digit(dbase, dcaddr, d);
        self.b.ins().jump(carry, NOARGS);
        self.b.switch_to_block(h_only);
        let d1 = self.b.ins().iadd_imm_s(d, 1);
        self.emit_digit(dbase, dcaddr, d1);
        self.b.ins().jump(carry, NOARGS);
        self.b.switch_to_block(neither);
        self.emit_digit(dbase, dcaddr, d);
        self.b.ins().jump(dhead, NOARGS);

        // 繰り上がり（最後の桁が 10 になったときだけ）。
        self.b.switch_to_block(carry);
        let cnt = self.ld(dcaddr, 0);
        let cnt_m1 = self.b.ins().iadd_imm_s(cnt, -1);
        let last_at = self.b.ins().iadd(dbase, cnt_m1);
        let last = self.ld8(last_at);
        let (cgo, conv) = (self.b.create_block(), self.b.create_block());
        let is10 = self.b.ins().icmp_imm_s(IntCC::Equal, last, 10);
        self.b.ins().brif(is10, cgo, NOARGS, conv, NOARGS);
        self.b.switch_to_block(cgo);
        self.st8(z, last_at);
        let chead = self.b.create_block();
        self.b.append_block_param(chead, types::I64);
        let i0 = self.b.ins().iadd_imm_s(cnt, -2);
        self.b.ins().jump(chead, &[BlockArg::Value(i0)]);
        self.b.switch_to_block(chead);
        let ci = self.b.block_params(chead)[0];
        let (call9, cbody) = (self.b.create_block(), self.b.create_block());
        let under = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, ci, 0);
        self.b.ins().brif(under, call9, NOARGS, cbody, NOARGS);
        self.b.switch_to_block(call9);
        let one8 = self.ic(1);
        self.st8(one8, dbase);
        let kk = self.ld(kaddr, 0);
        let kk1 = self.b.ins().iadd_imm_s(kk, 1);
        self.st(kk1, kaddr, 0);
        self.b.ins().jump(conv, NOARGS);
        self.b.switch_to_block(cbody);
        let cat = self.b.ins().iadd(dbase, ci);
        let cv = self.ld8(cat);
        let (c9, cfin) = (self.b.create_block(), self.b.create_block());
        let is9 = self.b.ins().icmp_imm_s(IntCC::Equal, cv, 9);
        self.b.ins().brif(is9, c9, NOARGS, cfin, NOARGS);
        self.b.switch_to_block(c9);
        self.st8(z, cat);
        let ci2 = self.b.ins().iadd_imm_s(ci, -1);
        self.b.ins().jump(chead, &[BlockArg::Value(ci2)]);
        self.b.switch_to_block(cfin);
        let cv1 = self.b.ins().iadd_imm_s(cv, 1);
        self.st8(cv1, cat);
        self.b.ins().jump(conv, NOARGS);

        // 数字の文字へ直す。
        self.b.switch_to_block(conv);
        let ahead = self.b.create_block();
        self.b.append_block_param(ahead, types::I64);
        let layout = self.b.create_block();
        self.b.ins().jump(ahead, &[BlockArg::Value(z)]);
        self.b.switch_to_block(ahead);
        let ai = self.b.block_params(ahead)[0];
        let n_now = self.ld(dcaddr, 0);
        let amore = self.b.ins().icmp(IntCC::SignedLessThan, ai, n_now);
        let abody = self.b.create_block();
        self.b.ins().brif(amore, abody, NOARGS, layout, NOARGS);
        self.b.switch_to_block(abody);
        let aat = self.b.ins().iadd(dbase, ai);
        let av = self.ld8(aat);
        let ach = self.b.ins().iadd_imm_s(av, 48);
        self.st8(ach, aat);
        let ai2 = self.b.ins().iadd_imm_s(ai, 1);
        self.b.ins().jump(ahead, &[BlockArg::Value(ai2)]);

        // 並べ方（補足 A-2 の形: 指数表記は使わない）。
        self.b.switch_to_block(layout);
        let k = self.ld(kaddr, 0);
        let n = self.ld(dcaddr, 0);
        let (lay_a, rest) = (self.b.create_block(), self.b.create_block());
        let kle = self.b.ins().icmp_imm_s(IntCC::SignedLessThanOrEqual, k, 0);
        self.b.ins().brif(kle, lay_a, NOARGS, rest, NOARGS);
        self.b.switch_to_block(lay_a);
        self.sb_lit(b"0.");
        let zeros = self.b.ins().isub(z, k);
        self.sb_zeros(zeros);
        self.scall(SFn::Push, &[dbase, n]);
        self.b.ins().return_(&[]);
        self.b.switch_to_block(rest);
        let (lay_b, lay_c) = (self.b.create_block(), self.b.create_block());
        let kge = self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, k, n);
        self.b.ins().brif(kge, lay_b, NOARGS, lay_c, NOARGS);
        self.b.switch_to_block(lay_b);
        self.scall(SFn::Push, &[dbase, n]);
        let zeros = self.b.ins().isub(k, n);
        self.sb_zeros(zeros);
        self.sb_lit(b".0");
        self.b.ins().return_(&[]);
        self.b.switch_to_block(lay_c);
        self.scall(SFn::Push, &[dbase, k]);
        self.sb_lit(b".");
        let tail = self.b.ins().iadd(dbase, k);
        let tn = self.b.ins().isub(n, k);
        self.scall(SFn::Push, &[tail, tn]);
        self.b.ins().return_(&[]);
    }

    /// 桁を 1 つ置いて数を進める。
    fn emit_digit(&mut self, dbase: Value, dcaddr: Value, d: Value) {
        let cnt = self.ld(dcaddr, 0);
        let at = self.b.ins().iadd(dbase, cnt);
        self.st8(d, at);
        let cnt2 = self.b.ins().iadd_imm_s(cnt, 1);
        self.st(cnt2, dcaddr, 0);
    }
}

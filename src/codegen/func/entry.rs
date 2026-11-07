//! 入口の引数の生成（ADR 0025 補足 I-1・I-2・I-3・I-4・I-5）。
//!
//! 解釈は 2 段のままで、両段とも生成する:
//!
//! - 第 1 段 `sigil_arg_parse(ptr, len, err)`: 引数のバイト列をタグつきのリテラル値に読む。
//!   規則と順序は処理系と評価器が使う共有の `parse_arg`（`src/shared/args.rs`）と完全一致
//!   （真偽値 → 整数 → 数字だけで範囲外なら範囲外の誤り → 小数 → 一覧 → 文字列）。
//!   リテラル値は `[種類][中身…]` の塊（種類は移行前の `sigil_rt_arg_kind` と同じ番号）で、
//!   文字列の中身はバイト列への番地と長さのまま持ち、`RtStr` 化は合わせ込みで型が
//!   `Str`/`Path` に決まったときだけ行う。塊は `raw_alloc` で取り、入口の間だけ生きる。
//! - 第 2 段 `sigil_coerce_<k>(lit, out)`: 型ごとの生成関数がリテラル値を語に変える
//!   （規則は移行前の `runtime/core` の `coerce` と同じ）。失敗のときは、要る型の名前と
//!   渡された値の名前を静的な領域（`Cg::coerce_err_data`）へ書き、thunk が文言を組み立てる。
//!
//! `Float` の解析 `sigil_arg_float(ptr, len, out)` は、文法を Rust の `f64` の `FromStr` と
//! 一致させ（文法の一致そのものが仕様。成否が値の種類を決める）、変換は正確な多倍長演算で
//! 正しく丸める（最近接、同点は偶数）。有効桁 1200 桁まで保持し、それ以降は sticky として
//! 丸めにだけ効かせる。
//!
//! `Str`/`Path` への変換は、既存の `str_new` シム（`std.core.string.new_lossy` の包み、
//! 補足 D-3・I-5）をそのまま使う。言語への追加はない。

use super::{strip, Fx, NOARGS};
use crate::codegen::{norm_with, Gk, SFn};
use crate::ir::Ty;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, BlockArg, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;

/// リテラル値の塊の種類（補足 I-1。移行前の `sigil_rt_arg_kind` と同じ番号）。
const LK_BOOL: i64 = 0;
const LK_INT: i64 = 1;
const LK_FLOAT: i64 = 2;
const LK_STR: i64 = 3;
const LK_LIST: i64 = 4;

/// `Float` の解析の多倍長演算の語の数（64 ビット × 132 = 8448 ビット）。
/// 必要なのは、有効桁 1200 桁（約 3987 ビット）を 10 のべきで拡縮した後でも
/// 高々約 6500 ビット（割り算の被除数の大きさ）まで。
const FW: i64 = 132;
/// 多倍長演算の置き場の大きさ（M・D・R・Q の 4 つ分）。
const FW_BYTES: u32 = (FW * 8 * 4) as u32;
/// 保持する有効桁の上限（補足 I-3）。
const MAX_DIGITS: i64 = 1200;

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 小さな部品

    /// 生成した関数の本体の入口。ブロックを作って切り替え、引数の語を返す。
    fn ehead(&mut self) -> Vec<Value> {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.b.switch_to_block(entry);
        self.b.block_params(entry).to_vec()
    }

    /// 比較して 0/1 の語にする（`icmp` と `b2i` の組み合わせ）。
    fn cmpi(&mut self, cc: IntCC, a: Value, b: Value) -> Value {
        let c = self.b.ins().icmp(cc, a, b);
        self.b2i(c)
    }

    /// 即値と比較して 0/1 の語にする（`icmp_imm_s` と `b2i` の組み合わせ）。
    fn cmpi_imm(&mut self, cc: IntCC, a: Value, b: i64) -> Value {
        let c = self.b.ins().icmp_imm_s(cc, a, b);
        self.b2i(c)
    }

    /// バイトを読む（符号なしで広げる）。
    fn get8(&mut self, addr: Value) -> Value {
        self.b
            .ins()
            .uload8(types::I64, MemFlagsData::trusted(), addr, 0)
    }

    /// リテラルの（番地, 長さ）。
    fn bytes_ref(&mut self, s: &[u8]) -> (Value, Value) {
        let ptr = match self.cg.bytes_data(s) {
            Ok(d) => self.data_addr(d),
            Err(e) => self.fail(e),
        };
        (ptr, self.ic(s.len() as i64))
    }

    /// `raw_alloc` で語の並びを取り、番地を返す（頭は付けない）。
    fn raw_words(&mut self, words: Value) -> Value {
        let bytes = self.b.ins().imul_imm_s(words, 8);
        let eight = self.ic(8);
        self.rt("raw_alloc", &[bytes, eight])
    }

    /// 2 語のリテラル値の塊 `[種類][中身]`。
    fn lit1(&mut self, kind: i64, w: Value) -> Value {
        let two = self.ic(2);
        let blk = self.raw_words(two);
        let k = self.ic(kind);
        self.st(k, blk, 0);
        self.st(w, blk, 8);
        blk
    }

    /// バイト列がリテラル `lit` と一致するか（1/0）。`ci` なら英字の大文字小文字を区別しない
    /// （`lit` は小文字で渡す）。長さが違えば読まない。
    fn match_lit(&mut self, ptr: Value, len: Value, lit: &[u8], ci: bool) -> Value {
        let slot = self.tmp(8);
        let sa = self.saddr(slot);
        let done = self.b.create_block();
        let failb = self.b.create_block();
        let z = self.ic(0);
        self.st(z, sa, 0);
        let eq = self.b.ins().icmp_imm_s(IntCC::Equal, len, lit.len() as i64);
        let mut cur = self.b.create_block();
        self.b.ins().brif(eq, cur, NOARGS, failb, NOARGS);
        for (j, &want) in lit.iter().enumerate() {
            self.b.switch_to_block(cur);
            let at = self.b.ins().iadd_imm_s(ptr, j as i64);
            let byte = self.get8(at);
            let byte = if ci {
                self.b.ins().bor_imm_s(byte, 0x20)
            } else {
                byte
            };
            let e = self.b.ins().icmp_imm_s(IntCC::Equal, byte, want as i64);
            let next = self.b.create_block();
            self.b.ins().brif(e, next, NOARGS, failb, NOARGS);
            cur = next;
        }
        self.b.switch_to_block(cur);
        let one = self.ic(1);
        self.st(one, sa, 0);
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(failb);
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(done);
        self.ld(sa, 0)
    }

    /// C 文字列の長さ（NUL の手前まで）。
    fn cstr_len(&mut self, ptr: Value) -> Value {
        let (head, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b.append_block_param(done, types::I64);
        let z = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let at = self.b.ins().iadd(ptr, i);
        let byte = self.get8(at);
        let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, byte, 0);
        self.b
            .ins()
            .brif(nz, body, &[BlockArg::Value(i)], done, &[BlockArg::Value(i)]);
        self.b.switch_to_block(body);
        let i = self.b.block_params(body)[0];
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(head, &[BlockArg::Value(next)]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }

    /// UTF-8 の継続バイトか（0/1）。
    fn cont_b(&mut self, x: Value) -> Value {
        let lo = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, x, 0x80);
        let hi = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, x, 0xBF);
        let a = self.b2i(lo);
        let b = self.b2i(hi);
        self.b.ins().band(a, b)
    }

    /// 位置 `p`（残り `rem` バイト）の先頭の文字が Unicode の空白かを `out` の 2 語へ書く
    /// （[空白か, バイト数]）。`str::trim` が使う `char::is_whitespace`（White_Space）と同じ集合。
    /// 不正な並びは空白でなく長さ 1 とする（移行前は `new_lossy` の置き換え文字として扱われた）。
    fn ws_at(&mut self, p: Value, rem: Value, out: Value) {
        let b0 = self.get8(p);
        let (end, other) = (self.b.create_block(), self.b.create_block());
        let ascii = self.b.ins().icmp_imm_s(IntCC::UnsignedLessThan, b0, 0x80);
        let (ab, mb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(ascii, ab, NOARGS, mb, NOARGS);
        // ASCII: 09–0D と 20。
        self.b.switch_to_block(ab);
        let mut is = self.ic(0);
        for c in [9i64, 10, 11, 12, 13, 32] {
            let e = self.b.ins().icmp_imm_s(IntCC::Equal, b0, c);
            let v = self.b2i(e);
            is = self.b.ins().bor(is, v);
        }
        self.st(is, out, 0);
        let one = self.ic(1);
        self.st(one, out, 8);
        self.b.ins().jump(end, NOARGS);
        // 多バイト: 2 バイトか 3 バイトだけが空白を含み、4 バイトには空白がない。
        self.b.switch_to_block(mb);
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b0, 0xC2);
        let le = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, b0, 0xDF);
        let (ge, le) = (self.b2i(ge), self.b2i(le));
        let is2 = self.b.ins().band(ge, le);
        let (two, chk3) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is2, two, NOARGS, chk3, NOARGS);
        self.b.switch_to_block(chk3);
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, b0, 0xE0);
        let le = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, b0, 0xEF);
        let (ge, le) = (self.b2i(ge), self.b2i(le));
        let is3 = self.b.ins().band(ge, le);
        let three = self.b.create_block();
        self.b.ins().brif(is3, three, NOARGS, other, NOARGS);
        // その他（継続バイト・4 バイトの先頭・不正な先頭）は空白でない。
        self.b.switch_to_block(other);
        let z = self.ic(0);
        let one = self.ic(1);
        self.st(z, out, 0);
        self.st(one, out, 8);
        self.b.ins().jump(end, NOARGS);
        // 2 バイト: 符号点は 0085（NEL）と 00A0（NBSP）だけが空白。
        self.b.switch_to_block(two);
        let enough = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, rem, 2);
        let two_ok = self.b.create_block();
        self.b.ins().brif(enough, two_ok, NOARGS, other, NOARGS);
        self.b.switch_to_block(two_ok);
        let p1 = self.b.ins().iadd_imm_s(p, 1);
        let b1 = self.get8(p1);
        let valid = self.cont_b(b1);
        let hi = self.b.ins().band_imm_s(b0, 0x1F);
        let hi = self.b.ins().ishl_imm_s(hi, 6);
        let lo = self.b.ins().band_imm_s(b1, 0x3F);
        let cp = self.b.ins().bor(hi, lo);
        let e1 = self.b.ins().icmp_imm_s(IntCC::Equal, cp, 0x85);
        let e2 = self.b.ins().icmp_imm_s(IntCC::Equal, cp, 0xA0);
        let (e1, e2) = (self.b2i(e1), self.b2i(e2));
        let e = self.b.ins().bor(e1, e2);
        let is = self.b.ins().band(valid, e);
        let vc = self.b.ins().icmp_imm_s(IntCC::NotEqual, valid, 0);
        let (two_v, one_v) = (self.ic(2), self.ic(1));
        let blen = self.b.ins().select(vc, two_v, one_v);
        self.st(is, out, 0);
        self.st(blen, out, 8);
        self.b.ins().jump(end, NOARGS);
        // 3 バイト: 空白は 1680・2000–200A・2028・2029・202F・205F・3000。
        self.b.switch_to_block(three);
        let enough = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, rem, 3);
        let three_ok = self.b.create_block();
        self.b.ins().brif(enough, three_ok, NOARGS, other, NOARGS);
        self.b.switch_to_block(three_ok);
        let p1 = self.b.ins().iadd_imm_s(p, 1);
        let p2 = self.b.ins().iadd_imm_s(p, 2);
        let b1 = self.get8(p1);
        let b2 = self.get8(p2);
        let v1 = self.cont_b(b1);
        let v2 = self.cont_b(b2);
        let mut valid = self.b.ins().band(v1, v2);
        // 過長（E0 で第 2 バイトが A0 未満）と代理対（ED で第 2 バイトが 9F 超）は不正。
        let e0 = self.b.ins().icmp_imm_s(IntCC::Equal, b0, 0xE0);
        let low = self.b.ins().icmp_imm_s(IntCC::UnsignedLessThan, b1, 0xA0);
        let (e0, low) = (self.b2i(e0), self.b2i(low));
        let bad1 = self.b.ins().band(e0, low);
        let ed = self.b.ins().icmp_imm_s(IntCC::Equal, b0, 0xED);
        let high = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThan, b1, 0x9F);
        let (ed, high) = (self.b2i(ed), self.b2i(high));
        let bad2 = self.b.ins().band(ed, high);
        let bad = self.b.ins().bor(bad1, bad2);
        let notbad = self.b.ins().bxor_imm_s(bad, 1);
        valid = self.b.ins().band(valid, notbad);
        let h = self.b.ins().band_imm_s(b0, 0x0F);
        let h = self.b.ins().ishl_imm_s(h, 12);
        let m = self.b.ins().band_imm_s(b1, 0x3F);
        let m = self.b.ins().ishl_imm_s(m, 6);
        let l = self.b.ins().band_imm_s(b2, 0x3F);
        let hm = self.b.ins().bor(h, m);
        let cp = self.b.ins().bor(hm, l);
        let mut e = self.ic(0);
        for c in [0x1680i64, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000] {
            let eq = self.b.ins().icmp_imm_s(IntCC::Equal, cp, c);
            let v = self.b2i(eq);
            e = self.b.ins().bor(e, v);
        }
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, cp, 0x2000);
        let le = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, cp, 0x200A);
        let (ge, le) = (self.b2i(ge), self.b2i(le));
        let inr = self.b.ins().band(ge, le);
        e = self.b.ins().bor(e, inr);
        let is = self.b.ins().band(valid, e);
        let vc = self.b.ins().icmp_imm_s(IntCC::NotEqual, valid, 0);
        let (three_v, one_v) = (self.ic(3), self.ic(1));
        let blen = self.b.ins().select(vc, three_v, one_v);
        self.st(is, out, 0);
        self.st(blen, out, 8);
        self.b.ins().jump(end, NOARGS);
        self.b.switch_to_block(end);
    }

    /// 前後の空白を取り除いた（番地, 長さ）を `out` の 2 語へ書く（`str::trim` と同じ）。
    fn trim_into(&mut self, ptr: Value, len: Value, out: Value) {
        let ws = self.tmp(16);
        let wsa = self.saddr(ws);
        let soff = self.tmp(8);
        let soffa = self.saddr(soff);
        // 前から: 空白の間は進める。
        let (head, body, fdone) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b.append_block_param(fdone, types::I64);
        let z = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let off = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, off, len);
        self.b.ins().brif(
            more,
            body,
            &[BlockArg::Value(off)],
            fdone,
            &[BlockArg::Value(off)],
        );
        self.b.switch_to_block(body);
        let off = self.b.block_params(body)[0];
        let at = self.b.ins().iadd(ptr, off);
        let rem = self.b.ins().isub(len, off);
        self.ws_at(at, rem, wsa);
        let is = self.ld(wsa, 0);
        let (adv, stop) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is, adv, NOARGS, stop, NOARGS);
        self.b.switch_to_block(stop);
        self.b.ins().jump(fdone, &[BlockArg::Value(off)]);
        self.b.switch_to_block(adv);
        let blen = self.ld(wsa, 8);
        let next = self.b.ins().iadd(off, blen);
        self.b.ins().jump(head, &[BlockArg::Value(next)]);
        // 後ろから: 末尾の文字が空白なら縮める。
        self.b.switch_to_block(fdone);
        let start_off = self.b.block_params(fdone)[0];
        self.st(start_off, soffa, 0);
        let (head2, body2, fin) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head2, types::I64);
        self.b.append_block_param(body2, types::I64);
        self.b.append_block_param(fin, types::I64);
        self.b.ins().jump(head2, &[BlockArg::Value(len)]);
        self.b.switch_to_block(head2);
        let end = self.b.block_params(head2)[0];
        let s0 = self.ld(soffa, 0);
        let more = self.b.ins().icmp(IntCC::SignedGreaterThan, end, s0);
        self.b.ins().brif(
            more,
            body2,
            &[BlockArg::Value(end)],
            fin,
            &[BlockArg::Value(end)],
        );
        self.b.switch_to_block(body2);
        let end = self.b.block_params(body2)[0];
        // 末尾の文字の先頭バイトを探す（継続バイトの間は戻る。高々 3 バイト）。
        let (khead, kbody, kdone) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(khead, types::I64);
        self.b.append_block_param(kbody, types::I64);
        self.b.append_block_param(kdone, types::I64);
        let k0 = self.b.ins().iadd_imm_s(end, -1);
        self.b.ins().jump(khead, &[BlockArg::Value(k0)]);
        self.b.switch_to_block(khead);
        let k = self.b.block_params(khead)[0];
        let kat = self.b.ins().iadd(ptr, k);
        let byte = self.get8(kat);
        let cont = self.cont_b(byte);
        let s0 = self.ld(soffa, 0);
        let above = self.b.ins().icmp(IntCC::SignedGreaterThan, k, s0);
        let span = self.b.ins().isub(end, k);
        let narrow = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, span, 4);
        let above = self.b2i(above);
        let back = self.b.ins().band(cont, above);
        let narrow = self.b2i(narrow);
        let back = self.b.ins().band(back, narrow);
        self.b.ins().brif(
            back,
            kbody,
            &[BlockArg::Value(k)],
            kdone,
            &[BlockArg::Value(k)],
        );
        self.b.switch_to_block(kbody);
        let k = self.b.block_params(kbody)[0];
        let prev = self.b.ins().iadd_imm_s(k, -1);
        self.b.ins().jump(khead, &[BlockArg::Value(prev)]);
        self.b.switch_to_block(kdone);
        let k = self.b.block_params(kdone)[0];
        let kat = self.b.ins().iadd(ptr, k);
        let rem = self.b.ins().isub(end, k);
        self.ws_at(kat, rem, wsa);
        let is = self.ld(wsa, 0);
        let blen = self.ld(wsa, 8);
        let kend = self.b.ins().iadd(k, blen);
        let exact = self.b.ins().icmp(IntCC::Equal, kend, end);
        let exact = self.b2i(exact);
        let shrink = self.b.ins().band(is, exact);
        let finb = self.b.create_block();
        self.b
            .ins()
            .brif(shrink, head2, &[BlockArg::Value(k)], finb, NOARGS);
        self.b.switch_to_block(finb);
        self.b.ins().jump(fin, &[BlockArg::Value(end)]);
        self.b.switch_to_block(fin);
        let end = self.b.block_params(fin)[0];
        let s0 = self.ld(soffa, 0);
        let np = self.b.ins().iadd(ptr, s0);
        let nl = self.b.ins().isub(end, s0);
        self.st(np, out, 0);
        self.st(nl, out, 8);
    }
}

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 第 1 段: リテラルの読み取り（補足 I-2）

    /// `sigil_arg_free(lit)`: リテラル値の塊を片づける。一覧は要素の塊を先に再帰で
    /// 片づけてから、塊そのものを返す。大きさは読み取りで取った語数と一致させる
    /// （真偽値・整数・小数は 2 語、文字列は 3 語、一覧は要素数 + 2 語）。
    /// 合わせ込みは塊の中身を借りない（数値はコピー、文字列と一覧は新しく作られる）
    /// ので、合わせ込みが終わった後に呼んでよい（補足 I-1「入口の間だけ生きる」）。
    pub(crate) fn arg_free_body(&mut self) {
        let ps = self.ehead();
        let lit = ps[0];
        let kind = self.ld(lit, 0);
        let is_list = self.cmpi_imm(IntCC::Equal, kind, LK_LIST);
        let bytes = self.branch(
            is_list,
            |s| {
                let cnt = s.ld(lit, 8);
                s.loop_range(cnt, |s2, i| {
                    let off8 = s2.b.ins().imul_imm_s(i, 8);
                    let off = s2.b.ins().iadd_imm_s(off8, 16);
                    let addr = s2.b.ins().iadd(lit, off);
                    let elit = s2.ld(addr, 0);
                    s2.scall(SFn::ArgFree, &[elit]);
                });
                let nw = s.b.ins().iadd_imm_s(cnt, 2);
                s.b.ins().imul_imm_s(nw, 8)
            },
            |s| {
                let is_str = s.cmpi_imm(IntCC::Equal, kind, LK_STR);
                let b24 = s.ic(24);
                let b16 = s.ic(16);
                s.b.ins().select(is_str, b24, b16)
            },
        );
        let eight = self.ic(8);
        self.rt("raw_free", &[lit, bytes, eight]);
        self.b.ins().return_(&[]);
    }

    /// `sigil_arg_parse(ptr, len, err) -> lit`: 引数 1 つをリテラル値の塊に読む。
    /// 範囲外の誤りなら 0 を返し、`err` の 2 語へ対象のバイト列を書く。
    pub(crate) fn arg_parse_body(&mut self) {
        let ps = self.ehead();
        let (ptr, len, err) = (ps[0], ps[1], ps[2]);

        // 真偽値。
        let is_true = self.match_lit(ptr, len, b"true", false);
        let (tb, rest1) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is_true, tb, NOARGS, rest1, NOARGS);
        self.b.switch_to_block(tb);
        let one = self.ic(1);
        let lit = self.lit1(LK_BOOL, one);
        self.b.ins().return_(&[lit]);
        self.b.switch_to_block(rest1);
        let is_false = self.match_lit(ptr, len, b"false", false);
        let (fb, rest2) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is_false, fb, NOARGS, rest2, NOARGS);
        self.b.switch_to_block(fb);
        let zero = self.ic(0);
        let lit = self.lit1(LK_BOOL, zero);
        self.b.ins().return_(&[lit]);
        self.b.switch_to_block(rest2);

        // 整数の走査（符号・桁・大きさ）。`i64` の `parse` と一致させる:
        // 符号は先頭の 1 つだけ、桁は ASCII の数字だけ、大きさは符号側の上限まで。
        let mag = self.tmp(8);
        let maga = self.saddr(mag);
        let alldig = self.tmp(8);
        let alldiga = self.saddr(alldig);
        let ovf = self.tmp(8);
        let ovfa = self.saddr(ovf);
        let z = self.ic(0);
        self.st(z, maga, 0);
        let one = self.ic(1);
        self.st(one, alldiga, 0);
        self.st(z, ovfa, 0);
        let cond = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, len, 1);
        let b0 = self.branch(cond, |s| s.get8(ptr), |s| s.ic(0));
        let negc = self.b.ins().icmp_imm_s(IntCC::Equal, b0, b'-' as i64);
        let plusc = self.b.ins().icmp_imm_s(IntCC::Equal, b0, b'+' as i64);
        let sgn = self.b.ins().bor(negc, plusc);
        let start = self.b2i(sgn);
        let negv = self.b2i(negc);
        // 大きさの上限: 負なら 2^63、正なら 2^63-1（符号なしとして扱う）。
        let (lim_neg, lim_pos) = (self.ic(i64::MIN), self.ic(i64::MAX));
        let limit = self.b.ins().select(negc, lim_neg, lim_pos);
        let (head, body, after) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b.ins().jump(head, &[BlockArg::Value(start)]);
        self.b.switch_to_block(head);
        let i = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, len);
        self.b
            .ins()
            .brif(more, body, &[BlockArg::Value(i)], after, NOARGS);
        self.b.switch_to_block(body);
        let i = self.b.block_params(body)[0];
        let at = self.b.ins().iadd(ptr, i);
        let byte = self.get8(at);
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, byte, b'0' as i64);
        let le = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, byte, b'9' as i64);
        let (ge, le) = (self.b2i(ge), self.b2i(le));
        let isd = self.b.ins().band(ge, le);
        let d = self.b.ins().iadd_imm_s(byte, -(b'0' as i64));
        let (clr, keep) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(isd, keep, NOARGS, clr, NOARGS);
        self.b.switch_to_block(clr);
        let z = self.ic(0);
        self.st(z, alldiga, 0);
        self.b.ins().jump(keep, NOARGS);
        self.b.switch_to_block(keep);
        let ovfv = self.ld(ovfa, 0);
        let noovf = self.cmpi_imm(IntCC::Equal, ovfv, 0);
        let do_acc = self.b.ins().band(isd, noovf);
        let acc = self.branch(
            do_acc,
            |s| {
                let magv = s.ld(maga, 0);
                let dm = s.b.ins().isub(limit, d);
                let ten = s.ic(10);
                let q = s.b.ins().udiv(dm, ten);
                let over = s.cmpi(IntCC::UnsignedGreaterThan, magv, q);
                s.branch(
                    over,
                    |s| {
                        let one = s.ic(1);
                        s.st(one, ovfa, 0);
                        s.ic(0)
                    },
                    |s| {
                        let m10 = s.b.ins().imul_imm_s(magv, 10);
                        let nm = s.b.ins().iadd(m10, d);
                        s.st(nm, maga, 0);
                        s.ic(0)
                    },
                )
            },
            |s| s.ic(0),
        );
        let _ = acc;
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.b.ins().jump(head, &[BlockArg::Value(next)]);
        self.b.switch_to_block(after);

        let ndig = self.b.ins().isub(len, start);
        let alldigv = self.ld(alldiga, 0);
        let ovfv = self.ld(ovfa, 0);
        let has_dig = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, ndig, 1);
        let no_ovf = self.cmpi_imm(IntCC::Equal, ovfv, 0);
        let int_ok = self.b.ins().band(alldigv, has_dig);
        let int_ok = self.b.ins().band(int_ok, no_ovf);
        let (intb, rest3) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(int_ok, intb, NOARGS, rest3, NOARGS);
        self.b.switch_to_block(intb);
        let magv = self.ld(maga, 0);
        let z = self.ic(0);
        let negated = self.b.ins().isub(z, magv);
        let v = self.b.ins().select(negc, negated, magv);
        let lit = self.lit1(LK_INT, v);
        self.b.ins().return_(&[lit]);

        // 範囲外の誤り: 先頭の `-` を 1 つだけ外した残り（符号がなければ全体）が
        // 空でなく全部数字なら、整数が範囲外（共有の `parse_arg` と同じ条件）。
        self.b.switch_to_block(rest3);
        let ge2 = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, len, 2);
        let r1 = self.b.ins().band(ge2, alldigv);
        let r1 = self.b.ins().band(negv, r1);
        let one = self.ic(1);
        let nosign = self.b.ins().isub(one, start);
        let r2 = self.b.ins().band(alldigv, has_dig);
        let r2 = self.b.ins().band(nosign, r2);
        let range_err = self.b.ins().bor(r1, r2);
        let (errb, rest4) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(range_err, errb, NOARGS, rest4, NOARGS);
        self.b.switch_to_block(errb);
        self.st(ptr, err, 0);
        self.st(len, err, 8);
        let z = self.ic(0);
        self.b.ins().return_(&[z]);

        // 小数。
        self.b.switch_to_block(rest4);
        let out = self.tmp(8);
        let outa = self.saddr(out);
        let fok = self.scall(SFn::ArgFloat, &[ptr, len, outa]);
        let (flb, rest5) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(fok, flb, NOARGS, rest5, NOARGS);
        self.b.switch_to_block(flb);
        let bits = self.ld(outa, 0);
        let lit = self.lit1(LK_FLOAT, bits);
        self.b.ins().return_(&[lit]);

        // 一覧（前後が `[` と `]`）か、文字列か。
        self.b.switch_to_block(rest5);
        let cond = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, len, 1);
        let last = self.branch(
            cond,
            |s| {
                let e = s.b.ins().iadd_imm_s(len, -1);
                let at = s.b.ins().iadd(ptr, e);
                s.get8(at)
            },
            |s| s.ic(0),
        );
        let ge2 = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, len, 2);
        let lb = self.cmpi_imm(IntCC::Equal, b0, b'[' as i64);
        let rb = self.cmpi_imm(IntCC::Equal, last, b']' as i64);
        let is_list = self.b.ins().band(ge2, lb);
        let is_list = self.b.ins().band(is_list, rb);
        let (listb, strb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(is_list, listb, NOARGS, strb, NOARGS);
        self.b.switch_to_block(strb);
        let three = self.ic(3);
        let blk = self.raw_words(three);
        let k = self.ic(LK_STR);
        self.st(k, blk, 0);
        self.st(ptr, blk, 8);
        self.st(len, blk, 16);
        self.b.ins().return_(&[blk]);

        // 一覧: 中身をカンマで割り（入れ子のカンマでも割れる癖まで再現）、
        // 要素ごとに前後の空白を取り除いて再帰する。
        self.b.switch_to_block(listb);
        let iptr = self.b.ins().iadd_imm_s(ptr, 1);
        let ilen = self.b.ins().iadd_imm_s(len, -2);
        let tslot = self.tmp(16);
        let ta = self.saddr(tslot);
        self.trim_into(iptr, ilen, ta);
        let tlen = self.ld(ta, 8);
        let empty = self.b.ins().icmp_imm_s(IntCC::Equal, tlen, 0);
        let (emptyb, countb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(empty, emptyb, NOARGS, countb, NOARGS);
        self.b.switch_to_block(emptyb);
        let two = self.ic(2);
        let blk = self.raw_words(two);
        let k = self.ic(LK_LIST);
        self.st(k, blk, 0);
        let z = self.ic(0);
        self.st(z, blk, 8);
        self.b.ins().return_(&[blk]);
        self.b.switch_to_block(countb);
        let cnt = self.tmp(8);
        let cnta = self.saddr(cnt);
        let one = self.ic(1);
        self.st(one, cnta, 0);
        self.loop_range(ilen, |s, j| {
            let at = s.b.ins().iadd(iptr, j);
            let byte = s.get8(at);
            let eq = s.cmpi_imm(IntCC::Equal, byte, b',' as i64);
            s.branch(
                eq,
                |s| {
                    let c = s.ld(cnta, 0);
                    let c1 = s.b.ins().iadd_imm_s(c, 1);
                    s.st(c1, cnta, 0);
                    s.ic(0)
                },
                |s| s.ic(0),
            );
        });
        let cntv = self.ld(cnta, 0);
        let nwords = self.b.ins().iadd_imm_s(cntv, 2);
        let blk = self.raw_words(nwords);
        let k = self.ic(LK_LIST);
        self.st(k, blk, 0);
        self.st(cntv, blk, 8);
        let seg = self.tmp(8);
        let sega = self.saddr(seg);
        let kk = self.tmp(8);
        let kka = self.saddr(kk);
        let z = self.ic(0);
        self.st(z, sega, 0);
        self.st(z, kka, 0);
        let (shead, sbody, sdone) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(shead, types::I64);
        self.b.append_block_param(sbody, types::I64);
        let z = self.ic(0);
        self.b.ins().jump(shead, &[BlockArg::Value(z)]);
        self.b.switch_to_block(shead);
        let j = self.b.block_params(shead)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThanOrEqual, j, ilen);
        self.b
            .ins()
            .brif(more, sbody, &[BlockArg::Value(j)], sdone, NOARGS);
        self.b.switch_to_block(sbody);
        let j = self.b.block_params(sbody)[0];
        let at_end = self.b.ins().icmp(IntCC::Equal, j, ilen);
        let sep = self.branch(
            at_end,
            |s| s.ic(1),
            |s| {
                let at = s.b.ins().iadd(iptr, j);
                let byte = s.get8(at);
                s.cmpi_imm(IntCC::Equal, byte, b',' as i64)
            },
        );
        let (sepb, contb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(sep, sepb, NOARGS, contb, NOARGS);
        self.b.switch_to_block(contb);
        let next = self.b.ins().iadd_imm_s(j, 1);
        self.b.ins().jump(shead, &[BlockArg::Value(next)]);
        self.b.switch_to_block(sepb);
        let ss = self.ld(sega, 0);
        let sl = self.b.ins().isub(j, ss);
        let sp0 = self.b.ins().iadd(iptr, ss);
        self.trim_into(sp0, sl, ta);
        let sp = self.ld(ta, 0);
        let sl2 = self.ld(ta, 8);
        let elit = self.scall(SFn::ArgParse, &[sp, sl2, err]);
        let isz = self.b.ins().icmp_imm_s(IntCC::Equal, elit, 0);
        let (failb, storeb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(isz, failb, NOARGS, storeb, NOARGS);
        self.b.switch_to_block(failb);
        let z = self.ic(0);
        self.b.ins().return_(&[z]);
        self.b.switch_to_block(storeb);
        let kv = self.ld(kka, 0);
        let off8 = self.b.ins().imul_imm_s(kv, 8);
        let off = self.b.ins().iadd_imm_s(off8, 16);
        let addr = self.b.ins().iadd(blk, off);
        self.st(elit, addr, 0);
        let kv1 = self.b.ins().iadd_imm_s(kv, 1);
        self.st(kv1, kka, 0);
        let next_seg = self.b.ins().iadd_imm_s(j, 1);
        self.st(next_seg, sega, 0);
        self.b.ins().jump(shead, &[BlockArg::Value(next_seg)]);
        self.b.switch_to_block(sdone);
        self.b.ins().return_(&[blk]);
    }
}

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 多倍長演算（`Float` の解析用。置き場は作業領域の中の語の並び）

    /// 語 `idx` を読む。
    fn bw_ld(&mut self, base: Value, idx: Value) -> Value {
        let off = self.b.ins().imul_imm_s(idx, 8);
        let at = self.b.ins().iadd(base, off);
        self.ld(at, 0)
    }

    /// 語 `idx` へ書く。
    fn bw_st(&mut self, base: Value, idx: Value, v: Value) {
        let off = self.b.ins().imul_imm_s(idx, 8);
        let at = self.b.ins().iadd(base, off);
        self.st(v, at, 0);
    }

    /// `base *= 10; base += d0`（`d0` は 0–9。繰り上がりは高々 10）。
    fn bw_mul10add(&mut self, base: Value, d0: Value) {
        let (head, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b.append_block_param(body, types::I64);
        let z = self.ic(0);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(z), BlockArg::Value(d0)]);
        self.b.switch_to_block(head);
        let w = self.b.block_params(head)[0];
        let carry = self.b.block_params(head)[1];
        let more = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, w, FW);
        self.b.ins().brif(
            more,
            body,
            &[BlockArg::Value(w), BlockArg::Value(carry)],
            done,
            NOARGS,
        );
        self.b.switch_to_block(body);
        let w = self.b.block_params(body)[0];
        let carry = self.b.block_params(body)[1];
        let cur = self.bw_ld(base, w);
        let ten = self.ic(10);
        let hi = self.b.ins().umulhi(cur, ten);
        let lo = self.b.ins().imul_imm_s(cur, 10);
        let sum = self.b.ins().iadd(lo, carry);
        let wrapped = self.cmpi(IntCC::UnsignedLessThan, sum, lo);
        let carry2 = self.b.ins().iadd(hi, wrapped);
        self.bw_st(base, w, sum);
        let next = self.b.ins().iadd_imm_s(w, 1);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(next), BlockArg::Value(carry2)]);
        self.b.switch_to_block(done);
    }

    /// 1 語のビット長。
    fn word_bitlen(&mut self, x: Value) -> Value {
        let (head, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b.append_block_param(done, types::I64);
        let z = self.ic(0);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(x), BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let t = self.b.block_params(head)[0];
        let n = self.b.block_params(head)[1];
        let nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, t, 0);
        self.b.ins().brif(
            nz,
            body,
            &[BlockArg::Value(t), BlockArg::Value(n)],
            done,
            &[BlockArg::Value(n)],
        );
        self.b.switch_to_block(body);
        let t = self.b.block_params(body)[0];
        let n = self.b.block_params(body)[1];
        let t2 = self.b.ins().ushr_imm_s(t, 1);
        let n2 = self.b.ins().iadd_imm_s(n, 1);
        self.b
            .ins()
            .jump(head, &[BlockArg::Value(t2), BlockArg::Value(n2)]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }

    /// 多倍長のビット長（0 なら 0）。
    fn bw_bitlen(&mut self, base: Value) -> Value {
        let res = self.tmp(8);
        let resa = self.saddr(res);
        let z = self.ic(0);
        self.st(z, resa, 0);
        let n = self.ic(FW);
        self.loop_range(n, |s, w| {
            let word = s.bw_ld(base, w);
            let nz = s.cmpi_imm(IntCC::NotEqual, word, 0);
            s.branch(
                nz,
                |s| {
                    let m64 = s.b.ins().imul_imm_s(w, 64);
                    let bl = s.word_bitlen(word);
                    let tot = s.b.ins().iadd(m64, bl);
                    s.st(tot, resa, 0);
                    s.ic(0)
                },
                |s| s.ic(0),
            );
        });
        self.ld(resa, 0)
    }

    /// 多倍長が 0 か（1/0）。
    fn bw_is_zero(&mut self, base: Value) -> Value {
        let acc = self.tmp(8);
        let acca = self.saddr(acc);
        let z = self.ic(0);
        self.st(z, acca, 0);
        let n = self.ic(FW);
        self.loop_range(n, |s, w| {
            let word = s.bw_ld(base, w);
            let a = s.ld(acca, 0);
            let a2 = s.b.ins().bor(a, word);
            s.st(a2, acca, 0);
        });
        let a = self.ld(acca, 0);
        self.cmpi_imm(IntCC::Equal, a, 0)
    }

    /// 多倍長の複写。
    fn bw_copy(&mut self, dst: Value, src: Value) {
        let n = self.ic(FW);
        self.loop_range(n, |s, w| {
            let word = s.bw_ld(src, w);
            s.bw_st(dst, w, word);
        });
    }

    /// `base << i` の語 `w`（範囲外は 0）。
    fn bw_shifted_word(&mut self, base: Value, i: Value, w: Value) -> Value {
        let ws = self.b.ins().ushr_imm_s(i, 6);
        let bs = self.b.ins().band_imm_s(i, 63);
        let idx = self.b.ins().isub(w, ws);
        let ge = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, idx, 0);
        let lt = self.cmpi_imm(IntCC::SignedLessThan, idx, FW);
        let inr = self.b.ins().band(ge, lt);
        self.branch(
            inr,
            |s| {
                let word = s.bw_ld(base, idx);
                let a = s.b.ins().ishl(word, bs);
                let bs_nz = s.cmpi_imm(IntCC::NotEqual, bs, 0);
                let idx_pos = s.cmpi_imm(IntCC::SignedGreaterThan, idx, 0);
                let need = s.b.ins().band(bs_nz, idx_pos);
                s.branch(
                    need,
                    |s| {
                        let prev = s.b.ins().iadd_imm_s(idx, -1);
                        let word2 = s.bw_ld(base, prev);
                        let c64 = s.ic(64);
                        let sh = s.b.ins().isub(c64, bs);
                        let hi = s.b.ins().ushr(word2, sh);
                        s.b.ins().bor(a, hi)
                    },
                    |_| a,
                )
            },
            |s| s.ic(0),
        )
    }

    /// `dst = src << s`（全語を書く）。
    fn bw_shl(&mut self, dst: Value, src: Value, s: Value) {
        let n = self.ic(FW);
        self.loop_range(n, |slf, w| {
            let v = slf.bw_shifted_word(src, s, w);
            slf.bw_st(dst, w, v);
        });
    }

    /// `r` と `d << i` の比較（-1/0/1）。`bl_d` は `d` のビット長。
    fn bw_cmp_shifted(&mut self, r: Value, d: Value, i: Value, bl_d: Value) -> Value {
        let res = self.tmp(8);
        let resa = self.saddr(res);
        let bl_r = self.bw_bitlen(r);
        let tgt = self.b.ins().iadd(bl_d, i);
        let lt = self.b.ins().icmp(IntCC::SignedLessThan, bl_r, tgt);
        let gt = self.b.ins().icmp(IntCC::SignedGreaterThan, bl_r, tgt);
        let (ltb, nltb, gtb, scanb, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().brif(lt, ltb, NOARGS, nltb, NOARGS);
        self.b.switch_to_block(ltb);
        let m1 = self.ic(-1);
        self.st(m1, resa, 0);
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(nltb);
        self.b.ins().brif(gt, gtb, NOARGS, scanb, NOARGS);
        self.b.switch_to_block(gtb);
        let one = self.ic(1);
        self.st(one, resa, 0);
        self.b.ins().jump(done, NOARGS);
        // ビット長が等しい: 最上位の語から比べ、最初に違う語で決まる。
        self.b.switch_to_block(scanb);
        let z = self.ic(0);
        self.st(z, resa, 0);
        let tm1 = self.b.ins().iadd_imm_s(tgt, -1);
        let tw = self.b.ins().ushr_imm_s(tm1, 6);
        let steps = self.b.ins().iadd_imm_s(tw, 1);
        self.loop_range(steps, |s, c| {
            let w = s.b.ins().isub(tw, c);
            let cur = s.ld(resa, 0);
            let still = s.cmpi_imm(IntCC::Equal, cur, 0);
            s.branch(
                still,
                |s| {
                    let a = s.bw_ld(r, w);
                    let b = s.bw_shifted_word(d, i, w);
                    let ne = s.cmpi(IntCC::NotEqual, a, b);
                    s.branch(
                        ne,
                        |s| {
                            let g = s.b.ins().icmp(IntCC::UnsignedGreaterThan, a, b);
                            let (one, mone) = (s.ic(1), s.ic(-1));
                            let v = s.b.ins().select(g, one, mone);
                            s.st(v, resa, 0);
                            s.ic(0)
                        },
                        |s| s.ic(0),
                    )
                },
                |s| s.ic(0),
            );
        });
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(done);
        self.ld(resa, 0)
    }

    /// `r -= d << i`（`r >= d << i` のときにだけ呼ぶ）。
    fn bw_sub_shifted(&mut self, r: Value, d: Value, i: Value) {
        let bor = self.tmp(8);
        let bora = self.saddr(bor);
        let z = self.ic(0);
        self.st(z, bora, 0);
        let n = self.ic(FW);
        self.loop_range(n, |s, w| {
            let b = s.bw_shifted_word(d, i, w);
            let cur = s.bw_ld(r, w);
            let bin = s.ld(bora, 0);
            let t1 = s.b.ins().isub(cur, bin);
            let b1 = s.cmpi(IntCC::UnsignedLessThan, cur, bin);
            let t = s.b.ins().isub(t1, b);
            let b2 = s.cmpi(IntCC::UnsignedLessThan, t1, b);
            s.bw_st(r, w, t);
            let nb = s.b.ins().bor(b1, b2);
            s.st(nb, bora, 0);
        });
    }

    /// `base` のビット `idx`（0/1）。`idx` は範囲内であること。
    fn bw_bit(&mut self, base: Value, idx: Value) -> Value {
        let w = self.b.ins().ushr_imm_s(idx, 6);
        let word = self.bw_ld(base, w);
        let sh = self.b.ins().band_imm_s(idx, 63);
        let v = self.b.ins().ushr(word, sh);
        self.b.ins().band_imm_s(v, 1)
    }
}

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 第 1 段の補助: 数字の並びの走査と、多倍長への桁の送り込み

    /// 位置の置き場 `i_a` が指すところから、ASCII の数字が続く間だけ進める。
    fn digit_run(&mut self, ptr: Value, len: Value, i_a: Value) {
        let (head, chk, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(head);
        let i = self.ld(i_a, 0);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, len);
        self.b.ins().brif(more, chk, NOARGS, done, NOARGS);
        self.b.switch_to_block(chk);
        let i = self.ld(i_a, 0);
        let at = self.b.ins().iadd(ptr, i);
        let byte = self.get8(at);
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, byte, b'0' as i64);
        let le = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, byte, b'9' as i64);
        let (ge, le) = (self.b2i(ge), self.b2i(le));
        let isd = self.b.ins().band(ge, le);
        self.b.ins().brif(isd, body, NOARGS, done, NOARGS);
        self.b.switch_to_block(body);
        let i = self.ld(i_a, 0);
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.st(next, i_a, 0);
        self.b.ins().jump(head, NOARGS);
        self.b.switch_to_block(done);
    }

    /// 数字の並び `base[0..count]` を多倍長 `m` へ送る（補足 I-3）。
    /// 先行する 0 は数えず、有効桁 `MAX_DIGITS` 桁までを保持し、落とした桁は
    /// `dropped` に数え、0 でない桁を落としたら `sticky` を立てる。
    fn feed_range(
        &mut self,
        m: Value,
        base: Value,
        count: Value,
        kept_a: Value,
        dropped_a: Value,
        sticky_a: Value,
    ) {
        self.loop_range(count, |s, j| {
            let at = s.b.ins().iadd(base, j);
            let byte = s.get8(at);
            let d = s.b.ins().iadd_imm_s(byte, -(b'0' as i64));
            let k = s.ld(kept_a, 0);
            let kz = s.cmpi_imm(IntCC::Equal, k, 0);
            let dz = s.cmpi_imm(IntCC::Equal, d, 0);
            let lead = s.b.ins().band(kz, dz);
            s.branch(
                lead,
                |s| s.ic(0),
                |s| {
                    let lt = s.cmpi_imm(IntCC::SignedLessThan, k, MAX_DIGITS);
                    s.branch(
                        lt,
                        |s| {
                            s.bw_mul10add(m, d);
                            let k1 = s.b.ins().iadd_imm_s(k, 1);
                            s.st(k1, kept_a, 0);
                            s.ic(0)
                        },
                        |s| {
                            let dr = s.ld(dropped_a, 0);
                            let dr1 = s.b.ins().iadd_imm_s(dr, 1);
                            s.st(dr1, dropped_a, 0);
                            let dnz = s.cmpi_imm(IntCC::NotEqual, d, 0);
                            s.branch(
                                dnz,
                                |s| {
                                    let one = s.ic(1);
                                    s.st(one, sticky_a, 0);
                                    s.ic(0)
                                },
                                |s| s.ic(0),
                            )
                        },
                    )
                },
            );
        });
    }

    // -----------------------------------------------------------------------
    // Float の解析（補足 I-3）

    /// `sigil_arg_float(ptr, len, out) -> ok`: バイト列を `Float` として読めるか確かめ、
    /// 読めたらビット列を `out` へ書く。文法は Rust の `f64` の `FromStr` と一致させ、
    /// 変換は正確な多倍長演算で正しく丸める。
    pub(crate) fn arg_float_body(&mut self) {
        let ps = self.ehead();
        let (ptr, len, out) = (ps[0], ps[1], ps[2]);
        let frame = self.zero_tmp(FW_BYTES);
        let word = FW * 8;
        let m_buf = frame;
        let d_buf = self.b.ins().iadd_imm_s(frame, word);
        let r_buf = self.b.ins().iadd_imm_s(frame, 2 * word);
        let q_buf = self.b.ins().iadd_imm_s(frame, 3 * word);
        // 状態の置き場。
        let slot = |s: &mut Self| {
            let t = s.tmp(8);
            s.saddr(t)
        };
        let i_a = slot(self);
        let neg_a = slot(self);
        let exp_a = slot(self);
        let istart_a = slot(self);
        let intd_a = slot(self);
        let fstart_a = slot(self);
        let fracd_a = slot(self);
        let kept_a = slot(self);
        let dropped_a = slot(self);
        let sticky_a = slot(self);
        let q10_a = slot(self);
        let s_a = slot(self);
        let delta_a = slot(self);
        let z = self.ic(0);
        for a in [
            i_a, neg_a, exp_a, istart_a, intd_a, fstart_a, fracd_a, kept_a, dropped_a, sticky_a,
            q10_a, s_a, delta_a,
        ] {
            self.st(z, a, 0);
        }
        // 共通の戻り先。
        let ret_inf = self.b.create_block();
        let ret_zero = self.b.create_block();
        let ret_nan = self.b.create_block();
        let ret_fail = self.b.create_block();
        // 符号。
        let (signb, scanb) = (self.b.create_block(), self.b.create_block());
        let has = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, len, 1);
        self.b.ins().brif(has, signb, NOARGS, scanb, NOARGS);
        self.b.switch_to_block(signb);
        let b0 = self.get8(ptr);
        let pc = self.cmpi_imm(IntCC::Equal, b0, b'+' as i64);
        let mc = self.cmpi_imm(IntCC::Equal, b0, b'-' as i64);
        let iss = self.b.ins().bor(pc, mc);
        let (setb, toscan) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(iss, setb, NOARGS, toscan, NOARGS);
        self.b.switch_to_block(setb);
        self.st(mc, neg_a, 0);
        let one = self.ic(1);
        self.st(one, i_a, 0);
        self.b.ins().jump(scanb, NOARGS);
        self.b.switch_to_block(toscan);
        self.b.ins().jump(scanb, NOARGS);
        // `inf`・`infinity`・`NaN`（大文字小文字を区別しない）。
        self.b.switch_to_block(scanb);
        let i0 = self.ld(i_a, 0);
        let rp = self.b.ins().iadd(ptr, i0);
        let rl = self.b.ins().isub(len, i0);
        let inf1 = self.match_lit(rp, rl, b"inf", true);
        let inf2 = self.match_lit(rp, rl, b"infinity", true);
        let inf = self.b.ins().bor(inf1, inf2);
        let nanb = self.b.create_block();
        self.b.ins().brif(inf, ret_inf, NOARGS, nanb, NOARGS);
        self.b.switch_to_block(nanb);
        let nan = self.match_lit(rp, rl, b"nan", true);
        let numb = self.b.create_block();
        self.b.ins().brif(nan, ret_nan, NOARGS, numb, NOARGS);
        // 仮数部: 整数部・小数部（どちらか一方は 1 桁以上要る）。
        self.b.switch_to_block(numb);
        let i0 = self.ld(i_a, 0);
        self.st(i0, istart_a, 0);
        self.digit_run(ptr, len, i_a);
        let i1 = self.ld(i_a, 0);
        let is0 = self.ld(istart_a, 0);
        let intd = self.b.ins().isub(i1, is0);
        self.st(intd, intd_a, 0);
        let cond = self.cmpi(IntCC::SignedLessThan, i1, len);
        let dot = self.branch(
            cond,
            |s| {
                let iv = s.ld(i_a, 0);
                let at = s.b.ins().iadd(ptr, iv);
                let byte = s.get8(at);
                s.cmpi_imm(IntCC::Equal, byte, b'.' as i64)
            },
            |s| s.ic(0),
        );
        let i1 = self.ld(i_a, 0);
        self.st(i1, fstart_a, 0);
        let (dotb, expb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(dot, dotb, NOARGS, expb, NOARGS);
        self.b.switch_to_block(dotb);
        let i1 = self.ld(i_a, 0);
        let i2 = self.b.ins().iadd_imm_s(i1, 1);
        self.st(i2, i_a, 0);
        self.st(i2, fstart_a, 0);
        self.digit_run(ptr, len, i_a);
        let i3 = self.ld(i_a, 0);
        let fs = self.ld(fstart_a, 0);
        let fracd = self.b.ins().isub(i3, fs);
        self.st(fracd, fracd_a, 0);
        self.b.ins().jump(expb, NOARGS);
        self.b.switch_to_block(expb);
        let intd = self.ld(intd_a, 0);
        let fracd = self.ld(fracd_a, 0);
        let tot = self.b.ins().iadd(intd, fracd);
        let noz = self.b.ins().icmp_imm_s(IntCC::Equal, tot, 0);
        let contb = self.b.create_block();
        self.b.ins().brif(noz, ret_fail, NOARGS, contb, NOARGS);
        // 指数部（任意）: `e`/`E`、符号、1 桁以上の数字。値は 10^9 で頭打ちにする
        // （それ以上は大小の早期判定でしか使わない）。
        self.b.switch_to_block(contb);
        let i1 = self.ld(i_a, 0);
        let cond = self.cmpi(IntCC::SignedLessThan, i1, len);
        let has_e = self.branch(
            cond,
            |s| {
                let iv = s.ld(i_a, 0);
                let at = s.b.ins().iadd(ptr, iv);
                let byte = s.get8(at);
                let e1 = s.cmpi_imm(IntCC::Equal, byte, b'e' as i64);
                let e2 = s.cmpi_imm(IntCC::Equal, byte, b'E' as i64);
                s.b.ins().bor(e1, e2)
            },
            |s| s.ic(0),
        );
        let (eb, finb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(has_e, eb, NOARGS, finb, NOARGS);
        self.b.switch_to_block(eb);
        let eneg_a = slot(self);
        let es_a = slot(self);
        let ev_a = slot(self);
        let z = self.ic(0);
        self.st(z, eneg_a, 0);
        self.st(z, ev_a, 0);
        let i1 = self.ld(i_a, 0);
        let i2 = self.b.ins().iadd_imm_s(i1, 1);
        self.st(i2, i_a, 0);
        let cond = self.cmpi(IntCC::SignedLessThan, i2, len);
        let sc = self.branch(
            cond,
            |s| {
                let iv = s.ld(i_a, 0);
                let at = s.b.ins().iadd(ptr, iv);
                let byte = s.get8(at);
                let pc = s.cmpi_imm(IntCC::Equal, byte, b'+' as i64);
                let mc = s.cmpi_imm(IntCC::Equal, byte, b'-' as i64);
                let iss = s.b.ins().bor(pc, mc);
                s.branch(
                    iss,
                    |s| {
                        s.st(mc, eneg_a, 0);
                        let i = s.ld(i_a, 0);
                        let i2 = s.b.ins().iadd_imm_s(i, 1);
                        s.st(i2, i_a, 0);
                        s.ic(0)
                    },
                    |s| s.ic(0),
                )
            },
            |s| s.ic(0),
        );
        let _ = sc;
        let es = self.ld(i_a, 0);
        self.st(es, es_a, 0);
        let (ehead, ebody, edone) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.ins().jump(ehead, NOARGS);
        self.b.switch_to_block(ehead);
        let i = self.ld(i_a, 0);
        let more = self.b.ins().icmp(IntCC::SignedLessThan, i, len);
        let (echk, etail) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(more, echk, NOARGS, etail, NOARGS);
        self.b.switch_to_block(echk);
        let i = self.ld(i_a, 0);
        let at = self.b.ins().iadd(ptr, i);
        let byte = self.get8(at);
        let ge = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, byte, b'0' as i64);
        let le = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, byte, b'9' as i64);
        let (ge, le) = (self.b2i(ge), self.b2i(le));
        let isd = self.b.ins().band(ge, le);
        self.b.ins().brif(isd, ebody, NOARGS, etail, NOARGS);
        self.b.switch_to_block(ebody);
        let d = self.b.ins().iadd_imm_s(byte, -(b'0' as i64));
        let ev = self.ld(ev_a, 0);
        let small = self.cmpi_imm(IntCC::SignedLessThan, ev, 100_000_000);
        let nev = self.branch(
            small,
            |s| {
                let ev = s.ld(ev_a, 0);
                let m10 = s.b.ins().imul_imm_s(ev, 10);
                s.b.ins().iadd(m10, d)
            },
            |s| s.ld(ev_a, 0),
        );
        self.st(nev, ev_a, 0);
        let i = self.ld(i_a, 0);
        let next = self.b.ins().iadd_imm_s(i, 1);
        self.st(next, i_a, 0);
        self.b.ins().jump(ehead, NOARGS);
        self.b.switch_to_block(etail);
        self.b.ins().jump(edone, NOARGS);
        self.b.switch_to_block(edone);
        let i = self.ld(i_a, 0);
        let es = self.ld(es_a, 0);
        let nod = self.b.ins().icmp(IntCC::Equal, i, es);
        let eset = self.b.create_block();
        self.b.ins().brif(nod, ret_fail, NOARGS, eset, NOARGS);
        self.b.switch_to_block(eset);
        let ev = self.ld(ev_a, 0);
        let en = self.ld(eneg_a, 0);
        let z = self.ic(0);
        let negated = self.b.ins().isub(z, ev);
        let enc = self.b.ins().icmp_imm_s(IntCC::NotEqual, en, 0);
        let exv = self.b.ins().select(enc, negated, ev);
        self.st(exv, exp_a, 0);
        self.b.ins().jump(finb, NOARGS);
        // 全体を読み終えていること。
        self.b.switch_to_block(finb);
        let i = self.ld(i_a, 0);
        let left = self.b.ins().icmp(IntCC::NotEqual, i, len);
        let digitsb = self.b.create_block();
        self.b.ins().brif(left, ret_fail, NOARGS, digitsb, NOARGS);
        // 桁を多倍長へ送り、10 進の指数を確定する。値 = (M + tail) × 10^q10。
        self.b.switch_to_block(digitsb);
        let is0 = self.ld(istart_a, 0);
        let base1 = self.b.ins().iadd(ptr, is0);
        let intd = self.ld(intd_a, 0);
        self.feed_range(m_buf, base1, intd, kept_a, dropped_a, sticky_a);
        let fs = self.ld(fstart_a, 0);
        let base2 = self.b.ins().iadd(ptr, fs);
        let fracd = self.ld(fracd_a, 0);
        self.feed_range(m_buf, base2, fracd, kept_a, dropped_a, sticky_a);
        let exp = self.ld(exp_a, 0);
        let fracd = self.ld(fracd_a, 0);
        let dropped = self.ld(dropped_a, 0);
        let q10 = self.b.ins().isub(exp, fracd);
        let q10 = self.b.ins().iadd(q10, dropped);
        self.st(q10, q10_a, 0);
        let mz = self.bw_is_zero(m_buf);
        let e10b = self.b.create_block();
        self.b.ins().brif(mz, ret_zero, NOARGS, e10b, NOARGS);
        // 大きさの早期判定: 10^309 以上は無限大、10^-324 未満は 0。
        self.b.switch_to_block(e10b);
        let kept = self.ld(kept_a, 0);
        let qk = self.b.ins().iadd(q10, kept);
        let e10 = self.b.ins().iadd_imm_s(qk, -1);
        let big = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, e10, 309);
        let (chk2, exactb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(big, ret_inf, NOARGS, chk2, NOARGS);
        self.b.switch_to_block(chk2);
        let small = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedLessThanOrEqual, e10, -325);
        self.b.ins().brif(small, ret_zero, NOARGS, exactb, NOARGS);
        // 正確な変換: q10 ≥ 0 なら N = M × 10^q10 をそのまま、そうでなければ割る。
        self.b.switch_to_block(exactb);
        let q10v = self.ld(q10_a, 0);
        let qpos = self.b.create_block();
        let qneg = self.b.create_block();
        let roundb = self.b.create_block();
        let qnn = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, q10v, 0);
        self.b.ins().brif(qnn, qpos, NOARGS, qneg, NOARGS);
        self.b.switch_to_block(qpos);
        self.loop_range(q10v, |s, _| {
            let z = s.ic(0);
            s.bw_mul10add(m_buf, z);
        });
        self.bw_copy(q_buf, m_buf);
        let z = self.ic(0);
        self.st(z, s_a, 0);
        self.st(z, delta_a, 0);
        self.b.ins().jump(roundb, NOARGS);
        self.b.switch_to_block(qneg);
        let z = self.ic(0);
        let one = self.ic(1);
        self.bw_st(d_buf, z, one);
        let nq = self.b.ins().isub(z, q10v);
        self.loop_range(nq, |s, _| {
            let z = s.ic(0);
            s.bw_mul10add(d_buf, z);
        });
        let bl_d = self.bw_bitlen(d_buf);
        let bl_m = self.bw_bitlen(m_buf);
        let dm = self.b.ins().isub(bl_d, bl_m);
        let base = self.ic(1400);
        let s0 = self.b.ins().iadd(base, dm);
        let sneg = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, s0, 0);
        let z = self.ic(0);
        let sv = self.b.ins().select(sneg, z, s0);
        self.st(sv, s_a, 0);
        self.bw_shl(r_buf, m_buf, sv);
        let bl_r = self.bw_bitlen(r_buf);
        let top = self.b.ins().isub(bl_r, bl_d);
        let steps = self.b.ins().iadd_imm_s(top, 1);
        self.loop_range(steps, |s, c| {
            let i = s.b.ins().isub(top, c);
            let cmp = s.bw_cmp_shifted(r_buf, d_buf, i, bl_d);
            let ge = s.cmpi_imm(IntCC::SignedGreaterThanOrEqual, cmp, 0);
            s.branch(
                ge,
                |s| {
                    s.bw_sub_shifted(r_buf, d_buf, i);
                    let qw = s.b.ins().ushr_imm_s(i, 6);
                    let cur = s.bw_ld(q_buf, qw);
                    let sh = s.b.ins().band_imm_s(i, 63);
                    let one = s.ic(1);
                    let bit = s.b.ins().ishl(one, sh);
                    let nv = s.b.ins().bor(cur, bit);
                    s.bw_st(q_buf, qw, nv);
                    s.ic(0)
                },
                |s| s.ic(0),
            );
        });
        let rz = self.bw_is_zero(r_buf);
        let delta = self.b.ins().bxor_imm_s(rz, 1);
        self.st(delta, delta_a, 0);
        self.b.ins().jump(roundb, NOARGS);
        // 丸め: Q の上位 p ビットを取り、残り（δ と sticky を含む）で最近接・同点は偶数。
        self.b.switch_to_block(roundb);
        let sv = self.ld(s_a, 0);
        let bl_q = self.bw_bitlen(q_buf);
        let bl1 = self.b.ins().iadd_imm_s(bl_q, -1);
        let e = self.b.ins().isub(bl1, sv);
        let cond = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, e, -1022);
        let p = self.branch(cond, |s| s.ic(53), |s| s.b.ins().iadd_imm_s(e, 1075));
        let shift = self.b.ins().isub(bl_q, p);
        let cond = self.cmpi_imm(IntCC::SignedLessThanOrEqual, shift, 0);
        let kept_v = self.branch(
            cond,
            |s| {
                let z = s.ic(0);
                let w0 = s.bw_ld(q_buf, z);
                let z = s.ic(0);
                let nsh = s.b.ins().isub(z, shift);
                s.b.ins().ishl(w0, nsh)
            },
            |s| {
                let acc = s.tmp(8);
                let acca = s.saddr(acc);
                let z = s.ic(0);
                s.st(z, acca, 0);
                let n = s.ic(53);
                s.loop_range(n, |s, j| {
                    let idx = s.b.ins().iadd(shift, j);
                    let bit = s.bw_bit(q_buf, idx);
                    let a = s.ld(acca, 0);
                    let bj = s.b.ins().ishl(bit, j);
                    let a2 = s.b.ins().bor(a, bj);
                    s.st(a2, acca, 0);
                });
                s.ld(acca, 0)
            },
        );
        let cond = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, shift, 1);
        let rb = self.branch(
            cond,
            |s| {
                let idx = s.b.ins().iadd_imm_s(shift, -1);
                s.bw_bit(q_buf, idx)
            },
            |s| s.ic(0),
        );
        let cond = self.cmpi_imm(IntCC::SignedGreaterThanOrEqual, shift, 2);
        let sbits = self.branch(
            cond,
            |s| {
                let acc = s.tmp(8);
                let acca = s.saddr(acc);
                let z = s.ic(0);
                s.st(z, acca, 0);
                let sm1 = s.b.ins().iadd_imm_s(shift, -1);
                let wlim = s.b.ins().ushr_imm_s(sm1, 6);
                s.loop_range(wlim, |s, w| {
                    let word = s.bw_ld(q_buf, w);
                    let a = s.ld(acca, 0);
                    let a2 = s.b.ins().bor(a, word);
                    s.st(a2, acca, 0);
                });
                let word = s.bw_ld(q_buf, wlim);
                let remb = s.b.ins().band_imm_s(sm1, 63);
                let one = s.ic(1);
                let pow = s.b.ins().ishl(one, remb);
                let one = s.ic(1);
                let mask = s.b.ins().isub(pow, one);
                let a = s.ld(acca, 0);
                let wm = s.b.ins().band(word, mask);
                let a2 = s.b.ins().bor(a, wm);
                s.cmpi_imm(IntCC::NotEqual, a2, 0)
            },
            |s| s.ic(0),
        );
        let dl = self.ld(delta_a, 0);
        let st = self.ld(sticky_a, 0);
        let dst = self.b.ins().bor(dl, st);
        let sticky_any = self.b.ins().bor(sbits, dst);
        let odd = self.b.ins().band_imm_s(kept_v, 1);
        let so = self.b.ins().bor(sticky_any, odd);
        let round_c = self.b.ins().band(rb, so);
        let round_nz = self.b.ins().icmp_imm_s(IntCC::NotEqual, round_c, 0);
        let kept_p1 = self.b.ins().iadd_imm_s(kept_v, 1);
        let kept2 = self.b.ins().select(round_nz, kept_p1, kept_v);
        let p53 = self.cmpi_imm(IntCC::Equal, p, 53);
        let big = self.cmpi_imm(IntCC::Equal, kept2, 1i64 << 53);
        let carry = self.b.ins().band(p53, big);
        let carry_c = self.b.ins().icmp_imm_s(IntCC::NotEqual, carry, 0);
        let kept_sh = self.b.ins().ushr_imm_s(kept2, 1);
        let kept3 = self.b.ins().select(carry_c, kept_sh, kept2);
        let e_p1 = self.b.ins().iadd_imm_s(e, 1);
        let e2 = self.b.ins().select(carry_c, e_p1, e);
        let asmb = self.b.create_block();
        let over = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, e2, 1023);
        self.b.ins().brif(over, ret_inf, NOARGS, asmb, NOARGS);
        self.b.switch_to_block(asmb);
        let normal = self
            .b
            .ins()
            .icmp_imm_s(IntCC::SignedGreaterThanOrEqual, e2, -1022);
        let ef = self.b.ins().iadd_imm_s(e2, 1023);
        let efsh = self.b.ins().ishl_imm_s(ef, 52);
        let frac = self.b.ins().band_imm_s(kept3, (1i64 << 52) - 1);
        let bits_n = self.b.ins().bor(efsh, frac);
        let bits = self.b.ins().select(normal, bits_n, kept3);
        let negv = self.ld(neg_a, 0);
        let sign = self.b.ins().ishl_imm_s(negv, 63);
        let bits = self.b.ins().bor(bits, sign);
        self.st(bits, out, 0);
        let one = self.ic(1);
        self.b.ins().return_(&[one]);
        // 共通の戻り先の本体。
        self.b.switch_to_block(ret_inf);
        let negv = self.ld(neg_a, 0);
        let enc = self.b.ins().icmp_imm_s(IntCC::NotEqual, negv, 0);
        let (ninf, pinf) = (
            self.ic(0xFFF0_0000_0000_0000u64 as i64),
            self.ic(0x7FF0_0000_0000_0000),
        );
        let bits = self.b.ins().select(enc, ninf, pinf);
        self.st(bits, out, 0);
        let one = self.ic(1);
        self.b.ins().return_(&[one]);
        self.b.switch_to_block(ret_nan);
        let negv = self.ld(neg_a, 0);
        let enc = self.b.ins().icmp_imm_s(IntCC::NotEqual, negv, 0);
        let (nnan, pnan) = (
            self.ic(0xFFF8_0000_0000_0000u64 as i64),
            self.ic(0x7FF8_0000_0000_0000),
        );
        let bits = self.b.ins().select(enc, nnan, pnan);
        self.st(bits, out, 0);
        let one = self.ic(1);
        self.b.ins().return_(&[one]);
        self.b.switch_to_block(ret_zero);
        let negv = self.ld(neg_a, 0);
        let bits = self.b.ins().ishl_imm_s(negv, 63);
        self.st(bits, out, 0);
        let one = self.ic(1);
        self.b.ins().return_(&[one]);
        self.b.switch_to_block(ret_fail);
        let z = self.ic(0);
        self.b.ins().return_(&[z]);
    }
}

impl<'a, 'p> Fx<'a, 'p> {
    // -----------------------------------------------------------------------
    // 第 2 段: 合わせ込み（補足 I-4）

    /// 型 `t` の合わせ込み `sigil_coerce_<k>` を呼ぶ。戻りは成否（1/0）。
    pub(super) fn coerce_call(&mut self, t: &Ty, lit: Value, out: Value) -> Value {
        let key = norm_with(&strip(t), true);
        let id = match self.cg.gen_fn(Gk::Coerce, &key) {
            Ok(i) => i,
            Err(e) => return self.fail(e),
        };
        let fr = self.cg.m.declare_func_in_func(id, self.b.func);
        let inst = self.b.ins().call(fr, &[lit, out]);
        self.b.inst_results(inst)[0]
    }

    /// 合わせ込みの失敗: 要る型の名前 `needed` と、リテラルの種類 `kind` から
    /// 渡された値の名前を、静的な領域へ書いて 0 で戻る。
    fn coerce_fail(&mut self, needed: &[u8], kind: Value) {
        let d = self.cg.coerce_err_data();
        let base = self.data_addr(d);
        let (p, l) = self.bytes_ref(needed);
        self.st(p, base, 0);
        self.st(l, base, 8);
        let names: [&[u8]; 5] = [
            "真偽値".as_bytes(),
            "整数".as_bytes(),
            "小数".as_bytes(),
            "文字列".as_bytes(),
            "一覧".as_bytes(),
        ];
        let (mut ap, mut al) = self.bytes_ref(names[4]);
        for (k, nm) in names[..4].iter().enumerate().rev() {
            let (p2, l2) = self.bytes_ref(nm);
            let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, k as i64);
            ap = self.b.ins().select(eq, p2, ap);
            al = self.b.ins().select(eq, l2, al);
        }
        self.st(ap, base, 16);
        self.st(al, base, 24);
        let z = self.ic(0);
        self.b.ins().return_(&[z]);
    }

    /// `sigil_coerce_<k>(lit, out) -> ok`: リテラル値を型の語に変える。
    /// 規則は移行前の `runtime/core` の `coerce` と同じ（`ty` は鍵: 参照を外し正規化済み）。
    pub(crate) fn coerce_body(&mut self, ty: &Ty) {
        let ps = self.ehead();
        let (lit, out) = (ps[0], ps[1]);
        let kind = self.ld(lit, 0);
        // それ以外の型は常に不一致（移行前の `kind_name` の既定の腕と同じく、要る型は `List`）。
        if !matches!(
            ty,
            Ty::Int | Ty::Float | Ty::Bool | Ty::Str | Ty::Path | Ty::List(_)
        ) {
            self.coerce_fail(b"List", kind);
            return;
        }
        let (okb, ngb) = (self.b.create_block(), self.b.create_block());
        match ty {
            Ty::Int => {
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LK_INT);
                self.b.ins().brif(eq, okb, NOARGS, ngb, NOARGS);
                self.b.switch_to_block(okb);
                let w = self.ld(lit, 8);
                self.st(w, out, 0);
            }
            Ty::Float => {
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LK_FLOAT);
                let (cpb, ib, ng2) = (
                    self.b.create_block(),
                    self.b.create_block(),
                    self.b.create_block(),
                );
                self.b.ins().brif(eq, cpb, NOARGS, ib, NOARGS);
                self.b.switch_to_block(cpb);
                let w = self.ld(lit, 8);
                self.st(w, out, 0);
                self.b.ins().jump(okb, NOARGS);
                self.b.switch_to_block(ib);
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LK_INT);
                self.b.ins().brif(eq, ng2, NOARGS, ngb, NOARGS);
                self.b.switch_to_block(ng2);
                let n = self.ld(lit, 8);
                let f = self.b.ins().fcvt_from_sint(types::F64, n);
                let bits = self.b.ins().bitcast(types::I64, MemFlagsData::new(), f);
                self.st(bits, out, 0);
                self.b.ins().jump(okb, NOARGS);
                self.b.switch_to_block(okb);
            }
            Ty::Bool => {
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LK_BOOL);
                self.b.ins().brif(eq, okb, NOARGS, ngb, NOARGS);
                self.b.switch_to_block(okb);
                let w = self.ld(lit, 8);
                self.st(w, out, 0);
            }
            Ty::Str | Ty::Path => {
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LK_STR);
                self.b.ins().brif(eq, okb, NOARGS, ngb, NOARGS);
                self.b.switch_to_block(okb);
                let p = self.ld(lit, 8);
                let l = self.ld(lit, 16);
                // `new_lossy` のシム（補足 I-5）: 合わせ込みで初めて文字列にする。
                let w = self.rt("str_new", &[p, l]);
                self.st(w, out, 0);
            }
            Ty::List(e) => {
                let eq = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LK_LIST);
                let lb = self.b.create_block();
                self.b.ins().brif(eq, lb, NOARGS, ngb, NOARGS);
                self.b.switch_to_block(lb);
                let cap = self.ic(8);
                let l = self.rt("list_new", &[cap]);
                let cnt = self.ld(lit, 8);
                let flag = self.tmp(8);
                let flaga = self.saddr(flag);
                let one = self.ic(1);
                self.st(one, flaga, 0);
                let wslot = self.tmp(8);
                let wa = self.saddr(wslot);
                self.loop_range(cnt, |s, i| {
                    let fv = s.ld(flaga, 0);
                    let go = s.cmpi_imm(IntCC::Equal, fv, 1);
                    s.branch(
                        go,
                        |s| {
                            let off8 = s.b.ins().imul_imm_s(i, 8);
                            let off = s.b.ins().iadd_imm_s(off8, 16);
                            let addr = s.b.ins().iadd(lit, off);
                            let elit = s.ld(addr, 0);
                            let ok = s.coerce_call(e, elit, wa);
                            s.branch(
                                ok,
                                |s| {
                                    let w = s.ld(wa, 0);
                                    let a2s = s.tmp(8);
                                    let a2 = s.saddr(a2s);
                                    s.st(w, a2, 0);
                                    s.rt("list_push", &[l, a2]);
                                    s.ic(0)
                                },
                                |s| {
                                    let z = s.ic(0);
                                    s.st(z, flaga, 0);
                                    s.ic(0)
                                },
                            )
                        },
                        |s| s.ic(0),
                    );
                });
                let fv = self.ld(flaga, 0);
                let good = self.b.ins().icmp_imm_s(IntCC::Equal, fv, 1);
                let done = self.b.create_block();
                self.b.ins().brif(good, okb, NOARGS, done, NOARGS);
                self.b.switch_to_block(done);
                let z = self.ic(0);
                self.b.ins().return_(&[z]);
                self.b.switch_to_block(okb);
                self.st(l, out, 0);
            }
            _ => unreachable!(),
        }
        // 成功の合流（`okb` にいるはず）: 1 で戻る。
        let one = self.ic(1);
        self.b.ins().return_(&[one]);
        self.b.switch_to_block(ngb);
        let needed: &[u8] = match ty {
            Ty::Int => b"Int",
            Ty::Float => b"Float",
            Ty::Bool => b"Bool",
            Ty::Str => b"String",
            Ty::Path => b"Path",
            _ => b"List",
        };
        self.coerce_fail(needed, kind);
    }

    // -----------------------------------------------------------------------
    // thunk の補助（補足 I-2・I-4）: 読み取りの繰り返しと、誤りの文言の組み立て。
    // 文言は表示バッファに積んで fd 2 へ出す（移行前の `writeln!` と同じ内容）。

    /// 表示バッファの中身を fd 2 へ出し、バッファを片づける。
    fn err_flush2(&mut self) {
        let (buf, len) = self.show_buf();
        let fd = self.ic(2);
        self.scall(SFn::WriteAll, &[fd, buf, len]);
        self.scall(SFn::End, &[]);
    }

    /// 範囲外の誤り（`err` の 2 語が対象のバイト列）。
    fn entry_err_range(&mut self, err: Value) {
        self.scall(SFn::Begin, &[]);
        self.sb_lit("入口の指定が誤り: 整数 ".as_bytes());
        let p = self.ld(err, 0);
        let l = self.ld(err, 8);
        self.scall(SFn::Push, &[p, l]);
        self.sb_lit(" が Int の範囲外\n".as_bytes());
        self.err_flush2();
    }

    /// 個数の不一致の誤り。
    pub(super) fn entry_err_count(&mut self, func: &str, n: i64, given: Value) {
        self.scall(SFn::Begin, &[]);
        self.sb_lit("入口の指定が誤り: ".as_bytes());
        let (p, l) = self.bytes_ref(func.as_bytes());
        self.scall(SFn::Push, &[p, l]);
        self.sb_lit(" は引数を ".as_bytes());
        let nv = self.ic(n);
        self.scall(SFn::Int, &[nv]);
        self.sb_lit(" 個とる（".as_bytes());
        self.scall(SFn::Int, &[given]);
        self.sb_lit(" 個渡された）\n".as_bytes());
        self.err_flush2();
    }

    /// 型の合わない誤り（内訳は合わせ込みが静的な領域へ書いたもの）。
    pub(super) fn entry_err_mismatch(&mut self, pname: &str) {
        let d = self.cg.coerce_err_data();
        let base = self.data_addr(d);
        let np = self.ld(base, 0);
        let nl = self.ld(base, 8);
        let ap = self.ld(base, 16);
        let al = self.ld(base, 24);
        self.scall(SFn::Begin, &[]);
        self.sb_lit("入口の指定が誤り: 引数 ".as_bytes());
        let (p, l) = self.bytes_ref(pname.as_bytes());
        self.scall(SFn::Push, &[p, l]);
        self.sb_lit(" の型が合わない: ".as_bytes());
        self.scall(SFn::Push, &[np, nl]);
        self.sb_lit(" が要るが、".as_bytes());
        self.scall(SFn::Push, &[ap, al]);
        self.sb_lit("が渡された\n".as_bytes());
        self.err_flush2();
    }

    /// コマンドラインの引数を全部リテラル値に読む。戻りは（リテラルの並び, 失敗したか）。
    /// 失敗のときは、文言を標準エラーへ出し終えている。
    pub(super) fn entry_parse_args(&mut self, argc: Value, argv: Value) -> (Value, Value) {
        let one = self.ic(1);
        let gm1 = self.b.ins().isub(argc, one);
        let z = self.ic(0);
        let negc = self.b.ins().icmp(IntCC::SignedLessThan, gm1, z);
        let given = self.b.ins().select(negc, z, gm1);
        let is0 = self.b.ins().icmp_imm_s(IntCC::Equal, given, 0);
        let cnt = self.b.ins().select(is0, one, given);
        let lits = self.raw_words(cnt);
        let err = self.zero_tmp(16);
        let fail = self.tmp(8);
        let faila = self.saddr(fail);
        let z = self.ic(0);
        self.st(z, faila, 0);
        let (head, body, done) = (
            self.b.create_block(),
            self.b.create_block(),
            self.b.create_block(),
        );
        self.b.append_block_param(head, types::I64);
        self.b.append_block_param(body, types::I64);
        let z = self.ic(0);
        self.b.ins().jump(head, &[BlockArg::Value(z)]);
        self.b.switch_to_block(head);
        let gi = self.b.block_params(head)[0];
        let more = self.b.ins().icmp(IntCC::SignedLessThan, gi, given);
        self.b
            .ins()
            .brif(more, body, &[BlockArg::Value(gi)], done, NOARGS);
        self.b.switch_to_block(body);
        let gi = self.b.block_params(body)[0];
        let idx = self.b.ins().iadd_imm_s(gi, 1);
        let off = self.b.ins().imul_imm_s(idx, 8);
        let slot = self.b.ins().iadd(argv, off);
        let aptr = self.ld(slot, 0);
        let alen = self.cstr_len(aptr);
        let lit = self.scall(SFn::ArgParse, &[aptr, alen, err]);
        let isz = self.b.ins().icmp_imm_s(IntCC::Equal, lit, 0);
        let (ngb, okb) = (self.b.create_block(), self.b.create_block());
        self.b.ins().brif(isz, ngb, NOARGS, okb, NOARGS);
        self.b.switch_to_block(ngb);
        self.entry_err_range(err);
        let one = self.ic(1);
        self.st(one, faila, 0);
        self.b.ins().jump(done, NOARGS);
        self.b.switch_to_block(okb);
        let off = self.b.ins().imul_imm_s(gi, 8);
        let addr = self.b.ins().iadd(lits, off);
        self.st(lit, addr, 0);
        let next = self.b.ins().iadd_imm_s(gi, 1);
        self.b.ins().jump(head, &[BlockArg::Value(next)]);
        self.b.switch_to_block(done);
        (lits, self.ld(faila, 0))
    }

    /// 読み終わったリテラル値の並びと、その置き場（`lits`）を片づける。
    /// 置き場の語数は読み取りで取った数と同じ（引数が 0 個でも 1 語取る）。
    /// 合わせ込みが終わった後（成功の経路）と、個数・型の不一致の経路で呼ぶ。
    /// 読み取りの失敗の経路では呼ばない（読めた分が置き場に残るが、プロセスは
    /// そのまま終了コード 1 で終わる。合わせ込みの不一致でも、途中で作った
    /// 合わせ込み済みの値までは片づけないのは移行前と同じ）。
    pub(super) fn entry_free_args(&mut self, lits: Value, given: Value) {
        self.loop_range(given, |s, i| {
            let off = s.b.ins().imul_imm_s(i, 8);
            let addr = s.b.ins().iadd(lits, off);
            let lit = s.ld(addr, 0);
            s.scall(SFn::ArgFree, &[lit]);
        });
        let one = self.ic(1);
        let is0 = self.cmpi_imm(IntCC::Equal, given, 0);
        let cnt = self.b.ins().select(is0, one, given);
        let bytes = self.b.ins().imul_imm_s(cnt, 8);
        let eight = self.ic(8);
        self.rt("raw_free", &[lits, bytes, eight]);
    }
}

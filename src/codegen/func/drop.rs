use super::{aggregate, CgError, Fx, NOARGS};
use crate::ir::Ty;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, Value};

impl<'a, 'p> Fx<'a, 'p> {
    /// 影（`Copy` の複写。共有するものは共有のまま。補足 G-1）。
    pub(super) fn shadow(&mut self, t: &Ty, w: Value) -> Value {
        self.copy_val(t, w, false)
    }

    /// 型 `t` の値 `w` を破棄する（0 は移動済みで何もしない）。生成した `sigil_drop_<k>` を呼ぶ（補足 G-2）。
    /// 文字列・パス・バイト列は子を持たないので、生成の関数を通さずに直接解放する。解放の関数は空番地（0）を
    /// 何もせずに返すので（`std.core.string.free`・`std.core.bytes.free`）、ここに分岐を置かない（補足 G-2）。
    pub(super) fn drop_val(&mut self, t: &Ty, w: Value) {
        match t {
            Ty::Str | Ty::Path => {
                self.rt("str_free", &[w]);
            }
            Ty::Bytes => {
                self.rt("bytes_free", &[w]);
            }
            _ => {
                if !self.drop_flat(t, w) {
                    self.drop_call(t, w);
                }
            }
        }
    }

    pub(super) fn jump_cleanup(&mut self, code: Value) {
        let Some(c) = self.cleanup else {
            self.fail(CgError::Internal("cleanup がない".into()));
            return;
        };
        let a = self.pending_addr();
        self.st(code, a, 0);
        // 取り消しでない巻き戻し（異常停止）に入った印。この後で放す `Lock` は壊れる（評価器の `UNWINDING`）。
        if self.cg.task.as_ref().is_some_and(|t| t.roots) {
            let base = self.region_base();
            let ca = self
                .b
                .ins()
                .iadd_imm_s(base, crate::codegen::TCB_CANCEL_OFF);
            let can = self.ld(ca, 0);
            let one = self.ic(1);
            let z = self.ic(0);
            let v = self.b.ins().select(can, z, one);
            let ua = self
                .b
                .ins()
                .iadd_imm_s(base, crate::codegen::TCB_UNWIND_OFF);
            let old = self.ld(ua, 0);
            let v = self.b.ins().bor(v, old);
            self.st(v, ua, 0);
        }
        self.b.ins().jump(c, NOARGS);
    }

    /// `cond` が真なら符号 `code` で巻き戻しへ進む。
    pub(super) fn fail_if(&mut self, cond: Value, code: i64) {
        let bad = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(cond, bad, NOARGS, ok, NOARGS);
        self.b.switch_to_block(bad);
        let c = self.ic(code);
        self.jump_cleanup(c);
        self.b.switch_to_block(ok);
    }

    /// 呼び出しの符号が 0 でなければ巻き戻しへ進む。
    pub(super) fn after_call(&mut self, code: Value) {
        let bad = self.b.create_block();
        let ok = self.b.create_block();
        self.b.ins().brif(code, bad, NOARGS, ok, NOARGS);
        self.b.switch_to_block(bad);
        self.jump_cleanup(code);
        self.b.switch_to_block(ok);
    }

    pub(super) fn guard_stack(&mut self) {
        let sp = self.b.ins().get_stack_pointer(types::I64);
        let lim = self.rt("stack_limit", &[]);
        let c = self.b.ins().icmp(IntCC::UnsignedLessThan, sp, lim);
        let code = self.stack_code;
        self.fail_if(c, code);
    }

    /// 場所へ書き、古い値を破棄する（新しい値を作った後に古い値を読む）。
    pub(super) fn store_drop(&mut self, addr: Value, v: Value, ty: &Ty) {
        if aggregate(ty) {
            let old = self.ld(addr, 0);
            self.st(v, addr, 0);
            self.drop_val(ty, old);
        } else {
            self.st(v, addr, 0);
        }
    }
}

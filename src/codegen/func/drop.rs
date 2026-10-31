use super::{aggregate, strip, CgError, Fx, NOARGS};
use crate::ir::Ty;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{types, InstBuilder, Value};

impl<'a, 'p> Fx<'a, 'p> {
    pub(super) fn shadow(&mut self, t: &Ty, w: Value) -> Value {
        let d = self.desc_addr(t);
        self.rt("val_shadow", &[d, w])
    }

    /// 見出しつきの塊（`sigil_rt_val_drop` が解放する）。
    pub(super) fn blk(&mut self, bytes: i64) -> Value {
        let sz = self.ic(bytes);
        self.rt("blk_new", &[sz])
    }

    /// 型 `t` の値 `w` を破棄する（0 は移動済みで何もしない）。
    pub(super) fn drop_val(&mut self, t: &Ty, w: Value) {
        if !aggregate(t) {
            return;
        }
        let d = self.desc_addr(&strip(t));
        self.rt("val_drop", &[d, w]);
    }

    pub(super) fn jump_cleanup(&mut self, code: Value) {
        let Some(c) = self.cleanup else {
            self.fail(CgError::Internal("cleanup がない".into()));
            return;
        };
        let p = self.pending.expect("pending");
        let a = self.saddr(p);
        self.st(code, a, 0);
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

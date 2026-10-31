use crate::ir::*;
use std::rc::Rc;

use super::value::{
    compare, deep_clone, deref_val, follow_all, locate, new_slot, ref_at, shadow, with_val,
    with_val_mut, Agg, Clos, Ptr, Tag, Val,
};
use super::{Slot, M};

impl<'p> M<'p> {
    fn konst(&self, c: &Const) -> Val {
        match c {
            Const::Unit => Val::Unit,
            Const::Int(n) => Val::Int(*n),
            Const::Float(x) => Val::Float(*x),
            Const::Bool(b) => Val::Bool(*b),
            Const::Str(i) => Val::Str(self.strs[*i].clone()),
            Const::Fn(f) => Val::Fn(*f),
        }
    }

    pub(super) fn read(&self, locals: &[Slot], op: &Operand) -> Val {
        match op {
            Operand::Const(c) => self.konst(c),
            Operand::Copy(pl) => {
                let (s, p) = locate(locals, pl);
                with_val(&s, &p, shadow)
            }
            Operand::Move(pl) => {
                let (s, p) = locate(locals, pl);
                with_val_mut(&s, &p, |v| match v {
                    Val::Ref(_) | Val::Fn(_) => shadow(v),
                    _ => std::mem::replace(v, Val::Empty),
                })
                .unwrap_or(Val::Empty)
            }
        }
    }

    /// 参照をたどった値（スカラーの読み）。
    pub(super) fn scalar(&self, locals: &[Slot], op: &Operand) -> Val {
        deref_val(self.read(locals, op))
    }

    /// 所有する引数（参照が来たら指す先の写し）。
    pub(super) fn owned(&self, locals: &[Slot], op: &Operand) -> Val {
        match self.read(locals, op) {
            v @ Val::Ref(_) => deref_val(v),
            v => v,
        }
    }

    pub(super) fn read_int(&self, locals: &[Slot], op: &Operand) -> i64 {
        match self.scalar(locals, op) {
            Val::Int(n) => n,
            _ => 0,
        }
    }

    pub(super) fn read_bool(&self, locals: &[Slot], op: &Operand) -> bool {
        matches!(self.scalar(locals, op), Val::Bool(true))
    }

    /// 参照をすべてたどった先の場所。
    pub(super) fn view(&self, locals: &[Slot], op: &Operand) -> (Slot, Vec<u32>) {
        match op {
            Operand::Const(c) => (new_slot(self.konst(c)), vec![]),
            Operand::Copy(pl) | Operand::Move(pl) => {
                let (mut s, mut p) = locate(locals, pl);
                follow_all(&mut s, &mut p);
                (s, p)
            }
        }
    }

    pub(super) fn write(&self, locals: &[Slot], pl: &Place, v: Val) {
        let (s, p) = locate(locals, pl);
        // 古い値は借用を外してから破棄する。
        let old = with_val_mut(&s, &p, |slot| std::mem::replace(slot, v));
        drop(old);
    }

    fn canon_tag(&self, k: &AggKind) -> Tag {
        match k {
            AggKind::Tuple => Tag::Tuple,
            AggKind::Struct(t) => Tag::Struct(self.canon[*t]),
            AggKind::Variant(t, i) => Tag::Variant(self.canon[*t], *i),
        }
    }

    pub(super) fn rvalue(&mut self, locals: &[Slot], rv: &Rvalue) -> Val {
        match rv {
            Rvalue::Use(op) => self.read(locals, op),
            Rvalue::Unary(op, a) => match (op, self.scalar(locals, a)) {
                (UnOp::Not, Val::Bool(b)) => Val::Bool(!b),
                (UnOp::NegFloat, Val::Float(x)) => Val::Float(-x),
                (UnOp::BitNot, Val::Int(x)) => Val::Int(!x),
                _ => Val::Unit,
            },
            Rvalue::Binary(op, a, b) => {
                let x = self.scalar(locals, a);
                let y = self.scalar(locals, b);
                match (op, &x, &y) {
                    (BinOp::BitAnd, Val::Int(p), Val::Int(q)) => Val::Int(p & q),
                    (BinOp::BitOr, Val::Int(p), Val::Int(q)) => Val::Int(p | q),
                    (BinOp::BitXor, Val::Int(p), Val::Int(q)) => Val::Int(p ^ q),
                    (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, ..) => {
                        Val::Bool(compare(*op, &x, &y))
                    }
                    (_, Val::Float(p), Val::Float(q)) => Val::Float(match op {
                        BinOp::Add => p + q,
                        BinOp::Sub => p - q,
                        BinOp::Mul => p * q,
                        BinOp::Div => p / q,
                        _ => p % q,
                    }),
                    (BinOp::Add, Val::Str(p), Val::Str(q)) => {
                        Val::Str(format!("{p}{q}").as_str().into())
                    }
                    _ => Val::Unit,
                }
            }
            Rvalue::Aggregate(k, ops) => {
                let f: Vec<Val> = ops.iter().map(|o| self.read(locals, o)).collect();
                Val::Agg(Box::new(Agg {
                    tag: self.canon_tag(k),
                    f,
                    silent: false,
                }))
            }
            Rvalue::Ref(pl) | Rvalue::RefMut(pl) => {
                let (s, p) = locate(locals, pl);
                if let Some(q) = ref_at(&s, &p) {
                    return Val::Ref(q);
                }
                Val::Ref(Ptr { slot: s, path: p })
            }
            Rvalue::Clone(op, _) => {
                let (s, p) = self.view(locals, op);
                with_val(&s, &p, deep_clone)
            }
            Rvalue::Discriminant(op) => {
                let (s, p) = self.view(locals, op);
                Val::Int(with_val(&s, &p, |v| match v {
                    Val::Agg(a) => match a.tag {
                        Tag::Variant(_, i) => i as i64,
                        _ => 0,
                    },
                    _ => 0,
                }))
            }
            Rvalue::MakeClosure(fid, ops) => {
                let f: Vec<Val> = ops.iter().map(|o| self.read(locals, o)).collect();
                let env = new_slot(Val::Agg(Box::new(Agg {
                    tag: Tag::Tuple,
                    f,
                    silent: false,
                })));
                Val::Closure(Rc::new(Clos { func: *fid, env }))
            }
        }
    }
}

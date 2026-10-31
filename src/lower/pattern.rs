use crate::ast as a;
use crate::ir::{self, Const, Operand, Place, Rvalue, Term};

use super::{bail, int_lit, is_ref, needs_drop, peel, L, R};

impl<'a> L<'a> {
    pub(super) fn variant_of(&self, ty: &ir::Ty, name: &str) -> R<(usize, usize, Vec<ir::Ty>)> {
        if let ir::Ty::Adt(tid) = peel(ty) {
            if let ir::TypeKind::Enum { variants } = &self.types[*tid].kind {
                if let Some(i) = variants.iter().position(|v| v.name == name) {
                    return Ok((*tid, i, variants[i].fields.clone()));
                }
            }
        }
        bail(format!("列挙型の腕 {name} がない"))
    }

    pub(super) fn pat_test(
        &mut self,
        p: &a::Pattern,
        place: &Place,
        ty: &ir::Ty,
        fail: usize,
    ) -> R<()> {
        match p {
            a::Pattern::Wild | a::Pattern::Bind(_) => {}
            a::Pattern::Int(_)
            | a::Pattern::NegInt(_)
            | a::Pattern::Str(_)
            | a::Pattern::Bool(_) => {
                let k = match p {
                    a::Pattern::Int(s) => Const::Int(int_lit(s)?),
                    a::Pattern::NegInt(s) => {
                        let t = s.replace('_', "");
                        let t = if t.starts_with('-') {
                            t
                        } else {
                            format!("-{t}")
                        };
                        Const::Int(int_lit(&t)?)
                    }
                    a::Pattern::Str(s) => Const::Str(self.string(s)),
                    a::Pattern::Bool(b) => Const::Bool(*b),
                    _ => unreachable!(),
                };
                let c = self.tmp(ir::Ty::Bool);
                self.assign(
                    Place::local(c),
                    Rvalue::Binary(
                        ir::BinOp::Eq,
                        Operand::Copy(place.clone()),
                        Operand::Const(k),
                    ),
                );
                let ok = self.new_block();
                self.term(Term::Branch(Operand::Copy(Place::local(c)), ok, fail));
                self.fb.cur = ok;
            }
            a::Pattern::Tuple(ps) => {
                let ts = match peel(ty) {
                    ir::Ty::Tuple(ts) => ts.clone(),
                    _ => return bail("組のパターンの型"),
                };
                for (i, sp) in ps.iter().enumerate() {
                    let t = ts.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_test(sp, &place.field(i), &t, fail)?;
                }
            }
            a::Pattern::Variant { path, args } => {
                let name = path.last().map(|s| s.as_str()).unwrap_or("");
                let (_, idx, fields) = self.variant_of(ty, name)?;
                let d = self.tmp(ir::Ty::Int);
                self.assign(
                    Place::local(d),
                    Rvalue::Discriminant(Operand::Copy(place.clone())),
                );
                let ok = self.new_block();
                self.term(Term::Switch(
                    Operand::Copy(Place::local(d)),
                    vec![(idx as i64, ok)],
                    fail,
                ));
                self.fb.cur = ok;
                for (i, sp) in args.iter().enumerate() {
                    let t = fields.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_test(sp, &place.downcast(idx).field(i), &t, fail)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn pat_bind(
        &mut self,
        p: &a::Pattern,
        place: &Place,
        ty: &ir::Ty,
        by_ref: Option<bool>,
    ) -> R<()> {
        match p {
            a::Pattern::Bind(n) => {
                if let Some(m) = by_ref {
                    let inner = peel(ty).clone();
                    let (bty, rv) = if is_ref(ty) {
                        (ty.clone(), Rvalue::Use(Operand::Copy(place.clone())))
                    } else if m {
                        (
                            ir::Ty::MutRef(Box::new(inner)),
                            Rvalue::RefMut(place.clone()),
                        )
                    } else {
                        (ir::Ty::Ref(Box::new(inner)), Rvalue::Ref(place.clone()))
                    };
                    let l = self.new_local(n, bty.clone());
                    self.assign(Place::local(l), rv);
                    self.push_bind(n, Place::local(l), bty);
                } else {
                    let l = self.new_local(n, ty.clone());
                    let op = self.mv(place, ty);
                    self.assign(Place::local(l), Rvalue::Use(op));
                    if needs_drop(ty) {
                        self.register(l, false);
                    }
                    self.push_bind(n, Place::local(l), ty.clone());
                }
            }
            a::Pattern::Tuple(ps) => {
                let ts = match peel(ty) {
                    ir::Ty::Tuple(ts) => ts.clone(),
                    _ => return bail("組のパターンの型"),
                };
                for (i, sp) in ps.iter().enumerate() {
                    let t = ts.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_bind(sp, &place.field(i), &t, by_ref)?;
                }
            }
            a::Pattern::Variant { path, args } => {
                let name = path.last().map(|s| s.as_str()).unwrap_or("");
                let (_, idx, fields) = self.variant_of(ty, name)?;
                for (i, sp) in args.iter().enumerate() {
                    let t = fields.get(i).cloned().unwrap_or(ir::Ty::Unit);
                    self.pat_bind(sp, &place.downcast(idx).field(i), &t, by_ref)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}

use super::{aggregate, norm, strip, Cur, Fx};
use crate::ir::{AggKind, BinOp, Const, Operand, Place, Proj, Rvalue, Ty, TypeKind, UnOp};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{types, InstBuilder, MemFlagsData, Value};
use cranelift_module::Module;

impl<'a, 'p> Fx<'a, 'p> {
    // --- 場所と被演算子 ---

    fn autoload(&mut self, addr: &mut Value, cur: &mut Cur) {
        if let Cur::T(Ty::Ref(inner)) = cur {
            *addr = self.ld(*addr, 0);
            let t = (**inner).clone();
            *cur = Cur::T(t);
        }
    }

    pub(super) fn place(&mut self, pl: &Place) -> (Value, Cur) {
        let mut addr = self.saddr(self.slots[pl.base]);
        let mut cur = Cur::T(self.lt[pl.base].clone());
        for pj in &pl.proj {
            match *pj {
                Proj::Field(i) => {
                    self.autoload(&mut addr, &mut cur);
                    let bp = self.ld(addr, 0);
                    let p = self.cg.p;
                    let (off, ty) = match &cur {
                        Cur::T(Ty::Tuple(ts)) if i < ts.len() => (8 * i as i32, norm(&ts[i])),
                        Cur::T(Ty::Adt(t)) => match &p.types[*t].kind {
                            TypeKind::Struct { fields } if i < fields.len() => {
                                (8 * i as i32, norm(&fields[i].1))
                            }
                            _ => {
                                self.unsupported("場所の形が想定と違う（構造体でない）");
                                (0, Ty::Unit)
                            }
                        },
                        Cur::V(t, v) => match &p.types[*t].kind {
                            TypeKind::Enum { variants } if i < variants[*v].fields.len() => {
                                (8 + 8 * i as i32, norm(&variants[*v].fields[i]))
                            }
                            _ => {
                                self.unsupported("場所の形が想定と違う（腕のフィールド）");
                                (8, Ty::Unit)
                            }
                        },
                        _ => {
                            self.unsupported("この場所の射影はまだ対応していない");
                            (0, Ty::Unit)
                        }
                    };
                    addr = self.b.ins().iadd_imm_s(bp, off as i64);
                    cur = Cur::T(ty);
                }
                Proj::Downcast(v) => {
                    self.autoload(&mut addr, &mut cur);
                    match &cur {
                        Cur::T(Ty::Adt(t)) => cur = Cur::V(*t, v),
                        _ => {
                            self.unsupported("腕への射影が列挙型でない");
                        }
                    }
                }
                Proj::Deref => {
                    if let Cur::T(Ty::Ref(inner)) = &cur {
                        let t = (**inner).clone();
                        addr = self.ld(addr, 0);
                        cur = Cur::T(t);
                    }
                }
            }
        }
        (addr, cur)
    }

    pub(super) fn cur_ty(c: &Cur) -> Ty {
        match c {
            Cur::T(t) => t.clone(),
            Cur::V(..) => Ty::Unit,
        }
    }

    /// 被演算子を語として読む（型・複製かどうかつき）。
    fn raw(&mut self, op: &Operand) -> (Value, Ty, bool) {
        let (v, t, c, _) = self.raw_a(op);
        (v, t, c)
    }

    /// `raw` に加えて、移動の元の番地（所有する型の場合だけ）を返す。
    fn raw_a(&mut self, op: &Operand) -> (Value, Ty, bool, Option<Value>) {
        if let Operand::Copy(p) | Operand::Move(p) = op {
            let (a, c) = self.place(p);
            let v = self.ld(a, 0);
            let t = Self::cur_ty(&c);
            let mv = matches!(op, Operand::Move(_)) && aggregate(&t);
            return (v, t, matches!(op, Operand::Copy(_)), mv.then_some(a));
        }
        let (v, t, c) = self.raw_const(op);
        (v, t, c, None)
    }

    fn raw_const(&mut self, op: &Operand) -> (Value, Ty, bool) {
        match op {
            Operand::Copy(_) | Operand::Move(_) => unreachable!(),
            Operand::Const(c) => match c {
                Const::Unit => (self.ic(0), Ty::Unit, false),
                Const::Int(n) => (self.ic(*n), Ty::Int, false),
                Const::Float(f) => (self.ic(f.to_bits() as i64), Ty::Float, false),
                Const::Bool(b) => (self.ic(*b as i64), Ty::Bool, false),
                Const::Str(s) => {
                    let text = self.cg.p.strings[*s].clone();
                    let d = match self.cg.bytes_data(text.as_bytes()) {
                        Ok(d) => d,
                        Err(e) => return (self.fail(e), Ty::Str, false),
                    };
                    let a = self.data_addr(d);
                    let n = self.ic(text.len() as i64);
                    (self.rt("str_new", &[a, n]), Ty::Str, false)
                }
                Const::Fn(f) => {
                    let func = &self.cg.p.funcs[*f];
                    let ps: Vec<Ty> = func
                        .params
                        .iter()
                        .map(|&l| norm(&func.locals[l].ty))
                        .collect();
                    let r = norm(&func.ret);
                    let d = match self.cg.fnblock(*f) {
                        Ok(d) => d,
                        Err(e) => return (self.fail(e), Ty::Unit, false),
                    };
                    (self.data_addr(d), Ty::FnRef(ps, Box::new(r)), false)
                }
            },
        }
    }

    /// 型の食い違いを直し、複製なら影（黙った深い複写）を作る。
    pub(super) fn finish(&mut self, w: Value, ty: &Ty, is_copy: bool, dest: Option<&Ty>) -> Value {
        if let (Ty::Ref(inner), Some(d)) = (ty, dest) {
            if !matches!(d, Ty::Ref(_)) {
                let v = self.ld(w, 0);
                return if aggregate(inner) {
                    self.shadow(inner, v)
                } else {
                    v
                };
            }
        }
        if is_copy && aggregate(ty) {
            return self.shadow(ty, w);
        }
        w
    }

    pub(super) fn argf(&mut self, op: &Operand, hint: Option<&Ty>) -> (Value, Ty) {
        let (w, ty, c, moved) = self.raw_a(op);
        let v = self.finish(w, &ty, c, hint);
        if let Some(a) = moved {
            // 移動した元は空にする（SIR の評価器の `Empty` と同じ）。
            let z = self.ic(0);
            self.st(z, a, 0);
        }
        let t = match (&ty, hint) {
            (Ty::Ref(i), Some(h)) if !matches!(h, Ty::Ref(_)) => (**i).clone(),
            _ => ty,
        };
        (v, t)
    }

    /// 所有する値として読む（参照ならたどって影を作る）。
    pub(super) fn own(&mut self, op: &Operand) -> (Value, Ty) {
        self.argf(op, Some(&Ty::Unit))
    }

    /// 値として見る（参照を 1 段たどる。影は作らない）。
    pub(super) fn val(&mut self, op: &Operand) -> (Value, Ty) {
        let (w, ty, _) = self.raw(op);
        if let Ty::Ref(i) = ty {
            (self.ld(w, 0), *i)
        } else {
            (w, ty)
        }
    }

    pub(super) fn rvalue(&mut self, rv: &Rvalue, dty: &Ty) -> Value {
        match rv {
            Rvalue::Use(op) => self.argf(op, Some(dty)).0,
            Rvalue::Unary(UnOp::Not, op) => {
                let (v, _) = self.val(op);
                self.b.ins().bxor_imm_s(v, 1)
            }
            Rvalue::Unary(UnOp::NegFloat, op) => {
                let (v, _) = self.val(op);
                let f = self.b.ins().bitcast(types::F64, MemFlagsData::new(), v);
                let n = self.b.ins().fneg(f);
                self.b.ins().bitcast(types::I64, MemFlagsData::new(), n)
            }
            Rvalue::Unary(UnOp::BitNot, op) => {
                let (v, _) = self.val(op);
                self.b.ins().bnot(v)
            }
            Rvalue::Binary(op, a, b) => {
                let (x, tx) = self.val(a);
                let (y, _) = self.val(b);
                self.binary(*op, &tx, x, y)
            }
            Rvalue::Aggregate(kind, ops) => {
                let p = self.cg.p;
                let hints: Vec<Ty> = match kind {
                    AggKind::Tuple => match dty {
                        Ty::Tuple(ts) => ts.clone(),
                        _ => vec![],
                    },
                    AggKind::Struct(t) => match &p.types[*t].kind {
                        TypeKind::Struct { fields } => fields.iter().map(|f| norm(&f.1)).collect(),
                        _ => vec![],
                    },
                    AggKind::Variant(t, v) => match &p.types[*t].kind {
                        TypeKind::Enum { variants } => {
                            variants[*v].fields.iter().map(norm).collect()
                        }
                        _ => vec![],
                    },
                };
                let mut vals = vec![];
                for (i, o) in ops.iter().enumerate() {
                    vals.push(self.argf(o, hints.get(i)).0);
                }
                match kind {
                    AggKind::Variant(_, v) => self.mk_enum_c(*v as i64, &vals),
                    _ => {
                        let size = std::cmp::max(8, 8 * vals.len() as i64);
                        let blk = self.blk(size);
                        for (i, x) in vals.iter().enumerate() {
                            self.st(*x, blk, 8 * i as i32);
                        }
                        blk
                    }
                }
            }
            Rvalue::Ref(pl) | Rvalue::RefMut(pl) => {
                let (a, c) = self.place(pl);
                if matches!(c, Cur::T(Ty::Ref(_))) {
                    self.ld(a, 0)
                } else {
                    a
                }
            }
            Rvalue::Clone(op, ty) => {
                let (v, _) = self.val(op);
                let t = strip(&norm(ty));
                match t {
                    Ty::Int | Ty::Float | Ty::Bool | Ty::Unit | Ty::Addr | Ty::ParseError => v,
                    Ty::Str | Ty::Path => self.rt("str_clone", &[v]),
                    _ => {
                        let d = self.desc_addr(&t);
                        self.rt("val_clone", &[d, v])
                    }
                }
            }
            Rvalue::Discriminant(op) => {
                let (v, _) = self.val(op);
                self.ld(v, 0)
            }
            Rvalue::MakeClosure(f, ops) => {
                let p = self.cg.p;
                let func = &p.funcs[*f];
                let hints: Vec<Ty> = match func.params.first().map(|&l| norm(&func.locals[l].ty)) {
                    Some(Ty::Ref(i)) => match *i {
                        Ty::Tuple(ts) => ts,
                        _ => vec![],
                    },
                    _ => vec![],
                };
                let mut vals = vec![];
                for (i, o) in ops.iter().enumerate() {
                    vals.push(self.argf(o, hints.get(i)).0);
                }
                let size = std::cmp::max(8, 8 * vals.len() as i64);
                let env = self.blk(size);
                for (i, x) in vals.iter().enumerate() {
                    self.st(*x, env, 8 * i as i32);
                }
                // 環境の型（クロージャを破棄するときに中身を破棄するため）。
                let envty = if hints.len() == vals.len() {
                    Ty::Tuple(hints.clone())
                } else {
                    Ty::Tuple(vec![Ty::Unit; vals.len()])
                };
                let envd = self.desc_addr(&envty);
                let fr = self
                    .cg
                    .m
                    .declare_func_in_func(self.cg.fids[*f], self.b.func);
                let fa = self.b.ins().func_addr(types::I64, fr);
                let blk = self.blk(24);
                self.st(fa, blk, 0);
                self.st(env, blk, 8);
                self.st(envd, blk, 16);
                blk
            }
        }
    }

    /// 構造的な比較（記述子を通す）。結果は 0/1 の語。
    pub(super) fn compare(&mut self, op: BinOp, t: &Ty, x: Value, y: Value) -> Value {
        let d = self.desc_addr(&strip(t));
        match op {
            BinOp::Eq | BinOp::Ne => {
                let e = self.rt("val_eq", &[d, x, y]);
                if op == BinOp::Ne {
                    self.b.ins().bxor_imm_s(e, 1)
                } else {
                    e
                }
            }
            _ => {
                let c = self.rt("val_cmp", &[d, x, y]);
                let r = match op {
                    BinOp::Lt => self.b.ins().icmp_imm_s(IntCC::Equal, c, -1),
                    BinOp::Gt => self.b.ins().icmp_imm_s(IntCC::Equal, c, 1),
                    BinOp::Le => {
                        let c1 = self.b.ins().iadd_imm_s(c, 1);
                        self.b
                            .ins()
                            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, c1, 1)
                    }
                    _ => self
                        .b
                        .ins()
                        .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, c, 1),
                };
                self.b2i(r)
            }
        }
    }

    fn binary(&mut self, op: BinOp, t: &Ty, x: Value, y: Value) -> Value {
        match t {
            Ty::Int if matches!(op, BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor) => match op {
                BinOp::BitAnd => self.b.ins().band(x, y),
                BinOp::BitOr => self.b.ins().bor(x, y),
                BinOp::BitXor => self.b.ins().bxor(x, y),
                _ => unreachable!(),
            },
            Ty::Int | Ty::Bool | Ty::Addr | Ty::Unit => {
                let cc = match op {
                    BinOp::Eq => IntCC::Equal,
                    BinOp::Ne => IntCC::NotEqual,
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    BinOp::Ge => IntCC::SignedGreaterThanOrEqual,
                    _ => return self.unsupported("Int の算術が CheckedOp でない"),
                };
                let c = self.b.ins().icmp(cc, x, y);
                self.b2i(c)
            }
            Ty::Float => {
                let fx = self.b.ins().bitcast(types::F64, MemFlagsData::new(), x);
                let fy = self.b.ins().bitcast(types::F64, MemFlagsData::new(), y);
                let cc = match op {
                    BinOp::Eq => Some(FloatCC::Equal),
                    BinOp::Ne => Some(FloatCC::NotEqual),
                    BinOp::Lt => Some(FloatCC::LessThan),
                    BinOp::Le => Some(FloatCC::LessThanOrEqual),
                    BinOp::Gt => Some(FloatCC::GreaterThan),
                    BinOp::Ge => Some(FloatCC::GreaterThanOrEqual),
                    _ => None,
                };
                if let Some(cc) = cc {
                    let c = self.b.ins().fcmp(cc, fx, fy);
                    return self.b2i(c);
                }
                let r = match op {
                    BinOp::Add => self.b.ins().fadd(fx, fy),
                    BinOp::Sub => self.b.ins().fsub(fx, fy),
                    BinOp::Mul => self.b.ins().fmul(fx, fy),
                    BinOp::Div => self.b.ins().fdiv(fx, fy),
                    _ => return self.rt("fmod", &[x, y]),
                };
                self.b.ins().bitcast(types::I64, MemFlagsData::new(), r)
            }
            Ty::Str if op == BinOp::Add => self.rt("str_concat", &[x, y]),
            _ => self.compare(op, t, x, y),
        }
    }
}

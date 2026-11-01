use super::util::is_literal;
use super::{
    contains_ref_outside_fn, show, strip_paren, substitute, CapMode, Ck, Pending, Prop, Ty,
    TypeDef, R,
};
use crate::ast::*;
use crate::error::{Error, ErrorKind};

impl Ck {
    // ---------- 型変数と単一化 ----------

    pub(super) fn fresh(&mut self) -> Ty {
        self.subst.push(None);
        Ty::Var(self.subst.len() as u32 - 1)
    }

    /// 推論で決まるべき型変数を作る（本体の終わりまでに決まらなければ `CannotInfer`）。
    pub(super) fn fresh_known(&mut self, what: String) -> Ty {
        let v = self.fresh();
        self.must_know.push((v.clone(), what, self.span));
        v
    }

    pub(super) fn shallow(&self, t: &Ty) -> Ty {
        let mut t = t.clone();
        while let Ty::Var(v) = t {
            match &self.subst[v as usize] {
                Some(n) => t = n.clone(),
                None => break,
            }
        }
        t
    }

    pub(super) fn zonk(&self, t: &Ty) -> Ty {
        let go = |ts: &[Ty]| ts.iter().map(|t| self.zonk(t)).collect::<Vec<_>>();
        match self.shallow(t) {
            Ty::Tuple(ts) => Ty::Tuple(go(&ts)),
            Ty::Ref(m, i) => Ty::Ref(m, Box::new(self.zonk(&i))),
            Ty::Named(n, a) => Ty::Named(n, go(&a)),
            Ty::Fn(ps, r, e) => Ty::Fn(go(&ps), Box::new(self.zonk(&r)), e),
            t => t,
        }
    }

    pub(super) fn sh(&self, t: &Ty) -> String {
        show(&self.zonk(t))
    }

    fn occurs(&self, v: u32, t: &Ty) -> bool {
        match self.shallow(t) {
            Ty::Var(w) => v == w,
            Ty::Tuple(ts) => ts.iter().any(|t| self.occurs(v, t)),
            Ty::Ref(_, i) => self.occurs(v, &i),
            Ty::Named(_, a) => a.iter().any(|t| self.occurs(v, t)),
            Ty::Fn(ps, r, _) => ps.iter().any(|t| self.occurs(v, t)) || self.occurs(v, &r),
            _ => false,
        }
    }

    pub(super) fn unify(&mut self, a: &Ty, b: &Ty) -> Result<(), ()> {
        let (a, b) = (self.shallow(a), self.shallow(b));
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), t) | (t, Ty::Var(x)) => {
                if self.occurs(*x, t) {
                    return Err(());
                }
                self.subst[*x as usize] = Some(t.clone());
                Ok(())
            }
            (Ty::Int, Ty::Int)
            | (Ty::Float, Ty::Float)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Str, Ty::Str)
            | (Ty::Unit, Ty::Unit) => Ok(()),
            (Ty::Param(p), Ty::Param(q)) if p == q => Ok(()),
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => self.unify_all(xs, ys),
            (Ty::Ref(m, x), Ty::Ref(n, y)) if m == n => self.unify(x, y),
            (Ty::Named(n, xs), Ty::Named(m, ys)) if n == m && xs.len() == ys.len() => {
                self.unify_all(xs, ys)
            }
            // 効果節は比べない。置ける位置の宣言との効果の順序（⊑）は、
            // 値を入れる場所で別に確かめる（ADR 0022 決定 3）。
            (Ty::Fn(ps, r, _), Ty::Fn(qs, s, _)) if ps.len() == qs.len() => {
                self.unify_all(ps, qs)?;
                self.unify(r, s)
            }
            _ => Err(()),
        }
    }

    fn unify_all(&mut self, xs: &[Ty], ys: &[Ty]) -> Result<(), ()> {
        for (x, y) in xs.iter().zip(ys) {
            self.unify(x, y)?;
        }
        Ok(())
    }

    /// 失敗したら束縛を元に戻す単一化。
    pub(super) fn try_unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let snap = self.subst.clone();
        if self.unify(a, b).is_ok() {
            true
        } else {
            self.subst = snap;
            false
        }
    }

    /// 値を `expected` が求められる位置に渡せるか。暗黙の移しは篩型 -> 基の型と、リテラル -> 篩型
    /// だけ（ADR 0011 の決定 8。型の中では行わない）。`&mut T` から `&T` への変換はしない（決定 4）。
    pub(super) fn coerce(&mut self, expected: &Ty, actual: &Ty, e: &Expr) -> bool {
        if self.try_unify(expected, actual) {
            return true;
        }
        // 参照は値を表す（ADR 0011 の決定 4）。`Copyable` な値への参照は、値の位置で複製される
        // （`min` の `&T` の結果を `T` として返す、など）。
        if !matches!(self.shallow(expected), Ty::Ref(..) | Ty::Var(_)) {
            if let Ty::Ref(_, ai) = self.shallow(actual) {
                if self.holds(&ai, Prop::Copyable, 0) == Some(true) && self.coerce(expected, &ai, e)
                {
                    return true;
                }
            }
        }
        if let Some(b) = self.refinement_base(actual) {
            if self.try_unify(expected, &b) {
                return true;
            }
        }
        if is_literal(e) {
            if let Some(b) = self.refinement_base(expected) {
                if self.try_unify(&b, actual) {
                    return true;
                }
            }
        }
        // `&篩型` は `&基の型` として渡せる（`&mut` は渡せない。ADR 0012 の決定 1 の 7）
        if let (Ty::Ref(false, ei), Ty::Ref(false, ai)) =
            (self.shallow(expected), self.shallow(actual))
        {
            if let Some(b) = self.refinement_base(&ai) {
                if self.try_unify(&ei, &b) {
                    return true;
                }
            }
        }
        false
    }

    /// `&mut 篩型` を `&mut 基の型` の位置に渡そうとしているか（篩型の条件を壊す書き換えができてしまう）。
    pub(super) fn refine_mut_mismatch(&self, expected: &Ty, actual: &Ty) -> bool {
        match (self.shallow(expected), self.shallow(actual)) {
            (Ty::Ref(true, ei), Ty::Ref(true, ai)) => self
                .refinement_base(&ai)
                .is_some_and(|b| self.zonk(&b) == self.zonk(&ei)),
            _ => false,
        }
    }

    /// 型の食い違いのエラーの種類。
    pub(super) fn mismatch_kind(&self, expected: &Ty, actual: &Ty) -> ErrorKind {
        if self.refine_mut_mismatch(expected, actual) {
            ErrorKind::RefineMutBorrow
        } else {
            ErrorKind::TypeMismatch
        }
    }

    pub(super) fn refinement_base(&self, t: &Ty) -> Option<Ty> {
        match self.shallow(t) {
            Ty::Named(n, _) => match self.types.get(&n) {
                Some(TypeDef::Alias(b)) => Some(b.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// 値の位置での型: 参照を外し、篩型は基の型にする（ADR 0011 の決定 4・8）。
    pub(super) fn val(&self, t: &Ty) -> Ty {
        match self.shallow(t) {
            Ty::Ref(_, i) => self.val(&i),
            t @ Ty::Named(..) => match self.refinement_base(&t) {
                Some(b) => b,
                None => self.zonk(&t),
            },
            t => self.zonk(&t),
        }
    }

    // ---------- 型の性質（導出。ADR 0011 の決定 6） ----------

    /// 性質を持つか。持つ・持たないが決まらない（型変数が残っている）ときは `None`。
    pub(super) fn holds(&self, t: &Ty, p: Prop, depth: u32) -> Option<bool> {
        if depth > 32 {
            return Some(false);
        }
        fn all(it: impl Iterator<Item = Option<bool>>) -> Option<bool> {
            let mut unknown = false;
            for r in it {
                match r {
                    Some(false) => return Some(false),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            if unknown {
                None
            } else {
                Some(true)
            }
        }
        let d = depth + 1;
        match self.shallow(t) {
            Ty::Var(_) => None,
            Ty::Int | Ty::Float => Some(true),
            Ty::Bool | Ty::Unit => Some(!matches!(p, Prop::Comparable | Prop::Numeric)),
            Ty::Str => Some(!matches!(p, Prop::Copyable | Prop::Numeric)),
            Ty::Tuple(ts) => match p {
                Prop::Comparable | Prop::Numeric | Prop::Displayable => Some(false),
                _ => all(ts.iter().map(|t| self.holds(t, p, d))),
            },
            Ty::Ref(m, i) => match p {
                Prop::Sendable => Some(false),
                Prop::Shareable if m => Some(false),
                _ => self.holds(&i, p, d),
            },
            // 置ける位置の関数の型は `Sendable`・`Shareable` を持ち、他の性質は持たない
            // （ADR 0022 決定 3。ADR 0011 決定 6 の関数の型の扱いの改訂）。
            Ty::Fn(_, _, eff) => match eff {
                Some(_) => Some(matches!(p, Prop::Sendable | Prop::Shareable)),
                None => Some(false),
            },
            Ty::Param(n) => {
                let bs = self.env.get(&n).map(|v| &v[..]).unwrap_or(&[]);
                Some(bs.contains(&p) || (p == Prop::Comparable && bs.contains(&Prop::Numeric)))
            }
            Ty::Named(n, args) => match self.types.get(&n) {
                Some(TypeDef::Alias(b)) => {
                    if p == Prop::Numeric {
                        Some(false)
                    } else {
                        self.holds(b, p, d)
                    }
                }
                // 番地は値の複製だけができる（演算子は持たない。ADR 0016 の決定 1）
                Some(TypeDef::Opaque(_)) if n == "Addr" => Some(p == Prop::Copyable),
                // 性質を持たない型（ADR 0026 決定 3）。`Scope` は借用の引数としてだけ
                // 現れる値でなく、`TaskHandle` と `LockGuard` は作った（取得した）
                // タスクだけが持つので、タスクの境界を越えられない。
                Some(TypeDef::Opaque(_))
                    if matches!(n.as_str(), "Scope" | "TaskHandle" | "LockGuard") =>
                {
                    Some(false)
                }
                Some(TypeDef::Opaque(_)) => match (n.as_str(), p) {
                    // ADR 0007 決定 2 の表。参照カウントの増減が原子的でないので、
                    // `Shared` は移すことも共有することもできない。
                    ("Shared", Prop::Sendable | Prop::Shareable) => Some(false),
                    // `Cell` は移せるが、排他の検査が原子的でないので共有できない。
                    ("Cell", Prop::Shareable) => Some(false),
                    ("Cell", Prop::Sendable) => {
                        all(args.iter().map(|t| self.holds(t, Prop::Sendable, d)))
                    }
                    // ADR 0026 決定 3。チャネルの口は、中身が移せれば移せて、常に共有できる。
                    ("Sender" | "Receiver", Prop::Shareable) => Some(true),
                    ("Sender" | "Receiver", Prop::Sendable) => {
                        all(args.iter().map(|t| self.holds(t, Prop::Sendable, d)))
                    }
                    // `SyncShared` と `Atomic` は、中身が移せて共有できるときだけ、
                    // 移せて共有できる（ADR 0026 決定 3）。
                    ("SyncShared" | "Atomic", Prop::Sendable | Prop::Shareable) => {
                        all(args.iter().flat_map(|t| {
                            [
                                self.holds(t, Prop::Sendable, d),
                                self.holds(t, Prop::Shareable, d),
                            ]
                        }))
                    }
                    // `Lock` は、中身が移せれば、移せて共有できる（ADR 0026 決定 3）。
                    ("Lock", Prop::Sendable | Prop::Shareable) => {
                        all(args.iter().map(|t| self.holds(t, Prop::Sendable, d)))
                    }
                    (_, Prop::Equatable | Prop::Sendable | Prop::Shareable) => {
                        all(args.iter().map(|t| self.holds(t, p, d)))
                    }
                    _ => Some(false),
                },
                Some(TypeDef::Struct { .. } | TypeDef::Enum { .. })
                    if p == Prop::Copyable && self.copy_override.iter().any(|(k, _)| *k == n) =>
                {
                    self.copy_override
                        .iter()
                        .find(|(k, _)| *k == n)
                        .map(|(_, v)| *v)
                }
                Some(TypeDef::Struct { .. } | TypeDef::Enum { .. }) => match p {
                    Prop::Copyable | Prop::Comparable | Prop::Numeric | Prop::Displayable => {
                        Some(false)
                    }
                    _ => {
                        // 輪になった型は、輪の先でも性質が成り立つと仮定する
                        let key = (Ty::Named(n.clone(), args.clone()), p);
                        if self.holding.borrow().contains(&key) {
                            return Some(true);
                        }
                        self.holding.borrow_mut().push(key);
                        let r = all(self
                            .component_types(&n, &args)
                            .iter()
                            .map(|t| self.holds(t, p, d)));
                        self.holding.borrow_mut().pop();
                        r
                    }
                },
                None => Some(false),
            },
        }
    }

    /// 構造体のフィールド・列挙型の値の中身の型を、型引数を入れて返す。
    fn component_types(&self, n: &str, args: &[Ty]) -> Vec<Ty> {
        match self.types.get(n) {
            Some(TypeDef::Struct { params, fields }) => {
                let m = params.iter().cloned().zip(args.iter().cloned()).collect();
                fields.iter().map(|(_, t)| substitute(t, &m)).collect()
            }
            Some(TypeDef::Enum { params, variants }) => {
                let m = params.iter().cloned().zip(args.iter().cloned()).collect();
                variants
                    .iter()
                    .flat_map(|(_, ts)| ts.iter().map(|t| substitute(t, &m)))
                    .collect()
            }
            _ => vec![],
        }
    }

    /// `ty` が性質 `prop` を持つことを求める。決まらなければ本体の終わりまで繰り延べる。
    pub(super) fn require(&mut self, ty: &Ty, prop: Prop, kind: ErrorKind, text: String) -> R<()> {
        match self.holds(ty, prop, 0) {
            Some(true) => Ok(()),
            Some(false) => self.err(kind, text.replace("{}", &self.sh(ty))),
            None => {
                self.pending.push(Pending {
                    ty: ty.clone(),
                    prop,
                    kind,
                    text,
                    span: self.span,
                });
                Ok(())
            }
        }
    }

    /// `Scope.spawn` に渡す値の検査（ADR 0026 決定 3・ADR 0007 決定 1 の規則 3）。
    ///
    /// タスクは生成元と切り離されて走るので、渡すクロージャは脱出するクロージャとして
    /// 扱う。(a) 借用をとらえていたらエラー、(b) とらえた値の型が `Sendable` でなければ
    /// エラー、(c) 戻り値の型が `Sendable` でなければエラー。`handle` は登録した
    /// 戻り値の型（`TaskHandle<T>`）で、`T` がクロージャの戻り値の型になる。
    /// とらえた値の記録が取れるのは、クロージャリテラルと `let` で束縛した
    /// クロージャの名前だけ（置ける位置の検査 `check_stored_closure_captures` と同じ範囲）。
    pub(super) fn check_spawn(&mut self, arg: Option<&Expr>, handle: &Ty) -> R<()> {
        let caps = match arg.map(strip_paren) {
            Some(Expr::Closure { .. }) => {
                self.cur_closures.last().map(|r| (r.span, r.caps.clone()))
            }
            Some(Expr::Name(n)) => self
                .cur_closures
                .iter()
                .rev()
                .find(|r| r.bound.as_deref() == Some(n.as_str()))
                .map(|r| (r.span, r.caps.clone())),
            _ => None,
        };
        if let Some((span, caps)) = caps {
            // (a) 借用: 借りる捕獲のほか、参照を含む値を値で持つことも借用をとらえたことになる。
            for (name, ty, mode, _) in &caps {
                if matches!(mode, CapMode::Ref | CapMode::RefMut)
                    || contains_ref_outside_fn(&self.zonk(ty))
                {
                    return Err(Error::new(
                        ErrorKind::SpawnCaptureBorrow,
                        span,
                        format!(
                            "`spawn` に渡すクロージャは借用をとらえられない（とらえた値 `{name}`。タスクは生成元と切り離されて走る）"
                        ),
                    ));
                }
            }
            // (b) とらえた値の型は `Sendable` でなければならない。
            for (name, ty, _, _) in &caps {
                self.require(
                    ty,
                    Prop::Sendable,
                    ErrorKind::SpawnCaptureNotSendable,
                    format!(
                        "`spawn` に渡すクロージャがとらえた値 `{name}` の型 {{}} は `Sendable` でないため、タスクへ移せない"
                    ),
                )?;
            }
        }
        // (c) 戻り値の型は `Sendable` でなければならない。
        if let Ty::Named(n, ts) = self.zonk(handle) {
            if n == "TaskHandle" {
                if let Some(t) = ts.first().cloned() {
                    self.require(
                        &t,
                        Prop::Sendable,
                        ErrorKind::SpawnReturnNotSendable,
                        "`spawn` したタスクの戻り値の型 {} は `Sendable` でないため、タスクの境界を越えられない"
                            .to_string(),
                    )?;
                }
            }
        }
        Ok(())
    }
}

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::borrow::{overlaps, Dead, TEMP};
use crate::error::{Error, ErrorKind, Span};

use super::closure::Cap;
use super::types::{show, Prop, Ty};
use super::util::{contains_ref, contains_ref_outside_fn, place_path, strip_paren};
use super::{addr, Acc, AccKind, Ck, Src, R};

impl Ck {
    pub(super) fn block(&mut self, b: &Block) -> R<()> {
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.pop_scope();
        Ok(())
    }

    /// 見えなくなった名前を `fn_borrowed` から外す。
    pub(super) fn sync_borrowed(&mut self) {
        let live: HashSet<&String> = self.scopes.iter().flat_map(|s| s.keys()).collect();
        self.fn_borrowed.retain(|k| live.contains(k));
    }

    pub(super) fn pop_scope(&mut self) {
        self.scopes.pop();
        self.sync_borrowed();
        // 見えなくなった変数の移動の記録は捨てる
        let live: HashSet<&String> = self.scopes.iter().flat_map(|s| s.keys()).collect();
        self.flow
            .moved
            .retain(|k, _| live.contains(&k.split('.').next().unwrap_or(k).to_string()));
        self.flow
            .maybe
            .retain(|k, _| live.contains(&k.split('.').next().unwrap_or(k).to_string()));
        self.flow.loans.retain(|l| live.contains(&l.holder));
    }

    // ---------- 移動（ADR 0012 の決定 1 の 1・2） ----------

    /// 場所 `path` を使えるか。移動済み（部分的な移動を含む）ならエラー。
    /// 型が決まっていない値を移動したかもしれない場所は、型が決まってから調べ直す。
    pub(super) fn use_place(&mut self, path: &str) -> R<()> {
        if !self.track || self.refine {
            return Ok(());
        }
        let near = |k: &str| {
            k == path || k.starts_with(&format!("{path}.")) || path.starts_with(&format!("{k}."))
        };
        let is_fn = {
            let root = path.split('.').next().unwrap_or(path);
            self.lookup(root)
                .is_some_and(|l| matches!(self.zonk(&l.ty), Ty::Fn(..)))
        };
        let msg = |k: &str, at: Span| {
            if k == path && is_fn {
                format!(
                    "`{path}` は {} 行目で移動していて、ここでは使えない（関数の値は、引数として渡しても移動しない。別名 `let h = {path};` や、クロージャへの `move` のとらえ込みが移動になる。元の名前をそのまま使う）",
                    at.line
                )
            } else if k == path {
                format!(
                    "`{path}` は {} 行目で移動していて、ここでは使えない（移動した値は、再び代入するまで使えない。複製が要るなら `.clone()` を使う）",
                    at.line
                )
            } else if k.len() > path.len() {
                format!(
                    "`{path}` の一部 `{k}` は {} 行目で移動していて、`{path}` の全体はここでは使えない（他のフィールドは使える）",
                    at.line
                )
            } else {
                format!(
                    "`{k}` は {} 行目で移動していて、その一部 `{path}` もここでは使えない",
                    at.line
                )
            }
        };
        if let Some((k, at)) = self.flow.moved.iter().find(|(k, _)| near(k)) {
            let m = msg(k, *at);
            return self.err(ErrorKind::UseAfterMove, m);
        }
        let hit = self
            .flow
            .maybe
            .iter()
            .find(|(k, _)| near(k))
            .map(|(k, (at, ty))| (msg(k, *at), ty.clone()));
        if let Some((m, ty)) = hit {
            let e = Error::new(ErrorKind::UseAfterMove, self.span, m);
            self.moved_pending.push((ty, e));
        }
        Ok(())
    }

    // ---------- 借用（ADR 0012 の決定 1 の 3・4） ----------

    /// 場所 `path` への書き込み（`write`）または読み出しで、重なる貸しを使えなくする。
    /// 読み出しは `&mut` の貸しだけ、書き込み・`&mut` の借用・移動・代入はすべての貸しを使えなくする。
    pub(super) fn touch(&mut self, path: &str, write: bool, op: &str) {
        if !self.track || self.refine {
            return;
        }
        self.note_capture(path, write);
        let at = self.span;
        self.flow.loans.touch(path, write, at, op);
    }

    /// クロージャの外側で宣言された変数の場所 `path` が使われた。いま検査中のクロージャのうち、
    /// その変数が外側にあるものすべてに、とらえたと記録する。
    fn note_capture(&mut self, path: &str, write: bool) {
        if self.frames.is_empty() {
            return;
        }
        let root = path.split('.').next().unwrap_or(path);
        let Some(idx) = self.scopes.iter().rposition(|s| s.contains_key(root)) else {
            return;
        };
        for f in &mut self.frames {
            if idx >= f.base {
                continue;
            }
            match f.caps.iter_mut().find(|c| c.name == root) {
                Some(c) => {
                    c.write |= write;
                    match c.paths.iter_mut().find(|p| p.0 == path) {
                        Some(p) => p.1 |= write,
                        None => c.paths.push((path.to_string(), write)),
                    }
                }
                None => f.caps.push(Cap {
                    name: root.to_string(),
                    write,
                    paths: vec![(path.to_string(), write)],
                }),
            }
        }
    }

    /// 変数 `n` が貸しを持つなら、使えるかを調べて、使われたと記録する。
    pub(super) fn check_holder(&mut self, n: &str) -> R<()> {
        if !self.track || self.refine || !self.flow.loans.iter().any(|l| l.holder == n) {
            return Ok(());
        }
        self.used.insert(n.to_string());
        let Some(l) = self
            .flow
            .loans
            .iter()
            .find(|l| l.holder == n && l.dead.is_some())
            .cloned()
        else {
            return Ok(());
        };
        let Some((at, why)) = l.dead else {
            return Ok(());
        };
        match why {
            Dead::Conflict(op) => self.err(
                ErrorKind::BorrowConflict,
                format!(
                    "`{n}` は `{}` を借りているが、{} 行目の{op}で `{}` が使われたため、ここでは使えない（借りた値は、`{}` を変更・移動する前に使い終える。あとで要る値は先に取り出すか `.clone()` で複製しておく）",
                    l.path, at.line, l.path, l.path
                ),
            ),
            Dead::Temp => self.err(
                ErrorKind::TempBorrow,
                format!(
                    "`{n}` は一時的な値を借りていて、その値は {} 行目の文の終わりで破棄されたので、ここでは使えない（一時的な値を先に `let` の変数に入れてから借りる）",
                    at.line
                ),
            ),
        }
    }

    pub(super) fn log_acc(&mut self, path: &str, kind: AccKind) {
        if self.track && !self.refine {
            self.log.push(Acc {
                path: path.to_string(),
                kind,
            });
        }
    }

    /// 1 つの呼び出しの中（`mark` 以降の記録）で、`&mut` の借用が他の使い方と重なっていないか。
    pub(super) fn check_overlap(&mut self, mark: usize) -> R<()> {
        if !self.track || self.refine || mark >= self.log.len() {
            return Ok(());
        }
        let l = self.log[mark..].to_vec();
        for (i, a) in l.iter().enumerate() {
            for (j, b) in l.iter().enumerate() {
                if i == j || !overlaps(&a.path, &b.path) {
                    continue;
                }
                let bad = matches!(
                    (a.kind, b.kind),
                    (AccKind::Mut, _) | (AccKind::Move, AccKind::Shared)
                );
                if !bad {
                    continue;
                }
                let what = match b.kind {
                    AccKind::Read => "読み出し",
                    AccKind::Shared => "借用",
                    AccKind::Mut => "`&mut` の借用",
                    AccKind::Move => "移動",
                };
                let (own, other) = if a.kind == AccKind::Mut {
                    (&a.path, &b.path)
                } else {
                    (&b.path, &a.path)
                };
                let msg = if a.kind == AccKind::Mut {
                    format!(
                        "`{}` を `&mut` で借りる引数と、同じ呼び出しの中の `{}` の{what}が重なっている（変更の最中に同じ場所を読めない）。先に値を変数に取り出して分ける（例: `xs.push(xs.len());` は `let n = xs.len(); xs.push(n);`）",
                        a.path, other
                    )
                } else {
                    format!(
                        "`{other}` の移動と、同じ呼び出しの中の `{own}` の借用が重なっている（借りた値が、渡した先で使われる間に移動してしまう）。先に値を変数に取り出して分ける"
                    )
                };
                return self.err(ErrorKind::BorrowOverlap, msg);
            }
        }
        Ok(())
    }

    /// 借用 `&place`・`&mut place` の貸し。場所なら場所そのものと、根の変数が持つ貸し。一時的な値なら `TEMP`。
    pub(super) fn borrow_srcs(
        &self,
        inner: &Expr,
        mutable: bool,
        inner_res: &[Src],
        inner_ty: &Ty,
    ) -> Vec<Src> {
        let inner = strip_paren(inner);
        match place_path(inner) {
            Some((root, path)) if self.lookup(&root).is_some() => {
                let mut v = vec![(path, mutable)];
                v.extend(self.flow.loans.held(&root));
                v
            }
            _ => {
                let mut v = vec![(TEMP.to_string(), mutable)];
                if contains_ref(&self.zonk(inner_ty)) {
                    v.extend(inner_res.iter().cloned());
                }
                v
            }
        }
    }

    /// 呼び出しの値が参照を含むなら、その貸しを `srcs` にする。
    pub(super) fn set_res(&mut self, ret: &Ty, srcs: Vec<Src>) {
        self.res.clear();
        if contains_ref(&self.zonk(ret)) {
            for s in srcs {
                if !self.res.contains(&s) {
                    self.res.push(s);
                }
            }
        }
    }

    /// 現在のスコープで宣言された、参照を含む型の名前に、貸し `srcs` を持たせる。
    pub(super) fn adopt(&mut self, names: &[String], srcs: &[Src], stmt_match: bool) -> R<()> {
        if !self.track || self.refine {
            return Ok(());
        }
        let at = self.span;
        for n in names {
            let Some(l) = self.lookup(n) else { continue };
            if !contains_ref(&self.zonk(&l.ty)) {
                continue;
            }
            for (path, mutable) in srcs {
                let dead = if path == TEMP && stmt_match {
                    Some((at, Dead::Temp))
                } else {
                    None
                };
                self.flow.loans.lend(n, path, *mutable, dead);
            }
        }
        Ok(())
    }

    /// 呼び出しの戻り値の型が、置き換えの規則（ADR 0012 の決定 1 の 4）を満たさない理由。
    pub(super) fn subst_problem(&self, ps: &[Ty], ret: &Ty) -> Option<String> {
        for p in ps {
            if let Ty::Ref(true, i) = self.zonk(p) {
                if contains_ref(&i) {
                    return Some(format!(
                        "`&mut` で渡す先の型 `{}` が参照を含んでいる（参照を入れ物に入れて書き換える形は、まだ扱えない）",
                        self.sh(&i)
                    ));
                }
            }
        }
        let ret = self.zonk(ret);
        // 関数の型の中の参照は、戻り値の参照の置き換えの規則の対象でない
        // （ADR 0022 決定 3 で関数の型が戻り値に置けるようになったため）。
        if !contains_ref_outside_fn(&ret) {
            return None;
        }
        let top_ok = match &ret {
            Ty::Ref(_, i) => !contains_ref(i),
            Ty::Named(n, a) if n == "Option" && a.len() == 1 => {
                matches!(&a[0], Ty::Ref(_, i) if !contains_ref(i))
            }
            _ => false,
        };
        if !top_ok {
            return Some(format!(
                "戻り値の型 `{}` の参照は、戻り値そのものか、最も外側の `Option<>` の中にしか置けない",
                show(&ret)
            ));
        }
        let n = ps
            .iter()
            .filter(|p| contains_ref_outside_fn(&self.zonk(p)))
            .count();
        if n != 1 {
            return Some(format!(
                "参照を返す呼び出しは、借用の引数（参照を含む型の引数）がちょうど 1 つでなければならないが、{n} 個ある"
            ));
        }
        None
    }

    /// 変数 `root` が、いま検査しているクロージャの外側で宣言されたものか。
    pub(super) fn is_outer(&self, root: &str) -> bool {
        self.scopes[..self.closure_base.min(self.scopes.len())]
            .iter()
            .any(|s| s.contains_key(root))
    }

    /// 値で持つ場所 `path`（根は変数 `root`）を移動したと記録する。参照を通した場所は移動できない。
    pub(super) fn record_move(&mut self, root: &str, path: &str) -> R<()> {
        let Some(l) = self.lookup(root) else {
            return Ok(());
        };
        if matches!(self.shallow(&l.ty), Ty::Ref(..)) {
            let what = if root == path {
                format!("`{root}` は参照なので、参照先の値を移動できない")
            } else {
                format!("`{path}` は参照 `{root}` を通して届く場所なので、移動できない")
            };
            return self.err(
                ErrorKind::MoveFromRef,
                format!("{what}（複製は `.clone()`。`Copyable` な型なら複製として渡せる）"),
            );
        }
        self.log_acc(path, AccKind::Move);
        if self.is_outer(root) {
            return self.err(
                ErrorKind::ClosureMoveCaptured,
                format!(
                    "`{path}` はクロージャの外で宣言された値で、クロージャがとらえている。本体の中で移動すると、とらえた値が呼び出しのたびに失われるので書けない（値が要るなら `.clone()` で複製する）"
                ),
            );
        }
        self.touch(path, true, "移動");
        let at = self.span;
        self.flow.moved.entry(path.to_string()).or_insert(at);
        Ok(())
    }

    /// `e` が、移動しない関数の値（関数の型の引数・その別名・名前で参照した関数）の名前か。
    pub(super) fn borrowed_fn(&self, e: &Expr) -> bool {
        let Expr::Name(n) = strip_paren(e) else {
            return false;
        };
        match self.lookup(n) {
            Some(_) => self.fn_borrowed.contains(n),
            None => self.fns.contains_key(n),
        }
    }

    /// 式 `e`（型 `t`）の値が、移動する位置に置かれた。`Copyable` でない値を持つ場所なら、移動を記録する。
    pub(super) fn consume(&mut self, e: &Expr, t: &Ty) -> R<()> {
        // 借りている関数の値（引数・その別名）は、渡しても別名にしても移動しない
        if self.borrowed_fn(e) {
            return Ok(());
        }
        if self.contract {
            if let Some((root, _)) = place_path(e) {
                let ty = self.zonk(t);
                if self.lookup(&root).is_some() && self.holds(&ty, Prop::Copyable, 0) == Some(false)
                {
                    return self.err(
                        ErrorKind::ContractMove,
                        format!(
                            "契約の式の中で `{root}` の値を移動できない（契約は本体の前後で値を読むだけ。参照で渡すか、`Copyable` な値だけを値で渡す）"
                        ),
                    );
                }
            }
        }
        if !self.track || self.refine {
            return Ok(());
        }
        let Some((root, path)) = place_path(e) else {
            return Ok(());
        };
        if self.lookup(&root).is_none() {
            return Ok(());
        }
        let ty = self.zonk(t);
        if self.skip.is_none() {
            // 移動かどうか（`Copyable` でないか）は、型が決まった後の `flush_typed` で決める。
            self.rec_moves.push((addr(e), ty.clone()));
        }
        match self.holds(&ty, Prop::Copyable, 0) {
            Some(false) => {}
            None => {
                // 型が決まっていない値: 移動かもしれない。型が決まってから、使用を調べ直す
                let is_ref = self
                    .lookup(&root)
                    .is_some_and(|l| matches!(self.shallow(&l.ty), Ty::Ref(..)));
                if !is_ref && !self.is_outer(&root) {
                    let at = self.span;
                    self.flow.maybe.entry(path).or_insert((at, ty));
                }
                return Ok(());
            }
            Some(true) => return Ok(()),
        }
        self.record_move(&root, &path)
    }

    /// 代入で場所を初期化し直す。全体が移動済みの変数のフィールドには代入できない。
    pub(super) fn reinit(&mut self, root: &str, path: &str) -> R<()> {
        if !self.track || self.refine || self.is_outer(root) {
            return Ok(());
        }
        if let Some((k, at)) = self
            .flow
            .moved
            .iter()
            .find(|(k, _)| path.starts_with(&format!("{k}.")))
        {
            return self.err(
                ErrorKind::UseAfterMove,
                format!(
                    "`{k}` は {} 行目で移動していて、その一部 `{path}` へは代入できない（先に `{k}` 全体へ代入する）",
                    at.line
                ),
            );
        }
        let sub = format!("{path}.");
        self.flow
            .moved
            .retain(|k, _| k != path && !k.starts_with(&sub));
        self.flow
            .maybe
            .retain(|k, _| k != path && !k.starts_with(&sub));
        Ok(())
    }

    /// 参照を返す `return` の式 `e` が借りる場所の根が、参照の引数（またはそれを通して借りた値）か。
    pub(super) fn return_roots(&mut self, e: &Expr) -> R<()> {
        if !self.track || self.refine {
            return Ok(());
        }
        let mut srcs: Vec<Src> = vec![];
        self.return_srcs(e, &mut srcs);
        for (path, _) in &srcs {
            if path == TEMP {
                return self.err(
                    ErrorKind::ReturnBorrowRoot,
                    "一時的な値の参照は、文の終わりで破棄されるので返せない（返せるのは、参照の引数を通して借りた場所だけ）",
                );
            }
            let root = path.split('.').next().unwrap_or(path);
            let ok = self
                .lookup(root)
                .is_some_and(|l| contains_ref(&self.zonk(&l.ty)));
            if !ok {
                return self.err(
                    ErrorKind::ReturnBorrowRoot,
                    format!(
                        "`{root}` はこの関数が所有する値なので、その参照は返せない（関数を抜けると破棄される。返せるのは、参照の引数を通して借りた場所だけ）"
                    ),
                );
            }
        }
        Ok(())
    }

    fn return_srcs(&self, e: &Expr, out: &mut Vec<Src>) {
        let e = strip_paren(e);
        if let Some((root, path)) = place_path(e) {
            if self.lookup(&root).is_some() {
                out.push((path, false));
                out.extend(self.flow.loans.held(&root));
                return;
            }
        }
        if let Expr::Call { callee, args } = e {
            if let Expr::Name(n) = &**callee {
                if matches!(n.as_str(), "Some" | "Ok" | "Err") && self.lookup(n).is_none() {
                    for a in args {
                        let mut v = vec![];
                        self.return_srcs(a, &mut v);
                        out.extend(v);
                    }
                    return;
                }
            }
        }
        out.extend(self.res.iter().cloned());
    }
}

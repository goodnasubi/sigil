//! タスクの状態機械の変換（ADR 0030 決定 1、PR 5-2d）の関数単位の計画と、補助関数の宣言・定義。
//!
//! 方式（ADR 0030 決定 1 の A）: 中断しうる関数（`task.suspend` を起こす操作を呼ぶ関数と、それを呼ぶ関数）を、
//! 状態オブジェクト（枠）を持つ再開可能な関数に変換する。局所は枠の中に置き（すでにどの局所も 8 バイトの語なので、
//! stack slot を枠の中の番地に置き換えるだけで済む）、中断点（呼び出しの終端）ごとに状態番号を持つ。再開は、同じ入口
//! を呼び直して状態番号の中断点へ飛ぶ。中断しうる関数の呼び出しは、子の枠を作って子の入口を呼び、子が中断を返せば
//! 自分も中断を返す（再開時は連鎖を再入する。決定 1 の代償）。
//!
//! 枠の並び（語）: `[0 入口][8 未使用][16 状態][24 停止の符号][32 大きさ][40 持ち主][48 結果の破棄]` のあとに局所、
//! 中断点ごとの 3 語（`[子の枠][補助 1][補助 2]`）。
//!
//! 入口 `sigil_step_<f>(枠) -> (値, 符号)` の符号: 0 は完了（値が戻り値）、正は停止、`SUSPEND`（-1）は中断。
//! 取り消しは「停止の符号 1 で再開する」ことで行う（取り消しの旗が立っている間に再開された入口は、中断点で値を取らず
//! 巻き戻しへ進む）ので、破棄の順序は異常停止の巻き戻しと同じ `cleanup` の連鎖になる。
use super::*;
use crate::ir::{Builtin, Callee, Const, Operand, Rvalue, Stmt, Term};
use std::collections::HashMap;

/// 枠の頭の大きさ（バイト）。
pub(crate) const HDR: i32 = 56;
/// 中断点 1 つあたりの枠の語数（子の枠・補助 1・補助 2）。
pub(crate) const SITE_W: usize = 3;
/// 入口が返す中断の符号。
pub(crate) const SUSPEND: i64 = -1;
/// 取り消しで再開したときに巻き戻しへ進む停止の符号（報告には出ない）。
pub(crate) const CANCEL_CODE: i64 = 1;

/// 中断点の種類。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SiteKind {
    /// 中断しうる関数の直接の呼び出し。
    Direct(usize),
    /// 関数の値の呼び出し（相手が中断しうる関数かもしれない）。
    Indirect,
    /// ブロックしうる並行処理の操作。
    Block(Builtin),
}

/// 変換の計画（プログラム全体）。
pub(crate) struct Plan {
    /// 中断しうる関数（再開可能な入口を持つ）。
    pub susp: Vec<bool>,
    /// 関数の値として使われる関数（`MakeClosure`・`Const::Fn`）。
    pub addr_taken: Vec<usize>,
    /// 関数の値として使われる関数に、中断しうるものがある（`Indirect` の呼び出しが中断しうる）。
    pub indirect_susp: bool,
    pub step: HashMap<usize, FuncId>,
    pub mk: HashMap<usize, FuncId>,
    /// 補助関数（`sigil_fn_lookup`・`sigil_fn_start`・`sigil_frame_done`・`sigil_plain_step`・`sigil_drain`）。
    pub fn_lookup: FuncId,
    pub fn_start: FuncId,
    pub frame_done: FuncId,
    pub plain_step: FuncId,
    pub drain: FuncId,
    pub frame_fp: FuncId,
    pub sched_run: FuncId,
    pub worker_run: FuncId,
}

/// ブロックしうる並行処理の操作か。
pub(crate) fn blocking(b: Builtin) -> bool {
    use Builtin::*;
    matches!(
        b,
        TaskScope
            | TaskJoin
            | TaskYield
            | TaskSleep
            | ChanRecv
            | LockAcquire
            | AtomicLoad
            | IoWaitFd
            | TaskTimeout
    )
}

fn visit_operand(o: &Operand, out: &mut Vec<usize>) {
    if let Operand::Const(Const::Fn(f)) = o {
        out.push(*f);
    }
}

fn visit_rvalue(r: &Rvalue, out: &mut Vec<usize>) {
    match r {
        Rvalue::Use(o) | Rvalue::Unary(_, o) | Rvalue::Clone(o, _) | Rvalue::Discriminant(o) => {
            visit_operand(o, out)
        }
        Rvalue::Binary(_, a, b) => {
            visit_operand(a, out);
            visit_operand(b, out);
        }
        Rvalue::Aggregate(_, ops) => ops.iter().for_each(|o| visit_operand(o, out)),
        Rvalue::MakeClosure(f, ops) => {
            out.push(*f);
            ops.iter().for_each(|o| visit_operand(o, out));
        }
        Rvalue::Ref(_) | Rvalue::RefMut(_) | Rvalue::GlobalRef(_) => {}
    }
}

/// 関数の値として使われる関数（重複なし・昇順）。
fn address_taken(p: &Program) -> Vec<usize> {
    let mut v = vec![];
    for f in &p.funcs {
        for b in &f.blocks {
            for s in &b.stmts {
                if let Stmt::Assign(_, r) = s {
                    visit_rvalue(r, &mut v);
                }
            }
            match &b.term {
                Term::Call { func, args, .. } => {
                    if let Callee::Indirect(o) = func {
                        visit_operand(o, &mut v);
                    }
                    args.iter().for_each(|o| visit_operand(o, &mut v));
                }
                Term::Branch(o, ..) | Term::Switch(o, ..) => visit_operand(o, &mut v),
                Term::CheckedOp { a, b, .. } => {
                    visit_operand(a, &mut v);
                    visit_operand(b, &mut v);
                }
                Term::Assert { cond, .. } => visit_operand(cond, &mut v),
                _ => {}
            }
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// 呼び出しの終端が中断点か（`susp` と `indirect_susp` は計画の途中のものでもよい）。
pub(crate) fn classify(t: &Term, susp: &[bool], indirect_susp: bool) -> Option<SiteKind> {
    let Term::Call { func, .. } = t else {
        return None;
    };
    match func {
        Callee::Func(g) if susp[*g] => Some(SiteKind::Direct(*g)),
        Callee::Indirect(_) if indirect_susp => Some(SiteKind::Indirect),
        Callee::Builtin(b) if blocking(*b) => Some(SiteKind::Block(*b)),
        _ => None,
    }
}

/// 中断しうる関数を求める。`sched` は実行の割り当て（`std.os.sched`）が結ばれているか。結ばれていなければ空
/// （`task.yield` だけを使うプログラムは、従来どおり何もしない）。
pub(crate) fn analyse(p: &Program, sched: bool) -> (Vec<bool>, Vec<usize>, bool) {
    let taken = address_taken(p);
    let mut susp = vec![false; p.funcs.len()];
    if !sched {
        return (susp, taken, false);
    }
    let mut indirect = false;
    loop {
        let mut changed = false;
        for (i, f) in p.funcs.iter().enumerate() {
            if susp[i] {
                continue;
            }
            let hit = f
                .blocks
                .iter()
                .any(|b| classify(&b.term, &susp, indirect).is_some());
            if hit {
                susp[i] = true;
                changed = true;
            }
        }
        let ind = taken.iter().any(|&f| susp[f]);
        if ind != indirect {
            indirect = ind;
            changed = true;
        }
        if !changed {
            break;
        }
    }
    (susp, taken, indirect)
}

/// 関数 `f` の中断点の数。
pub(crate) fn site_count(f: &Func, susp: &[bool], indirect: bool) -> usize {
    f.blocks
        .iter()
        .filter(|b| classify(&b.term, susp, indirect).is_some())
        .count()
}

/// 枠の大きさ（バイト）。
pub(crate) fn frame_bytes(f: &Func, susp: &[bool], indirect: bool) -> i64 {
    HDR as i64 + 8 * (f.locals.len() + SITE_W * site_count(f, susp, indirect)) as i64
}

/// 補助関数と、中断しうる関数の入口・作成関数を宣言し、本体を作る。`Cg` の `task` に計画を置く。
pub(super) fn define_all(cg: &mut Cg<'_>) -> Result<(), CgError> {
    let p = cg.p;
    let cc = cg.cc;
    let (susp, taken, indirect) = analyse(p, cg.rt.contains_key("sched_start"));
    // 並行処理の操作を使うプログラム（実行の割り当てが結ばれている）には、中断しうる関数がなくても補助関数を作る
    // （入口が `Lock.new` やチャネルだけを使うときの後始末が、`sigil_drain` を使う）。
    if !cg.rt.contains_key("sched_start") {
        return Ok(());
    }
    let decl = |cg: &mut Cg<'_>, name: &str, np: usize, ret: char| -> Result<FuncId, CgError> {
        cg.m.declare_function(name, Linkage::Local, &shim_sig(cc, np, ret))
            .map_err(internal)
    };
    let mut step = HashMap::new();
    let mut mk = HashMap::new();
    for (i, f) in p.funcs.iter().enumerate() {
        if !susp[i] {
            continue;
        }
        let mut s = Signature::new(cc);
        s.params.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        step.insert(
            i,
            cg.m.declare_function(&format!("sigil_step_{i}"), Linkage::Local, &s)
                .map_err(internal)?,
        );
        mk.insert(i, decl(cg, &format!("sigil_mk_{i}"), 1, 'p')?);
        let _ = f;
    }
    let mut one = Signature::new(cc);
    one.params.push(AbiParam::new(types::I64));
    one.returns.push(AbiParam::new(types::I64));
    one.returns.push(AbiParam::new(types::I64));
    let plain_step =
        cg.m.declare_function("sigil_plain_step", Linkage::Local, &one)
            .map_err(internal)?;
    let sched_run =
        cg.m.declare_function("sigil_sched_run", Linkage::Local, &one)
            .map_err(internal)?;
    let mut worker_sig = one.clone();
    worker_sig.params.clear();
    let worker_run =
        cg.m.declare_function("sigil_worker_run", Linkage::Local, &worker_sig)
            .map_err(internal)?;
    let fn_lookup = decl(cg, "sigil_fn_lookup", 2, 'p')?;
    let fn_start = decl(cg, "sigil_fn_start", 4, 'p')?;
    let frame_done = decl(cg, "sigil_frame_done", 1, 'v')?;
    let drain = decl(cg, "sigil_drain", 0, 'v')?;
    let frame_fp = decl(cg, "sigil_frame_fp", 1, 'p')?;
    cg.task = Some(Plan {
        susp: susp.clone(),
        addr_taken: taken,
        indirect_susp: indirect,
        step: step.clone(),
        mk: mk.clone(),
        fn_lookup,
        fn_start,
        frame_done,
        plain_step,
        drain,
        frame_fp,
        sched_run,
        worker_run,
    });

    let mut idx = 2_000_000u32;
    let mut next = || {
        idx += 1;
        idx
    };
    for (i, f) in p.funcs.iter().enumerate() {
        if !susp[i] {
            continue;
        }
        let lt: Vec<Ty> = f.locals.iter().map(|l| norm(&l.ty)).collect();
        let mut s = Signature::new(cc);
        s.params.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        cg.define(step[&i], next(), s, lt, |fx| fx.run_step(f, i))?;
        cg.define(mk[&i], next(), shim_sig(cc, 1, 'p'), vec![], |fx| {
            fx.mk_body(f, i)
        })?;
    }
    cg.define(plain_step, next(), one.clone(), vec![], |fx| {
        fx.plain_step_body()
    })?;
    cg.define(fn_lookup, next(), shim_sig(cc, 2, 'p'), vec![], |fx| {
        fx.fn_lookup_body()
    })?;
    cg.define(fn_start, next(), shim_sig(cc, 4, 'p'), vec![], |fx| {
        fx.fn_start_body()
    })?;
    cg.define(frame_done, next(), shim_sig(cc, 1, 'v'), vec![], |fx| {
        fx.frame_done_body()
    })?;
    cg.define(drain, next(), shim_sig(cc, 0, 'v'), vec![], |fx| {
        fx.drain_body()
    })?;
    cg.define(frame_fp, next(), shim_sig(cc, 1, 'p'), vec![], |fx| {
        fx.frame_fp_body()
    })?;
    cg.define(worker_run, next(), worker_sig, vec![], |fx| {
        fx.worker_run_body()
    })?;
    cg.define(sched_run, next(), one, vec![], |fx| fx.sched_run_body())?;
    Ok(())
}

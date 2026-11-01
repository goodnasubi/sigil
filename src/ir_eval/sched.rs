//! 評価器の枠（フレーム）の積み重ねとタスクのスケジューラ（ADR 0026 決定 5・6）。
//!
//! 呼び出しは Rust の再帰ではなく、明示的な枠の積み重ね（`Vec<Frame>`）で行う。
//! タスクは枠の積み重ねを状態として持ち、中断点（呼び出しの終端）でだけ切り替わる。
//! スケジューラは既定で決定的な FIFO（`Options::schedule_seed` で乱数の種による攪拌に差し替え可能）で、
//! 時計は仮想（実時間では待たない）。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::exec::StopKind;
use crate::ir::*;

use super::value::{
    list_of, new_slot, shadow, with_val, with_val_mut, Agg, GuardTok, Ptr, Tag, Val, LOCK_EVENTS,
    UNWINDING,
};
use super::{Slot, E, M};

pub(super) struct Frame {
    fid: FuncId,
    locals: Vec<Slot>,
    bb: usize,
    pending: Option<E>,
    /// この枠が範囲の本体なら、その範囲の番号。
    scope: Option<usize>,
    /// 呼び出し元の停止の地点（`Resume` で理由がないときの報告に使う）。
    loc: StopId,
}

pub(super) enum Run {
    Done(Val),
    Suspend,
    Fail(E),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum State {
    Running,
    Ready,
    Joining,
    Recv,
    LockWait,
    Scoping,
    Sleeping,
    Done,
}

struct Task {
    frames: Vec<Frame>,
    state: State,
    resume: Option<Val>,
    result: Option<Val>,
    scope: Option<usize>,
    /// `AllOk` の子タスクなら、結果を入れる添字。
    slot_idx: usize,
    joiner: Option<usize>,
    /// このタスクが現在待機している範囲（`Scoping` のとき）。
    waiting_scope: Option<usize>,
    /// このタスクが属するルートタスクの番号（ルートタスク自身は自分）。
    root: usize,
}

#[derive(Default)]
enum ScopeKind {
    #[default]
    Normal,
    Timeout {
        seq: u64,
    },
    Race,
    AllOk {
        results: Vec<Option<Val>>,
    },
}

#[derive(Default)]
struct ScopeSt {
    live: usize,
    waiter: Option<usize>,
    children: Vec<usize>,
    kind: ScopeKind,
}

#[derive(Default)]
struct Chan {
    q: VecDeque<Val>,
    waiters: VecDeque<usize>,
    /// 作ったルートタスク。
    root: usize,
}

/// `Lock` の状態。値は `Lock.new` で渡されたもの（評価器では番号で指す）。
struct LockSt {
    value: Val,
    holder: Option<usize>,
    waiters: VecDeque<usize>,
    /// 異常停止したタスクが持っていた（壊れた）。一度壊れたら戻さない（ADR 0026 決定 4）。
    broken: bool,
    /// 作ったルートタスク。ホストが共有するものは `usize::MAX`。
    root: usize,
}

/// 確保器（ADR 0025 決定 3）の関数か。核の `std.alloc` と OS 層の `std.os.alloc` の中。
fn is_allocator_func(name: &str) -> bool {
    name.starts_with("std.alloc.") || name.starts_with("std.os.alloc.")
}

#[cfg(test)]
mod alloc_tests {
    use super::is_allocator_func;

    #[test]
    fn allocator_functions_are_recognised_by_module() {
        assert!(is_allocator_func("std.alloc.grab"));
        assert!(is_allocator_func("std.os.alloc.map"));
        assert!(!is_allocator_func("lib.util.grab"));
        assert!(!is_allocator_func("app.std.alloc.grab"));
    }
}

/// 進行なしに続けて `task.yield` が起きてよい回数の上限（暫定。ライブロックの検出用）。
///
/// 全タスクが `task.yield` と読み出し（`Atomic.load`・`SyncShared.get`）だけを繰り返し、共有の値を
/// 変える操作もタスクの完了も起きない状態は、止まらずデッドロックとしても検出されない（Issue #191）。
/// 時計の進むのを待つタスク（`task.sleep`）がいない場合に限り、この回数を超えたらデッドロックと同じ
/// 異常停止にする。値は、正常な協調ループ（公平のための `yield`）が超えない大きさで、かつ
/// 検出が数秒以内に終わる大きさとして選んだ暫定値であり、仕様ではない。
const MAX_IDLE_YIELDS: usize = 1_000_000;

#[derive(Clone, Copy)]
enum Wake {
    Sleep(usize),
    Timeout(usize),
}

pub(super) struct Sched {
    tasks: Vec<Task>,
    ready: VecDeque<usize>,
    scopes: Vec<ScopeSt>,
    chans: Vec<Chan>,
    locks: Vec<LockSt>,
    /// ルートタスクの任務番号（`tasks` の添字）と結果。`results[i]` は `roots[i]` の結果。
    roots: Vec<usize>,
    results: Vec<Option<Result<Val, E>>>,
    /// 異常停止の巻き戻しが確保器の関数の枠を通った（プロセス全体の停止にする）。
    alloc_stop: bool,
    sleepers: Vec<(i64, u64, Wake)>,
    clock: i64,
    seq: u64,
    cur: usize,
    live: usize,
    max_depth: usize,
    rng: Option<u64>,
    /// 進行（`task.yield` と読み出し以外のタスク操作、タスクの完了）なしに続けて起きた `task.yield` の回数。
    idle_yields: usize,
}

impl Sched {
    pub(super) fn new(stack_bytes: usize, schedule_seed: Option<u64>) -> Sched {
        Sched {
            tasks: vec![],
            ready: VecDeque::new(),
            scopes: vec![],
            chans: vec![],
            locks: vec![],
            roots: vec![],
            results: vec![],
            alloc_stop: false,
            sleepers: vec![],
            clock: 0,
            seq: 0,
            cur: 0,
            live: 0,
            // 枠 1 つを約 1 KiB と見なして、深さの上限を決める（スタックの枯渇の判定）。
            max_depth: (stack_bytes / 1024).max(64),
            rng: schedule_seed,
            idle_yields: 0,
        }
    }

    /// ホストが全ルートタスクに共有する `Lock` を作る（どのルートタスクにも属さない）。
    pub(super) fn new_shared_lock(&mut self, value: Val) -> usize {
        self.locks.push(LockSt {
            value,
            holder: None,
            waiters: VecDeque::new(),
            broken: false,
            root: usize::MAX,
        });
        self.locks.len() - 1
    }

    /// 生きている枠の数。
    pub(super) fn live_frames(&self) -> usize {
        self.live
    }

    fn next_rand(&mut self) -> Option<u64> {
        let st = self.rng.as_mut()?;
        // SplitMix64（種が 0 でも偏らず進む決定的な疑似乱数）
        *st = st.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = *st;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        Some(z ^ (z >> 31))
    }

    fn pop_ready(&mut self) -> Option<usize> {
        if self.ready.is_empty() {
            return None;
        }
        if let Some(r) = self.next_rand() {
            let idx = (r as usize) % self.ready.len();
            self.ready.remove(idx)
        } else {
            self.ready.pop_front()
        }
    }
}

enum Op {
    Val(Val),
    Push(FuncId, Vec<Val>, Option<usize>),
    Block,
    Fail(E),
}

enum Act {
    Continue,
    Push(FuncId, Vec<Val>, StopId),
    Ret(Val),
    Resume,
    Suspend,
}

fn is_task_op(b: Builtin) -> bool {
    use Builtin::*;
    matches!(
        b,
        TaskScope
            | TaskAllOk
            | TaskRace
            | TaskTimeout
            | TaskYield
            | TaskSleep
            | TaskChannel
            | ScopeSpawn
            | TaskJoin
            | ChanSend
            | ChanRecv
            | SyncSharedNew
            | SyncSharedGet
            | LockNew
            | LockAcquire
            | AtomicNew
            | AtomicLoad
            | AtomicStore
    )
}

impl<'p> M<'p> {
    fn new_frame(
        &mut self,
        fid: FuncId,
        args: Vec<Val>,
        loc: StopId,
        scope: Option<usize>,
    ) -> Frame {
        let f = &self.p.funcs[fid];
        let locals: Vec<Slot> = (0..f.locals.len()).map(|_| new_slot(Val::Empty)).collect();
        for (a, &pi) in args.into_iter().zip(&f.params) {
            *locals[pi].borrow_mut() = a;
        }
        self.sch.live += 1;
        Frame {
            fid,
            locals,
            bb: 0,
            pending: None,
            scope,
            loc,
        }
    }

    fn drop_frame_locals(mut locals: Vec<Slot>) {
        while let Some(s) = locals.pop() {
            let old = std::mem::replace(&mut *s.borrow_mut(), Val::Empty);
            drop(old);
            drop(s);
        }
    }

    /// 関数の値を、呼ぶ関数と引数にする（呼べない値は `None`）。
    pub(super) fn resolve_callee(&self, f: &Val, args: Vec<Val>) -> Option<(FuncId, Vec<Val>)> {
        match f {
            Val::Closure(c) => {
                let mut a = vec![Val::Ref(Ptr {
                    slot: c.env.clone(),
                    path: vec![],
                })];
                a.extend(args);
                Some((c.func, a))
            }
            Val::Fn(g) => Some((*g, args)),
            Val::Ref(p) => {
                let t = p.with(shadow);
                self.resolve_callee(&t, args)
            }
            _ => None,
        }
    }

    /// 入れ子の呼び出し（組み込みの中からの関数の呼び出し）。中断点は持てない。
    pub(super) fn run_func(&mut self, fid: FuncId, args: Vec<Val>, loc: StopId) -> Result<Val, E> {
        let (base, budget) = super::value::STACK_LIMIT.with(|c| c.get());
        if base.abs_diff(super::value::stack_addr()) > budget || self.sch.live >= self.sch.max_depth
        {
            return Err(self.stop_at(loc, Some(StopKind::StackExhausted), Some("")));
        }
        let mut frames = vec![self.new_frame(fid, args, loc, None)];
        match self.drive(&mut frames, false) {
            Run::Done(v) => Ok(v),
            Run::Fail(e) => Err(e),
            Run::Suspend => unreachable!("入れ子の呼び出しは中断しない"),
        }
    }

    fn call_term_of(&self, fr: &Frame) -> &'p Term {
        &self.p.funcs[fr.fid].blocks[fr.bb].term
    }

    fn deliver(&self, fr: &mut Frame, v: Val) {
        if let Term::Call { dest, next, .. } = self.call_term_of(fr) {
            self.write(&fr.locals, dest, v);
            fr.bb = *next;
        }
    }

    fn deliver_err(&self, fr: &mut Frame, e: E) {
        if let Term::Call { cleanup, .. } = self.call_term_of(fr) {
            fr.pending = Some(e);
            fr.bb = *cleanup;
        }
    }

    pub(super) fn drive(&mut self, frames: &mut Vec<Frame>, top: bool) -> Run {
        let p = self.p;
        loop {
            let act = {
                let fr = frames.last_mut().expect("枠がある");
                let blk = &p.funcs[fr.fid].blocks[fr.bb];
                // 後始末の基本ブロック（異常停止の巻き戻し）での破棄は、持っていた `Lock` を壊す。
                UNWINDING.with(|c| c.set(fr.pending.is_some()));
                for s in &blk.stmts {
                    self.exec(&fr.locals, s);
                }
                UNWINDING.with(|c| c.set(false));
                let locals = &fr.locals;
                match &blk.term {
                    Term::Goto(b) => {
                        fr.bb = *b;
                        Act::Continue
                    }
                    Term::Branch(c, t, e) => {
                        fr.bb = if self.read_bool(locals, c) { *t } else { *e };
                        Act::Continue
                    }
                    Term::Switch(c, arms, d) => {
                        let n = self.read_int(locals, c);
                        fr.bb = arms.iter().find(|(k, _)| *k == n).map_or(*d, |(_, b)| *b);
                        Act::Continue
                    }
                    Term::Call {
                        func,
                        args,
                        dest,
                        next,
                        cleanup,
                        stop,
                    } => {
                        let mut push: Option<(FuncId, Vec<Val>)> = None;
                        let mut block = false;
                        let r: Option<Result<Val, E>> = match func {
                            Callee::Func(g) => {
                                let argv: Vec<Val> =
                                    args.iter().map(|a| self.read(locals, a)).collect();
                                push = Some((*g, argv));
                                None
                            }
                            Callee::Indirect(op) => {
                                let (s, pth) = self.view(locals, op);
                                let fv = with_val(&s, &pth, shadow);
                                let argv: Vec<Val> =
                                    args.iter().map(|a| self.read(locals, a)).collect();
                                match self.resolve_callee(&fv, argv) {
                                    Some(x) => {
                                        push = Some(x);
                                        None
                                    }
                                    None => Some(Ok(Val::Unit)),
                                }
                            }
                            Callee::Builtin(b) if is_task_op(*b) => {
                                if !top {
                                    Some(Err(self.stop_at(
                                        *stop,
                                        Some(StopKind::StdRequiresViolated),
                                        Some("関数値の呼び出しの中ではタスクを中断できない"),
                                    )))
                                } else {
                                    match self.task_op(locals, *b, args, *stop) {
                                        Op::Val(v) => Some(Ok(v)),
                                        Op::Push(f, a, sc) => {
                                            push = Some((f, a));
                                            self.pending_scope = sc;
                                            None
                                        }
                                        Op::Block => {
                                            block = true;
                                            None
                                        }
                                        Op::Fail(e) => Some(Err(e)),
                                    }
                                }
                            }
                            Callee::Builtin(b) => Some(self.builtin(locals, *b, args, *stop)),
                        };
                        if block {
                            Act::Suspend
                        } else if let Some((g, argv)) = push {
                            Act::Push(g, argv, *stop)
                        } else {
                            match r.expect("結果がある") {
                                Ok(v) => {
                                    self.write(locals, dest, v);
                                    fr.bb = *next;
                                }
                                Err(e) => {
                                    fr.pending = Some(e);
                                    fr.bb = *cleanup;
                                }
                            }
                            Act::Continue
                        }
                    }
                    Term::CheckedOp {
                        op,
                        a,
                        b,
                        dest,
                        next,
                        stop,
                        cleanup,
                    } => {
                        let x = self.read_int(locals, a);
                        let y = self.read_int(locals, b);
                        let r = match op {
                            ArithOp::Add => x.checked_add(y),
                            ArithOp::Sub => x.checked_sub(y),
                            ArithOp::Mul => x.checked_mul(y),
                            ArithOp::Div => x.checked_div(y),
                            ArithOp::Rem => x.checked_rem(y),
                            ArithOp::Shl => {
                                if (0..64).contains(&y) {
                                    Some(x.wrapping_shl(y as u32))
                                } else {
                                    None
                                }
                            }
                            ArithOp::Shr => {
                                if (0..64).contains(&y) {
                                    // 論理右シフト（ADR 0023 決定 3）
                                    Some((x as u64).wrapping_shr(y as u32) as i64)
                                } else {
                                    None
                                }
                            }
                        };
                        match r {
                            Some(n) => {
                                self.write(locals, dest, Val::Int(n));
                                fr.bb = *next;
                            }
                            None => {
                                fr.pending = Some(self.stop_at(*stop, None, None));
                                fr.bb = *cleanup;
                            }
                        }
                        Act::Continue
                    }
                    Term::Assert {
                        cond,
                        stop,
                        next,
                        cleanup,
                    } => {
                        if self.read_bool(locals, cond) {
                            fr.bb = *next;
                        } else {
                            fr.pending = Some(self.stop_at(*stop, None, None));
                            fr.bb = *cleanup;
                        }
                        Act::Continue
                    }
                    Term::Return => {
                        let v = std::mem::replace(&mut *locals[0].borrow_mut(), Val::Empty);
                        Act::Ret(v)
                    }
                    Term::Unreachable => unreachable!("到達しない基本ブロックに来た"),
                    Term::Resume => Act::Resume,
                }
            };
            match act {
                Act::Continue => {}
                Act::Suspend => return Run::Suspend,
                Act::Push(g, argv, stop) => {
                    if self.sch.live >= self.sch.max_depth {
                        let e = self.stop_at(stop, Some(StopKind::StackExhausted), Some(""));
                        self.pending_scope = None;
                        let fr = frames.last_mut().expect("枠がある");
                        self.deliver_err(fr, e);
                    } else {
                        let sc = self.pending_scope.take();
                        let f = self.new_frame(g, argv, stop, sc);
                        frames.push(f);
                    }
                }
                Act::Ret(v) => {
                    let fr = frames.pop().expect("枠がある");
                    self.sch.live -= 1;
                    let sc = fr.scope;
                    Self::drop_frame_locals(fr.locals);
                    if frames.is_empty() {
                        return Run::Done(v);
                    }
                    if let Some(sid) = sc {
                        if self.sch.scopes[sid].live > 0 {
                            let cur = self.sch.cur;
                            self.sch.scopes[sid].waiter = Some(cur);
                            self.sch.tasks[cur].state = State::Scoping;
                            self.sch.tasks[cur].waiting_scope = Some(sid);
                            self.sch.tasks[cur].resume = Some(v);
                            return Run::Suspend;
                        }
                        self.finish_scope(sid);
                    }
                    let fr = frames.last_mut().expect("枠がある");
                    self.deliver(fr, v);
                }
                Act::Resume => {
                    let mut fr = frames.pop().expect("枠がある");
                    self.sch.live -= 1;
                    let e = fr
                        .pending
                        .take()
                        .unwrap_or_else(|| self.stop_at(fr.loc, None, None));
                    if is_allocator_func(&self.p.funcs[fr.fid].name) {
                        self.sch.alloc_stop = true;
                    }
                    // スコープ配下の子の取り消しも巻き戻しの一部（子が持つガードでも `Lock` が壊れる）。
                    UNWINDING.with(|c| c.set(true));
                    if let Some(sid) = fr.scope {
                        self.cancel_scope(sid);
                    }
                    Self::drop_frame_locals(fr.locals);
                    UNWINDING.with(|c| c.set(false));
                    match frames.last_mut() {
                        None => return Run::Fail(e),
                        Some(fr) => self.deliver_err(fr, e),
                    }
                }
            }
        }
    }

    fn finish_scope(&mut self, sid: usize) {
        let kids = std::mem::take(&mut self.sch.scopes[sid].children);
        for c in kids {
            let r = self.sch.tasks[c].result.take();
            drop(r);
        }
    }

    fn cancel_scope(&mut self, sid: usize) {
        self.sch.scopes[sid].waiter = None;
        self.sch.scopes[sid].live = 0;
        match &mut self.sch.scopes[sid].kind {
            ScopeKind::Timeout { seq } => {
                let sq = *seq;
                self.sch.sleepers.retain(|s| s.1 != sq);
            }
            ScopeKind::AllOk { results } => {
                for r in results.drain(..).rev() {
                    drop(r);
                }
            }
            _ => {}
        }
        let kids = std::mem::take(&mut self.sch.scopes[sid].children);
        for c in kids.into_iter().rev() {
            if self.sch.tasks[c].state != State::Done {
                self.cancel_task(c);
            } else {
                let r = self.sch.tasks[c].result.take();
                drop(r);
            }
        }
    }

    fn cancel_task(&mut self, t: usize) {
        if self.sch.tasks[t].state == State::Done {
            let r = self.sch.tasks[t].result.take();
            drop(r);
            return;
        }
        self.sch.tasks[t].state = State::Done;
        self.sch.tasks[t].joiner = None;
        self.sch.ready.retain(|&x| x != t);
        self.sch
            .sleepers
            .retain(|s| !matches!(s.2, Wake::Sleep(x) if x == t));
        for ch in &mut self.sch.chans {
            ch.waiters.retain(|&x| x != t);
        }
        for l in &mut self.sch.locks {
            l.waiters.retain(|&x| x != t);
        }
        for other in &mut self.sch.tasks {
            if other.joiner == Some(t) {
                other.joiner = None;
            }
        }
        if let Some(ws) = self.sch.tasks[t].waiting_scope.take() {
            self.cancel_scope(ws);
        }
        let mut frames = std::mem::take(&mut self.sch.tasks[t].frames);
        while let Some(fr) = frames.pop() {
            self.sch.live -= 1;
            if let Some(inner_sid) = fr.scope {
                self.cancel_scope(inner_sid);
            }
            Self::drop_frame_locals(fr.locals);
        }
        let r = self.sch.tasks[t].resume.take();
        drop(r);
        let r = self.sch.tasks[t].result.take();
        drop(r);
        if let Some(sid) = self.sch.tasks[t].scope {
            self.sch.scopes[sid].live = self.sch.scopes[sid].live.saturating_sub(1);
        }
    }

    fn wake(&mut self, t: usize, v: Option<Val>) {
        let task = &mut self.sch.tasks[t];
        if v.is_some() {
            task.resume = v;
        }
        task.state = State::Ready;
        self.sch.ready.push_back(t);
    }

    fn spawn_in_scope(
        &mut self,
        sid: usize,
        slot_idx: usize,
        fid: FuncId,
        args: Vec<Val>,
        stop: StopId,
    ) -> usize {
        let frame = self.new_frame(fid, args, stop, None);
        let tid = self.sch.tasks.len();
        let root = self.sch.tasks[self.sch.cur].root;
        self.sch.tasks.push(Task {
            frames: vec![frame],
            state: State::Ready,
            resume: None,
            result: None,
            scope: Some(sid),
            slot_idx,
            joiner: None,
            waiting_scope: None,
            root,
        });
        self.sch.scopes[sid].live += 1;
        self.sch.scopes[sid].children.push(tid);
        self.sch.ready.push_back(tid);
        tid
    }

    fn task_op(&mut self, locals: &[Slot], b: Builtin, args: &[Operand], stop: StopId) -> Op {
        use Builtin::*;
        let cur = self.sch.cur;
        if !matches!(b, TaskYield | AtomicLoad | SyncSharedGet) {
            self.sch.idle_yields = 0;
        }
        match b {
            TaskScope => {
                let f = self.read(locals, &args[0]);
                let sid = self.sch.scopes.len();
                self.sch.scopes.push(ScopeSt::default());
                let sref = Val::Ref(Ptr {
                    slot: new_slot(Val::Int(sid as i64)),
                    path: vec![],
                });
                match self.resolve_callee(&f, vec![sref]) {
                    Some((g, a)) => Op::Push(g, a, Some(sid)),
                    None => Op::Val(Val::Unit),
                }
            }
            ScopeSpawn => {
                let sid = self.read_int(locals, &args[0]) as usize;
                let g = self.read(locals, &args[1]);
                let Some((fid, a)) = self.resolve_callee(&g, vec![]) else {
                    return Op::Val(Val::Int(-1));
                };
                let tid = self.spawn_in_scope(sid, 0, fid, a, stop);
                Op::Val(Val::Int(tid as i64))
            }
            TaskJoin => {
                let tid = self.read_int(locals, &args[0]) as usize;
                if self.sch.tasks[tid].state == State::Done {
                    let v = self.sch.tasks[tid].result.take().unwrap_or(Val::Unit);
                    Op::Val(v)
                } else {
                    self.sch.tasks[tid].joiner = Some(cur);
                    self.sch.tasks[cur].state = State::Joining;
                    Op::Block
                }
            }
            TaskTimeout => {
                let ms = self.read_int(locals, &args[0]).max(0);
                let g = self.read(locals, &args[1]);
                let Some((fid, a)) = self.resolve_callee(&g, vec![]) else {
                    return Op::Val(self.none());
                };
                let sid = self.sch.scopes.len();
                self.sch.seq += 1;
                let sq = self.sch.seq;
                let at = self.sch.clock.saturating_add(ms);
                self.sch.scopes.push(ScopeSt {
                    live: 0,
                    waiter: Some(cur),
                    children: vec![],
                    kind: ScopeKind::Timeout { seq: sq },
                });
                self.spawn_in_scope(sid, 0, fid, a, stop);
                self.sch.sleepers.push((at, sq, Wake::Timeout(sid)));
                self.sch.tasks[cur].state = State::Scoping;
                self.sch.tasks[cur].waiting_scope = Some(sid);
                Op::Block
            }
            TaskRace => {
                let fns = match self.owned(locals, &args[0]) {
                    Val::Agg(mut a) => {
                        a.silent = true;
                        std::mem::take(&mut a.f)
                    }
                    _ => vec![],
                };
                if fns.is_empty() {
                    return Op::Fail(self.stop_at(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("task.race: 空の一覧"),
                    ));
                }
                let sid = self.sch.scopes.len();
                self.sch.scopes.push(ScopeSt {
                    live: 0,
                    waiter: Some(cur),
                    children: vec![],
                    kind: ScopeKind::Race,
                });
                for (idx, g) in fns.into_iter().enumerate() {
                    if let Some((fid, a)) = self.resolve_callee(&g, vec![]) {
                        self.spawn_in_scope(sid, idx, fid, a, stop);
                    }
                }
                self.sch.tasks[cur].state = State::Scoping;
                self.sch.tasks[cur].waiting_scope = Some(sid);
                Op::Block
            }
            TaskAllOk => {
                let fns = match self.owned(locals, &args[0]) {
                    Val::Agg(mut a) => {
                        a.silent = true;
                        std::mem::take(&mut a.f)
                    }
                    _ => vec![],
                };
                if fns.is_empty() {
                    return Op::Val(self.ok(list_of(vec![])));
                }
                let n = fns.len();
                let sid = self.sch.scopes.len();
                self.sch.scopes.push(ScopeSt {
                    live: 0,
                    waiter: Some(cur),
                    children: vec![],
                    kind: ScopeKind::AllOk {
                        results: (0..n).map(|_| None).collect(),
                    },
                });
                for (idx, g) in fns.into_iter().enumerate() {
                    if let Some((fid, a)) = self.resolve_callee(&g, vec![]) {
                        self.spawn_in_scope(sid, idx, fid, a, stop);
                    }
                }
                self.sch.tasks[cur].state = State::Scoping;
                self.sch.tasks[cur].waiting_scope = Some(sid);
                Op::Block
            }
            TaskYield => {
                self.sch.idle_yields += 1;
                if self.sch.idle_yields > MAX_IDLE_YIELDS && self.sch.sleepers.is_empty() {
                    self.sch.idle_yields = 0;
                    return Op::Fail(self.stop_at(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("デッドロック（task.yield だけで進行しない）"),
                    ));
                }
                self.sch.tasks[cur].resume = Some(Val::Unit);
                self.sch.tasks[cur].state = State::Ready;
                self.sch.ready.push_back(cur);
                Op::Block
            }
            TaskSleep => {
                let ms = self.read_int(locals, &args[0]).max(0);
                let at = self.sch.clock.saturating_add(ms);
                self.sch.seq += 1;
                let sq = self.sch.seq;
                self.sch.sleepers.push((at, sq, Wake::Sleep(cur)));
                self.sch.tasks[cur].resume = Some(Val::Unit);
                self.sch.tasks[cur].state = State::Sleeping;
                Op::Block
            }
            TaskChannel => {
                let id = self.sch.chans.len() as i64;
                let root = self.sch.tasks[cur].root;
                self.sch.chans.push(Chan {
                    root,
                    ..Chan::default()
                });
                Op::Val(Val::Agg(Box::new(Agg {
                    tag: Tag::Tuple,
                    f: vec![Val::Int(id), Val::Int(id)],
                    silent: false,
                })))
            }
            ChanSend => {
                let id = self.read_int(locals, &args[0]) as usize;
                let v = self.owned(locals, &args[1]);
                if let Some(w) = self.sch.chans[id].waiters.pop_front() {
                    self.wake(w, Some(v));
                } else {
                    self.sch.chans[id].q.push_back(v);
                }
                Op::Val(Val::Unit)
            }
            ChanRecv => {
                let id = self.read_int(locals, &args[0]) as usize;
                if let Some(v) = self.sch.chans[id].q.pop_front() {
                    Op::Val(v)
                } else {
                    self.sch.chans[id].waiters.push_back(cur);
                    self.sch.tasks[cur].state = State::Recv;
                    Op::Block
                }
            }
            SyncSharedNew | AtomicNew => Op::Val(Val::Cell(Rc::new(RefCell::new(
                self.owned(locals, &args[0]),
            )))),
            SyncSharedGet => {
                let (s, p) = self.view(locals, &args[0]);
                Op::Val(with_val(&s, &p, |v| match v {
                    Val::Cell(c) => shadow(&c.borrow()),
                    _ => Val::Unit,
                }))
            }
            AtomicLoad => {
                // `Atomic.load` は中断点（ADR 0007 決定 3・決定 6 条件 11）。
                let (s, p) = self.view(locals, &args[0]);
                let val = with_val(&s, &p, |v| match v {
                    Val::Cell(c) => shadow(&c.borrow()),
                    _ => Val::Unit,
                });
                self.sch.tasks[cur].resume = Some(val);
                self.sch.tasks[cur].state = State::Ready;
                self.sch.ready.push_back(cur);
                Op::Block
            }
            AtomicStore => {
                let (s, p) = self.view(locals, &args[0]);
                let x = self.owned(locals, &args[1]);
                let old = with_val_mut(&s, &p, |v| match v {
                    Val::Cell(c) => Some(std::mem::replace(&mut *c.borrow_mut(), x)),
                    _ => None,
                })
                .flatten();
                drop(old);
                Op::Val(Val::Unit)
            }
            LockNew => {
                let value = self.owned(locals, &args[0]);
                let root = self.sch.tasks[cur].root;
                let id = self.sch.locks.len();
                self.sch.locks.push(LockSt {
                    value,
                    holder: None,
                    waiters: VecDeque::new(),
                    broken: false,
                    root,
                });
                Op::Val(Val::Int(id as i64))
            }
            LockAcquire => {
                // 直前の破棄による解放を先に反映する。
                self.settle_locks();
                let id = self.read_int(locals, &args[0]) as usize;
                let l = &mut self.sch.locks[id];
                if l.broken {
                    Op::Val(self.lock_broken())
                } else if l.holder.is_none() {
                    l.holder = Some(cur);
                    Op::Val(self.ok(Val::Guard(Rc::new(GuardTok { lock: id }))))
                } else {
                    l.waiters.push_back(cur);
                    self.sch.tasks[cur].state = State::LockWait;
                    Op::Block
                }
            }
            _ => Op::Fail(self.stop_at(
                stop,
                Some(StopKind::StdRequiresViolated),
                Some("未対応のタスク操作"),
            )),
        }
    }

    /// `Err(LockBroken)`。
    fn lock_broken(&self) -> Val {
        let lb = self.p.types.iter().position(|t| t.name == "LockBroken");
        let inner = Val::Agg(Box::new(Agg {
            tag: Tag::Struct(lb.expect("`Lock` を使うプログラムには `LockBroken` の型がある")),
            f: vec![],
            silent: false,
        }));
        self.err(inner)
    }

    /// プロセス全体の停止。全ルートタスクを取り消して、元の停止を返す。
    fn stop_all(&mut self, e: E) -> Result<Vec<Result<Val, E>>, E> {
        self.cancel_all();
        self.settle_locks();
        self.take_deep();
        Err(e)
    }

    /// `LockGuard` の破棄の知らせを反映する。解放なら次の待機者へ渡し、異常停止の巻き戻しでの破棄なら
    /// `Lock` を壊して、待機者をすべて起こして `Err(LockBroken)` を返す（ADR 0026 決定 4）。
    fn settle_locks(&mut self) {
        let events = LOCK_EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
        for (id, unwinding) in events {
            let l = &mut self.sch.locks[id];
            l.holder = None;
            if unwinding {
                l.broken = true;
            }
            if l.broken {
                let ws: Vec<usize> = l.waiters.drain(..).collect();
                for w in ws {
                    let e = self.lock_broken();
                    self.wake(w, Some(e));
                }
            } else if let Some(w) = l.waiters.pop_front() {
                l.holder = Some(w);
                let g = self.ok(Val::Guard(Rc::new(GuardTok { lock: id })));
                self.wake(w, Some(g));
            }
        }
    }

    fn complete(&mut self, t: usize, v: Val) {
        let (joiner, scope, slot_idx) = {
            let task = &mut self.sch.tasks[t];
            task.state = State::Done;
            (task.joiner.take(), task.scope, task.slot_idx)
        };
        let Some(sid) = scope else {
            if let Some(j) = joiner {
                self.wake(j, Some(v));
            } else {
                self.sch.tasks[t].result = Some(v);
            }
            return;
        };
        match &mut self.sch.scopes[sid].kind {
            ScopeKind::Normal => {
                match joiner {
                    Some(j) => self.wake(j, Some(v)),
                    None => self.sch.tasks[t].result = Some(v),
                }
                let sc = &mut self.sch.scopes[sid];
                sc.live = sc.live.saturating_sub(1);
                if sc.live == 0 {
                    if let Some(w) = sc.waiter.take() {
                        self.sch.tasks[w].waiting_scope = None;
                        self.finish_scope(sid);
                        self.wake(w, None);
                    }
                }
            }
            ScopeKind::Timeout { seq } => {
                let sq = *seq;
                let waiter = self.sch.scopes[sid].waiter.take();
                self.sch.scopes[sid].live = 0;
                self.sch.sleepers.retain(|s| s.1 != sq);
                if let Some(w) = waiter {
                    self.sch.tasks[w].waiting_scope = None;
                    self.finish_scope(sid);
                    let some_v = self.some(v);
                    self.wake(w, Some(some_v));
                } else {
                    drop(v);
                }
            }
            ScopeKind::Race => {
                if let Some(w) = self.sch.scopes[sid].waiter.take() {
                    self.cancel_scope(sid);
                    self.sch.tasks[w].waiting_scope = None;
                    self.wake(w, Some(v));
                } else {
                    drop(v);
                }
            }
            ScopeKind::AllOk { .. } => {
                let is_err =
                    matches!(&v, Val::Agg(a) if matches!(a.tag, Tag::Variant(_, RESULT_ERR)));
                if is_err {
                    if let Some(w) = self.sch.scopes[sid].waiter.take() {
                        if let ScopeKind::AllOk { results } = &mut self.sch.scopes[sid].kind {
                            for r in results.drain(..).rev() {
                                drop(r);
                            }
                        }
                        self.cancel_scope(sid);
                        self.sch.tasks[w].waiting_scope = None;
                        self.wake(w, Some(v));
                    } else {
                        drop(v);
                    }
                } else {
                    let ok_val = match v {
                        Val::Agg(mut a) => {
                            a.silent = true;
                            a.f.pop().unwrap_or(Val::Unit)
                        }
                        other => other,
                    };
                    let done = {
                        let sc = &mut self.sch.scopes[sid];
                        if let ScopeKind::AllOk { results } = &mut sc.kind {
                            if slot_idx < results.len() {
                                results[slot_idx] = Some(ok_val);
                            }
                        }
                        sc.live = sc.live.saturating_sub(1);
                        sc.live == 0
                    };
                    if done {
                        if let Some(w) = self.sch.scopes[sid].waiter.take() {
                            let vals = match &mut self.sch.scopes[sid].kind {
                                ScopeKind::AllOk { results } => {
                                    results.drain(..).map(|x| x.unwrap_or(Val::Unit)).collect()
                                }
                                _ => vec![],
                            };
                            self.sch.tasks[w].waiting_scope = None;
                            self.finish_scope(sid);
                            let res_val = self.ok(list_of(vals));
                            self.wake(w, Some(res_val));
                        }
                    }
                }
            }
        }
    }

    /// 残っているタスクを、新しいものから順に取り消す。枠は内側から外側へ畳み、局所は逆順に破棄する。
    fn cancel_all(&mut self) {
        // 全体停止の後始末。破棄は異常停止の巻き戻しとして扱う（持っていた `Lock` は壊れる）。
        UNWINDING.with(|c| c.set(true));
        self.cancel_all_inner();
        UNWINDING.with(|c| c.set(false));
    }

    fn cancel_all_inner(&mut self) {
        for t in (0..self.sch.tasks.len()).rev() {
            if self.sch.tasks[t].state != State::Done {
                self.cancel_task(t);
            } else {
                let r = self.sch.tasks[t].result.take();
                drop(r);
            }
        }
        for c in std::mem::take(&mut self.sch.chans) {
            drop(c);
        }
    }

    /// ルートタスク 1 つ分の後始末。そのルートに属するタスクを新しいものから順に取り消し、
    /// 作った通信路と `Lock` の値を捨てる（異常停止の破棄の範囲。ADR 0026 決定 4）。
    fn stop_root(&mut self, root: usize) {
        // 取り消しの間の破棄は異常停止の巻き戻しとして扱う。親や兄弟のタスクが持つ `LockGuard` の破棄でも
        // `Lock` が壊れる（ADR 0026 決定 4）。
        UNWINDING.with(|c| c.set(true));
        self.stop_root_inner(root);
        UNWINDING.with(|c| c.set(false));
        self.settle_locks();
    }

    fn stop_root_inner(&mut self, root: usize) {
        for t in (0..self.sch.tasks.len()).rev() {
            if self.sch.tasks[t].root != root {
                continue;
            }
            if self.sch.tasks[t].state != State::Done {
                self.cancel_task(t);
            } else {
                let r = self.sch.tasks[t].result.take();
                drop(r);
            }
        }
        for ch in self.sch.chans.iter_mut().filter(|c| c.root == root) {
            let q = std::mem::take(&mut ch.q);
            drop(q);
            ch.waiters.clear();
        }
        let mut dead = vec![];
        for l in self.sch.locks.iter_mut().filter(|l| l.root == root) {
            l.waiters.clear();
            l.broken = true;
            dead.push(std::mem::replace(&mut l.value, Val::Unit));
        }
        drop(dead);
    }

    /// ルートタスクを実行する。既定は決定的な FIFO（`schedule_seed` 指定時は種による攪拌）で、中断点でだけ切り替える。
    ///
    /// ルートタスクごとに結果を返す。ルートタスクの異常停止は、そのルートタスクと子のタスクだけを止める
    /// （他のルートタスクは止めない）。スタックの枯渇と、確保器の枠の中の異常停止は、プロセス全体の停止として
    /// 外側の `Err` で返す（ADR 0026 決定 4）。
    pub(super) fn run_roots(
        &mut self,
        fid: FuncId,
        arg_sets: Vec<Vec<Val>>,
    ) -> Result<Vec<Result<Val, E>>, E> {
        for args in arg_sets {
            let frame = self.new_frame(fid, args, 0, None);
            let tid = self.sch.tasks.len();
            self.sch.tasks.push(Task {
                frames: vec![frame],
                state: State::Ready,
                resume: None,
                result: None,
                scope: None,
                slot_idx: 0,
                joiner: None,
                waiting_scope: None,
                root: tid,
            });
            self.sch.roots.push(tid);
            self.sch.results.push(None);
            self.sch.ready.push_back(tid);
        }
        loop {
            // どのルートタスクにも属さない `DEEP`（破棄・取り消しの途中で立ったもの）は、全体停止にする。
            if let Some(e) = self.take_deep() {
                self.cancel_all();
                self.settle_locks();
                self.take_deep();
                return Err(e);
            }
            if self.sch.results.iter().all(Option::is_some) {
                return Ok(std::mem::take(&mut self.sch.results)
                    .into_iter()
                    .map(|r| r.expect("結果がある"))
                    .collect());
            }
            self.settle_locks();
            let Some(t) = self.sch.pop_ready() else {
                if self.sch.sleepers.is_empty() {
                    // 進行できるタスクがない。結果の出ていないルートタスクのうち一番新しいものを 1 つだけ
                    // デッドロックとして止め、次の反復で進行できるかを判定し直す（止めたことで起きた
                    // ルートタスクを、続けて潰さない）。
                    if let Some(i) = (0..self.sch.roots.len())
                        .rev()
                        .find(|&i| self.sch.results[i].is_none())
                    {
                        let root = self.sch.roots[i];
                        self.stop_root(root);
                        self.sch.results[i] = Some(Err(self.stop_at(
                            0,
                            Some(StopKind::StdRequiresViolated),
                            Some("デッドロック（進行できるタスクがない）"),
                        )));
                    }
                    continue;
                }
                let at = self
                    .sch
                    .sleepers
                    .iter()
                    .map(|s| (s.0, s.1))
                    .min()
                    .expect("要素")
                    .0;
                self.sch.clock = at;
                let mut due: Vec<(u64, Wake)> = vec![];
                self.sch.sleepers.retain(|&(a, sq, w)| {
                    if a == at {
                        due.push((sq, w));
                        false
                    } else {
                        true
                    }
                });
                due.sort_by_key(|&(sq, _)| sq);
                for (_, w) in due {
                    match w {
                        Wake::Sleep(t) => {
                            if self.sch.tasks[t].state == State::Sleeping {
                                self.wake(t, None);
                            }
                        }
                        Wake::Timeout(sid) => {
                            if let Some(w) = self.sch.scopes[sid].waiter.take() {
                                self.cancel_scope(sid);
                                self.sch.tasks[w].waiting_scope = None;
                                let none = self.none();
                                self.wake(w, Some(none));
                            }
                        }
                    }
                }
                continue;
            };
            self.sch.cur = t;
            let mut frames = std::mem::take(&mut self.sch.tasks[t].frames);
            if let Some(v) = self.sch.tasks[t].resume.take() {
                if let Some(fr) = frames.last_mut() {
                    self.deliver(fr, v);
                }
            }
            self.sch.tasks[t].state = State::Running;
            self.sch.alloc_stop = false;
            match self.drive(&mut frames, true) {
                Run::Suspend => self.sch.tasks[t].frames = frames,
                Run::Done(v) => {
                    self.sch.idle_yields = 0;
                    if let Some(i) = self.sch.roots.iter().position(|&r| r == t) {
                        self.sch.tasks[t].state = State::Done;
                        self.sch.results[i] = Some(Ok(v));
                    } else {
                        self.complete(t, v);
                    }
                }
                Run::Fail(e) => {
                    let alloc = std::mem::take(&mut self.sch.alloc_stop);
                    if e.kind == StopKind::StackExhausted {
                        // スタックの枯渇は、どのルートタスクでも全体停止（隔離しない）。
                        return self.stop_all(e);
                    }
                    if alloc {
                        // 確保器の枠の中の異常停止は全体停止（隔離しない）。
                        return self.stop_all(e);
                    }
                    let root = self.sch.tasks[t].root;
                    self.stop_root(root);
                    if let Some(i) = self.sch.roots.iter().position(|&r| r == root) {
                        self.sch.results[i] = Some(Err(e));
                    }
                }
            }
        }
    }
}

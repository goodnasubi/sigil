//! 評価器の枠（フレーム）の積み重ねとタスクのスケジューラ（ADR 0026 決定 5・6）。
//!
//! 呼び出しは Rust の再帰ではなく、明示的な枠の積み重ね（`Vec<Frame>`）で行う。
//! タスクは枠の積み重ねを状態として持ち、中断点（呼び出しの終端）でだけ切り替わる。
//! スケジューラは決定的な FIFO で、時計は仮想（実時間では待たない）。

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use crate::exec::StopKind;
use crate::ir::*;

use super::value::{new_slot, shadow, with_val, with_val_mut, Agg, Ptr, Tag, Val};
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
    joiner: Option<usize>,
}

#[derive(Default)]
struct ScopeSt {
    live: usize,
    waiter: Option<usize>,
    children: Vec<usize>,
}

#[derive(Default)]
struct Chan {
    q: VecDeque<Val>,
    waiters: VecDeque<usize>,
}

pub(super) struct Sched {
    tasks: Vec<Task>,
    ready: VecDeque<usize>,
    scopes: Vec<ScopeSt>,
    chans: Vec<Chan>,
    sleepers: Vec<(i64, u64, usize)>,
    clock: i64,
    seq: u64,
    cur: usize,
    live: usize,
    max_depth: usize,
}

impl Sched {
    pub(super) fn new(stack_bytes: usize) -> Sched {
        Sched {
            tasks: vec![],
            ready: VecDeque::new(),
            scopes: vec![],
            chans: vec![],
            sleepers: vec![],
            clock: 0,
            seq: 0,
            cur: 0,
            live: 0,
            // 枠 1 つを約 1 KiB と見なして、深さの上限を決める（スタックの枯渇の判定）。
            max_depth: (stack_bytes / 1024).max(64),
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
                for s in &blk.stmts {
                    self.exec(&fr.locals, s);
                }
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
                    drop(fr);
                    if frames.is_empty() {
                        return Run::Done(v);
                    }
                    if let Some(sid) = sc {
                        if self.sch.scopes[sid].live > 0 {
                            let cur = self.sch.cur;
                            self.sch.scopes[sid].waiter = Some(cur);
                            self.sch.tasks[cur].state = State::Scoping;
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
                    drop(fr);
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

    fn wake(&mut self, t: usize, v: Option<Val>) {
        let task = &mut self.sch.tasks[t];
        if v.is_some() {
            task.resume = v;
        }
        task.state = State::Ready;
        self.sch.ready.push_back(t);
    }

    fn task_op(&mut self, locals: &[Slot], b: Builtin, args: &[Operand], stop: StopId) -> Op {
        use Builtin::*;
        let cur = self.sch.cur;
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
                let frame = self.new_frame(fid, a, stop, None);
                let tid = self.sch.tasks.len();
                self.sch.tasks.push(Task {
                    frames: vec![frame],
                    state: State::Ready,
                    resume: None,
                    result: None,
                    scope: Some(sid),
                    joiner: None,
                });
                self.sch.scopes[sid].live += 1;
                self.sch.scopes[sid].children.push(tid);
                self.sch.ready.push_back(tid);
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
            TaskYield => {
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
                self.sch.sleepers.push((at, sq, cur));
                self.sch.tasks[cur].resume = Some(Val::Unit);
                self.sch.tasks[cur].state = State::Sleeping;
                Op::Block
            }
            TaskChannel => {
                let id = self.sch.chans.len() as i64;
                self.sch.chans.push(Chan::default());
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
            SyncSharedGet | AtomicLoad => {
                let (s, p) = self.view(locals, &args[0]);
                Op::Val(with_val(&s, &p, |v| match v {
                    Val::Cell(c) => shadow(&c.borrow()),
                    _ => Val::Unit,
                }))
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
            _ => Op::Fail(self.stop_at(
                stop,
                Some(StopKind::StdRequiresViolated),
                Some("Lock は評価器でまだ実行できない"),
            )),
        }
    }

    fn complete(&mut self, t: usize, v: Val) {
        let (joiner, scope) = {
            let task = &mut self.sch.tasks[t];
            task.state = State::Done;
            (task.joiner.take(), task.scope)
        };
        match joiner {
            Some(j) => self.wake(j, Some(v)),
            None => self.sch.tasks[t].result = Some(v),
        }
        if let Some(sid) = scope {
            let sc = &mut self.sch.scopes[sid];
            sc.live -= 1;
            if sc.live == 0 {
                if let Some(w) = sc.waiter.take() {
                    self.finish_scope(sid);
                    self.wake(w, None);
                }
            }
        }
    }

    /// 残っているタスクを、新しいものから順に取り消す。枠は内側から外側へ畳み、局所は逆順に破棄する。
    fn cancel_all(&mut self) {
        for t in (0..self.sch.tasks.len()).rev() {
            let mut frames = std::mem::take(&mut self.sch.tasks[t].frames);
            while let Some(fr) = frames.pop() {
                self.sch.live -= 1;
                let mut locals = fr.locals;
                while let Some(s) = locals.pop() {
                    drop(s);
                }
            }
            let r = self.sch.tasks[t].resume.take();
            drop(r);
            let r = self.sch.tasks[t].result.take();
            drop(r);
        }
        for c in std::mem::take(&mut self.sch.chans) {
            drop(c);
        }
    }

    /// ルートタスクを実行する。決定的な FIFO で、中断点でだけ切り替える。
    pub(super) fn run_root(&mut self, fid: FuncId, args: Vec<Val>) -> Result<Val, E> {
        let frame = self.new_frame(fid, args, 0, None);
        self.sch.tasks.push(Task {
            frames: vec![frame],
            state: State::Ready,
            resume: None,
            result: None,
            scope: None,
            joiner: None,
        });
        self.sch.ready.push_back(0);
        loop {
            let Some(t) = self.sch.ready.pop_front() else {
                if self.sch.sleepers.is_empty() {
                    self.cancel_all();
                    return Err(self.stop_at(
                        0,
                        Some(StopKind::StdRequiresViolated),
                        Some("デッドロック（進行できるタスクがない）"),
                    ));
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
                let mut due: Vec<(u64, usize)> = vec![];
                self.sch.sleepers.retain(|&(a, sq, t)| {
                    if a == at {
                        due.push((sq, t));
                        false
                    } else {
                        true
                    }
                });
                due.sort();
                for (_, t) in due {
                    self.wake(t, None);
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
            match self.drive(&mut frames, true) {
                Run::Suspend => self.sch.tasks[t].frames = frames,
                Run::Done(v) => {
                    if t == 0 {
                        return Ok(v);
                    }
                    self.complete(t, v);
                }
                Run::Fail(e) => {
                    self.cancel_all();
                    return Err(e);
                }
            }
        }
    }
}

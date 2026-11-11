//! 評価器の枠（フレーム）の積み重ねとタスクのスケジューラ（ADR 0026 決定 5・6）。
//!
//! 呼び出しは Rust の再帰ではなく、明示的な枠の積み重ね（`Vec<Frame>`）で行う。
//! タスクは枠の積み重ねを状態として持ち、中断点（呼び出しの終端）でだけ切り替わる。
//! スケジューラは既定で決定的な FIFO（`Options::schedule_seed` で乱数の種による攪拌に差し替え可能）で、
//! 時計は仮想（実時間では待たない）。

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::exec::StopKind;
use crate::ir::*;

use super::value::{
    list_of, new_slot, shadow, with_val, Agg, GuardTok, Ptr, RootSlotTok, Tag, Val, LOCK_EVENTS,
    ROOT_EVENTS, UNWINDING,
};
use super::{Slot, E, M};

#[cfg(unix)]
#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

#[cfg(unix)]
extern "C" {
    fn poll(fds: *mut PollFd, nfds: std::ffi::c_ulong, timeout: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

#[cfg(unix)]
fn is_valid_fd(fd: i32) -> bool {
    const F_GETFD: i32 = 1;
    // SAFETY: `F_GETFD` は追加の引数を取らず、記述子が有効かだけを検査する。
    unsafe { fcntl(fd, F_GETFD) != -1 }
}

#[cfg(not(unix))]
fn is_valid_fd(_fd: i32) -> bool {
    false
}

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
    IoWait,
    /// `RootGroup.reserve` で、空きのスロットを待っている。
    RootWait,
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
    /// 直近の `task.yield` での（枠の指紋, 共有の状態の版, 同じ状態が 2 回続いたか）。
    spin: Option<(u64, u64, bool)>,
    /// `RootSlot.start` が起動したルートタスク（`g` または `on_abort`）なら、その記録の番号（`Sched.started`）。
    started: Option<usize>,
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
    /// `task.roots` の本体（`Sched.groups` の番号）。`live` は起動済みで終わっていないルートの数。
    Roots {
        group: usize,
    },
}

/// `task.roots` のルートの集まり（ADR 0031 決定 4-1）。
struct GroupSt {
    /// 本体の枠が待つ範囲（`ScopeKind::Roots`）。
    sid: usize,
    limit: usize,
    /// 使っているスロットの数（予約済みの `RootSlot` と、起動済みで終わっていないルート）。
    used: usize,
    /// 本体が戻った、または集まりが取り消された。以後の `reserve`・`start` は停止にする。
    closed: bool,
    /// 空きを待っている `reserve` のタスク（到着順）。
    waiters: VecDeque<usize>,
    /// 起動したルートの記録（`Sched.started` の番号。起動の順）。
    starts: Vec<usize>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Phase {
    /// `g(&conn)` を実行している。
    Body,
    /// `g` のルートが異常停止し、`on_abort(conn)` を実行している（決定 4-2 の案 2）。
    Abort,
    Done,
}

/// 起動したルート 1 つの記録。接続（`conn`）はルートの外のここが所有し、ルートの中のタスクは借用だけを持つ。
struct StartedSt {
    group: usize,
    /// いま走っているルートタスクの番号（`g` のルート、のち `on_abort` のルート）。
    task: usize,
    /// 接続の置き場。`g` へは借用で渡す。`on_abort` へ渡したら空になる。
    conn: Slot,
    on_abort: Option<Val>,
    phase: Phase,
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
    /// 作ったルートが終わって回収した（以後の使用は停止。ADR 0031 決定 4-2）。
    reclaimed: bool,
}

/// `Lock` の状態。値は `Lock.new` で渡されたもの（評価器では番号で指す）。
struct LockSt {
    /// 保護対象の置き場。`GuardTok.slot` が同じ `Slot` を共有する。
    value: Slot,
    holder: Option<usize>,
    waiters: VecDeque<usize>,
    /// 異常停止したタスクが持っていた（壊れた）。一度壊れたら戻さない（ADR 0026 決定 4）。
    broken: bool,
    /// 作ったルートタスク。ホストが共有するものは `usize::MAX`。
    root: usize,
    /// 作ったルート（`task.roots` が起動したもの）が正常に終わって回収した（以後の取得は停止）。
    reclaimed: bool,
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

/// `Val` の内容の指紋を `h` に足す。内容を完全には辿れない値（`LockGuard` など）、深すぎる値、
/// 辿るノード数が予算 `n` を超える値では `false`（局所が大きいタスクの `yield` を遅くしないため）。
///
/// 参照・共有の値は、指す先の場所ではなく中身で指紋を取る（呼び出しのたびに作り直される一時の場所に
/// 左右されないため）。
fn fp_val(v: &Val, h: &mut DefaultHasher, d: u32, n: &mut usize) -> bool {
    if d > 12 || *n == 0 {
        return false;
    }
    *n -= 1;
    match v {
        Val::Empty => 0u8.hash(h),
        Val::Unit => 1u8.hash(h),
        Val::Int(n) => (2u8, *n).hash(h),
        Val::Float(x) => (3u8, x.to_bits()).hash(h),
        Val::Bool(b) => (4u8, *b).hash(h),
        Val::Str(x) => (5u8, &**x).hash(h),
        Val::Path(x) => (6u8, &**x).hash(h),
        Val::ParseError => 7u8.hash(h),
        Val::Fn(f) => (8u8, *f).hash(h),
        Val::Addr(a) => (9u8, *a).hash(h),
        Val::Agg(a) => {
            match a.tag {
                Tag::Tuple => (10u8, 0usize, 0usize).hash(h),
                Tag::List => (11u8, 0, 0).hash(h),
                Tag::Struct(i) => (12u8, i, 0).hash(h),
                Tag::Variant(i, j) => (13u8, i, j).hash(h),
            }
            a.f.len().hash(h);
            return a.f.iter().all(|x| fp_val(x, h, d + 1, n));
        }
        Val::Ref(p) => {
            14u8.hash(h);
            return p.with(|t| fp_val(t, h, d + 1, n));
        }
        Val::Closure(c) => {
            (15u8, c.func).hash(h);
            return with_val(&c.env, &[], |t| fp_val(t, h, d + 1, n));
        }
        Val::Cell(c) => {
            16u8.hash(h);
            return match c.try_borrow() {
                Ok(b) => fp_val(&b, h, d + 1, n),
                Err(_) => false,
            };
        }
        Val::Shared(x) => {
            17u8.hash(h);
            return fp_val(x, h, d + 1, n);
        }
        Val::Bytes(b) => match b.try_borrow() {
            Ok(b) => (18u8, &*b).hash(h),
            Err(_) => return false,
        },
        Val::Guard(_) | Val::RootSlot(_) => return false,
    }
    true
}

/// 指紋を取るときに辿る値のノード数の予算（暫定）。超えたタスクは検出の対象にしない。
/// 局所の小さい空転ループ（フラグ待ちなど）が入り、大きな `List` を持つタスクの `yield` が
/// 遅くならない大きさとして選んだ暫定値であり、仕様ではない。
const FP_BUDGET: usize = 2000;

/// タスクの枠の積み重ね全体の指紋。辿れない値があれば `None`（検出の対象にしない）。
fn fp_frames(frames: &[Frame]) -> Option<u64> {
    let mut h = DefaultHasher::new();
    let mut budget = FP_BUDGET;
    for f in frames {
        (f.fid, f.bb, f.locals.len()).hash(&mut h);
        for sl in &f.locals {
            match sl.try_borrow() {
                Ok(v) => {
                    if !fp_val(&v, &mut h, 0, &mut budget) {
                        return None;
                    }
                }
                Err(_) => return None,
            }
        }
    }
    Some(h.finish())
}

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
    /// `task.roots` の集まりと、起動したルートの記録（ADR 0031 決定 4）。
    groups: Vec<GroupSt>,
    started: Vec<StartedSt>,
    /// 外の待ち（`raw.io_wait_fd`）の登録。素の `fd` をキーとし、`(タスク番号, イベントマスク)` の一覧を持つ。
    io_waiters: BTreeMap<i32, Vec<(usize, i16)>>,
    /// 実時間モードの起点 `(開始時刻, その時点の仮想時計)`。最初の `raw.io_wait_fd` 登録で設定され、以後は戻らない（ADR 0031 決定 1-3）。
    realtime: Option<(Instant, i64)>,
    clock: i64,
    seq: u64,
    cur: usize,
    live: usize,
    max_depth: usize,
    rng: Option<u64>,
    /// 共有の状態の版。`task.yield` と読み出し以外のタスク操作・起床・タスクの完了・ルートタスクの停止で進む。
    ver: u64,
    /// 直前の実行が `task.yield` で中断した。
    yielded: bool,
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
            groups: vec![],
            started: vec![],
            io_waiters: BTreeMap::new(),
            realtime: None,
            clock: 0,
            seq: 0,
            cur: 0,
            live: 0,
            // 枠 1 つを約 1 KiB と見なして、深さの上限を決める（スタックの枯渇の判定）。
            max_depth: (stack_bytes / 1024).max(64),
            rng: schedule_seed,
            ver: 0,
            yielded: false,
        }
    }

    /// ホストが全ルートタスクに共有する `Lock` を作る（どのルートタスクにも属さない）。
    pub(super) fn new_shared_lock(&mut self, value: Val) -> usize {
        self.locks.push(LockSt {
            value: new_slot(value),
            holder: None,
            waiters: VecDeque::new(),
            broken: false,
            root: usize::MAX,
            reclaimed: false,
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
        IoWaitFd
            | RootsScope
            | RootReserve
            | RootStart
            | TaskScope
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
            | LockGuardGet
            | LockGuardGetMut
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
                        // `task.roots` の本体が戻ったら、以後の起動を止める（起動済みのルートは待つ）。
                        if let ScopeKind::Roots { group } = self.sch.scopes[sid].kind {
                            self.sch.groups[group].closed = true;
                        }
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
            ScopeKind::Roots { group } => {
                let g = *group;
                self.cancel_roots(g);
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
        for waiters in self.sch.io_waiters.values_mut() {
            waiters.retain(|&(w, _)| w != t);
        }
        self.sch.io_waiters.retain(|_, waiters| !waiters.is_empty());
        self.sch
            .sleepers
            .retain(|s| !matches!(s.2, Wake::Sleep(x) if x == t));
        for ch in &mut self.sch.chans {
            ch.waiters.retain(|&x| x != t);
        }
        for l in &mut self.sch.locks {
            l.waiters.retain(|&x| x != t);
        }
        for g in &mut self.sch.groups {
            g.waiters.retain(|&x| x != t);
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
        self.sch.ver += 1;
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
            spin: None,
            started: None,
        });
        self.sch.scopes[sid].live += 1;
        self.sch.scopes[sid].children.push(tid);
        self.sch.ready.push_back(tid);
        tid
    }

    fn enter_realtime(&mut self) {
        if self.sch.realtime.is_none() {
            self.sch.realtime = Some((Instant::now(), self.sch.clock));
        }
    }

    fn rt_clock(&self) -> i64 {
        match self.sch.realtime {
            Some((t0, clk0)) => {
                let elapsed_ms = t0.elapsed().as_millis().min(i64::MAX as u128) as i64;
                clk0.saturating_add(elapsed_ms).max(self.sch.clock)
            }
            None => self.sch.clock,
        }
    }

    fn rt_clock_up(&self) -> i64 {
        match self.sch.realtime {
            Some((t0, clk0)) => {
                let ns = t0.elapsed().as_nanos();
                let elapsed_ms = ns
                    .saturating_add(999_999)
                    .div_euclid(1_000_000)
                    .min(i64::MAX as u128) as i64;
                clk0.saturating_add(elapsed_ms).max(self.sch.clock)
            }
            None => self.sch.clock,
        }
    }

    fn due_sleepers(&mut self, clock: i64) {
        self.sch.clock = clock;
        let mut due: Vec<(i64, u64, Wake)> = vec![];
        self.sch.sleepers.retain(|&(a, sq, w)| {
            if a <= clock {
                due.push((a, sq, w));
                false
            } else {
                true
            }
        });
        due.sort_by_key(|&(a, sq, _)| (a, sq));
        for (_, _, w) in due {
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
    }

    fn poll_realtime(&mut self, wait: bool) {
        if self.sch.realtime.is_none() {
            return;
        }
        let clock = self.rt_clock();
        self.due_sleepers(clock);
        let timeout_ms: i32 = if !wait || !self.sch.ready.is_empty() {
            0
        } else if let Some(min_at) = self.sch.sleepers.iter().map(|s| s.0).min() {
            let now = self.rt_clock();
            if min_at <= now {
                0
            } else {
                (min_at - now).min(50) as i32
            }
        } else if !self.sch.io_waiters.is_empty() {
            50
        } else {
            0
        };
        if !self.sch.io_waiters.is_empty() {
            #[cfg(unix)]
            {
                const POLLERR: i16 = 0x0008;
                const POLLHUP: i16 = 0x0010;
                const POLLNVAL: i16 = 0x0020;
                const EBADF: i64 = 9;
                let mut fds: Vec<PollFd> = self
                    .sch
                    .io_waiters
                    .iter()
                    .map(|(&fd, waiters)| {
                        let events = waiters.iter().fold(0i16, |acc, &(_, ev)| acc | ev);
                        PollFd {
                            fd,
                            events,
                            revents: 0,
                        }
                    })
                    .collect();
                // SAFETY: `fds` は有効な配列を指し、`fds.len()` 件を `poll` に渡す。
                let n =
                    unsafe { poll(fds.as_mut_ptr(), fds.len() as std::ffi::c_ulong, timeout_ms) };
                if n > 0 {
                    for pfd in fds {
                        if pfd.revents != 0 {
                            if let Some(waiters) = self.sch.io_waiters.remove(&pfd.fd) {
                                let mut remaining = Vec::new();
                                for (tid, events) in waiters {
                                    let matched = (pfd.revents
                                        & (events | POLLERR | POLLHUP | POLLNVAL))
                                        != 0;
                                    if matched {
                                        if self.sch.tasks[tid].state == State::IoWait {
                                            let val = if (pfd.revents & POLLNVAL) != 0 {
                                                -EBADF
                                            } else {
                                                (pfd.revents as u16) as i64
                                            };
                                            self.wake(tid, Some(Val::Int(val)));
                                        }
                                    } else {
                                        remaining.push((tid, events));
                                    }
                                }
                                if !remaining.is_empty() {
                                    self.sch.io_waiters.insert(pfd.fd, remaining);
                                }
                            }
                        }
                    }
                }
            }
            #[cfg(not(unix))]
            if timeout_ms > 0 {
                std::thread::sleep(Duration::from_millis(timeout_ms as u64));
            }
        } else if timeout_ms > 0 {
            std::thread::sleep(Duration::from_millis(timeout_ms as u64));
        }
        let clock = self.rt_clock();
        self.due_sleepers(clock);
    }

    fn task_op(&mut self, locals: &[Slot], b: Builtin, args: &[Operand], stop: StopId) -> Op {
        use Builtin::*;
        let cur = self.sch.cur;
        if !matches!(b, TaskYield | AtomicLoad | SyncSharedGet) {
            self.sch.ver += 1;
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
                let base = if self.sch.realtime.is_some() {
                    self.sch.clock = self.rt_clock();
                    self.rt_clock_up()
                } else {
                    self.sch.clock
                };
                let at = base.saturating_add(ms);
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
                self.sch.yielded = true;
                self.sch.tasks[cur].resume = Some(Val::Unit);
                self.sch.tasks[cur].state = State::Ready;
                self.sch.ready.push_back(cur);
                Op::Block
            }
            TaskSleep => {
                let ms = self.read_int(locals, &args[0]).max(0);
                let base = if self.sch.realtime.is_some() {
                    self.sch.clock = self.rt_clock();
                    self.rt_clock_up()
                } else {
                    self.sch.clock
                };
                let at = base.saturating_add(ms);
                self.sch.seq += 1;
                let sq = self.sch.seq;
                self.sch.sleepers.push((at, sq, Wake::Sleep(cur)));
                self.sch.tasks[cur].resume = Some(Val::Unit);
                self.sch.tasks[cur].state = State::Sleeping;
                Op::Block
            }
            IoWaitFd => {
                let fd = self.read_int(locals, &args[0]);
                let events = self.read_int(locals, &args[1]);
                let token = cur.saturating_add(1);
                if token >= 2_147_483_646 || events <= 0 || (events & !(1 | 4)) != 0 {
                    return Op::Val(Val::Int(-22));
                }
                let Ok(fd_i32) = i32::try_from(fd) else {
                    return Op::Val(Val::Int(-9));
                };
                if fd_i32 < 0 || !is_valid_fd(fd_i32) {
                    return Op::Val(Val::Int(-9));
                }
                self.enter_realtime();
                self.sch
                    .io_waiters
                    .entry(fd_i32)
                    .or_default()
                    .push((cur, events as i16));
                self.sch.tasks[cur].resume = None;
                self.sch.tasks[cur].state = State::IoWait;
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
                if self.sch.chans[id].reclaimed {
                    drop(v);
                    return Op::Fail(self.reclaimed_stop(stop, "通信路"));
                }
                if let Some(w) = self.sch.chans[id].waiters.pop_front() {
                    self.wake(w, Some(v));
                } else {
                    self.sch.chans[id].q.push_back(v);
                }
                Op::Val(Val::Unit)
            }
            ChanRecv => {
                let id = self.read_int(locals, &args[0]) as usize;
                if self.sch.chans[id].reclaimed {
                    return Op::Fail(self.reclaimed_stop(stop, "通信路"));
                }
                if let Some(v) = self.sch.chans[id].q.pop_front() {
                    Op::Val(v)
                } else {
                    self.sch.chans[id].waiters.push_back(cur);
                    self.sch.tasks[cur].state = State::Recv;
                    Op::Block
                }
            }
            SyncSharedNew | AtomicNew => Op::Val(Val::Shared(Rc::new(Val::Cell(Rc::new(
                RefCell::new(self.owned(locals, &args[0])),
            ))))),
            SyncSharedGet => {
                let (s, p) = self.view(locals, &args[0]);
                Op::Val(with_val(&s, &p, |v| match shared_cell(v) {
                    Some(c) => shadow(&c.borrow()),
                    None => Val::Unit,
                }))
            }
            AtomicLoad => {
                // `Atomic.load` は中断点（ADR 0007 決定 3・決定 6 条件 11）。
                let (s, p) = self.view(locals, &args[0]);
                let val = with_val(&s, &p, |v| match shared_cell(v) {
                    Some(c) => shadow(&c.borrow()),
                    None => Val::Unit,
                });
                self.sch.tasks[cur].resume = Some(val);
                self.sch.tasks[cur].state = State::Ready;
                self.sch.ready.push_back(cur);
                Op::Block
            }
            AtomicStore => {
                let (s, p) = self.view(locals, &args[0]);
                let x = self.owned(locals, &args[1]);
                let cell = with_val(&s, &p, |v| shared_cell(v).cloned());
                let old = cell.map(|c| std::mem::replace(&mut *c.borrow_mut(), x));
                drop(old);
                Op::Val(Val::Unit)
            }
            LockNew => {
                let value = self.owned(locals, &args[0]);
                let root = self.sch.tasks[cur].root;
                let id = self.sch.locks.len();
                self.sch.locks.push(LockSt {
                    value: new_slot(value),
                    holder: None,
                    waiters: VecDeque::new(),
                    broken: false,
                    root,
                    reclaimed: false,
                });
                Op::Val(Val::Int(id as i64))
            }
            LockAcquire => {
                // 直前の破棄による解放を先に反映する。
                self.settle_locks();
                let id = self.read_int(locals, &args[0]) as usize;
                if self.sch.locks[id].reclaimed {
                    return Op::Fail(self.reclaimed_stop(stop, "Lock"));
                }
                let l = &mut self.sch.locks[id];
                if l.broken {
                    Op::Val(self.lock_broken())
                } else if l.holder.is_none() {
                    l.holder = Some(cur);
                    let slot = Rc::clone(&l.value);
                    Op::Val(self.ok(Val::Guard(Rc::new(GuardTok { lock: id, slot }))))
                } else {
                    l.waiters.push_back(cur);
                    self.sch.tasks[cur].state = State::LockWait;
                    Op::Block
                }
            }
            LockGuardGet | LockGuardGetMut => {
                // 中断しない（`task.share` のみ）。参照は `Lock` の中身の `Slot` を指す。
                // 借用の検査が、参照をガードより長く生かさないことを保証する。
                let (s, p) = self.view(locals, &args[0]);
                let slot = with_val(&s, &p, |v| match v {
                    Val::Guard(g) => Some(Rc::clone(&g.slot)),
                    _ => None,
                });
                match slot {
                    Some(slot) => Op::Val(Val::Ref(Ptr { slot, path: vec![] })),
                    None => Op::Fail(self.stop_at(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("LockGuard でない値への get / get_mut"),
                    )),
                }
            }
            RootsScope => {
                let limit = self.read_int(locals, &args[0]);
                if !(1..=4096).contains(&limit) {
                    return Op::Fail(self.stop_at(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("task.roots: limit は 1 以上 4096 以下"),
                    ));
                }
                let f = self.read(locals, &args[1]);
                let sid = self.sch.scopes.len();
                let gid = self.sch.groups.len();
                self.sch.scopes.push(ScopeSt {
                    kind: ScopeKind::Roots { group: gid },
                    ..ScopeSt::default()
                });
                self.sch.groups.push(GroupSt {
                    sid,
                    limit: limit as usize,
                    used: 0,
                    closed: false,
                    waiters: VecDeque::new(),
                    starts: vec![],
                });
                let gref = Val::Ref(Ptr {
                    slot: new_slot(Val::Int(gid as i64)),
                    path: vec![],
                });
                match self.resolve_callee(&f, vec![gref]) {
                    Some((g, a)) => Op::Push(g, a, Some(sid)),
                    None => Op::Val(Val::Unit),
                }
            }
            RootReserve => {
                self.settle_roots();
                let gid = self.read_int(locals, &args[0]) as usize;
                let g = &mut self.sch.groups[gid];
                if g.closed {
                    return Op::Fail(self.stop_at(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("RootGroup.reserve: task.roots の範囲が終わっている"),
                    ));
                }
                if g.used < g.limit {
                    g.used += 1;
                    Op::Val(root_slot(gid))
                } else {
                    g.waiters.push_back(cur);
                    self.sch.tasks[cur].state = State::RootWait;
                    Op::Block
                }
            }
            RootStart => {
                let slot = self.owned(locals, &args[0]);
                let conn = self.owned(locals, &args[1]);
                let g = self.read(locals, &args[2]);
                let on_abort = self.read(locals, &args[3]);
                let tok = match &slot {
                    Val::RootSlot(t) if !t.used.get() => Rc::clone(t),
                    _ => {
                        return Op::Fail(self.stop_at(
                            stop,
                            Some(StopKind::StdRequiresViolated),
                            Some("RootSlot.start: 使えないスロット"),
                        ))
                    }
                };
                let gid = tok.group;
                if self.sch.groups[gid].closed {
                    return Op::Fail(self.stop_at(
                        stop,
                        Some(StopKind::StdRequiresViolated),
                        Some("RootSlot.start: task.roots の範囲が終わっている"),
                    ));
                }
                // スロットを消費する（以後の破棄では上限を返さない。上限はルートの終わりで返す）。
                tok.used.set(true);
                drop(tok);
                drop(slot);
                let conn_slot = new_slot(conn);
                let cref = Val::Ref(Ptr {
                    slot: Rc::clone(&conn_slot),
                    path: vec![],
                });
                let Some((fid, a)) = self.resolve_callee(&g, vec![cref]) else {
                    drop(conn_slot.replace(Val::Empty));
                    drop(on_abort);
                    self.release_slot(gid);
                    return Op::Val(Val::Unit);
                };
                let si = self.sch.started.len();
                let tid = self.start_root_task(fid, a, stop, si);
                self.sch.started.push(StartedSt {
                    group: gid,
                    task: tid,
                    conn: conn_slot,
                    on_abort: Some(on_abort),
                    phase: Phase::Body,
                });
                self.sch.groups[gid].starts.push(si);
                let sid = self.sch.groups[gid].sid;
                self.sch.scopes[sid].live += 1;
                Op::Val(Val::Unit)
            }
            _ => Op::Fail(self.stop_at(
                stop,
                Some(StopKind::StdRequiresViolated),
                Some("未対応のタスク操作"),
            )),
        }
    }

    /// 新しいルートタスクを起動する（`RootSlot.start` の `g`、または異常停止の後の `on_abort`）。
    fn start_root_task(&mut self, fid: FuncId, args: Vec<Val>, stop: StopId, si: usize) -> usize {
        let frame = self.new_frame(fid, args, stop, None);
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
            spin: None,
            started: Some(si),
        });
        self.sch.ready.push_back(tid);
        tid
    }

    /// スロットを 1 つ返す。`reserve` で待っているタスクがあれば、そのまま渡して起こす。
    fn release_slot(&mut self, gid: usize) {
        let g = &mut self.sch.groups[gid];
        if !g.closed {
            if let Some(w) = g.waiters.pop_front() {
                self.wake(w, Some(root_slot(gid)));
                return;
            }
        }
        g.used = g.used.saturating_sub(1);
    }

    /// 起動せずに破棄された `RootSlot` の知らせを反映する。
    fn settle_roots(&mut self) {
        let events = ROOT_EVENTS.with(|e| std::mem::take(&mut *e.borrow_mut()));
        for gid in events {
            if gid < self.sch.groups.len() {
                self.release_slot(gid);
            }
        }
    }

    /// 起動したルートが終わった（正常終了、または `on_abort` の異常停止の後始末の済み）。接続と `on_abort` を
    /// 破棄し、スロットを返す。集まりの本体が待っていて、これが最後なら起こす。
    fn root_finished(&mut self, si: usize) {
        let r = self.sch.started[si].task;
        self.reclaim_root(r);
        let st = &mut self.sch.started[si];
        st.phase = Phase::Done;
        let conn = st.conn.replace(Val::Empty);
        let oa = st.on_abort.take();
        let gid = st.group;
        drop(conn);
        drop(oa);
        self.release_slot(gid);
        let sid = self.sch.groups[gid].sid;
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

    /// 起動したルート `root` が終わった後、そのルートで作った通信路の項と `Lock` の値を捨てる（要求ごとに
    /// 積み上がらないように。ADR 0031 決定 4-2）。待機者・保持者がいるもの（持ち手がルートの外へ出て使われて
    /// いる）は残す。回収したものは記録し、外へ出た持ち手で後から使うと停止にする（黙って値を失わない）。
    fn reclaim_root(&mut self, root: usize) {
        // 終わったタスクの `LockGuard` の破棄（解放）を先に反映する。
        self.settle_locks();
        let mut dead = vec![];
        for ch in self.sch.chans.iter_mut() {
            if ch.root == root && !ch.reclaimed && ch.waiters.is_empty() {
                ch.reclaimed = true;
                dead.extend(std::mem::take(&mut ch.q));
            }
        }
        for l in self.sch.locks.iter_mut() {
            if l.root == root && !l.reclaimed && l.holder.is_none() && l.waiters.is_empty() {
                l.reclaimed = true;
                dead.push(l.value.replace(Val::Unit));
            }
        }
        drop(dead);
    }

    /// 回収済みの通信路・`Lock` の使用の停止。
    fn reclaimed_stop(&self, stop: StopId, what: &str) -> E {
        self.stop_at(
            stop,
            Some(StopKind::StdRequiresViolated),
            Some(&format!(
                "回収済みの{what}を使った（作ったルートが終わっている。ADR 0031 決定 4-2）"
            )),
        )
    }

    /// 起動したルートが異常停止した（そのルートのタスクは `stop_root` で破棄済み）。`g` のルートなら、接続を
    /// 破棄せずに、同じスロットの新しいルートで `on_abort(conn)` を実行する。`on_abort` のルートなら、
    /// それ以上は続けない（接続は `on_abort` の枠と一緒に破棄済み。ADR 0031 決定 4-2 の案 2）。
    fn root_aborted(&mut self, si: usize, e: &E) {
        if !self.capture {
            eprintln!("{e}");
        }
        if self.sch.started[si].phase == Phase::Body {
            let conn = self.sch.started[si].conn.replace(Val::Empty);
            let oa = self.sch.started[si].on_abort.take();
            let call = oa.as_ref().and_then(|f| self.resolve_callee(f, vec![conn]));
            drop(oa);
            if let Some((fid, a)) = call {
                let tid = self.start_root_task(fid, a, 0, si);
                self.sch.started[si].task = tid;
                self.sch.started[si].phase = Phase::Abort;
                return;
            }
        }
        self.root_finished(si);
    }

    /// 集まりの取り消し（本体の取り消し・異常停止）。起動済みのルートを起動の逆順に取り消し、`on_abort` を
    /// 呼ばずに接続を破棄する（決定 4-2 の (d)）。
    fn cancel_roots(&mut self, gid: usize) {
        self.sch.groups[gid].closed = true;
        self.sch.groups[gid].waiters.clear();
        let starts = self.sch.groups[gid].starts.clone();
        for si in starts.into_iter().rev() {
            if self.sch.started[si].phase == Phase::Done {
                continue;
            }
            let r = self.sch.started[si].task;
            self.stop_root_inner(r);
            let st = &mut self.sch.started[si];
            st.phase = Phase::Done;
            let conn = st.conn.replace(Val::Empty);
            let oa = st.on_abort.take();
            drop(conn);
            drop(oa);
        }
        self.sch.groups[gid].used = 0;
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
                let slot = Rc::clone(&l.value);
                let g = self.ok(Val::Guard(Rc::new(GuardTok { lock: id, slot })));
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
            // 起動したルートは範囲の子でない（`Task.scope` を持たない）ので、ここへは来ない。
            ScopeKind::Roots { .. } => drop(v),
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

    /// `task.yield` で中断したタスク `t` の後で、全タスクが `task.yield` と読み出しだけを繰り返し、
    /// 共有の状態が変わらない状態か（ライブロック。Issue #191、修正は #193、時間待ちとの共存は #198）。
    ///
    /// タスクの枠の指紋（枠の積み重ね全体の局所の中身）と共有の状態の版が、連続する 2 回の `task.yield` で
    /// 同じなら、そのタスクは同じ状態に戻っている（決定的な実行なので、次も同じになる）。実行可能な
    /// タスクがすべてそうなら、時計が進まない限り誰も状態を変えられない。時間待ちのタスクがいなければ永久に続く
    /// （呼び出し側が停止にする）。いれば呼び出し側が時計を進める。
    /// 指紋を取れないタスク（`LockGuard` を持つなど）は対象にしない（検出の取りこぼしはあるが、誤検出はない）。
    fn yield_livelock(&mut self, t: usize) -> bool {
        let ver = self.sch.ver;
        let fp = fp_frames(&self.sch.tasks[t].frames);
        let task = &mut self.sch.tasks[t];
        task.spin = match (fp, task.spin) {
            (Some(f), Some((pf, pv, _))) if pf == f && pv == ver => Some((f, ver, true)),
            (Some(f), _) => Some((f, ver, false)),
            (None, _) => None,
        };
        self.sch
            .ready
            .iter()
            .all(|&r| matches!(self.sch.tasks[r].spin, Some((_, v, true)) if v == ver))
    }

    /// 仮想の時計を最も早い目覚めの時刻まで進め、その時刻のタスクを登録順に起こす。時間待ちのタスクがいること。
    fn advance_clock(&mut self) {
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
    }

    /// ルートタスク 1 つ分の後始末。そのルートに属するタスクを新しいものから順に取り消し、
    /// 作った通信路と `Lock` の値を捨てる（異常停止の破棄の範囲。ADR 0026 決定 4）。
    fn stop_root(&mut self, root: usize) {
        // 取り消しの間の破棄は異常停止の巻き戻しとして扱う。親や兄弟のタスクが持つ `LockGuard` の破棄でも
        // `Lock` が壊れる（ADR 0026 決定 4）。
        self.sch.ver += 1;
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
            ch.reclaimed = true;
        }
        let mut dead = vec![];
        let mut woken = vec![];
        for l in self.sch.locks.iter_mut().filter(|l| l.root == root) {
            // 持ち手を外へ渡した `Lock` を別のルートが待っていれば、`Err(LockBroken)` で起こす（ADR 0031 決定 4-3）。
            woken.extend(l.waiters.drain(..));
            l.broken = true;
            dead.push(l.value.replace(Val::Unit));
        }
        drop(dead);
        for w in woken {
            let e = self.lock_broken();
            self.wake(w, Some(e));
        }
    }

    /// 進行できるタスクがない（デッドロック）。結果の出ていないルートのうち一番新しいものを 1 つだけ止める。
    /// `task.roots` が起動したルートが生きていれば、その中の一番新しいものを先に止め、異常停止と同じ後始末
    /// （`on_abort`）をする（隔離。ADR 0031 決定 4-2）。親や兄弟のルートは止めない。
    fn stop_deadlocked(&mut self) {
        let e = self.stop_at(
            0,
            Some(StopKind::StdRequiresViolated),
            Some("デッドロック（進行できるタスクがない）"),
        );
        let started = (0..self.sch.started.len())
            .filter(|&si| self.sch.started[si].phase != Phase::Done)
            .max_by_key(|&si| self.sch.started[si].task);
        if let Some(si) = started {
            let root = self.sch.started[si].task;
            self.stop_root(root);
            self.root_aborted(si, &e);
            return;
        }
        if let Some(i) = (0..self.sch.roots.len())
            .rev()
            .find(|&i| self.sch.results[i].is_none())
        {
            let root = self.sch.roots[i];
            self.stop_root(root);
            self.sch.results[i] = Some(Err(e));
        }
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
                spin: None,
                started: None,
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
            self.settle_roots();
            if self.sch.realtime.is_some() {
                self.poll_realtime(false);
            }
            let Some(t) = self.sch.pop_ready() else {
                if self.sch.realtime.is_some() {
                    if self.sch.sleepers.is_empty() && self.sch.io_waiters.is_empty() {
                        self.stop_deadlocked();
                        continue;
                    }
                    self.poll_realtime(true);
                    continue;
                }
                if self.sch.sleepers.is_empty() {
                    // 進行できるタスクがない。結果の出ていないルートタスクのうち一番新しいものを 1 つだけ
                    // デッドロックとして止め、次の反復で進行できるかを判定し直す（止めたことで起きた
                    // ルートタスクを、続けて潰さない）。
                    self.stop_deadlocked();
                    continue;
                }
                self.advance_clock();
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
            self.sch.yielded = false;
            let mut run = self.drive(&mut frames, true);
            if matches!(run, Run::Suspend) {
                self.sch.tasks[t].frames = frames;
                if std::mem::take(&mut self.sch.yielded) && self.yield_livelock(t) {
                    if self.sch.sleepers.is_empty() && self.sch.io_waiters.is_empty() {
                        run = Run::Fail(self.stop_at(
                            0,
                            Some(StopKind::StdRequiresViolated),
                            Some("デッドロック（task.yield だけで進行しない）"),
                        ));
                    } else if self.sch.realtime.is_some() {
                        // 実時間モードでは時計を飛ばさず、次の期限または I/O 完了まで少し実時間を進めてから続ける。
                        let before_ver = self.sch.ver;
                        self.poll_realtime(false);
                        if self.sch.ver == before_ver {
                            let ms = self
                                .sch
                                .sleepers
                                .iter()
                                .map(|s| s.0)
                                .min()
                                .map_or(5, |at| (at - self.rt_clock()).clamp(1, 5))
                                as u64;
                            std::thread::sleep(Duration::from_millis(ms));
                            self.poll_realtime(false);
                        }
                    } else {
                        // 実行可能なタスクが全員空転している間は、時計を進めないと起きられない
                        // タスクがいる。時計を最も早い起床時刻まで進めて続ける（起床で共有の状態の版が進む）。
                        self.advance_clock();
                    }
                }
                frames = vec![];
            }
            match run {
                Run::Suspend => {}
                Run::Done(v) => {
                    self.sch.ver += 1;
                    if let Some(i) = self.sch.roots.iter().position(|&r| r == t) {
                        self.sch.tasks[t].state = State::Done;
                        self.sch.results[i] = Some(Ok(v));
                    } else if let Some(si) = self.sch.tasks[t].started {
                        self.sch.tasks[t].state = State::Done;
                        drop(v);
                        self.root_finished(si);
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
                    } else if let Some(si) = self.sch.tasks[root].started {
                        // 起動したルートの異常停止は、そのルートだけを止める（ADR 0031 決定 4-2）。
                        self.root_aborted(si, &e);
                        drop(e);
                    }
                }
            }
        }
    }
}

/// 未使用の `RootSlot`（起動の権利）。
fn root_slot(gid: usize) -> Val {
    Val::RootSlot(Rc::new(RootSlotTok {
        group: gid,
        used: std::cell::Cell::new(false),
    }))
}

/// `Atomic` / `SyncShared` の中身の入れ物。`Val::Shared` を外して見る（`SharedGet` と同じ形）。
fn shared_cell(v: &Val) -> Option<&Rc<RefCell<Val>>> {
    match v {
        Val::Shared(rc) => match &**rc {
            Val::Cell(c) => Some(c),
            _ => None,
        },
        Val::Cell(c) => Some(c),
        _ => None,
    }
}

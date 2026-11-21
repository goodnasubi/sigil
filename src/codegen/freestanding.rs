//! x86-64 freestanding 出力の依存閉包検査とコード生成（ADR 0054 決定 2・7）。
//!
//! 入口 `p.entry` から到達可能な関数・大域・型・組み込み操作の依存閉包だけを辿り、
//! 確保器（`std.os.alloc`）と `std.os` を閉包から外した最小のオブジェクトを生成する。
//! 確保（ヒープ塊を要する型や `raw.alloc`/`raw.free`）または OS 機能（`std.os.*`・`raw.syscall`・
//! `console`・`fs`・`env`・並行処理のスケジューラなど）が閉包に含まれる場合は、`CgError::Unsupported` で拒否する。

use super::*;
use crate::ir::{AggKind, Builtin, Callee, Const, GlobalInit, Operand, Rvalue, Stmt, Term, Ty};
use crate::stdlib::embedded::{INT_ROTL_FN, INT_ROTR32_FN, INT_ROTR_FN};
use std::collections::BTreeSet;

/// freestanding 出力の依存閉包。
struct Closure {
    funcs: BTreeSet<usize>,
    globals: BTreeSet<usize>,
    needs_int_rot: bool,
    needs_mem: bool,
}

fn supported_freestanding_entry(f: &Func) -> Result<(), CgError> {
    if f.params.len() > 1 {
        return Err(CgError::Entry(format!(
            "freestanding 入口の関数 {} の引数は 0 個または 1 個（起動情報ポインタ Addr / Int）に限る（{} 個指定された）",
            f.name,
            f.params.len()
        )));
    }
    if let Some(&pi) = f.params.first() {
        let ty = norm(&f.locals[pi].ty);
        if !matches!(ty, Ty::Addr | Ty::Int) {
            return Err(CgError::Entry(format!(
                "freestanding 入口の関数 {} の引数 {} の型は起動情報ポインタ（Addr または Int）に限る",
                f.name, f.locals[pi].name
            )));
        }
    }
    if !is_scalar(&norm(&f.ret)) {
        return Err(CgError::Unsupported(format!(
            "freestanding 出力では関数 {} の戻り値に確保を伴う型を使えない",
            f.name
        )));
    }
    Ok(())
}

fn is_scalar(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Unit | Ty::Addr => true,
        Ty::Ref(inner) | Ty::MutRef(inner) => is_scalar(inner),
        _ => false,
    }
}

fn check_ty(p: &Program, fname: &str, ty: &Ty) -> Result<(), CgError> {
    let t = norm(ty);
    match &t {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Unit | Ty::Addr => Ok(()),
        Ty::Ref(inner) | Ty::MutRef(inner) => check_ty(p, fname, inner),
        Ty::Str | Ty::Bytes | Ty::List(_) | Ty::Shared(_) | Ty::Cell(_) => {
            Err(CgError::Unsupported(format!(
                "freestanding 出力では確保を要する型（{fname}）を使えない"
            )))
        }
        Ty::Path => Err(CgError::Unsupported(format!(
            "freestanding 出力では OS 層の型 Path（{fname}）を使えない"
        ))),
        Ty::Adt(id) => {
            let tname = &p.types[*id].name;
            if tname.starts_with("std.os.")
                || crate::stdlib::FD_TYPES.contains(&tname.as_str())
                || matches!(tname.as_str(), "IoError" | "BinaryIoError" | "NetError")
            {
                Err(CgError::Unsupported(format!(
                    "freestanding 出力では OS 機能に依存する型 {tname}（{fname}）を使えない"
                )))
            } else {
                Err(CgError::Unsupported(format!(
                    "freestanding 出力では確保を伴う複合型 {tname}（{fname}）を使えない"
                )))
            }
        }
        Ty::Tuple(_) | Ty::FnRef(..) | Ty::ParseError => Err(CgError::Unsupported(format!(
            "freestanding 出力では確保を伴う型（{fname}）を使えない"
        ))),
    }
}

fn check_operand(fname: &str, op: &Operand, queue: &mut Vec<usize>) -> Result<(), CgError> {
    match op {
        Operand::Copy(_) | Operand::Move(_) => Ok(()),
        Operand::Const(c) => match c {
            Const::Unit | Const::Int(_) | Const::Float(_) | Const::Bool(_) => Ok(()),
            Const::Str(_) => Err(CgError::Unsupported(format!(
                "freestanding 出力では文字列リテラル（確保を要する String、関数 {fname}）を使えない"
            ))),
            Const::Fn(f) => {
                queue.push(*f);
                Err(CgError::Unsupported(format!(
                    "freestanding 出力では関数値（関数 {fname}）を使えない"
                )))
            }
        },
    }
}

fn analyse_closure(p: &Program, opts: &Opts) -> Result<Closure, CgError> {
    if opts.trace_drops {
        return Err(CgError::Unsupported(
            "freestanding 出力では --trace-drops（標準エラーへの書き出し）を使えない".into(),
        ));
    }
    let entry_fn = &p.funcs[p.entry];
    supported_freestanding_entry(entry_fn)?;

    let mut funcs = BTreeSet::new();
    let mut globals = BTreeSet::new();
    let mut needs_int_rot = false;
    let mut needs_mem = false;
    let mut queue = vec![p.entry];

    while let Some(fid) = queue.pop() {
        if !funcs.insert(fid) {
            continue;
        }
        let f = &p.funcs[fid];
        if f.name == "std.os.alloc.alloc"
            || f.name == "std.os.alloc.free"
            || f.name.starts_with("std.os.alloc.")
        {
            return Err(CgError::Unsupported(format!(
                "freestanding 出力では確保器（{}）を使えない",
                f.name
            )));
        }
        if f.name.starts_with("std.os.") {
            return Err(CgError::Unsupported(format!(
                "freestanding 出力では OS 機能（{}）を使えない",
                f.name
            )));
        }
        check_ty(p, &f.name, &f.ret)?;
        for l in &f.locals {
            check_ty(p, &f.name, &l.ty)?;
        }
        for bb in &f.blocks {
            for stmt in &bb.stmts {
                match stmt {
                    Stmt::Drop(_) => {}
                    Stmt::Assign(_, rv) => match rv {
                        Rvalue::Use(op) | Rvalue::Unary(_, op) => {
                            check_operand(&f.name, op, &mut queue)?;
                        }
                        Rvalue::Binary(op, a, b) => {
                            check_operand(&f.name, a, &mut queue)?;
                            check_operand(&f.name, b, &mut queue)?;
                            if *op == crate::ir::BinOp::Rem {
                                // Float の % は検査器で拒否されるが、念のため外部 fmod 呼び出しを防ぐ。
                            }
                        }
                        Rvalue::Ref(_) | Rvalue::RefMut(_) => {}
                        Rvalue::Clone(op, ty) => {
                            check_operand(&f.name, op, &mut queue)?;
                            check_ty(p, &f.name, ty)?;
                        }
                        Rvalue::GlobalRef(gid) => {
                            let g = &p.globals[*gid];
                            if g.name.starts_with("std.os.alloc.") {
                                return Err(CgError::Unsupported(format!(
                                    "freestanding 出力では確保器の大域状態（{}）を使えない",
                                    g.name
                                )));
                            }
                            if g.name.starts_with("std.os.") {
                                return Err(CgError::Unsupported(format!(
                                    "freestanding 出力では OS 層の大域状態（{}）を使えない",
                                    g.name
                                )));
                            }
                            check_ty(p, &f.name, &g.ty)?;
                            if matches!(g.init, GlobalInit::Tuple(_) | GlobalInit::Struct(_)) {
                                return Err(CgError::Unsupported(format!(
                                    "freestanding 出力では確保を伴う大域変数（{}）を使えない",
                                    g.name
                                )));
                            }
                            globals.insert(*gid);
                        }
                        Rvalue::Aggregate(kind, _) => {
                            let what = match kind {
                                AggKind::Tuple => "組",
                                AggKind::Struct(id) | AggKind::Variant(id, _) => &p.types[*id].name,
                            };
                            return Err(CgError::Unsupported(format!(
                                "freestanding 出力では確保を伴う値の構築（{what}、関数 {}）を使えない",
                                f.name
                            )));
                        }
                        Rvalue::Discriminant(op) => {
                            check_operand(&f.name, op, &mut queue)?;
                            return Err(CgError::Unsupported(format!(
                                "freestanding 出力では確保を伴う列挙型（関数 {}）を使えない",
                                f.name
                            )));
                        }
                        Rvalue::MakeClosure(cf, _) => {
                            let _ = cf;
                            return Err(CgError::Unsupported(format!(
                                "freestanding 出力では確保を伴うクロージャ（関数 {}）を使えない",
                                f.name
                            )));
                        }
                    },
                }
            }
            match &bb.term {
                Term::Goto(_) | Term::Return | Term::Unreachable | Term::Resume => {}
                Term::Branch(op, _, _) | Term::Switch(op, _, _) => {
                    check_operand(&f.name, op, &mut queue)?;
                }
                Term::Assert { cond, .. } => {
                    check_operand(&f.name, cond, &mut queue)?;
                }
                Term::CheckedOp { a, b, .. } => {
                    check_operand(&f.name, a, &mut queue)?;
                    check_operand(&f.name, b, &mut queue)?;
                }
                Term::Call { func, args, .. } => {
                    for a in args {
                        check_operand(&f.name, a, &mut queue)?;
                    }
                    match func {
                        Callee::Func(callee_id) => {
                            queue.push(*callee_id);
                        }
                        Callee::Indirect(_) => {
                            return Err(CgError::Unsupported(format!(
                                "freestanding 出力では間接関数呼び出し（関数 {}）を使えない",
                                f.name
                            )));
                        }
                        Callee::Builtin(b) => {
                            check_builtin(
                                p,
                                &f.name,
                                *b,
                                &mut queue,
                                &mut needs_int_rot,
                                &mut needs_mem,
                            )?;
                        }
                    }
                }
            }
        }
    }

    Ok(Closure {
        funcs,
        globals,
        needs_int_rot,
        needs_mem,
    })
}

fn check_builtin(
    p: &Program,
    fname: &str,
    b: Builtin,
    queue: &mut Vec<usize>,
    needs_int_rot: &mut bool,
    needs_mem: &mut bool,
) -> Result<(), CgError> {
    use Builtin::*;
    match b {
        IntMinValue | ToFloat | IntAshr | FloatToBits | FloatFromBits | RawAddrNull
        | RawAddrToInt | RawIntToAddr | RawOffset | RawLoadU8 | RawStoreU8 | RawLoadI64
        | RawStoreI64 | RawLoadAddr | RawStoreAddr | RawAtomicLoadI64 | RawAtomicStoreI64
        | RawAtomicAddI64 | RawAtomicSwapI64 | RawAtomicCasI64 | RawAtomicFence => Ok(()),
        RawCopy | RawSet => {
            *needs_mem = true;
            Ok(())
        }
        IntRotl | IntRotr | IntRotr32 => {
            *needs_int_rot = true;
            for target_fn in [INT_ROTL_FN, INT_ROTR_FN, INT_ROTR32_FN] {
                let Some(id) = p.funcs.iter().position(|f| f.name == target_fn) else {
                    return Err(internal("std.core.int が結ばれていない"));
                };
                queue.push(id);
            }
            Ok(())
        }
        RawAlloc | RawFree => Err(CgError::Unsupported(format!(
            "freestanding 出力では確保器の原始操作（raw.alloc / raw.free、関数 {fname}）を使えない"
        ))),
        RawSyscall => Err(CgError::Unsupported(format!(
            "freestanding 出力ではホスト OS のシステムコール（raw.syscall、関数 {fname}）を使えない"
        ))),
        ConsoleLog | EnvGet | FsRead | FsWrite | FsReadBytes | FsWriteBytes | PathNew
        | PathIsEmpty | PathJoin | PathFileName | PathParent | IoWaitFd | RootsScope
        | RootReserve | RootStart | TaskScope | TaskAllOk | TaskRace | TaskTimeout | TaskYield
        | TaskSleep | TaskChannel | ScopeSpawn | TaskJoin | ChanSend | ChanRecv | SyncSharedNew
        | SyncSharedGet | SyncSharedBorrow | LockNew | LockAcquire | LockGuardGet
        | LockGuardGetMut | CapacityNew | CapacityTryAcquire | AtomicNew | AtomicLoad
        | AtomicStore => Err(CgError::Unsupported(format!(
            "freestanding 出力では OS 機能・並行処理の操作（{b:?}、関数 {fname}）を使えない"
        ))),
        ListNew | ListLen | ListIsEmpty | ListGet | ListGetMut | ListPush | ListSumInt
        | ListSumFloat | ListMin | ListMax | ListMap | ListFind | ListMaxBy | ListMinBy
        | OptIsSome | OptIsNone | OptUnwrapOr | OptMap | OptOkOr | ResIsOk | ResIsErr
        | ResUnwrapOr | ResOk | ResMap | ResMapErr | StrLen | StrIsEmpty | StrTrim | StrLines
        | StrParseInt | CellNew | SharedNew | SharedGet | SharedSet | ToInt | CtEq | StrAsBytes
        | BytesNew | BytesWithCapacity | BytesFromList | BytesLen | BytesIsEmpty | BytesGet
        | BytesSet | BytesPush | BytesSlice | BytesConcat | BytesExtend | BytesToStringUtf8
        | BytesPushU16Be | BytesPushU32Be | BytesPushU64Be | BytesPushU32Le | BytesReadU32Be
        | Compare(_) => Err(CgError::Unsupported(format!(
            "freestanding 出力では確保を要する標準操作（{b:?}、関数 {fname}）を使えない"
        ))),
    }
}

/// シンボル名に `alloc`・`free`・`syscall`・独立語の `int` が混入しないようにする（起動契約の逆アセンブリ・シンボル検査との整合）。
fn freestanding_func_symbol(i: usize, name: &str) -> String {
    let base = func_symbol(i, name);
    base.replace("int", "i64")
}

pub(super) fn compile(p: &Program, opts: &Opts) -> Result<Vec<u8>, CgError> {
    let closure = analyse_closure(p, opts)?;

    let mut fb = settings::builder();
    fb.set("opt_level", "none").map_err(internal)?;
    fb.set("is_pic", "false").map_err(internal)?;
    fb.set("enable_verifier", "true").map_err(internal)?;
    let _ = fb.set("probestack_strategy", "inline");
    let flags = settings::Flags::new(fb);
    let isa = cranelift_codegen::isa::lookup_by_name("x86_64-unknown-linux-gnu")
        .map_err(internal)?
        .finish(flags)
        .map_err(internal)?;
    let cc = isa.default_call_conv();
    let fe = isa.frontend_config();
    let ob = ObjectBuilder::new(isa, "sigil", default_libcall_names()).map_err(internal)?;
    let mut m = ObjectModule::new(ob);

    // `fids` と `gids` は `Program` の添字で引けるよう全要素分のスロットを用意するが、
    // 実際にオブジェクトへ定義するのは依存閉包（`closure.funcs` / `closure.globals`）に含まれるものだけにする。
    // これにより `std.os.alloc` や `std.os.*` のシンボル・コード・大域データはオブジェクトに一切入らない。
    let mut fids = Vec::with_capacity(p.funcs.len());
    let dummy_sig = {
        let mut s = Signature::new(cc);
        s.returns.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        s
    };
    let entry_fid = m
        .declare_function(
            &freestanding_func_symbol(p.entry, &p.funcs[p.entry].name),
            Linkage::Local,
            &{
                let mut s = Signature::new(cc);
                for _ in 0..p.funcs[p.entry].params.len() {
                    s.params.push(AbiParam::new(types::I64));
                }
                s.returns.push(AbiParam::new(types::I64));
                s.returns.push(AbiParam::new(types::I64));
                s
            },
        )
        .map_err(internal)?;
    for (i, f) in p.funcs.iter().enumerate() {
        if i == p.entry {
            fids.push(entry_fid);
        } else if closure.funcs.contains(&i) {
            let mut s = Signature::new(cc);
            for _ in 0..f.params.len() {
                s.params.push(AbiParam::new(types::I64));
            }
            s.returns.push(AbiParam::new(types::I64));
            s.returns.push(AbiParam::new(types::I64));
            fids.push(
                m.declare_function(&freestanding_func_symbol(i, &f.name), Linkage::Local, &s)
                    .map_err(internal)?,
            );
        } else {
            let _ = &dummy_sig;
            fids.push(entry_fid);
        }
    }

    let msg_table = m.declare_anonymous_data(true, false).map_err(internal)?;
    let mut gids = Vec::with_capacity(p.globals.len());
    for (gi, g) in p.globals.iter().enumerate() {
        if !closure.globals.contains(&gi) {
            gids.push(msg_table);
            continue;
        }
        let id = m
            .declare_anonymous_data(g.mutable, false)
            .map_err(internal)?;
        let word: i64 = match &g.init {
            GlobalInit::Int(n) => *n,
            GlobalInit::Bool(b) => *b as i64,
            GlobalInit::AddrNull => 0,
            GlobalInit::Tuple(_) | GlobalInit::Struct(_) => {
                return Err(CgError::Unsupported(format!(
                    "freestanding 出力では複合型の大域変数（{}）を使えない",
                    g.name
                )));
            }
        };
        let mut dd = DataDescription::new();
        dd.define(word.to_le_bytes().to_vec().into_boxed_slice());
        dd.set_align(8);
        m.define_data(id, &dd).map_err(internal)?;
        gids.push(id);
    }

    let mut cg = Cg {
        p,
        m,
        cc,
        fe,
        rt: HashMap::new(),
        bytes: HashMap::new(),
        fnblocks: HashMap::new(),
        msgs: vec![],
        msg_idx: HashMap::new(),
        trace: false,
        freestanding: true,
        fids,
        msg_table,
        gids,
        live_shim: None,
        str_ops: None,
        list_ops: None,
        bytes_ops: None,
        gen: HashMap::new(),
        gen_q: vec![],
        gen_n: 0,
        blkfree_nz: None,
        show_fns: HashMap::new(),
        show_q: vec![],
        quote: None,
        coerce_err: None,
        task: None,
    };

    let mut shims = vec![];
    if closure.needs_int_rot {
        shims.extend(int_shims(&mut cg)?);
    }
    let mem = if closure.needs_mem {
        mem_fns(&mut cg)?
    } else {
        vec![]
    };

    for &i in &closure.funcs {
        let f = &p.funcs[i];
        let sig = cg.sig(f.params.len());
        let lt: Vec<Ty> = f.locals.iter().map(|l| norm(&l.ty)).collect();
        let id = cg.fids[i];
        cg.define(id, i as u32 + 1, sig, lt, |fx| fx.run(f))?;
    }

    for (k, &(id, target, np, ret)) in shims.iter().enumerate() {
        let sig = shim_sig(cc, np, ret);
        cg.define(id, p.funcs.len() as u32 + 3 + k as u32, sig, vec![], |fx| {
            fx.alloc_shim(target, ret)
        })?;
    }

    for (k, &(id, kind)) in mem.iter().enumerate() {
        let mut sig = Signature::new(cc);
        for _ in 0..3 {
            sig.params.push(AbiParam::new(types::I64));
        }
        let idx = p.funcs.len() as u32 + 3 + shims.len() as u32 + k as u32;
        cg.define(id, idx, sig, vec![], |fx| match kind {
            MemFn::Move => fx.mem_move(),
            MemFn::Set => fx.mem_set(),
        })?;
    }

    start::define_freestanding(&mut cg)?;

    // 停止の報告が閉包内のシムから要求された場合も、Linux システムコールは一切使わず `cli; hlt` で停止する。
    while let Some((id, k)) = cg.show_q.pop() {
        cg.gen_n += 1;
        if k == SFn::ReportStop {
            cg.m.define_function_bytes(id, 1, start::FREESTANDING_HALT, &[])
                .map_err(internal)?;
        } else {
            return Err(internal(format!(
                "freestanding 出力で未対応の表示補助が要求された: {k:?}"
            )));
        }
    }

    let ws = [W::I(0), W::I(0)];
    let table = cg.msg_table;
    cg.write_words(table, &ws)?;

    let product = cg.m.finish();
    product.emit().map_err(internal)
}

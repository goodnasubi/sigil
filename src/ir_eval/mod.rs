//! SIR の評価器（ADR 0015 の決定 2・5）。`lower` が作った `Program` を、値・出力・破棄の順・停止を観測として返して実行する。
//!
//! 値の持ち方:
//! - 局所はすべて `Slot`（`Rc<RefCell<Val>>`）。組・構造体・列挙型の腕・一覧は `Agg`（フィールドの並び）。
//! - 参照は `Ptr`（スロットと、その中の位置の列）。借用の間は元の値が動かないことを検査器が保証する。
//! - 移動は「読んで元を `Empty` にする」。`Drop` は `Empty` に対して何もしない。破棄は型でなく値の形から行う。
//! - 構造体の破棄は、自分、次にフィールドを宣言の順（`Agg` の `Drop` と `drain`）。移動した部分を持つ構造体は、自分を記録しない（Rust と同じ）。
//! - 複写（`Copy`）は破棄を記録しない写し（`silent`）。`clone` だけが破棄される値を増やす。

mod call;
mod expr;
mod sched;
mod value;

use crate::ast::{File, Item, Type};
use crate::exec::{Arg, Failure, Options, Outcome, RunError, Stop, StopKind};
use crate::ir::*;
use crate::shared_args::arg_name;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use value::{
    list_of, locate, new_slot, show_val, stack_addr, types, with_val_mut, Ptr, Val, DEEP, DROP_LOG,
    STACK_LIMIT, TYPES,
};

type Slot = Rc<RefCell<Val>>;
type E = Box<Stop>;

// ---------------------------------------------------------------------------
// 評価器

struct M<'p> {
    p: &'p Program,
    strs: Vec<Rc<str>>,
    out: String,
    capture: bool,
    fs: Option<BTreeMap<String, String>>,
    canon: Vec<usize>,
    opt: usize,
    res: usize,
    io: usize,
    /// 確保済みチャンクの境界情報 (番地 -> (バイトサイズ, アライメント))
    chunks: BTreeMap<usize, (usize, usize)>,
    sch: sched::Sched,
    /// 次に積む枠が範囲の本体なら、その範囲の番号。
    pending_scope: Option<usize>,
}

impl<'p> M<'p> {
    fn stop_at(&self, id: StopId, kind: Option<StopKind>, detail: Option<&str>) -> E {
        let s = &self.p.stop_sites[id];
        Box::new(Stop {
            kind: kind.unwrap_or(s.kind),
            func: s.func.clone(),
            line: s.line,
            during: s.during.clone(),
            detail: detail.map_or_else(|| s.detail.clone(), str::to_string),
        })
    }

    fn after_deep(&self, loc: StopId) -> Result<(), E> {
        if DEEP.with(|d| d.replace(false)) {
            Err(self.stop_at(
                loc,
                Some(StopKind::StackExhausted),
                Some("値の入れ子が深すぎる"),
            ))
        } else {
            Ok(())
        }
    }

    fn exec(&mut self, locals: &[Slot], s: &Stmt) {
        match s {
            Stmt::Assign(pl, rv) => {
                let v = self.rvalue(locals, rv);
                self.write(locals, pl, v);
            }
            Stmt::Drop(pl) => {
                let (s, p) = locate(locals, pl);
                let old = with_val_mut(&s, &p, |v| match v {
                    Val::Ref(_) | Val::Fn(_) => Val::Empty,
                    _ => std::mem::replace(v, Val::Empty),
                });
                drop(old);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 入口

fn to_val(a: &Arg) -> Val {
    match a {
        Arg::Int(n) => Val::Int(*n),
        Arg::Float(x) => Val::Float(*x),
        Arg::Bool(b) => Val::Bool(*b),
        Arg::Str(s) => Val::Str(s.as_str().into()),
        Arg::List(xs) => list_of(xs.iter().map(to_val).collect()),
    }
}

/// 入口の引数を宣言の型に合わせて値にする（`runtime/core` の `coerce` と同じ規則）。
fn coerce(a: &Arg, ty: &Type) -> Result<Val, String> {
    let ty = match ty {
        Type::Ref { inner, .. } => inner,
        t => t,
    };
    let Type::Path { path, args } = ty else {
        return Ok(to_val(a));
    };
    let name = path.last().map(String::as_str).unwrap_or("");
    let mismatch = || Err(format!("{name} が要るが、{}が渡された", arg_name(a)));
    match (name, a) {
        ("Int", Arg::Int(n)) => Ok(Val::Int(*n)),
        ("Float", Arg::Float(x)) => Ok(Val::Float(*x)),
        ("Float", Arg::Int(n)) => Ok(Val::Float(*n as f64)),
        ("Bool", Arg::Bool(b)) => Ok(Val::Bool(*b)),
        ("String", Arg::Str(x)) => Ok(Val::Str(x.as_str().into())),
        ("Path", Arg::Str(x)) => Ok(Val::Path(x.as_str().into())),
        ("List", Arg::List(xs)) => {
            let elem = args.first();
            let mut items = Vec::with_capacity(xs.len());
            for x in xs {
                items.push(match elem {
                    Some(t) => coerce(x, t)?,
                    None => to_val(x),
                });
            }
            Ok(list_of(items))
        }
        ("Int" | "Float" | "Bool" | "String" | "Path" | "List", _) => mismatch(),
        _ => Ok(to_val(a)),
    }
}

/// ソースの並び（先頭が入口のモジュール。残りは `use` で辿られるモジュール）を結んで、
/// 入口の関数から SIR にする（ネイティブ出力の入口）。
pub fn lower_sources(srcs: &[&str], func: &str) -> Result<Program, RunError> {
    lower_sources_with(srcs, func, &[])
}

/// `lower_sources` に `--trust-base` のモジュール名を渡す版。
pub fn lower_sources_with(
    srcs: &[&str],
    func: &str,
    trust_base: &[String],
) -> Result<Program, RunError> {
    let opts = crate::effects::Options {
        trust_base: trust_base.to_vec(),
        ..Default::default()
    };
    let file =
        crate::project::link_sources(srcs, &opts).map_err(|e| crate::exec::link_error(e, true))?;
    lower_file(&file, func, &opts)
}

/// ファイルの `use` を `root` から辿って結び、入口の関数から SIR にする（`sigil build`）。
pub fn lower_project(
    root: &std::path::Path,
    entry: &std::path::Path,
    func: &str,
    trust_base: &[String],
) -> Result<Program, RunError> {
    let opts = crate::effects::Options {
        trust_base: trust_base.to_vec(),
        ..Default::default()
    };
    let file = crate::project::link_project(root, entry, &opts)
        .map_err(|e| crate::exec::link_error(e, false))?;
    lower_file(&file, func, &opts)
}

fn lower_file(
    file: &File,
    func: &str,
    opts: &crate::effects::Options,
) -> Result<Program, RunError> {
    let typed = crate::check::typed_with(file, opts).map_err(RunError::Check)?;
    let found = file
        .items
        .iter()
        .any(|i| matches!(i, Item::Fn(d) if d.name == func));
    if !found {
        return Err(RunError::Entry(format!("関数 {func} がない")));
    }
    crate::lower::lower(&typed, func).map_err(|e| RunError::Unsupported(e.0))
}

/// 実行の入力。ソースの並び、またはファイル（`root` と入口）。
enum Input<'a> {
    Sources(&'a [&'a str]),
    Project(&'a std::path::Path, &'a std::path::Path),
}

pub fn run(src: &str, func: &str, args: &[Arg]) -> Result<Outcome, Failure> {
    run_sources_with(&[src], func, args, &Options::captured())
}

pub fn run_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    run_sources_with(&[src], func, args, opts)
}

pub fn run_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Result<Outcome, Failure> {
    run_sources_with(srcs, func, args, &Options::captured())
}

pub fn run_sources_with(
    srcs: &[&str],
    func: &str,
    args: &[Arg],
    opts: &Options,
) -> Result<Outcome, Failure> {
    run_input(Input::Sources(srcs), func, args, opts)
}

/// 入口のファイルから `use` を `root` で辿って結び、関数を実行する（`sigil run`）。
pub fn run_project_with(
    root: &std::path::Path,
    entry: &std::path::Path,
    func: &str,
    args: &[Arg],
    opts: &Options,
) -> Result<Outcome, Failure> {
    run_input(Input::Project(root, entry), func, args, opts)
}

/// 複数のルートタスクの実行の結果（ADR 0026 決定 4）。
pub struct RootsOutcome {
    /// ルートタスクごとの結果（引数の組の順）。異常停止したルートタスクは `Err`（他のルートタスクは止まらない）。
    pub roots: Vec<Result<String, Box<Stop>>>,
    pub output: String,
    pub drops: Vec<String>,
}

/// 同じ入口の関数を、引数の組ごとのルートタスクとして 1 つの評価器で実行する（要求ごとのタスクの範囲の隔離）。
///
/// 1 つのルートタスクの異常停止は、そのルートタスクと子のタスクだけを止める。スタックの枯渇と確保器の枠の中の
/// 異常停止は、プロセス全体の停止として `Err(Failure)` で返す。入口の引数の型が `Lock<T>` / `&Lock<T>` なら、
/// 全ルートタスクで共有する 1 つの `Lock` を渡す（初期値は先頭のルートタスクの引数）。
pub fn run_roots_with(
    srcs: &[&str],
    func: &str,
    arg_sets: &[Vec<Arg>],
    opts: &Options,
) -> Result<RootsOutcome, Failure> {
    let input = Input::Sources(srcs);
    in_big_thread(opts, || execute_roots(&input, func, arg_sets, opts))
}

fn in_big_thread<T: Send>(
    opts: &Options,
    f: impl FnOnce() -> Result<T, Failure> + Send,
) -> Result<T, Failure> {
    std::thread::scope(|s| {
        let spawned = std::thread::Builder::new()
            .stack_size(opts.stack_bytes)
            .spawn_scoped(s, f);
        match spawned {
            Ok(h) => h.join().unwrap_or_else(|_| {
                Err(crate::exec::fail(RunError::Entry(
                    "評価器の内部エラー".into(),
                )))
            }),
            Err(e) => Err(crate::exec::fail(RunError::Entry(format!(
                "スレッドを作れない: {e}"
            )))),
        }
    })
}

fn run_input(input: Input, func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    in_big_thread(opts, || execute(&input, func, args, opts))
}

/// 入口が 1 つのルートタスクの実行。その異常停止は従来どおりプログラム全体の失敗になる。
fn execute(input: &Input, func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    let mut o = execute_roots(input, func, std::slice::from_ref(&args.to_vec()), opts)?;
    match o.roots.remove(0) {
        Ok(value) => Ok(Outcome {
            value,
            output: o.output,
            drops: o.drops,
        }),
        Err(s) => Err(Failure {
            error: RunError::Stop(s),
            output: o.output,
            drops: o.drops.into_boxed_slice(),
        }),
    }
}

fn execute_roots(
    input: &Input,
    func: &str,
    arg_sets: &[Vec<Arg>],
    opts: &Options,
) -> Result<RootsOutcome, Failure> {
    use crate::exec::fail;
    let copts = crate::exec::check_options(opts);
    let file: File = match input {
        Input::Sources(srcs) => crate::project::link_sources(srcs, &copts)
            .map_err(|e| fail(crate::exec::link_error(e, true)))?,
        Input::Project(root, entry) => crate::project::link_project(root, entry, &copts)
            .map_err(|e| fail(crate::exec::link_error(e, false)))?,
    };
    let typed = crate::check::typed_with(&file, &copts).map_err(|e| fail(RunError::Check(e)))?;
    let decl = file.items.iter().find_map(|i| match i {
        Item::Fn(d) if d.name == func => Some(d),
        _ => None,
    });
    let Some(decl) = decl else {
        return Err(fail(RunError::Entry(format!("関数 {func} がない"))));
    };
    let is_lock = |t: &Type| {
        let t = match t {
            Type::Ref { inner, .. } => inner,
            t => t,
        };
        matches!(t, Type::Path { path, .. } if path.last().is_some_and(|n| n == "Lock"))
    };
    // 共有の `Lock` の引数は、評価器ができてから作る（ここでは場所だけ空けておく）。
    let mut argvs: Vec<Vec<Val>> = Vec::with_capacity(arg_sets.len());
    for args in arg_sets {
        if decl.params.len() != args.len() {
            return Err(fail(RunError::Entry(format!(
                "{func} は引数を {} 個とる（{} 個渡された）",
                decl.params.len(),
                args.len()
            ))));
        }
        let mut argv = Vec::with_capacity(args.len());
        for (p, a) in decl.params.iter().zip(args) {
            if is_lock(&p.ty) {
                argv.push(Val::Empty);
                continue;
            }
            match coerce(a, &p.ty) {
                Ok(v) => argv.push(if matches!(p.ty, Type::Ref { .. }) {
                    Val::Ref(Ptr {
                        slot: new_slot(v),
                        path: vec![],
                    })
                } else {
                    v
                }),
                Err(m) => {
                    return Err(fail(RunError::Entry(format!(
                        "引数 {} の型が合わない: {m}",
                        p.name
                    ))))
                }
            }
        }
        argvs.push(argv);
    }
    let program = match crate::lower::lower(&typed, func) {
        Ok(p) => p,
        Err(e) => return Err(fail(RunError::Unsupported(e.0))),
    };
    drop(typed);

    let (base, budget) = (stack_addr(), opts.stack_bytes / 4 * 3);
    STACK_LIMIT.with(|c| c.set((base, budget)));
    DEEP.with(|d| d.set(false));
    DROP_LOG.with(|l| *l.borrow_mut() = opts.trace_drops.then(Vec::new));
    TYPES.with(|t| *t.borrow_mut() = Rc::new(program.types.clone()));

    let mut canon = Vec::with_capacity(program.types.len());
    for (i, t) in program.types.iter().enumerate() {
        canon.push(
            program
                .types
                .iter()
                .position(|u| u.name == t.name)
                .unwrap_or(i),
        );
    }
    let find = |n: &str| program.types.iter().position(|t| t.name == n).unwrap_or(0);
    let mut m = M {
        p: &program,
        strs: program.strings.iter().map(|s| s.as_str().into()).collect(),
        out: String::new(),
        capture: opts.capture_console,
        fs: opts.fs_files.clone(),
        opt: find("Option"),
        res: find("Result"),
        io: find("IoError"),
        canon,
        chunks: BTreeMap::new(),
        sch: sched::Sched::new(opts.stack_bytes, opts.schedule_seed),
        pending_scope: None,
    };
    // 共有の `Lock` を作って、`Lock` の引数に入れる。
    for (pi, p) in decl.params.iter().enumerate() {
        if !is_lock(&p.ty) {
            continue;
        }
        let inner = match &p.ty {
            Type::Ref { inner, .. } => inner,
            t => t,
        };
        let init = match (arg_sets.first(), inner) {
            (Some(args), Type::Path { args: targs, .. }) => match targs.first() {
                Some(t) => coerce(&args[pi], t),
                None => Ok(to_val(&args[pi])),
            },
            _ => Ok(Val::Unit),
        };
        let init = match init {
            Ok(v) => v,
            Err(msg) => {
                return Err(fail(RunError::Entry(format!(
                    "引数 {} の型が合わない: {msg}",
                    p.name
                ))))
            }
        };
        let id = m.sch.new_shared_lock(init);
        for argv in &mut argvs {
            argv[pi] = if matches!(p.ty, Type::Ref { .. }) {
                Val::Ref(Ptr {
                    slot: new_slot(Val::Int(id as i64)),
                    path: vec![],
                })
            } else {
                Val::Int(id as i64)
            };
        }
    }
    let result = m.run_roots(program.entry, argvs);
    let output = std::mem::take(&mut m.out);
    let res = match result {
        Ok(rs) => {
            let mut roots = Vec::with_capacity(rs.len());
            for r in rs {
                roots.push(match r {
                    Ok(v) => {
                        let ts = types();
                        let value = show_val(&ts, &v, true);
                        drop(v);
                        if DEEP.with(|d| d.replace(false)) {
                            Err(Box::new(Stop {
                                kind: StopKind::StackExhausted,
                                func: func.to_string(),
                                line: program.exit_line,
                                during: None,
                                detail: "値の入れ子が深すぎる".into(),
                            }))
                        } else {
                            Ok(value)
                        }
                    }
                    Err(s) => Err(s),
                });
            }
            // 隔離した異常停止の報告は、従来どおり標準エラーに出す（複数のルートタスクのとき。
            // 1 つのときは呼び出し側が失敗として報告する）。
            if roots.len() > 1 && !opts.capture_console {
                for s in roots.iter().filter_map(|r| r.as_ref().err()) {
                    eprintln!("{s}");
                }
            }
            Ok(roots)
        }
        Err(s) => Err(s),
    };
    // テスト用: 環境変数 `SIGIL_RT_LIVE` があれば、終わった時点で生きている枠と確保済みの塊の数を
    // 標準エラーに出す（ネイティブのランタイムと同じ流儀。停止の経路での漏れの確認に使う）。
    if std::env::var_os("SIGIL_RT_LIVE").is_some() {
        eprintln!(
            "live: frames={} chunks={}",
            m.sch.live_frames(),
            m.chunks.len()
        );
    }
    drop(m);
    let drops = DROP_LOG.with(|l| l.borrow_mut().take()).unwrap_or_default();
    match res {
        Ok(roots) => Ok(RootsOutcome {
            roots,
            output,
            drops,
        }),
        Err(s) => Err(Failure {
            error: RunError::Stop(s),
            output,
            drops: drops.into_boxed_slice(),
        }),
    }
}

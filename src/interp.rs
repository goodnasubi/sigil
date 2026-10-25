//! 参照用のインタプリタ（M3）。構文木をそのまま実行する。
//!
//! 範囲（ADR 0006 の M3、ADR 0013）:
//! - 効果を持たない部分の実行、`requires`・`ensures`・`old(...)` の実行時検査、篩型の `T.check`。
//! - 異常停止（契約違反・`Int` の桁あふれ・0 での除算と剰余・標準の関数の `requires` 違反・スタックの枯渇）。
//!   どれもプログラムからは捕まえられず、`Stop` として報告する。終了コードは種類によらず `STOP_EXIT_CODE`。
//! - 実行の歩数に上限は設けない（ADR 0013 の決定 1）。スタックの枯渇だけは検出して異常停止にする。
//! - `console.log` は既定で本物の標準出力へ書く。`Options::capture_console`（`Options::captured()`）で文字列に溜める切替えができる（テスト用）。`fs.read`・`fs.write` は既定で実ファイル（M4）。`Options::fs_files` を渡すと、その表だけを読み書きする差し替えになる。
//!
//! 値の持ち方:
//! - 共有借用 `&x` は、その時点の値の写し（`Rc` の複製）。借用の間は元の値が変わらないことを検査器が保証する。
//! - 排他借用 `&mut x` は、変数の置き場所（スロット）と、その中の位置（フィールド名・添字の列）を持つ `MutRef`。
//!   参照の名前への代入は参照先を書き換える（ADR 0011 の決定 4）。
//! - 集合・構造体・組は `Rc` で持ち、書き換えるときに複製する（書き込み時複製）。
//!
//! 既知の制限（後続の PR で扱う）:
//! - 深い入れ子の値の破棄の反復化は、一覧・構造体・列挙・組の入れ子だけが対象。`Slot`・`MutRef`・クロージャの環境を通した深い入れ子の破棄は反復化の対象外（ホストのスタックを使う）。
//! - 値の破棄の順序（ADR 0006 の条件 9）は未実装。破棄の記録を持つ型がまだなく、インタプリタは値を `Rc` で解放する。
//! - 空の `List<Float>` の `sum` が `0.0` になるよう、一覧に `Float` の印を持たせる。印は関数の引数・戻り値・`let` の型注釈から付く。

use crate::ast::*;
use crate::error::Error;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

/// 異常停止のときのプロセスの終了コード（種類によらず 1 つ。`EX_SOFTWARE`）。ADR 0006 の M3 の記録を参照。
pub const STOP_EXIT_CODE: u8 = 70;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clause {
    Requires,
    Ensures,
}

impl fmt::Display for Clause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Clause::Requires => "requires",
            Clause::Ensures => "ensures",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopKind {
    /// 呼び出し側が `requires` を破った。
    RequiresViolated,
    /// 実装が `ensures` を破った。
    EnsuresViolated,
    /// 標準の関数の `requires` 違反（空の一覧の `min` など）。
    StdRequiresViolated,
    Overflow,
    DivisionByZero,
    StackExhausted,
}

/// 異常停止の報告（種類と位置。ADR 0013 の決定 1）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub kind: StopKind,
    /// 停止したときに実行していた関数。
    pub func: String,
    /// 停止したときに実行していた文（契約の式なら、その節）の行。
    pub line: u32,
    /// 契約の式の評価中に起きた停止なら、その契約を持つ関数と節。契約違反ではなく「評価中の異常停止」である。
    pub during: Option<(String, Clause)>,
    pub detail: String,
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let what = match self.kind {
            StopKind::RequiresViolated => "契約違反: requires を満たしていない".to_string(),
            StopKind::EnsuresViolated => "契約違反: ensures を満たしていない".to_string(),
            StopKind::StdRequiresViolated => {
                format!("標準の関数の requires 違反: {}", self.detail)
            }
            StopKind::Overflow => "Int の桁あふれ".to_string(),
            StopKind::DivisionByZero => "0 での除算".to_string(),
            StopKind::StackExhausted => "スタックの枯渇".to_string(),
        };
        write!(
            f,
            "異常停止: {what}（関数 {}、{} 行目）",
            self.func, self.line
        )?;
        if let Some((func, clause)) = &self.during {
            write!(f, "。{func} の {clause} の式の評価中に起きた")?;
        }
        Ok(())
    }
}

/// 入口の関数に渡す引数。
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    List(Vec<Arg>),
}

#[derive(Debug)]
pub enum RunError {
    /// 構文または型検査のエラー（実行の前に止める）。
    Check(Error),
    Stop(Box<Stop>),
    /// 現在の範囲にない機能（`fs`・`Shared` など）。
    Unsupported(String),
    /// 入口の指定の誤り（関数がない・引数の数が違う）。
    Entry(String),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Check(e) => write!(f, "{e}"),
            RunError::Stop(s) => write!(f, "{s}"),
            RunError::Unsupported(m) => write!(f, " 未対応: {m}"),
            RunError::Entry(m) => write!(f, " 入口の指定が誤り: {m}"),
        }
    }
}

/// 実行の成功。`value` は戻り値の表示（文字列は引用符付き）、`output` は `console.log` の出力。
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub value: String,
    pub output: String,
    /// `Options::trace_drops` のときの、構造体の値の破棄の記録（破棄の順に、破棄の時点の表示）。
    pub drops: Vec<String>,
}

/// 実行の失敗。異常停止の前に出力があれば `output` に残る。
#[derive(Debug)]
pub struct Failure {
    pub error: RunError,
    pub output: String,
    /// `Outcome::drops` と同じ。異常停止の前に破棄した値も含む。
    pub drops: Box<[String]>,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)
    }
}

pub struct Options {
    /// 実行するスレッドのスタックの大きさ。この 3/4 を使い切ると `StackExhausted` で停止する。
    pub stack_bytes: usize,
    /// 構造体の値が破棄された順序を `drops` に記録する（ADR 0006 の条件 9 のテスト用）。
    /// 記録中は、名前だけの値の使用を移動として扱い、`clone` は構造体を複製する。
    pub trace_drops: bool,
    /// `fs.read` のテスト用の差し替え（ADR 0004 の規則 7）。`Some` のとき、`fs.read(&p)` は
    /// この表（パス -> 内容）を引き、ない `p` は `Err(IoError.NotFound(p))` を返す。`fs.write` は表を書き換える。
    /// `None`（既定）のときは実際のファイルを読み書きする。
    pub fs_files: Option<std::collections::BTreeMap<String, String>>,
    /// `console.log` の出力を `Outcome::output` に溜める（テスト用）。偽（既定）なら本物の標準出力へ書き、`output` は空。
    pub capture_console: bool,
}

impl Options {
    /// `console.log` の出力を溜める設定（テスト用）。`run`・`run_sources` はこれを使う。
    pub fn captured() -> Self {
        Options {
            capture_console: true,
            ..Options::default()
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Options {
            stack_bytes: 256 * 1024 * 1024,
            trace_drops: false,
            fs_files: None,
            capture_console: false,
        }
    }
}

pub fn run(src: &str, func: &str, args: &[Arg]) -> Result<Outcome, Failure> {
    run_sources_with(&[src], func, args, &Options::captured())
}

pub fn run_with(src: &str, func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    run_sources_with(&[src], func, args, opts)
}

/// 複数のソースの宣言を 1 つにまとめて検査し、実行する（モジュールの宣言は全ソースで同じでなければ `ModuleMismatch`）。
/// サンプルのファイルと、テスト用の入口の関数を持つファイルを合わせるために使う。
pub fn run_sources(srcs: &[&str], func: &str, args: &[Arg]) -> Result<Outcome, Failure> {
    run_sources_with(srcs, func, args, &Options::captured())
}

pub fn run_sources_with(
    srcs: &[&str],
    func: &str,
    args: &[Arg],
    opts: &Options,
) -> Result<Outcome, Failure> {
    std::thread::scope(|s| {
        let spawned = std::thread::Builder::new()
            .stack_size(opts.stack_bytes)
            .spawn_scoped(s, || execute(srcs, func, args, opts));
        match spawned {
            Ok(h) => h
                .join()
                .unwrap_or_else(|_| Err(fail(RunError::Entry("インタプリタの内部エラー".into())))),
            Err(e) => Err(fail(RunError::Entry(format!("スレッドを作れない: {e}")))),
        }
    })
}

fn fail(error: RunError) -> Failure {
    Failure {
        error,
        output: String::new(),
        drops: Box::default(),
    }
}

fn execute(srcs: &[&str], func: &str, args: &[Arg], opts: &Options) -> Result<Outcome, Failure> {
    let mut module = None;
    let mut items = vec![];
    for s in srcs {
        let f = crate::parse(s).map_err(|e| fail(RunError::Check(e)))?;
        match &module {
            None => module = Some(f.module.clone()),
            // 最初のファイルの宣言を全体に使うので、食い違うファイルは黙って検査が外れる。
            // 核（`bare`）の検査を迂回させないため、宣言がすべて同じことを求める（ADR 0014 決定 4）。
            Some(m) => {
                let d = &f.module;
                if d.name != m.name || d.targets != m.targets || d.trust != m.trust {
                    return Err(fail(RunError::Check(crate::Error::new(
                        crate::ErrorKind::ModuleMismatch,
                        d.span,
                        "合わせるファイルのモジュールの宣言（名前・`targets`・`trust`）が最初のファイルと食い違う（宣言がすべて同じファイルだけを合わせられる）",
                    ))));
                }
            }
        }
        items.extend(f.items);
    }
    let Some(module) = module else {
        return Err(fail(RunError::Entry("ソースがない".into())));
    };
    let file = File { module, items };
    crate::check::check_file(&file).map_err(|e| fail(RunError::Check(e)))?;
    let mut it = Interp::new(&file, opts);
    DROP_LOG.with(|l| *l.borrow_mut() = opts.trace_drops.then(Vec::new));
    let Some(decl) = it.prog.fns.get(func).copied() else {
        return Err(fail(RunError::Entry(format!("関数 {func} がない"))));
    };
    if decl.params.len() != args.len() {
        return Err(fail(RunError::Entry(format!(
            "{func} は引数を {} 個とる（{} 個渡された）",
            decl.params.len(),
            args.len()
        ))));
    }
    let mut argv = Vec::with_capacity(args.len());
    for (p, a) in decl.params.iter().zip(args) {
        match coerce(a, &p.ty) {
            Ok(v) => argv.push(v),
            Err(m) => {
                return Err(fail(RunError::Entry(format!(
                    "引数 {} の型が合わない: {m}",
                    p.name
                ))))
            }
        }
    }
    let result = it.call_decl(decl, argv);
    let output = std::mem::take(&mut it.out);
    let res = match result {
        Ok(v) => {
            let value = v.show(true);
            // 戻り値は表示した後に破棄する（破棄の記録に含める）。
            drop(v);
            match it.after_deep() {
                Err(Abort::Stop(s)) => Err((RunError::Stop(Box::new(s)), output)),
                _ => Ok((value, output)),
            }
        }
        Err(Abort::Stop(s)) => Err((RunError::Stop(Box::new(s)), output)),
        Err(Abort::Unsupported(m)) => Err((RunError::Unsupported(m), output)),
        Err(Abort::Return(_)) => Err((RunError::Unsupported("関数の外への return".into()), output)),
    };
    drop(it);
    let drops = DROP_LOG.with(|l| l.borrow_mut().take()).unwrap_or_default();
    match res {
        Ok((value, output)) => Ok(Outcome {
            value,
            output,
            drops,
        }),
        Err((error, output)) => Err(Failure {
            error,
            output,
            drops: drops.into_boxed_slice(),
        }),
    }
}

fn arg_name(a: &Arg) -> &'static str {
    match a {
        Arg::Int(_) => "整数",
        Arg::Float(_) => "小数",
        Arg::Bool(_) => "真偽値",
        Arg::Str(_) => "文字列",
        Arg::List(_) => "一覧",
    }
}

/// 入口の引数を、宣言の型に合わせて値にする。`Float` の位置の整数は `Float` にする。
/// 組み込みの型（`Int`・`Float`・`Bool`・`String`・`List<T>`）に合わないものは誤りとする。
/// それ以外の型（構造体・篩型など）は、与えられた値をそのまま渡す。
fn coerce(a: &Arg, ty: &Type) -> Result<Value, String> {
    let ty = match ty {
        Type::Ref { inner, .. } => inner,
        t => t,
    };
    let Type::Path { path, args } = ty else {
        return Ok(to_value(a));
    };
    let name = path.last().map(String::as_str).unwrap_or("");
    let mismatch = || Err(format!("{name} が要るが、{}が渡された", arg_name(a)));
    match (name, a) {
        ("Int", Arg::Int(n)) => Ok(Value::Int(*n)),
        ("Float", Arg::Float(x)) => Ok(Value::Float(*x)),
        ("Float", Arg::Int(n)) => Ok(Value::Float(*n as f64)),
        ("Bool", Arg::Bool(b)) => Ok(Value::Bool(*b)),
        ("String", Arg::Str(x)) => Ok(Value::Str(x.as_str().into())),
        ("Path", Arg::Str(x)) => Ok(Value::Path(x.as_str().into())),
        ("List", Arg::List(xs)) => {
            let elem = args.first();
            let mut items = Vec::with_capacity(xs.len());
            for x in xs {
                items.push(match elem {
                    Some(t) => coerce(x, t)?,
                    None => to_value(x),
                });
            }
            let float = matches!(elem, Some(Type::Path { path, .. }) if path.last().map(String::as_str) == Some("Float"));
            Ok(Value::List(Rc::new(ListVal { items, float })))
        }
        ("Int" | "Float" | "Bool" | "String" | "Path" | "List", _) => mismatch(),
        _ => Ok(to_value(a)),
    }
}

fn to_value(a: &Arg) -> Value {
    match a {
        Arg::Int(n) => Value::Int(*n),
        Arg::Float(x) => Value::Float(*x),
        Arg::Bool(b) => Value::Bool(*b),
        Arg::Str(s) => Value::Str(s.as_str().into()),
        Arg::List(xs) => {
            let items: Vec<Value> = xs.iter().map(to_value).collect();
            let float = matches!(items.first(), Some(Value::Float(_)));
            Value::List(Rc::new(ListVal { items, float }))
        }
    }
}

// ---------------------------------------------------------------------------
// 値

type Slot = Rc<RefCell<Value>>;

#[derive(Debug, Clone)]
enum Value {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(Rc<str>),
    /// `Path`。`String` とは別の値で、`String` のメソッドは使えない。
    Path(Rc<str>),
    Tuple(Rc<Vec<Value>>),
    List(Rc<ListVal>),
    Struct(Rc<StructVal>),
    Variant(Rc<VariantVal>),
    ParseError,
    /// `Cell<T>`（`Shared<Cell<T>>` の実体。共有するので `Rc` の中身を書き換える）。
    Cell(Rc<std::cell::RefCell<Value>>),
    /// `Shared<Cell<T>>`。`Rc` を共有するので、`clone` は同じ `Cell` を指す（`Cell` 単体の `clone` は独立した複製）。
    Shared(Rc<Value>),
    Closure(Rc<ClosureVal>),
    /// 名前で参照した関数。
    Named(Rc<str>),
    /// 列挙型の腕の構築子（`Some`・`ConfigError.Io` など）。
    Ctor(Rc<str>, Rc<str>),
    MutRef(Rc<MutRef>),
}

#[derive(Debug, Clone)]
struct ListVal {
    items: Vec<Value>,
    /// `Float` の一覧であることの印（空の `sum` が `0.0` を返すため）。
    float: bool,
}

#[derive(Debug, Clone)]
struct StructVal {
    name: Rc<str>,
    fields: Vec<(Rc<str>, Value)>,
    /// 破棄の記録を済ませた（`drain` が先に記録した）印。
    logged: bool,
}

#[derive(Debug, Clone)]
struct VariantVal {
    enum_name: Rc<str>,
    name: Rc<str>,
    args: Vec<Value>,
}

fn show_struct(s: &StructVal) -> String {
    let xs: Vec<String> = s
        .fields
        .iter()
        .map(|(k, v)| format!("{k}: {}", v.show(true)))
        .collect();
    format!("{} {{ {} }}", s.name, xs.join(", "))
}

/// 破棄の記録（`Options::trace_drops`）。構造体の値が破棄された時点の表示を、破棄の順に残す。
fn log_drop(s: &StructVal) {
    let _ = DROP_LOG.try_with(|l| {
        if l.borrow().is_some() {
            let text = show_struct(s);
            if let Some(v) = l.borrow_mut().as_mut() {
                v.push(text);
            }
        }
    });
}

/// `Copyable` な値か（値の位置で複製され、移動しない）。記録中の移動の判定に使う。
fn is_copy_value(v: &Value) -> bool {
    match v {
        Value::Unit
        | Value::Int(_)
        | Value::Float(_)
        | Value::Bool(_)
        | Value::ParseError
        | Value::Closure(_)
        | Value::Named(_)
        | Value::Ctor(..)
        | Value::MutRef(_) => true,
        Value::Str(_)
        | Value::Path(_)
        | Value::List(_)
        | Value::Struct(_)
        | Value::Cell(_)
        | Value::Shared(_) => false,
        Value::Tuple(t) => t.iter().all(is_copy_value),
        Value::Variant(x) => {
            matches!(&*x.enum_name, "Option" | "Result") && x.args.iter().all(is_copy_value)
        }
    }
}

/// 構造体・一覧・列挙・組を作り直して複製する（記録中の `clone` は、破棄される値を 2 つにする）。
fn deep_clone(v: &Value) -> Value {
    match v {
        Value::Tuple(t) => Value::Tuple(Rc::new(t.iter().map(deep_clone).collect())),
        Value::List(l) => Value::List(Rc::new(ListVal {
            items: l.items.iter().map(deep_clone).collect(),
            float: l.float,
        })),
        Value::Struct(s) => Value::Struct(Rc::new(StructVal {
            name: s.name.clone(),
            fields: s
                .fields
                .iter()
                .map(|(k, x)| (k.clone(), deep_clone(x)))
                .collect(),
            logged: false,
        })),
        Value::Variant(x) => Value::Variant(Rc::new(VariantVal {
            enum_name: x.enum_name.clone(),
            name: x.name.clone(),
            args: x.args.iter().map(deep_clone).collect(),
        })),
        // `Cell` 単体の複製は独立する（ADR 0006 の M4 の記録）。`Shared` は同じ `Cell` を指したまま。
        Value::Cell(c) => Value::Cell(Rc::new(RefCell::new(deep_clone(&c.borrow())))),
        other => other.clone(),
    }
}

/// 値の中に `Cell` が（`Shared` の外に）あるか。あれば `clone` は `Rc` の共有でなく複製を作る。
fn contains_cell(v: &Value) -> bool {
    let mut stack = vec![v];
    while let Some(v) = stack.pop() {
        match v {
            Value::Cell(_) => return true,
            Value::Tuple(t) => stack.extend(t.iter()),
            Value::List(l) => stack.extend(l.items.iter()),
            Value::Struct(s) => stack.extend(s.fields.iter().map(|(_, x)| x)),
            Value::Variant(x) => stack.extend(x.args.iter()),
            _ => {}
        }
    }
    false
}

fn tracing() -> bool {
    DROP_LOG.try_with(|l| l.borrow().is_some()).unwrap_or(false)
}

/// 深く入れ子になった値（`Cons(1, Cons(2, ...))` を何百万段もつないだもの）を、再帰せずに破棄する。
/// 既定の `Drop` は Rc の入れ子の深さだけ再帰し、ホストのスタックを溢れさせる。
/// 破棄の順序は Rust と同じ（値そのもの、次にフィールド・要素を宣言の順）にする（ADR 0006 の条件 9）。
/// 子を逆順に積み、後入れ先出しで取り出すと、先頭の子から順に破棄される。
fn drain(mut stack: Vec<Value>) {
    stack.reverse();
    while let Some(v) = stack.pop() {
        match v {
            Value::List(rc) => {
                if let Ok(mut inner) = Rc::try_unwrap(rc) {
                    let n = stack.len();
                    stack.append(&mut inner.items);
                    stack[n..].reverse();
                }
            }
            Value::Struct(rc) => {
                if let Ok(mut inner) = Rc::try_unwrap(rc) {
                    if !inner.logged {
                        log_drop(&inner);
                        inner.logged = true;
                    }
                    let n = stack.len();
                    stack.extend(
                        std::mem::take(&mut inner.fields)
                            .into_iter()
                            .map(|(_, x)| x),
                    );
                    stack[n..].reverse();
                }
            }
            Value::Variant(rc) => {
                if let Ok(mut inner) = Rc::try_unwrap(rc) {
                    let n = stack.len();
                    stack.append(&mut inner.args);
                    stack[n..].reverse();
                }
            }
            Value::Tuple(rc) => {
                if let Ok(inner) = Rc::try_unwrap(rc) {
                    let n = stack.len();
                    stack.extend(inner);
                    stack[n..].reverse();
                }
            }
            Value::Cell(rc) => {
                if let Ok(inner) = Rc::try_unwrap(rc) {
                    stack.push(inner.into_inner());
                }
            }
            Value::Shared(rc) => {
                if let Ok(inner) = Rc::try_unwrap(rc) {
                    stack.push(inner);
                }
            }
            _ => {}
        }
    }
}

impl Drop for ListVal {
    fn drop(&mut self) {
        if !self.items.is_empty() {
            drain(std::mem::take(&mut self.items));
        }
    }
}

impl Drop for StructVal {
    fn drop(&mut self) {
        if !self.logged {
            log_drop(self);
            self.logged = true;
        }
        if !self.fields.is_empty() {
            drain(
                std::mem::take(&mut self.fields)
                    .into_iter()
                    .map(|(_, x)| x)
                    .collect(),
            );
        }
    }
}

impl Drop for VariantVal {
    fn drop(&mut self) {
        if !self.args.is_empty() {
            drain(std::mem::take(&mut self.args));
        }
    }
}

thread_local! {
    /// 実行中のスレッドのスタックの基準と予算。表示・比較のような呼び出しを伴わない再帰も、これで止める。
    static STACK_LIMIT: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, usize::MAX)) };
    static DEEP: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// `Some` のあいだ、構造体の破棄を記録する。
    static DROP_LOG: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// スタックの予算を超えたら印を立てて真を返す。呼び出し側が `Interp::after_deep` で停止に変える。
fn deep() -> bool {
    let (base, budget) = STACK_LIMIT.with(|c| c.get());
    if base.abs_diff(stack_addr()) > budget {
        DEEP.with(|d| d.set(true));
        true
    } else {
        false
    }
}

#[derive(Debug)]
struct ClosureVal {
    params: Vec<String>,
    body: ClosureBody,
    env: Vec<(String, Slot)>,
    owner: String,
}

#[derive(Debug, Clone)]
enum Seg {
    Field(Rc<str>),
    Index(usize),
}

#[derive(Debug)]
struct MutRef {
    slot: Slot,
    path: Vec<Seg>,
}

fn variant(e: &str, n: &str, args: Vec<Value>) -> Value {
    Value::Variant(Rc::new(VariantVal {
        enum_name: e.into(),
        name: n.into(),
        args,
    }))
}

fn some(v: Value) -> Value {
    variant("Option", "Some", vec![v])
}

fn none() -> Value {
    variant("Option", "None", vec![])
}

/// `fs` の失敗を `IoError` の腕（`NotFound`・`PermissionDenied`・`Other`。どれもパスを持つ）にする。
fn io_error(kind: std::io::ErrorKind, path: Rc<str>) -> Value {
    let name = match kind {
        std::io::ErrorKind::NotFound => "NotFound",
        std::io::ErrorKind::PermissionDenied => "PermissionDenied",
        _ => "Other",
    };
    variant("IoError", name, vec![Value::Path(path)])
}

fn ok(v: Value) -> Value {
    variant("Result", "Ok", vec![v])
}

fn err(v: Value) -> Value {
    variant("Result", "Err", vec![v])
}

fn new_slot(v: Value) -> Slot {
    Rc::new(RefCell::new(v))
}

fn read_path(v: &Value, path: &[Seg]) -> Value {
    let mut cur = v.clone();
    for seg in path {
        cur = match (&cur, seg) {
            (Value::Struct(s), Seg::Field(n)) => s
                .fields
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.clone())
                .unwrap_or(Value::Unit),
            (Value::Tuple(t), Seg::Field(n)) => n
                .parse::<usize>()
                .ok()
                .and_then(|i| t.get(i))
                .cloned()
                .unwrap_or(Value::Unit),
            (Value::List(l), Seg::Index(i)) => l.items.get(*i).cloned().unwrap_or(Value::Unit),
            _ => Value::Unit,
        };
    }
    cur
}

fn with_path<T>(v: &mut Value, path: &[Seg], f: impl FnOnce(&mut Value) -> T) -> Option<T> {
    let Some((seg, rest)) = path.split_first() else {
        return Some(f(v));
    };
    match (v, seg) {
        (Value::Struct(s), Seg::Field(n)) => {
            let s = Rc::make_mut(s);
            let (_, fv) = s.fields.iter_mut().find(|(k, _)| k == n)?;
            with_path(fv, rest, f)
        }
        (Value::Tuple(t), Seg::Field(n)) => {
            let i: usize = n.parse().ok()?;
            let t = Rc::make_mut(t);
            with_path(t.get_mut(i)?, rest, f)
        }
        (Value::List(l), Seg::Index(i)) => {
            let l = Rc::make_mut(l);
            with_path(l.items.get_mut(*i)?, rest, f)
        }
        _ => None,
    }
}

fn deref(v: Value) -> Value {
    let mut v = v;
    while let Value::MutRef(m) = &v {
        let n = read_path(&m.slot.borrow(), &m.path);
        v = n;
    }
    v
}

/// 変数が排他借用を持つとき、その参照先まで辿る。
fn follow(slot: Slot, path: Vec<Seg>) -> (Slot, Vec<Seg>) {
    let mut slot = slot;
    let mut path = path;
    loop {
        let cur = read_path(&slot.borrow(), &path);
        match cur {
            Value::MutRef(m) => {
                slot = m.slot.clone();
                path = m.path.clone();
            }
            _ => break,
        }
    }
    (slot, path)
}

fn fmt_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        let s = format!("{f}");
        if s.contains('.') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

impl Value {
    /// 表示。`quote` が偽なら、最上位の文字列だけ引用符を付けない（`console.log` 用）。
    fn show(&self, quote: bool) -> String {
        if deep() {
            return String::new();
        }
        match self {
            Value::Unit => "()".into(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => fmt_float(*f),
            Value::Bool(b) => b.to_string(),
            Value::Str(s) | Value::Path(s) => {
                if quote {
                    format!("{:?}", &**s)
                } else {
                    s.to_string()
                }
            }
            Value::Tuple(t) => {
                let xs: Vec<String> = t.iter().map(|v| v.show(true)).collect();
                format!("({})", xs.join(", "))
            }
            Value::List(l) => {
                let xs: Vec<String> = l.items.iter().map(|v| v.show(true)).collect();
                format!("[{}]", xs.join(", "))
            }
            Value::Struct(s) => show_struct(s),
            Value::Variant(v) => {
                let head = if &*v.enum_name == "Option" || &*v.enum_name == "Result" {
                    v.name.to_string()
                } else {
                    format!("{}.{}", v.enum_name, v.name)
                };
                if v.args.is_empty() {
                    head
                } else {
                    let xs: Vec<String> = v.args.iter().map(|a| a.show(true)).collect();
                    format!("{head}({})", xs.join(", "))
                }
            }
            Value::ParseError => "ParseError".into(),
            Value::Cell(c) => format!("Cell({})", c.borrow().show(true)),
            Value::Shared(x) => format!("Shared({})", x.show(true)),
            Value::Closure(_) | Value::Named(_) | Value::Ctor(..) => "<関数>".into(),
            Value::MutRef(_) => deref(self.clone()).show(quote),
        }
    }

    fn type_name(&self) -> Option<String> {
        Some(match self {
            Value::Int(_) => "Int".into(),
            Value::Float(_) => "Float".into(),
            Value::Bool(_) => "Bool".into(),
            Value::Str(_) => "String".into(),
            Value::Path(_) => "Path".into(),
            Value::List(_) => "List".into(),
            Value::Struct(s) => s.name.to_string(),
            Value::Variant(v) => v.enum_name.to_string(),
            _ => return None,
        })
    }
}

fn values_eq(a: &Value, b: &Value) -> bool {
    if deep() {
        return false;
    }
    match (a, b) {
        (Value::Unit, Value::Unit) | (Value::ParseError, Value::ParseError) => true,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Str(x), Value::Str(y)) | (Value::Path(x), Value::Path(y)) => x == y,
        (Value::Tuple(x), Value::Tuple(y)) => seq_eq(x, y),
        (Value::List(x), Value::List(y)) => seq_eq(&x.items, &y.items),
        (Value::Struct(x), Value::Struct(y)) => {
            x.name == y.name
                && x.fields.len() == y.fields.len()
                && x.fields
                    .iter()
                    .zip(&y.fields)
                    .all(|((k1, v1), (k2, v2))| k1 == k2 && values_eq(v1, v2))
        }
        (Value::Variant(x), Value::Variant(y)) => {
            x.enum_name == y.enum_name && x.name == y.name && seq_eq(&x.args, &y.args)
        }
        (Value::Cell(x), Value::Cell(y)) => Rc::ptr_eq(x, y) || values_eq(&x.borrow(), &y.borrow()),
        (Value::Shared(x), Value::Shared(y)) => Rc::ptr_eq(x, y) || values_eq(x, y),
        _ => false,
    }
}

fn seq_eq(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_eq(x, y))
}

fn cmp_val(a: &Value, b: &Value) -> Option<Ordering> {
    if deep() {
        return None;
    }
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        (Value::Tuple(x), Value::Tuple(y)) => cmp_seq(x, y),
        (Value::List(x), Value::List(y)) => cmp_seq(&x.items, &y.items),
        _ => None,
    }
}

fn cmp_seq(a: &[Value], b: &[Value]) -> Option<Ordering> {
    for (x, y) in a.iter().zip(b) {
        match cmp_val(x, y)? {
            Ordering::Equal => {}
            o => return Some(o),
        }
    }
    Some(a.len().cmp(&b.len()))
}

/// 一覧の型注釈 `List<Float>` から、値に `Float` の印を付ける。
fn hint(v: Value, ty: &Type) -> Value {
    match ty {
        Type::Ref {
            mutable: false,
            inner,
        } => hint(v, inner),
        Type::Path { path, args } if path.last().map(String::as_str) == Some("List") => {
            let float = matches!(
                args.first(),
                Some(Type::Path { path, .. }) if path.last().map(String::as_str) == Some("Float")
            );
            match v {
                Value::List(mut l) if float && !l.float => {
                    Rc::make_mut(&mut l).float = true;
                    Value::List(l)
                }
                other => other,
            }
        }
        _ => v,
    }
}

// ---------------------------------------------------------------------------
// 実行

enum Abort {
    Stop(Stop),
    Return(Value),
    Unsupported(String),
}

type R<T> = Result<T, Abort>;

fn unsupported<T>(m: impl Into<String>) -> R<T> {
    Err(Abort::Unsupported(m.into()))
}

struct Program<'a> {
    fns: HashMap<String, &'a FnDecl>,
    methods: HashMap<(String, String), &'a FnDecl>,
    structs: HashMap<String, &'a StructDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    refined: HashMap<String, &'a TypeDecl>,
}

struct Frame {
    name: String,
    scopes: Vec<Vec<(String, Slot)>>,
    result: Option<Value>,
    self_val: Option<Value>,
    olds: HashMap<usize, Value>,
}

impl Frame {
    fn new(name: &str) -> Frame {
        Frame {
            name: name.to_string(),
            scopes: vec![],
            result: None,
            self_val: None,
            olds: HashMap::new(),
        }
    }
}

struct Interp<'a> {
    prog: Program<'a>,
    frames: Vec<Frame>,
    out: String,
    /// `console.log` を `out` に溜める（テスト用。偽なら本物の標準出力へ書く）。
    capture: bool,
    line: u32,
    /// 入口の関数が戻った時点の名前と行。戻り値の表示中の停止（フレームが空）の位置に使う。
    exit_at: (String, u32),
    base: usize,
    budget: usize,
    /// `Options::fs_files` の写し（`fs.read` の差し替え）。
    fs: Option<std::collections::BTreeMap<String, String>>,
}

#[inline(never)]
fn stack_addr() -> usize {
    let x = 0u8;
    std::hint::black_box(&x) as *const u8 as usize
}

fn type_key(t: &Type) -> Option<String> {
    match t {
        Type::Path { path, .. } => path.last().cloned(),
        _ => None,
    }
}

fn is_place(e: &Expr) -> bool {
    match e {
        Expr::Name(_) => true,
        Expr::Paren(i) => is_place(i),
        Expr::Field { expr, .. } => is_place(expr),
        _ => false,
    }
}

/// `ensures` の中の `old(e)` の `e`（捕捉する式）を集める。
fn collect_olds<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
    match e {
        Expr::Old(i) => out.push(i),
        Expr::Paren(i) | Expr::Try(i) => collect_olds(i, out),
        Expr::Tuple(es) => es.iter().for_each(|x| collect_olds(x, out)),
        Expr::Unary { expr, .. } => collect_olds(expr, out),
        Expr::Binary { lhs, rhs, .. } => {
            collect_olds(lhs, out);
            collect_olds(rhs, out);
        }
        Expr::Field { expr, .. } => collect_olds(expr, out),
        Expr::Call { callee, args } => {
            collect_olds(callee, out);
            args.iter().for_each(|x| collect_olds(x, out));
        }
        Expr::StructLit { fields, .. } => fields.iter().for_each(|(_, x)| collect_olds(x, out)),
        Expr::Match {
            scrutinee, arms, ..
        } => {
            collect_olds(scrutinee, out);
            arms.iter().for_each(|a| collect_olds(&a.body, out));
        }
        _ => {}
    }
}

fn old_key(e: &Expr) -> usize {
    e as *const Expr as usize
}

impl<'a> Interp<'a> {
    fn new(file: &'a File, opts: &Options) -> Interp<'a> {
        let mut prog = Program {
            fns: HashMap::new(),
            methods: HashMap::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            refined: HashMap::new(),
        };
        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    prog.fns.insert(f.name.clone(), f);
                }
                Item::Impl(i) => {
                    if let Some(k) = type_key(&i.ty) {
                        for f in &i.fns {
                            prog.methods.insert((k.clone(), f.name.clone()), f);
                        }
                    }
                }
                Item::Struct(s) => {
                    prog.structs.insert(s.name.clone(), s);
                }
                Item::Enum(e) => {
                    prog.enums.insert(e.name.clone(), e);
                }
                Item::Type(t) => {
                    prog.refined.insert(t.name.clone(), t);
                }
            }
        }
        let base = stack_addr();
        STACK_LIMIT.with(|c| c.set((base, opts.stack_bytes / 4 * 3)));
        DEEP.with(|d| d.set(false));
        Interp {
            prog,
            frames: vec![],
            out: String::new(),
            capture: opts.capture_console,
            line: 0,
            exit_at: (String::new(), 0),
            base,
            budget: opts.stack_bytes / 4 * 3,
            fs: opts.fs_files.clone(),
        }
    }

    // --- 停止 ---

    fn stop(&self, kind: StopKind, detail: &str) -> Abort {
        let (func, line) = match self.frames.last() {
            Some(f) => (f.name.clone(), self.line),
            None => self.exit_at.clone(),
        };
        Abort::Stop(Stop {
            kind,
            func,
            line,
            during: None,
            detail: detail.to_string(),
        })
    }

    /// 表示・比較の途中でスタックの予算を超えていたら、スタック枯渇の停止にする。
    fn after_deep(&self) -> R<()> {
        if DEEP.with(|d| d.replace(false)) {
            Err(self.stop(StopKind::StackExhausted, "値の入れ子が深すぎる"))
        } else {
            Ok(())
        }
    }

    fn check_stack(&self) -> R<()> {
        if self.base.abs_diff(stack_addr()) > self.budget {
            Err(self.stop(StopKind::StackExhausted, ""))
        } else {
            Ok(())
        }
    }

    // --- 環境 ---

    fn lookup(&self, name: &str) -> Option<Slot> {
        let f = self.frames.last()?;
        for sc in f.scopes.iter().rev() {
            for (k, s) in sc.iter().rev() {
                if k == name {
                    return Some(s.clone());
                }
            }
        }
        None
    }

    fn push_scope(&mut self, binds: Vec<(String, Value)>) {
        let sc = binds.into_iter().map(|(k, v)| (k, new_slot(v))).collect();
        if let Some(f) = self.frames.last_mut() {
            f.scopes.push(sc);
        }
    }

    /// スコープを抜ける。宣言の逆順に値を破棄する（ADR 0006 の条件 9）。
    fn pop_scope(&mut self) {
        if let Some(f) = self.frames.last_mut() {
            if let Some(mut sc) = f.scopes.pop() {
                while sc.pop().is_some() {}
            }
        }
    }

    /// 関数の枠を抜ける。戻り値・`old` の写しを先に手放し、内側のスコープから外側へ、各スコープでは宣言の逆順に破棄する。
    fn release_frame(&mut self) {
        if let Some(mut f) = self.frames.pop() {
            f.result = None;
            f.self_val = None;
            f.olds.clear();
            while let Some(mut sc) = f.scopes.pop() {
                while sc.pop().is_some() {}
            }
        }
    }

    /// 値の位置で使う式を評価する。破棄の記録中は、`Copyable` でない値を持つ変数の名前だけの式を移動とし、
    /// 変数の中身を空にする（移動元が残って破棄が遅れないようにする）。
    fn eval_move(&mut self, e: &Expr) -> R<Value> {
        let mut inner = e;
        while let Expr::Paren(i) = inner {
            inner = i;
        }
        if let Expr::Name(n) = inner {
            if tracing() {
                if let Some(slot) = self.lookup(n) {
                    let copy = is_copy_value(&slot.borrow());
                    if !copy {
                        let v = std::mem::replace(&mut *slot.borrow_mut(), Value::Unit);
                        return Ok(v);
                    }
                }
            }
        }
        self.eval(e)
    }

    fn bind(&mut self, name: &str, v: Value) {
        if let Some(sc) = self.frames.last_mut().and_then(|f| f.scopes.last_mut()) {
            sc.push((name.to_string(), new_slot(v)));
        }
    }

    // --- 関数の呼び出し ---

    fn call_decl(&mut self, d: &'a FnDecl, args: Vec<Value>) -> R<Value> {
        self.check_stack()?;
        if args.len() != d.params.len() {
            return unsupported(format!("{} の引数の数", d.name));
        }
        let saved_line = self.line;
        let mut frame = Frame::new(&d.name);
        let scope = d
            .params
            .iter()
            .zip(args)
            .map(|(p, a)| (p.name.clone(), new_slot(hint(a, &p.ty))))
            .collect();
        frame.scopes.push(scope);
        self.frames.push(frame);
        let r = self.run_decl(d);
        if self.frames.len() == 1 {
            self.exit_at = (d.name.clone(), self.line);
        }
        self.release_frame();
        self.line = saved_line;
        r
    }

    fn run_decl(&mut self, d: &'a FnDecl) -> R<Value> {
        if let Some(req) = &d.requires {
            self.line = d.clause_lines[1];
            if !self.contract(req, &d.name, Clause::Requires)? {
                return Err(self.stop(StopKind::RequiresViolated, ""));
            }
        }
        if let Some(ens) = &d.ensures {
            let mut olds = vec![];
            collect_olds(ens, &mut olds);
            for o in olds {
                self.line = d.clause_lines[2];
                let v = self.in_contract(&d.name, Clause::Ensures, |s| s.val(o))?;
                if let Some(f) = self.frames.last_mut() {
                    f.olds.insert(old_key(o), v);
                }
            }
        }
        let r = match self.exec_block(&d.body) {
            Ok(()) => Value::Unit,
            Err(Abort::Return(v)) => v,
            Err(e) => return Err(e),
        };
        let r = match &d.ret {
            Some(t) => hint(r, t),
            None => r,
        };
        if let Some(ens) = &d.ensures {
            if let Some(f) = self.frames.last_mut() {
                f.result = Some(r.clone());
            }
            self.line = d.clause_lines[2];
            if !self.contract(ens, &d.name, Clause::Ensures)? {
                return Err(self.stop(StopKind::EnsuresViolated, ""));
            }
        }
        Ok(r)
    }

    /// 契約の式の評価中の異常停止に、どの契約の評価中かを付ける（契約違反とは区別する。ADR 0013 の決定 1）。
    fn in_contract<T>(
        &mut self,
        func: &str,
        clause: Clause,
        f: impl FnOnce(&mut Self) -> R<T>,
    ) -> R<T> {
        match f(self) {
            Err(Abort::Stop(mut s)) => {
                if s.during.is_none() {
                    s.during = Some((func.to_string(), clause));
                }
                Err(Abort::Stop(s))
            }
            other => other,
        }
    }

    fn contract(&mut self, e: &Expr, func: &str, clause: Clause) -> R<bool> {
        match self.in_contract(func, clause, |s| s.val(e))? {
            Value::Bool(b) => Ok(b),
            _ => unsupported("契約の式が Bool でない"),
        }
    }

    fn call_closure(&mut self, c: &ClosureVal, args: Vec<Value>) -> R<Value> {
        self.check_stack()?;
        if args.len() != c.params.len() {
            return unsupported("クロージャの引数の数");
        }
        let saved_line = self.line;
        let mut frame = Frame::new(&c.owner);
        frame.scopes.push(c.env.clone());
        frame.scopes.push(
            c.params
                .iter()
                .zip(args)
                .map(|(p, a)| (p.clone(), new_slot(a)))
                .collect(),
        );
        self.frames.push(frame);
        let r = match &c.body {
            ClosureBody::Expr(e) => self.val(e),
            ClosureBody::Block(b) => match self.exec_block(b) {
                Ok(()) => Ok(Value::Unit),
                Err(Abort::Return(v)) => Ok(v),
                Err(e) => Err(e),
            },
        };
        self.release_frame();
        self.line = saved_line;
        r
    }

    fn call_value(&mut self, f: &Value, args: Vec<Value>) -> R<Value> {
        match f {
            Value::Closure(c) => {
                let c = c.clone();
                self.call_closure(&c, args)
            }
            Value::Named(n) => {
                let Some(d) = self.prog.fns.get(&**n).copied() else {
                    return unsupported(format!("関数 {n}"));
                };
                self.call_decl(d, args)
            }
            Value::Ctor(e, n) => Ok(variant(e, n, args)),
            Value::MutRef(_) => {
                let d = deref(f.clone());
                self.call_value(&d, args)
            }
            _ => unsupported("関数でない値の呼び出し"),
        }
    }

    // --- 文 ---

    fn exec_block(&mut self, b: &Block) -> R<()> {
        self.push_scope(vec![]);
        let mut res = Ok(());
        for s in &b.stmts {
            if let Err(e) = self.exec_stmt(s) {
                res = Err(e);
                break;
            }
        }
        self.pop_scope();
        res
    }

    fn exec_stmt(&mut self, s: &Stmt) -> R<()> {
        self.line = s.span.line;
        match &s.kind {
            StmtKind::Let {
                pattern, ty, value, ..
            } => {
                let mut v = self.eval_move(value)?;
                if let Some(t) = ty {
                    v = hint(v, t);
                }
                let mut binds = vec![];
                if !self.match_pat(pattern, &v, &mut binds) {
                    return unsupported("let のパターンが合わない");
                }
                for (k, v) in binds {
                    self.bind(&k, v);
                }
                Ok(())
            }
            StmtKind::Assign { target, value } => {
                let v = deref(self.eval_move(value)?);
                let (slot, path) = self.resolve_place(target)?;
                let done = with_path(&mut slot.borrow_mut(), &path, |x| *x = v);
                if done.is_none() {
                    return unsupported("代入先が見つからない");
                }
                Ok(())
            }
            StmtKind::Expr(e) => self.eval(e).map(|_| ()),
            StmtKind::Return(e) => {
                let v = match e {
                    Some(e) => deref(self.eval_move(e)?),
                    None => Value::Unit,
                };
                Err(Abort::Return(v))
            }
            StmtKind::If(i) => self.exec_if(i),
            StmtKind::While { cond, body } => {
                loop {
                    self.line = s.span.line;
                    match self.val(cond)? {
                        Value::Bool(true) => self.exec_block(body)?,
                        Value::Bool(false) => break,
                        _ => return unsupported("while の条件が Bool でない"),
                    }
                }
                Ok(())
            }
            StmtKind::Match {
                scrutinee, arms, ..
            } => {
                let v = self.eval(scrutinee)?;
                for arm in arms {
                    let mut binds = vec![];
                    if self.match_pat(&arm.pattern, &v, &mut binds) {
                        self.push_scope(binds);
                        let r = self.exec_block(&arm.body);
                        self.pop_scope();
                        return r;
                    }
                }
                unsupported("match のどの腕にも合わない")
            }
        }
    }

    fn exec_if(&mut self, i: &IfStmt) -> R<()> {
        match self.val(&i.cond)? {
            Value::Bool(true) => self.exec_block(&i.then),
            Value::Bool(false) => match i.otherwise.as_deref() {
                Some(Else::If(n)) => self.exec_if(n),
                Some(Else::Block(b)) => self.exec_block(b),
                None => Ok(()),
            },
            _ => unsupported("if の条件が Bool でない"),
        }
    }

    // --- パターン ---

    fn match_pat(&self, p: &Pattern, v: &Value, out: &mut Vec<(String, Value)>) -> bool {
        match p {
            Pattern::Wild => true,
            Pattern::Bind(n) => {
                out.push((n.clone(), v.clone()));
                true
            }
            _ => {
                let d = deref(v.clone());
                match (p, &d) {
                    (Pattern::Int(s), Value::Int(i)) => {
                        s.replace('_', "").parse::<i64>().ok() == Some(*i)
                    }
                    (Pattern::NegInt(s), Value::Int(i)) => {
                        let t = s.replace('_', "");
                        let t = if t.starts_with('-') {
                            t
                        } else {
                            format!("-{t}")
                        };
                        t.parse::<i64>().ok() == Some(*i)
                    }
                    (Pattern::Str(s), Value::Str(x)) => s.as_str() == &**x,
                    (Pattern::Bool(b), Value::Bool(x)) => b == x,
                    (Pattern::Tuple(ps), Value::Tuple(vs)) => {
                        ps.len() == vs.len()
                            && ps
                                .iter()
                                .zip(vs.iter())
                                .all(|(p, v)| self.match_pat(p, v, out))
                    }
                    (Pattern::Variant { path, args }, Value::Variant(x)) => {
                        let Some(name) = path.last() else {
                            return false;
                        };
                        if path.len() >= 2 && path[path.len() - 2] != *x.enum_name {
                            return false;
                        }
                        *name == *x.name
                            && args.len() == x.args.len()
                            && args
                                .iter()
                                .zip(x.args.iter())
                                .all(|(p, v)| self.match_pat(p, v, out))
                    }
                    _ => false,
                }
            }
        }
    }

    // --- 式 ---

    /// 式を評価する。結果が排他借用のまま返ることがある。
    fn eval(&mut self, e: &Expr) -> R<Value> {
        Ok(match e {
            Expr::Int(s) => match s.replace('_', "").parse::<i64>() {
                Ok(n) => Value::Int(n),
                Err(_) => return unsupported(format!("Int リテラル {s}")),
            },
            Expr::Float(s) => match s.replace('_', "").parse::<f64>() {
                Ok(x) => Value::Float(x),
                Err(_) => return unsupported(format!("Float リテラル {s}")),
            },
            Expr::Str(s) => Value::Str(s.as_str().into()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Name(n) => return self.eval_name(n),
            Expr::Result => match self.frames.last().and_then(|f| f.result.clone()) {
                Some(v) => v,
                None => return unsupported("result が使えない位置"),
            },
            Expr::SelfValue => match self.frames.last().and_then(|f| f.self_val.clone()) {
                Some(v) => v,
                None => return unsupported("self が使えない位置"),
            },
            Expr::Old(i) => match self
                .frames
                .last()
                .and_then(|f| f.olds.get(&old_key(i)).cloned())
            {
                Some(v) => v,
                None => return unsupported("old の値が記録されていない"),
            },
            Expr::Paren(i) => return self.eval(i),
            Expr::Tuple(es) => {
                let mut xs = vec![];
                for x in es {
                    xs.push(deref(self.eval_move(x)?));
                }
                Value::Tuple(Rc::new(xs))
            }
            Expr::Unary { op, expr } => return self.eval_unary(*op, expr),
            Expr::Binary { op, lhs, rhs } => return self.eval_binary(*op, lhs, rhs),
            Expr::Field { expr, name } => return self.eval_field(expr, name),
            Expr::Call { callee, args } => return self.eval_call(callee, args),
            Expr::Try(i) => {
                let v = self.val(i)?;
                let Value::Variant(x) = &v else {
                    return unsupported("? の対象が Option・Result でない");
                };
                match (&*x.enum_name, &*x.name) {
                    ("Option", "Some") | ("Result", "Ok") => x.args[0].clone(),
                    ("Option", "None") | ("Result", "Err") => return Err(Abort::Return(v.clone())),
                    _ => return unsupported("? の対象が Option・Result でない"),
                }
            }
            Expr::StructLit { path, fields } => {
                let Some(name) = path.last() else {
                    return unsupported("構造体リテラル");
                };
                let mut vals = vec![];
                for (k, x) in fields {
                    vals.push((k.clone(), deref(self.eval_move(x)?)));
                }
                let Some(decl) = self.prog.structs.get(name).copied() else {
                    return unsupported(format!("構造体 {name}"));
                };
                let mut ordered = vec![];
                for f in &decl.fields {
                    let Some((_, v)) = vals.iter().find(|(k, _)| *k == f.name) else {
                        return unsupported(format!("フィールド {}", f.name));
                    };
                    ordered.push((Rc::from(f.name.as_str()), v.clone()));
                }
                Value::Struct(Rc::new(StructVal {
                    name: name.as_str().into(),
                    fields: ordered,
                    logged: false,
                }))
            }
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let v = self.eval(scrutinee)?;
                for arm in arms {
                    let mut binds = vec![];
                    if self.match_pat(&arm.pattern, &v, &mut binds) {
                        self.push_scope(binds);
                        let r = self.val(&arm.body);
                        self.pop_scope();
                        return r;
                    }
                }
                return unsupported("match のどの腕にも合わない");
            }
            Expr::Closure {
                is_move,
                params,
                body,
            } => {
                let mut env: Vec<(String, Slot)> = vec![];
                if let Some(f) = self.frames.last() {
                    for sc in &f.scopes {
                        for (k, s) in sc {
                            let slot = if *is_move {
                                new_slot(s.borrow().clone())
                            } else {
                                s.clone()
                            };
                            env.push((k.clone(), slot));
                        }
                    }
                }
                Value::Closure(Rc::new(ClosureVal {
                    params: params.clone(),
                    body: body.clone(),
                    env,
                    owner: self
                        .frames
                        .last()
                        .map(|f| f.name.clone())
                        .unwrap_or_default(),
                }))
            }
        })
    }

    /// 式を評価し、排他借用を値にたどる。
    fn val(&mut self, e: &Expr) -> R<Value> {
        self.eval(e).map(deref)
    }

    fn eval_name(&mut self, n: &str) -> R<Value> {
        if let Some(s) = self.lookup(n) {
            return Ok(s.borrow().clone());
        }
        match n {
            "None" => Ok(none()),
            "Some" => Ok(Value::Ctor("Option".into(), "Some".into())),
            "Ok" => Ok(Value::Ctor("Result".into(), "Ok".into())),
            "Err" => Ok(Value::Ctor("Result".into(), "Err".into())),
            _ if self.prog.fns.contains_key(n) => Ok(Value::Named(n.into())),
            _ => unsupported(format!("名前 {n}")),
        }
    }

    fn eval_field(&mut self, expr: &Expr, name: &str) -> R<Value> {
        if let Expr::Name(n) = expr {
            if self.lookup(n).is_none() {
                if let Some(e) = self.prog.enums.get(n.as_str()).copied() {
                    let Some(v) = e.variants.iter().find(|v| v.name == name) else {
                        return unsupported(format!("{n}.{name}"));
                    };
                    return Ok(if v.fields.is_empty() {
                        variant(n, name, vec![])
                    } else {
                        Value::Ctor(n.as_str().into(), name.into())
                    });
                }
                return unsupported(format!("{n}.{name}"));
            }
        }
        match self.val(expr)? {
            Value::Struct(s) => match s.fields.iter().find(|(k, _)| &**k == name) {
                Some((_, v)) => Ok(v.clone()),
                None => unsupported(format!("フィールド {name}")),
            },
            Value::Tuple(t) => match name.parse::<usize>().ok().and_then(|i| t.get(i)) {
                Some(v) => Ok(v.clone()),
                None => unsupported(format!("組の要素 {name}")),
            },
            _ => unsupported(format!("フィールド {name} の読み出し")),
        }
    }

    fn eval_unary(&mut self, op: UnOp, expr: &Expr) -> R<Value> {
        match op {
            UnOp::Ref => self.val(expr),
            UnOp::RefMut => {
                let (slot, path) = self.resolve_place(expr)?;
                Ok(Value::MutRef(Rc::new(MutRef { slot, path })))
            }
            UnOp::Not => match self.val(expr)? {
                Value::Bool(b) => Ok(Value::Bool(!b)),
                _ => unsupported("! の対象が Bool でない"),
            },
            UnOp::Neg => match self.val(expr)? {
                Value::Int(n) => n
                    .checked_neg()
                    .map(Value::Int)
                    .ok_or_else(|| self.stop(StopKind::Overflow, "-")),
                Value::Float(x) => Ok(Value::Float(-x)),
                _ => unsupported("- の対象が数でない"),
            },
        }
    }

    fn eval_binary(&mut self, op: BinOp, lhs: &Expr, rhs: &Expr) -> R<Value> {
        match op {
            BinOp::And | BinOp::Or => {
                let Value::Bool(l) = self.val(lhs)? else {
                    return unsupported("&& / || の対象が Bool でない");
                };
                if (op == BinOp::And && !l) || (op == BinOp::Or && l) {
                    return Ok(Value::Bool(l));
                }
                match self.val(rhs)? {
                    Value::Bool(r) => Ok(Value::Bool(r)),
                    _ => unsupported("&& / || の対象が Bool でない"),
                }
            }
            _ => {
                let l = self.val(lhs)?;
                let r = self.val(rhs)?;
                self.binop(op, l, r)
            }
        }
    }

    fn binop(&self, op: BinOp, l: Value, r: Value) -> R<Value> {
        use BinOp::*;
        match op {
            Eq | Ne => {
                let e = values_eq(&l, &r);
                self.after_deep()?;
                return Ok(Value::Bool(if op == Eq { e } else { !e }));
            }
            Lt | Le | Gt | Ge => {
                let o = cmp_val(&l, &r);
                self.after_deep()?;
                return Ok(Value::Bool(match op {
                    Lt => o == Some(Ordering::Less),
                    Le => matches!(o, Some(Ordering::Less | Ordering::Equal)),
                    Gt => o == Some(Ordering::Greater),
                    _ => matches!(o, Some(Ordering::Greater | Ordering::Equal)),
                }));
            }
            _ => {}
        }
        match (l, r) {
            (Value::Int(a), Value::Int(b)) => {
                let res = match op {
                    Add => a.checked_add(b),
                    Sub => a.checked_sub(b),
                    Mul => a.checked_mul(b),
                    Div | Rem => {
                        if b == 0 {
                            return Err(self.stop(StopKind::DivisionByZero, ""));
                        }
                        if op == Div {
                            a.checked_div(b)
                        } else {
                            a.checked_rem(b)
                        }
                    }
                    _ => return unsupported("Int の演算"),
                };
                res.map(Value::Int)
                    .ok_or_else(|| self.stop(StopKind::Overflow, ""))
            }
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(match op {
                Add => a + b,
                Sub => a - b,
                Mul => a * b,
                Div => a / b,
                Rem => a % b,
                _ => return unsupported("Float の演算"),
            })),
            (Value::Str(a), Value::Str(b)) if op == Add => {
                Ok(Value::Str(format!("{a}{b}").as_str().into()))
            }
            _ => unsupported("二項演算の型"),
        }
    }

    /// 代入・排他借用の対象の置き場所（スロットと位置）。排他借用を持つ変数は参照先に進む。
    fn resolve_place(&mut self, e: &Expr) -> R<(Slot, Vec<Seg>)> {
        match e {
            Expr::Paren(i) => self.resolve_place(i),
            Expr::Name(n) => match self.lookup(n) {
                Some(s) => Ok(follow(s, vec![])),
                None => unsupported(format!("名前 {n}")),
            },
            Expr::Field { expr, name } => {
                let (s, mut p) = self.resolve_place(expr)?;
                p.push(Seg::Field(name.as_str().into()));
                Ok((s, p))
            }
            _ => {
                let v = self.val(e)?;
                Ok((new_slot(v), vec![]))
            }
        }
    }

    fn eval_args(&mut self, args: &[Expr]) -> R<Vec<Value>> {
        let mut out = vec![];
        for a in args {
            out.push(self.eval_move(a)?);
        }
        Ok(out)
    }

    fn eval_call(&mut self, callee: &Expr, args: &[Expr]) -> R<Value> {
        match callee {
            Expr::Paren(i) => self.eval_call(i, args),
            Expr::Name(n) => {
                if let Some(slot) = self.lookup(n) {
                    let f = deref(slot.borrow().clone());
                    let a = self.eval_args(args)?;
                    return self.call_value(&f, a);
                }
                let a = self.eval_args(args)?;
                self.call_named(n, a)
            }
            Expr::Field { expr, name } => {
                if let Expr::Name(ns) = &**expr {
                    if self.lookup(ns).is_none() {
                        let a = self.eval_args(args)?;
                        return self.call_static(ns, name, a);
                    }
                }
                self.call_method(expr, name, args)
            }
            other => {
                let f = self.val(other)?;
                let a = self.eval_args(args)?;
                self.call_value(&f, a)
            }
        }
    }

    fn call_named(&mut self, n: &str, mut a: Vec<Value>) -> R<Value> {
        if let Some(d) = self.prog.fns.get(n).copied() {
            return self.call_decl(d, a);
        }
        match (n, a.len()) {
            ("Some", 1) => Ok(some(deref(a.remove(0)))),
            ("Ok", 1) => Ok(ok(deref(a.remove(0)))),
            ("Err", 1) => Ok(err(deref(a.remove(0)))),
            ("to_float", 1) => match deref(a.remove(0)) {
                Value::Int(n) => Ok(Value::Float(n as f64)),
                _ => unsupported("to_float の引数"),
            },
            ("to_int", 1) => match deref(a.remove(0)) {
                Value::Float(x) => {
                    // 0 の方向に切り捨てる。NaN・無限大・Int の範囲外は None。
                    let lo = -9_223_372_036_854_775_808.0_f64;
                    let hi = 9_223_372_036_854_775_808.0_f64;
                    Ok(if x.is_finite() && x.trunc() >= lo && x.trunc() < hi {
                        some(Value::Int(x.trunc() as i64))
                    } else {
                        none()
                    })
                }
                _ => unsupported("to_int の引数"),
            },
            _ => unsupported(format!("関数 {n}")),
        }
    }

    fn call_static(&mut self, ns: &str, name: &str, mut a: Vec<Value>) -> R<Value> {
        if let Some(d) = self
            .prog
            .methods
            .get(&(ns.to_string(), name.to_string()))
            .copied()
        {
            return self.call_decl(d, a);
        }
        if let Some(t) = self.prog.refined.get(ns).copied() {
            if name == "check" && a.len() == 1 {
                return self.refine_check(t, deref(a.remove(0)));
            }
        }
        if let Some(e) = self.prog.enums.get(ns).copied() {
            if e.variants.iter().any(|v| v.name == name) {
                return Ok(variant(ns, name, a.into_iter().map(deref).collect()));
            }
        }
        match (ns, name) {
            ("console", "log") if a.len() == 1 => {
                let v = deref(a.remove(0));
                let text = v.show(false);
                self.after_deep()?;
                if self.capture {
                    self.out.push_str(&text);
                    self.out.push('\n');
                } else {
                    use std::io::Write;
                    // 標準出力が閉じていても実行は続ける（`sigil run ... | head` など）。
                    let _ = writeln!(std::io::stdout().lock(), "{text}");
                }
                Ok(Value::Unit)
            }
            ("Int", "min_value") => Ok(Value::Int(i64::MIN)),
            ("List", "new") => Ok(Value::List(Rc::new(ListVal {
                items: vec![],
                float: false,
            }))),
            ("Cell", "new") if a.len() == 1 => Ok(Value::Cell(Rc::new(std::cell::RefCell::new(
                deref(a.remove(0)),
            )))),
            // `Shared<T>` は共有の所有。値は `Rc` で共有されるので、そのまま包まずに返す。
            // どの型の値も `Value::Shared` で包む（`clone` は同じ値を指したまま。ADR 0006 の M4 の記録）。
            ("Shared", "new") if a.len() == 1 => Ok(Value::Shared(Rc::new(deref(a.remove(0))))),
            ("Path", "new") if a.len() == 1 => match deref(a.remove(0)) {
                Value::Str(x) => Ok(Value::Path(x)),
                _ => unsupported("Path.new の引数"),
            },
            ("fs", "read") if a.len() == 1 => {
                let Value::Path(p) = deref(a.remove(0)) else {
                    return unsupported("fs.read の引数");
                };
                // 差し替えの表があればそれだけを引く。なければ実ファイル。
                Ok(match &self.fs {
                    Some(files) => match files.get(&*p) {
                        Some(text) => ok(Value::Str(text.as_str().into())),
                        None => err(io_error(std::io::ErrorKind::NotFound, p)),
                    },
                    None => match std::fs::read_to_string(&*p) {
                        Ok(text) => ok(Value::Str(text.into())),
                        Err(e) => err(io_error(e.kind(), p)),
                    },
                })
            }
            ("fs", "write") if a.len() == 2 => {
                let s = deref(a.remove(1));
                let p = deref(a.remove(0));
                let (Value::Path(p), Value::Str(s)) = (p, s) else {
                    return unsupported("fs.write の引数");
                };
                Ok(match &mut self.fs {
                    Some(files) => {
                        files.insert(p.to_string(), s.to_string());
                        ok(Value::Unit)
                    }
                    None => match std::fs::write(&*p, s.as_bytes()) {
                        Ok(()) => ok(Value::Unit),
                        Err(e) => err(io_error(e.kind(), p)),
                    },
                })
            }
            _ => unsupported(format!("{ns}.{name}")),
        }
    }

    fn refine_check(&mut self, t: &'a TypeDecl, v: Value) -> R<Value> {
        let mut f = Frame::new(&t.name);
        f.self_val = Some(v.clone());
        f.scopes.push(vec![]);
        self.frames.push(f);
        let saved = self.line;
        self.line = t.span.line;
        let r = self.val(&t.cond);
        self.release_frame();
        self.line = saved;
        match r? {
            Value::Bool(true) => Ok(some(v)),
            Value::Bool(false) => Ok(none()),
            _ => unsupported("篩型の条件が Bool でない"),
        }
    }

    fn call_method(&mut self, recv: &Expr, name: &str, args: &[Expr]) -> R<Value> {
        let recv_val = self.val(recv)?;
        let place = is_place(recv);
        let user = recv_val
            .type_name()
            .and_then(|t| self.prog.methods.get(&(t, name.to_string())).copied());
        if matches!(recv, Expr::Name(_))
            && user.is_some_and(|d| {
                d.params
                    .first()
                    .is_some_and(|p| !matches!(p.ty, Type::Ref { .. }))
            })
        {
            // 値で受け取る `self` は移動する（記録中は移動元を空にする）。
            self.eval_move(recv)?;
        }
        let needs_mut = match user {
            Some(d) => matches!(
                d.params.first().map(|p| &p.ty),
                Some(Type::Ref { mutable: true, .. })
            ),
            None => matches!((&recv_val, name), (Value::List(_), "push" | "get_mut")),
        };
        let recv_arg = if needs_mut {
            let (slot, path) = if place {
                self.resolve_place(recv)?
            } else {
                (new_slot(recv_val.clone()), vec![])
            };
            Value::MutRef(Rc::new(MutRef { slot, path }))
        } else {
            recv_val
        };
        let a = self.eval_args(args)?;
        if let Some(d) = user {
            let mut all = vec![recv_arg];
            all.extend(a);
            return self.call_decl(d, all);
        }
        self.builtin_method(name, recv_arg, a)
    }

    // --- 標準ライブラリの本体（ADR 0013 の決定 2） ---

    fn builtin_method(&mut self, name: &str, recv: Value, args: Vec<Value>) -> R<Value> {
        let mut args: Vec<Value> = args
            .into_iter()
            .map(|a| match a {
                Value::MutRef(_) => deref(a),
                other => other,
            })
            .collect();
        if let Value::MutRef(m) = &recv {
            let m = m.clone();
            match name {
                "push" if args.len() == 1 => {
                    let x = args.remove(0);
                    let float = matches!(x, Value::Float(_));
                    let done = with_path(&mut m.slot.borrow_mut(), &m.path, |v| {
                        if let Value::List(l) = v {
                            let l = Rc::make_mut(l);
                            l.items.push(x);
                            l.float |= float;
                        }
                    });
                    return match done {
                        Some(()) => Ok(Value::Unit),
                        None => unsupported("push の対象"),
                    };
                }
                "get_mut" if args.len() == 1 => {
                    let cur = read_path(&m.slot.borrow(), &m.path);
                    let (Value::List(l), Value::Int(i)) = (&cur, &args[0]) else {
                        return unsupported("get_mut の対象");
                    };
                    if *i < 0 || *i as usize >= l.items.len() {
                        return Ok(none());
                    }
                    let mut path = m.path.clone();
                    path.push(Seg::Index(*i as usize));
                    return Ok(some(Value::MutRef(Rc::new(MutRef {
                        slot: m.slot.clone(),
                        path,
                    }))));
                }
                _ => {}
            }
        }
        let recv = deref(recv);
        if name == "clone" && args.is_empty() {
            return Ok(if tracing() || contains_cell(&recv) {
                deep_clone(&recv)
            } else {
                recv
            });
        }
        match &recv {
            Value::List(l) => self.list_method(name, l, args),
            Value::Str(s) => str_method(name, s, args),
            Value::Path(p) => path_method(name, p, args),
            Value::Shared(inner) => self.builtin_method(name, (**inner).clone(), args),
            Value::Cell(c) => match (name, args.len()) {
                ("get", 0) => Ok(c.borrow().clone()),
                ("set", 1) => {
                    let new = args.into_iter().next().map(deref).unwrap_or(Value::Unit);
                    // 古い値は借用を外してから破棄する（破棄が借用中に走らない）。
                    let old = std::mem::replace(&mut *c.borrow_mut(), new);
                    drop(old);
                    Ok(Value::Unit)
                }
                _ => unsupported(format!("Cell.{name}")),
            },
            Value::Variant(v) if &*v.enum_name == "Option" => self.option_method(name, v, args),
            Value::Variant(v) if &*v.enum_name == "Result" => self.result_method(name, v, args),
            _ => unsupported(format!("メソッド {name}")),
        }
    }

    fn list_method(&mut self, name: &str, l: &Rc<ListVal>, mut a: Vec<Value>) -> R<Value> {
        let items = &l.items;
        match (name, a.len()) {
            ("len", 0) => Ok(Value::Int(items.len() as i64)),
            ("is_empty", 0) => Ok(Value::Bool(items.is_empty())),
            ("get", 1) => match &a[0] {
                Value::Int(i) if *i >= 0 => {
                    Ok(items.get(*i as usize).cloned().map_or_else(none, some))
                }
                Value::Int(_) => Ok(none()),
                _ => unsupported("get の引数"),
            },
            ("map", 1) => {
                let f = a.remove(0);
                let mut out = vec![];
                for x in items {
                    out.push(self.call_value(&f, vec![x.clone()])?);
                }
                let float = matches!(out.first(), Some(Value::Float(_)));
                Ok(Value::List(Rc::new(ListVal { items: out, float })))
            }
            ("find", 1) => {
                let f = a.remove(0);
                for x in items {
                    if let Value::Bool(true) = self.call_value(&f, vec![x.clone()])? {
                        return Ok(some(x.clone()));
                    }
                }
                Ok(none())
            }
            ("max_by" | "min_by", 1) => {
                let f = a.remove(0);
                let mut best: Option<(Value, Value)> = None;
                for x in items {
                    let k = self.call_value(&f, vec![x.clone()])?;
                    let better = match &best {
                        None => true,
                        Some((bk, _)) => {
                            let o = cmp_val(&k, bk);
                            self.after_deep()?;
                            if name == "max_by" {
                                o == Some(Ordering::Greater)
                            } else {
                                o == Some(Ordering::Less)
                            }
                        }
                    };
                    if better {
                        best = Some((k, x.clone()));
                    }
                }
                Ok(best.map_or_else(none, |(_, x)| some(x)))
            }
            ("sum", 0) => {
                if items.is_empty() {
                    return Ok(if l.float {
                        Value::Float(0.0)
                    } else {
                        Value::Int(0)
                    });
                }
                let mut acc = items[0].clone();
                for x in &items[1..] {
                    acc = self.binop(BinOp::Add, acc, x.clone())?;
                }
                Ok(acc)
            }
            ("min" | "max", 0) => {
                if items.is_empty() {
                    return Err(self.stop(
                        StopKind::StdRequiresViolated,
                        &format!("List.{name} は self.len() > 0 を要求する"),
                    ));
                }
                let mut best = &items[0];
                for x in &items[1..] {
                    let o = cmp_val(x, best);
                    self.after_deep()?;
                    let better = if name == "min" {
                        o == Some(Ordering::Less)
                    } else {
                        o == Some(Ordering::Greater)
                    };
                    if better {
                        best = x;
                    }
                }
                Ok(best.clone())
            }
            _ => unsupported(format!("List.{name}")),
        }
    }

    fn option_method(&mut self, name: &str, v: &VariantVal, mut a: Vec<Value>) -> R<Value> {
        let is_some = &*v.name == "Some";
        match (name, a.len()) {
            ("is_some", 0) => Ok(Value::Bool(is_some)),
            ("is_none", 0) => Ok(Value::Bool(!is_some)),
            ("unwrap_or", 1) => Ok(if is_some {
                v.args[0].clone()
            } else {
                a.remove(0)
            }),
            ("map", 1) => {
                if is_some {
                    let f = a.remove(0);
                    Ok(some(self.call_value(&f, vec![v.args[0].clone()])?))
                } else {
                    Ok(none())
                }
            }
            ("ok_or", 1) => Ok(if is_some {
                ok(v.args[0].clone())
            } else {
                err(a.remove(0))
            }),
            _ => unsupported(format!("Option.{name}")),
        }
    }

    fn result_method(&mut self, name: &str, v: &VariantVal, mut a: Vec<Value>) -> R<Value> {
        let is_ok = &*v.name == "Ok";
        match (name, a.len()) {
            ("is_ok", 0) => Ok(Value::Bool(is_ok)),
            ("is_err", 0) => Ok(Value::Bool(!is_ok)),
            ("unwrap_or", 1) => Ok(if is_ok {
                v.args[0].clone()
            } else {
                a.remove(0)
            }),
            ("ok", 0) => Ok(if is_ok {
                some(v.args[0].clone())
            } else {
                none()
            }),
            ("map", 1) => {
                if is_ok {
                    let f = a.remove(0);
                    Ok(ok(self.call_value(&f, vec![v.args[0].clone()])?))
                } else {
                    Ok(err(v.args[0].clone()))
                }
            }
            ("map_err", 1) => {
                if is_ok {
                    Ok(ok(v.args[0].clone()))
                } else {
                    let f = a.remove(0);
                    Ok(err(self.call_value(&f, vec![v.args[0].clone()])?))
                }
            }
            _ => unsupported(format!("Result.{name}")),
        }
    }
}

/// `Path` のメソッド。意味は `std::path::Path` に従う（実行環境の OS の区切りを使う。ADR 0006 の M4 の記録）。
fn path_method(name: &str, p: &Rc<str>, a: Vec<Value>) -> R<Value> {
    let std_path = std::path::Path::new(&**p);
    let text = |x: &std::path::Path| -> Rc<str> { x.to_string_lossy().as_ref().into() };
    match (name, a.as_slice()) {
        ("is_empty", []) => Ok(Value::Bool(p.is_empty())),
        ("join", [Value::Str(other)]) => Ok(Value::Path(text(&std_path.join(&**other)))),
        ("file_name", []) => Ok(std_path
            .file_name()
            .map_or_else(none, |n| some(Value::Str(text(n.as_ref()))))),
        ("parent", []) => Ok(std_path
            .parent()
            .map_or_else(none, |n| some(Value::Path(text(n))))),
        _ => unsupported(format!("Path.{name}")),
    }
}

fn str_method(name: &str, s: &Rc<str>, a: Vec<Value>) -> R<Value> {
    if !a.is_empty() {
        return unsupported(format!("String.{name}"));
    }
    match name {
        "len" => Ok(Value::Int(s.chars().count() as i64)),
        "is_empty" => Ok(Value::Bool(s.is_empty())),
        "trim" => Ok(Value::Str(s.trim().into())),
        "lines" => {
            let mut parts: Vec<&str> = s.split('\n').collect();
            if parts.last() == Some(&"") {
                parts.pop();
            }
            let items = parts
                .into_iter()
                .map(|l| Value::Str(l.strip_suffix('\r').unwrap_or(l).into()))
                .collect();
            Ok(Value::List(Rc::new(ListVal {
                items,
                float: false,
            })))
        }
        "parse_int" => {
            let digits = s.strip_prefix('-').unwrap_or(s);
            let valid = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
            match valid.then(|| s.parse::<i64>().ok()).flatten() {
                Some(n) => Ok(ok(Value::Int(n))),
                None => Ok(err(Value::ParseError)),
            }
        }
        _ => unsupported(format!("String.{name}")),
    }
}

//! 実行の入出力の型と、ソースの合わせ方（ADR 0018 の PR 1-5。インタプリタの削除で `interp` から残した部分）。
//!
//! 実行そのものは SIR の評価器（`ir_eval`）とネイティブの実行ファイルが行う。ここには、両方が共有する
//! 異常停止の報告（`Stop`・`StopKind`・`Clause`）、実行の結果（`Outcome`・`Failure`・`RunError`）、
//! 実行の設定（`Options`）、複数のソースを 1 つにまとめる処理を置く。

use crate::ast::*;
use crate::error::Error;
use std::fmt;

/// 異常停止のときのプロセスの終了コード（種類によらず 1 つ。`EX_SOFTWARE`）。ADR 0006 の M3 の記録を参照。
pub const STOP_EXIT_CODE: u8 = 70;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        let during = self
            .during
            .as_ref()
            .map(|(func, clause)| (func.as_str(), *clause));
        f.write_str(&crate::report::stop_message(
            self.kind,
            &self.func,
            self.line,
            during,
            &self.detail,
        ))
    }
}

pub use crate::shared_args::Arg;
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
    pub trace_drops: bool,
    /// `fs.read` のテスト用の差し替え（ADR 0004 の規則 7）。`Some` のとき、`fs.read(&p)` は
    /// この表（パス -> 内容）を引き、ない `p` は `Err(IoError.NotFound(p))` を返す。`fs.write` は表を書き換える。
    /// `None`（既定）のときは実際のファイルを読み書きする。
    pub fs_files: Option<std::collections::BTreeMap<String, String>>,
    /// `console.log` の出力を `Outcome::output` に溜める（テスト用）。偽（既定）なら本物の標準出力へ書き、`output` は空。
    pub capture_console: bool,
    /// `trust base` の宣言を許すモジュール名（`--trust-base`。ADR 0016）。
    pub trust_base: Vec<String>,
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
            trust_base: Vec::new(),
        }
    }
}

pub(crate) fn fail(error: RunError) -> Failure {
    Failure {
        error,
        output: String::new(),
        drops: Box::default(),
    }
}

/// `run`・`build` 用。`use` を含むソースは、複数モジュールの実行が入る（2-2c）まで明示のエラーで拒否する
/// （黙って無視すると、取り込んだ名前が解決されないまま 1 モジュールだけが動く。ADR 0021 の決定 7）。
pub(crate) fn merge_sources_for_run(srcs: &[&str]) -> Result<File, Failure> {
    let f = merge_sources(srcs)?;
    if let Some(u) = f.uses.first() {
        return Err(fail(RunError::Check(crate::Error::new(
            crate::ErrorKind::Unsupported,
            u.span,
            "`use` を含むプログラムの実行・組み立ては未対応（複数のモジュールの実行は 2-2c で入る）",
        ))));
    }
    Ok(f)
}

/// 複数のソースを 1 つのファイルに合わせる（モジュールの宣言がすべて同じことを求める）。
pub(crate) fn merge_sources(srcs: &[&str]) -> Result<File, Failure> {
    let mut module = None;
    let mut items = vec![];
    let mut uses = vec![];
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
        uses.extend(f.uses);
        items.extend(f.items);
    }
    let Some(module) = module else {
        return Err(fail(RunError::Entry("ソースがない".into())));
    };
    Ok(File {
        module,
        uses,
        items,
    })
}

/// 検査器に渡す設定（`--trust-base` を載せる）。
pub(crate) fn check_options(opts: &Options) -> crate::effects::Options {
    crate::effects::Options {
        trust_base: opts.trust_base.clone(),
        ..Default::default()
    }
}

/// `ensures` の中の `old(e)` の `e`（捕捉する式）を集める。
pub(crate) fn collect_olds<'e>(e: &'e Expr, out: &mut Vec<&'e Expr>) {
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

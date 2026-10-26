//! SIR（Sigil の中間表現。ADR 0015 の決定 2）。
//!
//! 単相化した関数ごとの制御流れ図。所有権（移動・破棄）、契約の検査、異常停止の経路を、評価器（`ir_eval`）と
//! 後続のネイティブ出力が同じ形で読めるよう、明示的に持つ。
//!
//! 形（ADR 0015 の決定 2 が定めるもの）:
//! - `Program{funcs, types, strings, stop_sites, entry, core_only}`、`Func`、`Block{stmts, term}`、`Ty`、`Stmt`、`Term`。
//! - 停止しうる操作（`CheckedOp`・`Assert`・`Call`）は `cleanup` の基本ブロックを持つ。`cleanup` は、その時点で生きている
//!   値を内側のスコープから外側へ（スコープの中は宣言の逆順に）破棄して `Resume` で呼び出し元の `cleanup` へ戻る。
//!   停止も `?` の早期の return も、同じ破棄の並びを通る（決定 5）。
//!
//! ADR が形を定めていない部分（`Place`・`Rvalue`・`Operand`・`Callee`・`Const`）はこの版で決めたもの:
//! - `Operand::Move` は「読んで元を空にする」（移動元は呼び出しの前に空になるので、呼ばれた側が停止しても二重に破棄しない）。
//! - 空の場所の `Drop` は何もしない。破棄の旗は実行時の「空」の印で持ち、`cleanup` は登録された局所すべてを破棄する。
//! - 型ごとの破棄は評価器が値の形から行う（`Ref`・`MutRef` は何も破棄しない）。

use crate::interp::{Clause, StopKind};
use std::fmt;

pub type LocalId = usize;
pub type BlockId = usize;
pub type FuncId = usize;
pub type TypeId = usize;
pub type StrId = usize;
pub type StopId = usize;

/// 標準の列挙型の腕の番号（宣言の順）。評価器と lowering が同じ番号を使う。
pub const OPTION_SOME: usize = 0;
pub const OPTION_NONE: usize = 1;
pub const RESULT_OK: usize = 0;
pub const RESULT_ERR: usize = 1;
pub const IO_NOT_FOUND: usize = 0;
pub const IO_PERMISSION_DENIED: usize = 1;
pub const IO_OTHER: usize = 2;

#[derive(Debug, Clone)]
pub struct Program {
    pub funcs: Vec<Func>,
    pub types: Vec<TypeDef>,
    pub strings: Vec<String>,
    pub stop_sites: Vec<StopSite>,
    pub entry: FuncId,
    /// OS 層（`console`・`fs`・`Path`）を使わない（`bare` の対象にできる）。
    pub core_only: bool,
    /// 入口の関数が戻ったあとの値の表示で停止したときの行（入口の関数の宣言の行）。
    pub exit_line: u32,
}

#[derive(Debug, Clone)]
pub struct Func {
    /// 報告に出す関数名（クロージャは持ち主の関数、篩型の `check` は型の名前）。
    pub name: String,
    pub params: Vec<LocalId>,
    pub ret: Ty,
    pub locals: Vec<Local>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub term: Term,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    Float,
    Bool,
    Unit,
    Str,
    Path,
    ParseError,
    List(Box<Ty>),
    Tuple(Vec<Ty>),
    Adt(TypeId),
    Ref(Box<Ty>),
    MutRef(Box<Ty>),
    Shared(Box<Ty>),
    Cell(Box<Ty>),
    FnRef(Vec<Ty>, Box<Ty>),
}

#[derive(Debug, Clone)]
pub struct TypeDef {
    pub name: String,
    pub kind: TypeKind,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    Struct { fields: Vec<(String, Ty)> },
    Enum { variants: Vec<VariantDef> },
}

#[derive(Debug, Clone)]
pub struct VariantDef {
    pub name: String,
    pub fields: Vec<Ty>,
    /// 値が自分自身を含む腕（箱に入れる）。
    pub indirect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StopSite {
    pub kind: StopKind,
    pub func: String,
    pub line: u32,
    /// 契約の式の評価中の停止なら、その契約を持つ関数と節。
    pub during: Option<(String, Clause)>,
    pub detail: String,
}

impl StopSite {
    /// 報告の文字列（`report::stop_message`）。
    pub fn message(&self) -> String {
        crate::report::stop_message(
            self.kind,
            &self.func,
            self.line,
            self.during.as_ref().map(|(f, c)| (f.as_str(), *c)),
            &self.detail,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Place {
    pub base: LocalId,
    pub proj: Vec<Proj>,
}

impl Place {
    pub fn local(base: LocalId) -> Place {
        Place { base, proj: vec![] }
    }

    pub fn field(&self, i: usize) -> Place {
        let mut p = self.clone();
        p.proj.push(Proj::Field(i));
        p
    }

    pub fn downcast(&self, v: usize) -> Place {
        let mut p = self.clone();
        p.proj.push(Proj::Downcast(v));
        p
    }

    pub fn deref(&self) -> Place {
        let mut p = self.clone();
        p.proj.push(Proj::Deref);
        p
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proj {
    /// 構造体・組・列挙型の腕の値・一覧の要素の位置。
    Field(usize),
    /// 列挙型の値を、その腕の中身として見る（腕の番号）。
    Downcast(usize),
    /// 参照の指す先。
    Deref,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(StrId),
    /// 環境を持たない関数の値。
    Fn(FuncId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    /// 複製（`Copyable` な値）または参照の写し。
    Copy(Place),
    /// 読んで、元を空にする。
    Move(Place),
    Const(Const),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// 停止しない 2 項演算（比較は構造的。`Add` は `Float` か `String`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    NegFloat,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AggKind {
    Tuple,
    Struct(TypeId),
    Variant(TypeId, usize),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Rvalue {
    Use(Operand),
    Unary(UnOp, Operand),
    Binary(BinOp, Operand, Operand),
    Aggregate(AggKind, Vec<Operand>),
    /// 共有借用（元の場所の読み取り専用の見え方）。
    Ref(Place),
    /// 排他借用。
    RefMut(Place),
    /// 具体的な型での `clone`。
    Clone(Operand, Ty),
    /// 列挙型の値の腕の番号（`Switch` に渡す）。
    Discriminant(Operand),
    /// 環境つきの関数の値。環境は組（捕捉した値の並び）で、関数の第 1 引数（環境への参照）に渡る。
    MakeClosure(FuncId, Vec<Operand>),
}

/// 標準ライブラリの本体を持つ操作。`Call` の `Callee::Builtin` は関数の値を呼びうるもの（`map` など）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    ListNew,
    ListLen,
    ListIsEmpty,
    ListGet,
    ListGetMut,
    ListPush,
    ListSumInt,
    ListSumFloat,
    ListMin,
    ListMax,
    ListMap,
    ListFind,
    ListMaxBy,
    ListMinBy,
    OptIsSome,
    OptIsNone,
    OptUnwrapOr,
    OptMap,
    OptOkOr,
    ResIsOk,
    ResIsErr,
    ResUnwrapOr,
    ResOk,
    ResMap,
    ResMapErr,
    StrLen,
    StrIsEmpty,
    StrTrim,
    StrLines,
    StrParseInt,
    PathNew,
    PathIsEmpty,
    PathJoin,
    PathFileName,
    PathParent,
    CellNew,
    SharedNew,
    /// `Shared<Cell<T>>` の `get`・`set`。
    SharedGet,
    SharedSet,
    IntMinValue,
    ToFloat,
    ToInt,
    ConsoleLog,
    FsRead,
    FsWrite,
    /// 組・一覧・構造体・列挙型の比較（深い入れ子で止まりうるので `Call` を通す）。
    Compare(BinOp),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Callee {
    Func(FuncId),
    /// 関数の値（クロージャ・関数名）の呼び出し。
    Indirect(Operand),
    Builtin(Builtin),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    Assign(Place, Rvalue),
    /// 場所の値を破棄する（空なら何もしない）。破棄した場所は空になる。
    Drop(Place),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Goto(BlockId),
    Branch(Operand, BlockId, BlockId),
    Switch(Operand, Vec<(i64, BlockId)>, BlockId),
    Call {
        func: Callee,
        args: Vec<Operand>,
        dest: Place,
        next: BlockId,
        cleanup: BlockId,
        /// スタックの枯渇の地点（呼び出し側の関数と行）。
        stop: StopId,
    },
    CheckedOp {
        op: ArithOp,
        a: Operand,
        b: Operand,
        dest: Place,
        next: BlockId,
        stop: StopId,
        cleanup: BlockId,
    },
    Assert {
        cond: Operand,
        stop: StopId,
        next: BlockId,
        cleanup: BlockId,
    },
    Return,
    /// 停止の巻き戻しを呼び出し元の `cleanup` へ続ける。
    Resume,
}

// ---------------------------------------------------------------------------
// 表示（デバッグ用）

impl fmt::Display for Place {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "_{}", self.base)?;
        for p in &self.proj {
            match p {
                Proj::Field(i) => write!(f, ".{i}")?,
                Proj::Downcast(v) => write!(f, " as v{v}")?,
                Proj::Deref => write!(f, ".*")?,
            }
        }
        Ok(())
    }
}

impl fmt::Display for Program {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, func) in self.funcs.iter().enumerate() {
            writeln!(
                f,
                "fn #{i} {} params={:?} ret={:?}",
                func.name, func.params, func.ret
            )?;
            for (j, l) in func.locals.iter().enumerate() {
                writeln!(f, "  let _{j}: {:?} // {}", l.ty, l.name)?;
            }
            for (j, b) in func.blocks.iter().enumerate() {
                writeln!(f, " bb{j}:")?;
                for s in &b.stmts {
                    match s {
                        Stmt::Assign(p, r) => writeln!(f, "    {p} = {r:?}")?,
                        Stmt::Drop(p) => writeln!(f, "    drop {p}")?,
                    }
                }
                writeln!(f, "    {:?}", b.term)?;
            }
        }
        Ok(())
    }
}

//! 構文木（ADR 0009 の文法の要約に対応）。
//!
//! 整形器が括弧の要否を判定できるよう、括弧は `Expr::Paren` として残す。

use crate::error::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub module: ModuleDecl,
    /// `use a.b.name;`（ADR 0021 の決定 3。`module` の宣言の直後に 1 名ずつ、文字の順）。
    pub uses: Vec<UseDecl>,
    pub items: Vec<Item>,
}

/// `use app.http.server.handle;`。`path` の最後が使う名前、手前がモジュール名。
#[derive(Debug, Clone, PartialEq)]
pub struct UseDecl {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModuleDecl {
    /// `examples.average` -> ["examples", "average"]
    pub name: Vec<String>,
    pub targets: Vec<String>,
    pub trust: Trust,
    pub span: Span,
    /// `targets`・`trust` の節を書いた行（整形器がコメントを置くため）。
    pub targets_line: u32,
    pub trust_line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    None,
    Base,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Type(TypeDecl),
    Struct(StructDecl),
    Enum(EnumDecl),
    Fn(FnDecl),
    Impl(ImplDecl),
    Trait(TraitDecl),
    ImplFor(ImplForDecl),
}

/// `impl 型 { fn ... }`（ADR 0011 の決定 6。型に直接付けるメソッド）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImplDecl {
    pub generics: Vec<Generic>,
    pub ty: Type,
    pub fns: Vec<FnDecl>,
    pub span: Span,
    /// 閉じ波括弧の行。
    pub end_line: u32,
}

/// `trait 名前 { fn ...; }`（ADR 0022 の決定 2。PR 2-4b は構文・整形・仕様の表示まで）。
/// トレイトの関数は本体を持たない。`fns` の各 `FnDecl` の `body` は空で、
/// `body.open_line`・`body.end_line` は `effects` 節の後の `;` の行を指す。
#[derive(Debug, Clone, PartialEq)]
pub struct TraitDecl {
    /// `pub` を付けた宣言（ADR 0021 の決定 4。トレイトの関数はすべて公開される）。
    pub is_pub: bool,
    pub name: String,
    pub fns: Vec<FnDecl>,
    pub span: Span,
    /// 閉じ波括弧の行。
    pub end_line: u32,
}

/// `impl トレイト for 型 { fn ... }`（ADR 0022 の決定 2）。
#[derive(Debug, Clone, PartialEq)]
pub struct ImplForDecl {
    pub generics: Vec<Generic>,
    /// 実装するトレイトの名前（修飾しない名前 1 つ。書いたまま）。
    pub trait_name: String,
    pub ty: Type,
    pub fns: Vec<FnDecl>,
    pub span: Span,
    /// 閉じ波括弧の行。
    pub end_line: u32,
}

/// 篩型 `type Port = Int where 条件;`
#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    /// `pub` を付けた宣言（ADR 0021 の決定 4。付けなければ非公開）。
    pub is_pub: bool,
    pub name: String,
    pub base: Type,
    pub cond: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Generic {
    pub name: String,
    /// `T: Equatable + Sendable` の制約（`+` でつないだ性質の名前。書かれた順）。
    pub bounds: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructDecl {
    /// `pub` を付けた宣言（ADR 0021 の決定 4。付けなければ非公開）。
    pub is_pub: bool,
    pub name: String,
    pub generics: Vec<Generic>,
    pub fields: Vec<Field>,
    pub span: Span,
    /// 閉じ波括弧の行。
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub name: String,
    pub ty: Type,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumDecl {
    /// `pub` を付けた宣言（ADR 0021 の決定 4。付けなければ非公開）。
    pub is_pub: bool,
    pub name: String,
    pub generics: Vec<Generic>,
    pub variants: Vec<Variant>,
    pub span: Span,
    /// 閉じ波括弧の行。
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Type>,
    pub line: u32,
}

/// `FnDecl::clause_lines` の添字（各節の行）。
pub mod clause {
    pub const INTENT: usize = 0;
    pub const REQUIRES: usize = 1;
    pub const ENSURES: usize = 2;
    pub const EFFECTS: usize = 3;
    pub const UNCHECKED: usize = 4;
    pub const COUNT: usize = 5;
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    /// `pub` を付けた宣言（ADR 0021 の決定 4。付けなければ非公開）。
    pub is_pub: bool,
    pub name: String,
    pub generics: Vec<Generic>,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub intent: String,
    /// `none` は `None`。
    pub requires: Option<Expr>,
    pub ensures: Option<Expr>,
    /// `none` は空。
    pub effects: Vec<Effect>,
    /// `unchecked "前提"` の節（ADR 0016 の決定 2）。省けて、書かないことは「原始操作を呼ばない」を意味する。
    pub unchecked: Option<String>,
    pub body: Block,
    pub span: Span,
    /// `intent`・`requires`・`ensures`・`effects`・`unchecked` の各節の行（`unchecked` がなければ 0）。
    pub clause_lines: [u32; clause::COUNT],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// `of(f)`
    Of(String),
    /// `fs.read`・`console` などのカテゴリ。
    Category(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    Path {
        path: Vec<String>,
        args: Vec<Type>,
    },
    Ref {
        mutable: bool,
        inner: Box<Type>,
    },
    Fn {
        params: Vec<Type>,
        ret: Option<Box<Type>>,
        /// 戻り値の後に書かれた効果節（ADR 0022 の決定 3。PR 2-4d の 1/3 で構文だけ）。
        /// `None` は書かれていない（引数の位置・従来形）。`Some` の空は `effects none`、
        /// それ以外は `effects カテゴリ, ...`（`of(...)` は構文で弾くので入らない）。
        /// 位置による必須・禁止の検査と、型の同一性への反映は PR 2-4d の 2/3（型検査）。
        effects: Option<Vec<Effect>>,
    },
    Tuple(Vec<Type>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    /// 開き波括弧の行。
    pub open_line: u32,
    /// 閉じ波括弧の行（整形器が空行・コメントを扱うため）。
    pub end_line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    Let {
        mutable: bool,
        /// 束縛・`_`・組の入れ子だけ（ADR 0011 の決定 7）。
        pattern: Pattern,
        /// 型注釈（`let x: T = ...`）。
        ty: Option<Type>,
        value: Expr,
    },
    Assign {
        target: Expr,
        value: Expr,
    },
    /// 呼び出し（末尾の `?` を含む）だけ。
    Expr(Expr),
    Return(Option<Expr>),
    If(IfStmt),
    While {
        cond: Expr,
        body: Block,
    },
    /// 文の `match`（腕はブロック）。
    Match {
        scrutinee: Expr,
        arms: Vec<StmtArm>,
        /// 閉じ波括弧の行。
        end_line: u32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct IfStmt {
    /// `if` の行。
    pub line: u32,
    pub cond: Expr,
    pub then: Block,
    pub otherwise: Option<Box<Else>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Else {
    If(IfStmt),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StmtArm {
    pub pattern: Pattern,
    pub body: Block,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExprArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wild,
    Bind(String),
    Int(String),
    NegInt(String),
    Str(String),
    Bool(bool),
    /// `Some(p)`・`None`・`ConfigError.Io(e)`
    Variant {
        path: Vec<String>,
        args: Vec<Pattern>,
    },
    Tuple(Vec<Pattern>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
    Ref,
    RefMut,
    BitNot,
}

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
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClosureBody {
    Expr(Box<Expr>),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int(String),
    Float(String),
    Str(String),
    Bool(bool),
    Name(String),
    /// `result`（`ensures` の中だけ）
    Result,
    /// `self`（篩型の条件の中だけ）
    SelfValue,
    /// `old(式)`（`ensures` の中だけ）
    Old(Box<Expr>),
    Paren(Box<Expr>),
    Tuple(Vec<Expr>),
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Field {
        expr: Box<Expr>,
        name: String,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
    },
    Try(Box<Expr>),
    StructLit {
        path: Vec<String>,
        fields: Vec<(String, Expr)>,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<ExprArm>,
        /// 閉じ波括弧の行。
        end_line: u32,
    },
    Closure {
        is_move: bool,
        params: Vec<String>,
        body: ClosureBody,
    },
}

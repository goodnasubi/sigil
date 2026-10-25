//! 診断（字句エラー・構文エラー・型エラー）。M0 では最初の 1 件で打ち切る。

use std::fmt;

/// ソース上の位置（1 始まり。列は文字数で数える）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Lex,
    Syntax,
    Fmt,
    Check,
}

/// エラーの種類。テストと将来の機械可読な出力のために安定した名前を持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    // --- 字句エラー（ADR 0009 の決定 1〜6） ---
    Bom,
    CarriageReturn,
    Tab,
    NonAsciiOutsideLiteral,
    BlockComment,
    UnexpectedChar,
    UnderscoreName,
    FutureReservedWord,
    CharLiteral,
    UnterminatedString,
    NewlineInString,
    BadEscape,
    LeadingZero,
    BadNumber,
    CompoundAssign,
    RangeOperator,
    PathSeparator,
    // --- 構文エラー ---
    Unexpected,
    ReservedWordAsName,
    BadNameForm,
    ReservedNameDefined,
    ModuleMissing,
    ModuleDuplicate,
    ModuleClauseMissing,
    ModuleClauseOrder,
    UnknownTarget,
    TargetsDuplicate,
    TargetsUnsorted,
    MissingClause,
    ClauseOrder,
    EmptyIntent,
    ComparisonChain,
    MixedAndOr,
    RefOfRef,
    ExprStmtNotCall,
    InvalidAssignTarget,
    StandaloneBlock,
    OldOutsideEnsures,
    ResultOutsideEnsures,
    SelfOutsideRefinement,
    ValueMatchArmBlock,
    StmtMatchArmNotBlock,
    StmtMatchArmComma,
    EmptyEnum,
    EmptyMatch,
    EmptyStructLiteral,
    TypeAliasWithoutWhere,
    BadTuple,
    NotInP0,
    TooDeep,
    // --- 型検査（ADR 0011） ---
    /// PR1 の範囲外の構文・型（総称・標準ライブラリの型・`match`・クロージャ・`?`）。
    Unsupported,
    UnknownName,
    UnknownType,
    DuplicateDefinition,
    TypeMismatch,
    OperandTypes,
    NotEquatable,
    NotComparable,
    ArgCount,
    NotCallable,
    NoSuchMember,
    FieldSet,
    IntLiteralRange,
    AssignToImmutable,
    AssignThroughSharedRef,
    BorrowMutOfImmutable,
    BorrowOfNonPlace,
    LetMutRef,
    RedundantAnnotation,
    OldNotCopyable,
    MissingReturn,
    FnTypePosition,
    RecursiveType,
    RefinementBase,
    RefinementName,
    RefinementEffect,
    DestructureMismatch,
    MemberConflict,
    ShadowedName,
    ConstraintUnsatisfied,
    UnknownProperty,
    TypeParamBound,
    TypeArgCount,
    CannotInfer,
    NotExhaustive,
    UnreachableArm,
    TryOperand,
    UnknownEffect,
    OfNotParam,
    EffectMissing,
    EffectExtra,
    EffectTarget,
    GlobalValue,
    PatternMismatch,
    InfiniteType,
    // --- 整形 ---
    /// 整形結果の検証に失敗した（整形器の不具合か、深さの上限を超える出力）。
    FmtVerify,
}

impl ErrorKind {
    pub fn phase(&self) -> Phase {
        use ErrorKind::*;
        match self {
            Bom
            | CarriageReturn
            | Tab
            | NonAsciiOutsideLiteral
            | BlockComment
            | UnexpectedChar
            | UnderscoreName
            | FutureReservedWord
            | CharLiteral
            | UnterminatedString
            | NewlineInString
            | BadEscape
            | LeadingZero
            | BadNumber
            | CompoundAssign
            | RangeOperator
            | PathSeparator => Phase::Lex,
            FmtVerify => Phase::Fmt,
            Unsupported
            | UnknownName
            | UnknownType
            | DuplicateDefinition
            | TypeMismatch
            | OperandTypes
            | NotEquatable
            | NotComparable
            | ArgCount
            | NotCallable
            | NoSuchMember
            | FieldSet
            | IntLiteralRange
            | AssignToImmutable
            | AssignThroughSharedRef
            | BorrowMutOfImmutable
            | BorrowOfNonPlace
            | LetMutRef
            | RedundantAnnotation
            | OldNotCopyable
            | MissingReturn
            | FnTypePosition
            | RecursiveType
            | RefinementBase
            | RefinementName
            | RefinementEffect
            | DestructureMismatch
            | MemberConflict
            | ShadowedName
            | ConstraintUnsatisfied
            | UnknownProperty
            | TypeParamBound
            | TypeArgCount
            | CannotInfer
            | NotExhaustive
            | UnreachableArm
            | UnknownEffect
            | OfNotParam
            | EffectMissing
            | EffectExtra
            | EffectTarget
            | TryOperand
            | PatternMismatch
            | InfiniteType => Phase::Check,
            _ => Phase::Syntax,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub span: Span,
}

impl Error {
    pub fn new(kind: ErrorKind, span: Span, message: impl Into<String>) -> Self {
        Error {
            kind,
            message: message.into(),
            span,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = match self.kind.phase() {
            Phase::Lex => "字句エラー",
            Phase::Syntax => "構文エラー",
            Phase::Fmt => "整形エラー",
            Phase::Check => "型エラー",
        };
        write!(
            f,
            "{}:{}: {}: {}",
            self.span.line, self.span.col, phase, self.message
        )
    }
}

impl std::error::Error for Error {}

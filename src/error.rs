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
    /// `use` が `module` の宣言の直後以外にある（ADR 0021 の決定 3）。
    UseMisplaced,
    /// `use` の形が違う（モジュール名.名前 の 1 名でない。まとめ書き・ワイルドカード・別名）。
    UseForm,
    UseDuplicate,
    UseUnsorted,
    /// `pub` を付けられない宣言に付けた。
    PubTarget,
    ModuleClauseMissing,
    ModuleClauseOrder,
    UnknownTarget,
    TargetsDuplicate,
    TargetsUnsorted,
    MissingClause,
    ClauseOrder,
    EmptyIntent,
    EmptyUnchecked,
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
    OsItemInCore,
    /// 信頼基盤でないモジュールの関数に `unchecked` の節がある（ADR 0016 の決定 2）。
    UncheckedOutsideTrustBase,
    /// `trust base` を宣言したが、許可（`--trust-base`）がない（ADR 0016 の決定 3）。
    TrustBaseNotAllowed,
    /// 利用者のモジュールの名前が `std`・`std.` で始まる（ADR 0016 の決定 3）。
    ReservedModuleName,
    /// `unchecked` の関数の本体の外（クロージャ・契約・関数の値・節のない関数）で原始操作を使った（ADR 0016 の決定 2）。
    RawOutsideUnchecked,
    /// ファイルのパス（`--root` から）から決まるモジュール名と、`module` の宣言が違う。使えない文字・`..`・根の外もこれ（ADR 0021 の決定 1）。
    ModulePathMismatch,
    /// 大小だけが違う名前のファイルが並ぶ（大小を区別しないファイルシステムで指す先が環境で変わる。ADR 0021 の決定 1）。
    ModulePathCase,
    /// `use` の先のモジュールのファイルがない・その名前がない（ADR 0021 の決定 3）。
    UnresolvedUse,
    /// 非公開の宣言を `use` で取り込んだ（ADR 0021 の決定 4）。
    PrivateImport,
    /// 取り込んだ名前が、自分の宣言・別の取り込み・標準の名前と衝突する（ADR 0021 の決定 3）。
    UseConflict,
    /// 取り込んだのに使っていない名前（ADR 0021 の決定 3）。
    UnusedUse,
    /// モジュールの参照の輪（ADR 0021 の決定 3）。
    ModuleCycle,
    /// `use` の連鎖が深すぎる・辿るファイルが多すぎる（資源の上限。ADR 0021 2-2d 追補）。
    ModuleLimit,
    /// 取り込む側の `targets` が、取り込まれる側の `targets` に含まれない（ADR 0021 の決定 5）。
    TargetMismatch,
    /// 公開の宣言のシグネチャが、非公開の型を名指しする（ADR 0021 の決定 4）。
    PrivateInPublic,
    /// 他のモジュールの構造体を構造体リテラルで作った（ADR 0021 の決定 4）。
    PrivateConstruction,
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
    RefTypePosition,
    RefReturnBorrowCount,
    BorrowPosition,
    MatchBorrowForm,
    UseAfterMove,
    MoveFromRef,
    MovedInLoop,
    BorrowOverlap,
    BorrowConflict,
    TempBorrow,
    ReturnBorrowRoot,
    RefSubstitution,
    ClosureNeedsMove,
    ClosureCaptureRef,
    ClosureMoveCaptured,
    ContractMove,
    EnsuresMoved,
    RefineMutBorrow,
    RedundantAnnotation,
    OldNotCopyable,
    MissingReturn,
    FnTypePosition,
    FnValueReassign,
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
    CloneOfCopyable,
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
            | OsItemInCore
            | UncheckedOutsideTrustBase
            | TrustBaseNotAllowed
            | ReservedModuleName
            | RawOutsideUnchecked
            | ModulePathMismatch
            | ModulePathCase
            | UnresolvedUse
            | PrivateImport
            | UseConflict
            | UnusedUse
            | ModuleCycle
            | ModuleLimit
            | TargetMismatch
            | PrivateInPublic
            | PrivateConstruction
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
            | RefTypePosition
            | RefReturnBorrowCount
            | BorrowPosition
            | MatchBorrowForm
            | UseAfterMove
            | MoveFromRef
            | MovedInLoop
            | BorrowOverlap
            | BorrowConflict
            | TempBorrow
            | ReturnBorrowRoot
            | RefSubstitution
            | ClosureNeedsMove
            | ClosureCaptureRef
            | ClosureMoveCaptured
            | ContractMove
            | EnsuresMoved
            | RefineMutBorrow
            | RedundantAnnotation
            | OldNotCopyable
            | MissingReturn
            | FnTypePosition
            | FnValueReassign
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
            | InfiniteType
            | CloneOfCopyable => Phase::Check,
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

impl Error {
    /// 機械向けの診断（`sigil check --json`。ADR 0019 の決定 2）。`kind` は `ErrorKind` の変種の名前。
    pub fn to_json(&self, file: &str) -> crate::json::Value {
        let phase = match self.kind.phase() {
            Phase::Lex => "lex",
            Phase::Syntax => "syntax",
            Phase::Fmt => "fmt",
            Phase::Check => "check",
        };
        crate::json::obj(vec![
            ("file", file.into()),
            ("kind", format!("{:?}", self.kind).into()),
            ("phase", phase.into()),
            ("line", self.span.line.into()),
            ("col", self.span.col.into()),
            ("message", self.message.clone().into()),
        ])
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

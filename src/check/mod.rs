//! 型検査（M1。ADR 0011）。
//!
//! 基本型・組・参照・構造体・列挙型・篩型・総称（関数・構造体・列挙型・`impl`）・6 つの性質と型引数の
//! 制約・`Option`/`Result`/`?`・`match` と網羅性・クロージャ・単一化による推論・`T.check`・`let` の
//! 型注釈の要否を扱う。式を下から上へ型づけ、最初の型エラーで打ち切る（M0 と同じ）。標準ライブラリの
//! 関数・メソッド（`List.new`・`xs.len()` など）は後続の PR で扱うので、いまは `Unsupported` にする
//! （黙って通さない）。効果の検査は含まない。所有権の検査のうち、移動（ADR 0012 の決定 1 の 1・2）は
//! 型を見る流れの中で行う（`Flow`）。借用の区間・クロージャのとらえ方は後続の PR で扱う。
//!
//! 位置: 式は位置を持たないので、エラーの位置は式を含む文（または宣言）の位置になる。
//!
//! 推論: 型変数 `Ty::Var` に単一化で型を割り当てる。関数 1 つの本体ごとに状態を作り直す。性質の
//! 制約（`T: Equatable` など）は、型が決まった時点で調べ、決まっていなければ本体の終わりまで
//! 繰り延べる。`let` の型注釈の要否は、注釈を外して本体を検査し直し、同じ型に決まるかで判定する。

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ast::*;
use crate::borrow::Loans;
use crate::effects::{EffectTable, Options, Primitive};
use crate::error::{Error, ErrorKind, Span};
use crate::parser::parse;

type R<T> = Result<T, Error>;

mod call;
mod closure;
mod effects;
mod expr;
mod fn_check;
mod generics;
mod items;
mod ownership;
mod pattern;
mod register;
mod resolve;
mod stmt;
mod traits;

pub use closure::{closure_captures, CapMode, Capture, ClosureInfo};
use closure::{Frame, RawClosure};

/// 呼び出しの解決の種類（ADR 0015 の決定 3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResKind {
    /// 名前で呼ぶ関数（利用者の関数・標準の自由関数・`Some`/`Ok`/`Err` などの値の構築子）。
    Fn,
    /// 局所の関数の値（引数・`let` で束縛した関数）の呼び出し。
    Local,
    /// 名前空間の関数（`fs.read`・`console.log`）。
    Namespaced,
    /// 型に付く関数（`Path.new`・`List.new`）。`owner` が型の名前。
    TypeFn,
    /// 利用者の列挙型の値の構築子（`Color.Red(..)`）。`owner` が型の名前。
    Variant,
    /// 篩型の `T.check`。`owner` が型の名前。
    Check,
    /// 受け手のあるメソッド（`xs.push(..)`）。`owner` が受け手の型の名前。
    Method,
    /// トレイトの実装のメソッド（`x.m(..)`・`トレイト.m(x)`）。`owner` がトレイトの名前（ADR 0022 決定 2）。
    TraitMethod,
    /// 型引数の制約を通した、受け手のないトレイトの関数（`T.f(..)`）。`owner` がトレイトの名前、
    /// 型引数の名前と `Param` 型が `targs` に 1 つ入る（ADR 0022 決定 2）。
    TraitTypeFn,
}

/// 呼び出しの解決（どの関数・メソッドか、型引数の具体化）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolved {
    pub(crate) kind: ResKind,
    pub(crate) owner: Option<String>,
    pub(crate) name: String,
    /// 型引数の名前と、具体化した型（宣言の順）。
    pub(crate) targs: Vec<(String, Ty)>,
}

/// 検査器が記録した型の情報（ADR 0015 の決定 3）。表の鍵は式・文の番地（`*const Expr as usize`）。
/// 番地は `file` を借りている間だけ有効で、検査器は式を複製せずに検査する。
pub struct Typed<'a> {
    pub file: &'a File,
    /// 式の番地 -> 具体化前の型（型引数を含みうる）。
    pub(crate) expr_ty: HashMap<usize, Ty>,
    /// 呼び出しの式の番地 -> 解決した関数・メソッドと型引数。
    pub(crate) calls: HashMap<usize, Resolved>,
    /// 値を移動する名前の式の番地（それ以外の読みは複製か借用）。
    pub(crate) moves: HashSet<usize>,
    /// `let` の文（`&Stmt`）の番地 -> 束縛した型。
    pub(crate) let_ty: HashMap<usize, Ty>,
}

#[derive(Default)]
struct TypedTables {
    expr_ty: HashMap<usize, Ty>,
    calls: HashMap<usize, Resolved>,
    moves: HashSet<usize>,
    let_ty: HashMap<usize, Ty>,
}

/// 検査して、式ごとの型・呼び出しの解決・移動の地点の表を返す。`check_file` と同じ検査をする。
pub fn typed(file: &File) -> R<Typed<'_>> {
    typed_with(file, &Options::default())
}

pub fn typed_with<'a>(file: &'a File, opts: &Options) -> R<Typed<'a>> {
    let ck = analyze(file, &[], &[], opts)?;
    let t = ck.typed;
    Ok(Typed {
        file,
        expr_ty: t.expr_ty,
        calls: t.calls,
        moves: t.moves,
        let_ty: t.let_ty,
    })
}

fn addr(e: &Expr) -> usize {
    e as *const Expr as usize
}

/// ソースを構文解析してから型検査する。最初のエラーを返す。
pub fn check(src: &str) -> R<()> {
    check_with(src, &Options::default())
}

/// 効果のカテゴリ表や仮の関数（標準ライブラリが入るまで）を指定して検査する。
pub fn check_with(src: &str, opts: &Options) -> R<()> {
    check_file_with(&parse(src)?, opts)
}

#[cfg(test)]
mod tests;
mod types;
mod unify;
mod util;

pub(crate) use types::Ty;
use types::{args_named, show, substitute, Prop, Sig, TypeDef};
use util::{
    contains_ref, contains_ref_outside_fn, is_fn_or_fn_list, place_path, place_root, strip_paren,
};

/// トレイトの宣言（ADR 0022 決定 2）。関数のシグネチャは `Self` を `Param("Self")` として持つ。
#[derive(Debug, Clone)]
struct TraitDef {
    fns: Vec<(FnDecl, Sig)>,
}

/// `impl トレイト for 型` の登録（ADR 0022 決定 2）。
#[derive(Debug, Clone)]
struct TraitImpl {
    trait_name: String,
    /// 型の頭（型の構築子の名前。`List<Int>` なら `List`、組み込みの型はその名前）。
    head: String,
    /// 実装する型（`impl` の型引数を `Param` として持つ）。
    ty: Ty,
    generics: Vec<(String, Vec<Prop>)>,
    trait_bounds: Vec<(String, String)>,
    fns: Vec<(FnDecl, Sig)>,
    /// 同じ実装が複数の経路で届いたときの同一判定（宣言の位置と関数の構成）。
    /// 取り込みは複製を作るので、番地では同一と分からない。
    fp: (u32, u32, usize, String, u32),
}

/// 型が決まってから調べるトレイトの制約。
struct TraitPending {
    ty: Ty,
    trait_name: String,
    /// `{}` の位置に型を入れて表示する。
    text: String,
    span: Span,
}

/// シグネチャの効果の 1 項。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EffTerm {
    /// カテゴリ名（`fs`・`console` など。下位を含む）。
    Cat(String),
    /// `of(f)`: 引数の位置（`self` を含む）。
    Of(usize),
}

/// 推論した効果の 1 つ。
#[derive(Debug, Clone, PartialEq)]
enum Atom {
    Cat(String),
    /// 関数を受け取る引数 `f` の効果（`of(f)`）。
    Of(String),
}

/// 呼び出しで起こる効果。
#[derive(Default)]
struct Eff {
    terms: Vec<EffTerm>,
    /// 呼び出す関数が持つ引数の数のずれ（メソッドは `self` の分だけ 1）。
    off: usize,
    extra: Vec<Atom>,
}

#[derive(Debug, Clone)]
struct Local {
    ty: Ty,
    /// `let mut` で宣言した変数。
    mutable: bool,
    /// 引数（メッセージの出し分け用）。
    param: bool,
}

/// 標準の前置きの型。利用者は同じ名前の型を宣言できない。
const PRELUDE_TYPES: &[&str] = &[
    "List",
    "Option",
    "Result",
    "Map",
    "Set",
    "Path",
    "IoError",
    "ParseError",
    "Shared",
    "Cell",
    "Addr",
    // 並行処理の前置き型（ADR 0026 の決定 3）。
    "Scope",
    "TaskHandle",
    "Sender",
    "Receiver",
    "SyncShared",
    "Lock",
    "LockGuard",
    "LockBroken",
    "Atomic",
];
/// 型名を付けずに書く値（`Option`・`Result` の値）。
const PRELUDE_VALUES: &[(&str, &str)] = &[
    ("Some", "Option"),
    ("None", "Option"),
    ("Ok", "Result"),
    ("Err", "Result"),
];

#[derive(Clone, Copy, PartialEq)]
enum Pos {
    /// 関数・メソッドの引数の型（関数の型を書ける唯一の位置）。一番外に参照を書ける。
    Param,
    /// 関数の型の引数の型。一番外に参照を書ける（`fn(&T) -> K`）。
    FnParam,
    /// 戻り値の型。一番外と、一番外の `Option<…>` の中に参照を書ける。
    Ret,
    /// 戻り値の一番外の `Option<…>` の型引数。参照を書ける。
    RetOpt,
    /// 引数の一番外の `List<…>` の型引数（`task.all_ok`・`task.race` の `List<fn(...)>` を許可）。
    ParamFnList,
    /// 篩型の基の型。参照は書けるが、そのあと `RefinementBase` で弾く。
    Base,
    /// 関数の型を置ける位置（構造体のフィールド・列挙型の値の中身・`let` の型注釈。
    /// ADR 0022 決定 3）。関数の型には効果節が必須。参照は書けない。
    Placed,
    /// それ以外（型引数など）。参照を書けない。
    Other,
}

/// 型が決まってから調べる性質の制約。
struct Pending {
    ty: Ty,
    prop: Prop,
    kind: ErrorKind,
    /// `{}` の位置に型を入れて表示する。
    text: String,
    span: Span,
}

/// 標準ライブラリの OS 層の項目の名前（ADR 0014 の決定 4。鍵は `types`・`fns`・`methods` の表と同じ形）。
#[derive(Default)]
struct OsItems {
    /// 型名（`"Path"`）。
    types: HashSet<String>,
    /// 名前空間の関数（`"console.log"`）。
    fns: HashSet<String>,
    /// メソッドと型に付く関数（`("Path", "new")`）。
    methods: HashSet<(String, String)>,
}

struct Ck {
    /// 他のモジュールから取り込んだ宣言（シグネチャだけを登録し、本体は検査しない）。
    externs: Vec<Item>,
    /// 取り込んだ構造体の名前。構造体リテラルで作れない（`PrivateConstruction`）。
    extern_types: HashSet<String>,
    /// `targets` に `bare` を含む核のモジュールか（ADR 0014 の決定 1）。標準ライブラリの表を読む間は偽にする。
    core: bool,
    os_items: OsItems,
    types: HashMap<String, TypeDef>,
    fns: HashMap<String, Sig>,
    methods: HashMap<(String, String), Sig>,
    /// トレイトの宣言（名前 -> 関数の一覧。ADR 0022 決定 2）。
    traits: HashMap<String, TraitDef>,
    /// この検査で知っているトレイトの名前すべて（制約の名前の解決に使う）。
    trait_names: HashSet<String>,
    /// `impl トレイト for 型` の登録（自分のモジュールと、取り込んだもの。ADR 0022 決定 2）。
    trait_impls: Vec<TraitImpl>,
    /// `ImplForDecl` の番地 -> `trait_impls` の位置。
    impl_index: HashMap<usize, usize>,
    /// このファイルが宣言したトレイトと型の名前（置き場所の規則の判定に使う）。
    own_traits: HashSet<String>,
    own_types: HashSet<String>,
    /// メソッドとして呼べるトレイト（自分の宣言と、`use` で名指しした取り込み。ADR 0022 決定 2）。
    visible_traits: HashSet<String>,
    /// いま検査している関数の型引数のトレイトの制約。
    cur_trait_bounds: Vec<(String, String)>,
    /// 型が決まってから調べるトレイトの制約。
    trait_pending: Vec<TraitPending>,
    /// 多相再帰の検査: 循環する総称の関数の名前 -> 強連結成分の番号（ADR 0022 決定 2）。
    scc: HashMap<String, usize>,
    scopes: Vec<HashMap<String, Local>>,
    span: Span,
    ret: Ty,
    result: Option<Ty>,
    refine: bool,
    /// 型の中で名前として使える型引数。
    tparams: Vec<String>,
    /// いま検査している関数の型引数の制約。
    env: HashMap<String, Vec<Prop>>,
    subst: Vec<Option<Ty>>,
    pending: Vec<Pending>,
    /// 本体の終わりまでに決まっていなければならない型変数（説明つき）。
    must_know: Vec<(Ty, String, Span)>,
    /// `let` の型注釈の通し番号（要否の判定で、番号ごとに注釈を外して検査し直す）。
    annot_n: usize,
    annots: Vec<(Span, Ty)>,
    skip: Option<usize>,
    skipped: Option<Ty>,
    /// 型の情報の記録（`skip` が `None` の 1 回目の検査だけ。関数の終わりに `flush_typed` で確定する）。
    rec_ty: Vec<(usize, Ty)>,
    rec_calls: Vec<(usize, Resolved)>,
    rec_moves: Vec<(usize, Ty)>,
    rec_lets: Vec<(usize, Ty)>,
    /// 検査中の呼び出しの解決（呼び出しの入れ子ごとに 1 つ）。
    call_stack: Vec<Option<Resolved>>,
    /// 直近の `instantiate`・`fresh_named` が作った型引数。
    last_inst: Vec<(String, Ty)>,
    typed: TypedTables,
    table: EffectTable,
    prims: Vec<Primitive>,
    copy_override: Vec<(String, bool)>,
    /// `holds` が今たどっている（型, 性質）。輪になった型の導出を止める。
    holding: std::cell::RefCell<Vec<(Ty, Prop)>>,
    /// モジュールの `targets`。
    targets: Vec<String>,
    /// 効果の推論の枠（先頭が関数の本体、クロージャごとに積む）。
    eff_stack: Vec<Vec<Atom>>,
    /// 関数の型の引数（`of(f)` の `f` になれる）。
    fn_params: Vec<String>,
    /// 呼び出し側から借りている（または名前で参照した）関数の値の変数。移動しない（ADR 0013 の決定 3）。
    fn_borrowed: HashSet<String>,
    /// `let` で束縛したクロージャの効果。
    local_eff: HashMap<String, Vec<Atom>>,
    /// 次のクロージャの効果を、親の枠に足さずに `last_closure` へ取っておく。
    defer: bool,
    /// 次に評価する `&e` が、呼び出しの引数の位置にある（`args` が立て、`unary` が取る）。
    ref_ok: bool,
    /// `match` の対象が参照（`&x` か、参照を含む値）で、腕の束縛が参照になる。
    by_ref: bool,
    /// 移動の状態（ADR 0012 の決定 1 の 1・2）。
    flow: Flow,
    /// 移動を追う。契約の式と篩型の条件の中では追わない（ADR 0012 の決定 3）。
    track: bool,
    /// 現在のクロージャの本体のスコープの位置（これより前のスコープの変数が外側）。0 ならクロージャの外。
    closure_base: usize,
    /// 次に評価する場所の式は、移動済みかの検査を飛ばす（呼び出し側が全体の場所で調べる）。
    skip_use: bool,
    /// `pat` が、値で束縛する Copyable でない部分を見つけた。
    pat_moves: bool,
    /// 直前に評価した式の値が借りている場所（参照を含む値の「貸し」。ADR 0012 の決定 1 の 3）。
    res: Vec<Src>,
    /// 場所の読み出し・借用・移動の記録（1 つの呼び出しの中の重なりの検査用）。
    log: Vec<Acc>,
    /// 貸しを持つ変数のうち、使われたもの（ループの次の周での使用の検査用）。
    used: BTreeSet<String>,
    /// 置き換えた後のシグネチャ（引数の型・戻り値の型・呼び出しの説明・位置）。型が決まってから調べ直す。
    sig_checks: Vec<(Vec<Ty>, Ty, String, Span)>,
    /// 型が決まっていない値の移動の後の使用。型が決まって `Copyable` でないと分かればエラーにする。
    moved_pending: Vec<(Ty, Error)>,
    last_closure: Vec<Atom>,
    inferred: Vec<Atom>,
    /// いま検査しているクロージャの枠（外側から内側の順）。
    frames: Vec<Frame>,
    /// 次に評価するクロージャが、呼び出しの引数の位置にある（`args` が立て、`closure` が取る）。
    closure_arg: bool,
    /// 本体を検査する間に作ったクロージャの記録と、型が決まった後の記録、ファイル全体の記録。
    cur_closures: Vec<RawClosure>,
    fin_closures: Vec<ClosureInfo>,
    closures: Vec<ClosureInfo>,
    cur_fn: String,
    /// 契約の式（`requires`・`ensures`）を検査している。
    contract: bool,
    in_ensures: bool,
    in_old: bool,
    /// `ensures` が（`old` の外で）参照する、値で受け取る引数。
    ens_used: BTreeSet<String>,
    /// `let mut` で、型が決まっていなかった変数（名前・型・位置）。型が決まってから調べ直す。
    let_pending: Vec<(String, Ty, Span)>,
    /// 原始操作（ADR 0016）。`raw.名前` -> 前提の文と、効果を宣言に委ねるか。
    raw_ops: HashMap<String, RawInfo>,
    /// 登録する原始操作の定義（標準ライブラリの `RAW_OPS` とテスト用の差し込み）。
    raw_defs: Vec<crate::effects::RawOp>,
    /// 検査中の関数が `unchecked` の節を持つ（原始操作を直接呼べる）。
    raw_fn: bool,
    /// クロージャの本体を検査している入れ子の深さ。
    closure_depth: u32,
    /// 検査中の関数が呼んだ原始操作（`raw.名前`）と、宣言に委ねる原始操作を呼んだか。
    raw_used: BTreeSet<String>,
    raw_delegated: bool,
    /// このモジュールが `trust base` を宣言している。
    base_module: bool,
    /// `trust base` の宣言を許すモジュール名。
    allowed_base: Vec<String>,
    /// 関数ごとの記録（`sigil spec` が使う）。鍵は `FnDecl` の番地。
    reports: HashMap<usize, FnReport>,
    /// 大域変数（`global`・`global mut`）。名前 -> (型, 可変か)
    globals: HashMap<String, (Ty, bool)>,
    /// 性質の手付け（`properties`）。(型名, 性質)
    manual_properties: Vec<(String, Prop)>,
}

/// 原始操作の登録情報。
struct RawInfo {
    premise: String,
    delegated: bool,
}

/// 関数ごとの検査の記録（`sigil spec` の `使う原始操作:`・`推論できない効果:`。ADR 0016 の決定 6）。
#[derive(Debug, Clone, Default)]
pub struct FnReport {
    /// 本体から呼ぶ原始操作（`raw.名前`）と表の前提の文。名前の順で重複がない。
    pub raw_ops: Vec<(String, String)>,
    /// 宣言に委ねる原始操作を呼ぶか。
    pub delegated: bool,
    /// 宣言した効果のうち、本体から推論されなかったもの（葉のカテゴリか `of(x)`）。
    pub not_inferred: Vec<String>,
}

/// `FnReport` の表の鍵。
pub fn fn_key(f: &FnDecl) -> usize {
    f as *const FnDecl as usize
}

/// 検査して、関数ごとの記録を返す。`check_file_with` と同じ検査をする。
pub fn fn_reports(file: &File, opts: &Options) -> R<HashMap<usize, FnReport>> {
    analyze(file, &[], &[], opts).map(|ck| ck.reports)
}

/// 移動の状態。移動した場所（`a`・`a.owner`）とその位置、そして経路が早期の戻りで終わったか。
#[derive(Debug, Clone, Default, PartialEq)]
struct Flow {
    moved: BTreeMap<String, Span>,
    /// 型が決まっていない値を移動したかもしれない場所（型が決まってから移動かを調べる）。
    maybe: BTreeMap<String, (Span, Ty)>,
    /// 参照を含む値を持つ変数が、いま借りている場所（貸し）。
    loans: Loans,
    dead: bool,
}

/// 借りている場所（経路と、`&mut` か）。経路が `TEMP` なら一時的な値。
type Src = (String, bool);

#[derive(Debug, Clone, Copy, PartialEq)]
enum AccKind {
    Read,
    Shared,
    Mut,
    Move,
}

/// 1 つの呼び出しの中の、場所の使われ方の記録。
#[derive(Debug, Clone)]
struct Acc {
    path: String,
    kind: AccKind,
}

/// 経路の合流。早期に戻る経路は加えず、そうでなければ移動した場所の和集合（先の位置を残す）。
fn merge(a: Flow, b: Flow) -> Flow {
    if a.dead {
        return b;
    }
    if b.dead {
        return a;
    }
    let mut m = a.moved;
    for (k, v) in b.moved {
        m.entry(k).or_insert(v);
    }
    let mut maybe = a.maybe;
    for (k, v) in b.maybe {
        maybe.entry(k).or_insert(v);
    }
    let loans = a.loans.merge(b.loans);
    Flow {
        moved: m,
        maybe,
        loans,
        dead: false,
    }
}

pub fn check_file(file: &File) -> R<()> {
    check_file_with(file, &Options::default())
}

pub fn check_file_with(file: &File, opts: &Options) -> R<()> {
    // `use` の先は `--root` がないと決まらない（黙って取り込み元を無視しない）。
    if let Some(u) = file.uses.first() {
        return Err(Error::new(
            ErrorKind::Unsupported,
            u.span,
            "`use` の先のモジュールを探す根がない（`sigil check --root <根>` で検査する）"
                .to_string(),
        ));
    }
    analyze(file, &[], &[], opts).map(|_| ())
}

/// 他のモジュールから取り込んだ宣言（`externs`。シグネチャだけを使い、本体は検査しない）を
/// 加えて検査する（ADR 0021 の決定 3・5。`project` が集める）。
pub fn check_module(
    file: &File,
    externs: &[Item],
    visible_traits: &[String],
    opts: &Options,
) -> R<()> {
    analyze(file, externs, visible_traits, opts).map(|_| ())
}

/// 標準の名前空間の頭（`fs`・`console`・`raw`）と `std`。モジュール名の最初の部分には使えない（ADR 0021 の 2-2b の決め）。
pub fn std_heads() -> &'static [&'static str] {
    // 固定の一覧。追加は ADR 0021 の改訂を伴う（標準の名前空間が増えても自動では増えない。
    // 増えたときは下の試験が落ちるので、ADR を改訂して一覧に足す）。
    &["console", "fs", "raw", "std"]
}

/// 標準の前置きの名前（型・値）と名前空間の頭か。`use` で取り込む名前に使えない。
pub fn is_std_name(n: &str) -> bool {
    PRELUDE_TYPES.contains(&n)
        || PRELUDE_VALUES.iter().any(|(v, _)| *v == n)
        || std_heads().contains(&n)
}

fn analyze(file: &File, externs: &[Item], visible_traits: &[String], opts: &Options) -> R<Ck> {
    let mut ck = Ck {
        externs: externs.to_vec(),
        extern_types: externs
            .iter()
            .filter_map(|i| match i {
                Item::Struct(s) => Some(s.name.clone()),
                _ => None,
            })
            .collect(),
        core: file.module.targets.iter().any(|t| t == "bare"),
        os_items: OsItems::default(),
        types: HashMap::new(),
        fns: HashMap::new(),
        methods: HashMap::new(),
        traits: HashMap::new(),
        trait_names: file
            .items
            .iter()
            .chain(externs.iter())
            .filter_map(|i| match i {
                Item::Trait(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect(),
        trait_impls: vec![],
        impl_index: HashMap::new(),
        own_traits: file
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Trait(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect(),
        own_types: file
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Struct(s) => Some(s.name.clone()),
                Item::Enum(e) => Some(e.name.clone()),
                Item::Type(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect(),
        visible_traits: file
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Trait(t) => Some(t.name.clone()),
                _ => None,
            })
            .chain(visible_traits.iter().cloned())
            .collect(),
        cur_trait_bounds: vec![],
        trait_pending: vec![],
        scc: HashMap::new(),
        scopes: vec![],
        span: file.module.span,
        ret: Ty::Unit,
        result: None,
        refine: false,
        tparams: vec![],
        env: HashMap::new(),
        subst: vec![],
        pending: vec![],
        must_know: vec![],
        annot_n: 0,
        annots: vec![],
        skip: None,
        skipped: None,
        rec_ty: vec![],
        rec_calls: vec![],
        rec_moves: vec![],
        rec_lets: vec![],
        call_stack: vec![],
        last_inst: vec![],
        typed: TypedTables::default(),
        table: opts.table.clone(),
        prims: opts.primitives.clone(),
        copy_override: opts.copy_override.clone(),
        holding: Default::default(),
        targets: file.module.targets.clone(),
        eff_stack: vec![vec![]],
        fn_params: vec![],
        fn_borrowed: HashSet::new(),
        local_eff: HashMap::new(),
        defer: false,
        ref_ok: false,
        by_ref: false,
        flow: Flow::default(),
        track: true,
        closure_base: 0,
        skip_use: false,
        pat_moves: false,
        res: vec![],
        log: vec![],
        used: BTreeSet::new(),
        sig_checks: vec![],
        moved_pending: vec![],
        last_closure: vec![],
        inferred: vec![],
        frames: vec![],
        closure_arg: false,
        cur_closures: vec![],
        fin_closures: vec![],
        closures: vec![],
        cur_fn: String::new(),
        contract: false,
        in_ensures: false,
        in_old: false,
        ens_used: BTreeSet::new(),
        let_pending: vec![],
        raw_ops: HashMap::new(),
        raw_defs: {
            let user_names: BTreeSet<String> = opts
                .raw_ops
                .iter()
                .map(|r| r.signature.split(['(', '<']).next().unwrap().to_string())
                .collect();
            crate::stdlib::RAW_OPS
                .iter()
                .filter(|r| {
                    let name = r.signature.split(['(', '<']).next().unwrap();
                    !user_names.contains(name)
                })
                .map(|r| crate::effects::RawOp {
                    layer: r.layer,
                    signature: r.signature.to_string(),
                    effects: r.effects.to_string(),
                    requires: r.requires.to_string(),
                    premise: r.premise.to_string(),
                })
                .chain(opts.raw_ops.iter().cloned())
                .collect()
        },
        raw_fn: false,
        closure_depth: 0,
        raw_used: BTreeSet::new(),
        raw_delegated: false,
        base_module: file.module.trust == Trust::Base,
        allowed_base: opts.trust_base.clone(),
        reports: HashMap::new(),
        globals: HashMap::new(),
        manual_properties: vec![],
    };
    ck.prelude();
    ck.run(file)?;
    Ok(ck)
}

/// 効果の集合 `e` が `d` 以下か（ADR 0022 決定 2 の効果の順序 ⊑）。
/// カテゴリは葉のカテゴリへ広げて比べ、`of` は引数の位置で比べる。
fn effects_subset(table: &crate::effects::EffectTable, e: &[EffTerm], d: &[EffTerm]) -> bool {
    let mut datoms: HashSet<String> = HashSet::new();
    let mut dofs: HashSet<usize> = HashSet::new();
    for t in d {
        match t {
            EffTerm::Cat(c) => datoms.extend(table.atoms(c)),
            EffTerm::Of(i) => {
                dofs.insert(*i);
            }
        }
    }
    e.iter().all(|t| match t {
        EffTerm::Cat(c) => table.atoms(c).iter().all(|a| datoms.contains(a)),
        EffTerm::Of(i) => dofs.contains(i),
    })
}

impl Ck {
    fn err<T>(&self, kind: ErrorKind, msg: impl Into<String>) -> R<T> {
        Err(Error::new(kind, self.span, msg))
    }

    fn unsupported<T>(&self, what: &str) -> R<T> {
        self.err(
            ErrorKind::Unsupported,
            format!("{what}は、この版の型検査ではまだ扱えない（M1 の後続の PR で対応する）"),
        )
    }

    fn rec_expr(&mut self, e: &Expr, t: &Ty) {
        if self.skip.is_none() {
            self.rec_ty.push((addr(e), t.clone()));
        }
    }

    /// 呼び出しの解決を、いま検査中の呼び出しに結ぶ（型引数は直近の具体化から取る）。
    fn set_resolved(
        &mut self,
        kind: ResKind,
        owner: Option<&str>,
        name: &str,
        targs: Vec<(String, Ty)>,
    ) {
        if self.skip.is_none() {
            if let Some(top) = self.call_stack.last_mut() {
                *top = Some(Resolved {
                    kind,
                    owner: owner.map(str::to_string),
                    name: name.to_string(),
                    targs,
                });
            }
        }
    }

    /// 1 回目の検査で記録した型の情報を、型変数が決まった状態で確定する（本体の検査が通った直後に呼ぶ）。
    fn flush_typed(&mut self) {
        for (k, t) in std::mem::take(&mut self.rec_ty) {
            let z = self.zonk(&t);
            self.typed.expr_ty.insert(k, z);
        }
        for (k, mut r) in std::mem::take(&mut self.rec_calls) {
            for (_, t) in &mut r.targs {
                *t = self.zonk(t);
            }
            self.typed.calls.insert(k, r);
        }
        for (k, t) in std::mem::take(&mut self.rec_moves) {
            let z = self.zonk(&t);
            if self.holds(&z, Prop::Copyable, 0) != Some(true) {
                self.typed.moves.insert(k);
            }
        }
        for (k, t) in std::mem::take(&mut self.rec_lets) {
            let z = self.zonk(&t);
            self.typed.let_ty.insert(k, z);
        }
    }
}

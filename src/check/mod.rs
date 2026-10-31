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
use crate::borrow::{overlaps, Dead, Loans, TEMP};
use crate::effects::{EffectTable, Options, Primitive};
use crate::error::{Error, ErrorKind, Span};
use crate::parser::parse;

type R<T> = Result<T, Error>;

mod closure;
mod effects;
mod generics;
mod pattern;
mod resolve;

pub use closure::{closure_captures, CapMode, Capture, ClosureInfo};
use closure::{Cap, Frame, RawClosure};

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
use types::{
    args_named, contains_param, head_of, match_pattern, show, substitute, Prop, Sig, TypeDef,
};
use util::{
    block_returns, collect_calls_block, conjuncts, contains_ref, contains_ref_outside_fn, op_text,
    pat_names, place_path, place_root, rename_names, strip_paren, strip_refs,
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
enum EffTerm {
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
    /// 篩型の基の型。参照は書けるが、そのあと `RefinementBase` で弾く。
    Base,
    /// それ以外（構造体のフィールド・列挙子の値・型引数・`let` の注釈など）。参照を書けない。
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
        raw_defs: crate::stdlib::RAW_OPS
            .iter()
            .map(|r| crate::effects::RawOp {
                layer: r.layer,
                signature: r.signature.to_string(),
                effects: r.effects.to_string(),
                requires: r.requires.to_string(),
                premise: r.premise.to_string(),
            })
            .chain(opts.raw_ops.iter().cloned())
            .collect(),
        raw_fn: false,
        closure_depth: 0,
        raw_used: BTreeSet::new(),
        raw_delegated: false,
        base_module: file.module.trust == Trust::Base,
        allowed_base: opts.trust_base.clone(),
        reports: HashMap::new(),
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

    fn prelude(&mut self) {
        let t = || Ty::Param("T".into());
        let e = || Ty::Param("E".into());
        self.types.insert(
            "Option".into(),
            TypeDef::Enum {
                params: vec!["T".into()],
                variants: vec![("Some".into(), vec![t()]), ("None".into(), vec![])],
            },
        );
        self.types.insert(
            "Result".into(),
            TypeDef::Enum {
                params: vec!["T".into(), "E".into()],
                variants: vec![("Ok".into(), vec![t()]), ("Err".into(), vec![e()])],
            },
        );
        for (n, k, layer) in crate::stdlib::OPAQUE_TYPES {
            self.types.insert((*n).into(), TypeDef::Opaque(*k));
            if *layer == crate::stdlib::Layer::Os {
                self.os_items.types.insert((*n).into());
            }
        }
    }

    fn run(&mut self, file: &File) -> R<()> {
        // 0. モジュールの宣言（ADR 0016 の決定 3・6 の補足。名前の予約が許可より先）
        self.span = file.module.span;
        let mname = file.module.name.join(".");
        if file
            .module
            .name
            .first()
            .is_some_and(|h| std_heads().contains(&h.as_str()))
        {
            return self.err(
                ErrorKind::ReservedModuleName,
                format!("モジュール名 `{mname}` は標準ライブラリの名前（`std`・`raw`・`fs`・`console` で始まる名前は利用者は使えない）"),
            );
        }
        // 公開のシグネチャが非公開の型を名指ししない（ADR 0021 の決定 4）
        if let Some((span, msg)) = crate::project::private_in_public(file) {
            self.span = span;
            return self.err(ErrorKind::PrivateInPublic, msg);
        }
        let externs = self.externs.clone();
        let all: Vec<&Item> = externs.iter().chain(file.items.iter()).collect();
        if self.base_module && !self.allowed_base.contains(&mname) {
            return self.err(
                ErrorKind::TrustBaseNotAllowed,
                format!("モジュール `{mname}` は `trust base` を宣言しているが、許可がない（検査・実行・組み立ての呼び出しに `--trust-base {mname}` を付ける。許可は人間が決める）"),
            );
        }
        // 1. 型の名前と型引数
        let mut names: HashSet<String> = HashSet::new();
        let mut shells: Vec<(String, TypeDef)> = vec![];
        for item in &all {
            let (name, span, generics, shell) = match item {
                Item::Type(t) => (Some(&t.name), t.span, &[][..], TypeDef::Alias(Ty::Unit)),
                Item::Struct(s) => (
                    Some(&s.name),
                    s.span,
                    &s.generics[..],
                    TypeDef::Struct {
                        params: vec![],
                        fields: vec![],
                    },
                ),
                Item::Enum(e) => (
                    Some(&e.name),
                    e.span,
                    &e.generics[..],
                    TypeDef::Enum {
                        params: vec![],
                        variants: vec![],
                    },
                ),
                Item::Fn(f) => (None, f.span, &f.generics[..], TypeDef::Alias(Ty::Unit)),
                Item::Impl(i) => (None, i.span, &i.generics[..], TypeDef::Alias(Ty::Unit)),
                // トレイトは上の 0' で止まるのでここには来ない。型の同一性としては名前を持つ（ADR 0022 の決定 1）。
                Item::Trait(t) => (Some(&t.name), t.span, &[][..], TypeDef::Alias(Ty::Unit)),
                Item::ImplFor(i) => (None, i.span, &i.generics[..], TypeDef::Alias(Ty::Unit)),
            };
            self.span = span;
            let Some(n) = name else { continue };
            if PRELUDE_TYPES.contains(&n.as_str()) || PRELUDE_VALUES.iter().any(|(v, _)| v == n) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("`{n}` は標準の前置きの名前なので、型の名前に使えない"),
                );
            }
            if !names.insert(n.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型 `{n}` が二重に宣言されている"),
                );
            }
            let (gs, _) = self.generics(generics, false)?;
            let params: Vec<String> = gs.into_iter().map(|(n, _)| n).collect();
            let shell = match shell {
                TypeDef::Struct { fields, .. } => TypeDef::Struct { params, fields },
                TypeDef::Enum { variants, .. } => TypeDef::Enum { params, variants },
                other => other,
            };
            shells.push((n.clone(), shell));
        }
        // 型の名前を先に仮登録（互いに参照できるように）
        for (n, d) in shells {
            self.types.insert(n, d);
        }
        // 2. 型の中身
        for item in &all {
            match item {
                Item::Type(t) => {
                    self.span = t.span;
                    let base = self.resolve(&t.base, Pos::Base)?;
                    if self.contains_bad_base(&base) {
                        return self.err(
                            ErrorKind::RefinementBase,
                            format!(
                                "篩型 `{}` の基の型 `{}` は、篩型・参照・関数の型を含まない型でなければならない",
                                t.name,
                                show(&base)
                            ),
                        );
                    }
                    self.types.insert(t.name.clone(), TypeDef::Alias(base));
                }
                Item::Struct(s) => {
                    self.span = s.span;
                    self.tparams = s.generics.iter().map(|g| g.name.clone()).collect();
                    let mut fields = vec![];
                    let mut seen = HashSet::new();
                    for f in &s.fields {
                        if !seen.insert(f.name.clone()) {
                            return self.err(
                                ErrorKind::DuplicateDefinition,
                                format!("フィールド `{}` が二重にある", f.name),
                            );
                        }
                        fields.push((f.name.clone(), self.resolve(&f.ty, Pos::Other)?));
                    }
                    self.tparams.clear();
                    let params = s.generics.iter().map(|g| g.name.clone()).collect();
                    self.types
                        .insert(s.name.clone(), TypeDef::Struct { params, fields });
                }
                Item::Enum(e) => {
                    self.span = e.span;
                    self.tparams = e.generics.iter().map(|g| g.name.clone()).collect();
                    let mut variants = vec![];
                    let mut seen = HashSet::new();
                    for v in &e.variants {
                        if !seen.insert(v.name.clone()) {
                            return self.err(
                                ErrorKind::DuplicateDefinition,
                                format!("値 `{}` が二重にある", v.name),
                            );
                        }
                        let mut ts = vec![];
                        for t in &v.fields {
                            ts.push(self.resolve(t, Pos::Other)?);
                        }
                        variants.push((v.name.clone(), ts));
                    }
                    self.tparams.clear();
                    let params = e.generics.iter().map(|g| g.name.clone()).collect();
                    self.types
                        .insert(e.name.clone(), TypeDef::Enum { params, variants });
                }
                _ => {}
            }
        }
        // 3. 再帰する型（ADR 0013 補遺。輪を通らない腕のある列挙型だけ書ける）
        let founded = self.well_founded();
        for item in &file.items {
            let (name, span) = match item {
                Item::Struct(s) => (&s.name, s.span),
                Item::Enum(e) => (&e.name, e.span),
                _ => continue,
            };
            self.span = span;
            if !founded.contains(name) && self.reaches(name, name, &mut HashSet::new()) {
                return self.err(
                    ErrorKind::RecursiveType,
                    format!(
                        "型 `{name}` が自分自身を含んでいて、値を作り始められない（再帰する型は、輪を通らない腕を持つ列挙型を経由して書く）"
                    ),
                );
            }
        }
        // 4. 関数とメソッドのシグネチャ
        self.register_std()?;
        self.register_raw()?;
        self.register_primitives()?;
        for item in &all {
            match item {
                Item::Fn(f) => {
                    self.span = f.span;
                    if self.fns.contains_key(&f.name) {
                        return self.err(
                            ErrorKind::DuplicateDefinition,
                            format!("関数 `{}` が二重に宣言されている", f.name),
                        );
                    }
                    let sig = self.signature(f, None, &[], &[])?;
                    self.fns.insert(f.name.clone(), sig);
                }
                Item::Impl(i) => self.register_impl(i, false)?,
                _ => {}
            }
        }
        // 4.5. トレイトと、その実装（ADR 0022 決定 2）。取り込んだ実装は登録だけをし、
        // 置き場所・適合の検査は、それを書いたモジュールの検査で済んでいるものとして扱う。
        for item in &all {
            if let Item::Trait(t) = item {
                self.register_trait(t)?;
            }
        }
        let n_externs = self.externs.len();
        for (idx, item) in all.iter().enumerate() {
            if let Item::ImplFor(i) = item {
                // 同じ実装が複数の `use` の経路で届いても、登録は 1 回だけ（二重の実装ではない）。
                if self
                    .impl_index
                    .contains_key(&(i as *const ImplForDecl as usize))
                {
                    continue;
                }
                self.register_impl_for(i, idx >= n_externs)?;
            }
        }
        self.build_scc(file);
        // 5. 篩型の条件と関数の本体
        for item in &file.items {
            match item {
                Item::Type(t) => self.check_refinement(t)?,
                Item::Fn(f) => {
                    let sig = self.fns[&f.name].clone();
                    self.check_fn(f, &sig)?;
                }
                Item::Impl(i) => {
                    self.tparams = i.generics.iter().map(|g| g.name.clone()).collect();
                    let r = self.resolve(&i.ty, Pos::Other);
                    self.tparams.clear();
                    let Ok(Ty::Named(tn, _)) = r else { continue };
                    for f in &i.fns {
                        let sig = self.methods[&(tn.clone(), f.name.clone())].clone();
                        self.check_fn(f, &sig)?;
                    }
                }
                Item::ImplFor(i) => {
                    let Some(&idx) = self.impl_index.get(&(i as *const ImplForDecl as usize))
                    else {
                        continue;
                    };
                    let imp = self.trait_impls[idx].clone();
                    for f in &i.fns {
                        let sig = imp
                            .fns
                            .iter()
                            .find(|(d, _)| d.name == f.name)
                            .map(|(_, s)| s.clone())
                            .expect("登録済みの実装の関数");
                        self.check_fn(f, &sig)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// `impl` の型の名前（`impl<T> List<T>` なら `List`）。標準ライブラリの表の鍵に使う。
    fn impl_type_name(&self, i: &ImplDecl) -> String {
        match &i.ty {
            Type::Path { path, .. } => path.join("."),
            _ => String::new(),
        }
    }

    /// 型 `tn` の `name`（型に付く関数またはメソッド）が OS 層か。型そのものが OS 層でもそう扱う。
    fn os_item_type_fn(&self, tn: &str, name: &str) -> bool {
        self.os_items.types.contains(tn)
            || self
                .os_items
                .methods
                .contains(&(tn.to_string(), name.to_string()))
    }

    /// 核のモジュールで OS 層の項目を名指ししたときのエラー（ADR 0014 の決定 4）。
    fn os_in_core<T>(&self, name: &str) -> R<T> {
        self.err(
            ErrorKind::OsItemInCore,
            format!("`{name}` は標準ライブラリの OS 層（std.os）にあり、`targets` に `bare` を持つモジュールからは使えない（このモジュールを OS なしで使わないなら `targets` から `bare` を外す）"),
        )
    }

    fn register_impl(&mut self, i: &ImplDecl, std: bool) -> R<()> {
        self.span = i.span;
        let (outer, outer_tb) = self.generics(&i.generics, true)?;
        self.tparams = outer.iter().map(|(n, _)| n.clone()).collect();
        let ity = self.resolve(&i.ty, Pos::Other);
        self.tparams.clear();
        let tn = match ity? {
            Ty::Named(tn, _) => tn,
            Ty::Str if std => "String".to_string(),
            Ty::Int if std => "Int".to_string(),
            _ => {
                return self.err(
                    ErrorKind::TypeMismatch,
                    "`impl` は構造体・列挙型・篩型に付ける",
                )
            }
        };
        if !std && PRELUDE_TYPES.contains(&tn.as_str()) {
            return self.err(
                ErrorKind::TypeMismatch,
                format!("`impl` は型を宣言したモジュールの中にだけ書ける（標準の型 `{tn}` には付けられない）"),
            );
        }
        for f in &i.fns {
            self.span = f.span;
            let key = (tn.clone(), f.name.clone());
            if self.methods.contains_key(&key) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("`{tn}` のメソッド `{}` が二重に宣言されている", f.name),
                );
            }
            if let Some(TypeDef::Struct { fields, .. }) = self.types.get(&tn) {
                if fields.iter().any(|(n, _)| *n == f.name) {
                    return self.err(
                        ErrorKind::MemberConflict,
                        format!(
                            "`{tn}` のフィールドとメソッドに同じ名前 `{}` は付けられない",
                            f.name
                        ),
                    );
                }
            }
            let sig = self.signature(f, Some((&tn, &i.ty)), &outer, &outer_tb)?;
            self.methods.insert(key, sig);
        }
        Ok(())
    }

    /// トレイトの宣言を登録する（ADR 0022 決定 2）。シグネチャは `Self` を型引数として持つ。
    fn register_trait(&mut self, t: &TraitDecl) -> R<()> {
        self.span = t.span;
        if self.traits.contains_key(&t.name) {
            return self.err(
                ErrorKind::DuplicateDefinition,
                format!("トレイト `{}` が二重に宣言されている", t.name),
            );
        }
        let self_ty = Type::Path {
            path: vec!["Self".to_string()],
            args: vec![],
        };
        let mut fns = vec![];
        let mut seen = HashSet::new();
        for f in &t.fns {
            self.span = f.span;
            if !seen.insert(f.name.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("トレイト `{}` に関数 `{}` が二重にある", t.name, f.name),
                );
            }
            // `Self` を型引数として解決する（`Param("Self")` になる）。
            self.tparams = vec!["Self".to_string()];
            let sig = self.signature_in(f, Some((&t.name, &self_ty)), vec![], vec![]);
            self.tparams.clear();
            fns.push((f.clone(), sig?));
        }
        self.traits.insert(t.name.clone(), TraitDef { fns });
        Ok(())
    }

    /// `impl トレイト for 型` の登録（ADR 0022 決定 2 の一貫性の規則）。
    /// `own` のときだけ、置き場所と適合を検査する（取り込んだ実装は、書いたモジュールで検査済み）。
    fn register_impl_for(&mut self, i: &ImplForDecl, own: bool) -> R<()> {
        self.span = i.span;
        let (generics, trait_bounds) = self.generics(&i.generics, true)?;
        self.tparams = generics.iter().map(|(n, _)| n.clone()).collect();
        let ity = self.resolve(&i.ty, Pos::Other);
        self.tparams.clear();
        let ity = ity?;
        let Some((head, _)) = head_of(&ity) else {
            return self.err(
                ErrorKind::TraitImplMismatch,
                "型の頭のない実装は書けない（`impl<T> トレイト for T` の形は書けない。型の構築子を持つ型に実装する）",
            );
        };
        // 取り込んだ実装は、トレイト自体を取り込んでいなくても登録する（呼べるかどうかは見える範囲で決まる）。
        let tdef = self.traits.get(&i.trait_name).cloned();
        if own && tdef.is_none() {
            return self.err(
                ErrorKind::UnknownType,
                format!("トレイト `{}` が見つからない", i.trait_name),
            );
        }
        let fp = (
            i.span.line,
            i.span.col,
            i.fns.len(),
            i.fns.first().map(|f| f.name.clone()).unwrap_or_default(),
            i.fns.first().map_or(0, |f| f.span.line),
        );
        if let Some((eidx, _)) = self
            .trait_impls
            .iter()
            .enumerate()
            .find(|(_, x)| x.trait_name == i.trait_name && x.head == head)
        {
            // 同じ実装が複数の `use` の経路で届いた場合は、二重ではなく同じものとして扱う。
            if self.trait_impls[eidx].fp == fp {
                self.impl_index
                    .insert(i as *const ImplForDecl as usize, eidx);
                return Ok(());
            }
            return self.err(
                ErrorKind::TraitImplDuplicate,
                format!(
                    "トレイト `{}` の型 `{head}` への実装はすでに 1 つある（同じ（トレイト, 型の頭）の組の実装は 1 つだけ）",
                    i.trait_name
                ),
            );
        }
        let mut fns = vec![];
        for f in &i.fns {
            self.span = f.span;
            let sig = self.signature(f, Some((&head, &i.ty)), &generics, &trait_bounds)?;
            fns.push((f.clone(), sig));
        }
        if own {
            // 置ける場所: トレイトを宣言したモジュールか、型を宣言したモジュールだけ。
            if !self.own_traits.contains(&i.trait_name) && !self.own_types.contains(&head) {
                self.span = i.span;
                return self.err(
                    ErrorKind::TraitImplPlacement,
                    format!(
                        "`impl {} for {head}` は、トレイト `{}` を宣言したモジュールか、型 `{head}` を宣言したモジュールにだけ書ける",
                        i.trait_name, i.trait_name
                    ),
                );
            }
            self.check_impl_conformance(
                i,
                tdef.as_ref().expect("自分の実装のトレイトは登録済み"),
                &ity,
                &fns,
            )?;
        }
        let idx = self.trait_impls.len();
        self.impl_index
            .insert(i as *const ImplForDecl as usize, idx);
        self.trait_impls.push(TraitImpl {
            trait_name: i.trait_name.clone(),
            head,
            ty: ity,
            generics,
            trait_bounds,
            fns,
            fp,
        });
        Ok(())
    }

    /// 実装がトレイトの宣言に合うか（ADR 0022 決定 2 の 4 節の規則と、固有のメソッドとの名前の重なり）。
    fn check_impl_conformance(
        &mut self,
        i: &ImplForDecl,
        tdef: &TraitDef,
        ity: &Ty,
        fns: &[(FnDecl, Sig)],
    ) -> R<()> {
        let head = head_of(ity).map(|(h, _)| h).unwrap_or_default();
        // 固有のメソッド・フィールドと、トレイトの関数の名前は重ねられない。
        for (f, _) in fns {
            self.span = f.span;
            if self.methods.contains_key(&(head.clone(), f.name.clone())) {
                return self.err(
                    ErrorKind::TraitImplMismatch,
                    format!(
                        "型 `{head}` には固有のメソッド `{}` があり、トレイト `{}` の関数と同じ名前は付けられない",
                        f.name, i.trait_name
                    ),
                );
            }
            if let Some(TypeDef::Struct { fields, .. }) = self.types.get(&head) {
                if fields.iter().any(|(n, _)| *n == f.name) {
                    return self.err(
                        ErrorKind::TraitImplMismatch,
                        format!(
                            "型 `{head}` にはフィールド `{}` があり、トレイト `{}` の関数と同じ名前は付けられない",
                            f.name, i.trait_name
                        ),
                    );
                }
            }
        }
        let subst: HashMap<String, Ty> = HashMap::from([("Self".to_string(), ity.clone())]);
        for (tf, tsig) in &tdef.fns {
            let Some((imf, isig)) = fns.iter().find(|(f, _)| f.name == tf.name) else {
                self.span = i.span;
                return self.err(
                    ErrorKind::TraitImplMismatch,
                    format!(
                        "`impl {} for {}` に関数 `{}` がない（トレイトの関数はすべて実装する）",
                        i.trait_name,
                        show(ity),
                        tf.name
                    ),
                );
            };
            self.check_fn_conformance(i, tf, tsig, imf, isig, &subst)?;
        }
        for (f, _) in fns {
            if !tdef.fns.iter().any(|(tf, _)| tf.name == f.name) {
                self.span = f.span;
                return self.err(
                    ErrorKind::TraitImplMismatch,
                    format!(
                        "`impl {} for {}` の関数 `{}` は、トレイト `{}` にない（余分な関数は書けない）",
                        i.trait_name,
                        show(ity),
                        f.name,
                        i.trait_name
                    ),
                );
            }
        }
        Ok(())
    }

    /// トレイトの関数 1 つと実装の関数の対応（シグネチャと 4 節。ADR 0022 決定 2）。
    fn check_fn_conformance(
        &mut self,
        i: &ImplForDecl,
        tf: &FnDecl,
        tsig: &Sig,
        imf: &FnDecl,
        isig: &Sig,
        subst: &HashMap<String, Ty>,
    ) -> R<()> {
        self.span = imf.span;
        let bad = |ck: &Ck, what: String| {
            ck.err::<()>(
                ErrorKind::TraitImplMismatch,
                format!(
                    "`impl {} for {}` の関数 `{}`: {what}",
                    i.trait_name,
                    show(&subst["Self"]),
                    tf.name
                ),
            )
        };
        if !imf.generics.is_empty() {
            return bad(
                self,
                "トレイトの関数に型引数はないので、実装の関数にも型引数は書けない".into(),
            );
        }
        if tsig.has_self != isig.has_self {
            return bad(self, "受け手（`self`）の有無がトレイトの宣言と違う".into());
        }
        if tsig.params.len() != isig.params.len() {
            return bad(
                self,
                format!(
                    "引数の数が違う（トレイトは {} 個、実装は {} 個）",
                    tsig.params.len(),
                    isig.params.len()
                ),
            );
        }
        for (k, ((_, tt), (_, it))) in tsig.params.iter().zip(isig.params.iter()).enumerate() {
            let want = substitute(tt, subst);
            if want != *it {
                return bad(
                    self,
                    format!(
                        "第 {} 引数の型が違う（トレイトは `{}`、実装は `{}`。`Self` と型引数を置き換えて同じでなければならない）",
                        k + 1,
                        show(&want),
                        show(it)
                    ),
                );
            }
        }
        let want_ret = substitute(&tsig.ret, subst);
        if want_ret != isig.ret {
            return bad(
                self,
                format!(
                    "戻り値の型が違う（トレイトは `{}`、実装は `{}`）",
                    show(&want_ret),
                    show(&isig.ret)
                ),
            );
        }
        // 引数の名前の対応（位置で対応させる）。
        let ren: HashMap<String, String> = tsig
            .params
            .iter()
            .zip(isig.params.iter())
            .map(|((a, _), (b, _))| (a.clone(), b.clone()))
            .collect();
        // requires: `Self` を置き換えたうえでの α 同値でなければならない。
        match (&tf.requires, &imf.requires) {
            (None, None) => {}
            (Some(a), Some(b)) => {
                if rename_names(a, &ren) != *b {
                    return bad(
                        self,
                        "`requires` がトレイトの宣言と違う（同じものを書き直す。引数の名前の違いだけは許される）"
                            .into(),
                    );
                }
            }
            _ => {
                return bad(
                    self,
                    "`requires` がトレイトの宣言と違う（同じものを書き直す）".into(),
                )
            }
        }
        // ensures: トレイトの条件を消すことはできない（足すのはよい）。
        if let Some(te) = &tf.ensures {
            let want = rename_names(te, &ren);
            let ok = match &imf.ensures {
                Some(ie) => {
                    let mut cs = vec![];
                    conjuncts(ie, &mut cs);
                    cs.contains(&want)
                }
                None => false,
            };
            if !ok {
                return bad(
                    self,
                    "`ensures` からトレイトの条件が消えている（条件を足すのはよいが、消すことはできない）"
                        .into(),
                );
            }
        }
        // effects: トレイト以下でなければならない（効果の順序 ⊑。ADR 0022 決定 2）。
        if !effects_subset(&self.table, &isig.effects, &tsig.effects) {
            return bad(
                self,
                "効果がトレイトの宣言を超えている（実装の効果はトレイト以下でなければならない）"
                    .into(),
            );
        }
        Ok(())
    }

    /// 型引数のトレイトの制約を、型が決まってから調べるよう繰り延べる。
    fn require_traits(&mut self, m: &HashMap<String, Ty>, tb: &[(String, String)], what: &str) {
        for (p, tr) in tb {
            if let Some(t) = m.get(p) {
                self.trait_pending.push(TraitPending {
                    ty: t.clone(),
                    trait_name: tr.clone(),
                    text: format!(
                        "{what}の型引数 `{p}` はトレイト `{tr}` を実装しなければならないが、`{{}}` は実装していない"
                    ),
                    span: self.span,
                });
            }
        }
    }

    /// 型 `ty` がトレイト `trait_name` を実装するか（`Some(true/false)`。決まらなければ `None`）。
    fn trait_satisfied(&self, trait_name: &str, ty: &Ty, depth: u32) -> Option<bool> {
        if depth > 16 {
            return Some(false);
        }
        match self.shallow(ty) {
            Ty::Var(_) => None,
            Ty::Param(p) => Some(
                self.cur_trait_bounds
                    .iter()
                    .any(|(n, t)| *n == p && t == trait_name),
            ),
            t => {
                let z = self.zonk(&t);
                let Some((head, _)) = head_of(&z) else {
                    return Some(false);
                };
                // 実装がなければ「満たさない」（決まらない、ではない）。
                let Some(imp) = self
                    .trait_impls
                    .iter()
                    .find(|x| x.trait_name == trait_name && x.head == head)
                else {
                    return Some(false);
                };
                let Some(m) = match_pattern(self, &imp.ty, &z) else {
                    return Some(false);
                };
                for (g, props) in &imp.generics {
                    let Some(at) = m.get(g) else {
                        return Some(false);
                    };
                    for p in props {
                        match self.holds(at, *p, 0) {
                            Some(true) => {}
                            other => return other,
                        }
                    }
                }
                for (g, tr) in &imp.trait_bounds {
                    let Some(at) = m.get(g) else {
                        return Some(false);
                    };
                    match self.trait_satisfied(tr, at, depth + 1) {
                        Some(true) => {}
                        other => return other,
                    }
                }
                Some(true)
            }
        }
    }

    /// 多相再帰の拒否（ADR 0022 決定 2）: 循環する総称の関数どうしの呼び出しで、呼ぶ側の型引数を
    /// 真に含む型を型引数に渡していたら、その呼び出しの位置でエラーにする。
    fn poly_check(&mut self, m: &HashMap<String, Ty>, callee: &str) -> R<()> {
        let (Some(&a), Some(&b)) = (self.scc.get(&self.cur_fn), self.scc.get(callee)) else {
            return Ok(());
        };
        if a != b {
            return Ok(());
        }
        for t in m.values() {
            let z = self.zonk(t);
            if self
                .tparams
                .iter()
                .any(|p| contains_param(&z, p) && z != Ty::Param(p.clone()))
            {
                return self.err(
                    ErrorKind::PolymorphicRecursion,
                    format!(
                        "関数 `{}` から `{callee}` を呼ぶときに、型引数を真に含む型 `{}` を渡している（多相再帰は書けない。単相化が止まらなくなる）",
                        self.cur_fn,
                        show(&z)
                    ),
                );
            }
        }
        Ok(())
    }

    /// 循環する総称の関数の強連結成分を作る（名前で引く。メソッドとトレイトの関数も名前でまとめる）。
    fn build_scc(&mut self, file: &File) {
        let mut generic: HashSet<&str> = HashSet::new();
        let mut bodies: Vec<(&str, &Block, bool)> = vec![];
        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    let g = !f.generics.is_empty();
                    if g {
                        generic.insert(f.name.as_str());
                    }
                    bodies.push((f.name.as_str(), &f.body, g));
                }
                Item::Impl(i) => {
                    let g = !i.generics.is_empty();
                    for f in &i.fns {
                        let gf = g || !f.generics.is_empty();
                        if gf {
                            generic.insert(f.name.as_str());
                        }
                        bodies.push((f.name.as_str(), &f.body, gf));
                    }
                }
                Item::ImplFor(i) => {
                    let g = !i.generics.is_empty();
                    for f in &i.fns {
                        let gf = g || !f.generics.is_empty();
                        if gf {
                            generic.insert(f.name.as_str());
                        }
                        bodies.push((f.name.as_str(), &f.body, gf));
                    }
                }
                _ => {}
            }
        }
        let mut edges: HashMap<String, HashSet<String>> = HashMap::new();
        for (name, body, g) in &bodies {
            if !g {
                continue;
            }
            let mut called = vec![];
            collect_calls_block(body, &mut called);
            for c in called {
                if generic.contains(c.as_str()) {
                    edges.entry(name.to_string()).or_default().insert(c);
                }
            }
        }
        // 到達可能性で強連結成分に分ける（総称の関数の数は小さい）。
        let nodes: Vec<&str> = generic.iter().copied().collect();
        let reach = |from: &str, edges: &HashMap<String, HashSet<String>>| -> HashSet<String> {
            let mut seen = HashSet::new();
            let mut work: Vec<String> = edges
                .get(from)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            while let Some(n) = work.pop() {
                if seen.insert(n.clone()) {
                    if let Some(s) = edges.get(&n) {
                        work.extend(s.iter().cloned());
                    }
                }
            }
            seen
        };
        let reaches: HashMap<&str, HashSet<String>> =
            nodes.iter().map(|n| (*n, reach(n, &edges))).collect();
        let mut comp: HashMap<String, usize> = HashMap::new();
        let mut next = 0usize;
        for &n in &nodes {
            if !reaches[n].contains(n) {
                continue; // 循環していない
            }
            if comp.contains_key(n) {
                continue;
            }
            for &m in &nodes {
                if reaches[n].contains(m) && reaches[m].contains(n) {
                    comp.insert(m.to_string(), next);
                }
            }
            next += 1;
        }
        self.scc = comp;
    }

    /// 関数のシグネチャ。`recv` は `impl` の中のとき（型の名前と、`impl` の型）。`outer` は `impl` の型引数。
    fn signature(
        &mut self,
        f: &FnDecl,
        recv: Option<(&str, &Type)>,
        outer: &[(String, Vec<Prop>)],
        outer_tb: &[(String, String)],
    ) -> R<Sig> {
        let (own, own_tb) = self.generics(&f.generics, true)?;
        let mut generics = outer.to_vec();
        for (n, ps) in own {
            if generics.iter().any(|(m, _)| *m == n) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{n}` が `impl` の型引数と重なっている"),
                );
            }
            generics.push((n, ps));
        }
        let mut trait_bounds = outer_tb.to_vec();
        for tb in own_tb {
            if trait_bounds.iter().any(|(m, _)| *m == tb.0) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{}` が `impl` の型引数と重なっている", tb.0),
                );
            }
            trait_bounds.push(tb);
        }
        self.tparams = generics.iter().map(|(n, _)| n.clone()).collect();
        let r = self.signature_in(f, recv, generics, trait_bounds);
        self.tparams.clear();
        r
    }

    fn signature_in(
        &mut self,
        f: &FnDecl,
        recv: Option<(&str, &Type)>,
        generics: Vec<(String, Vec<Prop>)>,
        trait_bounds: Vec<(String, String)>,
    ) -> R<Sig> {
        let mut params = vec![];
        let mut seen = HashSet::new();
        for p in &f.params {
            if !seen.insert(p.name.clone()) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("引数 `{}` が二重にある", p.name),
                );
            }
            params.push((p.name.clone(), self.resolve(&p.ty, Pos::Param)?));
        }
        let has_self = params.first().is_some_and(|(n, _)| n == "self");
        if has_self {
            let Some((tn, ity)) = recv else {
                return self.err(
                    ErrorKind::TypeMismatch,
                    "`self` を受け手に書けるのは `impl` の中のメソッドだけ",
                );
            };
            let want = self.resolve(ity, Pos::Other)?;
            let inner = match &params[0].1 {
                Ty::Ref(_, i) => (**i).clone(),
                t => t.clone(),
            };
            if inner != want {
                return self.err(
                    ErrorKind::TypeMismatch,
                    format!(
                        "受け手 `self` の型は `{w}`・`&{w}`・`&mut {w}` のどれかでなければならない（`{}` は違う。型 `{tn}`）",
                        show(&params[0].1),
                        w = show(&want),
                    ),
                );
            }
        }
        let ret = match &f.ret {
            None => Ty::Unit,
            Some(t) => self.resolve(t, Pos::Ret)?,
        };
        if contains_ref(&ret) {
            let n = params
                .iter()
                .filter(|(_, t)| contains_ref_outside_fn(t))
                .count();
            if n != 1 {
                return self.err(
                    ErrorKind::RefReturnBorrowCount,
                    format!(
                        "参照を返せるのは、借用の引数（型のどこかに参照を含む引数。受け手を含む）がちょうど 1 つの関数だけだが、{n} 個ある（ADR 0003 の規則 3）"
                    ),
                );
            }
        }
        let effects = self.effect_terms(&f.effects, &params)?;
        Ok(Sig {
            generics,
            trait_bounds,
            params,
            ret,
            effects_none: f.effects.is_empty(),
            effects,
            has_self,
        })
    }

    /// 標準ライブラリの最小部分（`stdlib.rs` の表）を関数とメソッドの表に入れる。
    fn register_std(&mut self) -> R<()> {
        let saved = self.span;
        // 表のシグネチャは OS 層の型を名指しする。核の検査（`OsItemInCore`）は利用者のコードにだけかける。
        let core = std::mem::replace(&mut self.core, false);
        for (layer, sig, effects) in crate::stdlib::FREE_FNS {
            let (name, s) = self.std_sig(&crate::stdlib::free_source(*layer, sig, effects))?;
            self.fns.insert(name, s);
        }
        for (layer, ns, sig, effects) in crate::stdlib::NAMESPACED_FNS {
            let (name, s) = self.std_sig(&crate::stdlib::free_source(*layer, sig, effects))?;
            let key = format!("{ns}.{name}");
            if *layer == crate::stdlib::Layer::Os {
                self.os_items.fns.insert(key.clone());
            }
            self.fns.insert(key, s);
        }
        for (layer, head, fns) in crate::stdlib::IMPLS {
            let file = self.std_parse(&crate::stdlib::impl_source(*layer, head, fns))?;
            if let Some(Item::Impl(i)) = file.items.first() {
                self.register_impl(i, true)?;
                if *layer == crate::stdlib::Layer::Os {
                    let tn = self.impl_type_name(i);
                    for f in &i.fns {
                        self.os_items.methods.insert((tn.clone(), f.name.clone()));
                    }
                }
            }
        }
        self.core = core;
        self.span = saved;
        Ok(())
    }

    /// 原始操作（`raw.名前`）を関数の表に入れる（ADR 0016 の決定 1）。呼べるかどうかは呼ぶ場所で決める。
    fn register_raw(&mut self) -> R<()> {
        let saved = self.span;
        let core = std::mem::replace(&mut self.core, false);
        for d in self.raw_defs.clone() {
            if d.premise.trim().is_empty() {
                return self.err(
                    ErrorKind::Unsupported,
                    format!(
                        "原始操作 `{}` の前提の文が空（空にできない。ADR 0016 の決定 1）",
                        d.signature
                    ),
                );
            }
            let src = crate::stdlib::raw_source(d.layer, &d.signature, &d.effects, &d.requires);
            let (name, s) = self.std_sig(&src)?;
            let key = format!("raw.{name}");
            if self.fns.contains_key(&key) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("原始操作 `{key}` が二重にある"),
                );
            }
            if d.layer == crate::stdlib::Layer::Os {
                self.os_items.fns.insert(key.clone());
            }
            self.raw_ops.insert(
                key.clone(),
                RawInfo {
                    premise: d.premise.clone(),
                    delegated: d.effects == crate::stdlib::DELEGATED,
                },
            );
            self.fns.insert(key, s);
        }
        self.core = core;
        self.span = saved;
        Ok(())
    }

    /// 原始操作 `key` をいまの場所で使えるか（ADR 0016 の決定 2）。使えなければ `RawOutsideUnchecked`。
    fn raw_allowed(&self, key: &str, as_value: bool) -> R<()> {
        let why = if as_value {
            "原始操作は関数の値として使えない（`unchecked` の関数の本体で直接呼ぶ）"
        } else if self.refine || self.contract {
            "契約の式（`requires`・`ensures`・篩型の条件）の中では原始操作を呼べない"
        } else if self.closure_depth > 0 {
            "クロージャの本体の中では原始操作を呼べない（クロージャは関数の外へ持ち出されうる）"
        } else if !self.raw_fn {
            "原始操作は `unchecked` の節を持つ関数の本体でだけ呼べる"
        } else {
            return Ok(());
        };
        self.err(ErrorKind::RawOutsideUnchecked, format!("`{key}`: {why}"))
    }

    fn std_parse(&self, src: &str) -> R<File> {
        crate::parser::parse_std(src).map_err(|e| {
            Error::new(
                ErrorKind::Unsupported,
                self.span,
                format!(
                    "標準ライブラリの表が読めない（処理系の誤り）: {}\n{src}",
                    e.message
                ),
            )
        })
    }

    fn std_sig(&mut self, src: &str) -> R<(String, Sig)> {
        let file = self.std_parse(src)?;
        let Some(Item::Fn(f)) = file.items.first() else {
            return self.err(ErrorKind::Unsupported, "標準ライブラリの表が読めない");
        };
        let sig = self.signature(f, None, &[], &[])?;
        Ok((f.name.clone(), sig))
    }

    /// 仮の関数（標準ライブラリが入るまでのテスト用）を関数の表に入れる。
    fn register_primitives(&mut self) -> R<()> {
        for p in self.prims.clone() {
            let src = format!(
                "module p.p\n  targets server\n  trust none\n\nfn {}\n  intent \"p\"\n  requires none\n  ensures none\n  effects none\n{{\n}}\n",
                p.signature
            );
            let file = parse(&src)?;
            let Some(Item::Fn(f)) = file.items.first() else {
                continue;
            };
            if self.fns.contains_key(&f.name) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("仮の関数 `{}` が二重にある", f.name),
                );
            }
            let mut sig = self.signature(f, None, &[], &[])?;
            let fx: Vec<Effect> = p
                .effects
                .iter()
                .map(|e| Effect::Category(e.split('.').map(str::to_string).collect()))
                .collect();
            sig.effects = self.effect_terms(&fx, &sig.params)?;
            sig.effects_none = fx.is_empty();
            self.fns.insert(f.name.clone(), sig);
        }
        Ok(())
    }

    fn check_refinement(&mut self, t: &TypeDecl) -> R<()> {
        self.span = t.span;
        let Some(TypeDef::Alias(base)) = self.types.get(&t.name).cloned() else {
            return Ok(());
        };
        self.begin();
        self.scopes = vec![HashMap::from([(
            "self".to_string(),
            Local {
                ty: base,
                mutable: false,
                param: true,
            },
        )])];
        self.refine = true;
        let r = self.expect(&Ty::Bool, &t.cond, "篩型の条件");
        self.refine = false;
        r?;
        self.finish()?;
        self.flush_typed();
        Ok(())
    }

    fn check_fn(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.env = sig.generics.iter().cloned().collect();
        self.cur_trait_bounds = sig.trait_bounds.clone();
        self.tparams = sig.generics.iter().map(|(n, _)| n.clone()).collect();
        let r = self.check_fn_in(f, sig);
        self.env.clear();
        self.cur_trait_bounds.clear();
        self.tparams.clear();
        self.skip = None;
        r
    }

    fn check_fn_in(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        if f.unchecked.is_some() && !self.base_module {
            return Err(Error::new(
                ErrorKind::UncheckedOutsideTrustBase,
                f.span,
                format!(
                    "関数 `{}` に `unchecked` の節があるが、このモジュールは `trust base` ではない（`unchecked` は `trust base` のモジュールの関数にだけ書ける）",
                    f.name
                ),
            ));
        }
        self.raw_fn = f.unchecked.is_some();
        self.raw_used.clear();
        self.raw_delegated = false;
        let r = self.check_fn_body(f, sig);
        self.raw_fn = false;
        r
    }

    fn check_fn_body(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.skip = None;
        self.annots.clear();
        self.cur_fn = f.name.clone();
        self.body_once(f, sig)?;
        self.flush_typed();
        let mut fin = std::mem::take(&mut self.fin_closures);
        self.closures.append(&mut fin);
        self.span = f.span;
        self.record_report(f, sig);
        self.match_effects(f, sig)?;
        // 型注釈の要否: 注釈を 1 つ外して検査し直し、同じ型に決まれば冗長
        let annots = self.annots.clone();
        for (i, (span, declared)) in annots.iter().enumerate() {
            self.skip = Some(i);
            let again = self.body_once(f, sig);
            if again.is_ok() && self.skipped.as_ref() == Some(declared) {
                return Err(Error::new(
                    ErrorKind::RedundantAnnotation,
                    *span,
                    format!(
                        "型注釈 `{}` を外しても同じ型に決まるので、型注釈を取り除く",
                        show(declared)
                    ),
                ));
            }
        }
        Ok(())
    }

    /// 関数の記録（使った原始操作と、宣言だけの効果）を残す。
    fn record_report(&mut self, f: &FnDecl, sig: &Sig) {
        let mut declared: Vec<String> = vec![];
        for t in &sig.effects {
            match t {
                EffTerm::Cat(n) => declared.extend(self.table.atoms(n)),
                EffTerm::Of(i) => declared.push(format!("of({})", sig.params[*i].0)),
            }
        }
        let mut inferred: Vec<String> = vec![];
        for a in &self.inferred {
            match a {
                Atom::Cat(n) => inferred.extend(self.table.atoms(n)),
                Atom::Of(x) => inferred.push(format!("of({x})")),
            }
        }
        let mut not_inferred: Vec<String> = vec![];
        for e in declared {
            if !inferred.contains(&e) && !not_inferred.contains(&e) {
                not_inferred.push(e);
            }
        }
        let raw_ops = self
            .raw_used
            .iter()
            .map(|k| (k.clone(), self.raw_ops[k].premise.clone()))
            .collect();
        self.reports.insert(
            fn_key(f),
            FnReport {
                raw_ops,
                delegated: self.raw_delegated,
                not_inferred,
            },
        );
    }

    fn body_once(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.span = f.span;
        self.begin();
        let mut scope = HashMap::new();
        for (n, t) in &sig.params {
            scope.insert(
                n.clone(),
                Local {
                    ty: t.clone(),
                    mutable: false,
                    param: true,
                },
            );
        }
        self.scopes = vec![scope];
        self.fn_params = sig
            .params
            .iter()
            .filter(|(_, t)| matches!(t, Ty::Fn(..)))
            .map(|(n, _)| n.clone())
            .collect();
        self.fn_borrowed = self.fn_params.iter().cloned().collect();
        self.ret = sig.ret.clone();
        // 契約の式は効果の推論に数えない（捨てる枠で検査する）
        self.eff_stack.push(vec![]);
        self.track = false;
        self.contract = true;
        let mut contract_res: R<()> = Ok(());
        if let Some(r) = &f.requires {
            contract_res = self.expect(&Ty::Bool, r, "`requires`").map(|_| ());
        }
        if let (Ok(()), Some(e)) = (&contract_res, &f.ensures) {
            self.result = Some(sig.ret.clone());
            self.in_ensures = true;
            contract_res = self.expect(&Ty::Bool, e, "`ensures`").map(|_| ());
            self.in_ensures = false;
            self.result = None;
        }
        self.contract = false;
        self.track = true;
        contract_res?;
        self.eff_stack.pop();
        self.block(&f.body)?;
        if !self.flow.dead {
            self.ens_check()?;
        }
        self.inferred = self.eff_stack[0].clone();
        self.span = f.span;
        if sig.ret != Ty::Unit && !block_returns(&f.body) {
            return self.err(
                ErrorKind::MissingReturn,
                format!(
                    "関数 `{}` は `{}` を返すので、すべての道が `return` で終わらなければならない",
                    f.name,
                    show(&sig.ret)
                ),
            );
        }
        self.finish()
    }

    // ---------- 文 ----------

    fn block(&mut self, b: &Block) -> R<()> {
        self.scopes.push(HashMap::new());
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.pop_scope();
        Ok(())
    }

    /// 見えなくなった名前を `fn_borrowed` から外す。
    fn sync_borrowed(&mut self) {
        let live: HashSet<&String> = self.scopes.iter().flat_map(|s| s.keys()).collect();
        self.fn_borrowed.retain(|k| live.contains(k));
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.sync_borrowed();
        // 見えなくなった変数の移動の記録は捨てる
        let live: HashSet<&String> = self.scopes.iter().flat_map(|s| s.keys()).collect();
        self.flow
            .moved
            .retain(|k, _| live.contains(&k.split('.').next().unwrap_or(k).to_string()));
        self.flow
            .maybe
            .retain(|k, _| live.contains(&k.split('.').next().unwrap_or(k).to_string()));
        self.flow.loans.retain(|l| live.contains(&l.holder));
    }

    // ---------- 移動（ADR 0012 の決定 1 の 1・2） ----------

    /// 場所 `path` を使えるか。移動済み（部分的な移動を含む）ならエラー。
    /// 型が決まっていない値を移動したかもしれない場所は、型が決まってから調べ直す。
    fn use_place(&mut self, path: &str) -> R<()> {
        if !self.track || self.refine {
            return Ok(());
        }
        let near = |k: &str| {
            k == path || k.starts_with(&format!("{path}.")) || path.starts_with(&format!("{k}."))
        };
        let is_fn = {
            let root = path.split('.').next().unwrap_or(path);
            self.lookup(root)
                .is_some_and(|l| matches!(self.zonk(&l.ty), Ty::Fn(..)))
        };
        let msg = |k: &str, at: Span| {
            if k == path && is_fn {
                format!(
                    "`{path}` は {} 行目で移動していて、ここでは使えない（関数の値は、引数として渡しても移動しない。別名 `let h = {path};` や、クロージャへの `move` のとらえ込みが移動になる。元の名前をそのまま使う）",
                    at.line
                )
            } else if k == path {
                format!(
                    "`{path}` は {} 行目で移動していて、ここでは使えない（移動した値は、再び代入するまで使えない。複製が要るなら `.clone()` を使う）",
                    at.line
                )
            } else if k.len() > path.len() {
                format!(
                    "`{path}` の一部 `{k}` は {} 行目で移動していて、`{path}` の全体はここでは使えない（他のフィールドは使える）",
                    at.line
                )
            } else {
                format!(
                    "`{k}` は {} 行目で移動していて、その一部 `{path}` もここでは使えない",
                    at.line
                )
            }
        };
        if let Some((k, at)) = self.flow.moved.iter().find(|(k, _)| near(k)) {
            let m = msg(k, *at);
            return self.err(ErrorKind::UseAfterMove, m);
        }
        let hit = self
            .flow
            .maybe
            .iter()
            .find(|(k, _)| near(k))
            .map(|(k, (at, ty))| (msg(k, *at), ty.clone()));
        if let Some((m, ty)) = hit {
            let e = Error::new(ErrorKind::UseAfterMove, self.span, m);
            self.moved_pending.push((ty, e));
        }
        Ok(())
    }

    // ---------- 借用（ADR 0012 の決定 1 の 3・4） ----------

    /// 場所 `path` への書き込み（`write`）または読み出しで、重なる貸しを使えなくする。
    /// 読み出しは `&mut` の貸しだけ、書き込み・`&mut` の借用・移動・代入はすべての貸しを使えなくする。
    fn touch(&mut self, path: &str, write: bool, op: &str) {
        if !self.track || self.refine {
            return;
        }
        self.note_capture(path, write);
        let at = self.span;
        self.flow.loans.touch(path, write, at, op);
    }

    /// クロージャの外側で宣言された変数の場所 `path` が使われた。いま検査中のクロージャのうち、
    /// その変数が外側にあるものすべてに、とらえたと記録する。
    fn note_capture(&mut self, path: &str, write: bool) {
        if self.frames.is_empty() {
            return;
        }
        let root = path.split('.').next().unwrap_or(path);
        let Some(idx) = self.scopes.iter().rposition(|s| s.contains_key(root)) else {
            return;
        };
        for f in &mut self.frames {
            if idx >= f.base {
                continue;
            }
            match f.caps.iter_mut().find(|c| c.name == root) {
                Some(c) => {
                    c.write |= write;
                    match c.paths.iter_mut().find(|p| p.0 == path) {
                        Some(p) => p.1 |= write,
                        None => c.paths.push((path.to_string(), write)),
                    }
                }
                None => f.caps.push(Cap {
                    name: root.to_string(),
                    write,
                    paths: vec![(path.to_string(), write)],
                }),
            }
        }
    }

    /// 変数 `n` が貸しを持つなら、使えるかを調べて、使われたと記録する。
    fn check_holder(&mut self, n: &str) -> R<()> {
        if !self.track || self.refine || !self.flow.loans.iter().any(|l| l.holder == n) {
            return Ok(());
        }
        self.used.insert(n.to_string());
        let Some(l) = self
            .flow
            .loans
            .iter()
            .find(|l| l.holder == n && l.dead.is_some())
            .cloned()
        else {
            return Ok(());
        };
        let Some((at, why)) = l.dead else {
            return Ok(());
        };
        match why {
            Dead::Conflict(op) => self.err(
                ErrorKind::BorrowConflict,
                format!(
                    "`{n}` は `{}` を借りているが、{} 行目の{op}で `{}` が使われたため、ここでは使えない（借りた値は、`{}` を変更・移動する前に使い終える。あとで要る値は先に取り出すか `.clone()` で複製しておく）",
                    l.path, at.line, l.path, l.path
                ),
            ),
            Dead::Temp => self.err(
                ErrorKind::TempBorrow,
                format!(
                    "`{n}` は一時的な値を借りていて、その値は {} 行目の文の終わりで破棄されたので、ここでは使えない（一時的な値を先に `let` の変数に入れてから借りる）",
                    at.line
                ),
            ),
        }
    }

    fn log_acc(&mut self, path: &str, kind: AccKind) {
        if self.track && !self.refine {
            self.log.push(Acc {
                path: path.to_string(),
                kind,
            });
        }
    }

    /// 1 つの呼び出しの中（`mark` 以降の記録）で、`&mut` の借用が他の使い方と重なっていないか。
    fn check_overlap(&mut self, mark: usize) -> R<()> {
        if !self.track || self.refine || mark >= self.log.len() {
            return Ok(());
        }
        let l = self.log[mark..].to_vec();
        for (i, a) in l.iter().enumerate() {
            for (j, b) in l.iter().enumerate() {
                if i == j || !overlaps(&a.path, &b.path) {
                    continue;
                }
                let bad = matches!(
                    (a.kind, b.kind),
                    (AccKind::Mut, _) | (AccKind::Move, AccKind::Shared)
                );
                if !bad {
                    continue;
                }
                let what = match b.kind {
                    AccKind::Read => "読み出し",
                    AccKind::Shared => "借用",
                    AccKind::Mut => "`&mut` の借用",
                    AccKind::Move => "移動",
                };
                let (own, other) = if a.kind == AccKind::Mut {
                    (&a.path, &b.path)
                } else {
                    (&b.path, &a.path)
                };
                let msg = if a.kind == AccKind::Mut {
                    format!(
                        "`{}` を `&mut` で借りる引数と、同じ呼び出しの中の `{}` の{what}が重なっている（変更の最中に同じ場所を読めない）。先に値を変数に取り出して分ける（例: `xs.push(xs.len());` は `let n = xs.len(); xs.push(n);`）",
                        a.path, other
                    )
                } else {
                    format!(
                        "`{other}` の移動と、同じ呼び出しの中の `{own}` の借用が重なっている（借りた値が、渡した先で使われる間に移動してしまう）。先に値を変数に取り出して分ける"
                    )
                };
                return self.err(ErrorKind::BorrowOverlap, msg);
            }
        }
        Ok(())
    }

    /// 借用 `&place`・`&mut place` の貸し。場所なら場所そのものと、根の変数が持つ貸し。一時的な値なら `TEMP`。
    fn borrow_srcs(
        &self,
        inner: &Expr,
        mutable: bool,
        inner_res: &[Src],
        inner_ty: &Ty,
    ) -> Vec<Src> {
        let inner = strip_paren(inner);
        match place_path(inner) {
            Some((root, path)) if self.lookup(&root).is_some() => {
                let mut v = vec![(path, mutable)];
                v.extend(self.flow.loans.held(&root));
                v
            }
            _ => {
                let mut v = vec![(TEMP.to_string(), mutable)];
                if contains_ref(&self.zonk(inner_ty)) {
                    v.extend(inner_res.iter().cloned());
                }
                v
            }
        }
    }

    /// 呼び出しの値が参照を含むなら、その貸しを `srcs` にする。
    fn set_res(&mut self, ret: &Ty, srcs: Vec<Src>) {
        self.res.clear();
        if contains_ref(&self.zonk(ret)) {
            for s in srcs {
                if !self.res.contains(&s) {
                    self.res.push(s);
                }
            }
        }
    }

    /// 現在のスコープで宣言された、参照を含む型の名前に、貸し `srcs` を持たせる。
    fn adopt(&mut self, names: &[String], srcs: &[Src], stmt_match: bool) -> R<()> {
        if !self.track || self.refine {
            return Ok(());
        }
        let at = self.span;
        for n in names {
            let Some(l) = self.lookup(n) else { continue };
            if !contains_ref(&self.zonk(&l.ty)) {
                continue;
            }
            for (path, mutable) in srcs {
                let dead = if path == TEMP && stmt_match {
                    Some((at, Dead::Temp))
                } else {
                    None
                };
                self.flow.loans.lend(n, path, *mutable, dead);
            }
        }
        Ok(())
    }

    /// 呼び出しの戻り値の型が、置き換えの規則（ADR 0012 の決定 1 の 4）を満たさない理由。
    fn subst_problem(&self, ps: &[Ty], ret: &Ty) -> Option<String> {
        for p in ps {
            if let Ty::Ref(true, i) = self.zonk(p) {
                if contains_ref(&i) {
                    return Some(format!(
                        "`&mut` で渡す先の型 `{}` が参照を含んでいる（参照を入れ物に入れて書き換える形は、まだ扱えない）",
                        self.sh(&i)
                    ));
                }
            }
        }
        let ret = self.zonk(ret);
        if !contains_ref(&ret) {
            return None;
        }
        let top_ok = match &ret {
            Ty::Ref(_, i) => !contains_ref(i),
            Ty::Named(n, a) if n == "Option" && a.len() == 1 => {
                matches!(&a[0], Ty::Ref(_, i) if !contains_ref(i))
            }
            _ => false,
        };
        if !top_ok {
            return Some(format!(
                "戻り値の型 `{}` の参照は、戻り値そのものか、最も外側の `Option<>` の中にしか置けない",
                show(&ret)
            ));
        }
        let n = ps
            .iter()
            .filter(|p| contains_ref_outside_fn(&self.zonk(p)))
            .count();
        if n != 1 {
            return Some(format!(
                "参照を返す呼び出しは、借用の引数（参照を含む型の引数）がちょうど 1 つでなければならないが、{n} 個ある"
            ));
        }
        None
    }

    /// 変数 `root` が、いま検査しているクロージャの外側で宣言されたものか。
    fn is_outer(&self, root: &str) -> bool {
        self.scopes[..self.closure_base.min(self.scopes.len())]
            .iter()
            .any(|s| s.contains_key(root))
    }

    /// 値で持つ場所 `path`（根は変数 `root`）を移動したと記録する。参照を通した場所は移動できない。
    fn record_move(&mut self, root: &str, path: &str) -> R<()> {
        let Some(l) = self.lookup(root) else {
            return Ok(());
        };
        if matches!(self.shallow(&l.ty), Ty::Ref(..)) {
            let what = if root == path {
                format!("`{root}` は参照なので、参照先の値を移動できない")
            } else {
                format!("`{path}` は参照 `{root}` を通して届く場所なので、移動できない")
            };
            return self.err(
                ErrorKind::MoveFromRef,
                format!("{what}（複製は `.clone()`。`Copyable` な型なら複製として渡せる）"),
            );
        }
        self.log_acc(path, AccKind::Move);
        if self.is_outer(root) {
            return self.err(
                ErrorKind::ClosureMoveCaptured,
                format!(
                    "`{path}` はクロージャの外で宣言された値で、クロージャがとらえている。本体の中で移動すると、とらえた値が呼び出しのたびに失われるので書けない（値が要るなら `.clone()` で複製する）"
                ),
            );
        }
        self.touch(path, true, "移動");
        let at = self.span;
        self.flow.moved.entry(path.to_string()).or_insert(at);
        Ok(())
    }

    /// `e` が、移動しない関数の値（関数の型の引数・その別名・名前で参照した関数）の名前か。
    fn borrowed_fn(&self, e: &Expr) -> bool {
        let Expr::Name(n) = strip_paren(e) else {
            return false;
        };
        match self.lookup(n) {
            Some(_) => self.fn_borrowed.contains(n),
            None => self.fns.contains_key(n),
        }
    }

    /// 式 `e`（型 `t`）の値が、移動する位置に置かれた。`Copyable` でない値を持つ場所なら、移動を記録する。
    fn consume(&mut self, e: &Expr, t: &Ty) -> R<()> {
        // 借りている関数の値（引数・その別名）は、渡しても別名にしても移動しない
        if self.borrowed_fn(e) {
            return Ok(());
        }
        if self.contract {
            if let Some((root, _)) = place_path(e) {
                let ty = self.zonk(t);
                if self.lookup(&root).is_some() && self.holds(&ty, Prop::Copyable, 0) == Some(false)
                {
                    return self.err(
                        ErrorKind::ContractMove,
                        format!(
                            "契約の式の中で `{root}` の値を移動できない（契約は本体の前後で値を読むだけ。参照で渡すか、`Copyable` な値だけを値で渡す）"
                        ),
                    );
                }
            }
        }
        if !self.track || self.refine {
            return Ok(());
        }
        let Some((root, path)) = place_path(e) else {
            return Ok(());
        };
        if self.lookup(&root).is_none() {
            return Ok(());
        }
        let ty = self.zonk(t);
        if self.skip.is_none() {
            // 移動かどうか（`Copyable` でないか）は、型が決まった後の `flush_typed` で決める。
            self.rec_moves.push((addr(e), ty.clone()));
        }
        match self.holds(&ty, Prop::Copyable, 0) {
            Some(false) => {}
            None => {
                // 型が決まっていない値: 移動かもしれない。型が決まってから、使用を調べ直す
                let is_ref = self
                    .lookup(&root)
                    .is_some_and(|l| matches!(self.shallow(&l.ty), Ty::Ref(..)));
                if !is_ref && !self.is_outer(&root) {
                    let at = self.span;
                    self.flow.maybe.entry(path).or_insert((at, ty));
                }
                return Ok(());
            }
            Some(true) => return Ok(()),
        }
        self.record_move(&root, &path)
    }

    /// 代入で場所を初期化し直す。全体が移動済みの変数のフィールドには代入できない。
    fn reinit(&mut self, root: &str, path: &str) -> R<()> {
        if !self.track || self.refine || self.is_outer(root) {
            return Ok(());
        }
        if let Some((k, at)) = self
            .flow
            .moved
            .iter()
            .find(|(k, _)| path.starts_with(&format!("{k}.")))
        {
            return self.err(
                ErrorKind::UseAfterMove,
                format!(
                    "`{k}` は {} 行目で移動していて、その一部 `{path}` へは代入できない（先に `{k}` 全体へ代入する）",
                    at.line
                ),
            );
        }
        let sub = format!("{path}.");
        self.flow
            .moved
            .retain(|k, _| k != path && !k.starts_with(&sub));
        self.flow
            .maybe
            .retain(|k, _| k != path && !k.starts_with(&sub));
        Ok(())
    }

    fn expect(&mut self, expected: &Ty, e: &Expr, what: &str) -> R<()> {
        self.expect_ty(expected, e, what).map(|_| ())
    }

    fn expect_ty(&mut self, expected: &Ty, e: &Expr, what: &str) -> R<Ty> {
        let t = self.expr(e)?;
        if !self.coerce(expected, &t, e) {
            return self.err(
                self.mismatch_kind(expected, &t),
                format!(
                    "{what}は `{}` でなければならないが、`{}` である{}",
                    self.sh(expected),
                    self.sh(&t),
                    self.check_hint(expected, &t)
                ),
            );
        }
        Ok(t)
    }

    fn check_hint(&self, expected: &Ty, actual: &Ty) -> &'static str {
        let (ex, ac) = (self.zonk(expected), self.zonk(actual));
        if self
            .refinement_base(&ex)
            .is_some_and(|b| self.zonk(&b) == ac)
        {
            "（基の型から篩型への変換は、リテラルの定数か `T.check` に限る）"
        } else if self.refine_mut_mismatch(&ex, &ac) {
            "（`&mut` で篩型を基の型として渡すと、篩型の条件を壊す値を書き込めてしまう。`&` なら渡せる）"
        } else if matches!(ex, Ty::Int) && ac == Ty::Float
            || matches!(ex, Ty::Float) && ac == Ty::Int
        {
            "（`Int` と `Float` の暗黙の変換はない）"
        } else {
            ""
        }
    }

    fn stmt(&mut self, s: &Stmt) -> R<()> {
        self.span = s.span;
        match &s.kind {
            StmtKind::Let {
                mutable,
                pattern,
                ty,
                value,
            } => {
                let closure = matches!(strip_paren(value), Expr::Closure { .. });
                self.defer = closure;
                let vt = self.expr(value);
                self.defer = false;
                let vt = vt?;
                let let_srcs = std::mem::take(&mut self.res);
                let vt_moved = vt.clone();
                self.fn_value_form(value, &vt)?;
                if closure {
                    if let Pattern::Bind(n) = pattern {
                        let effs = std::mem::take(&mut self.last_closure);
                        self.local_eff.insert(n.clone(), effs);
                        if let Some(c) = self.cur_closures.last_mut() {
                            c.bound = Some(n.clone());
                        }
                    } else {
                        // 束縛されないクロージャは呼べない。効果は起こらない。
                        self.last_closure.clear();
                    }
                } else if matches!(self.zonk(&vt), Ty::Fn(..))
                    && matches!(strip_paren(value), Expr::Name(_))
                {
                    // 別名は元の関数の効果を引き継ぐ（引数の別名なら `of(f)` のまま）。
                    if let Pattern::Bind(n) = pattern {
                        let effs = self.fn_effects(value);
                        self.local_eff.insert(n.clone(), effs);
                    }
                }
                self.span = s.span;
                let fin = match ty {
                    None => vt,
                    Some(t) => {
                        let d = self.resolve(t, Pos::Other)?;
                        let i = self.annot_n;
                        self.annot_n += 1;
                        if self.skip == Some(i) {
                            self.skipped = Some(vt.clone());
                            vt
                        } else {
                            if self.skip.is_none() {
                                self.annots.push((s.span, d.clone()));
                            }
                            if !self.coerce(&d, &vt, value) {
                                return self.err(
                                    self.mismatch_kind(&d, &vt),
                                    format!(
                                        "型注釈は `{}` だが、式の型は `{}` である{}",
                                        show(&d),
                                        self.sh(&vt),
                                        self.check_hint(&d, &vt)
                                    ),
                                );
                            }
                            d
                        }
                    }
                };
                if self.skip.is_none() {
                    self.rec_lets.push((s as *const Stmt as usize, fin.clone()));
                }
                if *mutable && contains_ref(&self.zonk(&fin)) {
                    return self.err(
                        ErrorKind::LetMutRef,
                        "型が参照を含む変数（`&T`・`Option<&T>` など）は `let mut` で宣言できない（参照を別の参照で束縛し直す操作はない）",
                    );
                }
                if *mutable && matches!(self.zonk(&fin), Ty::Fn(..)) {
                    return self.err(
                        ErrorKind::FnValueReassign,
                        "関数の型の変数は `let mut` で宣言できない（関数の値の再代入では効果を追えない。`let` で束縛し直す）",
                    );
                }
                if !matches!(pattern, Pattern::Wild) {
                    self.consume(value, &vt_moved)?;
                }
                self.bind(pattern, &fin, *mutable)?;
                if matches!(self.zonk(&vt_moved), Ty::Fn(..)) && self.borrowed_fn(value) {
                    if let Pattern::Bind(n) = pattern {
                        self.fn_borrowed.insert(n.clone());
                    }
                }
                let mut names = vec![];
                pat_names(pattern, &mut names);
                if *mutable && self.has_var(&fin) {
                    for n in &names {
                        if let Some(l) = self.lookup(n) {
                            let t = l.ty.clone();
                            self.let_pending.push((n.clone(), t, s.span));
                        }
                    }
                }
                if self.track
                    && let_srcs.iter().any(|(p, _)| p == TEMP)
                    && names.iter().any(|n| {
                        self.lookup(n)
                            .is_some_and(|l| contains_ref(&self.zonk(&l.ty)))
                    })
                {
                    self.span = s.span;
                    return self.err(
                        ErrorKind::TempBorrow,
                        "一時的な値への参照は、この文の終わりで使えなくなるので、変数に持てない（一時的な値を先に `let` の変数に入れてから借りる。例: `let l = make(); let r = l.get(0);`）",
                    );
                }
                self.adopt(&names, &let_srcs, false)
            }
            StmtKind::Assign { target, value } => {
                let root = place_root(target).map(str::to_string);
                self.skip_use = place_path(target).is_some();
                let tt = self.expr(target);
                self.skip_use = false;
                let tt = tt?;
                self.span = s.span;
                let Some(root) = root else {
                    return self.err(
                        ErrorKind::AssignToImmutable,
                        "代入できるのは変数とそのフィールドだけ",
                    );
                };
                if matches!(self.zonk(&tt), Ty::Fn(..)) {
                    return self.err(
                        ErrorKind::FnValueReassign,
                        "関数の値は再代入できない（引数への代入も同じ。`let` で束縛し直す）",
                    );
                }
                self.assignable_place(&root, target, true)?;
                self.span = s.span;
                let vt = self.expr(value)?;
                self.span = s.span;
                if !self.coerce(&tt, &vt, value) {
                    return self.err(
                        ErrorKind::TypeMismatch,
                        format!(
                            "代入先は `{}` だが、右辺は `{}` である{}",
                            self.sh(&tt),
                            self.sh(&vt),
                            self.check_hint(&tt, &vt)
                        ),
                    );
                }
                self.consume(value, &vt)?;
                if let Some((r, p)) = place_path(target) {
                    self.touch(&p, true, "代入");
                    self.reinit(&r, &p)?;
                }
                Ok(())
            }
            StmtKind::Expr(e) => self.expr(e).map(|_| ()),
            StmtKind::Return(v) => {
                let ret = self.ret.clone();
                match v {
                    None => {
                        self.ens_check()?;
                        self.flow.dead = true;
                        if !self.try_unify(&ret, &Ty::Unit) {
                            return self.err(
                                ErrorKind::TypeMismatch,
                                format!(
                                    "`{}` を返す関数で、値のない `return;` は書けない",
                                    self.sh(&ret)
                                ),
                            );
                        }
                        Ok(())
                    }
                    Some(e) if contains_ref(&self.zonk(&ret)) => {
                        // 参照を返す関数（ADR 0012 の決定 1）。`return 場所;` は場所への参照を返す
                        // ので、型は参照を外して比べる。どの場所を返せるかは所有権の検査で調べる。
                        let t = self.expr(e)?;
                        self.return_roots(e)?;
                        let (want, got) =
                            (strip_refs(&self.zonk(&ret)), strip_refs(&self.zonk(&t)));
                        if !self.coerce(&want, &got, e) {
                            return self.err(
                                ErrorKind::TypeMismatch,
                                format!(
                                    "戻り値は `{}` でなければならないが、`{}` である",
                                    self.sh(&ret),
                                    self.sh(&t)
                                ),
                            );
                        }
                        self.ens_check()?;
                        self.flow.dead = true;
                        Ok(())
                    }
                    Some(e) => {
                        let t = self.expect_ty(&ret, e, "戻り値")?;
                        self.consume(e, &t)?;
                        self.ens_check()?;
                        self.flow.dead = true;
                        Ok(())
                    }
                }
            }
            StmtKind::If(i) => self.if_stmt(i),
            StmtKind::While { cond, body } => {
                // ループは、本体の終わりの状態が次の周の頭の状態に合流する。外側の場所を本体で
                // 移動して再代入しないまま周の終わりに届くなら、次の周でその移動が二重になる。
                let head = self.flow.moved.clone();
                let head_loans = self.flow.loans.clone();
                let head_used = std::mem::take(&mut self.used);
                self.expect(&Ty::Bool, cond, "`while` の条件")?;
                let after_cond = self.flow.clone();
                self.block(body)?;
                if !self.flow.dead && self.track {
                    let hit = self
                        .flow
                        .moved
                        .iter()
                        .find(|(k, _)| {
                            !head.contains_key(*k)
                                && self.lookup(k.split('.').next().unwrap_or(k)).is_some()
                        })
                        .map(|(k, at)| (k.clone(), *at));
                    if let Some((k, at)) = hit {
                        return Err(Error::new(
                            ErrorKind::MovedInLoop,
                            at,
                            format!(
                                "`{k}` は {} 行目でループの本体の中で移動していて、周の終わりまでに再代入されない。次の周でまた使われるので移動済みになる（本体の中で再代入するか、ループの外で移動する）",
                                at.line
                            ),
                        ));
                    }
                }
                // 周の中で使えなくなった貸しの変数が、周の中で使われていれば、次の周で使えない
                if !self.flow.dead && self.track {
                    let hit = self
                        .flow
                        .loans
                        .iter()
                        .find(|l| {
                            l.dead.is_some()
                                && self.used.contains(&l.holder)
                                && head_loans.iter().any(|h| {
                                    h.holder == l.holder
                                        && h.path == l.path
                                        && h.mutable == l.mutable
                                        && h.dead.is_none()
                                })
                        })
                        .cloned();
                    if let Some(l) = hit {
                        let at = l.dead.map_or(self.span, |d| d.0);
                        return Err(Error::new(
                            ErrorKind::BorrowConflict,
                            at,
                            format!(
                                "`{}` は `{}` を借りているが、ループの本体のこの行で `{}` が変更・移動されている。`{}` は次の周でも使われるので、使えなくなっている（借りた値の使用をループの中で完結させるか、変更の前に使い終える）",
                                l.holder, l.path, l.path, l.holder
                            ),
                        ));
                    }
                }
                self.used.extend(head_used);
                // 0 周で抜ける経路と、本体の終わりから抜ける経路の合流
                let end = std::mem::take(&mut self.flow);
                self.flow = merge(after_cond, end);
                Ok(())
            }
            StmtKind::Match {
                scrutinee, arms, ..
            } => {
                let (st, by_ref) = self.scrutinee(scrutinee)?;
                let m_srcs = std::mem::take(&mut self.res);
                let st = self.peel(&st);
                let sp = s.span;
                let mut pats = vec![];
                self.match_moves(scrutinee, &st, by_ref, arms.iter().map(|a| &a.pattern))?;
                let start = self.flow.clone();
                let mut joined: Option<Flow> = None;
                for a in arms {
                    self.span = sp;
                    self.flow = start.clone();
                    self.scopes.push(HashMap::new());
                    self.by_ref = by_ref;
                    let r = self.pat(&a.pattern, &st);
                    self.by_ref = false;
                    r?;
                    let mut names = vec![];
                    pat_names(&a.pattern, &mut names);
                    self.adopt(&names, &m_srcs, true)?;
                    self.block(&a.body)?;
                    self.pop_scope();
                    pats.push(&a.pattern);
                    let f = std::mem::take(&mut self.flow);
                    joined = Some(match joined {
                        None => f,
                        Some(j) => merge(j, f),
                    });
                }
                self.flow = joined.unwrap_or(start);
                self.span = sp;
                self.exhaustive(&pats)
            }
        }
    }

    /// 参照を返す `return` の式 `e` が借りる場所の根が、参照の引数（またはそれを通して借りた値）か。
    fn return_roots(&mut self, e: &Expr) -> R<()> {
        if !self.track || self.refine {
            return Ok(());
        }
        let mut srcs: Vec<Src> = vec![];
        self.return_srcs(e, &mut srcs);
        for (path, _) in &srcs {
            if path == TEMP {
                return self.err(
                    ErrorKind::ReturnBorrowRoot,
                    "一時的な値の参照は、文の終わりで破棄されるので返せない（返せるのは、参照の引数を通して借りた場所だけ）",
                );
            }
            let root = path.split('.').next().unwrap_or(path);
            let ok = self
                .lookup(root)
                .is_some_and(|l| contains_ref(&self.zonk(&l.ty)));
            if !ok {
                return self.err(
                    ErrorKind::ReturnBorrowRoot,
                    format!(
                        "`{root}` はこの関数が所有する値なので、その参照は返せない（関数を抜けると破棄される。返せるのは、参照の引数を通して借りた場所だけ）"
                    ),
                );
            }
        }
        Ok(())
    }

    /// `ensures` が参照する、値で受け取った引数が、ここまでの道で移動されていないこと。
    /// 関数から戻る点（`return`・`?`・本体の終わり）で調べる。
    fn ens_check(&self) -> R<()> {
        if self.closure_base > 0 || self.ens_used.is_empty() || !self.track {
            return Ok(());
        }
        for (k, at) in &self.flow.moved {
            let root = k.split('.').next().unwrap_or(k);
            if self.ens_used.contains(root) {
                return Err(Error::new(
                    ErrorKind::EnsuresMoved,
                    *at,
                    format!(
                        "`{root}` は `ensures` が参照する引数だが、{} 行目で移動していて、関数から戻る道に届く（`ensures` は戻るときに引数の値を読むので、引数は移動しない。移さずに使うか `.clone()` する）",
                        at.line
                    ),
                ));
            }
        }
        Ok(())
    }

    fn return_srcs(&self, e: &Expr, out: &mut Vec<Src>) {
        let e = strip_paren(e);
        if let Some((root, path)) = place_path(e) {
            if self.lookup(&root).is_some() {
                out.push((path, false));
                out.extend(self.flow.loans.held(&root));
                return;
            }
        }
        if let Expr::Call { callee, args } = e {
            if let Expr::Name(n) = &**callee {
                if matches!(n.as_str(), "Some" | "Ok" | "Err") && self.lookup(n).is_none() {
                    for a in args {
                        let mut v = vec![];
                        self.return_srcs(a, &mut v);
                        out.extend(v);
                    }
                    return;
                }
            }
        }
        out.extend(self.res.iter().cloned());
    }

    fn if_stmt(&mut self, i: &IfStmt) -> R<()> {
        self.expect(&Ty::Bool, &i.cond, "`if` の条件")?;
        let start = self.flow.clone();
        self.block(&i.then)?;
        let then_end = std::mem::replace(&mut self.flow, start);
        match i.otherwise.as_deref() {
            None => {}
            Some(Else::If(n)) => self.if_stmt(n)?,
            Some(Else::Block(b)) => self.block(b)?,
        }
        let else_end = std::mem::take(&mut self.flow);
        self.flow = merge(then_end, else_end);
        Ok(())
    }

    /// 新しく宣言する変数の名前の、古い移動の記録を捨てる。
    fn forget(&mut self, n: &str) {
        self.flow
            .moved
            .retain(|k, _| k.split('.').next().unwrap_or(k) != n);
        self.flow
            .maybe
            .retain(|k, _| k.split('.').next().unwrap_or(k) != n);
        self.flow.loans.retain(|l| l.holder != n);
    }

    fn shadow_check(&self, n: &str) -> R<()> {
        if self.lookup(n).is_some() {
            return self.err(
                ErrorKind::ShadowedName,
                format!(
                    "名前 `{n}` は、同じ関数の外側ですでに使われている（名前の隠蔽は禁止。別の名前にする）"
                ),
            );
        }
        Ok(())
    }

    fn declare(&mut self, n: &str, ty: Ty, mutable: bool) -> R<()> {
        self.shadow_check(n)?;
        self.forget(n);
        // 新しい束縛は、別名と分かるまで借用の関数値ではない
        self.fn_borrowed.remove(n);
        self.scopes.last_mut().expect("scope").insert(
            n.to_string(),
            Local {
                ty,
                mutable,
                param: false,
            },
        );
        Ok(())
    }

    /// `let` の分解。名前・`_`・組だけ（場合分けは `match`）。
    fn bind(&mut self, p: &Pattern, t: &Ty, mutable: bool) -> R<()> {
        match p {
            Pattern::Wild => Ok(()),
            Pattern::Bind(n) => self.declare(n, t.clone(), mutable),
            Pattern::Tuple(ps) => {
                let ts = match self.shallow(t) {
                    Ty::Tuple(ts) => ts,
                    Ty::Var(_) => {
                        let ts: Vec<Ty> = ps.iter().map(|_| self.fresh()).collect();
                        let _ = self.unify(t, &Ty::Tuple(ts.clone()));
                        ts
                    }
                    other => {
                        return self.err(
                            ErrorKind::DestructureMismatch,
                            format!(
                                "組として分解しているが、式の型は `{}` である",
                                self.sh(&other)
                            ),
                        )
                    }
                };
                if ps.len() != ts.len() {
                    return self.err(
                        ErrorKind::DestructureMismatch,
                        format!(
                            "{} 個に分解しているが、組の要素は {} 個ある",
                            ps.len(),
                            ts.len()
                        ),
                    );
                }
                for (p, t) in ps.iter().zip(&ts) {
                    self.bind(p, t, mutable)?;
                }
                Ok(())
            }
            _ => self.err(
                ErrorKind::PatternMismatch,
                "`let` で分解できるのは名前・`_`・組だけ（場合分けは `match` で書く）",
            ),
        }
    }

    /// 代入先（または `&mut` で借りる場所）の根の変数が書き換えられるか。
    fn assignable_place(&self, root: &str, target: &Expr, assign: bool) -> R<()> {
        let Some(v) = self.lookup(root) else {
            return self.err(
                ErrorKind::UnknownName,
                format!("名前 `{root}` が見つからない"),
            );
        };
        let direct = matches!(target, Expr::Name(_) | Expr::SelfValue);
        match &v.ty {
            Ty::Ref(true, _) => Ok(()),
            Ty::Ref(false, _) => {
                if assign {
                    self.err(
                        ErrorKind::AssignThroughSharedRef,
                        format!(
                            "`{root}` は `{}` なので、{}書き換えられない（`&mut` で受け取る）",
                            show(&v.ty),
                            if direct { "" } else { "フィールドも" }
                        ),
                    )
                } else {
                    self.err(
                        ErrorKind::BorrowMutOfImmutable,
                        format!(
                            "`{root}` は `{}` なので、`&mut` では借りられない",
                            show(&v.ty)
                        ),
                    )
                }
            }
            _ if v.mutable => Ok(()),
            _ => {
                let why = if v.param {
                    "値で受け取った引数は書き換えられない（`&mut` で受け取る）"
                } else {
                    "`mut` のない `let` の変数は書き換えられない"
                };
                self.err(
                    if assign {
                        ErrorKind::AssignToImmutable
                    } else {
                        ErrorKind::BorrowMutOfImmutable
                    },
                    format!("`{root}`: {why}"),
                )
            }
        }
    }

    // ---------- 式 ----------

    fn expr(&mut self, e: &Expr) -> R<Ty> {
        self.expr_hint(e, None)
    }

    /// `hint` は、クロージャの引数の型を文脈から決めるための期待される関数の型。
    fn expr_hint(&mut self, e: &Expr, hint: Option<&Ty>) -> R<Ty> {
        let t = self.expr_hint_inner(e, hint)?;
        self.rec_expr(e, &t);
        // 参照を含まない値は、貸しを持たない
        let ref_name = matches!(strip_paren(e), Expr::Name(n)
            if self.lookup(n).is_some_and(|l| contains_ref(&self.zonk(&l.ty))));
        if !ref_name && !contains_ref(&self.zonk(&t)) {
            self.res.clear();
        }
        // 関数の値は入れ物（組・借用・構造体・列挙型・List など）に入れられない
        // （ADR 0011 決定 10）。入れると効果の追跡が漏れる。
        if matches!(
            e,
            Expr::Tuple(..) | Expr::Call { .. } | Expr::StructLit { .. }
        ) && self.nested_fn(&self.zonk(&t))
        {
            return self.err(
                ErrorKind::FnTypePosition,
                "関数の値は、組・構造体・列挙型・Option・Result・List などの中に入れられない（M1 では let の束縛・関数の引数・直接の呼び出しだけ）",
            );
        }
        Ok(t)
    }

    /// 関数の型が、入れ物の中に入っているか。
    fn nested_fn(&self, t: &Ty) -> bool {
        match t {
            Ty::Tuple(ts) | Ty::Named(_, ts) => ts.iter().any(|x| self.has_fn(x)),
            Ty::Ref(_, x) => self.has_fn(x),
            _ => false,
        }
    }

    fn has_fn(&self, t: &Ty) -> bool {
        matches!(t, Ty::Fn(..)) || self.nested_fn(t)
    }

    /// 関数の型の式は、名前・クロージャ・値の構築子だけ。
    fn fn_value_form(&self, e: &Expr, t: &Ty) -> R<()> {
        if matches!(self.zonk(t), Ty::Fn(..))
            && !matches!(
                strip_paren(e),
                Expr::Name(_) | Expr::Closure { .. } | Expr::Field { .. }
            )
        {
            return self.err(
                ErrorKind::FnTypePosition,
                "関数の値は、名前かクロージャとしてだけ受け渡せる（呼び出しや分岐の結果として返せない）",
            );
        }
        Ok(())
    }

    fn expr_hint_inner(&mut self, e: &Expr, hint: Option<&Ty>) -> R<Ty> {
        match e {
            Expr::Int(s) => {
                if s.parse::<i64>().is_err() {
                    return self.err(
                        ErrorKind::IntLiteralRange,
                        format!("整数のリテラル `{s}` が `Int`（64 ビットの符号付き整数）の範囲を超えている"),
                    );
                }
                Ok(Ty::Int)
            }
            Expr::Float(_) => Ok(Ty::Float),
            Expr::Str(_) => Ok(Ty::Str),
            Expr::Bool(_) => Ok(Ty::Bool),
            Expr::Name(n) => self.name(n),
            Expr::SelfValue => self.name("self"),
            Expr::Result => match self.result.clone() {
                Some(t) => Ok(t),
                None => self.err(
                    ErrorKind::UnknownName,
                    "`result` は `ensures` の中だけで使える",
                ),
            },
            Expr::Old(inner) => {
                let was = std::mem::replace(&mut self.in_old, true);
                let t = self.expr(inner);
                self.in_old = was;
                let t = t?;
                self.require(
                    &t,
                    Prop::Copyable,
                    ErrorKind::OldNotCopyable,
                    "`old(...)` の引数は暗黙に複製される型（`Copyable`）でなければならないが、`{}` である"
                        .into(),
                )?;
                Ok(t)
            }
            Expr::Paren(i) => self.expr_hint(i, hint),
            Expr::Tuple(es) => {
                let mut ts = vec![];
                let mut srcs: Vec<Src> = vec![];
                for e in es {
                    let t = self.expr(e)?;
                    srcs.append(&mut self.res);
                    self.consume(e, &t)?;
                    ts.push(t);
                }
                let t = Ty::Tuple(ts);
                self.set_res(&t, srcs);
                Ok(t)
            }
            Expr::Unary { op, expr } => self.unary(*op, expr),
            Expr::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            Expr::Field { expr, name } => self.field(expr, name),
            Expr::Call { callee, args } => {
                self.call_stack.push(None);
                let r = self.call(callee, args);
                let res = self.call_stack.pop().flatten();
                let t = r?;
                if let Some(res) = res {
                    if self.skip.is_none() {
                        self.rec_calls.push((addr(e), res));
                    }
                }
                Ok(t)
            }
            Expr::StructLit { path, fields } => self.struct_lit(path, fields),
            Expr::Try(inner) => self.try_op(inner),
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let (st, by_ref) = self.scrutinee(scrutinee)?;
                let m_srcs = std::mem::take(&mut self.res);
                let mut arm_srcs: Vec<Src> = vec![];
                let st = self.peel(&st);
                let sp = self.span;
                let r = self.fresh();
                let mut pats = vec![];
                self.match_moves(scrutinee, &st, by_ref, arms.iter().map(|a| &a.pattern))?;
                let start = self.flow.clone();
                let mut joined: Option<Flow> = None;
                for (i, a) in arms.iter().enumerate() {
                    self.flow = start.clone();
                    self.scopes.push(HashMap::new());
                    self.by_ref = by_ref;
                    let pr = self.pat(&a.pattern, &st);
                    self.by_ref = false;
                    pr?;
                    let mut names = vec![];
                    pat_names(&a.pattern, &mut names);
                    self.adopt(&names, &m_srcs, false)?;
                    let t = self.expr(&a.body)?;
                    // 腕の値は `match` の値として移る
                    let arm_res = self.res.clone();
                    self.consume(&a.body, &t)?;
                    self.res = arm_res;
                    self.span = sp;
                    // 腕が所有する束縛を借りた参照は、腕の外へ持ち出せない
                    if self.track && !self.refine {
                        for (path, _) in &self.res {
                            let root = path.split('.').next().unwrap_or(path);
                            if names.iter().any(|n| n == root)
                                && self
                                    .lookup(root)
                                    .is_some_and(|l| !contains_ref(&self.zonk(&l.ty)))
                            {
                                return self.err(
                                    ErrorKind::TempBorrow,
                                    format!(
                                        "`{root}` は `match` の腕が所有する値で、腕の終わりで破棄されるので、その参照は `match` の外へ出せない（腕の中で使い終えるか、値を `.clone()` で取り出す）"
                                    ),
                                );
                            }
                        }
                    }
                    arm_srcs.append(&mut self.res);
                    if i == 0 {
                        let _ = self.unify(&r, &t);
                    } else if !self.coerce(&r, &t, &a.body) {
                        return self.err(
                            ErrorKind::TypeMismatch,
                            format!(
                                "`match` の腕の型が揃わない: `{}` と `{}`",
                                self.sh(&r),
                                self.sh(&t)
                            ),
                        );
                    }
                    self.pop_scope();
                    pats.push(&a.pattern);
                    let f = std::mem::take(&mut self.flow);
                    joined = Some(match joined {
                        None => f,
                        Some(j) => merge(j, f),
                    });
                }
                self.flow = joined.unwrap_or(start);
                self.exhaustive(&pats)?;
                let live: Vec<Src> = arm_srcs
                    .into_iter()
                    .filter(|(p, _)| {
                        p == TEMP || self.lookup(p.split('.').next().unwrap_or(p)).is_some()
                    })
                    .collect();
                self.set_res(&r, live);
                Ok(r)
            }
            Expr::Closure {
                is_move,
                params,
                body,
            } => self.closure(*is_move, params, body, hint),
        }
    }

    fn try_op(&mut self, inner: &Expr) -> R<Ty> {
        let t = self.expr(inner)?;
        // `x?` は `x` の中身を取り出すので、場所なら移動する（早期に戻る経路は合流に加えない）
        self.consume(inner, &t)?;
        self.ens_check()?;
        let ret = self.ret.clone();
        let (want, value) =
            match self.peel(&t) {
                Ty::Named(n, a) if n == "Option" => {
                    let v = a[0].clone();
                    (Ty::Named("Option".into(), vec![self.fresh()]), v)
                }
                Ty::Named(n, a) if n == "Result" => {
                    let (v, e) = (a[0].clone(), a[1].clone());
                    (Ty::Named("Result".into(), vec![self.fresh(), e]), v)
                }
                Ty::Var(_) => return self.err(
                    ErrorKind::CannotInfer,
                    "`?` を付けた式の型が決まっていない（`Option` か `Result` でなければならない）",
                ),
                other => {
                    return self.err(
                        ErrorKind::TryOperand,
                        format!(
                            "`?` は `Option` か `Result` に使うが、`{}` である",
                            self.sh(&other)
                        ),
                    )
                }
            };
        if !self.try_unify(&ret, &want) {
            return self.err(
                ErrorKind::TryOperand,
                format!(
                    "`?` は、関数の戻り値と同じ種類（`Option` か `Result`）で、`Result` ならエラーの型 E も同じでなければ使えない。ここでは対象が `{}`、戻り値が `{}` である",
                    self.sh(&t),
                    self.sh(&ret)
                ),
            );
        }
        Ok(value)
    }

    /// 列挙型の値（`Some`・`Shape.Circle` など）を値として使うときの型。
    fn variant_value(&mut self, tn: &str, vn: &str) -> R<Ty> {
        let Some(TypeDef::Enum { variants, .. }) = self.types.get(tn).cloned() else {
            return self.err(
                ErrorKind::UnknownType,
                format!("列挙型 `{tn}` が見つからない"),
            );
        };
        let Some((_, fs)) = variants.iter().find(|(n, _)| n == vn) else {
            return self.err(
                ErrorKind::NoSuchMember,
                format!("列挙型 `{tn}` に値 `{vn}` がない"),
            );
        };
        let (ty, m) = self.fresh_named(tn);
        let fs: Vec<Ty> = fs.iter().map(|t| substitute(t, &m)).collect();
        Ok(if fs.is_empty() {
            ty
        } else {
            Ty::Fn(fs, Box::new(ty))
        })
    }

    fn fn_type(&mut self, sig: &Sig, what: &str) -> R<Ty> {
        let m = self.instantiate(&sig.generics, what)?;
        self.require_traits(&m, &sig.trait_bounds, what);
        Ok(Ty::Fn(
            sig.params.iter().map(|(_, t)| substitute(t, &m)).collect(),
            Box::new(substitute(&sig.ret, &m)),
        ))
    }

    fn name(&mut self, n: &str) -> R<Ty> {
        let skip = std::mem::take(&mut self.skip_use);
        if self.refine && n != "self" {
            return self.err(
                ErrorKind::RefinementName,
                format!(
                    "篩型の条件で値として参照できるのは `self` とリテラルだけ（`{n}` は書けない）"
                ),
            );
        }
        if self.lookup(n).is_some() {
            if self.in_ensures
                && !self.in_old
                && self.scopes.first().is_some_and(|s| s.contains_key(n))
            {
                self.ens_used.insert(n.to_string());
            }
            if !skip {
                self.use_place(n)?;
            }
            self.check_holder(n)?;
            if !skip {
                self.touch(n, false, "読み出し");
                self.log_acc(n, AccKind::Read);
            }
            self.name_res(n);
        }
        if let Some(v) = self.lookup(n) {
            // 参照の型の名前は、式の中では参照先の値を表す（ADR 0011 の決定 4）。
            return Ok(match &v.ty {
                Ty::Ref(_, i) => (**i).clone(),
                t => t.clone(),
            });
        }
        if n == "Unit" {
            return Ok(Ty::Unit);
        }
        if let Some(sig) = self.fns.get(n).cloned() {
            return self.fn_type(&sig, &format!("関数 `{n}`"));
        }
        if let Some((_, tn)) = PRELUDE_VALUES.iter().find(|(v, _)| *v == n) {
            return self.variant_value(tn, n);
        }
        self.err(ErrorKind::UnknownName, format!("名前 `{n}` が見つからない"))
    }

    /// 変数 `n` の値が持つ貸し。貸しを持つ変数ならその貸し、参照の変数ならその変数自身。
    fn name_res(&mut self, n: &str) {
        self.res.clear();
        let Some(l) = self.lookup(n) else { return };
        let ty = self.shallow(&l.ty);
        let held: Vec<Src> = self
            .flow
            .loans
            .iter()
            .filter(|x| x.holder == n)
            .map(|x| (x.path.clone(), x.mutable))
            .collect();
        if !held.is_empty() {
            self.res = held;
        } else if contains_ref(&self.zonk(&ty)) {
            self.res = vec![(n.to_string(), matches!(ty, Ty::Ref(true, _)))];
            if !self.track || self.refine {
                self.res.clear();
            }
        }
    }

    fn unary(&mut self, op: UnOp, inner: &Expr) -> R<Ty> {
        match op {
            UnOp::Not => {
                let t = self.expr(inner)?;
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Bool) {
                    return self.err(
                        ErrorKind::OperandTypes,
                        format!("`!` は `Bool` に使うが、`{}` である", self.sh(&t)),
                    );
                }
                Ok(Ty::Bool)
            }
            UnOp::Neg => {
                let t = self.expr(inner)?;
                let v = self.val(&t);
                self.require(
                    &v,
                    Prop::Numeric,
                    ErrorKind::OperandTypes,
                    "前置の `-` は数（`Numeric`）に使うが、`{}` である".into(),
                )?;
                Ok(v)
            }
            UnOp::Ref | UnOp::RefMut => {
                let mutable = op == UnOp::RefMut;
                if !std::mem::take(&mut self.ref_ok) {
                    return self.err(
                        ErrorKind::BorrowPosition,
                        "`&`・`&mut` を書けるのは、関数・メソッド・構築子（`Some(&x)` など）の引数と、`match` の対象だけ（`let r = &x;`・`return &x;`・組やフィールドの値には書けない。参照は呼び出しの引数で渡し、戻り値の参照は呼び出しの結果として受ける。ADR 0012 の決定 1）",
                    );
                }
                let mark = self.log.len();
                let t = self.expr(inner)?;
                let inner_res = std::mem::take(&mut self.res);
                if mutable {
                    let Some(root) = place_root(inner).map(str::to_string) else {
                        return self.err(
                            ErrorKind::BorrowOfNonPlace,
                            "`&mut` で借りられるのは変数とそのフィールドだけ",
                        );
                    };
                    self.assignable_place(&root, inner, false)?;
                }
                let srcs = self.borrow_srcs(inner, mutable, &inner_res, &t);
                if let Some((root, path)) = place_path(strip_paren(inner)) {
                    if self.lookup(&root).is_some() {
                        self.log.truncate(mark);
                        self.log_acc(
                            &path,
                            if mutable {
                                AccKind::Mut
                            } else {
                                AccKind::Shared
                            },
                        );
                        if mutable {
                            self.touch(&path, true, "`&mut` の借用");
                        }
                    }
                }
                let rt = Ty::Ref(mutable, Box::new(t));
                self.res = srcs;
                Ok(rt)
            }
        }
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr) -> R<Ty> {
        let lt = self.expr(l)?;
        let rt = self.expr(r)?;
        let (lv, rv) = (self.val(&lt), self.val(&rt));
        let sym = op_text(op);
        let mismatch = |ck: &Ck| -> R<Ty> {
            let (a, b) = (ck.zonk(&lv), ck.zonk(&rv));
            ck.err(
                ErrorKind::OperandTypes,
                format!(
                    "`{sym}` の両辺の型が合わない: `{}` と `{}`{}",
                    show(&a),
                    show(&b),
                    if (a == Ty::Int && b == Ty::Float) || (a == Ty::Float && b == Ty::Int) {
                        "（`Int` と `Float` の暗黙の変換はない）"
                    } else {
                        ""
                    }
                ),
            )
        };
        if matches!(op, BinOp::And | BinOp::Or) {
            return if self.try_unify(&lv, &Ty::Bool) && self.try_unify(&rv, &Ty::Bool) {
                Ok(Ty::Bool)
            } else {
                mismatch(self)
            };
        }
        if !self.try_unify(&lv, &rv) {
            return mismatch(self);
        }
        let t = self.zonk(&lv);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
                self.require(
                    &t,
                    Prop::Numeric,
                    ErrorKind::OperandTypes,
                    format!("`{sym}` は数（`Numeric`）に使うが、`{{}}` である"),
                )?;
                Ok(t)
            }
            BinOp::Rem => {
                if t == Ty::Float {
                    return self.err(ErrorKind::OperandTypes, "`%` は `Float` に使えない");
                }
                if !self.try_unify(&t, &Ty::Int) {
                    return self.err(
                        ErrorKind::OperandTypes,
                        format!("`%` は `Int` に使うが、`{}` である", show(&t)),
                    );
                }
                Ok(Ty::Int)
            }
            BinOp::Eq | BinOp::Ne => {
                self.require(
                    &t,
                    Prop::Equatable,
                    ErrorKind::NotEquatable,
                    format!("`{{}}` は `{sym}` で比べられない（`Equatable` でない）"),
                )?;
                Ok(Ty::Bool)
            }
            _ => {
                self.require(
                    &t,
                    Prop::Comparable,
                    ErrorKind::NotComparable,
                    format!("`{{}}` は `{sym}` で大小を比べられない（`Comparable` でない）"),
                )?;
                Ok(Ty::Bool)
            }
        }
    }

    /// 篩型 `T` の `T.check` の型: 基の型を受け取り `Option<T>` を返す。
    fn check_fn_type(&self, tn: &str, base: &Ty) -> Ty {
        Ty::Fn(
            vec![base.clone()],
            Box::new(Ty::Named(
                "Option".into(),
                vec![Ty::Named(tn.to_string(), vec![])],
            )),
        )
    }

    fn field(&mut self, recv: &Expr, name: &str) -> R<Ty> {
        let skip = std::mem::take(&mut self.skip_use);
        if matches!(recv, Expr::Name(ns) if ns == "raw")
            && self.raw_ops.contains_key(&format!("raw.{name}"))
        {
            return self
                .raw_allowed(&format!("raw.{name}"), true)
                .map(|()| Ty::Unit);
        }
        if let Some(tn) = self.type_path(recv) {
            return match self.types.get(&tn).cloned() {
                Some(TypeDef::Enum { variants, .. }) => {
                    if variants.iter().any(|(v, _)| v == name) {
                        self.variant_value(&tn, name)
                    } else {
                        self.err(
                            ErrorKind::NoSuchMember,
                            format!("列挙型 `{tn}` に値 `{name}` がない"),
                        )
                    }
                }
                Some(TypeDef::Alias(base)) if name == "check" => Ok(self.check_fn_type(&tn, &base)),
                Some(TypeDef::Opaque(_)) => {
                    self.unsupported(&format!("標準ライブラリの `{tn}.{name}`"))
                }
                _ => self.err(
                    ErrorKind::NoSuchMember,
                    format!("`{tn}.{name}` は値として使えない"),
                ),
            };
        }
        // 場所の連なり（`a.b.c`）は、全体の場所で移動済みかを調べる（部分的な移動を見分ける）
        let chain = place_path(recv).filter(|(r, _)| self.lookup(r).is_some());
        if let Some((r, p)) = &chain {
            let full = format!("{p}.{name}");
            if !skip {
                self.use_place(&full)?;
            }
            self.check_holder(r)?;
            if !skip {
                self.touch(&full, false, "読み出し");
                self.log_acc(&full, AccKind::Read);
            }
            self.skip_use = true;
        }
        let t = self.expr(recv);
        self.skip_use = false;
        let t = t?;
        match self.val(&t) {
            Ty::Named(n, args) => match self.types.get(&n).cloned() {
                Some(TypeDef::Struct { params, fields }) => {
                    match fields.iter().find(|(f, _)| f == name) {
                        Some((_, ft)) => {
                            let m = params.into_iter().zip(args).collect();
                            Ok(substitute(ft, &m))
                        }
                        None => self.err(
                            ErrorKind::NoSuchMember,
                            format!("構造体 `{n}` にフィールド `{name}` がない"),
                        ),
                    }
                }
                _ => self.err(
                    ErrorKind::NoSuchMember,
                    format!("型 `{n}` にフィールド `{name}` はない"),
                ),
            },
            Ty::Var(_) => self.err(
                ErrorKind::CannotInfer,
                format!("フィールド `{name}` を取り出す式の型が決まっていない（先に型を決める）"),
            ),
            Ty::Tuple(_) => self.err(
                ErrorKind::NoSuchMember,
                format!("組の要素は `.{name}` で取り出せない（`let (a, b) = t;` で分解する）"),
            ),
            other => self.err(
                ErrorKind::NoSuchMember,
                format!("`{}` にフィールド `{name}` はない", show(&other)),
            ),
        }
    }

    /// 置き換えた後のシグネチャの検査を、型が決まってからに回す。
    fn sig_check(&mut self, ps: &[Ty], ret: &Ty, what: &str) {
        if self.track && !self.refine {
            self.sig_checks
                .push((ps.to_vec(), ret.clone(), what.to_string(), self.span));
        }
    }

    /// 関数の型 `ft` の値を `args` で呼ぶ。
    fn apply(&mut self, ft: Ty, args: &[Expr], what: &str, eff: Eff) -> R<Ty> {
        match self.shallow(&ft) {
            Ty::Fn(ps, r) => {
                let mark = self.log.len();
                let srcs = self.args(&ps, args, what, &eff)?;
                self.check_overlap(mark)?;
                self.sig_check(&ps, &r, what);
                self.set_res(&r, srcs);
                Ok(*r)
            }
            Ty::Var(_) => self.err(
                ErrorKind::CannotInfer,
                format!("{what}の型が決まっていないので、呼べるか分からない"),
            ),
            other => self.err(
                ErrorKind::NotCallable,
                format!("{what}は `{}` で、呼び出せない", self.sh(&other)),
            ),
        }
    }

    fn call(&mut self, callee: &Expr, args: &[Expr]) -> R<Ty> {
        match callee {
            Expr::Name(n) => {
                if self.refine {
                    return match self.fns.get(n).cloned() {
                        Some(sig) if sig.effects_none => {
                            let ft = self.fn_type(&sig, &format!("関数 `{n}`"))?;
                            let targs = std::mem::take(&mut self.last_inst);
                            self.set_resolved(ResKind::Fn, None, n, targs);
                            self.rec_expr(callee, &ft);
                            self.apply(ft, args, &format!("関数 `{n}`"), Eff::default())
                        }
                        Some(_) => self.err(
                            ErrorKind::RefinementEffect,
                            format!(
                                "篩型の条件で呼べるのは `effects none` の関数だけ（`{n}` は違う）"
                            ),
                        ),
                        None => {
                            self.err(ErrorKind::UnknownName, format!("関数 `{n}` が見つからない"))
                        }
                    };
                }
                let known = self.lookup(n).is_some()
                    || self.fns.contains_key(n)
                    || PRELUDE_VALUES.iter().any(|(v, _)| v == n);
                if !known {
                    return self.err(
                        ErrorKind::UnknownName,
                        format!(
                            "関数 `{n}` が見つからない（この版の標準ライブラリにある関数は ADR 0013 の決定 2 の一覧に限る）"
                        ),
                    );
                }
                self.last_inst.clear();
                let ft = self.name(n)?;
                let targs = std::mem::take(&mut self.last_inst);
                let kind = if self.lookup(n).is_some() {
                    ResKind::Local
                } else {
                    ResKind::Fn
                };
                self.set_resolved(kind, None, n, targs.clone());
                self.rec_expr(callee, &ft);
                let eff = if self.lookup(n).is_some() {
                    if self.fn_params.contains(&n.to_string()) {
                        Eff {
                            extra: vec![Atom::Of(n.clone())],
                            ..Eff::default()
                        }
                    } else {
                        Eff {
                            extra: self.local_eff.get(n).cloned().unwrap_or_default(),
                            ..Eff::default()
                        }
                    }
                } else {
                    self.fns.get(n).map_or_else(Eff::default, |sig| Eff {
                        terms: sig.effects.clone(),
                        ..Eff::default()
                    })
                };
                let cspan = self.span;
                let r = self.apply(ft, args, &format!("`{n}`"), eff)?;
                if kind == ResKind::Fn {
                    // 多相再帰の検査は、引数と対応づいて型引数が決まった後に行う。
                    self.span = cspan;
                    let m: HashMap<String, Ty> = targs.iter().cloned().collect();
                    self.poly_check(&m, n)?;
                }
                Ok(r)
            }
            Expr::Field { expr, name } => {
                // 名前空間の関数: `fs.read(p)`・`console.log(x)`。
                if let Expr::Name(ns) = &**expr {
                    let key = format!("{ns}.{name}");
                    if self.lookup(ns).is_none() && self.fns.contains_key(&key) {
                        if let Some(info) = self.raw_ops.get(&key) {
                            let delegated = info.delegated;
                            self.raw_allowed(&key, false)?;
                            self.raw_used.insert(key.clone());
                            self.raw_delegated |= delegated;
                        }
                        if self.core && self.os_items.fns.contains(&key) {
                            return self.os_in_core(&key);
                        }
                        let sig = self.fns[&key].clone();
                        let what = format!("`{key}`");
                        let ft = self.fn_type(&sig, &what)?;
                        let targs = std::mem::take(&mut self.last_inst);
                        self.set_resolved(ResKind::Namespaced, None, &key, targs);
                        self.rec_expr(callee, &ft);
                        let eff = Eff {
                            terms: sig.effects.clone(),
                            ..Eff::default()
                        };
                        return self.apply(ft, args, &what, eff);
                    }
                }
                // 型引数に属する関数: `T.f(...)`（`T` はいまの関数の型引数で、制約のトレイトの
                // 受け手のない関数を呼ぶ。ADR 0022 決定 2）。
                if let Expr::Name(p) = &**expr {
                    if self.lookup(p).is_none()
                        && self.tparams.iter().any(|x| x == p)
                        && !self.types.contains_key(p)
                    {
                        return self.trait_param_typefn(callee, p, name, args);
                    }
                }
                if let Some(tn) = self.type_path(expr) {
                    // `トレイト.m(x, ...)` の形（曖昧なメソッドをトレイトで選ぶ。ADR 0022 決定 2）。
                    if self.traits.contains_key(&tn) {
                        return self.trait_qualified_call(callee, &tn, name, args);
                    }
                    if self.core && self.os_item_type_fn(&tn, name) {
                        return self.os_in_core(&format!("{tn}.{name}"));
                    }
                    match self.types.get(&tn).cloned() {
                        Some(TypeDef::Enum { variants, .. })
                            if variants.iter().any(|(v, _)| v == name) =>
                        {
                            self.last_inst.clear();
                            let ft = self.variant_value(&tn, name)?;
                            let targs = std::mem::take(&mut self.last_inst);
                            self.set_resolved(ResKind::Variant, Some(&tn), name, targs);
                            self.rec_expr(callee, &ft);
                            return self.apply(ft, args, &format!("`{tn}.{name}`"), Eff::default());
                        }
                        Some(TypeDef::Alias(base)) if name == "check" => {
                            let ft = self.check_fn_type(&tn, &base);
                            self.set_resolved(ResKind::Check, Some(&tn), name, vec![]);
                            self.rec_expr(callee, &ft);
                            return self.apply(ft, args, &format!("`{tn}.check`"), Eff::default());
                        }
                        Some(TypeDef::Opaque(_))
                            if !self.methods.contains_key(&(tn.clone(), name.clone())) =>
                        {
                            return self.err(
                                ErrorKind::NoSuchMember,
                                format!("標準の型 `{tn}` に `{name}` という関数・値がない"),
                            );
                        }
                        _ => {}
                    }
                    let inherent = self.methods.get(&(tn.clone(), name.clone())).cloned();
                    let (sig, kind, owner) = match inherent {
                        Some(s) => (s, ResKind::TypeFn, tn.clone()),
                        None => match self.trait_typefn_sig(&tn, name)? {
                            Some((s, tr)) => (s, ResKind::TraitTypeFn, tr),
                            None => {
                                return self.err(
                                    ErrorKind::NoSuchMember,
                                    format!("`{tn}` に `{name}` という関数・値がない"),
                                )
                            }
                        },
                    };
                    if sig.has_self {
                        return self.err(
                            ErrorKind::NotCallable,
                            format!("`{tn}.{name}` は受け手のあるメソッドなので、値に対して `x.{name}(...)` と呼ぶ"),
                        );
                    }
                    let what = format!("`{tn}.{name}`");
                    let ft = self.fn_type(&sig, &what)?;
                    let targs = std::mem::take(&mut self.last_inst);
                    let m: HashMap<String, Ty> = targs.iter().cloned().collect();
                    self.poly_check(&m, name)?;
                    self.set_resolved(kind, Some(&owner), name, targs);
                    self.rec_expr(callee, &ft);
                    let eff = Eff {
                        terms: sig.effects.clone(),
                        ..Eff::default()
                    };
                    return self.apply(ft, args, &what, eff);
                }
                // メソッド呼び出し x.m(...)
                let mark = self.log.len();
                let rt0 = self.expr(expr)?;
                let recv_res = std::mem::take(&mut self.res);
                let rt = self.peel(&rt0);
                let clone_sig = if name == "clone" {
                    Some(self.clone_sig(&rt)?)
                } else {
                    None
                };
                // 型引数の受け手は、制約のトレイトの関数としてだけ呼べる（ADR 0022 決定 2）。
                // 組み込みの `clone` はこの限りでない（従来どおり型引数でも使える）。
                let mut preset: Option<(Sig, ResKind, String)> = None;
                if let Ty::Param(p) = &rt {
                    if clone_sig.is_none() {
                        match self.trait_param_method_sig(p, name)? {
                            Some((s, tr)) => preset = Some((s, ResKind::TraitMethod, tr)),
                            None => {
                                return self.err(
                                    ErrorKind::NoSuchMember,
                                    format!("型引数 `{p}` にはメソッド `{name}` がない（性質の制約で使えるのは演算子だけ。トレイトの関数は、型引数の制約にそのトレイトを書いたときだけ呼べる）"),
                                )
                            }
                        }
                    }
                }
                let (tn, _) = match &rt {
                    _ if clone_sig.is_some() => (self.sh(&rt), vec![]),
                    Ty::Named(tn, a) => (tn.clone(), a.clone()),
                    Ty::Str => ("String".to_string(), vec![]),
                    Ty::Var(_) => {
                        return self.err(
                            ErrorKind::CannotInfer,
                            format!(
                                "メソッド `{name}` を呼ぶ式の型が決まっていない（先に型を決める）"
                            ),
                        )
                    }
                    Ty::Param(p) => (p.clone(), vec![]),
                    other => {
                        return self.unsupported(&format!(
                            "標準ライブラリの型 `{}` のメソッド `{name}`",
                            show(&self.zonk(other))
                        ))
                    }
                };
                if self.core
                    && clone_sig.is_none()
                    && preset.is_none()
                    && self.os_item_type_fn(&tn, name)
                {
                    return self.os_in_core(&format!("{tn}.{name}"));
                }
                let (sig, res_kind, res_owner) = match preset {
                    Some(x) => x,
                    None => match clone_sig
                        .or_else(|| self.methods.get(&(tn.clone(), name.clone())).cloned())
                    {
                        Some(s) => (s, ResKind::Method, tn.clone()),
                        None => match self.trait_method_sig(&rt, name)? {
                            Some((s, tr)) => (s, ResKind::TraitMethod, tr),
                            None => {
                                return self.err(
                                    ErrorKind::NoSuchMember,
                                    format!("型 `{tn}` にメソッド `{name}` がない"),
                                )
                            }
                        },
                    },
                };
                if !sig.has_self {
                    return self.err(
                        ErrorKind::NotCallable,
                        format!("`{name}` は受け手のない関数なので、`{tn}.{name}(...)` と呼ぶ"),
                    );
                }
                let what = format!("メソッド `{tn}.{name}`");
                self.method_tail(
                    callee, expr, &rt0, &rt, recv_res, mark, &sig, res_kind, &res_owner, name,
                    args, &what,
                )
            }
            _ => self.err(
                ErrorKind::NotCallable,
                "呼び出せるのは関数・メソッド・値の構築子だけ",
            ),
        }
    }

    /// 具体的な型の受け手に対する、トレイトの実装のメソッドのシグネチャ（ADR 0022 決定 2）。
    /// 見えるトレイトの候補が 1 つならそれを返す。実装はあるが見えるトレイトがないときと、
    /// 見えるトレイトが複数あるときはエラーにする。
    fn trait_method_sig(&mut self, rt: &Ty, name: &str) -> R<Option<(Sig, String)>> {
        let z = self.zonk(rt);
        let Some((head, _)) = head_of(&z) else {
            return Ok(None);
        };
        let mut cands: Vec<(Sig, String)> = vec![];
        let mut hidden: Vec<String> = vec![];
        for imp in &self.trait_impls {
            if imp.head != head {
                continue;
            }
            let Some((_, sig)) = imp.fns.iter().find(|(f, _)| f.name == name) else {
                continue;
            };
            if self.visible_traits.contains(&imp.trait_name) {
                cands.push((sig.clone(), imp.trait_name.clone()));
            } else if !hidden.contains(&imp.trait_name) {
                hidden.push(imp.trait_name.clone());
            }
        }
        match cands.len() {
            1 => Ok(Some(cands.pop().expect("1 つ"))),
            0 => {
                if hidden.is_empty() {
                    Ok(None)
                } else {
                    self.err(
                        ErrorKind::NoSuchMember,
                        format!(
                            "型 `{head}` のメソッド `{name}` はトレイト `{}` の実装にあるが、このモジュールではそのトレイトが見えていない（`use` で取り込む）",
                            hidden.join("`・`")
                        ),
                    )
                }
            }
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "メソッド `{name}` は見えている複数のトレイト（{list}）の実装にあり、どれを呼ぶか決まらない（`トレイト.{name}(受け手, ...)` と書いて選ぶ）"
                    ),
                )
            }
        }
    }

    /// 型に属する関数としての、トレイトの実装の関数（受け手のないもの。ADR 0022 決定 2）。
    fn trait_typefn_sig(&mut self, tn: &str, name: &str) -> R<Option<(Sig, String)>> {
        let mut cands: Vec<(Sig, String)> = vec![];
        let mut hidden: Vec<String> = vec![];
        for imp in &self.trait_impls {
            if imp.head != tn {
                continue;
            }
            let Some((_, sig)) = imp.fns.iter().find(|(f, s)| f.name == name && !s.has_self) else {
                continue;
            };
            if self.visible_traits.contains(&imp.trait_name) {
                cands.push((sig.clone(), imp.trait_name.clone()));
            } else if !hidden.contains(&imp.trait_name) {
                hidden.push(imp.trait_name.clone());
            }
        }
        match cands.len() {
            1 => Ok(Some(cands.pop().expect("1 つ"))),
            0 => {
                if hidden.is_empty() {
                    Ok(None)
                } else {
                    self.err(
                        ErrorKind::NoSuchMember,
                        format!(
                            "型 `{tn}` の関数 `{name}` はトレイト `{}` の実装にあるが、このモジュールではそのトレイトが見えていない（`use` で取り込む）",
                            hidden.join("`・`")
                        ),
                    )
                }
            }
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "関数 `{name}` は見えている複数のトレイト（{list}）の実装にあり、どれを呼ぶか決まらない"
                    ),
                )
            }
        }
    }

    /// 型引数 `p` の制約のトレイトから、受け手のある関数 `name` のシグネチャ（`Self` を `p` に置き換えたもの）。
    fn trait_param_method_sig(&mut self, p: &str, name: &str) -> R<Option<(Sig, String)>> {
        let bounds: Vec<String> = self
            .cur_trait_bounds
            .iter()
            .filter(|(n, _)| n == p)
            .map(|(_, t)| t.clone())
            .collect();
        let mut cands: Vec<(Sig, String)> = vec![];
        for tr in &bounds {
            let Some(tdef) = self.traits.get(tr) else {
                continue;
            };
            if let Some((_, sig)) = tdef.fns.iter().find(|(f, s)| f.name == name && s.has_self) {
                let subst = HashMap::from([("Self".to_string(), Ty::Param(p.to_string()))]);
                let mut s = sig.clone();
                s.params = s
                    .params
                    .iter()
                    .map(|(n, t)| (n.clone(), substitute(t, &subst)))
                    .collect();
                s.ret = substitute(&s.ret, &subst);
                s.generics = vec![];
                s.trait_bounds = vec![];
                cands.push((s, tr.clone()));
            }
        }
        match cands.len() {
            1 => Ok(Some(cands.pop().expect("1 つ"))),
            0 => Ok(None),
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "メソッド `{name}` は制約の複数のトレイト（{list}）にあり、どれを呼ぶか決まらない（`トレイト.{name}(受け手, ...)` と書いて選ぶ）"
                    ),
                )
            }
        }
    }

    /// `T.f(...)`（型引数の、受け手のないトレイトの関数。ADR 0022 決定 2）。
    fn trait_param_typefn(&mut self, callee: &Expr, p: &str, name: &str, args: &[Expr]) -> R<Ty> {
        let bounds: Vec<String> = self
            .cur_trait_bounds
            .iter()
            .filter(|(n, _)| n == p)
            .map(|(_, t)| t.clone())
            .collect();
        let mut cands: Vec<(Sig, String)> = vec![];
        for tr in &bounds {
            let Some(tdef) = self.traits.get(tr) else {
                continue;
            };
            if let Some((_, sig)) = tdef.fns.iter().find(|(f, s)| f.name == name && !s.has_self) {
                cands.push((sig.clone(), tr.clone()));
            }
        }
        let what = format!("`{p}.{name}`");
        let (sig, tr) = match cands.len() {
            1 => cands.pop().expect("1 つ"),
            0 => {
                return self.err(
                    ErrorKind::NoSuchMember,
                    format!(
                        "型引数 `{p}` の制約にあるトレイトに、受け手のない関数 `{name}` がない（`T.{name}(...)` で呼べるのは、制約のトレイトの関数だけ）"
                    ),
                )
            }
            _ => {
                let list = cands
                    .iter()
                    .map(|(_, t)| format!("`{t}`"))
                    .collect::<Vec<_>>()
                    .join("・");
                return self.err(
                    ErrorKind::UseConflict,
                    format!(
                        "関数 `{name}` は制約の複数のトレイト（{list}）にあり、どれを呼ぶか決まらない"
                    ),
                );
            }
        };
        let subst = HashMap::from([("Self".to_string(), Ty::Param(p.to_string()))]);
        let ft = Ty::Fn(
            sig.params
                .iter()
                .map(|(_, t)| substitute(t, &subst))
                .collect(),
            Box::new(substitute(&sig.ret, &subst)),
        );
        self.set_resolved(
            ResKind::TraitTypeFn,
            Some(&tr),
            name,
            vec![(p.to_string(), Ty::Param(p.to_string()))],
        );
        self.rec_expr(callee, &ft);
        let eff = Eff {
            terms: sig.effects.clone(),
            ..Eff::default()
        };
        self.apply(ft, args, &what, eff)
    }

    /// `トレイト.m(x, ...)`（受け手を最初の引数に渡してトレイトを選ぶ形。ADR 0022 決定 2）。
    fn trait_qualified_call(
        &mut self,
        callee: &Expr,
        tr: &str,
        name: &str,
        args: &[Expr],
    ) -> R<Ty> {
        let Some(tdef) = self.traits.get(tr).cloned() else {
            return self.err(
                ErrorKind::UnknownType,
                format!("トレイト `{tr}` が見つからない"),
            );
        };
        let Some((_, tsig)) = tdef.fns.iter().find(|(f, _)| f.name == name) else {
            return self.err(
                ErrorKind::NoSuchMember,
                format!("トレイト `{tr}` に関数 `{name}` がない"),
            );
        };
        if !tsig.has_self {
            return self.err(
                ErrorKind::NotCallable,
                format!(
                    "`{tr}.{name}` は受け手のない関数なので、型に対して `{name}(...)` の形では呼べない（受け手のある関数をトレイトで選ぶ形）"
                ),
            );
        }
        if args.is_empty() {
            return self.err(
                ErrorKind::ArgCount,
                format!("`{tr}.{name}` は受け手を最初の引数に渡す形（`{tr}.{name}(受け手, ...)`）で呼ぶ"),
            );
        }
        let (recv, rest) = args.split_first().expect("1 つ以上");
        let mark = self.log.len();
        let rt0 = self.expr(recv)?;
        let recv_res = std::mem::take(&mut self.res);
        let rt = self.peel(&rt0);
        let (sig, kind) = match &rt {
            Ty::Param(p) => {
                if !self.cur_trait_bounds.iter().any(|(n, t)| n == p && t == tr) {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!(
                            "型引数 `{p}` の制約にトレイト `{tr}` がないので、`{tr}.{name}(...)` の形では呼べない"
                        ),
                    );
                }
                let subst = HashMap::from([("Self".to_string(), Ty::Param(p.clone()))]);
                let mut s = tsig.clone();
                s.params = s
                    .params
                    .iter()
                    .map(|(n, t)| (n.clone(), substitute(t, &subst)))
                    .collect();
                s.ret = substitute(&s.ret, &subst);
                s.generics = vec![];
                s.trait_bounds = vec![];
                (s, ResKind::TraitMethod)
            }
            _ => {
                let z = self.zonk(&rt);
                let Some((head, _)) = head_of(&z) else {
                    return self.err(
                        ErrorKind::TypeMismatch,
                        format!("`{tr}.{name}` の受け手の型が決まらない"),
                    );
                };
                let Some(imp) = self
                    .trait_impls
                    .iter()
                    .find(|x| x.trait_name == tr && x.head == head)
                else {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!("型 `{head}` はトレイト `{tr}` を実装していない"),
                    );
                };
                let Some((_, s)) = imp.fns.iter().find(|(f, _)| f.name == name) else {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!("型 `{head}` のトレイト `{tr}` の実装に関数 `{name}` がない"),
                    );
                };
                (s.clone(), ResKind::TraitMethod)
            }
        };
        let what = format!("メソッド `{tr}.{name}`");
        self.method_tail(
            callee, recv, &rt0, &rt, recv_res, mark, &sig, kind, tr, name, rest, &what,
        )
    }

    /// メソッド呼び出しの末尾（シグネチャが決まった後。固有・トレイト共通）。
    #[allow(clippy::too_many_arguments)]
    fn method_tail(
        &mut self,
        callee: &Expr,
        recv: &Expr,
        rt0: &Ty,
        rt: &Ty,
        recv_res: Vec<Src>,
        mark: usize,
        sig: &Sig,
        kind: ResKind,
        owner: &str,
        name: &str,
        args: &[Expr],
        what: &str,
    ) -> R<Ty> {
        let m = self.instantiate(&sig.generics, what)?;
        self.require_traits(&m, &sig.trait_bounds, what);
        let targs = std::mem::take(&mut self.last_inst);
        self.poly_check(&m, name)?;
        self.set_resolved(kind, Some(owner), name, targs);
        let selfty = self.peel(&substitute(&sig.params[0].1, &m));
        if !self.try_unify(&selfty, rt) {
            return self.err(
                ErrorKind::TypeMismatch,
                format!(
                    "{what}の受け手は `{}` だが、`{}` である",
                    self.sh(&selfty),
                    self.sh(rt)
                ),
            );
        }
        if let Ty::Ref(true, _) = sig.params[0].1 {
            let Some(root) = place_root(recv).map(str::to_string) else {
                return self.err(
                    ErrorKind::BorrowOfNonPlace,
                    format!("`{name}` は `&mut` の受け手を取るので、変数に対して呼ぶ"),
                );
            };
            self.assignable_place(&root, recv, false)?;
        }
        let mut srcs: Vec<Src> = vec![];
        if let Ty::Ref(m, _) = sig.params[0].1 {
            // 借りる受け手
            let kind = if m { AccKind::Mut } else { AccKind::Shared };
            let place = place_path(strip_paren(recv)).filter(|(r, _)| self.lookup(r).is_some());
            if let Some((_, path)) = &place {
                self.log.truncate(mark);
                self.log_acc(path, kind);
                if m {
                    self.touch(
                        path,
                        true,
                        &format!("`&mut` を受け取るメソッド `{name}` の呼び出し"),
                    );
                }
            }
            srcs = self.borrow_srcs(recv, m, &recv_res, rt0);
        } else {
            // 値で受け取る受け手（`opt.map(..)` など）は移動する
            srcs.extend(recv_res);
            self.consume(recv, rt0)?;
        }
        let ps: Vec<Ty> = sig.params[1..]
            .iter()
            .map(|(_, t)| substitute(t, &m))
            .collect();
        let eff = Eff {
            terms: sig.effects.clone(),
            off: 1,
            ..Eff::default()
        };
        let more = self.args(&ps, args, what, &eff)?;
        srcs.extend(more);
        self.check_overlap(mark)?;
        let ret = substitute(&sig.ret, &m);
        self.rec_expr(callee, &Ty::Fn(ps.clone(), Box::new(ret.clone())));
        let mut all: Vec<Ty> = vec![substitute(&sig.params[0].1, &m)];
        all.extend(ps.iter().cloned());
        self.sig_check(&all, &ret, what);
        self.set_res(&ret, srcs);
        Ok(ret)
    }

    /// 組み込みの `clone(self: &T) -> T` のシグネチャ（ADR 0013 の決定 2）。受け手の型ごとに作る。
    fn clone_sig(&self, rt: &Ty) -> R<Sig> {
        let shown = self.sh(rt);
        match self.shallow(rt) {
            Ty::Var(_) => {
                return self.err(
                    ErrorKind::CannotInfer,
                    "`clone` を呼ぶ式の型が決まっていない（先に型を決める）",
                )
            }
            Ty::Fn(..) => {
                return self.err(
                    ErrorKind::NoSuchMember,
                    "関数の型には `clone` がない（関数の値は引数に渡しても移動しないので、別名を作らずに元の名前を使う）",
                )
            }
            _ => {}
        }
        if self.holds(rt, Prop::Copyable, 0) == Some(true) {
            return self.err(
                ErrorKind::CloneOfCopyable,
                format!("`{shown}` は `Copyable` なので `clone` は要らない（値として読む位置で複製される）"),
            );
        }
        Ok(Sig {
            generics: vec![],
            trait_bounds: vec![],
            params: vec![("self".into(), Ty::Ref(false, Box::new(rt.clone())))],
            ret: rt.clone(),
            effects_none: true,
            effects: vec![],
            has_self: true,
        })
    }

    fn args(&mut self, params: &[Ty], args: &[Expr], what: &str, eff: &Eff) -> R<Vec<Src>> {
        if params.len() != args.len() {
            return self.err(
                ErrorKind::ArgCount,
                format!(
                    "{what}の引数は {} 個だが、{} 個渡している",
                    params.len(),
                    args.len()
                ),
            );
        }
        let sp = self.span;
        let mut gained: Vec<Atom> = vec![];
        let mut srcs: Vec<Src> = vec![];
        for (i, (p, a)) in params.iter().zip(args).enumerate() {
            let closure = matches!(strip_paren(a), Expr::Closure { .. });
            self.defer = closure;
            self.closure_arg = closure;
            self.ref_ok = matches!(
                a,
                Expr::Unary {
                    op: UnOp::Ref | UnOp::RefMut,
                    ..
                }
            ) && matches!(self.shallow(p), Ty::Ref(..) | Ty::Var(_));
            let t = self.expr_hint(a, Some(p));
            self.defer = false;
            self.closure_arg = false;
            self.ref_ok = false;
            let t = t?;
            if !closure {
                srcs.append(&mut self.res);
            }
            self.res.clear();
            self.span = sp;
            self.fn_value_form(a, &t)?;
            // 値で受け取る引数は移動する（参照の引数は `&x` と書くので場所ではない）
            // 関数の型の引数へ渡す関数の値は、呼び出しの間だけの共有借用（ADR 0013 の決定 3）
            let fn_arg = matches!(self.shallow(p), Ty::Fn(..))
                && matches!(self.zonk(&t), Ty::Fn(..))
                && matches!(strip_paren(a), Expr::Name(n) if self.lookup(n).is_some());
            if fn_arg {
                if let Expr::Name(n) = strip_paren(a) {
                    self.log_acc(n, AccKind::Shared);
                }
            } else if !matches!(self.shallow(p), Ty::Ref(..)) {
                self.consume(a, &t)?;
            }
            let of_this = eff
                .terms
                .iter()
                .any(|t| matches!(t, EffTerm::Of(j) if *j == i + eff.off));
            if closure {
                let ce = std::mem::take(&mut self.last_closure);
                if of_this {
                    gained.extend(ce);
                }
            } else if of_this {
                gained.extend(self.fn_effects(a));
            }
            if !self.coerce(p, &t, a) {
                let hint = if matches!(self.shallow(p), Ty::Ref(..))
                    && !matches!(self.shallow(&t), Ty::Ref(..))
                {
                    "（借用は呼び出しの位置で `&x`・`&mut x` と書く）"
                } else {
                    self.check_hint(p, &t)
                };
                return self.err(
                    self.mismatch_kind(p, &t),
                    format!(
                        "{what}の第 {} 引数は `{}` でなければならないが、`{}` である{hint}",
                        i + 1,
                        self.sh(p),
                        self.sh(&t)
                    ),
                );
            }
        }
        for t in &eff.terms {
            if let EffTerm::Cat(c) = t {
                self.add_eff(Atom::Cat(c.clone()));
            }
        }
        for a in eff.extra.iter().cloned().chain(gained) {
            self.add_eff(a);
        }
        Ok(srcs)
    }

    fn struct_lit(&mut self, path: &[String], fields: &[(String, Expr)]) -> R<Ty> {
        let name = path.join(".");
        if path.len() == 1 && self.extern_types.contains(&path[0]) {
            return self.err(
                ErrorKind::PrivateConstruction,
                format!("構造体 `{name}` は他のモジュールで定義されていて、構造体リテラルでは作れない（定義したモジュールが `pub` で出す関数で作る。ADR 0021 の決定 4）"),
            );
        }
        let def = if path.len() == 1 {
            self.types.get(&path[0]).cloned()
        } else {
            None
        };
        let Some(TypeDef::Struct { fields: defs, .. }) = def else {
            if matches!(def, Some(TypeDef::Opaque(_))) {
                return self.unsupported(&format!("標準ライブラリの型 `{name}`"));
            }
            return self.err(
                ErrorKind::UnknownType,
                format!("構造体 `{name}` が見つからない"),
            );
        };
        let (ty, m) = self.fresh_named(&path[0]);
        let mut seen = HashSet::new();
        for (f, e) in fields {
            let Some((_, ft)) = defs.iter().find(|(n, _)| n == f) else {
                return self.err(
                    ErrorKind::FieldSet,
                    format!("構造体 `{name}` にフィールド `{f}` がない"),
                );
            };
            if !seen.insert(f.clone()) {
                return self.err(
                    ErrorKind::FieldSet,
                    format!("フィールド `{f}` が二重にある"),
                );
            }
            let ft = substitute(ft, &m);
            let sp = self.span;
            let t = self.expr_hint(e, Some(&ft))?;
            self.consume(e, &t)?;
            self.span = sp;
            if !self.coerce(&ft, &t, e) {
                return self.err(
                    ErrorKind::TypeMismatch,
                    format!(
                        "フィールド `{f}`は `{}` でなければならないが、`{}` である{}",
                        self.sh(&ft),
                        self.sh(&t),
                        self.check_hint(&ft, &t)
                    ),
                );
            }
        }
        if let Some((m, _)) = defs.iter().find(|(n, _)| !seen.contains(n)) {
            return self.err(
                ErrorKind::FieldSet,
                format!("構造体 `{name}` のフィールド `{m}` がない"),
            );
        }
        Ok(ty)
    }
}

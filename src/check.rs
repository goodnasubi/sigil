//! 型検査（M1。ADR 0011）。
//!
//! 基本型・組・参照・構造体・列挙型・篩型・総称（関数・構造体・列挙型・`impl`）・6 つの性質と型引数の
//! 制約・`Option`/`Result`/`?`・`match` と網羅性・クロージャ・単一化による推論・`T.check`・`let` の
//! 型注釈の要否を扱う。式を下から上へ型づけ、最初の型エラーで打ち切る（M0 と同じ）。標準ライブラリの
//! 関数・メソッド（`List.new`・`xs.len()` など）は後続の PR で扱うので、いまは `Unsupported` にする
//! （黙って通さない）。効果の検査と所有権の検査は含まない。
//!
//! 位置: 式は位置を持たないので、エラーの位置は式を含む文（または宣言）の位置になる。
//!
//! 推論: 型変数 `Ty::Var` に単一化で型を割り当てる。関数 1 つの本体ごとに状態を作り直す。性質の
//! 制約（`T: Equatable` など）は、型が決まった時点で調べ、決まっていなければ本体の終わりまで
//! 繰り延べる。`let` の型注釈の要否は、注釈を外して本体を検査し直し、同じ型に決まるかで判定する。

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::effects::{EffectTable, Options, Primitive};
use crate::error::{Error, ErrorKind, Span};
use crate::exhaustive::{missing, useful, Ctor, P};
use crate::parser::parse;

type R<T> = Result<T, Error>;

/// ソースを構文解析してから型検査する。最初のエラーを返す。
pub fn check(src: &str) -> R<()> {
    check_with(src, &Options::default())
}

/// 効果のカテゴリ表や仮の関数（標準ライブラリが入るまで）を指定して検査する。
pub fn check_with(src: &str, opts: &Options) -> R<()> {
    check_file_with(&parse(src)?, opts)
}

#[derive(Debug, Clone, PartialEq)]
enum Ty {
    Int,
    Float,
    Bool,
    Str,
    Unit,
    Tuple(Vec<Ty>),
    Ref(bool, Box<Ty>),
    /// 構造体・列挙型・篩型・標準の型（`Option`・`List` など）の名前と型引数。
    Named(String, Vec<Ty>),
    /// 総称の宣言の中の型引数（剛体: 他の型とは等しくない）。
    Param(String),
    /// 推論中の未知の型。
    Var(u32),
    Fn(Vec<Ty>, Box<Ty>),
}

fn show(t: &Ty) -> String {
    let list = |ts: &[Ty]| ts.iter().map(show).collect::<Vec<_>>().join(", ");
    match t {
        Ty::Int => "Int".into(),
        Ty::Float => "Float".into(),
        Ty::Bool => "Bool".into(),
        Ty::Str => "String".into(),
        Ty::Unit => "Unit".into(),
        Ty::Tuple(ts) => format!("({})", list(ts)),
        Ty::Ref(m, i) => format!("&{}{}", if *m { "mut " } else { "" }, show(i)),
        Ty::Named(n, a) if a.is_empty() => n.clone(),
        Ty::Named(n, a) => format!("{n}<{}>", list(a)),
        Ty::Param(n) => n.clone(),
        Ty::Var(_) => "_".into(),
        Ty::Fn(ps, r) => {
            if **r == Ty::Unit {
                format!("fn({})", list(ps))
            } else {
                format!("fn({}) -> {}", list(ps), show(r))
            }
        }
    }
}

/// 型引数の制約に書ける性質（ADR 0011 の決定 6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prop {
    Copyable,
    Equatable,
    Comparable,
    Numeric,
    Sendable,
    Shareable,
    Displayable,
}

const PROPS: &[(&str, Prop)] = &[
    ("Copyable", Prop::Copyable),
    ("Equatable", Prop::Equatable),
    ("Comparable", Prop::Comparable),
    ("Numeric", Prop::Numeric),
    ("Sendable", Prop::Sendable),
    ("Shareable", Prop::Shareable),
    ("Displayable", Prop::Displayable),
];

impl Prop {
    fn name(self) -> &'static str {
        PROPS.iter().find(|(_, p)| *p == self).expect("prop").0
    }
}

#[derive(Debug, Clone)]
enum TypeDef {
    /// 篩型（基の型）。
    Alias(Ty),
    Struct {
        params: Vec<String>,
        fields: Vec<(String, Ty)>,
    },
    Enum {
        params: Vec<String>,
        variants: Vec<(String, Vec<Ty>)>,
    },
    /// 標準ライブラリの型で、中身は見せない（型引数の数だけ持つ）。
    Opaque(usize),
}

#[derive(Debug, Clone)]
struct Sig {
    /// 型引数（`impl` のものを先に、関数のものを後に）と制約。
    generics: Vec<(String, Vec<Prop>)>,
    params: Vec<(String, Ty)>,
    ret: Ty,
    effects_none: bool,
    /// 宣言した効果（`of(f)` は引数の位置で持つ）。
    effects: Vec<EffTerm>,
    has_self: bool,
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

struct Ck {
    types: HashMap<String, TypeDef>,
    fns: HashMap<String, Sig>,
    methods: HashMap<(String, String), Sig>,
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
    table: EffectTable,
    prims: Vec<Primitive>,
    /// モジュールの `targets`。
    targets: Vec<String>,
    /// 効果の推論の枠（先頭が関数の本体、クロージャごとに積む）。
    eff_stack: Vec<Vec<Atom>>,
    /// 関数の型の引数（`of(f)` の `f` になれる）。
    fn_params: Vec<String>,
    /// `let` で束縛したクロージャの効果。
    local_eff: HashMap<String, Vec<Atom>>,
    /// 次のクロージャの効果を、親の枠に足さずに `last_closure` へ取っておく。
    defer: bool,
    /// 次に評価する `&e` が、呼び出しの引数の位置にある（`args` が立て、`unary` が取る）。
    ref_ok: bool,
    /// `match` の対象が参照（`&x` か、参照を含む値）で、腕の束縛が参照になる。
    by_ref: bool,
    last_closure: Vec<Atom>,
    inferred: Vec<Atom>,
}

pub fn check_file(file: &File) -> R<()> {
    check_file_with(file, &Options::default())
}

pub fn check_file_with(file: &File, opts: &Options) -> R<()> {
    let mut ck = Ck {
        types: HashMap::new(),
        fns: HashMap::new(),
        methods: HashMap::new(),
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
        table: opts.table.clone(),
        prims: opts.primitives.clone(),
        targets: file.module.targets.clone(),
        eff_stack: vec![vec![]],
        fn_params: vec![],
        local_eff: HashMap::new(),
        defer: false,
        ref_ok: false,
        by_ref: false,
        last_closure: vec![],
        inferred: vec![],
    };
    ck.prelude();
    ck.run(file)
}

fn substitute(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
    let go = |ts: &[Ty]| ts.iter().map(|t| substitute(t, m)).collect::<Vec<_>>();
    match t {
        Ty::Param(p) => m.get(p).cloned().unwrap_or_else(|| t.clone()),
        Ty::Tuple(ts) => Ty::Tuple(go(ts)),
        Ty::Ref(mu, i) => Ty::Ref(*mu, Box::new(substitute(i, m))),
        Ty::Named(n, a) => Ty::Named(n.clone(), go(a)),
        Ty::Fn(ps, r) => Ty::Fn(go(ps), Box::new(substitute(r, m))),
        _ => t.clone(),
    }
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
        for (n, k) in crate::stdlib::OPAQUE_TYPES {
            self.types.insert((*n).into(), TypeDef::Opaque(*k));
        }
    }

    // ---------- 型変数と単一化 ----------

    fn fresh(&mut self) -> Ty {
        self.subst.push(None);
        Ty::Var(self.subst.len() as u32 - 1)
    }

    /// 推論で決まるべき型変数を作る（本体の終わりまでに決まらなければ `CannotInfer`）。
    fn fresh_known(&mut self, what: String) -> Ty {
        let v = self.fresh();
        self.must_know.push((v.clone(), what, self.span));
        v
    }

    fn shallow(&self, t: &Ty) -> Ty {
        let mut t = t.clone();
        while let Ty::Var(v) = t {
            match &self.subst[v as usize] {
                Some(n) => t = n.clone(),
                None => break,
            }
        }
        t
    }

    fn zonk(&self, t: &Ty) -> Ty {
        let go = |ts: &[Ty]| ts.iter().map(|t| self.zonk(t)).collect::<Vec<_>>();
        match self.shallow(t) {
            Ty::Tuple(ts) => Ty::Tuple(go(&ts)),
            Ty::Ref(m, i) => Ty::Ref(m, Box::new(self.zonk(&i))),
            Ty::Named(n, a) => Ty::Named(n, go(&a)),
            Ty::Fn(ps, r) => Ty::Fn(go(&ps), Box::new(self.zonk(&r))),
            t => t,
        }
    }

    fn sh(&self, t: &Ty) -> String {
        show(&self.zonk(t))
    }

    fn occurs(&self, v: u32, t: &Ty) -> bool {
        match self.shallow(t) {
            Ty::Var(w) => v == w,
            Ty::Tuple(ts) => ts.iter().any(|t| self.occurs(v, t)),
            Ty::Ref(_, i) => self.occurs(v, &i),
            Ty::Named(_, a) => a.iter().any(|t| self.occurs(v, t)),
            Ty::Fn(ps, r) => ps.iter().any(|t| self.occurs(v, t)) || self.occurs(v, &r),
            _ => false,
        }
    }

    fn unify(&mut self, a: &Ty, b: &Ty) -> Result<(), ()> {
        let (a, b) = (self.shallow(a), self.shallow(b));
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), t) | (t, Ty::Var(x)) => {
                if self.occurs(*x, t) {
                    return Err(());
                }
                self.subst[*x as usize] = Some(t.clone());
                Ok(())
            }
            (Ty::Int, Ty::Int)
            | (Ty::Float, Ty::Float)
            | (Ty::Bool, Ty::Bool)
            | (Ty::Str, Ty::Str)
            | (Ty::Unit, Ty::Unit) => Ok(()),
            (Ty::Param(p), Ty::Param(q)) if p == q => Ok(()),
            (Ty::Tuple(xs), Ty::Tuple(ys)) if xs.len() == ys.len() => self.unify_all(xs, ys),
            (Ty::Ref(m, x), Ty::Ref(n, y)) if m == n => self.unify(x, y),
            (Ty::Named(n, xs), Ty::Named(m, ys)) if n == m && xs.len() == ys.len() => {
                self.unify_all(xs, ys)
            }
            (Ty::Fn(ps, r), Ty::Fn(qs, s)) if ps.len() == qs.len() => {
                self.unify_all(ps, qs)?;
                self.unify(r, s)
            }
            _ => Err(()),
        }
    }

    fn unify_all(&mut self, xs: &[Ty], ys: &[Ty]) -> Result<(), ()> {
        for (x, y) in xs.iter().zip(ys) {
            self.unify(x, y)?;
        }
        Ok(())
    }

    /// 失敗したら束縛を元に戻す単一化。
    fn try_unify(&mut self, a: &Ty, b: &Ty) -> bool {
        let snap = self.subst.clone();
        if self.unify(a, b).is_ok() {
            true
        } else {
            self.subst = snap;
            false
        }
    }

    /// 値を `expected` が求められる位置に渡せるか。暗黙の移しは篩型 -> 基の型と、リテラル -> 篩型
    /// だけ（ADR 0011 の決定 8。型の中では行わない）。`&mut T` から `&T` への変換はしない（決定 4）。
    fn coerce(&mut self, expected: &Ty, actual: &Ty, e: &Expr) -> bool {
        if self.try_unify(expected, actual) {
            return true;
        }
        if let Some(b) = self.refinement_base(actual) {
            if self.try_unify(expected, &b) {
                return true;
            }
        }
        if is_literal(e) {
            if let Some(b) = self.refinement_base(expected) {
                if self.try_unify(&b, actual) {
                    return true;
                }
            }
        }
        false
    }

    fn refinement_base(&self, t: &Ty) -> Option<Ty> {
        match self.shallow(t) {
            Ty::Named(n, _) => match self.types.get(&n) {
                Some(TypeDef::Alias(b)) => Some(b.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// 値の位置での型: 参照を外し、篩型は基の型にする（ADR 0011 の決定 4・8）。
    fn val(&self, t: &Ty) -> Ty {
        match self.shallow(t) {
            Ty::Ref(_, i) => self.val(&i),
            t @ Ty::Named(..) => match self.refinement_base(&t) {
                Some(b) => b,
                None => self.zonk(&t),
            },
            t => self.zonk(&t),
        }
    }

    // ---------- 型の性質（導出。ADR 0011 の決定 6） ----------

    /// 性質を持つか。持つ・持たないが決まらない（型変数が残っている）ときは `None`。
    fn holds(&self, t: &Ty, p: Prop, depth: u32) -> Option<bool> {
        if depth > 32 {
            return Some(false);
        }
        fn all(it: impl Iterator<Item = Option<bool>>) -> Option<bool> {
            let mut unknown = false;
            for r in it {
                match r {
                    Some(false) => return Some(false),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            if unknown {
                None
            } else {
                Some(true)
            }
        }
        let d = depth + 1;
        match self.shallow(t) {
            Ty::Var(_) => None,
            Ty::Int | Ty::Float => Some(true),
            Ty::Bool | Ty::Unit => Some(!matches!(p, Prop::Comparable | Prop::Numeric)),
            Ty::Str => Some(!matches!(p, Prop::Copyable | Prop::Numeric)),
            Ty::Tuple(ts) => match p {
                Prop::Comparable | Prop::Numeric | Prop::Displayable => Some(false),
                _ => all(ts.iter().map(|t| self.holds(t, p, d))),
            },
            Ty::Ref(m, i) => match p {
                Prop::Sendable => Some(false),
                Prop::Shareable if m => Some(false),
                _ => self.holds(&i, p, d),
            },
            Ty::Fn(..) => Some(false),
            Ty::Param(n) => {
                let bs = self.env.get(&n).map(|v| &v[..]).unwrap_or(&[]);
                Some(bs.contains(&p) || (p == Prop::Comparable && bs.contains(&Prop::Numeric)))
            }
            Ty::Named(n, args) => match self.types.get(&n) {
                Some(TypeDef::Alias(b)) => {
                    if p == Prop::Numeric {
                        Some(false)
                    } else {
                        self.holds(b, p, d)
                    }
                }
                Some(TypeDef::Opaque(_)) => match p {
                    Prop::Equatable | Prop::Sendable | Prop::Shareable => {
                        all(args.iter().map(|t| self.holds(t, p, d)))
                    }
                    _ => Some(false),
                },
                Some(TypeDef::Struct { .. } | TypeDef::Enum { .. }) => match p {
                    Prop::Copyable | Prop::Comparable | Prop::Numeric | Prop::Displayable => {
                        Some(false)
                    }
                    _ => all(self
                        .component_types(&n, &args)
                        .iter()
                        .map(|t| self.holds(t, p, d))),
                },
                None => Some(false),
            },
        }
    }

    /// 構造体のフィールド・列挙型の値の中身の型を、型引数を入れて返す。
    fn component_types(&self, n: &str, args: &[Ty]) -> Vec<Ty> {
        match self.types.get(n) {
            Some(TypeDef::Struct { params, fields }) => {
                let m = params.iter().cloned().zip(args.iter().cloned()).collect();
                fields.iter().map(|(_, t)| substitute(t, &m)).collect()
            }
            Some(TypeDef::Enum { params, variants }) => {
                let m = params.iter().cloned().zip(args.iter().cloned()).collect();
                variants
                    .iter()
                    .flat_map(|(_, ts)| ts.iter().map(|t| substitute(t, &m)))
                    .collect()
            }
            _ => vec![],
        }
    }

    /// `ty` が性質 `prop` を持つことを求める。決まらなければ本体の終わりまで繰り延べる。
    fn require(&mut self, ty: &Ty, prop: Prop, kind: ErrorKind, text: String) -> R<()> {
        match self.holds(ty, prop, 0) {
            Some(true) => Ok(()),
            Some(false) => self.err(kind, text.replace("{}", &self.sh(ty))),
            None => {
                self.pending.push(Pending {
                    ty: ty.clone(),
                    prop,
                    kind,
                    text,
                    span: self.span,
                });
                Ok(())
            }
        }
    }

    /// 総称の型を具体化する: 型引数ごとに新しい型変数を作り、制約を繰り延べの調べに加える。
    fn instantiate(
        &mut self,
        generics: &[(String, Vec<Prop>)],
        what: &str,
    ) -> R<HashMap<String, Ty>> {
        let mut m = HashMap::new();
        for (n, props) in generics {
            let v = self.fresh_known(format!("{what}の型引数 `{n}`"));
            for p in props {
                self.require(
                    &v,
                    *p,
                    ErrorKind::ConstraintUnsatisfied,
                    format!(
                        "{what}の型引数 `{n}` は `{}` でなければならないが、`{{}}` は持たない",
                        p.name()
                    ),
                )?;
            }
            m.insert(n.clone(), v);
        }
        Ok(m)
    }

    /// 型引数に新しい型変数を入れた、構造体・列挙型の型。
    fn fresh_named(&mut self, name: &str) -> (Ty, HashMap<String, Ty>) {
        let params = match self.types.get(name) {
            Some(TypeDef::Struct { params, .. } | TypeDef::Enum { params, .. }) => params.clone(),
            _ => vec![],
        };
        let mut m = HashMap::new();
        let mut args = vec![];
        for p in params {
            let v = self.fresh_known(format!("`{name}` の型引数 `{p}`"));
            args.push(v.clone());
            m.insert(p, v);
        }
        (Ty::Named(name.to_string(), args), m)
    }

    // ---------- 宣言の収集 ----------

    /// 総称の宣言の型引数と制約。`bounded` が偽なら制約を書けない（構造体・列挙型）。
    fn generics(&self, gs: &[Generic], bounded: bool) -> R<Vec<(String, Vec<Prop>)>> {
        let mut out: Vec<(String, Vec<Prop>)> = vec![];
        for g in gs {
            if out.iter().any(|(n, _)| *n == g.name) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{}` が二重にある", g.name),
                );
            }
            if self.types.contains_key(&g.name) {
                return self.err(
                    ErrorKind::DuplicateDefinition,
                    format!("型引数 `{}` が同じ名前の型を隠している", g.name),
                );
            }
            if !bounded && !g.bounds.is_empty() {
                return self.err(
                    ErrorKind::TypeParamBound,
                    format!(
                        "構造体・列挙型の型引数 `{}` には制約を書けない（制約は関数と `impl` の型引数にだけ書ける）",
                        g.name
                    ),
                );
            }
            let mut props = vec![];
            for b in &g.bounds {
                let name = b.join(".");
                let Some((_, p)) = PROPS.iter().find(|(n, _)| *n == name) else {
                    return self.err(
                        ErrorKind::UnknownProperty,
                        format!(
                            "性質 `{name}` はない（使えるのは {}）",
                            PROPS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join("・")
                        ),
                    );
                };
                if !props.contains(p) {
                    props.push(*p);
                }
            }
            out.push((g.name.clone(), props));
        }
        Ok(out)
    }

    fn run(&mut self, file: &File) -> R<()> {
        // 1. 型の名前と型引数
        let mut names: HashSet<String> = HashSet::new();
        let mut shells: Vec<(String, TypeDef)> = vec![];
        for item in &file.items {
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
            let gs = self.generics(generics, false)?;
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
        for item in &file.items {
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
        // 3. 直接の再帰（間接化の型が決まるまでエラー。ADR 0011 の繰り延べの表）
        for item in &file.items {
            let (name, span) = match item {
                Item::Struct(s) => (&s.name, s.span),
                Item::Enum(e) => (&e.name, e.span),
                _ => continue,
            };
            self.span = span;
            if self.reaches(name, name, &mut HashSet::new()) {
                return self.err(
                    ErrorKind::RecursiveType,
                    format!(
                        "型 `{name}` が自分自身を直接含んでいる（間接化の型が決まるまで書けない）"
                    ),
                );
            }
        }
        // 4. 関数とメソッドのシグネチャ
        self.register_std()?;
        self.register_primitives()?;
        for item in &file.items {
            match item {
                Item::Fn(f) => {
                    self.span = f.span;
                    if self.fns.contains_key(&f.name) {
                        return self.err(
                            ErrorKind::DuplicateDefinition,
                            format!("関数 `{}` が二重に宣言されている", f.name),
                        );
                    }
                    let sig = self.signature(f, None, &[])?;
                    self.fns.insert(f.name.clone(), sig);
                }
                Item::Impl(i) => self.register_impl(i, false)?,
                _ => {}
            }
        }
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
                _ => {}
            }
        }
        Ok(())
    }

    fn register_impl(&mut self, i: &ImplDecl, std: bool) -> R<()> {
        self.span = i.span;
        let outer = self.generics(&i.generics, true)?;
        self.tparams = outer.iter().map(|(n, _)| n.clone()).collect();
        let ity = self.resolve(&i.ty, Pos::Other);
        self.tparams.clear();
        let tn = match ity? {
            Ty::Named(tn, _) => tn,
            Ty::Str if std => "String".to_string(),
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
            let sig = self.signature(f, Some((&tn, &i.ty)), &outer)?;
            self.methods.insert(key, sig);
        }
        Ok(())
    }

    /// 関数のシグネチャ。`recv` は `impl` の中のとき（型の名前と、`impl` の型）。`outer` は `impl` の型引数。
    fn signature(
        &mut self,
        f: &FnDecl,
        recv: Option<(&str, &Type)>,
        outer: &[(String, Vec<Prop>)],
    ) -> R<Sig> {
        let own = self.generics(&f.generics, true)?;
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
        self.tparams = generics.iter().map(|(n, _)| n.clone()).collect();
        let r = self.signature_in(f, recv, generics);
        self.tparams.clear();
        r
    }

    fn signature_in(
        &mut self,
        f: &FnDecl,
        recv: Option<(&str, &Type)>,
        generics: Vec<(String, Vec<Prop>)>,
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
            params,
            ret,
            effects_none: f.effects.is_empty(),
            effects,
            has_self,
        })
    }

    /// `effects` 節の名前を調べる（ADR 0010 決定 12）。
    fn effect_terms(&self, effects: &[Effect], params: &[(String, Ty)]) -> R<Vec<EffTerm>> {
        let mut out = vec![];
        for e in effects {
            match e {
                Effect::Category(path) => {
                    let name = path.join(".");
                    if !self.table.valid(&name) {
                        let hint = if name == "browser" {
                            "（`browser` は実行対象の語で、効果ではない。DOM は `dom`、保存領域は `browser.storage`）"
                        } else {
                            ""
                        };
                        return self.err(
                            ErrorKind::UnknownEffect,
                            format!("効果 `{name}` は、効果のカテゴリの表にない{hint}"),
                        );
                    }
                    out.push(EffTerm::Cat(name));
                }
                Effect::Of(x) => {
                    match params
                        .iter()
                        .position(|(n, t)| n == x && matches!(t, Ty::Fn(..)))
                    {
                        Some(i) => out.push(EffTerm::Of(i)),
                        None => {
                            return self.err(
                                ErrorKind::OfNotParam,
                                format!("`of({x})` の `{x}` は、関数の型の引数でなければならない"),
                            )
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// 標準ライブラリの最小部分（`stdlib.rs` の表）を関数とメソッドの表に入れる。
    fn register_std(&mut self) -> R<()> {
        let saved = self.span;
        for (sig, effects) in crate::stdlib::FREE_FNS {
            let (name, s) = self.std_sig(&crate::stdlib::free_source(sig, effects))?;
            self.fns.insert(name, s);
        }
        for (ns, sig, effects) in crate::stdlib::NAMESPACED_FNS {
            let (name, s) = self.std_sig(&crate::stdlib::free_source(sig, effects))?;
            self.fns.insert(format!("{ns}.{name}"), s);
        }
        for (head, fns) in crate::stdlib::IMPLS {
            let file = self.std_parse(&crate::stdlib::impl_source(head, fns))?;
            if let Some(Item::Impl(i)) = file.items.first() {
                self.register_impl(i, true)?;
            }
        }
        self.span = saved;
        Ok(())
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
        let sig = self.signature(f, None, &[])?;
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
            let mut sig = self.signature(f, None, &[])?;
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

    fn resolve(&self, t: &Type, pos: Pos) -> R<Ty> {
        match t {
            Type::Path { path, args } => {
                if path.len() != 1 {
                    return self.err(
                        ErrorKind::UnknownType,
                        format!("型 `{}` が見つからない", path.join(".")),
                    );
                }
                let n = path[0].as_str();
                let arity = if self.tparams.iter().any(|p| p == n) {
                    0
                } else {
                    match n {
                        "Int" | "Float" | "Bool" | "String" | "Unit" => 0,
                        _ => match self.types.get(n) {
                            Some(TypeDef::Alias(_)) => 0,
                            Some(TypeDef::Struct { params, .. } | TypeDef::Enum { params, .. }) => {
                                params.len()
                            }
                            Some(TypeDef::Opaque(k)) => *k,
                            None => {
                                return self.err(
                                    ErrorKind::UnknownType,
                                    format!("型 `{n}` が見つからない（標準ライブラリの型は、後続の PR で扱う）"),
                                )
                            }
                        },
                    }
                };
                if args.len() != arity {
                    return self.err(
                        ErrorKind::TypeArgCount,
                        format!(
                            "型 `{n}` の型引数は {arity} 個だが、{} 個書かれている",
                            args.len()
                        ),
                    );
                }
                if self.tparams.iter().any(|p| p == n) {
                    return Ok(Ty::Param(n.to_string()));
                }
                Ok(match n {
                    "Int" => Ty::Int,
                    "Float" => Ty::Float,
                    "Bool" => Ty::Bool,
                    "String" => Ty::Str,
                    "Unit" => Ty::Unit,
                    _ => {
                        let apos = if pos == Pos::Ret && n == "Option" {
                            Pos::RetOpt
                        } else {
                            Pos::Other
                        };
                        Ty::Named(
                            n.to_string(),
                            args.iter()
                                .map(|a| self.resolve(a, apos))
                                .collect::<R<_>>()?,
                        )
                    }
                })
            }
            Type::Ref { mutable, inner } => {
                if pos == Pos::Other {
                    return self.err(
                        ErrorKind::RefTypePosition,
                        "参照 `&T`・`&mut T` を書けるのは、引数の型の一番外、関数の型の引数の一番外、戻り値の型の一番外と一番外の `Option<…>` の中だけ（構造体のフィールド・列挙子の値・型引数・`let` の注釈には書けない。ADR 0012 の決定 1）",
                    );
                }
                Ok(Ty::Ref(
                    *mutable,
                    Box::new(self.resolve(inner, Pos::Other)?),
                ))
            }
            Type::Tuple(ts) => Ok(Ty::Tuple(
                ts.iter()
                    .map(|t| self.resolve(t, Pos::Other))
                    .collect::<R<_>>()?,
            )),
            Type::Fn { params, ret } => {
                if pos != Pos::Param {
                    return self.err(
                        ErrorKind::FnTypePosition,
                        "関数の型 `fn(...)` は、関数・メソッドの引数の型の一番外にだけ書ける",
                    );
                }
                let ps = params
                    .iter()
                    .map(|t| self.resolve(t, Pos::FnParam))
                    .collect::<R<_>>()?;
                let r = match ret {
                    None => Ty::Unit,
                    Some(r) => self.resolve(r, Pos::Other)?,
                };
                Ok(Ty::Fn(ps, Box::new(r)))
            }
        }
    }

    fn contains_bad_base(&self, t: &Ty) -> bool {
        match t {
            Ty::Ref(..) | Ty::Fn(..) => true,
            Ty::Named(n, a) => {
                matches!(self.types.get(n), Some(TypeDef::Alias(_)))
                    || a.iter().any(|t| self.contains_bad_base(t))
            }
            Ty::Tuple(ts) => ts.iter().any(|t| self.contains_bad_base(t)),
            _ => false,
        }
    }

    /// `from` の定義が（組・構造体・列挙型・篩型をたどって）`target` を直接含むか。標準の入れ物
    /// （`List` など）は中身を別の場所に持つので、含むとは数えない。
    fn reaches(&self, from: &str, target: &str, seen: &mut HashSet<String>) -> bool {
        let tys: Vec<Ty> = match self.types.get(from) {
            Some(TypeDef::Alias(b)) => vec![b.clone()],
            Some(TypeDef::Struct { fields, .. }) => fields.iter().map(|(_, t)| t.clone()).collect(),
            Some(TypeDef::Enum { variants, .. }) => variants
                .iter()
                .flat_map(|(_, ts)| ts.iter().cloned())
                .collect(),
            _ => return false,
        };
        fn names(ck: &Ck, t: &Ty, out: &mut Vec<String>) {
            match t {
                Ty::Named(n, a) => {
                    if !matches!(ck.types.get(n), Some(TypeDef::Opaque(_))) {
                        out.push(n.clone());
                        a.iter().for_each(|t| names(ck, t, out));
                    }
                }
                Ty::Tuple(ts) => ts.iter().for_each(|t| names(ck, t, out)),
                _ => {}
            }
        }
        let mut ns = vec![];
        tys.iter().for_each(|t| names(self, t, &mut ns));
        for n in ns {
            if n == target {
                return true;
            }
            if seen.insert(n.clone()) && self.reaches(&n, target, seen) {
                return true;
            }
        }
        false
    }

    // ---------- 篩型・関数の検査 ----------

    /// 関数 1 つ（または篩型 1 つ）ごとに推論の状態を作り直す。
    fn begin(&mut self) {
        self.subst.clear();
        self.pending.clear();
        self.must_know.clear();
        self.skipped = None;
        self.annot_n = 0;
        self.scopes.clear();
        self.refine = false;
        self.result = None;
        self.ret = Ty::Unit;
        self.eff_stack = vec![vec![]];
        self.fn_params.clear();
        self.local_eff.clear();
        self.defer = false;
    }

    fn add_eff(&mut self, a: Atom) {
        let top = self.eff_stack.last_mut().expect("effect frame");
        if !top.contains(&a) {
            top.push(a);
        }
    }

    fn has_var(&self, t: &Ty) -> bool {
        match self.shallow(t) {
            Ty::Var(_) => true,
            Ty::Tuple(ts) => ts.iter().any(|t| self.has_var(t)),
            Ty::Ref(_, i) => self.has_var(&i),
            Ty::Named(_, a) => a.iter().any(|t| self.has_var(t)),
            Ty::Fn(ps, r) => ps.iter().any(|t| self.has_var(t)) || self.has_var(&r),
            _ => false,
        }
    }

    /// 本体の終わり: 繰り延べた性質の制約を調べ、決まっていない型変数があれば `CannotInfer`。
    fn finish(&mut self) -> R<()> {
        let pend = std::mem::take(&mut self.pending);
        let mut unknown = None;
        for p in &pend {
            match self.holds(&p.ty, p.prop, 0) {
                Some(true) => {}
                Some(false) => {
                    return Err(Error::new(
                        p.kind.clone(),
                        p.span,
                        p.text.replace("{}", &self.sh(&p.ty)),
                    ))
                }
                None => unknown = Some(p.span),
            }
        }
        let mk = std::mem::take(&mut self.must_know);
        for (t, what, span) in &mk {
            if self.has_var(t) {
                return Err(Error::new(
                    ErrorKind::CannotInfer,
                    *span,
                    format!(
                        "{what}の型が決まらない（使い方で型が決まるようにするか、型注釈で示す）"
                    ),
                ));
            }
        }
        if let Some(span) = unknown {
            return Err(Error::new(
                ErrorKind::CannotInfer,
                span,
                "型が決まらないので、性質を調べられない",
            ));
        }
        if let Some(t) = self.skipped.take() {
            self.skipped = Some(self.zonk(&t));
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
        self.finish()
    }

    fn check_fn(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.env = sig.generics.iter().cloned().collect();
        self.tparams = sig.generics.iter().map(|(n, _)| n.clone()).collect();
        let r = self.check_fn_in(f, sig);
        self.env.clear();
        self.tparams.clear();
        self.skip = None;
        r
    }

    fn check_fn_in(&mut self, f: &FnDecl, sig: &Sig) -> R<()> {
        self.skip = None;
        self.annots.clear();
        self.body_once(f, sig)?;
        self.span = f.span;
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

    /// 宣言した効果と、本体から推論した効果の照合（ADR 0004 の規則 3・ADR 0006 の条件 11）。
    fn match_effects(&self, f: &FnDecl, sig: &Sig) -> R<()> {
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
        for t in &self.targets {
            for e in declared.iter().chain(&inferred) {
                if !e.starts_with("of(") && !self.table.allowed_on(e, t) {
                    return self.err(
                        ErrorKind::EffectTarget,
                        format!(
                            "効果 `{e}` は、実行対象 `{t}` では使えない（モジュールの `targets` に `{t}` がある）"
                        ),
                    );
                }
            }
        }
        if let Some(e) = inferred.iter().find(|e| !declared.contains(e)) {
            return self.err(
                ErrorKind::EffectMissing,
                format!(
                    "関数 `{}` の本体は効果 `{e}` を起こすが、`effects` 節に書かれていない（足す）",
                    f.name
                ),
            );
        }
        if let Some(e) = declared.iter().find(|e| !inferred.contains(e)) {
            return self.err(
                ErrorKind::EffectExtra,
                format!(
                    "関数 `{}` の `effects` 節に効果 `{e}` があるが、本体は起こさない（取り除く）",
                    f.name
                ),
            );
        }
        Ok(())
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
        self.ret = sig.ret.clone();
        // 契約の式は効果の推論に数えない（捨てる枠で検査する）
        self.eff_stack.push(vec![]);
        if let Some(r) = &f.requires {
            self.expect(&Ty::Bool, r, "`requires`")?;
        }
        if let Some(e) = &f.ensures {
            self.result = Some(sig.ret.clone());
            let r = self.expect(&Ty::Bool, e, "`ensures`");
            self.result = None;
            r?;
        }
        self.eff_stack.pop();
        self.block(&f.body)?;
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
        self.scopes.pop();
        Ok(())
    }

    fn lookup(&self, n: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|s| s.get(n))
    }

    /// 参照を外して、型変数は解決できるところまで解決した型。
    fn peel(&self, t: &Ty) -> Ty {
        match self.shallow(t) {
            Ty::Ref(_, i) => self.peel(&i),
            t => t,
        }
    }

    fn expect(&mut self, expected: &Ty, e: &Expr, what: &str) -> R<()> {
        let t = self.expr(e)?;
        if !self.coerce(expected, &t, e) {
            return self.err(
                ErrorKind::TypeMismatch,
                format!(
                    "{what}は `{}` でなければならないが、`{}` である{}",
                    self.sh(expected),
                    self.sh(&t),
                    self.check_hint(expected, &t)
                ),
            );
        }
        Ok(())
    }

    fn check_hint(&self, expected: &Ty, actual: &Ty) -> &'static str {
        let (ex, ac) = (self.zonk(expected), self.zonk(actual));
        if self
            .refinement_base(&ex)
            .is_some_and(|b| self.zonk(&b) == ac)
        {
            "（基の型から篩型への変換は、リテラルの定数か `T.check` に限る）"
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
                self.fn_value_form(value, &vt)?;
                if closure {
                    if let Pattern::Bind(n) = pattern {
                        let effs = std::mem::take(&mut self.last_closure);
                        self.local_eff.insert(n.clone(), effs);
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
                                    ErrorKind::TypeMismatch,
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
                self.bind(pattern, &fin, *mutable)
            }
            StmtKind::Assign { target, value } => {
                let root = place_root(target).map(str::to_string);
                let tt = self.expr(target)?;
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
                Ok(())
            }
            StmtKind::Expr(e) => self.expr(e).map(|_| ()),
            StmtKind::Return(v) => {
                let ret = self.ret.clone();
                match v {
                    None => {
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
                        Ok(())
                    }
                    Some(e) => self.expect(&ret, e, "戻り値"),
                }
            }
            StmtKind::If(i) => self.if_stmt(i),
            StmtKind::While { cond, body } => {
                self.expect(&Ty::Bool, cond, "`while` の条件")?;
                self.block(body)
            }
            StmtKind::Match {
                scrutinee, arms, ..
            } => {
                let (st, by_ref) = self.scrutinee(scrutinee)?;
                let st = self.peel(&st);
                let sp = s.span;
                let mut pats = vec![];
                for a in arms {
                    self.span = sp;
                    self.scopes.push(HashMap::new());
                    self.by_ref = by_ref;
                    let r = self.pat(&a.pattern, &st);
                    self.by_ref = false;
                    r?;
                    self.block(&a.body)?;
                    self.scopes.pop();
                    pats.push(&a.pattern);
                }
                self.span = sp;
                self.exhaustive(&pats)
            }
        }
    }

    fn if_stmt(&mut self, i: &IfStmt) -> R<()> {
        self.expect(&Ty::Bool, &i.cond, "`if` の条件")?;
        self.block(&i.then)?;
        match i.otherwise.as_deref() {
            None => Ok(()),
            Some(Else::If(n)) => self.if_stmt(n),
            Some(Else::Block(b)) => self.block(b),
        }
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

    // ---------- パターンと網羅性 ----------

    /// パターンの値の名前（`Some`・`Shape.Circle` など）から、列挙型の名前と値の名前。
    fn variant_of(&self, path: &[String]) -> R<(String, String)> {
        match path {
            [v] => match PRELUDE_VALUES.iter().find(|(n, _)| n == v) {
                Some((n, t)) => Ok((t.to_string(), n.to_string())),
                None => self.err(
                    ErrorKind::UnknownName,
                    format!("値 `{v}` が見つからない（列挙型の値は `型.値` と書く）"),
                ),
            },
            [e, v] => match self.types.get(e) {
                Some(TypeDef::Enum { variants, .. }) => {
                    if variants.iter().any(|(n, _)| n == v) {
                        Ok((e.clone(), v.clone()))
                    } else {
                        self.err(
                            ErrorKind::NoSuchMember,
                            format!("列挙型 `{e}` に値 `{v}` がない"),
                        )
                    }
                }
                _ => self.err(
                    ErrorKind::UnknownType,
                    format!("列挙型 `{e}` が見つからない"),
                ),
            },
            _ => self.err(
                ErrorKind::UnknownName,
                format!("値 `{}` が見つからない", path.join(".")),
            ),
        }
    }

    fn pat_mismatch<T>(&self, p: &str, t: &Ty) -> R<T> {
        self.err(
            ErrorKind::PatternMismatch,
            format!("{p}のパターンだが、調べる式の型は `{}` である", self.sh(t)),
        )
    }

    /// パターンを型 `t` に合わせ、束縛する名前を現在のスコープに宣言する。
    /// `match` の対象の型と、腕の束縛が参照になるか（ADR 0012 の決定 1 の 3）。
    /// `match &x` は所有する場所だけに書ける。参照の名前（`Option<&T>` の変数を含む）と、
    /// それを通して届く場所は `match x` と書く。`&mut` は対象にできない。
    fn scrutinee(&mut self, e: &Expr) -> R<(Ty, bool)> {
        if let Expr::Unary { op, expr } = e {
            if matches!(op, UnOp::Ref | UnOp::RefMut) {
                if *op == UnOp::RefMut {
                    return self.err(
                        ErrorKind::MatchBorrowForm,
                        "`match` の対象に `&mut` は書けない（`&` で借りるか、`match x` と書く）",
                    );
                }
                let Some(root) = place_root(expr).map(str::to_string) else {
                    return self.err(
                        ErrorKind::MatchBorrowForm,
                        "`match &e` の `e` は変数かそのフィールドでなければならない",
                    );
                };
                let t = self.expr(expr)?;
                if self
                    .lookup(&root)
                    .is_some_and(|l| contains_ref(&self.zonk(&l.ty)))
                {
                    return self.err(
                        ErrorKind::MatchBorrowForm,
                        format!(
                            "`{root}` は参照（または参照を含む値）なので、`&` を外して `match` の対象に直接書く（`match {root}`）。腕の束縛は参照になる"
                        ),
                    );
                }
                return Ok((t, true));
            }
        }
        let t = self.expr(e)?;
        let by_ref = contains_ref(&self.zonk(&t));
        Ok((t, by_ref))
    }

    fn pat(&mut self, p: &Pattern, t: &Ty) -> R<()> {
        let t = self.peel(t);
        match p {
            Pattern::Wild => Ok(()),
            Pattern::Bind(n) => {
                let t = if self.by_ref {
                    Ty::Ref(false, Box::new(t))
                } else {
                    t
                };
                self.declare(n, t, false)
            }
            Pattern::Int(s) | Pattern::NegInt(s) => {
                let text = if matches!(p, Pattern::NegInt(_)) {
                    format!("-{s}")
                } else {
                    s.clone()
                };
                if text.parse::<i64>().is_err() {
                    return self.err(
                        ErrorKind::IntLiteralRange,
                        format!("整数のリテラル `{text}` が `Int` の範囲を超えている"),
                    );
                }
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Int) {
                    return self.pat_mismatch("整数", &t);
                }
                Ok(())
            }
            Pattern::Str(_) => {
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Str) {
                    return self.pat_mismatch("文字列", &t);
                }
                Ok(())
            }
            Pattern::Bool(_) => {
                let v = self.val(&t);
                if !self.try_unify(&v, &Ty::Bool) {
                    return self.pat_mismatch("`Bool`", &t);
                }
                Ok(())
            }
            Pattern::Tuple(ps) => {
                let ts = match self.shallow(&t) {
                    Ty::Tuple(ts) => ts,
                    Ty::Var(_) => {
                        let ts: Vec<Ty> = ps.iter().map(|_| self.fresh()).collect();
                        let _ = self.unify(&t, &Ty::Tuple(ts.clone()));
                        ts
                    }
                    other => return self.pat_mismatch("組", &other),
                };
                if ps.len() != ts.len() {
                    return self.err(
                        ErrorKind::PatternMismatch,
                        format!(
                            "{} 個の組のパターンだが、組の要素は {} 個ある",
                            ps.len(),
                            ts.len()
                        ),
                    );
                }
                for (p, t) in ps.iter().zip(&ts) {
                    self.pat(p, t)?;
                }
                Ok(())
            }
            Pattern::Variant { path, args } => {
                let (en, vn) = self.variant_of(path)?;
                let (ty, m) = self.fresh_named(&en);
                if !self.try_unify(&t, &ty) {
                    return self.pat_mismatch(&format!("`{en}`"), &t);
                }
                let Some(TypeDef::Enum { variants, .. }) = self.types.get(&en).cloned() else {
                    return Ok(());
                };
                let (_, fs) = variants.iter().find(|(n, _)| *n == vn).expect("variant");
                if fs.len() != args.len() {
                    return self.err(
                        ErrorKind::PatternMismatch,
                        format!(
                            "値 `{en}.{vn}` の中身は {} 個だが、パターンは {} 個ある",
                            fs.len(),
                            args.len()
                        ),
                    );
                }
                for (a, f) in args.iter().zip(fs) {
                    self.pat(a, &substitute(f, &m))?;
                }
                Ok(())
            }
        }
    }

    fn to_p(&self, p: &Pattern) -> P {
        match p {
            Pattern::Wild | Pattern::Bind(_) => P::Wild,
            Pattern::Int(s) => P::Ctor(Ctor::Lit(s.clone()), vec![]),
            Pattern::NegInt(s) => P::Ctor(Ctor::Lit(format!("-{s}")), vec![]),
            Pattern::Str(s) => P::Ctor(Ctor::Lit(format!("{s:?}")), vec![]),
            Pattern::Bool(b) => P::Ctor(Ctor::Bool(*b), vec![]),
            Pattern::Tuple(ps) => P::Ctor(
                Ctor::Tuple(ps.len()),
                ps.iter().map(|p| self.to_p(p)).collect(),
            ),
            Pattern::Variant { path, args } => match self.variant_of(path) {
                Ok((ty, name)) => P::Ctor(
                    Ctor::Variant { ty, name },
                    args.iter().map(|p| self.to_p(p)).collect(),
                ),
                Err(_) => P::Wild,
            },
        }
    }

    fn enum_shapes(&self, ty: &str) -> Vec<(String, usize)> {
        match self.types.get(ty) {
            Some(TypeDef::Enum { variants, .. }) => variants
                .iter()
                .map(|(n, fs)| (n.clone(), fs.len()))
                .collect(),
            _ => vec![],
        }
    }

    /// 前の腕がすべて覆う値にしか当たらない腕はエラー（ADR 0011 の決定 9 の追記）。
    fn unreachable_arms(&self, pats: &[&Pattern]) -> R<()> {
        let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![self.to_p(p)]).collect();
        let enums = |ty: &str| self.enum_shapes(ty);
        for i in 1..rows.len() {
            if !useful(&rows[..i], &rows[i], &enums) {
                return self.err(
                    ErrorKind::UnreachableArm,
                    format!(
                        "`match` の第 {} 腕は、前の腕がすでに覆っているので到達できない（取り除く）",
                        i + 1
                    ),
                );
            }
        }
        Ok(())
    }

    /// `match` の腕がすべての値を覆うか（ADR 0011 の決定 9）。
    fn exhaustive(&self, pats: &[&Pattern]) -> R<()> {
        self.unreachable_arms(pats)?;
        let rows: Vec<Vec<P>> = pats.iter().map(|p| vec![self.to_p(p)]).collect();
        let enums = |ty: &str| self.enum_shapes(ty);
        match missing(&rows, 1, &enums) {
            None => Ok(()),
            Some(w) => self.err(
                ErrorKind::NotExhaustive,
                format!(
                    "`match` が値をすべて覆っていない: `{}` の場合がない（腕を足すか、`_` を最後に置く）",
                    w.first().cloned().unwrap_or_default()
                ),
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
                let t = self.expr(inner)?;
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
            Expr::Tuple(es) => Ok(Ty::Tuple(
                es.iter().map(|e| self.expr(e)).collect::<R<_>>()?,
            )),
            Expr::Unary { op, expr } => self.unary(*op, expr),
            Expr::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs),
            Expr::Field { expr, name } => self.field(expr, name),
            Expr::Call { callee, args } => self.call(callee, args),
            Expr::StructLit { path, fields } => self.struct_lit(path, fields),
            Expr::Try(inner) => self.try_op(inner),
            Expr::Match {
                scrutinee, arms, ..
            } => {
                let (st, by_ref) = self.scrutinee(scrutinee)?;
                let st = self.peel(&st);
                let sp = self.span;
                let r = self.fresh();
                let mut pats = vec![];
                for (i, a) in arms.iter().enumerate() {
                    self.scopes.push(HashMap::new());
                    self.by_ref = by_ref;
                    let pr = self.pat(&a.pattern, &st);
                    self.by_ref = false;
                    pr?;
                    let t = self.expr(&a.body)?;
                    self.span = sp;
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
                    self.scopes.pop();
                    pats.push(&a.pattern);
                }
                self.exhaustive(&pats)?;
                Ok(r)
            }
            Expr::Closure { params, body, .. } => self.closure(params, body, hint),
        }
    }

    fn closure(&mut self, params: &[String], body: &ClosureBody, hint: Option<&Ty>) -> R<Ty> {
        let (hps, hret) = match hint.map(|h| self.shallow(h)) {
            Some(Ty::Fn(ps, r)) => (Some(ps), Some(*r)),
            _ => (None, None),
        };
        if let Some(ps) = &hps {
            if ps.len() != params.len() {
                return self.err(
                    ErrorKind::ArgCount,
                    format!(
                        "クロージャの引数は {} 個だが、渡す先は {} 個の引数の関数を求めている",
                        params.len(),
                        ps.len()
                    ),
                );
            }
        }
        let defer = std::mem::take(&mut self.defer);
        self.eff_stack.push(vec![]);
        self.scopes.push(HashMap::new());
        let mut pts = vec![];
        for (i, p) in params.iter().enumerate() {
            self.shadow_check(p)?;
            let t = match &hps {
                Some(ps) => ps[i].clone(),
                None => self.fresh_known(format!("クロージャの引数 `{p}`")),
            };
            self.scopes.last_mut().expect("scope").insert(
                p.clone(),
                Local {
                    ty: t.clone(),
                    mutable: false,
                    param: true,
                },
            );
            pts.push(t);
        }
        let saved = self.ret.clone();
        let r = match hret {
            Some(r) => r,
            None => self.fresh(),
        };
        self.ret = r.clone();
        let sp = self.span;
        let res: R<()> = match body {
            ClosureBody::Expr(e) => match self.expr(e) {
                Ok(t) => {
                    self.span = sp;
                    if self.coerce(&r, &t, e) {
                        Ok(())
                    } else {
                        self.err(
                            ErrorKind::TypeMismatch,
                            format!(
                                "クロージャの戻り値は `{}` でなければならないが、`{}` である",
                                self.sh(&r),
                                self.sh(&t)
                            ),
                        )
                    }
                }
                Err(e) => Err(e),
            },
            ClosureBody::Block(b) => self.block(b).and_then(|()| {
                self.span = sp;
                if !block_returns(b) && !self.try_unify(&r, &Ty::Unit) {
                    return self.err(
                        ErrorKind::MissingReturn,
                        format!(
                            "クロージャは `{}` を返すので、すべての道が `return` で終わらなければならない",
                            self.sh(&r)
                        ),
                    );
                }
                Ok(())
            }),
        };
        self.ret = saved;
        self.scopes.pop();
        let effs = self.eff_stack.pop().unwrap_or_default();
        if defer {
            self.last_closure = effs;
        } else {
            for a in effs {
                self.add_eff(a);
            }
        }
        res?;
        Ok(Ty::Fn(pts, Box::new(r)))
    }

    fn try_op(&mut self, inner: &Expr) -> R<Ty> {
        let t = self.expr(inner)?;
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
        Ok(Ty::Fn(
            sig.params.iter().map(|(_, t)| substitute(t, &m)).collect(),
            Box::new(substitute(&sig.ret, &m)),
        ))
    }

    fn name(&mut self, n: &str) -> R<Ty> {
        if self.refine && n != "self" {
            return self.err(
                ErrorKind::RefinementName,
                format!(
                    "篩型の条件で値として参照できるのは `self` とリテラルだけ（`{n}` は書けない）"
                ),
            );
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
                        "`&`・`&mut` で借りられるのは、参照の引数への実引数と `match` の対象だけ（`let r = &x;` や戻り値・フィールドの値には書けない。ADR 0012 の決定 1）",
                    );
                }
                let t = self.expr(inner)?;
                if mutable {
                    let Some(root) = place_root(inner).map(str::to_string) else {
                        return self.err(
                            ErrorKind::BorrowOfNonPlace,
                            "`&mut` で借りられるのは変数とそのフィールドだけ",
                        );
                    };
                    self.assignable_place(&root, inner, false)?;
                }
                Ok(Ty::Ref(mutable, Box::new(t)))
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

    /// `T.名前`（`T` が型）のとき、その型の名前。
    fn type_path(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Name(n) if self.lookup(n).is_none() && self.types.contains_key(n) => {
                Some(n.clone())
            }
            _ => None,
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
        let t = self.expr(recv)?;
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

    /// 関数の型 `ft` の値を `args` で呼ぶ。
    fn apply(&mut self, ft: Ty, args: &[Expr], what: &str, eff: Eff) -> R<Ty> {
        match self.shallow(&ft) {
            Ty::Fn(ps, r) => {
                self.args(&ps, args, what, &eff)?;
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
                            "関数 `{n}` が見つからない（標準ライブラリの関数は、後続の PR で扱う）"
                        ),
                    );
                }
                let ft = self.name(n)?;
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
                self.apply(ft, args, &format!("`{n}`"), eff)
            }
            Expr::Field { expr, name } => {
                // 名前空間の関数: `fs.read(p)`・`console.log(x)`。
                if let Expr::Name(ns) = &**expr {
                    let key = format!("{ns}.{name}");
                    if self.lookup(ns).is_none() && self.fns.contains_key(&key) {
                        let sig = self.fns[&key].clone();
                        let what = format!("`{key}`");
                        let ft = self.fn_type(&sig, &what)?;
                        let eff = Eff {
                            terms: sig.effects.clone(),
                            ..Eff::default()
                        };
                        return self.apply(ft, args, &what, eff);
                    }
                }
                if let Some(tn) = self.type_path(expr) {
                    match self.types.get(&tn).cloned() {
                        Some(TypeDef::Enum { variants, .. })
                            if variants.iter().any(|(v, _)| v == name) =>
                        {
                            let ft = self.variant_value(&tn, name)?;
                            return self.apply(ft, args, &format!("`{tn}.{name}`"), Eff::default());
                        }
                        Some(TypeDef::Alias(base)) if name == "check" => {
                            let ft = self.check_fn_type(&tn, &base);
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
                    let Some(sig) = self.methods.get(&(tn.clone(), name.clone())).cloned() else {
                        return self.err(
                            ErrorKind::NoSuchMember,
                            format!("`{tn}` に `{name}` という関数・値がない"),
                        );
                    };
                    if sig.has_self {
                        return self.err(
                            ErrorKind::NotCallable,
                            format!("`{tn}.{name}` は受け手のあるメソッドなので、値に対して `x.{name}(...)` と呼ぶ"),
                        );
                    }
                    let what = format!("`{tn}.{name}`");
                    let ft = self.fn_type(&sig, &what)?;
                    let eff = Eff {
                        terms: sig.effects.clone(),
                        ..Eff::default()
                    };
                    return self.apply(ft, args, &what, eff);
                }
                // メソッド呼び出し x.m(...)
                let rt0 = self.expr(expr)?;
                let rt = self.peel(&rt0);
                let (tn, _) = match &rt {
                    Ty::Named(tn, a) => (tn.clone(), a.clone()),
                    Ty::Str => ("String".to_string(), vec![]),
                    Ty::Var(_) => {
                        return self.err(
                            ErrorKind::CannotInfer,
                            format!("メソッド `{name}` を呼ぶ式の型が決まっていない（先に型を決める）"),
                        )
                    }
                    Ty::Param(p) => {
                        return self.err(
                            ErrorKind::NoSuchMember,
                            format!("型引数 `{p}` にはメソッド `{name}` がない（性質の制約で使えるのは演算子だけ）"),
                        )
                    }
                    other => {
                        return self.unsupported(&format!(
                            "標準ライブラリの型 `{}` のメソッド `{name}`",
                            show(&self.zonk(other))
                        ))
                    }
                };
                let Some(sig) = self.methods.get(&(tn.clone(), name.clone())).cloned() else {
                    return self.err(
                        ErrorKind::NoSuchMember,
                        format!("型 `{tn}` にメソッド `{name}` がない"),
                    );
                };
                if !sig.has_self {
                    return self.err(
                        ErrorKind::NotCallable,
                        format!("`{name}` は受け手のない関数なので、`{tn}.{name}(...)` と呼ぶ"),
                    );
                }
                let what = format!("メソッド `{tn}.{name}`");
                let m = self.instantiate(&sig.generics, &what)?;
                let selfty = self.peel(&substitute(&sig.params[0].1, &m));
                if !self.try_unify(&selfty, &rt) {
                    return self.err(
                        ErrorKind::TypeMismatch,
                        format!(
                            "{what}の受け手は `{}` だが、`{}` である",
                            self.sh(&selfty),
                            self.sh(&rt)
                        ),
                    );
                }
                if let Ty::Ref(true, _) = sig.params[0].1 {
                    let Some(root) = place_root(expr).map(str::to_string) else {
                        return self.err(
                            ErrorKind::BorrowOfNonPlace,
                            format!("`{name}` は `&mut` の受け手を取るので、変数に対して呼ぶ"),
                        );
                    };
                    self.assignable_place(&root, expr, false)?;
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
                self.args(&ps, args, &what, &eff)?;
                Ok(substitute(&sig.ret, &m))
            }
            _ => self.err(
                ErrorKind::NotCallable,
                "呼び出せるのは関数・メソッド・値の構築子だけ",
            ),
        }
    }

    /// 引数として渡された関数の効果。
    fn fn_effects(&self, a: &Expr) -> Vec<Atom> {
        match strip_paren(a) {
            Expr::Name(n) => {
                if self.lookup(n).is_some() {
                    if self.fn_params.contains(n) {
                        vec![Atom::Of(n.clone())]
                    } else {
                        self.local_eff.get(n).cloned().unwrap_or_default()
                    }
                } else {
                    self.fns.get(n).map_or_else(Vec::new, |sig| {
                        sig.effects
                            .iter()
                            .filter_map(|t| match t {
                                EffTerm::Cat(c) => Some(Atom::Cat(c.clone())),
                                // TODO: 関数の型を入れ子にできるようにしたら、
                                // ここで `of(..)` を実引数の効果へ展開する必要がある。
                                EffTerm::Of(_) => None,
                            })
                            .collect()
                    })
                }
            }
            _ => vec![],
        }
    }

    fn args(&mut self, params: &[Ty], args: &[Expr], what: &str, eff: &Eff) -> R<()> {
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
        for (i, (p, a)) in params.iter().zip(args).enumerate() {
            let closure = matches!(strip_paren(a), Expr::Closure { .. });
            self.defer = closure;
            self.ref_ok = matches!(
                a,
                Expr::Unary {
                    op: UnOp::Ref | UnOp::RefMut,
                    ..
                }
            ) && matches!(self.shallow(p), Ty::Ref(..));
            let t = self.expr_hint(a, Some(p));
            self.defer = false;
            self.ref_ok = false;
            let t = t?;
            self.span = sp;
            self.fn_value_form(a, &t)?;
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
                    ErrorKind::TypeMismatch,
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
        Ok(())
    }

    fn struct_lit(&mut self, path: &[String], fields: &[(String, Expr)]) -> R<Ty> {
        let name = path.join(".");
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

fn strip_paren(e: &Expr) -> &Expr {
    match e {
        Expr::Paren(i) => strip_paren(i),
        e => e,
    }
}

/// 型から参照を外した形。
fn strip_refs(t: &Ty) -> Ty {
    match t {
        Ty::Ref(_, i) => strip_refs(i),
        Ty::Named(n, a) => Ty::Named(n.clone(), a.iter().map(strip_refs).collect()),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(strip_refs).collect()),
        Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(strip_refs).collect(), Box::new(strip_refs(r))),
        t => t.clone(),
    }
}

/// 型のどこかに参照を含むか。
fn contains_ref(t: &Ty) -> bool {
    match t {
        Ty::Ref(..) => true,
        Ty::Named(_, a) => a.iter().any(contains_ref),
        Ty::Tuple(ts) => ts.iter().any(contains_ref),
        Ty::Fn(ps, r) => ps.iter().any(contains_ref) || contains_ref(r),
        _ => false,
    }
}

/// 関数の型の中を数えずに、型のどこかに参照を含むか（借用の引数の判定）。
fn contains_ref_outside_fn(t: &Ty) -> bool {
    match t {
        Ty::Ref(..) => true,
        Ty::Named(_, a) => a.iter().any(contains_ref_outside_fn),
        Ty::Tuple(ts) => ts.iter().any(contains_ref_outside_fn),
        _ => false,
    }
}

fn place_root(e: &Expr) -> Option<&str> {
    match e {
        Expr::Name(n) => Some(n),
        Expr::SelfValue => Some("self"),
        Expr::Field { expr, .. } => place_root(expr),
        Expr::Paren(i) => place_root(i),
        _ => None,
    }
}

/// 基の型のリテラルの定数か（`-` を前置した数も含む）。
fn is_literal(e: &Expr) -> bool {
    match e {
        Expr::Int(_) | Expr::Float(_) | Expr::Str(_) | Expr::Bool(_) => true,
        Expr::Paren(i) => is_literal(i),
        Expr::Unary {
            op: UnOp::Neg,
            expr,
        } => matches!(**expr, Expr::Int(_) | Expr::Float(_)),
        _ => false,
    }
}

fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn block_returns(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(_) => true,
        StmtKind::If(i) => if_returns(i),
        StmtKind::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|a| block_returns(&a.body))
        }
        _ => false,
    })
}

fn if_returns(i: &IfStmt) -> bool {
    block_returns(&i.then)
        && match i.otherwise.as_deref() {
            None => false,
            Some(Else::If(n)) => if_returns(n),
            Some(Else::Block(b)) => block_returns(b),
        }
}

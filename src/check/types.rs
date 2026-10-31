use std::collections::HashMap;

use super::{Ck, EffTerm};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Ty {
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
    /// 関数の型。3 つ目は、置ける位置（フィールド・列挙型の値の中身・戻り値・
    /// `let` の型注釈、ADR 0022 決定 3）に置かれたときに宣言された効果（カテゴリのみ）。
    /// 引数の位置など従来の位置では `None`。
    Fn(Vec<Ty>, Box<Ty>, Option<Vec<EffTerm>>),
}

pub(super) fn show(t: &Ty) -> String {
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
        Ty::Fn(ps, r, eff) => {
            let base = if **r == Ty::Unit {
                format!("fn({})", list(ps))
            } else {
                format!("fn({}) -> {}", list(ps), show(r))
            };
            match eff {
                None => base,
                Some(ds) if ds.is_empty() => format!("{base} effects none"),
                Some(ds) => {
                    let names: Vec<String> = ds
                        .iter()
                        .map(|t| match t {
                            EffTerm::Cat(c) => c.clone(),
                            EffTerm::Of(n) => format!("of({n})"),
                        })
                        .collect();
                    format!("{base} effects {}", names.join(", "))
                }
            }
        }
    }
}

/// 型引数の制約に書ける性質（ADR 0011 の決定 6）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Prop {
    Copyable,
    Equatable,
    Comparable,
    Numeric,
    Sendable,
    Shareable,
    Displayable,
}

pub(super) const PROPS: &[(&str, Prop)] = &[
    ("Copyable", Prop::Copyable),
    ("Equatable", Prop::Equatable),
    ("Comparable", Prop::Comparable),
    ("Numeric", Prop::Numeric),
    ("Sendable", Prop::Sendable),
    ("Shareable", Prop::Shareable),
    ("Displayable", Prop::Displayable),
];

impl Prop {
    pub(super) fn name(self) -> &'static str {
        PROPS.iter().find(|(_, p)| *p == self).expect("prop").0
    }
}

#[derive(Debug, Clone)]
pub(super) enum TypeDef {
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
pub(super) struct Sig {
    /// 型引数（`impl` のものを先に、関数のものを後に）と制約。
    pub(super) generics: Vec<(String, Vec<Prop>)>,
    /// 型引数のトレイトの制約（型引数の名前, トレイトの名前。ADR 0022 決定 2）。
    pub(super) trait_bounds: Vec<(String, String)>,
    pub(super) params: Vec<(String, Ty)>,
    pub(super) ret: Ty,
    pub(super) effects_none: bool,
    /// 宣言した効果（`of(f)` は引数の位置で持つ）。
    pub(super) effects: Vec<EffTerm>,
    pub(super) has_self: bool,
}

/// 構造体・列挙型の型引数を宣言の順に並べる。
pub(super) fn args_named(def: Option<&TypeDef>, m: &HashMap<String, Ty>) -> Vec<(String, Ty)> {
    match def {
        Some(TypeDef::Struct { params, .. } | TypeDef::Enum { params, .. }) => params
            .iter()
            .filter_map(|p| m.get(p).map(|t| (p.clone(), t.clone())))
            .collect(),
        _ => vec![],
    }
}

/// 型の頭（型の構築子の名前と型引数）。頭のない型（型引数そのもの・組・関数の型・参照でない基本でない型）は `None`。
/// 参照は剥がして中を見る。
pub(super) fn head_of(t: &Ty) -> Option<(String, Vec<Ty>)> {
    match t {
        Ty::Named(n, a) => Some((n.clone(), a.clone())),
        Ty::Int => Some(("Int".into(), vec![])),
        Ty::Float => Some(("Float".into(), vec![])),
        Ty::Bool => Some(("Bool".into(), vec![])),
        Ty::Str => Some(("String".into(), vec![])),
        Ty::Ref(_, i) => head_of(i),
        _ => None,
    }
}

/// 型引数を `Param` として持つ型のパターン `pat` を具体的な型 `z` に対応させ、型引数の対応を作る。
pub(super) fn match_pattern(ck: &Ck, pat: &Ty, z: &Ty) -> Option<HashMap<String, Ty>> {
    fn go(ck: &Ck, pat: &Ty, z: &Ty, m: &mut HashMap<String, Ty>) -> Option<()> {
        let z = ck.shallow(z);
        match pat {
            Ty::Param(p) => match m.get(p) {
                Some(prev) => {
                    if *prev == z {
                        Some(())
                    } else {
                        None
                    }
                }
                None => {
                    m.insert(p.clone(), z.clone());
                    Some(())
                }
            },
            Ty::Named(n, a) => match &z {
                Ty::Named(n2, a2) if n == n2 && a.len() == a2.len() => {
                    for (x, y) in a.iter().zip(a2.iter()) {
                        go(ck, x, y, m)?;
                    }
                    Some(())
                }
                _ => None,
            },
            Ty::Ref(mu, i) => match &z {
                Ty::Ref(mu2, i2) if mu == mu2 => go(ck, i, i2, m),
                _ => None,
            },
            Ty::Tuple(ts) => match &z {
                Ty::Tuple(zs) if ts.len() == zs.len() => {
                    for (x, y) in ts.iter().zip(zs.iter()) {
                        go(ck, x, y, m)?;
                    }
                    Some(())
                }
                _ => None,
            },
            Ty::Fn(ps, r, _) => match &z {
                Ty::Fn(ps2, r2, _) if ps.len() == ps2.len() => {
                    for (x, y) in ps.iter().zip(ps2.iter()) {
                        go(ck, x, y, m)?;
                    }
                    go(ck, r, r2, m)
                }
                _ => None,
            },
            other => {
                if *other == z {
                    Some(())
                } else {
                    None
                }
            }
        }
    }
    let mut m = HashMap::new();
    go(ck, pat, z, &mut m)?;
    Some(m)
}

/// 型が型引数 `p` を含むか。
pub(super) fn contains_param(t: &Ty, p: &str) -> bool {
    match t {
        Ty::Param(q) => q == p,
        Ty::Tuple(ts) => ts.iter().any(|t| contains_param(t, p)),
        Ty::Ref(_, i) => contains_param(i, p),
        Ty::Named(_, a) => a.iter().any(|t| contains_param(t, p)),
        Ty::Fn(ps, r, _) => ps.iter().any(|t| contains_param(t, p)) || contains_param(r, p),
        _ => false,
    }
}

pub(super) fn substitute(t: &Ty, m: &HashMap<String, Ty>) -> Ty {
    let go = |ts: &[Ty]| ts.iter().map(|t| substitute(t, m)).collect::<Vec<_>>();
    match t {
        Ty::Param(p) => m.get(p).cloned().unwrap_or_else(|| t.clone()),
        Ty::Tuple(ts) => Ty::Tuple(go(ts)),
        Ty::Ref(mu, i) => Ty::Ref(*mu, Box::new(substitute(i, m))),
        Ty::Named(n, a) => Ty::Named(n.clone(), go(a)),
        Ty::Fn(ps, r, e) => Ty::Fn(go(ps), Box::new(substitute(r, m)), e.clone()),
        _ => t.clone(),
    }
}

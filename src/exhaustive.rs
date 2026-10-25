//! `match` の網羅性の検査（ADR 0011 の決定 9）。
//!
//! 古典的な行列の特殊化による。列挙型の値・`Bool`・組は構成子の集合が有限なので、すべての構成子が
//! 現れるかを調べ、整数・文字列のリテラルは無限なのでワイルドカードの腕を要する。

/// 検査用に単純化したパターン。
#[derive(Debug, Clone)]
pub enum P {
    Wild,
    Ctor(Ctor, Vec<P>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ctor {
    Variant { ty: String, name: String },
    Bool(bool),
    Tuple(usize),
    Lit(String),
}

fn show_ctor(c: &Ctor, args: &[String]) -> String {
    let tail = if args.is_empty() {
        String::new()
    } else {
        format!("({})", args.join(", "))
    };
    match c {
        Ctor::Variant { ty, name } => {
            if ty == "Option" || ty == "Result" {
                format!("{name}{tail}")
            } else {
                format!("{ty}.{name}{tail}")
            }
        }
        Ctor::Bool(b) => b.to_string(),
        Ctor::Tuple(_) => format!("({})", args.join(", ")),
        Ctor::Lit(s) => s.clone(),
    }
}

fn specialize(rows: &[Vec<P>], c: &Ctor, arity: usize) -> Vec<Vec<P>> {
    let mut out = vec![];
    for r in rows {
        match &r[0] {
            P::Wild => {
                let mut v = vec![P::Wild; arity];
                v.extend_from_slice(&r[1..]);
                out.push(v);
            }
            P::Ctor(c2, args) if c2 == c => {
                let mut v = args.clone();
                v.extend_from_slice(&r[1..]);
                out.push(v);
            }
            P::Ctor(..) => {}
        }
    }
    out
}

fn default_matrix(rows: &[Vec<P>]) -> Vec<Vec<P>> {
    rows.iter()
        .filter(|r| matches!(r[0], P::Wild))
        .map(|r| r[1..].to_vec())
        .collect()
}

/// 列数 `n` の行列が網羅していなければ、取りこぼす値の一例（各列の表示）を返す。
/// `enums(型名)` は、列挙型の値の名前と中身の数の一覧。
pub fn missing(
    rows: &[Vec<P>],
    n: usize,
    enums: &dyn Fn(&str) -> Vec<(String, usize)>,
) -> Option<Vec<String>> {
    if n == 0 {
        return if rows.is_empty() { Some(vec![]) } else { None };
    }
    let mut used: Vec<Ctor> = vec![];
    for r in rows {
        if let P::Ctor(c, _) = &r[0] {
            if !used.contains(c) {
                used.push(c.clone());
            }
        }
    }
    let sig: Option<Vec<(Ctor, usize)>> = match used.first() {
        Some(Ctor::Variant { ty, .. }) => Some(
            enums(ty)
                .into_iter()
                .map(|(name, ar)| {
                    (
                        Ctor::Variant {
                            ty: ty.clone(),
                            name,
                        },
                        ar,
                    )
                })
                .collect(),
        ),
        Some(Ctor::Bool(_)) => Some(vec![(Ctor::Bool(false), 0), (Ctor::Bool(true), 0)]),
        Some(Ctor::Tuple(k)) => Some(vec![(Ctor::Tuple(*k), *k)]),
        _ => None,
    };
    let Some(sig) = sig else {
        let def = default_matrix(rows);
        return missing(&def, n - 1, enums).map(|w| {
            let mut out = vec!["_".to_string()];
            out.extend(w);
            out
        });
    };
    if sig.iter().all(|(c, _)| used.contains(c)) {
        for (c, ar) in &sig {
            let spec = specialize(rows, c, *ar);
            if let Some(mut w) = missing(&spec, ar + n - 1, enums) {
                let rest = w.split_off(*ar);
                let mut out = vec![show_ctor(c, &w)];
                out.extend(rest);
                return Some(out);
            }
        }
        None
    } else {
        let def = default_matrix(rows);
        missing(&def, n - 1, enums).map(|w| {
            let (c, ar) = sig
                .iter()
                .find(|(c, _)| !used.contains(c))
                .expect("missing ctor");
            let mut out = vec![show_ctor(c, &vec!["_".to_string(); *ar])];
            out.extend(w);
            out
        })
    }
}

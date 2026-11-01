//! 標準ライブラリの最小部分のシグネチャと効果の表（M1）。
//!
//! 効果を持たない部分の一覧は ADR 0013 の決定 2 が、効果を持つ部分（`Path`・`IoError`・`console`・`fs`・
//! `Shared`・`Cell`）を含む全体の層への振り分けは ADR 0014 の決定 2 が正式に決めたもの（M3 で実行系が
//! 本体を持つ）。この表は本体を持たず、シグネチャと効果だけを持つ。型検査器はこの表を `Sigil` のシグネチャとして読み、利用者の宣言と同じ表に入れる。
//!
//! 層（ADR 0014 の決定 2・4）: 各表の行に `Layer` の列を持つ。`Core` は OS に依存しない部分（仮のモジュール
//! `std.core`、`targets bare, browser, server`）、`Os` は OS の機能に意味が依存する部分（`std.os`、`targets browser, server`）。
//! 行を層の間で移すのは 1 語の変更だが、`Os` の行の集合は単体テストで ADR の一覧に固定してある。
//!
//! 表の書き方:
//! - 自由関数（`FREE_FNS`）: 名前空間のない関数。`Copyable` な値だけを受け取るものに限る。
//!   標準の自由関数の名前は予約名で、利用者は同じ名前の関数を宣言できない（ADR 0012 の決定 5）。
//! - 名前空間の関数（`NAMESPACED_FNS`）: `fs.read(&p)`・`console.log(&x)` のように呼ぶ。
//! - メソッドと型に付く関数（`IMPLS`）: `impl` の見出し（型引数と型）と、その中の関数。
//!
//! 規則（ADR 0012 の決定 6）:
//! - 読むだけの操作は `&T` の受け手のメソッドにする（`xs.len()`）。受け手は自動で借用される。
//! - 値を複製して返す操作は `T: Copyable` を要求する。
//! - 値を決められない入力は `requires` で除く（`REQUIRES`）。本体は M3 まで持たない。
//!
//! 仮の決めごと（標準ライブラリの最小部分を決める ADR まで暫定）:
//! - `List` の取り出しは参照を返す（`get`: `Option<&T>`。ADR 0012 の決定 3）。

/// 標準ライブラリの層（ADR 0014 の決定 2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// OS に依存しない部分（メモリの確保は使う）。
    Core,
    /// OS の機能に意味が依存する部分。
    Os,
}

/// (シグネチャ, 効果)。効果は `effects` 節の中身（`none`・`console`・`of(f)` など）。
pub type Entry = (&'static str, &'static str);

/// 標準の型で、中身を見せないもの（名前, 型引数の数）。
pub const OPAQUE_TYPES: &[(&str, usize, Layer)] = &[
    ("Bytes", 0, Layer::Core),
    ("List", 1, Layer::Core),
    ("Path", 0, Layer::Os),
    ("IoError", 0, Layer::Os),
    ("ParseError", 0, Layer::Core),
    ("Shared", 1, Layer::Core),
    ("Cell", 1, Layer::Core),
    // 番地（ADR 0016 の決定 1）。`Copyable` で演算子を持たない。番地の計算は原始操作で行う。
    ("Addr", 0, Layer::Core),
];

pub mod core_sigil;

/// 原始操作の表の効果の列で、起こす作用が引数で決まること（システムコール）を表す語（ADR 0016 の決定 4）。
/// この語の操作を呼ぶ関数に限り、効果の検査は「宣言が推論を含む」になる。
pub const DELEGATED: &str = "宣言に委ねる";

/// 原始操作の表の 1 行（ADR 0016 の決定 1）。名前空間 `raw` に置く。
#[derive(Debug, Clone, Copy)]
pub struct RawRow {
    pub layer: Layer,
    /// `raw.` を除いたシグネチャ（例: `alloc(size: Int, align: Int) -> Addr`）。
    pub signature: &'static str,
    /// `effects` 節の中身か、`DELEGATED`。
    pub effects: &'static str,
    /// 実行時に検査する `requires`（式。ない行は `none`）。
    pub requires: &'static str,
    /// 式で書けない前提の文（日本語。空にできない）。
    pub premise: &'static str,
}

/// 原始操作の表。行を足す・変える・消すときは、先に ADR に記録する（ADR 0016 の決定 1）。
/// 実際の一覧は ADR 0018 決定 6 および ADR 0025 決定 1 に基づく。
pub const RAW_OPS: &[RawRow] = &[
    RawRow {
        layer: Layer::Core,
        signature: "addr_null() -> Addr",
        effects: "none",
        requires: "none",
        premise: "常にヌル番地（0）を返す",
    },
    RawRow {
        layer: Layer::Core,
        signature: "addr_to_int(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地のビットパターンを整数として読む",
    },
    RawRow {
        layer: Layer::Core,
        signature: "int_to_addr(v: Int) -> Addr",
        effects: "none",
        requires: "none",
        premise: "非負の整数を番地として解釈する",
    },
    RawRow {
        layer: Layer::Core,
        signature: "offset(a: Addr, n: Int) -> Addr",
        effects: "none",
        requires: "none",
        premise: "同一チャンクの境界内でのみオフセットを適用する",
    },
    RawRow {
        layer: Layer::Core,
        signature: "load_u8(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、1 バイト読み取り可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "store_u8(a: Addr, v: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、値は 0..=255 である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "load_i64(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、8 バイト境界かつ読み取り可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "store_i64(a: Addr, v: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ書き込み可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "load_addr(a: Addr) -> Addr",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、8 バイト境界かつ読み取り可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "store_addr(a: Addr, v: Addr)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ書き込み可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "copy(dst: Addr, src: Addr, n: Int)",
        effects: "none",
        requires: "none",
        premise: "転送元および転送先がそれぞれ n バイトの有効な領域である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "set(dst: Addr, byte: Int, n: Int)",
        effects: "none",
        requires: "none",
        premise: "対象番地から n バイトが有効な書き込み可能領域である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "alloc(size: Int, align: Int) -> Addr",
        effects: "none",
        requires: "none",
        premise: "確保器が初期化済みであり、要求サイズが有効である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "free(a: Addr, size: Int, align: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が alloc で得られた生きた領域の先頭であり、サイズ・アライメントが一致する",
    },
    RawRow {
        layer: Layer::Os,
        signature: "syscall(n: Int, a0: Int, a1: Int, a2: Int, a3: Int, a4: Int, a5: Int) -> Int",
        effects: DELEGATED,
        requires: "none",
        premise: "システムコール番号と引数がカーネル ABI に合致し、ポインタ引数は生きた領域を指す",
    },
];

/// 名前空間のない関数。読むだけの操作はメソッドに移したので、`Copyable` な値の関数だけが残る。
pub const FREE_FNS: &[(Layer, &str, &str)] = &[
    (Layer::Core, "to_float(n: Int) -> Float", "none"),
    (Layer::Core, "to_int(x: Float) -> Option<Int>", "none"),
    (Layer::Core, "ct_eq(a: &Bytes, b: &Bytes) -> Bool", "none"),
];

/// 名前空間の関数（名前空間, シグネチャ, 効果）。読むだけの引数は `&T` で受ける。
pub const NAMESPACED_FNS: &[(Layer, &str, &str, &str)] = &[
    (
        Layer::Os,
        "console",
        "log<T: Displayable>(x: &T)",
        "console",
    ),
    (
        Layer::Os,
        "fs",
        "read(p: &Path) -> Result<String, IoError>",
        "fs.read",
    ),
    (
        Layer::Os,
        "fs",
        "write(p: &Path, s: &String) -> Result<Unit, IoError>",
        "fs.write",
    ),
];

/// シグネチャ（`Entry` の最初の要素）に対応する `requires` 節。ない関数は `none`。
pub const REQUIRES: &[(&str, &str)] = &[
    ("min(self: &List<T>) -> &T", "self.len() > 0"),
    ("max(self: &List<T>) -> &T", "self.len() > 0"),
];

/// `impl` の見出し（例: `<T> List<T>`）と、その中の関数。
pub const IMPLS: &[(Layer, &str, &[Entry])] = &[
    (
        Layer::Core,
        "<T> List<T>",
        &[
            ("new() -> List<T>", "none"),
            ("push(self: &mut List<T>, x: T)", "none"),
            ("get(self: &List<T>, i: Int) -> Option<&T>", "none"),
            (
                "get_mut(self: &mut List<T>, i: Int) -> Option<&mut T>",
                "none",
            ),
            ("len(self: &List<T>) -> Int", "none"),
            ("is_empty(self: &List<T>) -> Bool", "none"),
            ("map<U>(self: &List<T>, f: fn(T) -> U) -> List<U>", "of(f)"),
            (
                "find(self: &List<T>, pred: fn(&T) -> Bool) -> Option<&T>",
                "of(pred)",
            ),
            (
                "max_by<K: Comparable>(self: &List<T>, key: fn(&T) -> K) -> Option<&T>",
                "of(key)",
            ),
            (
                "min_by<K: Comparable>(self: &List<T>, key: fn(&T) -> K) -> Option<&T>",
                "of(key)",
            ),
        ],
    ),
    (
        Layer::Core,
        "<T: Numeric> List<T>",
        &[("sum(self: &List<T>) -> T", "none")],
    ),
    (
        Layer::Core,
        "<T: Comparable> List<T>",
        &[
            ("min(self: &List<T>) -> &T", "none"),
            ("max(self: &List<T>) -> &T", "none"),
        ],
    ),
    (
        Layer::Core,
        "Int",
        &[
            ("min_value() -> Int", "none"),
            ("rotl(self: Int, n: Int, width: Int) -> Int", "none"),
            ("rotr(self: Int, n: Int, width: Int) -> Int", "none"),
            ("rotr32(self: Int, n: Int) -> Int", "none"),
            ("ashr(self: Int, n: Int) -> Int", "none"),
        ],
    ),
    (
        Layer::Core,
        "<T> Option<T>",
        &[
            ("is_some(self: &Option<T>) -> Bool", "none"),
            ("is_none(self: &Option<T>) -> Bool", "none"),
            ("unwrap_or(self: Option<T>, default: T) -> T", "none"),
            (
                "map<U>(self: Option<T>, f: fn(T) -> U) -> Option<U>",
                "of(f)",
            ),
            (
                "ok_or<E>(self: Option<T>, error: E) -> Result<T, E>",
                "none",
            ),
        ],
    ),
    (
        Layer::Core,
        "<T, E> Result<T, E>",
        &[
            ("is_ok(self: &Result<T, E>) -> Bool", "none"),
            ("is_err(self: &Result<T, E>) -> Bool", "none"),
            ("unwrap_or(self: Result<T, E>, default: T) -> T", "none"),
            ("ok(self: Result<T, E>) -> Option<T>", "none"),
            (
                "map<U>(self: Result<T, E>, f: fn(T) -> U) -> Result<U, E>",
                "of(f)",
            ),
            (
                "map_err<F>(self: Result<T, E>, f: fn(E) -> F) -> Result<T, F>",
                "of(f)",
            ),
        ],
    ),
    (
        Layer::Core,
        "String",
        &[
            ("len(self: &String) -> Int", "none"),
            ("is_empty(self: &String) -> Bool", "none"),
            ("trim(self: &String) -> String", "none"),
            ("lines(self: &String) -> List<String>", "none"),
            (
                "parse_int(self: &String) -> Result<Int, ParseError>",
                "none",
            ),
            ("as_bytes(self: &String) -> Bytes", "none"),
        ],
    ),
    (
        Layer::Core,
        "Bytes",
        &[
            ("new() -> Bytes", "none"),
            ("with_capacity(capacity: Int) -> Bytes", "none"),
            (
                "from_list(bytes: &List<Int>) -> Result<Bytes, String>",
                "none",
            ),
            ("len(self: &Bytes) -> Int", "none"),
            ("is_empty(self: &Bytes) -> Bool", "none"),
            ("get(self: &Bytes, index: Int) -> Option<Int>", "none"),
            (
                "set(self: &mut Bytes, index: Int, byte: Int) -> Result<Unit, String>",
                "none",
            ),
            ("push(self: &mut Bytes, byte: Int)", "none"),
            (
                "slice(self: &Bytes, start: Int, end: Int) -> Result<Bytes, String>",
                "none",
            ),
            ("concat(self: &Bytes, other: &Bytes) -> Bytes", "none"),
            ("extend(self: &mut Bytes, other: &Bytes)", "none"),
            (
                "to_string_utf8(self: &Bytes) -> Result<String, String>",
                "none",
            ),
            ("push_u16_be(self: &mut Bytes, value: Int)", "none"),
            ("push_u32_be(self: &mut Bytes, value: Int)", "none"),
            ("push_u64_be(self: &mut Bytes, value: Int)", "none"),
            ("push_u32_le(self: &mut Bytes, value: Int)", "none"),
            (
                "read_u32_be(self: &Bytes, offset: Int) -> Option<Int>",
                "none",
            ),
        ],
    ),
    (
        Layer::Os,
        "Path",
        &[
            ("new(s: &String) -> Path", "none"),
            ("is_empty(self: &Path) -> Bool", "none"),
            ("join(self: &Path, name: &String) -> Path", "none"),
            ("file_name(self: &Path) -> Option<String>", "none"),
            ("parent(self: &Path) -> Option<Path>", "none"),
        ],
    ),
    (
        Layer::Core,
        "<T> Cell<T>",
        &[("new(value: T) -> Cell<T>", "none")],
    ),
    (
        Layer::Core,
        "<T> Shared<T>",
        &[("new(value: T) -> Shared<T>", "none")],
    ),
    (
        Layer::Core,
        "<T: Copyable> Shared<Cell<T>>",
        &[("get(self: &Shared<Cell<T>>) -> T", "state")],
    ),
    (
        Layer::Core,
        "<T> Shared<Cell<T>>",
        &[("set(self: &Shared<Cell<T>>, value: T)", "state")],
    ),
];

fn clauses(sig: &str, effects: &str) -> String {
    let requires = REQUIRES
        .iter()
        .find(|(s, _)| *s == sig)
        .map_or("none", |(_, r)| r);
    clauses_with(requires, effects)
}

fn clauses_with(requires: &str, effects: &str) -> String {
    format!(
        "\n  intent \"s\"\n  requires {requires}\n  ensures none\n  effects {effects}\n{{\n}}\n"
    )
}

fn module_head(layer: Layer) -> &'static str {
    match layer {
        Layer::Core => "module std.core\n  targets bare, browser, server\n  trust base\n\n",
        Layer::Os => "module std.os\n  targets browser, server\n  trust base\n\n",
    }
}

/// 原始操作の 1 行を、型検査器が読む `fn` のソースにする（効果の `DELEGATED` は `none` として読む）。
pub fn raw_source(layer: Layer, sig: &str, effects: &str, requires: &str) -> String {
    let effects = if effects == DELEGATED {
        "none"
    } else {
        effects
    };
    format!(
        "{}fn {sig}{}",
        module_head(layer),
        clauses_with(requires, effects)
    )
}

/// 型検査器が読む `Sigil` のソース。自由関数と名前空間の関数は `fn` の並び、メソッドは `impl` の並び。
pub fn free_source(layer: Layer, sig: &str, effects: &str) -> String {
    format!("{}fn {sig}{}", module_head(layer), clauses(sig, effects))
}

pub fn impl_source(layer: Layer, head: &str, fns: &[Entry]) -> String {
    let mut s = String::from(module_head(layer));
    s.push_str(&format!("impl{} {{\n", head_with_space(head)));
    for (sig, effects) in fns {
        s.push_str(&format!("fn {sig}{}", clauses(sig, effects)));
    }
    s.push_str("}\n");
    s
}

fn head_with_space(head: &str) -> String {
    // `<T> List<T>` は `impl<T> List<T>`、`String` は `impl String`。
    if head.starts_with('<') {
        head.to_string()
    } else {
        format!(" {head}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EffectTable;
    use std::collections::BTreeSet;

    /// 型名・`ns.name`・`Type.name` の形で名前を集める。
    fn names(layer: Layer) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let fn_name = |sig: &str| sig.split(['(', '<']).next().unwrap().to_string();
        for (n, _, l) in OPAQUE_TYPES {
            if *l == layer {
                out.insert((*n).to_string());
            }
        }
        for (l, sig, _) in FREE_FNS {
            if *l == layer {
                out.insert(fn_name(sig));
            }
        }
        for (l, ns, sig, _) in NAMESPACED_FNS {
            if *l == layer {
                out.insert(format!("{ns}.{}", fn_name(sig)));
            }
        }
        for (l, head, fns) in IMPLS {
            if *l == layer {
                let ty = head.rsplit('>').next().unwrap().trim();
                let ty = ty.split('<').next().unwrap();
                for (sig, _) in *fns {
                    out.insert(format!("{ty}.{}", fn_name(sig)));
                }
            }
        }
        out
    }

    #[test]
    fn every_raw_op_has_premise() {
        // 本 ADR（0016）の時点では `RAW_OPS` は空。テスト用の差し込み（`Options::raw_ops`）の行は、
        // 検査器が登録するときに同じ規則で調べる（tests/trust_base.rs の `raw_op_without_premise_rejected`）。
        for r in RAW_OPS {
            assert!(!r.premise.trim().is_empty(), "{}", r.signature);
        }
    }

    #[test]
    fn std_modules_always_allowed() {
        // 標準ライブラリの仮のモジュール（`std.core`・`std.os`。どちらも `trust base`）は処理系が内部で読むので、
        // 許可（`trust_base`）が空でも、利用者のファイルの検査の前にそれを登録して通る。
        let opts = crate::effects::Options::default();
        assert!(opts.trust_base.is_empty());
        let src = "module m.a\n  targets server\n  trust none\n\nfn f() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n";
        crate::check::check_with(src, &opts).unwrap();
        for layer in [Layer::Core, Layer::Os] {
            let f = crate::parser::parse_std(&free_source(layer, "f() -> Int", "none")).unwrap();
            assert_eq!(f.module.trust, crate::ast::Trust::Base);
        }
    }

    #[test]
    fn os_layer_is_exactly_the_adr_list() {
        // ADR 0014 決定 2 の OS 層。行を層の間で移すには ADR を改める。
        let want: BTreeSet<String> = [
            "console.log",
            "fs.read",
            "fs.write",
            "Path",
            "Path.new",
            "Path.is_empty",
            "Path.join",
            "Path.file_name",
            "Path.parent",
            "IoError",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(names(Layer::Os), want);
    }

    fn mentions_os_type(sig: &str) -> bool {
        sig.split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|w| w == "Path" || w == "IoError")
    }

    #[test]
    fn core_rows_do_not_mention_os_types() {
        for (l, sig, _) in FREE_FNS {
            assert!(*l != Layer::Core || !mentions_os_type(sig), "{sig}");
        }
        for (l, _, sig, _) in NAMESPACED_FNS {
            assert!(*l != Layer::Core || !mentions_os_type(sig), "{sig}");
        }
        for (l, head, fns) in IMPLS {
            if *l == Layer::Core {
                assert!(!mentions_os_type(head), "{head}");
                for (sig, _) in *fns {
                    assert!(!mentions_os_type(sig), "{sig}");
                }
            }
        }
    }

    #[test]
    fn core_rows_effects_allowed_on_bare() {
        let table = EffectTable::standard();
        let mut effects: Vec<&str> = vec![];
        effects.extend(FREE_FNS.iter().filter(|r| r.0 == Layer::Core).map(|r| r.2));
        effects.extend(
            NAMESPACED_FNS
                .iter()
                .filter(|r| r.0 == Layer::Core)
                .map(|r| r.3),
        );
        for (l, _, fns) in IMPLS {
            if *l == Layer::Core {
                effects.extend(fns.iter().map(|(_, e)| *e));
            }
        }
        for e in effects {
            if e == "none" || e.starts_with("of(") {
                continue;
            }
            for atom in e.split(',').map(str::trim) {
                assert!(table.allowed_on(atom, "bare"), "{atom}");
            }
        }
    }
}

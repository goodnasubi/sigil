//! 標準ライブラリの最小部分のシグネチャと効果の表（M1）。
//!
//! 仮の範囲。標準ライブラリの分割（どの関数・型をどのモジュールに置くか）は後の ADR で決める。
//! ここにあるのは、5 つのサンプルと ADR 0011 の M1 完了条件のテストに必要なものだけで、
//! 本体を持たない。型検査器はこの表を `Sigil` のシグネチャとして読み、利用者の宣言と同じ表に入れる。
//!
//! 表の書き方:
//! - 自由関数（`FREE_FNS`）: 名前空間のない関数。利用者が同じ名前の関数を宣言すると、そちらが優先される。
//! - 名前空間の関数（`NAMESPACED_FNS`）: `fs.read(p)`・`console.log(x)` のように呼ぶ。
//! - メソッドと型に付く関数（`IMPLS`）: `impl` の見出し（型引数と型）と、その中の関数。
//!
//! 仮の決めごと（後の ADR で見直す）:
//! - `List` の取り出しは参照を返す（`get`: `Option<&T>`）。
//! - `console.log` は型引数に制約を付けず、どの型の値も受け取る（表示の性質は決めていない）。
//! - 読むだけの関数は、`&T` ではなく値の型で受け取る（式の中で参照の変数は参照先の値を表すため。ADR 0011 決定 4）。

/// (シグネチャ, 効果)。効果は `effects` 節の中身（`none`・`console`・`of(f)` など）。
pub type Entry = (&'static str, &'static str);

/// 標準の型で、中身を見せないもの（名前, 型引数の数）。
pub const OPAQUE_TYPES: &[(&str, usize)] = &[
    ("List", 1),
    ("Set", 1),
    ("Map", 2),
    ("Path", 0),
    ("IoError", 0),
    ("ParseError", 0),
    ("Shared", 1),
    ("Cell", 1),
];

/// 名前空間のない関数。
pub const FREE_FNS: &[Entry] = &[
    ("len<T>(xs: List<T>) -> Int", "none"),
    ("min<T: Comparable>(xs: List<T>) -> T", "none"),
    ("max<T: Comparable>(xs: List<T>) -> T", "none"),
    ("sum<T: Numeric>(xs: List<T>) -> T", "none"),
    ("to_float(n: Int) -> Float", "none"),
    ("empty(p: Path) -> Bool", "none"),
    ("trim(s: String) -> String", "none"),
    ("parse_int(s: String) -> Result<Int, ParseError>", "none"),
];

/// 名前空間の関数（名前空間, シグネチャ, 効果）。
pub const NAMESPACED_FNS: &[(&str, &str, &str)] = &[
    ("console", "log<T>(x: T)", "console"),
    ("fs", "read(p: Path) -> Result<String, IoError>", "fs.read"),
    (
        "fs",
        "write(p: Path, s: String) -> Result<Unit, IoError>",
        "fs.write",
    ),
];

/// `impl` の見出し（例: `<T> List<T>`）と、その中の関数。
pub const IMPLS: &[(&str, &[Entry])] = &[
    (
        "<T> List<T>",
        &[
            ("new() -> List<T>", "none"),
            ("push(self: &mut List<T>, x: T)", "none"),
            ("get(self: &List<T>, i: Int) -> Option<&T>", "none"),
            ("len(self: &List<T>) -> Int", "none"),
            ("is_empty(self: &List<T>) -> Bool", "none"),
            ("map<U>(self: &List<T>, f: fn(T) -> U) -> List<U>", "of(f)"),
            (
                "max_by<K: Comparable>(self: &List<T>, key: fn(&T) -> K) -> Option<&T>",
                "of(key)",
            ),
        ],
    ),
    (
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
        "String",
        &[
            ("len(self: &String) -> Int", "none"),
            ("is_empty(self: &String) -> Bool", "none"),
            ("trim(self: &String) -> String", "none"),
            ("lines(self: &String) -> List<String>", "none"),
        ],
    ),
    ("<T> Cell<T>", &[("new(value: T) -> Cell<T>", "none")]),
    ("<T> Shared<T>", &[("new(value: T) -> Shared<T>", "none")]),
    (
        "<T> Shared<Cell<T>>",
        &[
            ("get(self: &Shared<Cell<T>>) -> T", "state"),
            ("set(self: &Shared<Cell<T>>, value: T)", "state"),
        ],
    ),
];

fn clauses(effects: &str) -> String {
    format!("\n  intent \"s\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n}}\n")
}

/// 型検査器が読む `Sigil` のソース。自由関数と名前空間の関数は `fn` の並び、メソッドは `impl` の並び。
pub fn free_source(sig: &str, effects: &str) -> String {
    format!(
        "module std.std\n  targets browser, server\n  trust base\n\nfn {sig}{}",
        clauses(effects)
    )
}

pub fn impl_source(head: &str, fns: &[Entry]) -> String {
    let mut s = String::from("module std.std\n  targets browser, server\n  trust base\n\n");
    s.push_str(&format!("impl{} {{\n", head_with_space(head)));
    for (sig, effects) in fns {
        s.push_str(&format!("fn {sig}{}", clauses(effects)));
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

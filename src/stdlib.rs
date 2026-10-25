//! 標準ライブラリの最小部分のシグネチャと効果の表（M1）。
//!
//! 仮の範囲。標準ライブラリの分割（どの関数・型をどのモジュールに置くか）は後の ADR で決める。
//! ここにあるのは、5 つのサンプルと ADR 0011 の M1 完了条件のテストに必要なものだけで、
//! 本体を持たない。型検査器はこの表を `Sigil` のシグネチャとして読み、利用者の宣言と同じ表に入れる。
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

/// 名前空間のない関数。読むだけの操作はメソッドに移したので、`Copyable` な値の関数だけが残る。
pub const FREE_FNS: &[Entry] = &[("to_float(n: Int) -> Float", "none")];

/// 名前空間の関数（名前空間, シグネチャ, 効果）。読むだけの引数は `&T` で受ける。
pub const NAMESPACED_FNS: &[(&str, &str, &str)] = &[
    ("console", "log<T: Displayable>(x: &T)", "console"),
    ("fs", "read(p: &Path) -> Result<String, IoError>", "fs.read"),
    (
        "fs",
        "write(p: &Path, s: &String) -> Result<Unit, IoError>",
        "fs.write",
    ),
];

/// シグネチャ（`Entry` の最初の要素）に対応する `requires` 節。ない関数は `none`。
pub const REQUIRES: &[(&str, &str)] = &[
    ("min(self: &List<T>) -> T", "self.len() > 0"),
    ("max(self: &List<T>) -> T", "self.len() > 0"),
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
        "<T: Numeric> List<T>",
        &[("sum(self: &List<T>) -> T", "none")],
    ),
    (
        "<T: Comparable + Copyable> List<T>",
        &[
            ("min(self: &List<T>) -> T", "none"),
            ("max(self: &List<T>) -> T", "none"),
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
            (
                "parse_int(self: &String) -> Result<Int, ParseError>",
                "none",
            ),
        ],
    ),
    ("Path", &[("is_empty(self: &Path) -> Bool", "none")]),
    ("<T> Cell<T>", &[("new(value: T) -> Cell<T>", "none")]),
    ("<T> Shared<T>", &[("new(value: T) -> Shared<T>", "none")]),
    (
        "<T: Copyable> Shared<Cell<T>>",
        &[("get(self: &Shared<Cell<T>>) -> T", "state")],
    ),
    (
        "<T> Shared<Cell<T>>",
        &[("set(self: &Shared<Cell<T>>, value: T)", "state")],
    ),
];

fn clauses(sig: &str, effects: &str) -> String {
    let requires = REQUIRES
        .iter()
        .find(|(s, _)| *s == sig)
        .map_or("none", |(_, r)| r);
    format!(
        "\n  intent \"s\"\n  requires {requires}\n  ensures none\n  effects {effects}\n{{\n}}\n"
    )
}

/// 型検査器が読む `Sigil` のソース。自由関数と名前空間の関数は `fn` の並び、メソッドは `impl` の並び。
pub fn free_source(sig: &str, effects: &str) -> String {
    format!(
        "module std.std\n  targets browser, server\n  trust base\n\nfn {sig}{}",
        clauses(sig, effects)
    )
}

pub fn impl_source(head: &str, fns: &[Entry]) -> String {
    let mut s = String::from("module std.std\n  targets browser, server\n  trust base\n\n");
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

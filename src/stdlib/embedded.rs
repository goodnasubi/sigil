//! 処理系に埋め込んだ Sigil 製の標準モジュール（ADR 0025 補足 A-6、PR 3-4h）。
//!
//! 埋め込みモジュールは、利用者のプログラムと同じ検査・lowering にかけて 1 つのプログラムに結ぶ。
//! `std.*` の `trust base` は処理系のソースに入っていることを根拠に許可する（`trust-base.list` を経ない）。
//!
//! - 経路は暗黙のみ。利用者の `module std.*` / `use std.*` は従来どおり拒否する（ADR 0016 決定 3、ADR 0021）。
//!   結ぶ側は `effects::Options::embedded_roots` で名指しする（3-4i が `raw.alloc` の差し替えで使う）。
//! - 名前は `std.` で始まるものに限る。結合後の名前も `std.` 始まりになり、検査器はそれを出所の印にする
//!   （利用者のモジュールは `std` で始まれず、`.` を含む名前は結合でしか生まれない）。
//! - 本番のレジストリ `EMBEDDED` には確保器 `std.os.alloc` がある（3-4i）。`raw.alloc`/`raw.free` を使い、
//!   すべてのプログラムに暗黙に結ぶ（補足 C-2。`implicit_roots`）。
//! - `std.core.string`（3-5b-i）は `String` の表現（24B の頭と中身の 2 つの塊）を持ち、同じく暗黙に結ぶ（補足 D）。
//!   `raw_ops` で確保器を差し替えたときは、確保器も文字列も結ばない。

/// Sigil 製の確保器のモジュール名（ADR 0025 決定 3、補足 A-2・B-1）。
pub const ALLOC_MODULE: &str = "std.os.alloc";
/// 確保器の入口の関数（結合後の名前）。lowering は入口から辿れなくても SIR に入れ、ネイティブは
/// `sigil_rt_raw_alloc` / `sigil_rt_raw_free` / `sigil_rt_sigil_live_blocks` のシムから呼ぶ（補足 B-2・B-5）。
pub const ALLOC_FN: &str = "std.os.alloc.alloc";
pub const FREE_FN: &str = "std.os.alloc.free";
pub const LIVE_FN: &str = "std.os.alloc.live_blocks";

/// `Float` の余り（fmod）の Sigil 版のモジュール名と関数名（結合後の名前。ADR 0025 補足 C-3）。
/// 言語に `Float` の `%` はない（ADR 0011 の決定。検査器が拒否する）ので、プログラムには暗黙に結ばない。
/// 後続の PR で Sigil 製の標準ライブラリ（`Float` の表示・解析）が `use` して使う。
pub const FLOAT_MODULE: &str = "std.core.float";
pub const FMOD_FN: &str = "std.core.float.fmod";

/// `String` の表現の Sigil 実装のモジュール名と入口の関数（結合後の名前。ADR 0025 補足 D-3、PR 3-5b-i）。
/// lowering は入口から辿れなくても SIR に入れ、ネイティブは `sigil_rt_str_*` のシムから呼ぶ。
pub const STRING_MODULE: &str = "std.core.string";
pub const STR_NEW_LOSSY_FN: &str = "std.core.string.new_lossy";
pub const STR_FREE_FN: &str = "std.core.string.free";
pub const STR_CLONE_FN: &str = "std.core.string.clone";
pub const STR_DATA_FN: &str = "std.core.string.data";
pub const STR_BYTES_FN: &str = "std.core.string.bytes";
/// 3-5b-ii で移した操作。`eq`・`cmp` はシムを持たず、SIR にも入れない（3-5e が生成する `val_eq`・`val_cmp` が呼ぶ）。
pub const STR_LEN_FN: &str = "std.core.string.len";
pub const STR_IS_EMPTY_FN: &str = "std.core.string.is_empty";
pub const STR_TRIM_FN: &str = "std.core.string.trim";
pub const STR_CONCAT_FN: &str = "std.core.string.concat";
pub const STR_PARSE_INT_FN: &str = "std.core.string.parse_int";
pub const STR_EQ_FN: &str = "std.core.string.eq";
pub const STR_CMP_FN: &str = "std.core.string.cmp";

/// 埋め込みモジュールの一覧（モジュール名, Sigil のソース）。
pub const EMBEDDED: &[(&str, &str)] = &[
    (ALLOC_MODULE, include_str!("os_alloc.sigil")),
    (STRING_MODULE, include_str!("core_string.sigil")),
    (FLOAT_MODULE, include_str!("core_float.sigil")),
];

/// 暗黙に結ぶ埋め込みモジュール（補足 C-2）。
///
/// ネイティブ出力は常に Linux（ADR 0015）で、確保器 `std.os.alloc` は入口の `targets` や `raw.alloc`/`raw.free` の
/// 使用の有無によらず、すべてのプログラムに結ぶ（補足 B-3・B-4 を置き換える。Rust の確保器と混在させない）。
/// 確保器は出力先のプラットフォーム層が与えるもので、入口の `targets` との照合の対象ではない（補足 C-2。
/// `project::embedded` が確保器だけ `TargetMismatch` を飛ばす）。評価器も同じ条件で結ぶ（呼ぶのは組み込みのまま。A-2）。
/// 試験用の差し込み（`Options::raw_ops`）で `raw.alloc`/`raw.free` を置き換えたときは、本物の原始操作では
/// ないので結ばない。
pub fn implicit_roots(opts: &crate::effects::Options) -> Vec<&'static str> {
    let replaced = opts.raw_ops.iter().any(|r| {
        let name = r.signature.split('(').next().unwrap_or("").trim();
        name == "alloc" || name == "free"
    });
    if replaced {
        vec![]
    } else {
        vec![ALLOC_MODULE, STRING_MODULE]
    }
}

/// 埋め込みモジュールとして扱える名前か（先頭が `std` で、あとに 1 区間以上続く）。
pub fn is_embedded_name(name: &str) -> bool {
    name.strip_prefix("std.")
        .is_some_and(|rest| !rest.is_empty())
}

/// 名前から埋め込みモジュールのソースを引く。`extra` は試験用の追加（`Options::embedded_extra`）。
/// `std.` で始まらない名前は、一覧にあっても引けない（出所の印を守るため）。
pub fn lookup<'a>(name: &str, extra: &'a [(String, String)]) -> Option<&'a str> {
    if !is_embedded_name(name) {
        return None;
    }
    if let Some((_, s)) = extra.iter().find(|(n, _)| n == name) {
        return Some(s.as_str());
    }
    EMBEDDED.iter().find(|(n, _)| *n == name).map(|(_, s)| *s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_must_start_with_std_dot() {
        assert!(is_embedded_name("std.os.alloc"));
        assert!(!is_embedded_name("std"));
        assert!(!is_embedded_name("std."));
        assert!(!is_embedded_name("stdx.a"));
        assert!(!is_embedded_name("m.std.a"));
    }

    #[test]
    fn lookup_rejects_non_std_names_even_if_registered() {
        let extra = vec![("m.a".to_string(), "x".to_string())];
        assert_eq!(lookup("m.a", &extra), None);
        let extra = vec![("std.a".to_string(), "x".to_string())];
        assert_eq!(lookup("std.a", &extra), Some("x"));
        assert_eq!(lookup("std.b", &extra), None);
    }

    #[test]
    fn production_registry_names_are_embedded_names() {
        for (n, _) in EMBEDDED {
            assert!(is_embedded_name(n));
        }
    }
}

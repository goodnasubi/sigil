//! 処理系に埋め込んだ Sigil 製の標準モジュール（ADR 0025 補足 A-6、PR 3-4h）。
//!
//! 埋め込みモジュールは、利用者のプログラムと同じ検査・lowering にかけて 1 つのプログラムに結ぶ。
//! `std.*` の `trust base` は処理系のソースに入っていることを根拠に許可する（`trust-base.list` を経ない）。
//!
//! - 経路は暗黙のみ。利用者の `module std.*` / `use std.*` は従来どおり拒否する（ADR 0016 決定 3、ADR 0021）。
//!   結ぶ側は `effects::Options::embedded_roots` で名指しする（3-4i が `raw.alloc` の差し替えで使う）。
//! - 名前は `std.` で始まるものに限る。結合後の名前も `std.` 始まりになり、検査器はそれを出所の印にする
//!   （利用者のモジュールは `std` で始まれず、`.` を含む名前は結合でしか生まれない）。
//! - 本番のレジストリ `EMBEDDED` は 3-4h では空。確保器 `std.os.alloc` は 3-4i で足す。

/// 埋め込みモジュールの一覧（モジュール名, Sigil のソース）。
pub const EMBEDDED: &[(&str, &str)] = &[];

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

//! 信頼基盤の許可一覧ファイル（`trust-base.list`）の読み込み（ADR 0018 判断 1、PR 3-0）。
//!
//! リポジトリまたはプロジェクトの根（`--root`、指定がなければカレントディレクトリ）に
//! 置かれた `trust-base.list` から `trust base` を宣言できるモジュール名の一覧を読み込む。
//! `#` で始まるコメント行や空行は無視し、各行の空白をトリムして 1 行 1 モジュール名として扱う。

use std::path::Path;

/// 信頼基盤の許可一覧のファイル名（ADR 0018 判断 1）。
pub const TRUST_BASE_FILE: &str = "trust-base.list";

/// 文字列からモジュール名の一覧を抽出する（`#` で始まるコメント行や空行を無視）。
pub fn parse_trust_base_list(content: &str) -> Vec<String> {
    content
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|s| s.to_string())
        .collect()
}

/// 指定した探索基準ディレクトリ（根）にある許可一覧ファイル（`trust-base.list`）を読み込む。
/// ファイルが存在しない場合は空のベクタを返す。
pub fn load_trust_base_file(root: &Path) -> Result<Vec<String>, String> {
    let path = root.join(TRUST_BASE_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(parse_trust_base_list(&content)),
        Err(e) => Err(format!("{}: 読めない: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_list_ignores_comments_and_empty_lines() {
        let text = r#"
# 信頼基盤のモジュール一覧
m.a
  m.b.c  

# 別のコメント
m.d
"#;
        assert_eq!(
            parse_trust_base_list(text),
            vec!["m.a".to_string(), "m.b.c".to_string(), "m.d".to_string()]
        );
    }
}

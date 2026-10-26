//! 異常停止の報告の文字列（ADR 0015 の決定 5）。
//!
//! SIR の評価器（`exec::Stop` の表示）と、ネイティブ出力の lowering が作る停止の地点の文字列が
//! ずれないよう、文字列を作る処理をここに 1 つだけ置く。

use crate::exec::{Clause, StopKind};

/// 異常停止の 1 行の報告（`異常停止: 種類（関数 名前、N 行目）`。契約の式の評価中なら `。{関数} の {節} の式の評価中に起きた` が続く）。
///
/// - `detail`: 標準の関数の `requires` 違反（`StopKind::StdRequiresViolated`）のときの説明。他の種類では使わない。
/// - `during`: 契約の式の評価中に起きた停止なら、その契約を持つ関数と節。
pub fn stop_message(
    kind: StopKind,
    func: &str,
    line: u32,
    during: Option<(&str, Clause)>,
    detail: &str,
) -> String {
    let what = match kind {
        StopKind::RequiresViolated => "契約違反: requires を満たしていない".to_string(),
        StopKind::EnsuresViolated => "契約違反: ensures を満たしていない".to_string(),
        StopKind::StdRequiresViolated => format!("標準の関数の requires 違反: {detail}"),
        StopKind::Overflow => "Int の桁あふれ".to_string(),
        StopKind::DivisionByZero => "0 での除算".to_string(),
        StopKind::StackExhausted => "スタックの枯渇".to_string(),
    };
    let mut out = format!("異常停止: {what}（関数 {func}、{line} 行目）");
    if let Some((f, clause)) = during {
        out.push_str(&format!("。{f} の {clause} の式の評価中に起きた"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_stop() {
        assert_eq!(
            stop_message(StopKind::Overflow, "f", 3, None, ""),
            "異常停止: Int の桁あふれ（関数 f、3 行目）"
        );
    }

    #[test]
    fn std_requires_has_detail() {
        assert_eq!(
            stop_message(StopKind::StdRequiresViolated, "f", 1, None, "min: 空の一覧"),
            "異常停止: 標準の関数の requires 違反: min: 空の一覧（関数 f、1 行目）"
        );
    }

    #[test]
    fn stop_during_contract() {
        assert_eq!(
            stop_message(
                StopKind::DivisionByZero,
                "g",
                2,
                Some(("f", Clause::Ensures)),
                ""
            ),
            "異常停止: 0 での除算（関数 g、2 行目）。f の ensures の式の評価中に起きた"
        );
    }
}

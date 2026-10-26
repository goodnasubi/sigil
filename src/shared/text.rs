//! インタプリタ・SIR の評価器・暫定ランタイム（`runtime/core`）が共有する、文字列と数の表示・操作（ADR 0015 の決定 6）。
//!
//! 標準ライブラリだけに依存する。`#[path = "..."] mod` で取り込まれるので、`crate::` を参照しない。

/// `Float` の表示。小数点がなければ `.0` を付ける。
pub fn fmt_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "inf" } else { "-inf" }.into()
    } else {
        let s = format!("{f}");
        if s.contains('.') {
            s
        } else {
            format!("{s}.0")
        }
    }
}

/// `String.len`: Unicode のスカラー値の数。
pub fn char_len(s: &str) -> i64 {
    s.chars().count() as i64
}

/// `String.trim`。
pub fn trim(s: &str) -> &str {
    s.trim()
}

/// `String.lines`: `\n` で分け、最後の空の行は落とし、行末の `\r` を外す。
pub fn lines(s: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = s.split('\n').collect();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    parts
        .into_iter()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect()
}

/// `String.parse_int`: 先頭の `-` と数字だけ。`+`・空白・桁区切りは受けない。範囲外は `None`。
pub fn parse_int(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    let valid = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    valid.then(|| s.parse::<i64>().ok()).flatten()
}

/// 文字列の値の表示（引用符つき。`{:?}` と同じ）。
pub fn quote(s: &str) -> String {
    format!("{s:?}")
}

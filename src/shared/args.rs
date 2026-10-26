//! 入口の関数の引数の値と、コマンドラインの書き方の読み取り（`sigil run` と実行ファイルが共有する。ADR 0015 の決定 6・7）。

/// 入口の関数に渡す引数。
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    List(Vec<Arg>),
}

/// `true`/`false`・整数・小数・`[a,b,...]`・それ以外は文字列。Int の範囲外の整数は誤り。
pub fn parse_arg(s: &str) -> Result<Arg, String> {
    if s == "true" || s == "false" {
        return Ok(Arg::Bool(s == "true"));
    }
    if let Ok(i) = s.parse::<i64>() {
        return Ok(Arg::Int(i));
    }
    let digits = s.strip_prefix('-').unwrap_or(s);
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("整数 {s} が Int の範囲外"));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(Arg::Float(f));
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let items = if inner.trim().is_empty() {
            Vec::new()
        } else {
            inner
                .split(',')
                .map(|x| parse_arg(x.trim()))
                .collect::<Result<_, _>>()?
        };
        return Ok(Arg::List(items));
    }
    Ok(Arg::Str(s.to_string()))
}

/// 型の合わない引数の説明に出す名前。
pub fn arg_name(a: &Arg) -> &'static str {
    match a {
        Arg::Int(_) => "整数",
        Arg::Float(_) => "小数",
        Arg::Bool(_) => "真偽値",
        Arg::Str(_) => "文字列",
        Arg::List(_) => "一覧",
    }
}

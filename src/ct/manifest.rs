//! `Sigil.toml` の `[testing.constant_time]` の解釈（ADR 0061 決定 2・4）。
//!
//! 依存クレートを足さない方針（ADR 0061・JSON と同じ手書き）なので、TOML の部分集合だけを読む:
//! `#` のコメント、`[表]`・`[[表の列]]` の見出し、`キー = 値`、値は文字列（`"…"`、`\\` `\"` `\n` `\t`
//! の逃がし）・整数・真偽値・配列（複数行可）・インライン表。`[testing.constant_time]` 以外の表は
//! 構文だけ確かめて読み捨てる。`[testing.constant_time]` 内の未知のキーはエラー。
//!
//! ```toml
//! [testing.constant_time]
//! targets = [
//!   { name = "rune_auth.verify_session_token", public_args = ["token_len"] },
//!   { name = "rune_auth.compare_digest", public_args = [] },
//! ]
//! ```

use std::collections::BTreeMap;

/// 定数時間の対象 1 つ。`name` は `<モジュール名>.<関数名>`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    /// 公開の引数名。ここにない引数はすべて秘密として扱う（ADR 0061 決定 2）。
    pub public_args: Vec<String>,
}

impl Target {
    /// モジュール名と関数名に分ける（最後の `.` で切る）。
    pub fn split(&self) -> (&str, &str) {
        match self.name.rfind('.') {
            Some(i) => (&self.name[..i], &self.name[i + 1..]),
            None => ("", &self.name),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Str(String),
    Int(i64),
    Bool(bool),
    Arr(Vec<Val>),
    Tab(BTreeMap<String, Val>),
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    line: usize,
}

impl<'a> P<'a> {
    fn err<T>(&self, m: impl Into<String>) -> Result<T, String> {
        Err(format!("Sigil.toml:{}: {}", self.line, m.into()))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// 空白（改行を含むかは `nl`）とコメントを読み飛ばす。
    fn skip(&mut self, nl: bool) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\r' => self.i += 1,
                b'\n' if nl => {
                    self.line += 1;
                    self.i += 1;
                }
                b'#' => {
                    while self.peek().is_some_and(|c| c != b'\n') {
                        self.i += 1;
                    }
                }
                _ => break,
            }
        }
    }

    fn key(&mut self) -> Result<String, String> {
        let st = self.i;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            self.i += 1;
        }
        if self.i == st {
            return self.err("キーがない");
        }
        Ok(String::from_utf8_lossy(&self.s[st..self.i]).into_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        self.i += 1; // 開きの "
        let mut out: Vec<u8> = vec![];
        loop {
            match self.peek() {
                None | Some(b'\n') => return self.err("文字列が閉じていない"),
                Some(b'"') => {
                    self.i += 1;
                    return String::from_utf8(out).or_else(|_| self.err("文字列が UTF-8 でない"));
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = match self.peek() {
                        Some(b'\\') => b'\\',
                        Some(b'"') => b'"',
                        Some(b'n') => b'\n',
                        Some(b't') => b'\t',
                        _ => return self.err("未対応の逃がし（\\\\ \\\" \\n \\t だけ）"),
                    };
                    out.push(c);
                    self.i += 1;
                }
                Some(c) => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
    }

    fn value(&mut self) -> Result<Val, String> {
        match self.peek() {
            Some(b'"') => Ok(Val::Str(self.string()?)),
            Some(b'[') => {
                self.i += 1;
                let mut items = vec![];
                loop {
                    self.skip(true);
                    match self.peek() {
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Val::Arr(items));
                        }
                        None => return self.err("配列が閉じていない"),
                        _ => {}
                    }
                    items.push(self.value()?);
                    self.skip(true);
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return self.err("配列の要素は `,` で区切る"),
                    }
                }
            }
            Some(b'{') => {
                self.i += 1;
                let mut t = BTreeMap::new();
                loop {
                    self.skip(false);
                    match self.peek() {
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Val::Tab(t));
                        }
                        None | Some(b'\n') => {
                            return self.err("インライン表は 1 行に収め、`}` で閉じる")
                        }
                        _ => {}
                    }
                    let k = self.key()?;
                    self.skip(false);
                    if self.peek() != Some(b'=') {
                        return self.err("`=` がない");
                    }
                    self.i += 1;
                    self.skip(false);
                    let v = self.value()?;
                    if t.insert(k.clone(), v).is_some() {
                        return self.err(format!("キー `{k}` が重複している"));
                    }
                    self.skip(false);
                    match self.peek() {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {}
                        _ => return self.err("インライン表の要素は `,` で区切る"),
                    }
                }
            }
            Some(b't') | Some(b'f') => {
                let w = self.key()?;
                match w.as_str() {
                    "true" => Ok(Val::Bool(true)),
                    "false" => Ok(Val::Bool(false)),
                    _ => self.err(format!("未対応の値 `{w}`")),
                }
            }
            Some(c) if c.is_ascii_digit() || c == b'-' || c == b'+' => {
                let st = self.i;
                self.i += 1;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'_') {
                    self.i += 1;
                }
                let t: String = String::from_utf8_lossy(&self.s[st..self.i])
                    .chars()
                    .filter(|c| *c != '_')
                    .collect();
                match t.parse::<i64>() {
                    Ok(n) => Ok(Val::Int(n)),
                    Err(_) => self.err(format!("整数として読めない `{t}`")),
                }
            }
            _ => self.err("未対応の値（文字列・整数・真偽値・配列・インライン表だけ）"),
        }
    }
}

/// `Sigil.toml` の本文から `[testing.constant_time]` を取り出す。表がなければ空の `Manifest`。
pub fn parse(text: &str) -> Result<Manifest, String> {
    let mut p = P {
        s: text.as_bytes(),
        i: 0,
        line: 1,
    };
    // 現在の表の見出し（`[a.b]` なら "a.b"）。先頭は無名の根。
    let mut section = String::new();
    let mut seen_section = false;
    let mut ct: Option<BTreeMap<String, Val>> = None;
    loop {
        p.skip(true);
        let Some(c) = p.peek() else { break };
        if c == b'[' {
            p.i += 1;
            let array = p.peek() == Some(b'[');
            if array {
                p.i += 1;
            }
            let mut parts = vec![];
            loop {
                p.skip(false);
                parts.push(p.key()?);
                p.skip(false);
                match p.peek() {
                    Some(b'.') => p.i += 1,
                    Some(b']') => break,
                    _ => return p.err("表の見出しが閉じていない"),
                }
            }
            p.i += 1;
            if array {
                if p.peek() != Some(b']') {
                    return p.err("`[[…]]` が閉じていない");
                }
                p.i += 1;
            }
            section = parts.join(".");
            if section == "testing.constant_time" {
                if array || seen_section {
                    return p.err("`[testing.constant_time]` は 1 回だけ書く表である");
                }
                seen_section = true;
                ct = Some(BTreeMap::new());
            }
        } else {
            let k = p.key()?;
            p.skip(false);
            if p.peek() != Some(b'=') {
                return p.err("`=` がない");
            }
            p.i += 1;
            p.skip(false);
            let v = p.value()?;
            if section == "testing.constant_time" {
                let t = ct.as_mut().expect("見出しで作る");
                if t.insert(k.clone(), v).is_some() {
                    return p.err(format!("キー `{k}` が重複している"));
                }
            }
        }
        p.skip(false);
        match p.peek() {
            None | Some(b'\n') => {}
            _ => return p.err("1 行に書けるのは 1 つの項目だけ"),
        }
    }
    let Some(ct) = ct else {
        return Ok(Manifest::default());
    };
    let mut targets = vec![];
    for (k, v) in ct {
        match (k.as_str(), v) {
            ("targets", Val::Arr(items)) => {
                for (n, it) in items.into_iter().enumerate() {
                    targets.push(target(n + 1, it)?);
                }
            }
            ("targets", _) => {
                return Err(
                    "Sigil.toml: `targets` は表の配列（`{ name = …, public_args = […] }` の並び）"
                        .to_string(),
                )
            }
            (k, _) => {
                return Err(format!(
                    "Sigil.toml: [testing.constant_time] に未知のキー `{k}`（`targets` だけ）"
                ))
            }
        }
    }
    let mut names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    if let Some(w) = names.windows(2).find(|w| w[0] == w[1]) {
        return Err(format!("Sigil.toml: 対象 `{}` が重複している", w[0]));
    }
    Ok(Manifest { targets })
}

fn ident(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn target(n: usize, v: Val) -> Result<Target, String> {
    let at = |m: String| format!("Sigil.toml: targets の {n} 番目: {m}");
    let Val::Tab(mut t) = v else {
        return Err(at("`{ name = \"…\", public_args = […] }` の形にする".into()));
    };
    let name = match t.remove("name") {
        Some(Val::Str(s)) => s,
        Some(_) => return Err(at("`name` は文字列".into())),
        None => return Err(at("`name` がない".into())),
    };
    let segs: Vec<&str> = name.split('.').collect();
    if segs.len() < 2 || !segs.iter().all(|s| ident(s)) {
        return Err(at(format!(
            "`name` は `<モジュール名>.<関数名>`（例: rune_auth.compare_digest）: `{name}`"
        )));
    }
    let public_args = match t.remove("public_args") {
        None => vec![],
        Some(Val::Arr(xs)) => {
            let mut out = vec![];
            for x in xs {
                match x {
                    Val::Str(s) if ident(&s) => {
                        if out.contains(&s) {
                            return Err(at(format!("`public_args` に `{s}` が重複している")));
                        }
                        out.push(s)
                    }
                    _ => return Err(at("`public_args` は引数名（識別子）の文字列の並び".into())),
                }
            }
            out
        }
        Some(_) => return Err(at("`public_args` は文字列の配列".into())),
    };
    if let Some(k) = t.keys().next() {
        return Err(at(format!(
            "未知のキー `{k}`（`name` と `public_args` だけ）"
        )));
    }
    Ok(Target { name, public_args })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_adr_example() {
        let m = parse(
            r#"
# 外部モジュールの定数時間の対象
[package]
name = "rune-auth"
tags = ["a", "b"]

[testing.constant_time]
targets = [
  { name = "rune_auth.verify_session_token", public_args = ["token_len"] }, # 長さは公開
  { name = "rune_auth.compare_digest", public_args = [] },
]

[other]
x = 1
"#,
        )
        .unwrap();
        assert_eq!(m.targets.len(), 2);
        assert_eq!(m.targets[0].split(), ("rune_auth", "verify_session_token"));
        assert_eq!(m.targets[0].public_args, vec!["token_len".to_string()]);
        assert!(m.targets[1].public_args.is_empty());
    }

    #[test]
    fn no_section_is_empty() {
        assert_eq!(
            parse("[package]\nname = \"x\"\n").unwrap(),
            Manifest::default()
        );
    }

    #[test]
    fn public_args_is_optional_and_means_all_secret() {
        let m = parse("[testing.constant_time]\ntargets = [{ name = \"a.b\" }]\n").unwrap();
        assert!(m.targets[0].public_args.is_empty());
    }

    #[test]
    fn rejects_bad_manifests() {
        for (src, want) in [
            ("[testing.constant_time]\nbogus = 1\n", "未知のキー `bogus`"),
            (
                "[testing.constant_time]\ntargets = [{ name = \"nomodule\" }]\n",
                "<モジュール名>.<関数名>",
            ),
            (
                "[testing.constant_time]\ntargets = [{ name = \"a.b\" }, { name = \"a.b\" }]\n",
                "重複",
            ),
            (
                "[testing.constant_time]\ntargets = [{ name = \"a.b\", extra = 1 }]\n",
                "未知のキー `extra`",
            ),
            (
                "[testing.constant_time]\ntargets = [{ name = \"a.b\", public_args = [1] }]\n",
                "識別子",
            ),
            ("[testing.constant_time]\ntargets = [\n", "閉じていない"),
            ("[testing.constant_time]\ntargets = 3\n", "表の配列"),
            (
                "[testing.constant_time]\n[testing.constant_time]\n",
                "1 回だけ",
            ),
            ("x = \"abc\n", "閉じていない"),
        ] {
            let e = parse(src).unwrap_err();
            assert!(e.contains(want), "{src:?} → {e}");
        }
    }

    #[test]
    fn errors_carry_a_line_number() {
        let e = parse("[a]\n\nx = @\n").unwrap_err();
        assert!(e.starts_with("Sigil.toml:3:"), "{e}");
    }
}

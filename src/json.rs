//! JSON の書き出しと読み取り（RFC 8259。ADR 0019 の決定 3）。依存を増やさず、処理系の中に持つ。
//!
//! - 読み取りは JSON のすべての形（数・文字列のエスケープ・入れ子）を受ける。性能は求めない。
//!   入れ子の深さは `MAX_DEPTH` までで、超えたらエラーにする（スタックを使い切らない）。
//! - 書き出しは空白のない 1 行で、オブジェクトの欄は入れた順に並べる。
//! - 整数は `Int`、小数点か指数を持つ数（と `i64` に収まらない整数）は `Float` に読む。

use std::fmt;

/// 入れ子の深さの上限。
pub const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Arr(Vec<Value>),
    /// 欄の順を保つ。同じ名前が複数あっても読める（`get` は最後のものを返す）。
    Obj(Vec<(String, Value)>),
}

/// 読み取りのエラー。`pos` は入力の先頭からのバイト位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON の誤り（{} バイト目）: {}", self.pos, self.msg)
    }
}

impl std::error::Error for JsonError {}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Self {
        Value::Int(n)
    }
}

impl From<u32> for Value {
    fn from(n: u32) -> Self {
        Value::Int(i64::from(n))
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Self {
        o.map_or(Value::Null, Into::into)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Arr(v.into_iter().map(Into::into).collect())
    }
}

/// `obj(&[("名前", 値), ...])` の略記。
pub fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::Obj(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

impl Value {
    /// オブジェクトの欄。同じ名前が複数あれば最後のもの。
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fs) => fs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(a) => Some(a),
            _ => None,
        }
    }

    /// 空白のない 1 行の JSON にする。有限でない小数は `null` にする。
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Int(n) => out.push_str(&n.to_string()),
            Value::Float(x) if x.is_finite() => out.push_str(&format!("{x:?}")),
            Value::Float(_) => out.push_str("null"),
            Value::Str(s) => write_str(s, out),
            Value::Arr(a) => {
                out.push('[');
                for (i, v) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Value::Obj(fs) => {
                out.push('{');
                for (i, (k, v)) in fs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_str(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_json())
    }
}

fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// JSON の文書 1 つを読む。前後の空白は許し、後ろに他の字句があればエラー。
pub fn parse(src: &str) -> Result<Value, JsonError> {
    let mut p = Parser {
        b: src.as_bytes(),
        src,
        i: 0,
    };
    p.ws();
    let v = p.value(0)?;
    p.ws();
    if p.i < p.b.len() {
        return Err(p.err("文書の後ろに余計な字句がある"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    src: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> JsonError {
        JsonError {
            pos: self.i,
            msg: msg.to_string(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn lit(&mut self, word: &str, v: Value) -> Result<Value, JsonError> {
        if self.src[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(v)
        } else {
            Err(self.err("値として読めない"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, JsonError> {
        match self.peek() {
            None => Err(self.err("値がない（入力の終わり）")),
            Some(b'n') => self.lit("null", Value::Null),
            Some(b't') => self.lit("true", Value::Bool(true)),
            Some(b'f') => self.lit("false", Value::Bool(false)),
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'[') => self.array(depth),
            Some(b'{') => self.object(depth),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("値として読めない")),
        }
    }

    fn enter(&self, depth: usize) -> Result<usize, JsonError> {
        if depth >= MAX_DEPTH {
            Err(self.err("入れ子が深すぎる"))
        } else {
            Ok(depth + 1)
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, JsonError> {
        let depth = self.enter(depth)?;
        self.i += 1; // [
        let mut items = vec![];
        self.ws();
        if self.peek() == Some(b']') {
            self.i += 1;
            return Ok(Value::Arr(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth)?);
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Arr(items));
                }
                _ => return Err(self.err("配列の ',' か ']' が要る")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, JsonError> {
        let depth = self.enter(depth)?;
        self.i += 1; // {
        let mut fields = vec![];
        self.ws();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Obj(fields));
        }
        loop {
            self.ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("欄の名前（文字列）が要る"));
            }
            let key = self.string()?;
            self.ws();
            if self.peek() != Some(b':') {
                return Err(self.err("':' が要る"));
            }
            self.i += 1;
            self.ws();
            fields.push((key, self.value(depth)?));
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Obj(fields));
                }
                _ => return Err(self.err("オブジェクトの ',' か '}' が要る")),
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Value, JsonError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.i += 1;
                if matches!(self.peek(), Some(b'0'..=b'9')) {
                    return Err(self.err("数の先頭に 0 は置けない"));
                }
            }
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("数の桁がない")),
        }
        let mut float = false;
        if self.peek() == Some(b'.') {
            float = true;
            self.i += 1;
            if self.digits() == 0 {
                return Err(self.err("小数点の後ろに桁がない"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            float = true;
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("指数に桁がない"));
            }
        }
        let text = &self.src[start..self.i];
        if !float {
            if let Ok(n) = text.parse::<i64>() {
                return Ok(Value::Int(n));
            }
        }
        match text.parse::<f64>() {
            Ok(x) if x.is_finite() => Ok(Value::Float(x)),
            _ => Err(self.err("数が大きすぎる")),
        }
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        let mut n = 0u32;
        for _ in 0..4 {
            let d = match self.peek() {
                Some(c @ b'0'..=b'9') => c - b'0',
                Some(c @ b'a'..=b'f') => c - b'a' + 10,
                Some(c @ b'A'..=b'F') => c - b'A' + 10,
                _ => return Err(self.err("\\u の後ろに 16 進の 4 桁が要る")),
            };
            n = n * 16 + u32::from(d);
            self.i += 1;
        }
        Ok(n)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.i += 1; // "
        let mut out = String::new();
        loop {
            // 入力は &str なので、エスケープと制御文字以外は UTF-8 のまま写せる。
            let Some(c) = self.src[self.i..].chars().next() else {
                return Err(self.err("文字列が閉じていない"));
            };
            match c {
                '"' => {
                    self.i += 1;
                    return Ok(out);
                }
                '\\' => {
                    self.i += 1;
                    let e = self
                        .peek()
                        .ok_or_else(|| self.err("文字列が閉じていない"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{08}'),
                        b'f' => out.push('\u{0c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => {
                            self.i -= 1;
                            return Err(self.err("不正なエスケープ"));
                        }
                    }
                }
                c if (c as u32) < 0x20 => {
                    return Err(self.err("文字列の中に制御文字がある（エスケープが要る）"));
                }
                c => {
                    out.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }

    /// `\u` の後ろ。上位サロゲートは、続く `\uXXXX` の下位サロゲートと対にして 1 文字にする。
    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        let hi = self.hex4()?;
        match hi {
            0xD800..=0xDBFF => {
                if !self.src[self.i..].starts_with("\\u") {
                    return Err(self.err("上位サロゲートの後ろに下位サロゲートがない"));
                }
                self.i += 2;
                let lo = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&lo) {
                    return Err(self.err("上位サロゲートの後ろが下位サロゲートでない"));
                }
                char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                    .ok_or_else(|| self.err("不正なサロゲートの対"))
            }
            0xDC00..=0xDFFF => Err(self.err("対のない下位サロゲート")),
            n => char::from_u32(n).ok_or_else(|| self.err("不正な文字")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(v: &Value) {
        assert_eq!(&parse(&v.to_json()).unwrap(), v, "{}", v.to_json());
    }

    #[test]
    fn json_roundtrip() {
        let v = obj(vec![
            ("null", Value::Null),
            ("t", true.into()),
            ("f", false.into()),
            ("int", Value::Int(-42)),
            ("max", Value::Int(i64::MAX)),
            ("min", Value::Int(i64::MIN)),
            ("float", Value::Float(1.0)),
            ("small", Value::Float(-1.5e-7)),
            ("big", Value::Float(1e300)),
            (
                "s",
                "日本語 \"引用\" \\ / \n\r\t\u{8}\u{c}\u{1}\u{1f} 絵文字😀 \u{7f}".into(),
            ),
            ("empty_s", "".into()),
            (
                "arr",
                Value::Arr(vec![Value::Int(1), Value::Arr(vec![]), obj(vec![])]),
            ),
            ("入れ子", obj(vec![("a", obj(vec![("b", Value::Null)]))])),
        ]);
        rt(&v);
        // 整数と小数は区別して戻る
        assert_eq!(parse("1.0").unwrap(), Value::Float(1.0));
        assert_eq!(parse("1").unwrap(), Value::Int(1));
        // 有限でない小数は null で書く
        assert_eq!(Value::Float(f64::NAN).to_json(), "null");
        // 書き出しは空白なし、欄は入れた順
        assert_eq!(
            obj(vec![("b", 1i64.into()), ("a", vec![true, false].into())]).to_json(),
            "{\"b\":1,\"a\":[true,false]}"
        );
    }

    #[test]
    fn json_reads_rfc8259_forms() {
        // RFC 8259 の 13 章の例
        let v = parse(
            r#"{"Image": {"Width": 800, "Height": 600, "Title": "View from 15th Floor",
                "Thumbnail": {"Url": "http://www.example.com/image/481989943", "Height": 125, "Width": 100},
                "Animated": false, "IDs": [116, 943, 234, 38793]}}"#,
        )
        .unwrap();
        let img = v.get("Image").unwrap();
        assert_eq!(img.get("Width").unwrap().as_i64(), Some(800));
        assert_eq!(img.get("Animated").unwrap().as_bool(), Some(false));
        assert_eq!(img.get("IDs").unwrap().as_array().unwrap().len(), 4);
        let v = parse(r#"[{"precision": "zip", "Latitude": 37.7668, "Longitude": -122.3959, "Zip": "94107"}]"#)
            .unwrap();
        assert_eq!(
            v.as_array().unwrap()[0].get("Latitude"),
            Some(&Value::Float(37.7668))
        );
        assert_eq!(parse("\"Hello world!\"").unwrap(), "Hello world!".into());
        assert_eq!(parse("42").unwrap(), Value::Int(42));
        assert_eq!(parse(" \t\r\n true \n").unwrap(), Value::Bool(true));
        // 数
        assert_eq!(parse("-0").unwrap(), Value::Int(0));
        assert_eq!(parse("0.5").unwrap(), Value::Float(0.5));
        assert_eq!(parse("1E+2").unwrap(), Value::Float(100.0));
        assert_eq!(parse("1e-2").unwrap(), Value::Float(0.01));
        assert_eq!(
            parse("12345678901234567890").unwrap(),
            Value::Float(12345678901234567890.0)
        );
        // エスケープ
        assert_eq!(parse(r#""a\/b\u0041\u00e9""#).unwrap(), "a/bAé".into());
        assert_eq!(parse(r#""\ud83d\ude00""#).unwrap(), "😀".into());
        // 同じ名前は最後が勝つ
        assert_eq!(
            parse(r#"{"a":1,"a":2}"#).unwrap().get("a"),
            Some(&Value::Int(2))
        );
    }

    #[test]
    fn json_rejects_malformed() {
        let bad = [
            "",
            " ",
            "[",
            "[1",
            "[1,]",
            "[,1]",
            "{",
            "{\"a\"}",
            "{\"a\":}",
            "{\"a\":1,}",
            "{a:1}",
            "{\"a\":1",
            "]",
            "}",
            "\"abc",
            "\"a\\qb\"",
            "\"\\u12\"",
            "\"\\u12g4\"",
            "\"\\ud83d\"",
            "\"\\ud83dx\"",
            "\"\\ud83d\\u0041\"",
            "\"\\ude00\"",
            "\"a\nb\"",
            "\"a\tb\"",
            "\"\u{1}\"",
            "01",
            "-01",
            "00",
            "1.",
            ".5",
            "-",
            "+1",
            "1e",
            "1e+",
            "0x10",
            "nul",
            "True",
            "NaN",
            "Infinity",
            "1e999",
            "'a'",
            "1 2",
            "[] []",
            "{} x",
            "// c\n1",
            "\u{a0}1",
        ];
        for s in bad {
            assert!(parse(s).is_err(), "{s:?} は拒む");
        }
        let e = parse("[1,\n x]").unwrap_err();
        assert_eq!(e.pos, 5);
    }

    #[test]
    fn json_depth_limit() {
        let n = MAX_DEPTH;
        let ok = format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(parse(&ok).is_ok());
        let deep = format!("{}{}", "[".repeat(n + 1), "]".repeat(n + 1));
        assert!(parse(&deep).unwrap_err().msg.contains("深すぎる"));
        // 閉じない巨大な入れ子でもスタックを使い切らずにエラーになる
        let huge = "[".repeat(1_000_000);
        assert!(parse(&huge).is_err());
        let huge_obj = "{\"a\":".repeat(1_000_000);
        assert!(parse(&huge_obj).is_err());
    }
}

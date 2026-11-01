//! 整形器（`sigil fmt`。ADR 0009 の決定 11、ADR 0008 の決定 2・8）。
//!
//! 構文木とコメントとソースの行から正規形を出力する。出力は「構文木」「コメントの
//! 位置と本文」「ブロックの中の空行の有無」から一意に決まる。`fmt --check` は、出力が
//! 入力と一致するかを見る。
//!
//! 出力は行の並び（各行に、対応するソースの行番号を付ける）として作り、行末コメントは
//! 最後にソースの行番号で行に付ける。単独の行のコメントは、次の要素を出力する前に、
//! その要素より前の行にあるものをすべて出力する。

mod expr;
mod items;
mod types;
mod write;

pub use types::normalize_effects;
pub(crate) use types::{escape_str, needs_u_escape};
#[doc(hidden)]
pub use write::write_atomic_with_hook;
pub use write::{write_atomic_if_unchanged, Stamp};

use crate::ast::*;
use crate::error::{Error, ErrorKind, Span};
use crate::lexer::Comment;
use crate::parser::parse_with_comments;
use std::collections::BTreeMap;

/// 整形した結果を返す。構文エラーならそのエラー。
///
/// 返す前に出力を検証する: 再び構文解析でき、構文木が入力と（括弧の違いを除いて）一致し、
/// もう一度整形しても変わらないこと。満たさなければ `ErrorKind::FmtVerify` を返す
/// （書き込む側はファイルを変更しない）。
pub fn format(src: &str) -> Result<String, Error> {
    let out = render(src, false)?;
    let bad = |msg: String| Error::new(ErrorKind::FmtVerify, Span::default(), msg);
    if let Err(e) = parse_with_comments(&out) {
        return Err(bad(format!(
            "整形結果が構文解析できない（深さの上限を超える場合を含む）: {e}"
        )));
    }
    if render(src, true)? != render(&out, true)? {
        return Err(bad("整形結果の構文木が入力と一致しない".to_string()));
    }
    if render(&out, false)? != out {
        return Err(bad("整形結果をもう一度整形すると変わる".to_string()));
    }
    Ok(out)
}

/// `full` のとき、原始式以外をすべて括弧で囲み、コメントと空行を除いて出力する（構文木の比較用）。
fn render(src: &str, full: bool) -> Result<String, Error> {
    let parsed = parse_with_comments(src)?;
    let mut p = Printer::new(src, &parsed.comments);
    p.full = full;
    if full {
        // 比較用の出力にはコメントと空行を含めない（置き場所は整形で変わりうる）。
        p.own.clear();
        p.trailing.clear();
    }
    p.file(&parsed.file);
    Ok(p.finish())
}

/// 腕の本体が `return 式;` だけ、または、すべての腕がそうである `match` だけ。
pub(super) fn returns_only(b: &Block) -> bool {
    match b.stmts.as_slice() {
        [st] => match &st.kind {
            StmtKind::Return(Some(_)) => true,
            StmtKind::Match { arms, .. } => arms.iter().all(|a| returns_only(&a.body)),
            _ => false,
        },
        _ => false,
    }
}

/// 正規形との最初の違い。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    /// 1 始まりの行番号（入力の行）。
    pub line: usize,
    /// 入力のその行（ファイルの終わりなら空）。
    pub found: String,
    /// 正規形のその行（ファイルの終わりなら空）。
    pub expected: String,
}

/// 正規形かを調べる。正規形なら `Ok(None)`。
pub fn check(src: &str) -> Result<Option<Diff>, Error> {
    let want = format(src)?;
    Ok(first_diff(src, &want))
}

fn first_diff(src: &str, want: &str) -> Option<Diff> {
    if src == want {
        return None;
    }
    let a: Vec<&str> = src.split('\n').collect();
    let b: Vec<&str> = want.split('\n').collect();
    let n = a.len().max(b.len());
    for i in 0..n {
        let (x, y) = (a.get(i).copied(), b.get(i).copied());
        if x != y {
            return Some(Diff {
                line: i + 1,
                found: x.unwrap_or("").to_string(),
                expected: y.unwrap_or("").to_string(),
            });
        }
    }
    None
}

// ---- 優先順位の段（ADR 0023 決定 3） ----
pub(super) const LOWEST: u8 = 0; // クロージャ
pub(super) const OR: u8 = 1; // ||
pub(super) const AND: u8 = 2; // &&
pub(super) const CMP: u8 = 3; // == != < <= > >=
pub(super) const BITOR: u8 = 4; // |
pub(super) const BITXOR: u8 = 5; // ^
pub(super) const BITAND: u8 = 6; // &
pub(super) const SHIFT: u8 = 7; // << >>
pub(super) const ADD: u8 = 8; // + -
pub(super) const MUL: u8 = 9; // * / %
pub(super) const PREFIX: u8 = 10;
pub(super) const POSTFIX: u8 = 11;
pub(super) const PRIMARY: u8 = 12;

struct Line {
    pub(super) text: String,
    /// 対応するソースの行（空行は `None`）。
    pub(super) tag: Option<u32>,
}

struct Printer<'a> {
    pub(super) lines: Vec<Line>,
    pub(super) src_lines: Vec<&'a str>,
    pub(super) own: Vec<&'a Comment>,
    pub(super) own_i: usize,
    pub(super) trailing: Vec<&'a Comment>,
    pub(super) full: bool,
    /// 文の先頭の式を出力中（`match` は括弧で囲まないと文の `match` になる）。
    pub(super) lead: bool,
}

/// 名前 `Unit` の型（`Unit` は予約名なので、構文だけで判定できる。ADR 0011 の決定 2）。
pub(crate) fn is_unit(t: &Type) -> bool {
    matches!(t, Type::Path { path, args } if args.is_empty() && path.len() == 1 && path[0] == "Unit")
}

/// コメントの本文は書き換えない。直す（行末の空白を落とす・`// ` の空白を補う）のは次だけ。
/// - 本文が空なら `//`
/// - `//` の直後が空白または `/` なら（`///` や `//  x` を含め）そのまま
/// - それ以外は `// ` を補う
fn comment_text(c: &Comment) -> String {
    let t = c.text.trim_end();
    if t.is_empty() {
        "//".to_string()
    } else if t.starts_with([' ', '/']) {
        format!("//{t}")
    } else {
        format!("// {t}")
    }
}

/// 式を正規形の文字列の行として返す（`sigil spec` が条件の式をソースの表記で示すため）。
pub(crate) fn expr_lines(e: &Expr) -> Vec<String> {
    let mut p = Printer::new("", &[]);
    p.nl(0, 0);
    p.expr(e, 0, false, 0);
    p.lines.into_iter().map(|l| l.text).collect()
}

/// 型を正規形の文字列にする。
pub(crate) fn type_text(t: &Type) -> String {
    let mut p = Printer::new("", &[]);
    p.nl(0, 0);
    p.ty(t);
    p.lines.remove(0).text
}

impl<'a> Printer<'a> {
    fn new(src: &'a str, comments: &'a [Comment]) -> Self {
        Printer {
            lines: Vec::new(),
            src_lines: src.split('\n').collect(),
            own: comments.iter().filter(|c| !c.trailing).collect(),
            own_i: 0,
            trailing: comments.iter().filter(|c| c.trailing).collect(),
            full: false,
            lead: false,
        }
    }

    fn finish(mut self) -> String {
        // 行末コメントは、ソースの同じ行に対応する出力行（同じ番号が複数なら後の行）に付ける。
        // 対応する行がない（畳み込み・削除で消えた）か、その行にすでに付いているときは、
        // 次の行の前に単独の行のコメントとして置く（隣の行に移したり連結したりしない）。
        let mut by_tag: BTreeMap<u32, usize> = BTreeMap::new();
        for (i, l) in self.lines.iter().enumerate() {
            if let Some(t) = l.tag {
                by_tag.insert(t, i);
            }
        }
        let mut attached: Vec<Option<String>> = vec![None; self.lines.len()];
        let mut before: Vec<(usize, String)> = Vec::new();
        for c in std::mem::take(&mut self.trailing) {
            let text = comment_text(c);
            match by_tag.get(&c.span.line) {
                Some(&i) if attached[i].is_none() => attached[i] = Some(text),
                _ => {
                    let i = by_tag
                        .range(c.span.line + 1..)
                        .next()
                        .map(|(_, &i)| i)
                        .unwrap_or(self.lines.len().saturating_sub(1));
                    before.push((i, text));
                }
            }
        }
        let mut out = String::new();
        for (i, l) in self.lines.iter().enumerate() {
            for (_, text) in before.iter().filter(|(b, _)| *b == i) {
                let mut indent = l.text.len() - l.text.trim_start().len();
                if l.text.trim_start().starts_with('}') {
                    indent += 2; // 閉じ波括弧の前なら、ブロックの中の字下げ
                }
                out.push_str(&" ".repeat(indent));
                out.push_str(text);
                out.push('\n');
            }
            out.push_str(&l.text);
            if let Some(t) = &attached[i] {
                out.push_str("  ");
                out.push_str(t);
            }
            out.push('\n');
        }
        out
    }

    // ---- 行の操作 ----

    pub(super) fn nl(&mut self, indent: usize, tag: u32) {
        self.lines.push(Line {
            text: "  ".repeat(indent),
            tag: Some(tag),
        });
    }

    pub(super) fn blank(&mut self) {
        if self.full {
            return;
        }
        self.lines.push(Line {
            text: String::new(),
            tag: None,
        });
    }

    pub(super) fn w(&mut self, s: &str) {
        if let Some(l) = self.lines.last_mut() {
            l.text.push_str(s);
        }
    }

    pub(super) fn src_blank(&self, line: u32) -> bool {
        line >= 1
            && self
                .src_lines
                .get(line as usize - 1)
                .is_some_and(|l| l.trim().is_empty())
    }

    pub(super) fn pending_before(&self, line: u32) -> bool {
        self.own.get(self.own_i).is_some_and(|c| c.span.line < line)
    }

    /// `line` より前にある単独の行のコメントを出力する。`blanks` のとき、ソースで直前が
    /// 空行なら空行を 1 行置く（ブロックの中。先頭の要素の前は置かない）。
    pub(super) fn comments_before(
        &mut self,
        line: u32,
        indent: usize,
        blanks: bool,
        first: &mut bool,
    ) {
        while let Some(&c) = self.own.get(self.own_i) {
            if c.span.line >= line {
                break;
            }
            self.own_i += 1;
            if blanks && !*first && self.src_blank(c.span.line - 1) {
                self.blank();
            }
            self.nl(indent, c.span.line);
            self.w(&comment_text(c));
            *first = false;
        }
    }

    /// ブロックの中の要素の始まり。
    pub(super) fn item_start(&mut self, line: u32, indent: usize, first: &mut bool) {
        self.comments_before(line, indent, true, first);
        if !*first && self.src_blank(line - 1) {
            self.blank();
        }
        self.nl(indent, line);
        *first = false;
    }

    /// 節・フィールド・腕など、空行を置かない要素の前のコメント。
    pub(super) fn plain_start(&mut self, line: u32, indent: usize) {
        let mut f = true;
        self.comments_before(line, indent, false, &mut f);
        self.nl(indent, line);
    }

    /// トップレベルで `next_line` の要素の前にあるコメントの塊を出力する。
    /// `attach` のとき、直前の行にある塊は要素と空行なしでつなぐ。
    pub(super) fn top_comments(&mut self, next_line: u32, attach: bool, tail: bool) {
        let mut last_end: Option<u32> = None;
        while self.pending_before(next_line) {
            if !self.lines.is_empty() {
                self.blank();
            }
            let mut prev: Option<u32> = None;
            while let Some(&c) = self.own.get(self.own_i) {
                if c.span.line >= next_line || prev.is_some_and(|p| c.span.line != p + 1) {
                    break;
                }
                self.own_i += 1;
                self.nl(0, c.span.line);
                self.w(&comment_text(c));
                prev = Some(c.span.line);
            }
            last_end = prev;
        }
        if tail || self.lines.is_empty() {
            return;
        }
        let adjacent = attach && last_end.is_some_and(|e| e + 1 == next_line);
        if !adjacent {
            self.blank();
        }
    }
}

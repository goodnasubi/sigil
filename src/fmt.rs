//! 整形器（`sigil fmt`。ADR 0009 の決定 11、ADR 0008 の決定 2・8）。
//!
//! 構文木とコメントとソースの行から正規形を出力する。出力は「構文木」「コメントの
//! 位置と本文」「ブロックの中の空行の有無」から一意に決まる。`fmt --check` は、出力が
//! 入力と一致するかを見る。
//!
//! 出力は行の並び（各行に、対応するソースの行番号を付ける）として作り、行末コメントは
//! 最後にソースの行番号で行に付ける。単独の行のコメントは、次の要素を出力する前に、
//! その要素より前の行にあるものをすべて出力する。

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

// ---- 優先順位の段 ----
const LOWEST: u8 = 0; // クロージャ
const OR: u8 = 1;
const AND: u8 = 2;
const CMP: u8 = 3;
const ADD: u8 = 4;
const MUL: u8 = 5;
const PREFIX: u8 = 6;
const POSTFIX: u8 = 7;
const PRIMARY: u8 = 8;

struct Line {
    text: String,
    /// 対応するソースの行（空行は `None`）。
    tag: Option<u32>,
}

struct Printer<'a> {
    lines: Vec<Line>,
    src_lines: Vec<&'a str>,
    own: Vec<&'a Comment>,
    own_i: usize,
    trailing: Vec<&'a Comment>,
    full: bool,
    /// 文の先頭の式を出力中（`match` は括弧で囲まないと文の `match` になる）。
    lead: bool,
}

fn strip(mut e: &Expr) -> &Expr {
    while let Expr::Paren(inner) = e {
        e = inner;
    }
    e
}

fn level(e: &Expr) -> u8 {
    match e {
        Expr::Closure { .. } => LOWEST,
        Expr::Binary { op, .. } => binop_level(*op),
        Expr::Unary { .. } => PREFIX,
        Expr::Field { .. } | Expr::Call { .. } | Expr::Try(_) | Expr::StructLit { .. } => POSTFIX,
        _ => PRIMARY,
    }
}

fn binop_level(op: BinOp) -> u8 {
    match op {
        BinOp::Or => OR,
        BinOp::And => AND,
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => CMP,
        BinOp::Add | BinOp::Sub => ADD,
        BinOp::Mul | BinOp::Div | BinOp::Rem => MUL,
    }
}

fn binop_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn is_logic(op: BinOp) -> bool {
    matches!(op, BinOp::And | BinOp::Or)
}

/// 書式文字（Unicode の Cf）。文字列の中では `\u{}` で書く（ADR 0009 の決定 6）。
fn is_format_char(c: char) -> bool {
    let n = c as u32;
    matches!(n,
        0xAD | 0x61C | 0x6DD | 0x70F | 0x8E2 | 0x180E | 0xFEFF | 0x110BD | 0x110CD | 0xE0001
        | 0x600..=0x605 | 0x890..=0x891 | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFFF9..=0xFFFB | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0020..=0xE007F)
}

/// 文字列リテラルの正規形（ADR 0009 の決定 6）。
/// 文字列リテラル内で `\u{..}` 形式にしなければならない文字か。
pub(crate) fn needs_u_escape(c: char) -> bool {
    c.is_control() || is_format_char(c) || (c.is_whitespace() && c != ' ')
}

pub fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if needs_u_escape(c) => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// 小数の正規形: 小数部の末尾の 0 は、`0` 1 桁の場合を除いて書かない（ADR 0009 の決定 5）。
fn float_str(s: &str) -> String {
    match s.split_once('.') {
        Some((int, frac)) => {
            let t = frac.trim_end_matches('0');
            format!("{int}.{}", if t.is_empty() { "0" } else { t })
        }
        None => s.to_string(),
    }
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

/// ADR 0004 の表で、下位が 2 つ以上ある上位語。
const PARENTS: &[(&str, &[&str])] = &[
    ("fs", &["fs.read", "fs.write"]),
    ("net", &["net.client", "net.server"]),
    ("task", &["task.share", "task.spawn", "task.suspend"]),
];

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

/// 効果の並びの正規形（ADR 0008 の決定 8）。
pub fn normalize_effects(effects: &[Effect], params: &[Param]) -> Vec<String> {
    let mut ofs: Vec<(usize, String)> = Vec::new();
    let mut cats: Vec<String> = Vec::new();
    for e in effects {
        match e {
            Effect::Of(n) => {
                if !ofs.iter().any(|(_, m)| m == n) {
                    let idx = params
                        .iter()
                        .position(|p| &p.name == n)
                        .unwrap_or(usize::MAX);
                    ofs.push((idx, n.clone()));
                }
            }
            Effect::Category(path) => {
                let s = path.join(".");
                if !cats.contains(&s) {
                    cats.push(s);
                }
            }
        }
    }
    // 上位があれば下位を消し、下位がすべてそろえば上位にまとめる。
    for (parent, children) in PARENTS {
        if cats.iter().any(|c| c == parent) {
            cats.retain(|c| !children.contains(&c.as_str()));
        } else if children.iter().all(|ch| cats.iter().any(|c| c == ch)) {
            cats.retain(|c| !children.contains(&c.as_str()));
            cats.push((*parent).to_string());
        }
    }
    cats.sort();
    ofs.sort_by_key(|(i, _)| *i); // 安定ソート: 引数にない名前は元の順のまま最後
    ofs.into_iter()
        .map(|(_, n)| format!("of({n})"))
        .chain(cats)
        .collect()
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

    fn nl(&mut self, indent: usize, tag: u32) {
        self.lines.push(Line {
            text: "  ".repeat(indent),
            tag: Some(tag),
        });
    }

    fn blank(&mut self) {
        if self.full {
            return;
        }
        self.lines.push(Line {
            text: String::new(),
            tag: None,
        });
    }

    fn w(&mut self, s: &str) {
        if let Some(l) = self.lines.last_mut() {
            l.text.push_str(s);
        }
    }

    fn src_blank(&self, line: u32) -> bool {
        line >= 1
            && self
                .src_lines
                .get(line as usize - 1)
                .is_some_and(|l| l.trim().is_empty())
    }

    fn pending_before(&self, line: u32) -> bool {
        self.own.get(self.own_i).is_some_and(|c| c.span.line < line)
    }

    /// `line` より前にある単独の行のコメントを出力する。`blanks` のとき、ソースで直前が
    /// 空行なら空行を 1 行置く（ブロックの中。先頭の要素の前は置かない）。
    fn comments_before(&mut self, line: u32, indent: usize, blanks: bool, first: &mut bool) {
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
    fn item_start(&mut self, line: u32, indent: usize, first: &mut bool) {
        self.comments_before(line, indent, true, first);
        if !*first && self.src_blank(line - 1) {
            self.blank();
        }
        self.nl(indent, line);
        *first = false;
    }

    /// 節・フィールド・腕など、空行を置かない要素の前のコメント。
    fn plain_start(&mut self, line: u32, indent: usize) {
        let mut f = true;
        self.comments_before(line, indent, false, &mut f);
        self.nl(indent, line);
    }

    /// トップレベルで `next_line` の要素の前にあるコメントの塊を出力する。
    /// `attach` のとき、直前の行にある塊は要素と空行なしでつなぐ。
    fn top_comments(&mut self, next_line: u32, attach: bool, tail: bool) {
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

    // ---- ファイル ----

    fn file(&mut self, f: &File) {
        let m = &f.module;
        self.top_comments(m.span.line, false, false);
        self.nl(0, m.span.line);
        self.w(&format!("module {}", m.name.join(".")));
        self.plain_start(m.targets_line, 1);
        self.w(&format!("targets {}", m.targets.join(", ")));
        self.plain_start(m.trust_line, 1);
        self.w(match m.trust {
            Trust::None => "trust none",
            Trust::Base => "trust base",
        });
        for item in &f.items {
            match item {
                Item::Type(t) => {
                    self.top_comments(t.span.line, true, false);
                    self.nl(0, t.span.line);
                    self.w(&format!("type {} = ", t.name));
                    self.ty(&t.base);
                    self.w(" where ");
                    self.expr(&t.cond, 0, false, 0);
                    self.w(";");
                }
                Item::Struct(s) => self.struct_decl(s),
                Item::Enum(e) => self.enum_decl(e),
                Item::Fn(d) => self.fn_decl(d),
            }
        }
        self.top_comments(u32::MAX, false, true);
    }

    fn generics(&mut self, gs: &[Generic]) {
        if gs.is_empty() {
            return;
        }
        self.w("<");
        for (i, g) in gs.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.w(&g.name);
            if let Some(b) = &g.bound {
                self.w(&format!(": {}", b.join(".")));
            }
        }
        self.w(">");
    }

    fn struct_decl(&mut self, s: &StructDecl) {
        self.top_comments(s.span.line, true, false);
        self.nl(0, s.span.line);
        self.w(&format!("struct {}", s.name));
        self.generics(&s.generics);
        if s.fields.is_empty() && !self.pending_before(s.end_line) {
            self.w(" {}");
            return;
        }
        self.w(" {");
        for f in &s.fields {
            self.plain_start(f.line, 1);
            self.w(&format!("{}: ", f.name));
            self.ty(&f.ty);
            self.w(",");
        }
        let mut first = false;
        self.comments_before(s.end_line, 1, false, &mut first);
        self.nl(0, s.end_line);
        self.w("}");
    }

    fn enum_decl(&mut self, e: &EnumDecl) {
        self.top_comments(e.span.line, true, false);
        self.nl(0, e.span.line);
        self.w(&format!("enum {}", e.name));
        self.generics(&e.generics);
        self.w(" {");
        for v in &e.variants {
            self.plain_start(v.line, 1);
            self.w(&v.name);
            if !v.fields.is_empty() {
                self.w("(");
                for (i, t) in v.fields.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.ty(t);
                }
                self.w(")");
            }
            self.w(",");
        }
        let mut first = false;
        self.comments_before(e.end_line, 1, false, &mut first);
        self.nl(0, e.end_line);
        self.w("}");
    }

    fn fn_decl(&mut self, d: &FnDecl) {
        self.top_comments(d.span.line, true, false);
        self.nl(0, d.span.line);
        self.w(&format!("fn {}", d.name));
        self.generics(&d.generics);
        self.w("(");
        for (i, p) in d.params.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.w(&format!("{}: ", p.name));
            self.ty(&p.ty);
        }
        self.w(")");
        if let Some(r) = &d.ret {
            self.w(" -> ");
            self.ty(r);
        }
        self.plain_start(d.clause_lines[0], 1);
        self.w(&format!("intent {}", escape_str(&d.intent)));
        self.plain_start(d.clause_lines[1], 1);
        self.w("requires ");
        match &d.requires {
            Some(e) => self.expr(e, 0, false, 1),
            None => self.w("none"),
        }
        self.plain_start(d.clause_lines[2], 1);
        self.w("ensures ");
        match &d.ensures {
            Some(e) => self.expr(e, 0, false, 1),
            None => self.w("none"),
        }
        self.plain_start(d.clause_lines[3], 1);
        let effs = normalize_effects(&d.effects, &d.params);
        if effs.is_empty() {
            self.w("effects none");
        } else {
            self.w(&format!("effects {}", effs.join(", ")));
        }
        self.plain_start(d.body.open_line, 0);
        // 戻り値のない関数の末尾の `return;` は書かない（ADR 0008 の決定 2）。
        let drop_ret = d.ret.is_none()
            && matches!(
                d.body.stmts.last(),
                Some(Stmt {
                    kind: StmtKind::Return(None),
                    ..
                })
            );
        self.block(&d.body, 0, drop_ret);
    }

    // ---- 型・パターン ----

    fn ty(&mut self, t: &Type) {
        match t {
            Type::Path { path, args } => {
                self.w(&path.join("."));
                if !args.is_empty() {
                    self.w("<");
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            self.w(", ");
                        }
                        self.ty(a);
                    }
                    self.w(">");
                }
            }
            Type::Ref { mutable, inner } => {
                self.w(if *mutable { "&mut " } else { "&" });
                self.ty(inner);
            }
            Type::Fn { params, ret } => {
                self.w("fn(");
                for (i, a) in params.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.ty(a);
                }
                self.w(")");
                if let Some(r) = ret {
                    self.w(" -> ");
                    self.ty(r);
                }
            }
            Type::Tuple(ts) => {
                self.w("(");
                for (i, a) in ts.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.ty(a);
                }
                self.w(")");
            }
        }
    }

    fn pattern(&mut self, p: &Pattern) {
        match p {
            Pattern::Wild => self.w("_"),
            Pattern::Bind(n) => self.w(n),
            Pattern::Int(s) => self.w(s),
            Pattern::NegInt(s) => self.w(&format!("-{s}")),
            Pattern::Str(s) => self.w(&escape_str(s)),
            Pattern::Bool(b) => self.w(if *b { "true" } else { "false" }),
            Pattern::Variant { path, args } => {
                self.w(&path.join("."));
                if !args.is_empty() {
                    self.w("(");
                    self.patterns(args);
                    self.w(")");
                }
            }
            Pattern::Tuple(ps) => {
                self.w("(");
                self.patterns(ps);
                self.w(")");
            }
        }
    }

    fn patterns(&mut self, ps: &[Pattern]) {
        for (i, p) in ps.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.pattern(p);
        }
    }

    // ---- 文 ----

    /// `{` から `}` まで。`{` の前の空白は呼び出し側が置く。
    fn block(&mut self, b: &Block, indent: usize, drop_last_return: bool) {
        let n = b.stmts.len() - usize::from(drop_last_return);
        if n == 0 && !self.pending_before(b.end_line) {
            self.w("{}");
            return;
        }
        self.w("{");
        let mut first = true;
        for s in &b.stmts[..n] {
            self.item_start(s.span.line, indent + 1, &mut first);
            self.stmt(s, indent + 1);
        }
        self.comments_before(b.end_line, indent + 1, true, &mut first);
        self.nl(indent, b.end_line);
        self.w("}");
    }

    fn stmt(&mut self, s: &Stmt, indent: usize) {
        match &s.kind {
            StmtKind::Let {
                mutable,
                name,
                value,
            } => {
                self.w(if *mutable { "let mut " } else { "let " });
                self.w(name);
                self.w(" = ");
                self.expr(value, 0, false, indent);
                self.w(";");
            }
            StmtKind::Assign { target, value } => {
                self.lead = true;
                self.expr(target, 0, false, indent);
                self.w(" = ");
                self.expr(value, 0, false, indent);
                self.w(";");
            }
            StmtKind::Expr(e) => {
                self.lead = true;
                self.expr(e, 0, false, indent);
                self.w(";");
            }
            StmtKind::Return(None) => self.w("return;"),
            StmtKind::Return(Some(e)) => {
                self.w("return ");
                self.expr(e, 0, false, indent);
                self.w(";");
            }
            StmtKind::If(i) => self.if_stmt(i, indent),
            StmtKind::While { cond, body } => {
                self.w("while ");
                self.expr(cond, 0, true, indent);
                self.w(" ");
                self.block(body, indent, false);
            }
            StmtKind::Match {
                scrutinee,
                arms,
                end_line,
            } => self.stmt_match(scrutinee, arms, *end_line, indent),
        }
    }

    fn if_stmt(&mut self, i: &IfStmt, indent: usize) {
        let mut cur = i;
        loop {
            self.w("if ");
            self.expr(&cur.cond, 0, true, indent);
            self.w(" ");
            self.block(&cur.then, indent, false);
            match cur.otherwise.as_deref() {
                None => break,
                Some(Else::Block(b)) => {
                    self.w(" else ");
                    self.block(b, indent, false);
                    break;
                }
                Some(Else::If(next)) => {
                    self.w(" else ");
                    cur = next;
                }
            }
        }
    }

    fn stmt_match(&mut self, scrutinee: &Expr, arms: &[StmtArm], end_line: u32, indent: usize) {
        // すべての腕が `return 式;` だけなら `return match ...;`（ADR 0009 の決定 7）。
        let all_return = arms.iter().all(|a| {
            a.body.stmts.len() == 1 && matches!(a.body.stmts[0].kind, StmtKind::Return(Some(_)))
        });
        self.w(if all_return {
            "return match "
        } else {
            "match "
        });
        self.expr(scrutinee, 0, true, indent);
        self.w(" {");
        for a in arms {
            self.plain_start(a.line, indent + 1);
            self.pattern(&a.pattern);
            self.w(" => ");
            if all_return {
                if let StmtKind::Return(Some(e)) = &a.body.stmts[0].kind {
                    self.expr(e, 0, false, indent + 1);
                }
                self.w(",");
            } else {
                self.block(&a.body, indent + 1, false);
            }
        }
        let mut first = false;
        self.comments_before(end_line, indent + 1, false, &mut first);
        self.nl(indent, end_line);
        self.w(if all_return { "};" } else { "}" });
    }

    // ---- 式 ----

    /// 括弧は必要なものだけ書く。`min` は文脈が求める最低の段、`ns` は条件・`match` の
    /// 対象の中（構造体の値に括弧が要る）。
    fn expr(&mut self, e: &Expr, min: u8, ns: bool, indent: usize) {
        self.expr_ctx(e, min, ns, false, indent);
    }

    fn expr_ctx(&mut self, e: &Expr, min: u8, ns: bool, force: bool, indent: usize) {
        let e = strip(e);
        let lead = std::mem::take(&mut self.lead);
        let wrap = force
            || level(e) < min
            || (ns && matches!(e, Expr::StructLit { .. }))
            || (lead && matches!(e, Expr::Match { .. }))
            || (self.full && level(e) < PRIMARY);
        if wrap {
            self.w("(");
            self.expr_inner(e, false, indent);
            self.w(")");
        } else {
            // 最初に出力する子が文の先頭に残る式だけ、先頭であることを引き継ぐ。
            self.lead = lead
                && matches!(
                    e,
                    Expr::Field { .. } | Expr::Call { .. } | Expr::Try(_) | Expr::Binary { .. }
                );
            self.expr_inner(e, ns, indent);
            self.lead = false;
        }
    }

    fn args(&mut self, es: &[Expr], indent: usize) {
        for (i, a) in es.iter().enumerate() {
            if i > 0 {
                self.w(", ");
            }
            self.expr(a, 0, false, indent);
        }
    }

    fn expr_inner(&mut self, e: &Expr, ns: bool, indent: usize) {
        match e {
            Expr::Int(s) => self.w(s),
            Expr::Float(s) => self.w(&float_str(s)),
            Expr::Str(s) => self.w(&escape_str(s)),
            Expr::Bool(b) => self.w(if *b { "true" } else { "false" }),
            Expr::Name(n) => self.w(n),
            Expr::Result => self.w("result"),
            Expr::SelfValue => self.w("self"),
            Expr::Old(x) => {
                self.w("old(");
                self.expr(x, 0, false, indent);
                self.w(")");
            }
            Expr::Paren(x) => self.expr_ctx(x, 0, ns, false, indent),
            Expr::Tuple(items) => {
                self.w("(");
                self.args(items, indent);
                self.w(")");
            }
            Expr::Unary { op, expr } => {
                self.w(match op {
                    UnOp::Not => "!",
                    UnOp::Neg => "-",
                    UnOp::Ref => "&",
                    UnOp::RefMut => "&mut ",
                });
                // `&` が続くと `&&` になるので、参照の参照は括弧を残す。
                let force = matches!(op, UnOp::Ref | UnOp::RefMut)
                    && matches!(
                        strip(expr),
                        Expr::Unary {
                            op: UnOp::Ref | UnOp::RefMut,
                            ..
                        }
                    );
                self.expr_ctx(expr, PREFIX, ns, force, indent);
            }
            Expr::Binary { op, lhs, rhs } => {
                let p = binop_level(*op);
                let (lmin, rmin) = if p == CMP { (ADD, ADD) } else { (p, p + 1) };
                // `&&` と `||` は括弧なしで混ぜられない。
                let mixed = |x: &Expr| matches!(strip(x), Expr::Binary { op: o, .. } if is_logic(*o) && is_logic(*op) && o != op);
                self.expr_ctx(lhs, lmin, ns, mixed(lhs), indent);
                self.w(&format!(" {} ", binop_str(*op)));
                self.expr_ctx(rhs, rmin, ns, mixed(rhs), indent);
            }
            Expr::Field { expr, name } => {
                // `1.abs` は字句エラーなので整数の基には括弧が要る。
                let force = matches!(strip(expr), Expr::Int(_) | Expr::StructLit { .. });
                self.expr_ctx(expr, POSTFIX, ns, force, indent);
                self.w(".");
                self.w(name);
            }
            Expr::Call { callee, args } => {
                let force = matches!(strip(callee), Expr::StructLit { .. });
                self.expr_ctx(callee, POSTFIX, ns, force, indent);
                self.w("(");
                self.args(args, indent);
                self.w(")");
            }
            Expr::Try(x) => {
                let force = matches!(strip(x), Expr::StructLit { .. });
                self.expr_ctx(x, POSTFIX, ns, force, indent);
                self.w("?");
            }
            Expr::StructLit { path, fields } => {
                self.w(&path.join("."));
                self.w(" { ");
                for (i, (n, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        self.w(", ");
                    }
                    self.w(&format!("{n}: "));
                    self.expr(v, 0, false, indent);
                }
                self.w(" }");
            }
            Expr::Match {
                scrutinee,
                arms,
                end_line,
            } => {
                self.w("match ");
                self.expr(scrutinee, 0, true, indent);
                self.w(" {");
                for a in arms {
                    self.plain_start(a.line, indent + 1);
                    self.pattern(&a.pattern);
                    self.w(" => ");
                    self.expr(&a.body, 0, false, indent + 1);
                    self.w(",");
                }
                let mut first = false;
                self.comments_before(*end_line, indent + 1, false, &mut first);
                self.nl(indent, *end_line);
                self.w("}");
            }
            Expr::Closure {
                is_move,
                params,
                body,
            } => {
                if *is_move {
                    self.w("move ");
                }
                self.w("|");
                self.w(&params.join(", "));
                self.w("| ");
                match body {
                    ClosureBody::Expr(x) => self.expr(x, 0, ns, indent),
                    // `|x| { return e; }` は `|x| e`（ADR 0008 の決定 2）。
                    ClosureBody::Block(b) => match b.stmts.as_slice() {
                        [Stmt {
                            kind: StmtKind::Return(Some(x)),
                            ..
                        }] => self.expr(x, 0, ns, indent),
                        _ => self.block(b, indent, false),
                    },
                }
            }
        }
    }
}

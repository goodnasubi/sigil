use crate::ast::*;

use super::{is_unit, Printer};

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

/// ADR 0004 の表で、下位が 2 つ以上ある上位語。
const PARENTS: &[(&str, &[&str])] = &[
    ("fs", &["fs.read", "fs.write"]),
    ("net", &["net.client", "net.server"]),
    ("task", &["task.share", "task.spawn", "task.suspend"]),
];

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
    // ---- 型 ----

    pub(super) fn ty(&mut self, t: &Type) {
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
                if let Some(r) = ret.as_deref().filter(|r| !is_unit(r)) {
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
}

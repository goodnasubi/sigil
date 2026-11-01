use crate::ast::*;

use super::types::normalize_effects;
use super::{escape_str, is_unit, Printer};

impl<'a> Printer<'a> {
    // ---- ファイル ----

    pub(super) fn file(&mut self, f: &File) {
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
        for (i, u) in f.uses.iter().enumerate() {
            if i == 0 {
                self.top_comments(u.span.line, false, false);
            } else {
                let mut first = false;
                self.comments_before(u.span.line, 0, false, &mut first);
            }
            self.nl(0, u.span.line);
            self.w(&format!("use {};", u.path.join(".")));
        }
        for item in &f.items {
            match item {
                Item::Type(t) => {
                    self.top_comments(t.span.line, true, false);
                    self.nl(0, t.span.line);
                    self.w(&format!("{}type {} = ", pub_kw(t.is_pub), t.name));
                    self.ty(&t.base);
                    self.w(" where ");
                    self.expr(&t.cond, 0, false, 0);
                    self.w(";");
                }
                Item::Struct(s) => self.struct_decl(s),
                Item::Enum(e) => self.enum_decl(e),
                Item::Fn(d) => self.fn_decl(d, 0, true),
                Item::Impl(i) => self.impl_decl(i),
                Item::Trait(t) => self.trait_decl(t),
                Item::ImplFor(i) => self.impl_for_decl(i),
                Item::Global(g) => self.global_decl(g),
            }
        }
        self.top_comments(u32::MAX, false, true);
    }

    fn global_decl(&mut self, g: &GlobalDecl) {
        self.top_comments(g.span.line, true, false);
        self.nl(0, g.span.line);
        self.w("global");
        if g.is_mut {
            self.w(" mut");
        }
        self.w(" ");
        self.w(&g.name);
        self.w(": ");
        self.ty(&g.ty);
        self.w(" = ");
        self.expr(&g.init, 0, false, 0);
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
            if !g.bounds.is_empty() {
                // 性質の名前の文字の順に並べ、重複を取り除く（ADR 0011 の決定 6）。
                let mut bs: Vec<String> = g.bounds.iter().map(|b| b.join(".")).collect();
                bs.sort();
                bs.dedup();
                self.w(&format!(": {}", bs.join(" + ")));
            }
        }
        self.w(">");
    }

    fn struct_decl(&mut self, s: &StructDecl) {
        self.top_comments(s.span.line, true, false);
        self.nl(0, s.span.line);
        self.w(&format!("{}struct {}", pub_kw(s.is_pub), s.name));
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
        self.w(&format!("{}enum {}", pub_kw(e.is_pub), e.name));
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

    fn impl_decl(&mut self, i: &ImplDecl) {
        self.top_comments(i.span.line, true, false);
        self.nl(0, i.span.line);
        self.w("impl");
        if !i.generics.is_empty() {
            self.generics(&i.generics);
        }
        self.w(" ");
        self.ty(&i.ty);
        self.w(" {");
        for (k, f) in i.fns.iter().enumerate() {
            if k > 0 {
                self.blank();
            }
            self.fn_decl(f, 1, k == 0);
        }
        let mut first = i.fns.is_empty();
        self.comments_before(i.end_line, 1, false, &mut first);
        self.nl(0, i.end_line);
        self.w("}");
    }

    fn trait_decl(&mut self, t: &TraitDecl) {
        self.top_comments(t.span.line, true, false);
        self.nl(0, t.span.line);
        self.w(&format!("{}trait {}", pub_kw(t.is_pub), t.name));
        if t.fns.is_empty() && !self.pending_before(t.end_line) {
            self.w(" {}");
            return;
        }
        self.w(" {");
        for (k, f) in t.fns.iter().enumerate() {
            if k > 0 {
                self.blank();
            }
            self.fn_head(f, 1, k == 0);
            self.w(";");
        }
        let mut first = t.fns.is_empty();
        self.comments_before(t.end_line, 1, false, &mut first);
        self.nl(0, t.end_line);
        self.w("}");
    }

    fn impl_for_decl(&mut self, i: &ImplForDecl) {
        self.top_comments(i.span.line, true, false);
        self.nl(0, i.span.line);
        self.w("impl");
        if !i.generics.is_empty() {
            self.generics(&i.generics);
        }
        self.w(&format!(" {} for ", i.trait_name));
        self.ty(&i.ty);
        self.w(" {");
        for (k, f) in i.fns.iter().enumerate() {
            if k > 0 {
                self.blank();
            }
            self.fn_decl(f, 1, k == 0);
        }
        let mut first = i.fns.is_empty();
        self.comments_before(i.end_line, 1, false, &mut first);
        self.nl(0, i.end_line);
        self.w("}");
    }

    /// `ind` は宣言の字下げ（トップレベルは 0、`impl` の中は 1）。
    fn fn_decl(&mut self, d: &FnDecl, ind: usize, first: bool) {
        self.fn_head(d, ind, first);
        self.plain_start(d.body.open_line, ind);
        // 戻り値のない関数の末尾の `return;` は書かない（ADR 0008 の決定 2）。
        let drop_ret = d.ret.as_ref().is_none_or(is_unit)
            && matches!(
                d.body.stmts.last(),
                Some(Stmt {
                    kind: StmtKind::Return(None),
                    ..
                })
            );
        self.block(&d.body, ind, drop_ret);
    }

    /// シグネチャと契約節（`unchecked` まで）。本体（またはトレイトの関数の `;`）は呼び出し側が出す。
    fn fn_head(&mut self, d: &FnDecl, ind: usize, first: bool) {
        if ind == 0 {
            self.top_comments(d.span.line, true, false);
        } else {
            let mut f = first;
            self.comments_before(d.span.line, ind, false, &mut f);
        }
        self.nl(ind, d.span.line);
        self.w(&format!("{}fn {}", pub_kw(d.is_pub), d.name));
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
        // `-> Unit` は書かない（ADR 0011 の決定 2。`->` を省いた形が正規形）。
        if let Some(r) = d.ret.as_ref().filter(|r| !is_unit(r)) {
            self.w(" -> ");
            self.ty(r);
        }
        self.plain_start(d.clause_lines[clause::INTENT], ind + 1);
        self.w(&format!("intent {}", escape_str(&d.intent)));
        self.plain_start(d.clause_lines[clause::REQUIRES], ind + 1);
        self.w("requires ");
        match &d.requires {
            Some(e) => self.expr(e, 0, false, ind + 1),
            None => self.w("none"),
        }
        self.plain_start(d.clause_lines[clause::ENSURES], ind + 1);
        self.w("ensures ");
        match &d.ensures {
            Some(e) => self.expr(e, 0, false, ind + 1),
            None => self.w("none"),
        }
        self.plain_start(d.clause_lines[clause::EFFECTS], ind + 1);
        let effs = normalize_effects(&d.effects, &d.params);
        if effs.is_empty() {
            self.w("effects none");
        } else {
            self.w(&format!("effects {}", effs.join(", ")));
        }
        if let Some(u) = &d.unchecked {
            self.plain_start(d.clause_lines[clause::UNCHECKED], ind + 1);
            self.w(&format!("unchecked {}", escape_str(u)));
        }
    }
}

fn pub_kw(is_pub: bool) -> &'static str {
    if is_pub {
        "pub "
    } else {
        ""
    }
}

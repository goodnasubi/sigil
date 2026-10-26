//! 仕様の抽出（`sigil spec`。ADR 0008 の決定 7）。
//!
//! モジュールの宣言・型の宣言・関数のシグネチャと 4 節を日本語の定型文で出す。関数の本体は
//! 含めない。条件の式（`old(...)`・`result` を含む）は訳さず、正規形の表記で示す。出力は構文
//! だけから決まる（型の情報は使わない）。

use crate::ast::*;
use crate::error::{Error, Span};
use crate::fmt::{expr_lines, is_unit, normalize_effects, type_text};
use crate::json::{obj, Value};
use crate::parser::parse;

/// 仕様の構造（ADR 0019 の決定 1）。日本語の定型文（`text`）は、この構造から作る。
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub module: SpecItem,
    pub items: Vec<SpecItem>,
}

/// 仕様の 1 項目（モジュール・型・構造体・列挙型・関数・メソッド）。
#[derive(Debug, Clone, PartialEq)]
pub struct SpecItem {
    /// メソッドは `型.名前`。
    pub name: String,
    /// 宣言の始まりの位置。
    pub span: Span,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Module {
        /// 出力先の名前（`bare`・`browser`・`server`）。
        targets: Vec<String>,
        trust_base: bool,
        /// 検査を外す（`unchecked`）関数の数。
        unchecked_fns: usize,
    },
    Type {
        base: String,
        /// 条件の式の正規形の行。
        cond: Vec<String>,
    },
    Struct {
        generics: Vec<GenericSpec>,
        /// （名前, 型）
        fields: Vec<(String, String)>,
    },
    Enum {
        generics: Vec<GenericSpec>,
        /// （名前, 中身の型）
        variants: Vec<(String, Vec<String>)>,
    },
    Fn {
        method: bool,
        spec: FnSpec,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GenericSpec {
    pub name: String,
    /// 制約の性質。並べ替えて重複を除いた形。
    pub bounds: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 受け取る
    Own,
    /// 読む
    Ref,
    /// 書き換える
    Mut,
    /// 関数の値を呼ぶ
    Call,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParamSpec {
    pub name: String,
    pub mode: Mode,
    /// `Ref`・`Mut` は参照の中の型。
    pub ty: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnSpec {
    pub generics: Vec<GenericSpec>,
    /// 書かれたままの意図（定型文では必要なら `\u` でエスケープする）。
    pub intent: String,
    pub params: Vec<ParamSpec>,
    /// `Unit` と戻り値なしは `None`。
    pub returns: Option<String>,
    pub requires: Option<Vec<String>>,
    pub ensures: Option<Vec<String>>,
    /// 正規形の並び（`of(x)` を含む）。
    pub effects: Vec<String>,
    pub unchecked: Option<Unchecked>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unchecked {
    pub premise: String,
    /// 使う原始操作（名前, 前提）。検査が失敗して分からないときは `None`。
    pub raw_ops: Option<Vec<(String, String)>>,
    /// 推論できない効果。宣言に委ねる原始操作がない（行を出さない）か、検査が失敗したときは `None`。
    pub uninferred: Option<Vec<String>>,
}

impl Spec {
    /// `sigil spec` の出力。モジュールと各項目の `text` を、改行 1 つを挟んでつなぐ。
    pub fn text(&self) -> String {
        let mut blocks = vec![self.module.text()];
        blocks.extend(self.items.iter().map(SpecItem::text));
        blocks.join("\n")
    }

    /// `sigil spec --json` の値。
    pub fn to_json(&self) -> Value {
        obj(vec![
            ("format", 1i64.into()),
            ("module", self.module.to_json()),
            (
                "items",
                Value::Arr(self.items.iter().map(SpecItem::to_json).collect()),
            ),
        ])
    }
}

/// ソースから仕様のテキストを作る。構文エラーならそのエラー。
pub fn spec(src: &str) -> Result<String, Error> {
    spec_with(src, &crate::effects::Options::default())
}

/// 検査器の設定（テスト用の原始操作）を指定して仕様のテキストを作る。
pub fn spec_with(src: &str, opts: &crate::effects::Options) -> Result<String, Error> {
    Ok(extract_with(src, opts)?.text())
}

/// ソースから仕様の構造を作る。
pub fn extract(src: &str) -> Result<Spec, Error> {
    extract_with(src, &crate::effects::Options::default())
}

/// 検査器の設定（テスト用の原始操作）を指定して仕様の構造を作る。
pub fn extract_with(src: &str, opts: &crate::effects::Options) -> Result<Spec, Error> {
    let file = parse(src)?;
    // `使う原始操作:`・`推論できない効果:` は検査器の解決結果から作る。`unchecked` の関数があるときだけ検査器を呼ぶ。
    // `sigil spec` は許可を検査しないので、このモジュール自身を `trust_base` に挙げて呼ぶ（ADR 0016 の決定 6）。
    let unchecked = unchecked_count(&file.items);
    let reports = if unchecked > 0 {
        let mut o = opts.clone();
        o.trust_base.push(file.module.name.join("."));
        Some(crate::check::fn_reports(&file, &o).ok())
    } else {
        None
    };
    let reports = &reports;
    let m = &file.module;
    let module = SpecItem {
        name: m.name.join("."),
        span: m.span,
        body: Body::Module {
            targets: m.targets.clone(),
            trust_base: m.trust == Trust::Base,
            unchecked_fns: unchecked,
        },
    };
    let mut items = vec![];
    for item in &file.items {
        match item {
            Item::Type(t) => items.push(SpecItem {
                name: t.name.clone(),
                span: t.span,
                body: Body::Type {
                    base: type_text(&t.base),
                    cond: expr_lines(&t.cond),
                },
            }),
            Item::Struct(s) => items.push(SpecItem {
                name: s.name.clone(),
                span: s.span,
                body: Body::Struct {
                    generics: generic_specs(&s.generics),
                    fields: s
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), type_text(&f.ty)))
                        .collect(),
                },
            }),
            Item::Enum(e) => items.push(SpecItem {
                name: e.name.clone(),
                span: e.span,
                body: Body::Enum {
                    generics: generic_specs(&e.generics),
                    variants: e
                        .variants
                        .iter()
                        .map(|v| (v.name.clone(), v.fields.iter().map(type_text).collect()))
                        .collect(),
                },
            }),
            Item::Fn(f) => items.push(fn_item(f, false, &f.name, &f.generics, reports)),
            Item::Impl(i) => {
                for f in &i.fns {
                    let name = format!("{}.{}", type_text(&i.ty), f.name);
                    // `impl<T> 型<T>` の型引数と、メソッド自身の型引数をあわせて示す。
                    let gs: Vec<Generic> = i
                        .generics
                        .iter()
                        .chain(f.generics.iter())
                        .cloned()
                        .collect();
                    items.push(fn_item(f, true, &name, &gs, reports));
                }
            }
        }
    }
    Ok(Spec { module, items })
}

/// `unchecked` の節を持つ関数（`impl` のメソッドを含む）の数。
fn unchecked_count(items: &[Item]) -> usize {
    items
        .iter()
        .map(|i| match i {
            Item::Fn(f) => usize::from(f.unchecked.is_some()),
            Item::Impl(i) => i.fns.iter().filter(|f| f.unchecked.is_some()).count(),
            _ => 0,
        })
        .sum()
}

fn generic_specs(gs: &[Generic]) -> Vec<GenericSpec> {
    gs.iter()
        .map(|g| {
            let mut bounds: Vec<String> = g.bounds.iter().map(|b| b.join(".")).collect();
            bounds.sort();
            bounds.dedup();
            GenericSpec {
                name: g.name.clone(),
                bounds,
            }
        })
        .collect()
}

/// 検査器の記録。`None` は `unchecked` の関数がなく検査器を呼ばなかった場合、`Some(None)` は検査が失敗した場合。
type Reports = Option<Option<std::collections::HashMap<usize, crate::check::FnReport>>>;

fn param_spec(p: &Param) -> ParamSpec {
    let (mode, ty) = match &p.ty {
        Type::Ref {
            mutable: true,
            inner,
        } => (Mode::Mut, type_text(inner)),
        Type::Ref {
            mutable: false,
            inner,
        } => (Mode::Ref, type_text(inner)),
        // 関数の値の受け渡しは呼び出しの間の借用なので、受け取る（所有する）とは扱わない（ADR 0013）
        t @ Type::Fn { .. } => (Mode::Call, type_text(t)),
        t => (Mode::Own, type_text(t)),
    };
    ParamSpec {
        name: p.name.clone(),
        mode,
        ty,
    }
}

fn fn_item(f: &FnDecl, method: bool, name: &str, gs: &[Generic], reports: &Reports) -> SpecItem {
    let unchecked = f.unchecked.as_ref().map(|u| {
        let rep = match reports {
            Some(Some(m)) => m.get(&crate::check::fn_key(f)),
            _ => None,
        };
        Unchecked {
            premise: u.clone(),
            raw_ops: rep.map(|r| r.raw_ops.clone()),
            uninferred: rep.filter(|r| r.delegated).map(|r| r.not_inferred.clone()),
        }
    });
    SpecItem {
        name: name.to_string(),
        span: f.span,
        body: Body::Fn {
            method,
            spec: FnSpec {
                generics: generic_specs(gs),
                intent: f.intent.clone(),
                params: f.params.iter().map(param_spec).collect(),
                // `-> Unit` は正規形では書かない（ADR 0011 の決定 2）ので、`Unit` も戻り値なしとする。
                returns: f.ret.as_ref().filter(|r| !is_unit(r)).map(type_text),
                requires: f.requires.as_ref().map(expr_lines),
                ensures: f.ensures.as_ref().map(expr_lines),
                effects: normalize_effects(&f.effects, &f.params),
                unchecked,
            },
        },
    }
}

// ---- 日本語の定型文（`text`）。訳はここに置く。 ----

impl SpecItem {
    pub fn kind(&self) -> &'static str {
        match &self.body {
            Body::Module { .. } => "module",
            Body::Type { .. } => "type",
            Body::Struct { .. } => "struct",
            Body::Enum { .. } => "enum",
            Body::Fn { method: false, .. } => "fn",
            Body::Fn { method: true, .. } => "method",
        }
    }

    /// この項目の `sigil spec` の塊（最後に改行 1 つ、空行で終わらない）。
    pub fn text(&self) -> String {
        match &self.body {
            Body::Module {
                targets,
                trust_base,
                unchecked_fns,
            } => module_text(&self.name, targets, *trust_base, *unchecked_fns),
            Body::Type { base, cond } => format!(
                "型 {}\n  基にする型: {}\n  条件: {}\n",
                self.name,
                base,
                cond.join("\n    ")
            ),
            Body::Struct { generics, fields } => {
                let fs: Vec<String> = fields.iter().map(|(n, t)| format!("{n}（{t}）")).collect();
                format!(
                    "構造体 {}{}\n  フィールド: {}\n",
                    self.name,
                    generics_text(generics),
                    fs.join("、")
                )
            }
            Body::Enum { generics, variants } => {
                let vs: Vec<String> = variants
                    .iter()
                    .map(|(n, ts)| {
                        if ts.is_empty() {
                            n.clone()
                        } else {
                            format!("{}（{}）", n, ts.join(", "))
                        }
                    })
                    .collect();
                format!(
                    "列挙型 {}{}\n  バリアント: {}\n",
                    self.name,
                    generics_text(generics),
                    vs.join("、")
                )
            }
            Body::Fn { method, spec } => {
                fn_text(if *method { "メソッド" } else { "関数" }, &self.name, spec)
            }
        }
    }
}

fn module_text(name: &str, targets: &[String], trust_base: bool, unchecked: usize) -> String {
    let targets: Vec<&str> = targets
        .iter()
        .map(|t| match t.as_str() {
            "bare" => "実行環境なし",
            "browser" => "ブラウザ",
            "server" => "サーバー",
            other => other,
        })
        .collect();
    format!(
        "モジュール {}\n  出力先: {}\n  信頼基盤: {}\n",
        name,
        targets.join(", "),
        if trust_base {
            format!("はい（検査を外す関数 {unchecked} 個）")
        } else {
            "いいえ".to_string()
        }
    )
}

fn generics_text(gs: &[GenericSpec]) -> String {
    if gs.is_empty() {
        return String::new();
    }
    let v: Vec<String> = gs
        .iter()
        .map(|g| {
            if g.bounds.is_empty() {
                g.name.clone()
            } else {
                format!("{}: {}", g.name, g.bounds.join(" + "))
            }
        })
        .collect();
    format!("<{}>", v.join(", "))
}

/// 条件の式。複数行になる式は、2 行目以降を字下げして続ける。
fn cond_text(e: &Option<Vec<String>>) -> String {
    match e {
        None => "なし".to_string(),
        Some(lines) => lines.join("\n    "),
    }
}

/// 引数の受け渡し方の訳（ADR 0003 の規則 7。型を括弧で付記し、移動の注記は省く）。
fn param_text(p: &ParamSpec) -> String {
    let verb = match p.mode {
        Mode::Mut => "を書き換える",
        Mode::Ref => "を読む",
        Mode::Call => "を呼ぶ",
        Mode::Own => "を受け取る",
    };
    format!("{} {}（{}）", p.name, verb, p.ty)
}

/// 効果のカテゴリの訳（ADR 0004 の表）。
fn effect_ja(name: &str) -> String {
    match name {
        "console" => "コンソールに出力する",
        "fs" => "ファイルを読み書きする",
        "fs.read" => "ファイルを読む",
        "fs.write" => "ファイルに書く",
        "net" => "ネットワークに接続し、接続を待ち受ける",
        "net.client" => "ネットワークに接続する",
        "net.server" => "接続を待ち受ける",
        "db" => "データベースを読み書きする",
        "clock" => "現在時刻を読む・待つ",
        "random" => "乱数を得る",
        "env" => "環境変数・起動引数を読む",
        "process" => "他のプロセスを起動する・自分を終了する",
        "dom" => "画面（DOM）を読む・書き換える",
        "browser.storage" => "ブラウザの保存領域を読み書きする",
        "state" => "共有所有の型を通して値を書き換える",
        "foreign" => "外部から渡された関数を呼ぶ",
        "task" => "タスクを起動する・中断しうる・値を共有する",
        "task.spawn" => "並行に動くタスクを起動する",
        "task.suspend" => "中断しうる",
        "task.share" => "他のタスクと共有する値を読み書きする",
        other => return other.to_string(),
    }
    .to_string()
}

fn effects_text(effects: &[String]) -> String {
    if effects.is_empty() {
        return "なし".to_string();
    }
    let v: Vec<String> = effects
        .iter()
        .map(
            |n| match n.strip_prefix("of(").and_then(|r| r.strip_suffix(')')) {
                Some(arg) => format!("{arg} の効果"),
                None => effect_ja(n),
            },
        )
        .collect();
    v.join("、")
}

fn intent_text(s: &str) -> String {
    if s.chars().any(crate::fmt::needs_u_escape) {
        let e = crate::fmt::escape_str(s);
        e[1..e.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

const CHECK_FAILED: &str = "（検査のエラーのため表示できない）";

/// `unchecked` の関数の、検査を外す前提・使う原始操作・推論できない効果の行。
fn unchecked_text(u: &Unchecked) -> String {
    let mut out = format!("  検査を外す: {}\n", intent_text(&u.premise));
    let Some(raw_ops) = &u.raw_ops else {
        out.push_str(&format!("  使う原始操作: {CHECK_FAILED}\n"));
        out.push_str(&format!("  推論できない効果: {CHECK_FAILED}\n"));
        return out;
    };
    let ops = if raw_ops.is_empty() {
        "なし".to_string()
    } else {
        raw_ops
            .iter()
            .map(|(n, p)| format!("{n}（前提: {}）", intent_text(p)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    out.push_str(&format!("  使う原始操作: {ops}\n"));
    if let Some(not_inferred) = &u.uninferred {
        let ne = if not_inferred.is_empty() {
            "なし".to_string()
        } else {
            not_inferred.join(", ")
        };
        out.push_str(&format!("  推論できない効果: {ne}\n"));
    }
    out
}

fn fn_text(label: &str, name: &str, f: &FnSpec) -> String {
    let params = if f.params.is_empty() {
        "なし".to_string()
    } else {
        f.params
            .iter()
            .map(param_text)
            .collect::<Vec<_>>()
            .join("、")
    };
    let ret = f.returns.clone().unwrap_or_else(|| "なし".to_string());
    let unchecked = f.unchecked.as_ref().map_or(String::new(), unchecked_text);
    format!(
        "{} {}{}\n  意図: {}\n  引数: {}\n  戻り値: {}\n  事前条件: {}\n  事後条件: {}\n  効果: {}\n{}",
        label,
        name,
        generics_text(&f.generics),
        intent_text(&f.intent),
        params,
        ret,
        cond_text(&f.requires),
        cond_text(&f.ensures),
        effects_text(&f.effects),
        unchecked
    )
}

// ---- JSON（`sigil spec --json`。ADR 0019 の決定 1） ----

fn generics_json(gs: &[GenericSpec]) -> Value {
    Value::Arr(
        gs.iter()
            .map(|g| {
                obj(vec![
                    ("name", g.name.clone().into()),
                    ("bounds", g.bounds.clone().into()),
                ])
            })
            .collect(),
    )
}

fn cond_json(c: &Option<Vec<String>>) -> Value {
    c.as_ref().map(|l| l.join("\n")).into()
}

impl SpecItem {
    pub fn to_json(&self) -> Value {
        let mut fields: Vec<(&str, Value)> = vec![
            ("kind", self.kind().into()),
            ("name", self.name.clone().into()),
            (
                "span",
                obj(vec![
                    ("line", self.span.line.into()),
                    ("col", self.span.col.into()),
                ]),
            ),
            ("text", self.text().into()),
        ];
        match &self.body {
            Body::Module {
                targets,
                trust_base,
                unchecked_fns,
            } => {
                fields.push(("targets", targets.clone().into()));
                fields.push(("trust_base", (*trust_base).into()));
                fields.push((
                    "unchecked_fns",
                    i64::try_from(*unchecked_fns).unwrap_or(i64::MAX).into(),
                ));
            }
            Body::Type { base, cond } => {
                fields.push(("base", base.clone().into()));
                fields.push(("cond", cond.join("\n").into()));
            }
            Body::Struct {
                generics,
                fields: fs,
            } => {
                fields.push(("generics", generics_json(generics)));
                fields.push((
                    "fields",
                    Value::Arr(
                        fs.iter()
                            .map(|(n, t)| {
                                obj(vec![("name", n.clone().into()), ("type", t.clone().into())])
                            })
                            .collect(),
                    ),
                ));
            }
            Body::Enum { generics, variants } => {
                fields.push(("generics", generics_json(generics)));
                fields.push((
                    "variants",
                    Value::Arr(
                        variants
                            .iter()
                            .map(|(n, ts)| {
                                obj(vec![
                                    ("name", n.clone().into()),
                                    ("fields", ts.clone().into()),
                                ])
                            })
                            .collect(),
                    ),
                ));
            }
            Body::Fn { spec: f, .. } => {
                fields.push(("intent", f.intent.clone().into()));
                fields.push((
                    "params",
                    Value::Arr(
                        f.params
                            .iter()
                            .map(|p| {
                                let mode = match p.mode {
                                    Mode::Own => "own",
                                    Mode::Ref => "ref",
                                    Mode::Mut => "mut",
                                    Mode::Call => "call",
                                };
                                obj(vec![
                                    ("name", p.name.clone().into()),
                                    ("mode", mode.into()),
                                    ("type", p.ty.clone().into()),
                                ])
                            })
                            .collect(),
                    ),
                ));
                fields.push(("returns", f.returns.clone().into()));
                fields.push(("requires", cond_json(&f.requires)));
                fields.push(("ensures", cond_json(&f.ensures)));
                fields.push(("effects", f.effects.clone().into()));
                fields.push(("generics", generics_json(&f.generics)));
                let u = f.unchecked.as_ref();
                fields.push(("unchecked", u.map(|u| u.premise.clone()).into()));
                let raw_ops = u.and_then(|u| u.raw_ops.as_ref()).map_or(vec![], |ops| {
                    ops.iter()
                        .map(|(n, p)| {
                            obj(vec![
                                ("name", n.clone().into()),
                                ("premise", p.clone().into()),
                            ])
                        })
                        .collect()
                });
                fields.push(("raw_ops", Value::Arr(raw_ops)));
                let unin = u.and_then(|u| u.uninferred.clone()).unwrap_or_default();
                fields.push(("uninferred_effects", unin.into()));
                // 検査が失敗して `raw_ops`・`uninferred_effects` が分からないとき真（空の配列と区別する）
                fields.push((
                    "unchecked_unresolved",
                    u.is_some_and(|u| u.raw_ops.is_none()).into(),
                ));
            }
        }
        obj(fields)
    }
}

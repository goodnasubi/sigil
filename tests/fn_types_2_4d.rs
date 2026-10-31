//! ADR 0022 の PR 2-4d（1/3）: 効果つき関数の型の AST・構文解析・整形・仕様の表示。
//! 位置による必須・禁止の検査（`FnTypePosition`）・型検査・実行は 2/3・3/3 で対応する。
//! ここでは位置を問わず「書かれた効果節を AST に保持し、整形と仕様に出す」ことだけを見る。
//! 期待値は決め打ち（エラーの種類と位置）。

use sigil::ast::{Effect, Item, StmtKind, Type};
use sigil::{fmt, parse, ErrorKind};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

/// 置ける各位置（フィールド・列挙型の値の中身・戻り値・`let` の型注釈）に効果つき関数の型を
/// 書いた正規形。引数の位置は従来形（効果節なし）のまま。
/// 効果は正規形の並び（名前の順）なので `console, db`。
const SRC: &str = "module m.fn_types\n  targets server\n  trust none\n\nstruct Request {}\n\nstruct Response {\n  status: Int,\n}\n\nstruct Route {\n  handler: fn(Request) -> Response effects console, db,\n  path: String,\n}\n\nenum Handler {\n  Db(fn(Request) -> Response effects db),\n  Pair(fn(Int) -> Int effects console, db, Int),\n  Pure(fn(Int) -> Int effects none),\n}\n\nfn handler_impl(req: Request) -> Response\n  intent \"要求に応える\"\n  requires none\n  ensures none\n  effects db\n{\n  return Response { status: 200 };\n}\n\nfn make_handler() -> fn(Request) -> Response effects db\n  intent \"ハンドラを作る\"\n  requires none\n  ensures none\n  effects none\n{\n  return handler_impl;\n}\n\nfn apply_twice(f: fn(Int) -> Int, x: Int) -> Int\n  intent \"f を 2 回適用する\"\n  requires none\n  ensures none\n  effects of(f)\n{\n  return f(f(x));\n}\n\nfn use_let() -> Int\n  intent \"let の型注釈に書く\"\n  requires none\n  ensures none\n  effects none\n{\n  let h: fn(Request) -> Response effects db = handler_impl;\n  return 0;\n}\n";

fn err_at(src: &str) -> (ErrorKind, u32, u32) {
    match parse(src) {
        Ok(_) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => (e.kind, e.span.line, e.span.col),
    }
}

/// 関数の型の効果節を取り出す（関数の型でなければ失敗）。
fn fn_effects(t: &Type) -> &Option<Vec<Effect>> {
    let Type::Fn { effects, .. } = t else {
        panic!("関数の型のはず: {t:?}")
    };
    effects
}

/// カテゴリの並び（書かれた順）にする。
fn cats(es: &[Effect]) -> Vec<Vec<&str>> {
    es.iter()
        .map(|e| match e {
            Effect::Category(p) => p.iter().map(String::as_str).collect(),
            Effect::Of(n) => panic!("関数の型の効果に `of({n})` が入っている"),
        })
        .collect()
}

// ---- 構文 ----

#[test]
fn storable_positions_parse_and_keep_effects() {
    let f = parse(SRC).unwrap();

    // 構造体のフィールド。効果の後に続くフィールド（`path`）を効果と取り違えない。
    let Item::Struct(route) = &f.items[2] else {
        panic!("3 つ目は構造体 Route のはず")
    };
    assert_eq!(route.fields.len(), 2);
    assert_eq!(route.fields[1].name, "path");
    assert_eq!(
        cats(fn_effects(&route.fields[0].ty).as_ref().unwrap()),
        [vec!["console"], vec!["db"]]
    );

    // 列挙型の値の中身。`Pair` は効果の後に別の型（`Int`）が続く。
    let Item::Enum(h) = &f.items[3] else {
        panic!("4 つ目は列挙型 Handler のはず")
    };
    assert_eq!(
        cats(fn_effects(&h.variants[0].fields[0]).as_ref().unwrap()),
        [vec!["db"]]
    );
    assert_eq!(h.variants[1].fields.len(), 2);
    assert_eq!(
        cats(fn_effects(&h.variants[1].fields[0]).as_ref().unwrap()),
        [vec!["console"], vec!["db"]]
    );
    assert!(
        matches!(&h.variants[1].fields[1], Type::Path { path, .. } if path == &["Int".to_string()])
    );
    // `effects none` は「書かれていない」（`None`）と区別できる（空の `Some`）。
    assert_eq!(fn_effects(&h.variants[2].fields[0]), &Some(vec![]));

    // 関数・メソッドの戻り値。
    let Item::Fn(mk) = &f.items[5] else {
        panic!("6 つ目は関数 make_handler のはず")
    };
    let ret = mk.ret.as_ref().unwrap();
    assert_eq!(cats(fn_effects(ret).as_ref().unwrap()), [vec!["db"]]);

    // `let` の型注釈。
    let Item::Fn(ul) = &f.items[7] else {
        panic!("8 つ目は関数 use_let のはず")
    };
    let StmtKind::Let { ty, .. } = &ul.body.stmts[0].kind else {
        panic!("最初の文は let のはず")
    };
    assert_eq!(
        cats(fn_effects(ty.as_ref().unwrap()).as_ref().unwrap()),
        [vec!["db"]]
    );
}

#[test]
fn arg_position_keeps_legacy_form() {
    let f = parse(SRC).unwrap();
    // 引数の位置の関数の型は今のまま効果を書かない（`of(f)` で引き継ぐ）。効果節は `None`。
    let Item::Fn(at) = &f.items[6] else {
        panic!("7 つ目は関数 apply_twice のはず")
    };
    assert_eq!(fn_effects(&at.params[0].ty), &None);
    // 宣言の効果節の `of(f)` は従来どおり。
    assert_eq!(at.effects, [Effect::Of("f".to_string())]);
}

#[test]
fn dotted_category_and_generic_arg_position_parse() {
    // ドットつきカテゴリ。
    let src = format!("{HEAD}struct S {{\n  h: fn(Int) -> Int effects fs.read,\n}}\n");
    let f = parse(&src).unwrap();
    let Item::Struct(s) = &f.items[0] else {
        panic!("構造体のはず")
    };
    assert_eq!(
        cats(fn_effects(&s.fields[0].ty).as_ref().unwrap()),
        [vec!["fs", "read"]]
    );

    // 型引数の位置は、この段階では構文として読み、保持する（置けないことの検査は 2/3）。
    let src = format!("{HEAD}struct S {{\n  hs: List<fn(Int) -> Int effects none>,\n}}\n");
    let f = parse(&src).unwrap();
    let Item::Struct(s) = &f.items[0] else {
        panic!("構造体のはず")
    };
    let Type::Path { args, .. } = &s.fields[0].ty else {
        panic!("List<...> のはず")
    };
    assert_eq!(fn_effects(&args[0]), &Some(vec![]));
}

#[test]
fn of_in_fn_type_effects_is_a_syntax_error() {
    // 効果の先頭に `of(...)`（6 行目、29 桁目）。
    let src = format!("{HEAD}struct S {{\n  h: fn(Int) -> Int effects of(f),\n}}\n");
    assert_eq!(err_at(&src), (ErrorKind::OfInFnType, 6, 29));

    // カンマの後に `of(...)`（6 行目、33 桁目）。区切りのカンマと取り違えて通さない。
    let src = format!("{HEAD}struct S {{\n  h: fn(Int) -> Int effects db, of(f),\n}}\n");
    assert_eq!(err_at(&src), (ErrorKind::OfInFnType, 6, 33));

    // 戻り値の位置でも同じ（5 行目、34 桁目）。
    let src = format!(
        "{HEAD}fn g() -> fn(Int) -> Int effects of(f)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return handler_impl;\n}}\n"
    );
    assert_eq!(err_at(&src), (ErrorKind::OfInFnType, 5, 34));
}

// ---- 整形 ----

#[test]
fn formats_round_trip() {
    let out = fmt::format(SRC).unwrap();
    assert_eq!(out, SRC);
    // 冪等
    assert_eq!(fmt::format(&out).unwrap(), out);
    // 効果の並びは関数の節と同じ正規形（名前の順）に直る。
    let shuffled = SRC
        .replace(
            "handler: fn(Request) -> Response effects console, db,",
            "handler: fn(Request) -> Response effects db, console,",
        )
        .replace(
            "Pair(fn(Int) -> Int effects console, db, Int)",
            "Pair(fn(Int) -> Int effects db, console, Int)",
        );
    assert_eq!(fmt::format(&shuffled).unwrap(), SRC);
}

// ---- 仕様の表示 ----

#[test]
fn spec_text_shows_fn_type_effects() {
    let out = sigil::spec::spec(SRC).unwrap();
    // 構造体のフィールド。
    assert!(
        out.contains(
            "フィールド: handler（fn(Request) -> Response effects console, db）、path（String）"
        ),
        "{out}"
    );
    // 列挙型の値の中身。
    assert!(
        out.contains(
            "バリアント: Db（fn(Request) -> Response effects db）、Pair（fn(Int) -> Int effects console, db, Int）、Pure（fn(Int) -> Int effects none）"
        ),
        "{out}"
    );
    // 戻り値。
    assert!(
        out.contains("戻り値: fn(Request) -> Response effects db"),
        "{out}"
    );
    // 引数の位置の従来形は従来どおり（効果節は付かない）。
    assert!(
        out.contains("引数: f を呼ぶ（fn(Int) -> Int）、x を受け取る（Int）"),
        "{out}"
    );
}

#[test]
fn spec_json_shows_fn_type_effects() {
    let sp = sigil::spec::extract(SRC).unwrap();
    let v = sp.to_json();
    let items = v.get("items").unwrap().as_array().unwrap();

    // 構造体のフィールドの型（文字列の欄に効果節が入る。ADR 0019 の型の表記の流儀）。
    let route = &items[2];
    assert_eq!(route.get("kind").unwrap().as_str(), Some("struct"));
    let fs = route.get("fields").unwrap().as_array().unwrap();
    assert_eq!(
        fs[0].get("type").unwrap().as_str(),
        Some("fn(Request) -> Response effects console, db")
    );

    // 列挙型の値の中身の型。
    let handler = &items[3];
    let vs = handler.get("variants").unwrap().as_array().unwrap();
    let pair_fields = vs[1].get("fields").unwrap().as_array().unwrap();
    assert_eq!(
        pair_fields[0].as_str(),
        Some("fn(Int) -> Int effects console, db")
    );
    let pure_fields = vs[2].get("fields").unwrap().as_array().unwrap();
    assert_eq!(pure_fields[0].as_str(), Some("fn(Int) -> Int effects none"));

    // 戻り値の型。
    let mk = &items[5];
    assert_eq!(mk.get("name").unwrap().as_str(), Some("make_handler"));
    assert_eq!(
        mk.get("returns").unwrap().as_str(),
        Some("fn(Request) -> Response effects db")
    );

    // 引数の位置の従来形: 型は従来どおり、宣言の効果は `of(f)` のまま。
    let at = &items[6];
    let ps = at.get("params").unwrap().as_array().unwrap();
    assert_eq!(ps[0].get("mode").unwrap().as_str(), Some("call"));
    assert_eq!(ps[0].get("type").unwrap().as_str(), Some("fn(Int) -> Int"));
    let effs = at.get("effects").unwrap().as_array().unwrap();
    assert_eq!(effs.len(), 1);
    assert_eq!(effs[0].as_str(), Some("of(f)"));
}

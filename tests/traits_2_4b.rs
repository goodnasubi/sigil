//! ADR 0022 の PR 2-4b: `trait` 宣言と `impl トレイト for 型` の構文・整形・仕様の表示。
//! 検査と実行は 2-4c で対応した（規則の試験は traits_2_4c.rs）。
//! 期待値は決め打ち（エラーの種類と位置）。

mod common;

use sigil::ast::{Item, Type};
use sigil::{fmt, parse, ErrorKind};

/// ADR 0022 決定 2 の例（Handler/Health）に、参照される型を足した正規形。
/// 効果は正規形の並び（名前の順）なので `console, db`。
const SRC: &str = "module m.traits\n  targets server\n  trust none\n\nstruct Request {}\n\nstruct Response {\n  status: Int,\n}\n\nstruct Health {}\n\npub trait Handler {\n  fn handle(self: &Self, req: Request) -> Response\n    intent \"要求に応える\"\n    requires none\n    ensures none\n    effects console, db;\n\n  fn name() -> String\n    intent \"ハンドラの名前を返す\"\n    requires none\n    ensures none\n    effects none;\n}\n\nimpl Handler for Health {\n  fn handle(self: &Health, req: Request) -> Response\n    intent \"常に 200 を返す\"\n    requires none\n    ensures result.status == 200\n    effects none\n  {\n    return Response.ok();\n  }\n\n  fn name() -> String\n    intent \"ハンドラの名前を返す\"\n    requires none\n    ensures none\n    effects none\n  {\n    return \"health\";\n  }\n}\n";

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn err_at(src: &str) -> (ErrorKind, u32, u32) {
    match parse(src) {
        Ok(_) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => (e.kind, e.span.line, e.span.col),
    }
}

// ---- 構文 ----

#[test]
fn adr0022_example_parses() {
    let f = parse(SRC).unwrap();
    let kinds: Vec<&str> = f
        .items
        .iter()
        .map(|i| match i {
            Item::Struct(_) => "struct",
            Item::Trait(_) => "trait",
            Item::ImplFor(_) => "implfor",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["struct", "struct", "struct", "trait", "implfor"]);

    let Item::Trait(t) = &f.items[3] else {
        panic!("4 つ目はトレイトのはず")
    };
    assert!(t.is_pub);
    assert_eq!(t.name, "Handler");
    assert_eq!(t.fns.len(), 2);
    // トレイトの関数は本体を持たない。
    assert!(t.fns.iter().all(|f| f.body.stmts.is_empty()));
    // `self: &Self` の型は `&Self` のまま（整形器も書き換えない）。
    let Type::Ref { inner, mutable } = &t.fns[0].params[0].ty else {
        panic!("&Self のはず: {:?}", t.fns[0].params[0].ty)
    };
    assert!(!mutable);
    let Type::Path { path, args } = &**inner else {
        panic!("Self のはず")
    };
    assert_eq!(path, &["Self".to_string()]);
    assert!(args.is_empty());

    let Item::ImplFor(i) = &f.items[4] else {
        panic!("5 つ目は impl-for のはず")
    };
    assert_eq!(i.trait_name, "Handler");
    assert!(matches!(&i.ty, Type::Path { path, .. } if path == &["Health".to_string()]));
    assert_eq!(i.fns.len(), 2);
    // `impl` の中では `Self` でなく実装する型を書く（ADR 0022 決定 2）。
    let Type::Ref { inner, .. } = &i.fns[0].params[0].ty else {
        panic!("&Health のはず")
    };
    assert!(matches!(&**inner, Type::Path { path, .. } if path == &["Health".to_string()]));
}

#[test]
fn generic_impl_for_parses_and_formats() {
    // `impl<T> トレイト for 型<T>`（ADR 0022 決定 2 の一意の規則）。
    let src = "module m.t\n  targets server\n  trust none\n\nstruct Tree<T> {\n  v: T,\n}\n\ntrait Show {\n  fn show(self: &Self) -> String\n    intent \"見せる\"\n    requires none\n    ensures none\n    effects none;\n}\n\nimpl<T> Show for Tree<T> {\n  fn show(self: &Tree<T>) -> String\n    intent \"見せる\"\n    requires none\n    ensures none\n    effects none\n  {\n    return \"tree\";\n  }\n}\n";
    let f = parse(src).unwrap();
    let Item::ImplFor(i) = &f.items[2] else {
        panic!("impl-for のはず")
    };
    assert_eq!(i.generics.len(), 1);
    assert_eq!(i.trait_name, "Show");
    assert_eq!(fmt::format(src).unwrap(), src);
}

#[test]
fn self_outside_trait_is_an_error() {
    // 通常の関数の引数（5 行目、`Self` は 9 桁目）
    let src = format!(
        "{HEAD}fn f(x: Self) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 0;\n}}\n"
    );
    assert_eq!(err_at(&src), (ErrorKind::SelfOutsideTrait, 5, 9));

    // 構造体のフィールド（6 行目、6 桁目）
    let src = format!("{HEAD}struct S {{\n  x: Self,\n}}\n");
    assert_eq!(err_at(&src), (ErrorKind::SelfOutsideTrait, 6, 6));

    // `impl トレイト for 型` の引数（`impl` の中では実装する型を書く。14 行目、15 桁目）
    let src = "module m.a\n  targets server\n  trust none\n\ntrait Tr {\n  fn f(self: &Self)\n    intent \"x\"\n    requires none\n    ensures none\n    effects none;\n}\n\nimpl Tr for Health {\n  fn f(self: &Self)\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {\n  }\n}\n";
    assert_eq!(err_at(src), (ErrorKind::SelfOutsideTrait, 14, 15));

    // `Self` は予約名なので、型として定義できない（5 行目）
    let src = format!("{HEAD}struct Self {{}}\n");
    assert_eq!(err_at(&src).0, ErrorKind::ReservedNameDefined);
    assert_eq!(err_at(&src).1, 5);
}

#[test]
fn trait_syntax_rejections() {
    // トレイトの関数に本体を付けるとエラー（`effects` の後は `;`）
    let src = format!(
        "{HEAD}trait Tr {{\n  fn f() -> Int\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {{\n    return 1;\n  }}\n}}\n"
    );
    assert_eq!(err_at(&src).0, ErrorKind::Unexpected);

    // トレイトの関数に型引数は書けない
    let src = format!(
        "{HEAD}trait Tr {{\n  fn f<T>(x: T) -> T\n    intent \"x\"\n    requires none\n    ensures none\n    effects none;\n}}\n"
    );
    assert_eq!(err_at(&src).0, ErrorKind::NotInP0);

    // トレイトの関数に `unchecked` は書けない（本体のある関数だけ）
    let src = format!(
        "{HEAD}trait Tr {{\n  fn f()\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n    unchecked \"前提\"\n  ;\n}}\n"
    );
    assert_eq!(err_at(&src).0, ErrorKind::NotInP0);

    // `impl` のトレイトは修飾しない名前 1 つ（型引数も付けない）
    let src = format!(
        "{HEAD}trait Tr {{\n  fn f(self: &Self)\n    intent \"x\"\n    requires none\n    ensures none\n    effects none;\n}}\n\nimpl m.Tr for Health {{\n  fn f(self: &Health)\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {{\n  }}\n}}\n"
    );
    assert_eq!(err_at(&src).0, ErrorKind::Unexpected);
}

// ---- 整形 ----

#[test]
fn adr0022_example_formats_to_itself() {
    let out = fmt::format(SRC).unwrap();
    assert_eq!(out, SRC);
    // 冪等
    assert_eq!(fmt::format(&out).unwrap(), out);
    // ADR の例の書き方（効果の順が違う）からも正規形になる
    let adr_order = SRC.replace("effects console, db;", "effects db, console;");
    assert_eq!(fmt::format(&adr_order).unwrap(), SRC);
}

// ---- 仕様の表示 ----

#[test]
fn spec_shows_trait_and_impl_for() {
    let out = sigil::spec::spec(SRC).unwrap();
    let trait_block = "トレイト Handler（公開）\n  関数 handle\n    意図: 要求に応える\n    引数: self を読む（Self）、req を受け取る（Request）\n    戻り値: Response\n    事前条件: なし\n    事後条件: なし\n    効果: コンソールに出力する、データベースを読み書きする\n  関数 name\n    意図: ハンドラの名前を返す\n    引数: なし\n    戻り値: String\n    事前条件: なし\n    事後条件: なし\n    効果: なし\n";
    assert!(out.contains(trait_block), "{out}");
    // 実装の関数は、その型の項目の下に、どのトレイトの実装かを添えて示す。
    let method_block = "メソッド Health.handle（非公開）\n  トレイト: Handler\n  意図: 常に 200 を返す\n  引数: self を読む（Health）、req を受け取る（Request）\n  戻り値: Response\n  事前条件: なし\n  事後条件: result.status == 200\n  効果: なし\n";
    assert!(out.contains(method_block), "{out}");
}

#[test]
fn spec_json_has_trait_kind_and_trait_field() {
    let sp = sigil::spec::extract(SRC).unwrap();
    let kinds: Vec<&str> = sp.items.iter().map(|i| i.kind()).collect();
    assert_eq!(
        kinds,
        ["struct", "struct", "struct", "trait", "method", "method"]
    );
    let v = sp.to_json();
    assert_eq!(v.get("format").unwrap().as_i64(), Some(1));
    let items = v.get("items").unwrap().as_array().unwrap();
    assert_eq!(items.len(), 6);

    let tr = &items[3];
    assert_eq!(tr.get("kind").unwrap().as_str(), Some("trait"));
    assert_eq!(tr.get("name").unwrap().as_str(), Some("Handler"));
    assert_eq!(tr.get("visibility").unwrap().as_str(), Some("pub"));
    let fns = tr.get("fns").unwrap().as_array().unwrap();
    assert_eq!(fns.len(), 2);
    assert_eq!(fns[0].get("name").unwrap().as_str(), Some("handle"));
    let p0 = &fns[0].get("params").unwrap().as_array().unwrap()[0];
    assert_eq!(p0.get("name").unwrap().as_str(), Some("self"));
    assert_eq!(p0.get("mode").unwrap().as_str(), Some("ref"));
    assert_eq!(p0.get("type").unwrap().as_str(), Some("Self"));
    assert_eq!(fns[1].get("name").unwrap().as_str(), Some("name"));

    // メソッドには実装するトレイトの名前が入る。通常の項目には入らない。
    let m = &items[4];
    assert_eq!(m.get("kind").unwrap().as_str(), Some("method"));
    assert_eq!(m.get("trait").unwrap().as_str(), Some("Handler"));
    assert!(items[0].get("trait").is_none());
    // テキストと JSON の `text` は連結の規則で一致する（m6_spec_json と同じ規則）。
    let mut blocks = vec![v
        .get("module")
        .unwrap()
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()];
    for it in items {
        blocks.push(it.get("text").unwrap().as_str().unwrap());
    }
    assert_eq!(blocks.join("\n"), sigil::spec::spec(SRC).unwrap());
}

// ---- 検査系（2-4c で検査・実行に対応。規則の詳細は traits_2_4c.rs）----

#[test]
fn check_accepts_trait_program() {
    // 2-4b では `Unsupported` で止まったプログラムが、2-4c では検査を通る
    // （`Response.ok()` は実在しない関数なので、構造体の構築に置き換えた形で確かめる）。
    let src = SRC.replace("Response.ok()", "Response { status: 200 }");
    sigil::check::check(&src).unwrap();
    // トレイトを使わないプログラムの挙動は変えない
    let plain = format!(
        "{HEAD}fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return x;\n}}\n"
    );
    sigil::check::check(&plain).unwrap();
}

fn bin(args: &[&str]) -> (i32, String, String) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(args)
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn cli_check_run_build_accept_trait_program() {
    // 2-4c: 検査・実行・ネイティブ化が通る（入口はトレイトのメソッドを呼ぶ）。
    let body = SRC
        .replace("Response.ok()", "Response { status: 200 }")
        .replace("struct Request {}", "struct Request {\n  id: Int,\n}")
        .replace("struct Health {}", "struct Health {\n  up: Int,\n}");
    let src = format!(
        "{}\nfn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{{\n  let h = Health {{ up: 1 }};\n  let r = h.handle(Request {{ id: 7 }});\n  return r.status;\n}}\n",
        body.trim_end()
    );
    let (root, file) = common::stage_src("traits-gate", &src);
    let root_s = root.to_str().unwrap();
    let file_s = file.to_str().unwrap();

    let (c, _, err) = bin(&["check", "--root", root_s, file_s]);
    assert_eq!(c, 0, "{err}");

    let (c, out_s, err) = bin(&["run", "--root", root_s, file_s, "main"]);
    assert_eq!(c, 0, "{err}");
    assert_eq!(out_s.trim(), "200");

    let out = root.join("a.out");
    let (c, _, err) = bin(&[
        "build",
        "--root",
        root_s,
        file_s,
        "main",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert_eq!(c, 0, "{err}");
    let o2 = std::process::Command::new(&out).output().unwrap();
    assert_eq!(o2.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&o2.stdout).trim(), "200");

    // spec は止まらない（2-4b の範囲）
    let (c, out, err) = bin(&["spec", "--root", root_s, file_s]);
    assert_eq!(c, 0, "{err}");
    assert!(out.contains("トレイト Handler（公開）"), "{out}");

    let _ = std::fs::remove_dir_all(&root);
}

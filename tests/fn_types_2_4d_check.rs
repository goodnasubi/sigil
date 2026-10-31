//! ADR 0022 の PR 2-4d（2/3）: 関数の型をフィールド・戻り値に置く型検査・効果検査・
//! 所有権/性質検査（決定 3）。
//! 置ける位置（構造体のフィールド・列挙型の値の中身・戻り値・`let` の型注釈）では効果節が
//! 必須で、入れる値の効果 E は宣言 D 以下（⊑）でなければならない。入れられるのは
//! 名前の関数と `move` クロージャだけで、とらえた値はすべて `Sendable`・`Shareable`。
//! 期待値は決め打ち（エラーの種類と位置）。位置は、その値を含む文または宣言の位置。

use sigil::check::check_with;
use sigil::effects::{EffectTable, Options, Primitive};
use sigil::ErrorKind;

const HEAD: &str = "module m.t\n  targets server\n  trust none\n\n";

fn prim(sig: &str, effects: &[&str]) -> Primitive {
    Primitive {
        signature: sig.to_string(),
        effects: effects.iter().map(|e| e.to_string()).collect(),
    }
}

/// `db`・`fs` の効果を持つ仮の原始操作を足した検査の選択肢。
fn opts() -> Options {
    Options {
        copy_override: vec![],
        trust_base: vec![],
        raw_ops: vec![],
        table: EffectTable::standard(),
        primitives: vec![
            prim("run_query(q: String) -> Int", &["db"]),
            prim("read_some(p: String) -> Int", &["fs.read"]),
            prim("read_any(p: String) -> Int", &["fs"]),
        ],
    }
}

fn ok(src: &str) {
    if let Err(e) = check_with(src, &opts()) {
        panic!("検査が通るはずが失敗した: {e}\n{src}");
    }
}

fn err_at(src: &str) -> (ErrorKind, u32, u32) {
    match check_with(src, &opts()) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => (e.kind, e.span.line, e.span.col),
    }
}

/// ハンドラ表の共通部分（5〜36 行目。次の項目は 37 行目から）。
const DEFS: &str = "struct Request {\n  id: Int,\n}\n\nstruct Response {\n  status: Int,\n}\n\nstruct Route {\n  handler: fn(Request) -> Response effects db,\n  path: String,\n}\n\nfn handler_impl(req: Request) -> Response\n  intent \"x\"\n  requires none\n  ensures none\n  effects db\n{\n  let n = run_query(\"q\");\n  return Response { status: n };\n}\n\nfn idle_handler(req: Request) -> Response\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Response { status: 204 };\n}\n\n";

fn prog(items: &str) -> String {
    format!("{HEAD}{DEFS}{items}")
}

fn prog_bare(items: &str) -> String {
    format!("{HEAD}{items}")
}

/// 4 節つきの関数 1 つ。
fn func(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

/// `fn(Int) -> Int effects none` のフィールドを持つ構造体（5〜7 行目）。
const FNBOX: &str = "struct FnBox {\n  f: fn(Int) -> Int effects none,\n}\n\n";

// ---- 成功: ハンドラ表（フィールドへの代入とフィールドの関数の呼び出し） ----

#[test]
fn handler_table_and_field_call_ok() {
    let src = prog(&format!(
        "{}{}{}",
        func(
            "serve() -> Response",
            "db",
            "  let r = Route { handler: handler_impl, path: \"/a\" };\n  return r.handler(Request { id: 1 });"
        ),
        // 効果の下位の関数（E = none）を D = db のフィールドに入れるのは ⊑ で通る
        func(
            "serve_idle() -> Response",
            "db",
            "  let r = Route { handler: idle_handler, path: \"/b\" };\n  return r.handler(Request { id: 2 });"
        ),
        // メソッドの受け手からも呼べる
        "impl Route {\n  fn serve(self: &Route, req: Request) -> Response\n    intent \"x\"\n    requires none\n    ensures none\n    effects db\n  {\n    return self.handler(req);\n  }\n}\n"
    ));
    ok(&src);
}

#[test]
fn handler_list_ok() {
    // ADR 0022 決定 3 の「構造体の一覧からハンドラを選んで呼ぶ事例」の検査まで
    // （評価器・ネイティブでの一致は 3/3）。
    let src = prog(&format!(
        "{}{}",
        func(
            "routes() -> List<Route>",
            "none",
            "  let mut rs = List.new();\n  rs.push(Route { handler: handler_impl, path: \"/a\" });\n  rs.push(Route { handler: idle_handler, path: \"/b\" });\n  return rs;"
        ),
        func(
            "call_first(routes: &List<Route>, req: Request) -> Response",
            "db",
            "  match routes.get(0) {\n    Some(r) => {\n      return r.handler(req);\n    }\n    None => {\n      return Response { status: 404 };\n    }\n  }"
        )
    ));
    ok(&src);
}

// ---- 成功: `of(f)` への D の置き換え ----

#[test]
fn of_substitution_ok() {
    let src = prog(&format!(
        "{}{}",
        func(
            "run_with(f: fn(Request) -> Response) -> Response",
            "of(f)",
            "  return f(Request { id: 1 });"
        ),
        // フィールドの関数（効果 D = db）を渡すと、この呼び出しの `of(f)` は db になる
        func(
            "via_field() -> Response",
            "db",
            "  let r = Route { handler: handler_impl, path: \"/a\" };\n  return run_with(r.handler);"
        )
    ));
    ok(&src);
}

#[test]
fn of_substitution_undeclared_is_effect_missing() {
    let src = prog(&format!(
        "{}{}",
        func(
            "run_with(f: fn(Request) -> Response) -> Response",
            "of(f)",
            "  return f(Request { id: 1 });"
        ),
        // 呼んだ側が db を宣言していない（run_with は 37〜44 行目、via_field は 45 行目）
        func(
            "via_field() -> Response",
            "none",
            "  let r = Route { handler: handler_impl, path: \"/a\" };\n  return run_with(r.handler);"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::EffectMissing, 45, 1));
}

#[test]
fn field_call_effect_is_the_declared_d() {
    // フィールドの関数を呼んだ効果は D（db）。宣言していない関数は通らない（37 行目の関数）。
    let src = prog(&func(
        "call_quiet(r: Route) -> Response",
        "none",
        "  return r.handler(Request { id: 1 });",
    ));
    assert_eq!(err_at(&src), (ErrorKind::EffectMissing, 37, 1));
}

// ---- 成功: 戻り値・`let` の型注釈 ----

#[test]
fn return_and_let_positions_ok() {
    let src = prog(&format!(
        "{}{}{}{}",
        func(
            "make_handler() -> fn(Request) -> Response effects db",
            "none",
            "  return handler_impl;"
        ),
        // 戻り値で作られた関数の値は、`let` で束縛してから呼べる
        func(
            "use_made() -> Response",
            "db",
            "  let h = make_handler();\n  return h(Request { id: 3 });"
        ),
        func(
            "use_let() -> Response",
            "db",
            "  let h: fn(Request) -> Response effects db = handler_impl;\n  return h(Request { id: 5 });"
        ),
        func(
            "use_let_closure() -> Int",
            "none",
            "  let h: fn(Int) -> Int effects none = move |x| x * 2;\n  return h(21);"
        )
    ));
    ok(&src);
}

#[test]
fn return_position_with_ref_param_ok() {
    // 関数の型の引数の中の参照は「参照を返す」ことにはならない。
    let src = prog_bare(&format!(
        "{}{}{}",
        func("add_ref(x: &Int) -> Int", "none", "  return x + 1;"),
        func(
            "pick() -> fn(&Int) -> Int effects none",
            "none",
            "  return add_ref;"
        ),
        func(
            "use_pick() -> Int",
            "none",
            "  let h = pick();\n  let v = 40;\n  return h(&v);"
        )
    ));
    ok(&src);
}

#[test]
fn return_closure_ok() {
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "make_adder() -> fn(Int) -> Int effects none",
            "none",
            "  return move |x| x + 1;"
        )
    ));
    ok(&src);
}

// ---- 成功: 列挙型の値の中身 ----

#[test]
fn enum_payload_ok() {
    let src = prog(&format!(
        "{}{}{}{}",
        "enum Handler {\n  Db(fn(Request) -> Response effects db),\n  Pure(fn(Int) -> Int effects none),\n}\n\n",
        func("make_db() -> Handler", "none", "  return Handler.Db(handler_impl);"),
        func(
            "make_pure() -> Handler",
            "none",
            "  return Handler.Pure(move |x| x + 1);"
        ),
        func(
            "call_handler(h: Handler, req: Request) -> Response",
            "db",
            "  return match h {\n    Handler.Db(f) => f(req),\n    Handler.Pure(g) => Response { status: g(1) },\n  };"
        )
    ));
    ok(&src);
}

// ---- 成功: クロージャのとらえ込みと性質 ----

#[test]
fn closure_captures_ok() {
    let src = prog_bare(&format!(
        "{}{}{}{}",
        FNBOX,
        func(
            "box_int() -> FnBox",
            "none",
            "  let base = 40;\n  return FnBox { f: move |x| x + base };"
        ),
        // `String` は `Copyable` でないが `Sendable`・`Shareable` なので移してとらえられる
        func(
            "box_str() -> FnBox",
            "none",
            "  let s = \"sigil\";\n  return FnBox { f: move |x| x + s.len() };"
        ),
        func(
            "call_box() -> Int",
            "none",
            "  let b = FnBox { f: move |x| x + 1 };\n  return b.f(41);"
        )
    ));
    ok(&src);
}

#[test]
fn generic_capture_with_bounds_ok() {
    let src = prog_bare(&format!(
        "{}{}{}",
        FNBOX,
        func("peek<T>(v: &T) -> Int", "none", "  return 0;"),
        func(
            "put_ok<T: Sendable + Shareable>(x: T) -> FnBox",
            "none",
            "  return FnBox { f: move |q| peek(&x) };"
        )
    ));
    ok(&src);
}

#[test]
fn fn_typed_struct_properties() {
    // フィールド・戻り値の関数の型は `Sendable`・`Shareable` を持つので、
    // それを持つ構造体も両性質を持つ。
    let src = prog(&format!(
        "{}{}",
        func(
            "need_ss<T: Sendable + Shareable>(x: T) -> Int",
            "none",
            "  return 0;"
        ),
        func(
            "use_props() -> Int",
            "none",
            "  let r = Route { handler: handler_impl, path: \"/a\" };\n  return need_ss(r);"
        )
    ));
    ok(&src);
}

#[test]
fn fn_typed_struct_is_not_equatable() {
    let src = prog(&format!(
        "{}{}",
        func("need_eq<T: Equatable>(x: T) -> Int", "none", "  return 0;"),
        // need_eq は 37〜44 行目、use_props_eq は 45 行目から。呼び出しは 52 行目の文。
        func(
            "use_props_eq() -> Int",
            "none",
            "  let r = Route { handler: idle_handler, path: \"/a\" };\n  return need_eq(r);"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::ConstraintUnsatisfied, 52, 3));

    // 関数の型を持つ構造体は比べられない（`Equatable` を持たない。文は 43 行目）。
    let src = prog(&func(
        "cmp(r1: Route, r2: Route) -> Bool",
        "none",
        "  return r1 == r2;",
    ));
    assert_eq!(err_at(&src), (ErrorKind::NotEquatable, 43, 3));
}

// ---- エラー: 効果の超過（⊑） ----

#[test]
fn effect_exceeds_declared() {
    // 名前の関数: E = db が D = none を超える（代入は 47 行目の文）。
    let src = prog(&format!(
        "{}{}",
        "struct Quiet {\n  h: fn(Request) -> Response effects none,\n}\n\n",
        func(
            "put() -> Quiet",
            "none",
            "  return Quiet { h: handler_impl };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnEffectExceeds, 47, 3));

    // クロージャ: 本体から推論した効果（console）が D = none を超える（文は 15 行目）。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put() -> FnBox",
            "none",
            "  return FnBox { f: move |x| { console.log(&\"y\"); return x; } };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnEffectExceeds, 15, 3));
}

#[test]
fn effect_hierarchy_subset() {
    // E = fs.read は D = fs 以下（階層の ⊑）なので通る。
    let src = prog_bare(&format!(
        "{}{}{}",
        "struct AnyReader {\n  h: fn(String) -> Int effects fs,\n}\n\n",
        func(
            "shallow(p: String) -> Int",
            "fs.read",
            "  return read_some(p);"
        ),
        func(
            "put() -> AnyReader",
            "none",
            "  return AnyReader { h: shallow };"
        )
    ));
    ok(&src);

    // 逆（E = fs が D = fs.read を超える）は落ちる（代入は 23 行目の文）。
    let src = prog_bare(&format!(
        "{}{}{}",
        "struct Reader {\n  h: fn(String) -> Int effects fs.read,\n}\n\n",
        func("deep(p: String) -> Int", "fs", "  return read_any(p);"),
        func("put() -> Reader", "none", "  return Reader { h: deep };")
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnEffectExceeds, 23, 3));
}

// ---- エラー: 置けない位置・効果節の必須 ----

#[test]
fn missing_effects_clause_is_rejected() {
    // フィールド（宣言は 5 行目）。
    let src = prog_bare("struct S {\n  h: fn(Int) -> Int,\n}\n");
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));

    // 戻り値（関数の宣言は 5 行目）。
    let src = prog_bare(&func("g() -> fn(Int) -> Int", "none", "  return g;"));
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));

    // `let` の型注釈（文は 11 行目）。
    let src = prog_bare(&func(
        "g() -> Int",
        "none",
        "  let h: fn(Int) -> Int = g;\n  return 0;",
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 11, 3));
}

#[test]
fn fn_type_in_type_arg_tuple_option_is_rejected() {
    // 型引数（宣言は 5 行目）。
    let src = prog_bare("struct S {\n  hs: List<fn(Int) -> Int effects none>,\n}\n");
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));

    // 組の中。
    let src = prog_bare("struct S {\n  t: (fn(Int) -> Int effects none, Int),\n}\n");
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));

    // `Option` の中。
    let src = prog_bare("struct S {\n  o: Option<fn(Int) -> Int effects none>,\n}\n");
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));

    // 置ける位置の関数の型の引数に関数の型は書けない。
    let src = prog_bare("struct S {\n  h: fn(fn(Int) -> Int) -> Int effects none,\n}\n");
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));
}

#[test]
fn effects_clause_on_param_is_rejected() {
    // 引数の位置に効果つきの関数の型は書けない（関数の宣言は 5 行目）。
    let src = prog_bare(&func(
        "g(f: fn(Int) -> Int effects none) -> Int",
        "none",
        "  return 0;",
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 5, 1));
}

// ---- エラー: `of(...)` の関数・引数の位置の引数は入れられない ----

#[test]
fn of_function_cannot_be_stored() {
    // 引数の別名は `of(f)` の効果を引き継ぐ。その別名の値も入れられない
    // （put は 9 行目から。代入は 16 行目の文）。
    // なお、効果に `of(...)` を持つ名前の関数は、引数に関数の型を持つため
    // その値の型を置ける位置に書けず、直接入れる形は型が合わずに弾かれる。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put(f: fn(Int) -> Int) -> FnBox",
            "none",
            "  let g = f;\n  return FnBox { f: g };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 16, 3));
}

#[test]
fn fn_param_cannot_be_stored() {
    // put は 9 行目から。代入は 15 行目の文。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put(f: fn(Int) -> Int) -> FnBox",
            "none",
            "  return FnBox { f: f };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnTypePosition, 15, 3));
}

// ---- エラー: クロージャのとらえ方 ----

#[test]
fn borrow_captures_cannot_be_stored() {
    // `move` のないクロージャ（put は 9 行目から。代入は 16 行目の文）。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put() -> FnBox",
            "none",
            "  let base = 1;\n  return FnBox { f: |x| x + base };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::ClosureNeedsMove, 16, 3));

    // 参照をとらえる `move` クロージャ（代入は 15 行目の文）。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put(r: &Int) -> FnBox",
            "none",
            "  return FnBox { f: move |x| x + r };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::ClosureCaptureRef, 15, 3));
}

#[test]
fn capture_write_cannot_be_stored() {
    // とらえた値を書き換えるクロージャ（put は 9 行目から。代入は 16 行目の文）。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put() -> FnBox",
            "none",
            "  let mut n = 0;\n  let b = FnBox { f: move |x| { n = n + 1; return x; } };\n  return b;"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnCaptureWrite, 16, 3));
}

#[test]
fn non_sendable_capture_cannot_be_stored() {
    // 引数の位置の関数の型の値（どの性質も持たない）をとらえる（代入は 15 行目の文）。
    let src = prog_bare(&format!(
        "{}{}",
        FNBOX,
        func(
            "put(g: fn(Int) -> Int) -> FnBox",
            "none",
            "  return FnBox { f: move |x| g(x) };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnCaptureNotSendable, 15, 3));
}

#[test]
fn non_shareable_capture_cannot_be_stored() {
    // `Sendable` だけを持つ型引数の値をとらえる（peek は 9〜16 行目、
    // put は 17 行目から。代入は 23 行目の文）。
    let src = prog_bare(&format!(
        "{}{}{}",
        FNBOX,
        func("peek<T>(v: &T) -> Int", "none", "  return 0;"),
        func(
            "put<T: Sendable>(x: T) -> FnBox",
            "none",
            "  return FnBox { f: move |q| peek(&x) };"
        )
    ));
    assert_eq!(err_at(&src), (ErrorKind::FnCaptureNotShareable, 23, 3));
}

//! ADR 0022 の PR 2-4d（3/3）: 関数の型をフィールド・戻り値に置く実行（決定 3）。
//! 置ける位置（構造体のフィールド・列挙型の値の中身・戻り値・`let` の型注釈）に
//! 入れた関数の値（名前の関数と `move` クロージャ）が、SIR 検証器・評価器・
//! ネイティブ実行ファイルで一致して動くことと、入れたクロージャのとらえた値が
//! 入れ物と一緒に破棄されることを確かめる。
//! 実行は `common::run` で評価器とネイティブを突き合わせる（破棄の順序も含む）。

mod common;

use common::native::{exec_env, scratch, try_build};
use sigil::exec::Arg;

const HEAD: &str = "module m.t\n  targets server\n  trust none\n\n";

fn func(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

fn run_int(src: &str, f: &str) -> String {
    match common::run(src, f, &[]) {
        Ok(o) => o.value,
        Err(e) => panic!("成功するはずが失敗した: {e}\n{src}"),
    }
}

/// ハンドラ表の共通部分。`Route.handler` は置ける位置の関数の型（効果は `none`）。
const DEFS: &str = "struct Request {\n  id: Int,\n}\n\nstruct Response {\n  status: Int,\n}\n\nstruct Route {\n  handler: fn(Request) -> Response effects none,\n  path: String,\n}\n\nfn handler_impl(req: Request) -> Response\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Response { status: req.id + 100 };\n}\n\nfn idle_handler(req: Request) -> Response\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Response { status: 204 };\n}\n\n";

fn prog(items: &str) -> String {
    format!("{HEAD}{DEFS}{items}")
}

// ---- ハンドラ表（フィールドの関数） ----

#[test]
fn handler_table_from_list_runs() {
    // ADR 0022 決定 3 の決め打ち事例: 構造体の一覧からハンドラを選んで呼ぶ。
    let src = prog(&format!(
        "{}{}{}",
        func(
            "routes() -> List<Route>",
            "none",
            "  let mut rs = List.new();\n  rs.push(Route { handler: handler_impl, path: \"/a\" });\n  rs.push(Route { handler: idle_handler, path: \"/b\" });\n  return rs;"
        ),
        func(
            "call_at(routes: &List<Route>, idx: Int, req: Request) -> Response",
            "none",
            "  match routes.get(idx) {\n    Some(r) => {\n      return r.handler(req);\n    }\n    None => {\n      return Response { status: 404 };\n    }\n  }"
        ),
        func(
            "main() -> Int",
            "none",
            "  let rs = routes();\n  let a = call_at(&rs, 0, Request { id: 1 });\n  let b = call_at(&rs, 1, Request { id: 2 });\n  let c = call_at(&rs, 9, Request { id: 3 });\n  return a.status * 1000000 + b.status * 1000 + c.status;"
        )
    ));
    assert_eq!(run_int(&src, "main"), "101204404");
}

#[test]
fn field_call_direct_and_via_method_runs() {
    let src = prog(&format!(
        "{}{}{}",
        func(
            "mk_route() -> Route",
            "none",
            "  return Route { handler: idle_handler, path: \"/b\" };"
        ),
        func(
            "main() -> Int",
            "none",
            "  let r = Route { handler: handler_impl, path: \"/a\" };\n  let a = r.handler(Request { id: 5 });\n  let b = r.serve(Request { id: 6 });\n  let c = mk_route().handler(Request { id: 7 });\n  return a.status * 1000000 + b.status * 1000 + c.status;"
        ),
        "impl Route {\n  fn serve(self: &Route, req: Request) -> Response\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {\n    return self.handler(req);\n  }\n}\n"
    ));
    assert_eq!(run_int(&src, "main"), "105106204");
}

#[test]
fn returned_struct_with_stored_closure_runs() {
    // 戻り値の構造体に入ったクロージャも、束縛してから複数回呼べる。
    let src = format!(
        "{HEAD}{FNBOX}{}{}",
        func(
            "make_box() -> FnBox",
            "none",
            "  let s = \"xyz\";\n  return FnBox { f: move |x| x + s.len() };"
        ),
        func(
            "main() -> Int",
            "none",
            "  let b = make_box();\n  return b.f(1) * 10 + b.f(2);"
        )
    );
    // s.len() = 3: 40 + 5
    assert_eq!(run_int(&src, "main"), "45");
}

#[test]
fn field_fn_passed_to_higher_order_runs() {
    // フィールドの関数の値を、引数の位置の関数の型へ渡して呼ぶ（`of(f)` の実行側）。
    let src = prog(&format!(
        "{}{}",
        func(
            "run_with(f: fn(Request) -> Response, req: Request) -> Response",
            "of(f)",
            "  return f(req);"
        ),
        func(
            "main() -> Int",
            "none",
            "  let r = Route { handler: handler_impl, path: \"/a\" };\n  let h = r.handler;\n  let a = run_with(h, Request { id: 7 });\n  let b = run_with(h, Request { id: 8 });\n  return a.status * 1000 + b.status;"
        )
    ));
    assert_eq!(run_int(&src, "main"), "107108");
}

// ---- 構造体に入れた `move` クロージャ ----

const FNBOX: &str = "struct FnBox {\n  f: fn(Int) -> Int effects none,\n}\n\n";

#[test]
fn stored_move_closure_called_repeatedly_runs() {
    // とらえた値（String）は書き換えられず、呼ぶたびに同じ値を読む。
    let src = format!(
        "{HEAD}{FNBOX}{}{}",
        func(
            "apply(b: &FnBox, x: Int) -> Int",
            "none",
            "  return b.f(x);"
        ),
        func(
            "main() -> Int",
            "none",
            "  let s = \"hello\";\n  let b = FnBox { f: move |x| x + s.len() };\n  return b.f(1) + b.f(2) * 10 + apply(&b, 3) * 100;"
        )
    );
    // s.len() = 5: 6 + 70 + 800
    assert_eq!(run_int(&src, "main"), "876");
}

#[test]
fn stored_named_fn_and_local_closure_alias_run() {
    let src = format!(
        "{HEAD}{FNBOX}{}{}",
        func("inc(x: Int) -> Int", "none", "  return x + 1;"),
        func(
            "main() -> Int",
            "none",
            "  let a = FnBox { f: inc };\n  let c = move |x| x * 3;\n  let b = FnBox { f: c };\n  return a.f(10) * 100 + b.f(10);"
        )
    );
    assert_eq!(run_int(&src, "main"), "1130");
}

// ---- 戻り値と `let` の型注釈 ----

#[test]
fn returned_fn_values_run() {
    let src = prog(&format!(
        "{}{}{}{}",
        func(
            "make_handler() -> fn(Request) -> Response effects none",
            "none",
            "  return handler_impl;"
        ),
        func(
            "make_adder() -> fn(Int) -> Int effects none",
            "none",
            "  let base = 40;\n  return move |x| x + base;"
        ),
        func(
            "make_lenner() -> fn(Int) -> Int effects none",
            "none",
            "  let s = \"abcd\";\n  return move |x| x + s.len();"
        ),
        func(
            "main() -> Int",
            "none",
            "  let h = make_handler();\n  let a = h(Request { id: 3 });\n  let b = h(Request { id: 4 });\n  let g = make_adder();\n  let l = make_lenner();\n  return a.status * 100000 + b.status * 1000 + (g(1) + g(2)) * 10 + l(1) + l(2);"
        )
    ));
    // 103*100000 + 104*1000 + 83*10 + 5 + 6
    assert_eq!(run_int(&src, "main"), "10404841");
}

#[test]
fn let_annotated_fn_values_run() {
    let src = prog(&func(
            "main() -> Int",
            "none",
            "  let h: fn(Int) -> Int effects none = move |x| x * 2;\n  let n: fn(Request) -> Response effects none = handler_impl;\n  let r = n(Request { id: 2 });\n  return h(21) + h(1) + r.status;"
    ));
    // 42 + 2 + 102
    assert_eq!(run_int(&src, "main"), "146");
}

// ---- 列挙型の値の中身 ----

#[test]
fn enum_payload_fn_values_run() {
    let src = prog(&format!(
        "{}{}{}{}",
        "enum Handler {\n  Db(fn(Request) -> Response effects none),\n  Pure(fn(Int) -> Int effects none),\n}\n\n",
        func(
            "call_db(h: Handler, req: Request) -> Int",
            "none",
            "  match h {\n    Handler.Db(g) => {\n      let a = g(req);\n      let b = g(Request { id: 2 });\n      return a.status + b.status;\n    }\n    Handler.Pure(f) => {\n      return f(0);\n    }\n  }"
        ),
        func(
            "call_pure(h: Handler) -> Int",
            "none",
            "  match h {\n    Handler.Pure(f) => {\n      return f(1) * 10 + f(2);\n    }\n    Handler.Db(g) => {\n      return 0;\n    }\n  }"
        ),
        func(
            "main() -> Int",
            "none",
            "  let a = call_db(Handler.Db(handler_impl), Request { id: 1 });\n  let s = \"ab\";\n  let b = call_pure(Handler.Pure(move |x| x + s.len()));\n  return a * 1000 + b;"
        )
    ));
    // a = 203, b = 30 + 4 = 34
    assert_eq!(run_int(&src, "main"), "203034");
}

// ---- 破棄: 入れたクロージャのとらえた値は入れ物と一緒に破棄される ----

#[test]
fn stored_closure_capture_drops_with_the_container() {
    // 破棄の順序（SIR の評価器とネイティブで一致）: w (6)、入れ物のクロージャの環境の p (1)。
    let src = format!(
        "{HEAD}struct Probe {{\n  id: Int,\n}}\n\nstruct Box2 {{\n  f: fn() -> Int effects none,\n}}\n\n{}{}",
        func("mk(n: Int) -> Probe", "none", "  return Probe { id: n };"),
        func(
            "t() -> Int",
            "none",
            "  let p = mk(1);\n  let b = Box2 { f: move || p.id };\n  let w = mk(6);\n  return b.f();"
        )
    );
    let sir = sigil::ir_eval::run_with(
        &src,
        "t",
        &[],
        &sigil::exec::Options {
            trace_drops: true,
            ..sigil::exec::Options::captured()
        },
    )
    .unwrap();
    assert_eq!(sir.value, "1");
    let (got, native) = common::native::native_drops(&[&src], "t", &[] as &[Arg]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(sir.drops, native, "評価器と実行ファイルの破棄の順序");
    let ids: Vec<i64> = sir
        .drops
        .iter()
        .filter_map(|s| s.strip_prefix("Probe { id: "))
        .map(|s| s.trim_end_matches(" }").parse().unwrap())
        .collect();
    assert_eq!(ids, [6, 1]);
}

/// ネイティブ: 構造体に入れたクロージャを作って捨てる繰り返しで、生きている塊の数が
/// 繰り返しの回数に比例して増えないこと（`SIGIL_RT_LIVE`。m5_free_strings の流儀）。
fn live(src: &str, n: i64) -> (i64, i64, String) {
    let dir = scratch("fnlive");
    let exe = dir.join("prog");
    try_build(&[src], "run", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec_env(&exe, &[n.to_string()], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "終了コード\n{}\n{src}", got.stderr);
    let line = got
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {}", got.stderr))
        .to_string();
    let num = |key: &str| -> i64 {
        line.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{key} がない: {line}"))
    };
    (num("strs="), num("blocks="), got.stdout)
}

#[test]
fn stored_closure_drop_does_not_grow_with_the_iteration_count() {
    let src = format!(
        "{HEAD}{FNBOX}enum Tag2 {{\n  Pure(fn(Int) -> Int effects none),\n  Empty,\n}}\n\n{}",
        func(
            "run(n: Int) -> Int",
            "none",
            "  let mut i = 0;\n  let mut total = 0;\n  while i < n {\n    let base = \"  abcdef  \";\n    let s = base.trim();\n    let b = FnBox { f: move |x| x + s.len() };\n    total = total + b.f(1) + b.f(2);\n    let s2 = base.trim();\n    let e = Tag2.Pure(move |x| x + s2.len());\n    total = total + match e {\n      Tag2.Pure(f) => f(0),\n      Tag2.Empty => 0,\n    };\n    i = i + 1;\n  }\n  return total;"
        )
    );
    let (s1, b1, out1) = live(&src, 1_000);
    assert!(out1.trim_end().ends_with("21000"), "{out1}");
    let (s2, b2, out) = live(&src, 100_000);
    assert!(out.trim_end().ends_with("2100000"), "{out}");
    assert_eq!(
        (s1, b1),
        (s2, b2),
        "1000 回と 100000 回で生きている数が違う"
    );
    assert!(
        s2 < 100 && b2 < 100,
        "生きている数が多い: strs={s2} blocks={b2}"
    );
}

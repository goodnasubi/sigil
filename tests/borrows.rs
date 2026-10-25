//! M2 の PR3（ADR 0012 の決定 1 の 3・4）: 借用の検査。最後に使う地点までの区間・
//! 1 回の呼び出しの中の重なり・一時的な値・返された参照の区間・総称の型引数に入れた参照。

use sigil::check::check;
use sigil::ErrorKind;
use ErrorKind::{
    BorrowConflict, BorrowOverlap, RefSubstitution, ReturnBorrowRoot, TempBorrow, UseAfterMove,
};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func_fx(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

fn func(sig: &str, body: &str) -> String {
    func_fx(sig, "none", body)
}

fn defs() -> String {
    format!(
        "struct Account {{\n  balance: Int,\n  owner: String,\n}}\nstruct Pair {{\n  x: Int,\n  y: Int,\n}}\n{}{}{}{}{}{}",
        func("show(n: &Int)", "  return;"),
        func("both(a: &mut Int, b: &Int)", "  return;"),
        func("bump(a: &mut Int, n: Int)", "  return;"),
        func(
            "first(xs: &List<Account>) -> Option<&Account>",
            "  return xs.get(0);"
        ),
        func("make() -> List<Account>", "  return List.new();"),
        func("pick(a: &Account) -> &Account", "  return a;"),
    )
}

fn src(sig: &str, body: &str) -> String {
    format!("{HEAD}{}{}", defs(), func(sig, body))
}

fn kind(sig: &str, body: &str) -> ErrorKind {
    let s = src(sig, body);
    match check(&s) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{s}"),
        Err(e) => e.kind,
    }
}

fn ok(sig: &str, body: &str) {
    let s = src(sig, body);
    if let Err(e) = check(&s) {
        panic!("{e}\n{s}");
    }
}

fn msg(sig: &str, body: &str) -> String {
    check(&src(sig, body)).unwrap_err().to_string()
}

// ---- 1 回の呼び出しの中の重なり ----

#[test]
fn overlap_inside_one_call() {
    // `&mut` と重なる借用・読み出し・移動
    assert_eq!(
        kind("f(a: Int)", "  let mut m = a;\n  both(&mut m, &m);"),
        BorrowOverlap
    );
    assert_eq!(
        kind("f(a: Int)", "  let mut m = a;\n  bump(&mut m, m);"),
        BorrowOverlap
    );
    assert_eq!(
        kind(
            "f(xs: Int)",
            "  let mut ys = List.new();\n  ys.push(ys.len());"
        ),
        BorrowOverlap
    );
    // 説明に直し方が出る
    let m = msg(
        "f(xs: Int)",
        "  let mut ys = List.new();\n  ys.push(ys.len());",
    );
    assert!(
        m.contains("let n = xs.len(); xs.push(n);") && m.contains("`ys`"),
        "{m}"
    );
}

#[test]
fn no_overlap_cases() {
    // 重ならない場所
    ok(
        "f(a: Int)",
        "  let mut p = Pair { x: a, y: a };\n  both(&mut p.x, &p.y);",
    );
    // 共有の借用どうし
    ok("f(a: Int)", "  show(&a);\n  let n = a;\n  show(&a);");
    // 先に取り出せばよい
    ok(
        "f(a: Int)",
        "  let mut ys = List.new();\n  let n = ys.len();\n  ys.push(n);",
    );
    // 別の変数
    ok(
        "f(a: Int)",
        "  let mut ys = List.new();\n  let zs: List<Int> = List.new();\n  ys.push(zs.len());",
    );
}

// ---- 返された参照の区間 ----

#[test]
fn get_then_push_conflicts() {
    let body =
        "  let mut xs = make();\n  let r = xs.get(0);\n  xs.push(a);\n  let b = r.is_some();";
    assert_eq!(kind("f(a: Account)", body), BorrowConflict);
    let m = msg("f(a: Account)", body);
    assert!(m.contains("`r`") && m.contains("`xs`"), "{m}");
    // 使い終えてから変更するなら通る
    ok(
        "f(a: Account)",
        "  let mut xs = make();\n  let r = xs.get(0);\n  let b = r.is_some();\n  xs.push(a);",
    );
    // 変更の後に使わない参照は残っていても構わない
    ok(
        "f(a: Account)",
        "  let mut xs = make();\n  let r = xs.get(0);\n  xs.push(a);",
    );
}

#[test]
fn returned_ref_region_extends_transitively() {
    // r は xs を借り、q は r を通して xs を借りる
    assert_eq!(
        kind(
            "f(a: Account)",
            "  let mut xs = make();\n  let r = first(&xs);\n  let q = first(&xs);\n  xs.push(a);\n  let n = q.is_some();"
        ),
        BorrowConflict
    );
    ok(
        "f(a: Account)",
        "  let mut xs = make();\n  let r = xs.get(0);\n  let n = match r {\n    Some(v) => v.balance,\n    None => 0,\n  };\n  xs.push(a);",
    );
    // 借りた参照を別の呼び出しに渡して得た参照も、元の場所を借りている
    assert_eq!(
        kind(
            "f(a: Account) -> Int",
            "  let mut xs = make();\n  let o = xs.get(0);\n  match o {\n    Some(v) => {\n      let w = pick(&v);\n      xs.push(a);\n      return w.balance;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;"
        ),
        BorrowConflict
    );
}

#[test]
fn mut_borrow_and_reads() {
    // 変数が書き換えられた後に、以前の借用を使う
    assert_eq!(
        kind(
            "f(a: Int)",
            "  let mut p = Pair { x: a, y: a };\n  let r = Some(&p.x);\n  p.x = 2;\n  let b = r.is_some();"
        ),
        BorrowConflict
    );
    // 別のフィールドの書き換えは重ならない
    ok(
        "f(a: Int)",
        "  let mut p = Pair { x: a, y: a };\n  let r = Some(&p.x);\n  p.y = 2;\n  let b = r.is_some();",
    );
    // 移動も貸しを無効にする
    assert_eq!(
        kind(
            "f(a: Account)",
            "  let mut xs = make();\n  let r = xs.get(0);\n  let ys = xs;\n  let b = r.is_some();"
        ),
        BorrowConflict
    );
}

#[test]
fn loop_next_iteration() {
    // 周の中で変更し、次の周で借用を使う
    assert_eq!(
        kind(
            "f(n: Int)",
            "  let mut xs: List<Int> = List.new();\n  let r = xs.get(0);\n  let mut i = 0;\n  while i < n {\n    let b = r.is_some();\n    xs.push(i);\n    i = i + 1;\n  }"
        ),
        BorrowConflict
    );
    // 周の中で借りて使い切るなら通る
    ok(
        "f(a: Int, n: Int)",
        "  let mut xs = make();\n  let mut i = 0;\n  while i < n {\n    let r = xs.get(0);\n    let b = r.is_some();\n    i = i + 1;\n  }",
    );
}

// ---- 一時的な値 ----

#[test]
fn temporaries_die_at_end_of_statement() {
    assert_eq!(kind("f()", "  let r = make().get(0);"), TempBorrow);
    assert_eq!(kind("f()", "  let r = first(&make());"), TempBorrow);
    let m = msg("f()", "  let r = make().get(0);");
    assert!(m.contains("let"), "{m}");
    // 文の `match` は対象の終わりが文の終わりなので、腕のブロックでは束縛を使えない
    assert_eq!(
        kind(
            "f() -> Int",
            "  match make().get(0) {\n    Some(a) => {\n      return a.balance;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;"
        ),
        TempBorrow
    );
    assert_eq!(
        kind(
            "f() -> Int",
            "  match &make() {\n    xs => {\n      return xs.len();\n    }\n  }\n  return 1;"
        ),
        TempBorrow
    );
    // 値の `match` は文の一部なので使える
    ok(
        "f() -> Int",
        "  let n = match make().get(0) {\n    Some(a) => a.balance,\n    None => 0,\n  };\n  return n;",
    );
    // 値を使い切る式は通る
    ok("f() -> Int", "  let n = make().len();\n  return n;");
    ok(
        "f() -> Int",
        "  let b = make().get(0).is_some();\n  return 1;",
    );
    ok("f()", "  let b = make().get(0).map(|a| a.balance);");
    // リテラルの借用
    ok("f()", "  show(&1);");
    let s = format!(
        "{HEAD}{}",
        func_fx("g()", "console", "  console.log(&\"text\");")
    );
    check(&s).unwrap();
}

#[test]
fn match_ref_keeps_borrow_for_arms() {
    // `match &x` の腕の間は x を借りている
    ok(
        "f(x: Option<Int>) -> Int",
        "  match &x {\n    Some(n) => {\n      return n;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;",
    );
    // 腕の中で x を移動して、束縛を使う
    assert_eq!(
        kind(
            "f(x: Option<Account>) -> Int",
            "  match &x {\n    Some(a) => {\n      let y = x;\n      return a.balance;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;"
        ),
        BorrowConflict
    );
    // match の後は x を使える
    ok(
        "f(x: Option<Account>) -> Int",
        "  match &x {\n    Some(a) => {\n      let n = a.balance;\n    }\n    None => {\n      let n = 0;\n    }\n  }\n  let y = x;\n  return 1;",
    );
}

// ---- 返せる参照の根 ----

#[test]
fn returned_ref_must_be_rooted_at_a_borrow_argument() {
    assert_eq!(
        kind("f(n: &Int, k: Int) -> &Int", "  let m = k;\n  return m;"),
        ReturnBorrowRoot
    );
    // 参照の引数なら返せる
    ok(
        "f(xs: &List<Account>) -> Option<&Account>",
        "  return xs.get(0);",
    );
    ok("f(a: &Account) -> &Account", "  return a;");
    // 借りた先が引数の中なら返せる
    ok(
        "f(xs: &List<Account>) -> Option<&Account>",
        "  let r = xs.get(0);\n  return r;",
    );
    // 所有する値の中への参照は返せない
    assert_eq!(
        kind(
            "f(xs: &List<Account>) -> Option<&Account>",
            "  let ys = make();\n  let r = ys.get(0);\n  return r;"
        ),
        ReturnBorrowRoot
    );
    assert_eq!(
        kind(
            "f(xs: &List<Account>) -> Option<&Account>",
            "  return make().get(0);"
        ),
        ReturnBorrowRoot
    );
    assert_eq!(
        kind("f(n: &Int, a: Int) -> Option<&Int>", "  return Some(a);"),
        ReturnBorrowRoot
    );
}

// ---- 総称の型引数に参照を入れる（決定 1 の 4） ----

#[test]
fn reference_in_generic_argument() {
    // `Some(&n)`・`Option<&T>` の is_some・map は通る
    ok("f(n: Int)", "  let o = Some(&n);\n  let b = o.is_some();");
    ok(
        "f(xs: &List<Account>) -> Int",
        "  let o = xs.get(0);\n  let b = o.is_some();\n  let m = o.map(|a| a.balance);\n  return m.unwrap_or(0);",
    );
    ok(
        "f(xs: &List<Account>) -> Bool",
        "  let m = xs.max_by(|a| a.balance);\n  return m.is_some();",
    );
    // 借用の引数が 2 つ（戻り値が T のとき）
    assert_eq!(
        kind(
            "f(xs: &List<Account>, d: &Account)",
            "  let r = xs.get(0).unwrap_or(&d);"
        ),
        RefSubstitution
    );
    // 戻り値の一番外でも Option の中でもない位置の参照
    assert_eq!(
        kind("f(xs: &List<Account>)", "  let r = xs.get(0).ok_or(1);"),
        RefSubstitution
    );
    // 変更できる入れ物に参照を入れる
    assert_eq!(
        kind("f(n: Int)", "  let mut xs = List.new();\n  xs.push(&n);"),
        RefSubstitution
    );
}

// ---- クロージャの引数（PR #28 のレビュー） ----

#[test]
fn unresolved_closure_param_double_move() {
    // 型が決まっていない引数を 2 回移動する形は、型が決まった後で Copyable でなければエラー
    let body = "  let f = move |x| {\n    let a = x;\n    let b = x;\n    return 0;\n  };";
    let e = check(&src("g()", body)).unwrap_err();
    assert!(
        matches!(e.kind, UseAfterMove | ErrorKind::CannotInfer),
        "{e}"
    );
    ok(
        "g() -> Int",
        "  let f = |x| {\n    let a = x;\n    let b = x;\n    return a + b;\n  };\n  return f(1);",
    );
}

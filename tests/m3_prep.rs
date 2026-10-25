//! M3 の着手時の変更（ADR 0013 の「M3 の着手時に必要な変更」）のテスト: 標準ライブラリの追加、`clone`、
//! 列挙型を通る再帰、関数の値の借用渡し。

use sigil::check::check;
use sigil::ErrorKind;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

const DEFS: &str = "struct Account {\n  balance: Int,\n  owner: String,\n}\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    )
}

fn src(items: &str) -> String {
    format!("{HEAD}{DEFS}{items}")
}

fn kind(items: &str) -> ErrorKind {
    let s = src(items);
    match check(&s) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{s}"),
        Err(e) => e.kind,
    }
}

fn ok(items: &str) {
    let s = src(items);
    if let Err(e) = check(&s) {
        panic!("{e}\n{s}");
    }
}

fn msg(items: &str) -> String {
    check(&src(items)).unwrap_err().to_string()
}

// ---- 標準ライブラリの追加（ADR 0013 の決定 2） ----

#[test]
fn std_additions_type_check() {
    ok(&func(
        "f(xs: &List<Int>) -> Option<&Int>",
        "  return xs.find(|a| a > 1);",
    ));
    ok(&func(
        "f(xs: &List<Account>) -> Option<&Account>",
        "  return xs.min_by(|a| a.balance);",
    ));
    ok(&func("f(x: Float) -> Option<Int>", "  return to_int(x);"));
    ok(&func("f() -> Int", "  return Int.min_value();"));
    ok(&func(
        "f(xs: &List<String>) -> &String",
        "  return xs.min();",
    ));
    ok(&func("f(xs: &List<Float>) -> Float", "  return xs.max();"));
}

#[test]
fn min_and_max_return_references() {
    // 結果を使い終わる前の push は衝突する（`get` と同じ）
    let k = kind(&func(
        "f(xs: &mut List<Int>)",
        "  let m = xs.min();\n  xs.push(1);\n  let n = m + 1;\n  return;",
    ));
    assert_eq!(k, ErrorKind::BorrowConflict);
    ok(&func(
        "f(xs: &mut List<Int>)",
        "  let n = xs.max() + 1;\n  xs.push(n);\n  return;",
    ));
}

#[test]
fn get_mut_writes_through_the_binding() {
    ok(&func(
        "f(xs: &mut List<Int>, v: Int)",
        "  match xs.get_mut(0) {\n    Some(a) => {\n      a = v;\n    }\n    None => {}\n  }\n  return;",
    ));
    ok(&func(
        "f(xs: &mut List<Account>, v: Int)",
        "  match xs.get_mut(0) {\n    Some(a) => {\n      a.balance = v;\n    }\n    None => {}\n  }\n  return;",
    ));
    assert_eq!(
        kind(&func(
            "f(xs: &mut List<Int>)",
            "  let r = xs.get_mut(0);\n  xs.push(1);\n  let t = r.is_some();\n  return;"
        )),
        ErrorKind::BorrowConflict
    );
}

#[test]
fn set_map_and_unwrap_are_gone() {
    assert_eq!(
        kind(&func("f(xs: &List<Int>)", "  let a = xs.get(0).unwrap();")),
        ErrorKind::NoSuchMember
    );
    assert_eq!(
        kind(&func("f(s: Set<Int>)", "  return;")),
        ErrorKind::UnknownType
    );
    assert_eq!(
        kind(&func("f(m: Map<Int, Int>)", "  return;")),
        ErrorKind::UnknownType
    );
}

// ---- clone（ADR 0013 の決定 2） ----

#[test]
fn clone_works_on_noncopyable_types() {
    ok(&func("f(a: &Account) -> Account", "  return a.clone();"));
    ok(&func(
        "f(xs: &List<Account>) -> List<Account>",
        "  return xs.clone();",
    ));
    ok(&func("f(s: &String) -> String", "  return s.clone();"));
    ok(&func("f<T>(x: &T) -> T", "  return x.clone();"));
    // 複製した後も元の値は使える
    ok(&func(
        "f(a: Account) -> Int",
        "  let b = a.clone();\n  let n = a.balance + b.balance;\n  return n;",
    ));
}

#[test]
fn clone_of_copyable_is_an_error() {
    for (sig, body) in [
        ("f(n: Int) -> Int", "  return n.clone();"),
        ("f(n: &Int) -> Int", "  return n.clone();"),
        ("f() -> Int", "  let a = 1;\n  return a.clone();"),
        ("f<T: Copyable>(x: &T) -> T", "  return x.clone();"),
        ("f(b: Bool) -> Bool", "  return b.clone();"),
    ] {
        assert_eq!(kind(&func(sig, body)), ErrorKind::CloneOfCopyable, "{sig}");
    }
    // 説明に、値として読めば複製されることが出る
    assert!(msg(&func("f(n: Int) -> Int", "  return n.clone();")).contains("Copyable"));
}

#[test]
fn clone_is_not_for_function_values_or_user_impls() {
    assert_eq!(
        kind(&func(
            "f(g: fn(Int) -> Int) -> Int",
            "  let h = g.clone();\n  return 0;"
        )),
        ErrorKind::NoSuchMember
    );
    assert_eq!(
        kind(
            "impl Account {\n  fn clone(self: &Account) -> Account\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {\n    return Account { balance: 1, owner: \"a\" };\n  }\n}\n"
        ),
        ErrorKind::ReservedNameDefined
    );
}

#[test]
fn clone_does_not_keep_the_receiver_borrowed() {
    ok(&func(
        "f(xs: &mut List<Account>, a: &Account)",
        "  let b = a.clone();\n  xs.push(b);\n  return;",
    ));
}

// ---- 列挙型を通る再帰（ADR 0013 の決定 2・補遺） ----

const EXPR: &str = "enum Expr {\n  Num(Int),\n  Add(Expr, Expr),\n}\n";

#[test]
fn recursive_enum_and_evaluator() {
    ok(&format!(
        "{EXPR}{}",
        func(
            "eval(e: &Expr) -> Int",
            "  return match e {\n    Expr.Num(n) => n,\n    Expr.Add(a, b) => eval(&a) + eval(&b),\n  };"
        )
    ));
    ok(&format!(
        "{EXPR}{}",
        func(
            "mk() -> Expr",
            "  return Expr.Add(Expr.Num(1), Expr.Add(Expr.Num(2), Expr.Num(3)));"
        )
    ));
    // 構造体から Option 経由の輪、列挙型を経由する輪
    ok("struct Chain {\n  v: Int,\n  next: Option<Chain>,\n}\n");
    ok("struct Node {\n  e: Tree,\n}\nenum Tree {\n  Leaf,\n  Br(Node, Node),\n}\n");
}

#[test]
fn recursive_types_need_a_base_arm_through_an_enum() {
    assert_eq!(kind("enum Ex {\n  A(Ex),\n}\n"), ErrorKind::RecursiveType);
    assert_eq!(kind("struct S {\n  s: S,\n}\n"), ErrorKind::RecursiveType);
    assert_eq!(
        kind("enum Ex {\n  A(Fx),\n}\nenum Fx {\n  B(Ex),\n}\n"),
        ErrorKind::RecursiveType
    );
    assert_eq!(
        kind("struct A {\n  t: (Int, A),\n}\n"),
        ErrorKind::RecursiveType
    );
    // 構造体を輪に含めても、列挙型に輪を通らない腕があればよい
    ok("struct P {\n  l: Ex,\n}\nenum Ex {\n  Z,\n  A(P),\n}\n");
}

#[test]
fn properties_on_recursive_types_terminate() {
    // Equatable は輪の先でも成り立つと仮定して導出する。二分木の深さ分の指数にならない
    let mut ty = String::from("enum Big {\n  Leaf,\n  Br(Big, Big, Big),\n}\n");
    ty.push_str(&func("f(a: &Big, b: &Big) -> Bool", "  return a == b;"));
    ok(&ty);
}

// ---- 関数の値の借用渡し（ADR 0013 の決定 3・補遺） ----

const FNDEFS: &str = "fn run(f: fn(Int) -> Int, n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(f)\n{\n  return f(n);\n}\nfn pair(a: fn(Int) -> Int, b: fn(Int) -> Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(a), of(b)\n{\n  return a(1) + b(2);\n}\nfn inc(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return x + 1;\n}\n";

fn fsrc(sig: &str, eff: &str, body: &str) -> String {
    format!(
        "{HEAD}{DEFS}{FNDEFS}fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {eff}\n{{\n{body}\n}}\n"
    )
}

fn fok(sig: &str, eff: &str, body: &str) {
    let s = fsrc(sig, eff, body);
    if let Err(e) = check(&s) {
        panic!("{e}\n{s}");
    }
}

fn fkind(sig: &str, eff: &str, body: &str) -> ErrorKind {
    let s = fsrc(sig, eff, body);
    match check(&s) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{s}"),
        Err(e) => e.kind,
    }
}

#[test]
fn function_parameter_is_borrowed_not_moved() {
    // 何度でも渡せる・呼べる・渡し続けられる
    fok(
        "f(g: fn(Int) -> Int) -> Int",
        "of(g)",
        "  return run(g, 1) + run(g, 2) + g(3);",
    );
    fok(
        "f(g: fn(Int) -> Int, n: Int) -> Int",
        "of(g)",
        "  let mut i = 0;\n  let mut s = 0;\n  while i < n {\n    s = s + run(g, i);\n    i = i + 1;\n  }\n  return s;",
    );
    // 別名にしても移動しない。別名の別名も同じ
    fok(
        "f(g: fn(Int) -> Int) -> Int",
        "of(g)",
        "  let h = g;\n  let k = g;\n  let m = h;\n  return run(h, 1) + run(k, 2) + run(m, 3);",
    );
    // `move` のクロージャがとらえても移動しない
    fok(
        "f(g: fn(Int) -> Int) -> Int",
        "of(g)",
        "  let c = move |x| g(x) + 1;\n  return run(g, 1) + run(c, 2);",
    );
}

#[test]
fn let_bound_closure_is_borrowed_when_passed_but_alias_moves() {
    fok(
        "f(k: Int) -> Int",
        "none",
        "  let g = move |x| x + k;\n  return run(g, 1) + run(g, 2);",
    );
    assert_eq!(
        fkind(
            "f(k: Int) -> Int",
            "none",
            "  let g = move |x| x + k;\n  let h = g;\n  return run(g, 1);"
        ),
        ErrorKind::UseAfterMove
    );
    // `move` のクロージャへのとらえ込みは移動のまま
    assert_eq!(
        fkind(
            "f(k: Int) -> Int",
            "none",
            "  let g = move |x| x + k;\n  let c = move |x| g(x);\n  return run(g, 1);"
        ),
        ErrorKind::UseAfterMove
    );
}

#[test]
fn borrow_and_move_in_one_call_overlap() {
    assert_eq!(
        fkind(
            "f(g: fn(Int) -> Int) -> Int",
            "of(g)",
            "  return pair(g, move |x| g(x));"
        ),
        ErrorKind::BorrowOverlap
    );
    fok(
        "f(g: fn(Int) -> Int) -> Int",
        "of(g)",
        "  return pair(g, |x| g(x));",
    );
}

#[test]
fn named_function_is_copyable() {
    fok(
        "f() -> Int",
        "none",
        "  let h = inc;\n  let k = inc;\n  return run(h, 1) + run(k, 2) + run(inc, 3);",
    );
    // 別名の連鎖
    fok(
        "f(g: fn(Int) -> Int) -> Int",
        "of(g)",
        "  let h = g;\n  let k = h;\n  return run(k, 1) + run(h, 2) + run(g, 3);",
    );
    fok(
        "f() -> Int",
        "none",
        "  let h = inc;\n  let k = h;\n  let m = h;\n  return run(k, 1) + run(m, 2);",
    );
}

#[test]
fn function_values_have_no_clone() {
    assert_eq!(
        fkind(
            "f(g: fn(Int) -> Int) -> Int",
            "of(g)",
            "  let h = g.clone();\n  return 0;"
        ),
        ErrorKind::NoSuchMember
    );
}

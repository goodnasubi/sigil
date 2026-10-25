//! M2 の PR2（ADR 0012 の決定 1 の 1・2）: 移動の検査。場所・分岐・ループ・部分的な移動・
//! `match` による移動・参照からの移動・再代入による初期化し直し。

use sigil::check::check;
use sigil::ErrorKind;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";
const DEFS: &str = "struct Account {\n  balance: Int,\n  owner: String,\n}\n\
fn close(a: Account) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a.balance;\n}\n\
fn balance_of(a: &Account) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a.balance;\n}\n\
fn open(n: Int) -> Account\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Account { balance: n, owner: \"o\" };\n}\n\
fn eat(s: String) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n\
fn go<T: Sendable>(x: T) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    )
}

fn src(sig: &str, body: &str) -> String {
    format!("{HEAD}{DEFS}{}", func(sig, body))
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

use ErrorKind::{MoveFromRef, MovedInLoop, UseAfterMove};

// ---- 03 のエラー例 ----

#[test]
fn example_03_use_after_move() {
    let path = "examples/03-ownership/ownership.sigil";
    let text = std::fs::read_to_string(path).unwrap();
    let bad = text.replace(
        "// balance_of(&a);  // コンパイルエラー",
        "balance_of(&a);  // コンパイルエラー",
    );
    assert_ne!(text, bad);
    let e = check(&bad).unwrap_err();
    assert_eq!(e.kind, UseAfterMove, "{e}");
    let s = e.to_string();
    assert!(s.contains("`a`") && s.contains("69 行目"), "{s}");
}

// ---- 移動する位置と複製される位置 ----

#[test]
fn moves_at_let_assign_argument_return() {
    let sig = "f(a: Account) -> Int";
    assert_eq!(kind(sig, "  let b = a;\n  return close(a);"), UseAfterMove);
    assert_eq!(
        kind(sig, "  let mut b = open(1);\n  b = a;\n  return close(a);"),
        UseAfterMove
    );
    assert_eq!(
        kind(sig, "  let n = close(a);\n  return close(a);"),
        UseAfterMove
    );
    assert_eq!(
        kind(sig, "  let n = close(a);\n  return balance_of(&a);"),
        UseAfterMove
    );
    // 呼び出しの引数の 2 つ目の使いも移動の後
    assert_eq!(
        kind("f(s: String) -> Int", "  let n = eat(s);\n  return eat(s);"),
        UseAfterMove
    );
}

#[test]
fn moves_into_constructors_and_tuples() {
    let sig = "f(a: Account) -> Int";
    assert_eq!(
        kind(sig, "  let t = (a, 1);\n  return close(a);"),
        UseAfterMove
    );
    assert_eq!(
        kind(sig, "  let o = Some(a);\n  return close(a);"),
        UseAfterMove
    );
    assert_eq!(
        kind(
            "f(s: String) -> Int",
            "  let a = Account { balance: 1, owner: s };\n  return eat(s);"
        ),
        UseAfterMove
    );
}

#[test]
fn copyable_values_are_duplicated() {
    ok(
        "f(n: Int, t: (Int, Float)) -> Int",
        "  let a = n;\n  let b = n;\n  let c = t;\n  let d = t;\n  return go(n) + go(n) + a + b;",
    );
    // 借用は移動しない
    ok(
        "f(a: Account) -> Int",
        "  let x = balance_of(&a);\n  let y = balance_of(&a);\n  return close(a) + x + y;",
    );
}

#[test]
fn last_use_may_move() {
    ok(
        "f(a: Account) -> Int",
        "  let x = balance_of(&a);\n  return close(a);",
    );
}

// ---- 参照からの移動（PR #27 のレビュー） ----

#[test]
fn move_out_of_a_reference_is_rejected() {
    // 参照の名前を値の引数に渡す: Copyable なら複製、そうでなければ移動になるのでエラー
    ok("f(r: &Int) -> Int", "  return go(r);");
    assert_eq!(
        kind("f(r: &Account) -> Int", "  return go(r);"),
        MoveFromRef
    );
    assert_eq!(
        kind("f(r: &Account) -> Int", "  return close(r);"),
        MoveFromRef
    );
    // let・return・フィールド
    assert_eq!(
        kind("f(r: &Account) -> Int", "  let a = r;\n  return 0;"),
        MoveFromRef
    );
    assert_eq!(
        kind("f(r: &Account) -> String", "  return r.owner;"),
        MoveFromRef
    );
    assert_eq!(
        kind("f(r: &Account) -> Int", "  let s = r.owner;\n  return 0;"),
        MoveFromRef
    );
    // Copyable なフィールドは複製
    ok("f(r: &Account) -> Int", "  let n = r.balance;\n  return n;");
    assert!(msg("f(r: &Account) -> Int", "  return go(r);").contains(".clone()"));
}

#[test]
fn reference_types_do_not_satisfy_sendable() {
    // `Option<&Int>` は参照を含むので Sendable でない（ADR 0011 の決定 6）
    let s = src(
        "f(xs: &List<Int>) -> Int",
        "  let o = xs.get(0);\n  return go(o);",
    );
    assert_eq!(
        check(&s).unwrap_err().kind,
        ErrorKind::ConstraintUnsatisfied,
        "{s}"
    );
}

// ---- 分岐 ----

#[test]
fn branch_merge_is_possibly_moved() {
    let sig = "f(a: Account, c: Bool) -> Int";
    let one = "  if c {\n    let n = close(a);\n  }\n  return balance_of(&a);";
    assert_eq!(kind(sig, one), UseAfterMove);
    let m = msg(sig, one);
    assert!(m.contains("行目で移動"), "{m}");
    // else だけの移動、else if の連なり
    assert_eq!(
        kind(
            sig,
            "  if c {\n    let n = 1;\n  } else {\n    let n = close(a);\n  }\n  return balance_of(&a);"
        ),
        UseAfterMove
    );
    assert_eq!(
        kind(
            "f(a: Account, c: Bool, d: Bool) -> Int",
            "  if c {\n    let n = 1;\n  } else if d {\n    let n = close(a);\n  }\n  return balance_of(&a);"
        ),
        UseAfterMove
    );
    // 別の枝では使える
    ok(
        sig,
        "  if c {\n    return close(a);\n  } else {\n    return balance_of(&a);\n  }",
    );
}

#[test]
fn paths_after_early_return_are_not_merged() {
    let sig = "f(a: Account, c: Bool) -> Int";
    ok(
        sig,
        "  if c {\n    return close(a);\n  }\n  return balance_of(&a);",
    );
    ok(
        sig,
        "  if c {\n    let n = close(a);\n    return n;\n  } else {\n    let n = 1;\n  }\n  return close(a);",
    );
}

#[test]
fn reassign_in_every_branch_reinitializes() {
    ok(
        "f(c: Bool) -> Int",
        "  let mut a = open(1);\n  let n = close(a);\n  if c {\n    a = open(2);\n  } else {\n    a = open(3);\n  }\n  return close(a);",
    );
    // 片方だけでは足りない
    assert_eq!(
        kind(
            "f(c: Bool) -> Int",
            "  let mut a = open(1);\n  let n = close(a);\n  if c {\n    a = open(2);\n  }\n  return close(a);"
        ),
        UseAfterMove
    );
}

#[test]
fn value_match_arms_merge() {
    let sig = "f(a: Account, k: Int) -> Int";
    assert_eq!(
        kind(
            sig,
            "  let n = match k {\n    0 => close(a),\n    _ => 1,\n  };\n  return balance_of(&a);"
        ),
        UseAfterMove
    );
    ok(
        sig,
        "  let n = match k {\n    0 => balance_of(&a),\n    _ => 1,\n  };\n  return close(a) + n;",
    );
    // 文の match
    assert_eq!(
        kind(
            sig,
            "  match k {\n    0 => {\n      let n = close(a);\n    }\n    _ => {\n      let n = 1;\n    }\n  }\n  return balance_of(&a);"
        ),
        UseAfterMove
    );
    ok(
        sig,
        "  match k {\n    0 => {\n      return close(a);\n    }\n    _ => {\n      let n = 1;\n    }\n  }\n  return close(a);",
    );
}

// ---- ループ ----

#[test]
fn move_in_loop_body_is_reused_next_iteration() {
    let sig = "f(a: Account, c: Bool) -> Int";
    assert_eq!(
        kind(sig, "  while c {\n    let n = close(a);\n  }\n  return 0;"),
        MovedInLoop
    );
    let m = msg(sig, "  while c {\n    let n = close(a);\n  }\n  return 0;");
    assert!(m.contains("行目") && m.contains("`a`"), "{m}");
    // 本体の中で再代入すれば次の周でも使える
    ok(
        "f(c: Bool) -> Int",
        "  let mut a = open(1);\n  while c {\n    let n = close(a);\n    a = open(2);\n  }\n  return close(a);",
    );
    // 移動の後に戻る経路は周に戻らない
    ok(sig, "  while c {\n    return close(a);\n  }\n  return 0;");
    // 本体で宣言した変数は周ごとに新しい
    ok(
        "f(c: Bool) -> Int",
        "  while c {\n    let b = open(1);\n    let n = close(b);\n  }\n  return 0;",
    );
}

#[test]
fn use_in_loop_of_value_moved_before() {
    assert_eq!(
        kind(
            "f(a: Account, c: Bool) -> Int",
            "  let n = close(a);\n  while c {\n    let m = balance_of(&a);\n  }\n  return n;"
        ),
        UseAfterMove
    );
    // ループの前に移動し、ループの中で再代入してから使うのは通る。抜けた後は移動済みのまま。
    ok(
        "f(c: Bool) -> Int",
        "  let mut a = open(1);\n  let n = close(a);\n  while c {\n    a = open(2);\n    let m = balance_of(&a);\n  }\n  return n;",
    );
    assert_eq!(
        kind(
            "f(c: Bool) -> Int",
            "  let mut a = open(1);\n  let n = close(a);\n  while c {\n    a = open(2);\n  }\n  return balance_of(&a);"
        ),
        UseAfterMove
    );
}

#[test]
fn move_inside_nested_branch_in_loop() {
    assert_eq!(
        kind(
            "f(a: Account, c: Bool, d: Bool) -> Int",
            "  while c {\n    if d {\n      let n = close(a);\n    }\n  }\n  return 0;"
        ),
        MovedInLoop
    );
}

// ---- 部分的な移動 ----

#[test]
fn partial_move_of_a_field() {
    let sig = "f(a: Account) -> Int";
    // 他のフィールドは使える
    ok(
        sig,
        "  let s = a.owner;\n  let n = a.balance;\n  return n + eat(s);",
    );
    // そのフィールドと全体は使えない
    assert_eq!(
        kind(sig, "  let s = a.owner;\n  return eat(a.owner);"),
        UseAfterMove
    );
    assert_eq!(
        kind(sig, "  let s = a.owner;\n  return close(a);"),
        UseAfterMove
    );
    assert_eq!(
        kind(sig, "  let s = a.owner;\n  return balance_of(&a);"),
        UseAfterMove
    );
    // 全体を移動した後はフィールドも使えない
    assert_eq!(
        kind(sig, "  let n = close(a);\n  return a.balance;"),
        UseAfterMove
    );
    let m = msg(sig, "  let s = a.owner;\n  return close(a);");
    assert!(m.contains("一部 `a.owner`"), "{m}");
}

#[test]
fn partial_move_by_return_is_allowed() {
    ok("f(a: Account) -> String", "  return a.owner;");
}

#[test]
fn field_reassignment_reinitializes() {
    ok(
        "f() -> Int",
        "  let mut a = open(1);\n  let s = a.owner;\n  a.owner = \"x\";\n  return close(a) + eat(s);",
    );
    // 全体が移動済みの変数のフィールドへは代入できない。変数全体なら初期化し直せる。
    assert_eq!(
        kind(
            "f() -> Int",
            "  let mut a = open(1);\n  let n = close(a);\n  a.balance = 2;\n  return n;"
        ),
        UseAfterMove
    );
    ok(
        "f() -> Int",
        "  let mut a = open(1);\n  let n = close(a);\n  a = open(2);\n  return close(a) + n;",
    );
}

// ---- match による移動 ----

#[test]
fn match_binding_noncopyable_parts_moves_the_target() {
    let sig = "f(o: Option<String>) -> Int";
    assert_eq!(
        kind(
            sig,
            "  match o {\n    Some(s) => {\n      let n = eat(s);\n    }\n    None => {\n      let n = 0;\n    }\n  }\n  return go(o);"
        ),
        UseAfterMove
    );
    // 値の match も同じ
    assert_eq!(
        kind(
            sig,
            "  let n = match o {\n    Some(s) => eat(s),\n    None => 0,\n  };\n  return go(o);"
        ),
        UseAfterMove
    );
}

#[test]
fn match_with_copyable_bindings_does_not_move() {
    ok(
        "f(o: Option<Int>) -> Int",
        "  let n = match o {\n    Some(k) => k,\n    None => 0,\n  };\n  return go(o) + n;",
    );
    // 束縛しない（`_`）だけなら移動しない
    ok(
        "f(o: Option<String>) -> Int",
        "  let n = match o {\n    Some(_) => 1,\n    None => 0,\n  };\n  return go(o) + n;",
    );
}

#[test]
fn match_on_borrow_or_reference_does_not_move() {
    ok(
        "f(o: Option<String>) -> Int",
        "  let n = match &o {\n    Some(s) => 1,\n    None => 0,\n  };\n  return go(o) + n;",
    );
    ok(
        "f(o: &Option<String>) -> Int",
        "  let n = match o {\n    Some(s) => 1,\n    None => 0,\n  };\n  return go(1) + n;",
    );
    // 参照を通した場所からは移動できないので、束縛は参照になって通る
    ok(
        "f(o: &Option<String>) -> Int",
        "  match o {\n    Some(s) => {\n      return 1;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 2;",
    );
}

#[test]
fn match_on_a_call_result_is_not_a_place() {
    ok(
        "f(xs: &List<Int>) -> Int",
        "  return match xs.get(0) {\n    Some(n) => n,\n    None => 0,\n  };",
    );
}

// ---- 範囲外（クロージャ・契約・早期の戻り） ----

#[test]
fn moved_value_cannot_be_used_in_a_closure() {
    assert_eq!(
        kind(
            "f(a: Account) -> Int",
            "  let n = close(a);\n  let g = move || balance_of(&a);\n  return n;"
        ),
        UseAfterMove
    );
}

#[test]
fn contracts_do_not_move() {
    let s = format!(
        "{HEAD}{DEFS}fn f(s: String) -> Int\n  intent \"x\"\n  requires eat(s) == 0\n  ensures none\n  effects none\n{{\n  return eat(s);\n}}\n"
    );
    if let Err(e) = check(&s) {
        panic!("{e}\n{s}");
    }
}

#[test]
fn try_operator_does_not_merge_the_early_return_path() {
    ok(
        "f(a: Account, o: Option<Int>) -> Option<Int>",
        "  let n = o?;\n  let k = close(a);\n  return Some(n + k);",
    );
}

#[test]
fn shadowing_names_in_sibling_scopes_do_not_leak() {
    ok(
        "f(c: Bool) -> Int",
        "  if c {\n    let a = open(1);\n    let n = close(a);\n  }\n  if c {\n    let a = open(2);\n    return balance_of(&a);\n  }\n  return 0;",
    );
}

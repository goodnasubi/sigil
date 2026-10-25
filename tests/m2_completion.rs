//! M2 の完了条件（ADR 0006 の M2、ADR 0012 の「M2 の完了条件に加えるもの」）の対応テスト。
//! 各テストの名前は ADR 0012 の表から参照される。個々の規則の詳しいテストは moves.rs・borrows.rs・
//! closures.rs・refs.rs にあり、ここには完了条件の項目ごとに 1 件ずつ、まとめて置く。

use sigil::check::check;
use sigil::ErrorKind;
use ErrorKind::*;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    )
}

fn defs() -> String {
    let mut s = String::from(
        "type Port = Int where 1 <= self && self <= 65535;\nstruct Account {\n  balance: Int,\n  owner: String,\n}\nstruct Pair {\n  x: Int,\n  y: Int,\n}\n",
    );
    for (sig, body) in [
        ("close(a: Account) -> Int", "  return a.balance;"),
        (
            "open(n: Int) -> Account",
            "  return Account { balance: n, owner: \"o\" };",
        ),
        ("eat(s: String) -> Int", "  return 0;"),
        ("show(n: &Int)", "  return;"),
        ("both(a: &mut Int, b: &Int)", "  return;"),
        ("bump(a: &mut Int, n: Int)", "  return;"),
        ("take(a: &mut Account, b: Account)", "  return;"),
        ("base_mut(n: &mut Int)", "  return;"),
        ("base(n: &Int)", "  return;"),
        ("pick(a: &Account) -> &Account", "  return a;"),
        (
            "mk() -> Account",
            "  return Account { balance: 1, owner: \"a\" };",
        ),
        ("make() -> List<Account>", "  return List.new();"),
        (
            "first(xs: &List<Account>) -> Option<&Account>",
            "  return xs.get(0);",
        ),
        ("opt(o: &Option<Account>) -> Int", "  return 0;"),
    ] {
        s += &func(sig, body);
    }
    s += "fn run(f: fn(Int) -> Int, n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(f)\n{\n  return f(n);\n}\n";
    s
}

fn src(sig: &str, body: &str) -> String {
    format!("{HEAD}{}{}", defs(), func(sig, body))
}

fn kind(sig: &str, body: &str) -> ErrorKind {
    let s = src(sig, body);
    match check(&s) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{sig}\n{body}"),
        Err(e) => e.kind,
    }
}

fn ok(sig: &str, body: &str) {
    if let Err(e) = check(&src(sig, body)) {
        panic!("{e}\n{sig}\n{body}");
    }
}

fn found(sig: &str, body: &str) -> String {
    check(&src(sig, body)).unwrap_err().to_string()
}

// ---- エラーになる: 移動 ----

#[test]
fn err_move_if_one_arm() {
    assert_eq!(
        kind(
            "f(a: Account, c: Bool) -> Int",
            "  if c {\n    let n = close(a);\n  }\n  return close(a);"
        ),
        UseAfterMove
    );
}

#[test]
fn err_move_in_while_body() {
    assert_eq!(
        kind(
            "f(a: Account, c: Bool)",
            "  while c {\n    let n = close(a);\n  }"
        ),
        MovedInLoop
    );
}

#[test]
fn err_move_match_binding_then_use_target() {
    assert_eq!(
        kind(
            "f(o: Option<Account>) -> Int",
            "  match o {\n    Some(a) => {\n      let n = close(a);\n    }\n    None => {\n      let n = 0;\n    }\n  }\n  return opt(&o);"
        ),
        UseAfterMove
    );
}

#[test]
fn err_move_field_through_reference() {
    assert_eq!(
        kind("f(a: &Account) -> String", "  return a.owner;"),
        MoveFromRef
    );
}

#[test]
fn err_whole_use_after_partial_move() {
    assert_eq!(
        kind(
            "f(a: Account) -> Int",
            "  let s = a.owner;\n  return close(a);"
        ),
        UseAfterMove
    );
}

// ---- エラーになる: 借用 ----

#[test]
fn err_let_borrow() {
    assert_eq!(kind("f(n: Int)", "  let r = &n;"), BorrowPosition);
}

#[test]
fn err_mut_and_shared_in_one_call() {
    assert_eq!(
        kind("f(a: Int)", "  let mut m = a;\n  both(&mut m, &m);"),
        BorrowOverlap
    );
}

#[test]
fn err_mut_and_move_in_one_call() {
    assert_eq!(
        kind("f(b: Account)", "  let mut a = mk();\n  take(&mut a, a);"),
        BorrowOverlap
    );
}

#[test]
fn err_push_len_with_fix_in_message() {
    let m = found(
        "f(x: Int)",
        "  let mut ys = List.new();\n  ys.push(ys.len());",
    );
    assert!(m.contains("let n = xs.len(); xs.push(n);"), "{m}");
    assert_eq!(
        kind(
            "f(x: Int)",
            "  let mut ys = List.new();\n  ys.push(ys.len());"
        ),
        BorrowOverlap
    );
}

#[test]
fn err_mut_borrow_of_immutable_variable() {
    assert_eq!(
        kind("f(x: Int)", "  let m = x;\n  base_mut(&mut m);"),
        BorrowMutOfImmutable
    );
}

#[test]
fn err_get_then_push() {
    assert_eq!(
        kind(
            "f(a: Account)",
            "  let mut xs = make();\n  let r = xs.get(0);\n  xs.push(a);\n  let b = r.is_some();"
        ),
        BorrowConflict
    );
}

#[test]
fn err_unwrap_or_with_option_of_ref() {
    assert_eq!(
        kind(
            "f(xs: &List<Account>, d: &Account)",
            "  let r = xs.get(0).unwrap_or(&d);"
        ),
        RefSubstitution
    );
}

#[test]
fn err_ok_or_with_option_of_ref() {
    assert_eq!(
        kind("f(xs: &List<Account>)", "  let r = xs.get(0).ok_or(1);"),
        RefSubstitution
    );
}

#[test]
fn err_return_ref_not_rooted_at_borrow_argument() {
    assert_eq!(
        kind("f(n: &Int, k: Int) -> &Int", "  let m = k;\n  return m;"),
        ReturnBorrowRoot
    );
}

#[test]
fn err_let_mut_option_of_ref() {
    assert_eq!(
        kind("f(xs: &List<Int>)", "  let mut r = xs.get(0);"),
        LetMutRef
    );
}

#[test]
fn err_ref_to_option_of_ref_declaration() {
    let s = format!("{HEAD}{}", func("f(x: &Option<&Int>)", "  return;"));
    assert_eq!(check(&s).unwrap_err().kind, RefTypePosition);
}

#[test]
fn err_push_while_returned_ref_alive_transitively() {
    assert_eq!(
        kind(
            "f(a: Account)",
            "  let mut xs = make();\n  let r = first(&xs);\n  let q = r;\n  xs.push(a);\n  let b = q.is_some();"
        ),
        BorrowConflict
    );
}

// ---- エラーになる: match &x ----

#[test]
fn err_match_ref_of_reference_name() {
    let body = "  match &o {\n    Some(a) => {\n      let n = 1;\n    }\n    None => {\n      let n = 0;\n    }\n  }";
    let sig = "f(o: &Option<Account>)";
    let m = found(sig, body);
    assert!(m.contains('&'), "{m}");
    let _ = kind(sig, body);
}

// ---- エラーになる: 一時的な値 ----

#[test]
fn err_temp_borrow_first_of_make() {
    assert_eq!(
        kind("f()", "  let r = first(&make());\n  let b = r.is_some();"),
        TempBorrow
    );
}

#[test]
fn err_temp_get_of_make() {
    assert_eq!(
        kind("f()", "  let r = make().get(0);\n  let b = r.is_some();"),
        TempBorrow
    );
}

#[test]
fn err_stmt_match_ref_temp_binding_in_block() {
    assert_eq!(
        kind(
            "f()",
            "  match &Some(mk()) {\n    Some(x) => {\n      let b = x.balance;\n    }\n    None => {\n      let b = 0;\n    }\n  }"
        ),
        TempBorrow
    );
}

#[test]
fn err_return_ref_of_temp() {
    assert_eq!(
        kind(
            "f(xs: &List<Account>) -> Option<&Account>",
            "  return make().get(0);"
        ),
        ReturnBorrowRoot
    );
}

#[test]
fn err_match_get_of_temp_binding_in_block() {
    assert_eq!(
        kind(
            "f()",
            "  match make().get(0) {\n    Some(a) => {\n      let b = a.balance;\n    }\n    None => {\n      let b = 0;\n    }\n  }"
        ),
        TempBorrow
    );
}

// ---- エラーになる: クロージャ・篩型・契約 ----

#[test]
fn err_closure_without_move_escaping() {
    assert_eq!(kind("f()", "  let g = |a| a + 1;"), ClosureNeedsMove);
}

#[test]
fn err_move_closure_captures_reference() {
    assert_eq!(
        kind(
            "f(r: &Int) -> Int",
            "  let g = move |a| a + r;\n  return g(1);"
        ),
        ClosureCaptureRef
    );
}

#[test]
fn err_closure_moves_captured_value() {
    assert_eq!(
        kind("f(s: String) -> Int", "  return run(|a| eat(s) + a, 1);"),
        ClosureMoveCaptured
    );
}

#[test]
fn err_refined_mut_borrow() {
    assert_eq!(
        kind("f(p: Port)", "  let mut q = p;\n  base_mut(&mut q);"),
        RefineMutBorrow
    );
}

#[test]
fn err_requires_moves_value() {
    let s = format!(
        "{HEAD}{}fn f(s: String) -> Int\n  intent \"x\"\n  requires eat(s) == 0\n  ensures none\n  effects none\n{{\n  return 0;\n}}\n",
        defs()
    );
    assert_eq!(check(&s).unwrap_err().kind, ContractMove);
}

#[test]
fn err_ensures_value_moved_by_body() {
    let s = format!(
        "{HEAD}{}fn f(a: Account) -> Int\n  intent \"x\"\n  requires none\n  ensures a.balance == result\n  effects none\n{{\n  let n = a.balance;\n  let t = a;\n  return n;\n}}\n",
        defs()
    );
    assert_eq!(check(&s).unwrap_err().kind, EnsuresMoved);
}

// ---- 通る ----

#[test]
fn pass_disjoint_fields() {
    ok(
        "f(a: Int)",
        "  let mut p = Pair { x: a, y: a };\n  both(&mut p.x, &p.y);",
    );
}

#[test]
fn pass_match_on_get_of_temp_value_only() {
    ok(
        "f() -> Int",
        "  let n = match make().get(0) {\n    Some(a) => a.balance,\n    None => 0,\n  };\n  return n;",
    );
}

#[test]
fn pass_match_ref_of_let_variable() {
    ok(
        "f() -> Int",
        "  let x = Some(1);\n  match &x {\n    Some(n) => {\n      let k = n + 1;\n    }\n    None => {\n      let k = 0;\n    }\n  }\n  let y = x;\n  return 1;",
    );
}

#[test]
fn pass_option_of_ref_combinators() {
    ok(
        "f(xs: &List<Account>) -> Bool",
        "  let m = xs.get(0).map(|a| a.balance);\n  let b = xs.get(0).is_some();\n  let c = xs.max_by(|a| a.balance);\n  return b && c.is_some();",
    );
}

#[test]
fn pass_if_both_arms_move_then_reassign() {
    ok(
        "f(c: Bool) -> Int",
        "  let mut a = mk();\n  if c {\n    let n = close(a);\n  } else {\n    let n = close(a);\n  }\n  a = mk();\n  return close(a);",
    );
}

#[test]
fn pass_return_partial_move() {
    ok("f(a: Account) -> String", "  return a.owner;");
}

#[test]
fn pass_push_after_last_use_of_get() {
    ok(
        "f(a: Account)",
        "  let mut xs = make();\n  let r = xs.get(0);\n  let b = r.is_some();\n  xs.push(a);",
    );
}

#[test]
fn pass_refined_shared_borrow_as_base() {
    ok("f(p: Port)", "  base(&p);");
}

#[test]
fn pass_console_log_literal_ref() {
    let s = "module m.a\n  targets server\n  trust none\n\nfn f()\n  intent \"x\"\n  requires none\n  ensures none\n  effects console\n{\n  console.log(&\"text\");\n}\n";
    if let Err(e) = check(s) {
        panic!("{e}");
    }
}

// ---- 決定 2 のサンプル・決定 5・03 のエラー例 ----

#[test]
fn examples_pass_all_checks() {
    for p in [
        "examples/01-average/c-brace.sigil",
        "examples/02-read-port/c-brace.sigil",
        "examples/03-ownership/ownership.sigil",
        "examples/04-effects/effects.sigil",
        "examples/05-control/control.sigil",
    ] {
        let text = std::fs::read_to_string(p).unwrap();
        if let Err(e) = check(&text) {
            panic!("{p}: {e}");
        }
    }
}

#[test]
fn stdlib_function_name_redeclared() {
    let s = format!(
        "{HEAD}{}",
        func("to_float(x: Int) -> Float", "  return 1.0;")
    );
    assert_eq!(check(&s).unwrap_err().kind, ReservedNameDefined);
}

fn example03(from: &str, to: &str) -> sigil::Error {
    let text = std::fs::read_to_string("examples/03-ownership/ownership.sigil").unwrap();
    let bad = text.replace(from, to);
    assert_ne!(text, bad);
    check(&bad).unwrap_err()
}

#[test]
fn example_03_error_use_after_move() {
    let e = example03("// balance_of(&a);", "balance_of(&a);");
    assert_eq!(e.kind, UseAfterMove, "{e}");
}

#[test]
fn example_03_error_reference_in_struct() {
    let e = example03(
        "// struct View {\n//   account: &Account,",
        "struct View {\n  account: &Account,\n}\n//",
    );
    assert_eq!(e.kind, RefTypePosition, "{e}");
}

#[test]
fn example_03_error_two_borrow_args_return() {
    let e = example03(
        "// fn pick(x: &Account, y: &Account) -> &Account",
        "fn pick(x: &Account, y: &Account) -> &Account\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{ return x; }\n//",
    );
    assert_eq!(e.kind, RefReturnBorrowCount, "{e}");
}

/// ADR 0006 条件 1（M2）: Copyable の規則を差し替えると、移動か複製かの判定が変わる。
#[test]
fn rule_override_changes_move_or_copy() {
    use sigil::check::check_with;
    use sigil::effects::Options;
    let s = format!(
        "{HEAD}{}{}",
        defs(),
        func(
            "f() -> Int",
            "  let a = Pair { x: 1, y: 2 };\n  let b = a;\n  let c = a;\n  return b.x + c.y;"
        )
    );
    // 既定では struct は Copyable でないので、2 回目の `a` は移動後の使用になる。
    assert_eq!(check(&s).unwrap_err().kind, UseAfterMove);
    let mut o = Options::default();
    o.copy_override.push(("Pair".into(), true));
    assert!(check_with(&s, &o).is_ok());
}

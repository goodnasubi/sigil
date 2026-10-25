//! M2 の PR4（ADR 0012 の決定 1 の 5〜8）: クロージャのとらえ方・篩型の借用・契約の式の規則・
//! 条件 3 の API・`let` のローカルの再検査・クロージャの中の検査。

use sigil::check::check;
use sigil::check::{closure_captures, CapMode};
use sigil::ErrorKind;
use ErrorKind::{
    BorrowConflict, BorrowOverlap, ClosureCaptureRef, ClosureMoveCaptured, ClosureNeedsMove,
    ContractMove, EnsuresMoved, LetMutRef, RefineMutBorrow, TempBorrow, UseAfterMove,
};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

const DEFS: &str = "fn pick(a: &Account) -> &Account\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a;\n}\nfn mk() -> Account\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return Account { balance: 1, owner: \"a\" };\n}\nfn make() -> List<Account>\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return List.new();\n}\ntype Port = Int where 1 <= self && self <= 65535;\nstruct Account {\n  balance: Int,\n  owner: String,\n}\nfn eat(s: String) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\nfn size(s: &String) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\nfn show(n: &Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn both(a: &mut Int, b: &Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn base(n: &Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn base_mut(n: &mut Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn anyv<T>() -> T\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return anyv();\n}\nfn rdg<T>(a: &T) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\nfn run(f: fn(Int) -> Int, n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(f)\n{\n  return f(n);\n}\nfn run_mut(f: fn(Int) -> Int, a: &mut Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(f)\n{\n  return f(1);\n}\n";

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

fn full(sig: &str, requires: &str, ensures: &str, body: &str) -> String {
    format!(
        "{HEAD}{DEFS}fn {sig}\n  intent \"x\"\n  requires {requires}\n  ensures {ensures}\n  effects none\n{{\n{body}\n}}\n"
    )
}

fn full_kind(sig: &str, requires: &str, ensures: &str, body: &str) -> ErrorKind {
    let s = full(sig, requires, ensures, body);
    match check(&s) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{s}"),
        Err(e) => e.kind,
    }
}

// ---- move の要求 ----

#[test]
fn escaping_closure_needs_move() {
    assert_eq!(kind("f()", "  let g = |a| a + 1;"), ClosureNeedsMove);
    ok("f()", "  let g = move |a| a + 1;");
    // 何もとらえなくても位置で決まる
    assert_eq!(
        kind("f()", "  let g = |a| a;\n  let h = g;"),
        ClosureNeedsMove
    );
    // 呼び出しの引数の位置なら要らない
    ok("f(k: Int) -> Int", "  return run(|a| a + k, 1);");
}

// ---- とらえ方 ----

#[test]
fn escaping_closure_cannot_capture_a_reference() {
    assert_eq!(
        kind(
            "f(r: &Int) -> Int",
            "  let g = move |a| a + r;\n  return g(1);"
        ),
        ClosureCaptureRef
    );
    // 値なら複製して持つ
    ok(
        "f(r: Int) -> Int",
        "  let g = move |a| a + r;\n  return g(1);",
    );
    // 呼び出しの引数の位置なら参照をとらえてよい
    ok("f(r: &Int) -> Int", "  return run(|a| a + r, 1);");
}

#[test]
fn move_closure_moves_noncopyable_capture() {
    assert_eq!(
        kind(
            "f(s: String) -> Int",
            "  let g = move |a| a + size(&s);\n  return size(&s);"
        ),
        UseAfterMove
    );
    // 複製して持つ Copyable は、あとでも使える
    ok(
        "f(n: Int) -> Int",
        "  let g = move |a| a + n;\n  return n + g(1);",
    );
}

#[test]
fn closure_cannot_move_a_captured_value() {
    assert_eq!(
        kind("f(s: String) -> Int", "  return run(|a| eat(s) + a, 1);"),
        ClosureMoveCaptured
    );
    assert_eq!(
        kind("f(s: String)", "  let g = move |a| eat(s) + a;"),
        ClosureMoveCaptured
    );
    // 自分の引数や中の束縛は移動してよい
    ok(
        "f() -> Int",
        "  return run(|a| { let t = \"x\"; return eat(t) + a; }, 1);",
    );
}

#[test]
fn nonescaping_closure_loans_overlap_with_other_arguments() {
    // クロージャが書き換える値を、同じ呼び出しのほかの引数が借りる
    assert_eq!(
        kind(
            "f()",
            "  let mut m = 0;\n  run_mut(|a| { m = a; return a; }, &mut m);"
        ),
        BorrowOverlap
    );
    ok(
        "f() -> Int",
        "  let mut m = 0;\n  let r = run(|a| { m = a; return a; }, 1);\n  return m + r;",
    );
}

// ---- 篩型の借用 ----

#[test]
fn refined_borrow() {
    ok("f(p: Port)", "  base(&p);");
    assert_eq!(
        kind("f(p: Port)", "  let mut q = p;\n  base_mut(&mut q);"),
        RefineMutBorrow
    );
    assert_eq!(
        kind("f(p: &mut Port)", "  base_mut(&mut p);"),
        RefineMutBorrow
    );
    ok("f(p: &Port)", "  base(&p);");
}

// ---- 契約の式 ----

#[test]
fn contracts_do_not_move_values() {
    assert_eq!(
        full_kind("f(s: String) -> Int", "eat(s) == 0", "none", "  return 0;"),
        ContractMove
    );
    assert_eq!(
        full_kind(
            "f(s: String) -> Int",
            "none",
            "eat(s) == result",
            "  return 0;"
        ),
        ContractMove
    );
    // 参照で渡すのは構わない。Copyable の値も構わない
    let s = full(
        "f(s: String, n: Int) -> Int",
        "size(&s) == 0",
        "result == n",
        "  return n;",
    );
    check(&s).unwrap_or_else(|e| panic!("{e}\n{s}"));
}

#[test]
fn ensures_value_not_moved_by_body() {
    // 値の引数を ensures が参照するなら、本体で移動してはならない
    assert_eq!(
        full_kind(
            "f(a: Account) -> Int",
            "none",
            "a.balance == result",
            "  let n = a.balance;\n  let t = a;\n  return n;"
        ),
        EnsuresMoved
    );
    // old(..) の中は入りの時点の値なので構わない
    let s = full(
        "f(a: Account) -> Int",
        "none",
        "old(a.balance) == result",
        "  let n = a.balance;\n  let t = a;\n  return n;",
    );
    check(&s).unwrap_or_else(|e| panic!("{e}\n{s}"));
    // 参照の引数は移動できない
    let s = full(
        "f(a: &Account) -> Int",
        "none",
        "a.balance == result",
        "  return a.balance;",
    );
    check(&s).unwrap_or_else(|e| panic!("{e}\n{s}"));
}

// ---- 条件 3 の API ----

#[test]
fn closure_captures_api() {
    let s = src(
        "f(n: Int, s: String, r: &Int) -> Int",
        "  let g = move |a| a + n;\n  return run(|a| a + r + size(&s), 1);",
    );
    let cs = closure_captures(&s).unwrap();
    assert_eq!(cs.len(), 2, "{cs:?}");
    let g = &cs[0];
    assert!(g.escaping && g.is_move);
    assert_eq!(g.bound.as_deref(), Some("g"));
    assert_eq!(g.func, "f");
    assert_eq!(g.captures.len(), 1);
    assert_eq!(g.captures[0].name, "n");
    assert_eq!(g.captures[0].mode, CapMode::Copy);
    assert_eq!(g.satisfies("Copyable"), Some(true));
    let h = &cs[1];
    assert!(!h.escaping && !h.is_move);
    assert_eq!(h.captures.len(), 2);
    assert!(h
        .captures
        .iter()
        .all(|c| c.mode == CapMode::Ref || c.mode == CapMode::Copy));
}

#[test]
fn move_closure_with_noncopyable_capture_is_not_copyable() {
    let s = src(
        "f(s: String) -> Int",
        "  let g = move |a| a + size(&s);\n  return 0;",
    );
    let cs = closure_captures(&s).unwrap();
    assert_eq!(cs[0].captures[0].mode, CapMode::Move);
    assert_eq!(cs[0].satisfies("Copyable"), Some(false));
}

// ---- let のローカルの再検査 ----

#[test]
fn let_local_revalidated_after_types_settle() {
    assert_eq!(
        kind("f(n: Int)", "  let mut r = List.new();\n  r.push(&n);"),
        ErrorKind::RefSubstitution
    );
    // 型が後で参照を含むと決まる
    assert_eq!(
        kind("f(xs: &List<Int>)", "  let mut r = None;\n  r = xs.get(0);"),
        LetMutRef
    );
    ok("f(xs: &List<Int>)", "  let r = xs.get(0);");
}

// ---- クロージャの中の検査 ----

#[test]
fn checks_run_inside_closures() {
    // 腕の束縛を借りた参照は、クロージャの中でも腕の外へ出せない
    assert_eq!(
        kind(
            "f(a: &Account) -> Int",
            "  return run(|k| { let n = match mk() { x => pick(&x), }; return n.balance + k; }, 1);"
        ),
        TempBorrow
    );
    ok(
        "f(a: &Account) -> Int",
        "  return run(|k| { let n = match mk() { x => pick(&x).balance, }; return n + k; }, 1);",
    );
    // 返された参照の区間も、クロージャの中で追う
    assert_eq!(
        kind(
            "f(a: Account) -> Int",
            "  return run(|k| { let mut xs = make(); let r = xs.get(0); xs.push(mk()); return k + r.len(); }, 1);"
        ),
        BorrowConflict
    );
    // クロージャの外の変数を借りた参照の区間は、クロージャの本体の外へ持ち出せる形ではない
    ok(
        "f(a: Account) -> Int",
        "  return run(|k| { let mut xs = make(); let r = xs.get(0); let b = r.is_some(); xs.push(mk()); return k; }, 1);",
    );
}

#[test]
fn unresolved_capture_is_treated_as_move() {
    // 型が決まらない値を move のクロージャがとらえたら、移動の疑いとして記録する
    let body =
        "  let x = anyv();\n  let g = move |a| a + rdg(&x);\n  let w = eat(x);\n  return g(1) + w;";
    assert_eq!(kind("f() -> Int", body), UseAfterMove);
    // 型が決まっていても同じ結果
    assert_eq!(
        kind(
            "f() -> Int",
            "  let x: String = anyv();\n  let g = move |a| a + rdg(&x);\n  let w = eat(x);\n  return g(1) + w;"
        ),
        UseAfterMove
    );
    // 決まってみると Copyable なら通る
    ok(
        "f() -> Int",
        "  let x = anyv();\n  let g = move |a| a + rdg(&x);\n  let w = x;\n  return g(1) + w + x;",
    );
}

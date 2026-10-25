//! `src/borrow.rs` の単体テスト（出来事の列を `regions` に渡す）の各ケースと、同じ判定になる
//! Sigil のソースのテスト（ADR 0013 の決定 4）。単体テストが部品の規則を、こちらが検査器を通した
//! 結果を確かめる。ケースの名前は単体テストと同じで、判定がずれたらどちらかが落ちる。

use sigil::check::check;
use sigil::ErrorKind;
use ErrorKind::{BorrowConflict, BorrowOverlap, UseAfterMove};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

const DEFS: &str = "struct Account {\n  balance: Int,\n  owner: String,\n}\nstruct Pt {\n  a: Int,\n  b: Int,\n}\nfn show(n: &Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn change(n: &mut Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn both(a: &mut Int, b: &Int)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nfn pick(a: &Int) -> &Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a;\n}\nfn pick_mut(a: &mut Int) -> &mut Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a;\n}\nfn pick_acc(a: &Account) -> &Account\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a;\n}\nfn look(a: &Account)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\n";

fn run(sig: &str, body: &str) -> Result<(), ErrorKind> {
    let s = format!(
        "{HEAD}{DEFS}fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    );
    check(&s).map_err(|e| e.kind)
}

fn ok(sig: &str, body: &str) {
    if let Err(k) = run(sig, body) {
        panic!("通るはずがエラー {k:?}:\n{body}");
    }
}

fn err(sig: &str, body: &str, want: ErrorKind) {
    assert_eq!(run(sig, body), Err(want), "{body}");
}

#[test]
fn overlap_is_prefix_on_field_boundary() {
    // 同じ場所・親と子は重なる
    err(
        "f()",
        "  let mut x = 1;\n  both(&mut x, &x);",
        BorrowOverlap,
    );
    err(
        "f()",
        "  let mut p = Pt { a: 1, b: 2 };\n  both(&mut p.a, &p.a);",
        BorrowOverlap,
    );
    // 別のフィールドは重ならない
    ok(
        "f()",
        "  let mut p = Pt { a: 1, b: 2 };\n  both(&mut p.a, &p.b);",
    );
    // 名前が前方一致するだけの別の変数は重ならない
    ok(
        "f()",
        "  let mut a = 1;\n  let ab = 2;\n  both(&mut a, &ab);",
    );
}

#[test]
fn exclusive_borrow_is_fine_after_last_use_of_shared() {
    ok(
        "f()",
        "  let mut x = 1;\n  let r = pick(&x);\n  show(&r);\n  let m = pick_mut(&mut x);\n  change(&mut m);",
    );
}

#[test]
fn exclusive_borrow_while_shared_is_live_conflicts() {
    err(
        "f()",
        "  let mut x = 1;\n  let r = pick(&x);\n  let m = pick_mut(&mut x);\n  change(&mut m);\n  show(&r);",
        BorrowConflict,
    );
}

#[test]
fn shared_borrows_and_reads_coexist() {
    ok(
        "f()",
        "  let x = 1;\n  let r = pick(&x);\n  let s = pick(&x);\n  let y = x;\n  show(&r);\n  show(&s);",
    );
}

#[test]
fn read_kills_exclusive_loan() {
    err(
        "f()",
        "  let mut x = 1;\n  let m = pick_mut(&mut x);\n  let y = x;\n  change(&mut m);",
        BorrowConflict,
    );
}

#[test]
fn write_and_move_kill_shared_loan() {
    err(
        "f()",
        "  let mut x = 1;\n  let r = pick(&x);\n  x = 2;\n  show(&r);",
        BorrowConflict,
    );
    err(
        "f(a: Account)",
        "  let r = pick_acc(&a);\n  let b = a;\n  look(&r);",
        BorrowConflict,
    );
}

#[test]
fn disjoint_fields_do_not_conflict() {
    ok(
        "f()",
        "  let mut p = Pt { a: 1, b: 2 };\n  let r = pick_mut(&mut p.a);\n  p.b = 3;\n  change(&mut r);",
    );
    err(
        "f()",
        "  let mut p = Pt { a: 1, b: 2 };\n  let r = pick_mut(&mut p.a);\n  p = Pt { a: 0, b: 0 };\n  change(&mut r);",
        BorrowConflict,
    );
}

#[test]
fn dead_loan_without_later_use_is_fine() {
    ok(
        "f()",
        "  let mut x = 1;\n  let r = pick(&x);\n  show(&r);\n  x = 2;",
    );
}

#[test]
fn derived_holder_keeps_the_original_region() {
    err(
        "f()",
        "  let mut x = 1;\n  let r = pick(&x);\n  let ret = pick(&r);\n  x = 2;\n  show(&ret);",
        BorrowConflict,
    );
}

#[test]
fn derivation_is_transitive_and_outlives_the_source_holder() {
    err(
        "f(acc: Account)",
        "  let a = pick_acc(&acc);\n  let b = pick_acc(&a);\n  let c = pick_acc(&b);\n  look(&a);\n  let moved = acc;\n  look(&c);",
        BorrowConflict,
    );
}

#[test]
fn reborrow_through_a_holder_keeps_the_root_loan() {
    err(
        "f()",
        "  let mut x = 1;\n  let r = pick(&x);\n  let s = pick(&r);\n  x = 2;\n  show(&s);",
        BorrowConflict,
    );
}

#[test]
fn merge_keeps_a_loan_dead_on_either_path() {
    err(
        "f(c: Bool)",
        "  let mut x = 1;\n  let r = pick(&x);\n  if c {\n    x = 2;\n  }\n  show(&r);",
        BorrowConflict,
    );
    // どちらの道でも使えなくならなければ通る
    ok(
        "f(c: Bool)",
        "  let mut x = 1;\n  let r = pick(&x);\n  if c {\n    let y = x;\n  }\n  show(&r);",
    );
}

// 移動の判定（`UseAfterMove`）は貸しではなく移動の追跡の仕事。取り違えていないことの対照
#[test]
fn moves_are_not_loans() {
    err("f(a: Account)", "  let b = a;\n  look(&a);", UseAfterMove);
}

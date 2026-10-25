//! M2 の最初の PR（ADR 0012）: 参照を書ける位置・借用の書き方・`match` の対象の規則と、
//! 標準ライブラリの新しい形（`&T` の受け手のメソッド・`Displayable`・制約つきの `impl`）のテスト。

use sigil::check::check;
use sigil::ErrorKind;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";
const ACCOUNT: &str = "struct Account {\n  balance: Int,\n  owner: String,\n}\n";

fn func_fx(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

fn func(sig: &str, body: &str) -> String {
    func_fx(sig, "none", body)
}

fn kind(items: &str) -> ErrorKind {
    let src = format!("{HEAD}{items}");
    match check(&src) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => e.kind,
    }
}

fn ok(items: &str) {
    let src = format!("{HEAD}{items}");
    if let Err(e) = check(&src) {
        panic!("{e}\n{src}");
    }
}

fn msg(items: &str) -> String {
    check(&format!("{HEAD}{items}")).unwrap_err().to_string()
}

// ---- 参照を書ける位置（決定 1 の 4） ----

#[test]
fn refs_are_allowed_in_the_three_positions() {
    // 1. 引数の型の一番外
    ok(&func("f(x: &Int, y: &mut List<Int>)", "  return;"));
    // 2. 関数の型の引数の一番外
    ok(&func(
        "f(xs: &List<Int>, key: fn(&Int) -> Int) -> Int",
        "  return 0;",
    ));
    // 3. 戻り値の型の一番外と、一番外の Option の中
    ok(&func("f(x: &Int) -> &Int", "  return x;"));
    ok(&func(
        "f(xs: &List<Int>) -> Option<&Int>",
        "  return xs.get(0);",
    ));
}

#[test]
fn refs_are_rejected_elsewhere() {
    let k = ErrorKind::RefTypePosition;
    assert_eq!(kind("struct V {\n  a: &Int,\n}\n"), k);
    assert_eq!(kind("enum E {\n  A(&Int),\n}\n"), k);
    assert_eq!(kind(&func("f(xs: List<&Int>)", "  return;")), k);
    assert_eq!(kind(&func("f(o: Option<&Int>)", "  return;")), k);
    assert_eq!(kind(&func("f(x: (&Int, Int))", "  return;")), k);
    assert_eq!(kind(&func("f(x: &Option<&Int>)", "  return;")), k);
    assert_eq!(kind(&func("f(x: &Int)", "  let y: &Int = x;")), k);
    // 関数の型の戻り値には書けない
    assert_eq!(kind(&func("f(g: fn(Int) -> &Int)", "  return;")), k);
    // 戻り値の Option 以外の型引数・組の中
    assert_eq!(
        kind(&func("f(x: &List<Int>) -> List<&Int>", "  return x;")),
        k
    );
    assert_eq!(
        kind(&func(
            "f(x: &Int) -> Result<&Int, String>",
            "  return Err(\"a\");"
        )),
        k
    );
}

#[test]
fn ref_returning_fn_needs_exactly_one_borrow_param() {
    let k = ErrorKind::RefReturnBorrowCount;
    // 03-ownership のエラー例 `pick`
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("pick(x: &Account, y: &Account) -> &Account", "  return x;")
        )),
        k
    );
    // 借用の引数が 0 個
    assert_eq!(kind(&func("f(x: Int) -> &Int", "  return x;")), k);
    // 関数の型の引数の中の参照は数えない
    ok(&func(
        "f(xs: &List<Int>, key: fn(&Int) -> Int) -> Option<&Int>",
        "  return xs.get(0);",
    ));
    // 参照を返さないなら、借用の引数はいくつあってもよい
    ok(&func("f(x: &Int, y: &Int) -> Int", "  return 0;"));
}

// ---- 借用の書き方（決定 1 の 3） ----

#[test]
fn borrow_only_as_call_argument() {
    let k = ErrorKind::BorrowPosition;
    assert_eq!(kind(&func("f(n: Int)", "  let r = &n;")), k);
    assert_eq!(kind(&func("f(n: Int)", "  let mut r = &mut n;")), k);
    assert_eq!(kind(&func("f(m: &Int, n: Int) -> &Int", "  return &n;")), k);
    assert_eq!(kind(&func("f(n: Int)", "  let t = (&n, 1);")), k);
    // 呼び出しの引数ならよい
    ok(&format!(
        "{}{}",
        func("show(n: &Int)", "  return;"),
        func("g(n: Int)", "  show(&n);")
    ));
}

#[test]
fn let_mut_of_a_type_containing_a_reference() {
    let src = func("f(xs: &List<Int>)", "  let mut r = xs.get(0);");
    assert_eq!(kind(&src), ErrorKind::LetMutRef);
    ok(&func("f(xs: &List<Int>)", "  let r = xs.get(0);"));
}

// ---- match の対象（決定 1 の 3） ----

#[test]
fn match_on_borrow_of_an_owned_place() {
    ok(&func(
        "f(o: Option<Int>) -> Int",
        "  match &o {\n    Some(n) => {\n      return 1;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 2;",
    ));
    // 腕の束縛は参照なので、値に使える
    ok(&func(
        "f(o: Option<Int>) -> Int",
        "  return match &o {\n    Some(n) => n + 1,\n    None => 0,\n  };",
    ));
}

#[test]
fn match_on_borrow_of_a_reference_name_is_rejected_with_a_hint() {
    let k = ErrorKind::MatchBorrowForm;
    let src = func(
        "f(o: &Option<Int>) -> Int",
        "  match &o {\n    Some(n) => {\n      return 1;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 2;",
    );
    assert_eq!(kind(&src), k);
    assert!(msg(&src).contains("`&` を外して"), "{}", msg(&src));
    // `match o` ならよい
    ok(&func(
        "f(o: &Option<Int>) -> Int",
        "  match o {\n    Some(n) => {\n      return 1;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 2;",
    ));
    // Option<&T> の let 変数も参照の名前
    let body = "  let r = xs.get(0);\n  match &r {\n    Some(n) => {\n      return 1;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 2;";
    assert_eq!(kind(&func("f(xs: &List<Int>) -> Int", body)), k);
    ok(&func(
        "f(xs: &List<Int>) -> Int",
        &body.replace("match &r", "match r"),
    ));
    // 参照を通して届く場所
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func(
                "f(a: &Account) -> Int",
                "  match &a.balance {\n    0 => {\n      return 1;\n    }\n    _ => {\n      return 0;\n    }\n  }\n  return 2;"
            )
        )),
        k
    );
}

#[test]
fn match_on_borrow_forms_other_than_shared_place() {
    let k = ErrorKind::MatchBorrowForm;
    let arms = "{\n    Some(n) => {\n      return 1;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 2;";
    assert_eq!(
        kind(&func(
            "f(o: Option<Int>) -> Int",
            &format!("  match &mut o {arms}")
        )),
        k
    );
    // 一時的な値の借用は、文の終わりまで（ADR 0012 の決定 1 の 3）
    ok(&func("f() -> Int", &format!("  match &Some(1) {arms}")));
}

// ---- 標準ライブラリ（決定 2・4・6） ----

#[test]
fn read_only_ops_are_methods_with_ref_receivers() {
    ok(&func(
        "f(xs: &List<Int>) -> Int",
        "  return xs.sum() + xs.len();",
    ));
    ok(&func("f(xs: &List<Float>) -> Float", "  return xs.min();"));
    ok(&func("f(xs: &List<Int>) -> Int", "  return xs.max();"));
    ok(&func("f(p: &Path) -> Bool", "  return p.is_empty();"));
    ok(&func(
        "f(s: &String) -> Result<Int, ParseError>",
        "  return s.trim().parse_int();",
    ));
    ok(&func("f(n: Int) -> Float", "  return to_float(n);"));
}

#[test]
fn constrained_impls_check_their_bounds() {
    // sum は Numeric、min / max は Comparable + Copyable
    assert_eq!(
        kind(&func(
            "f(xs: &List<String>) -> String",
            "  return xs.sum();"
        )),
        ErrorKind::ConstraintUnsatisfied
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f(xs: &List<Account>) -> Account", "  return xs.min();")
        )),
        ErrorKind::ConstraintUnsatisfied
    );
}

#[test]
fn free_read_only_functions_are_gone() {
    for call in ["len(xs)", "sum(xs)", "min(xs)", "max(xs)", "empty(p)"] {
        let src = func("f(xs: &List<Int>, p: &Path)", &format!("  let a = {call};"));
        assert_eq!(kind(&src), ErrorKind::UnknownName, "{call}");
    }
}

#[test]
fn console_log_takes_displayable_by_ref() {
    let log = |sig: &str, body: &str| func_fx(sig, "console", body);
    ok(&log(
        "f(n: Int, b: Bool)",
        "  console.log(&n);\n  console.log(&b);",
    ));
    ok(&log(
        "f()",
        "  console.log(&\"text\");\n  console.log(&1.5);",
    ));
    // 借用の書き忘れ
    assert_eq!(
        kind(&log("f(n: Int)", "  console.log(n);")),
        ErrorKind::TypeMismatch
    );
    // Displayable でない型: 構造体・組・一覧
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            log("f(a: Account)", "  console.log(&a);")
        )),
        ErrorKind::ConstraintUnsatisfied
    );
    assert_eq!(
        kind(&log("f(xs: List<Int>)", "  console.log(&xs);")),
        ErrorKind::ConstraintUnsatisfied
    );
    assert_eq!(
        kind(&log("f(t: (Int, Int))", "  console.log(&t);")),
        ErrorKind::ConstraintUnsatisfied
    );
    // 篩型は基の型から引き継ぐ
    ok(&format!(
        "type Port = Int where 1 <= self && self <= 65535;\n{}",
        log("f(p: Port)", "  console.log(&p);")
    ));
}

#[test]
fn shared_cell_get_needs_copyable() {
    let st = |sig: &str, body: &str| func_fx(sig, "state", body);
    ok(&st("f(c: &Shared<Cell<Int>>) -> Int", "  return c.get();"));
    assert_eq!(
        kind(&st(
            "f(c: &Shared<Cell<String>>) -> String",
            "  return c.get();"
        )),
        ErrorKind::ConstraintUnsatisfied
    );
    // set は Copyable でなくてもよい
    ok(&st("f(c: &Shared<Cell<String>>)", "  c.set(\"a\");"));
}

#[test]
fn fs_write_takes_borrowed_path_and_text() {
    let w = |target: &str, effects: &str| {
        format!(
            "module m.a\n  targets {target}\n  trust none\n\n{}",
            func_fx(
                "f(p: Path, s: String) -> Result<Unit, IoError>",
                effects,
                "  return fs.write(&p, &s);"
            )
        )
    };
    assert!(check(&w("server", "fs.write")).is_ok());
    assert_eq!(
        check(&w("server", "none")).unwrap_err().kind,
        ErrorKind::EffectMissing
    );
    assert_eq!(
        check(&w("server", "fs.read")).unwrap_err().kind,
        ErrorKind::EffectMissing
    );
    assert_eq!(
        check(&w("browser", "fs.write")).unwrap_err().kind,
        ErrorKind::EffectTarget
    );
    // 借用の書き忘れ
    let bad = format!(
        "{HEAD}{}",
        func_fx(
            "f(p: Path, s: String) -> Result<Unit, IoError>",
            "fs.write",
            "  return fs.write(p, s);"
        )
    );
    assert_eq!(check(&bad).unwrap_err().kind, ErrorKind::TypeMismatch);
}

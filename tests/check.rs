//! 型検査（M1 の最初の一区切り。ADR 0011）のテスト。

use sigil::check::check;
use sigil::ErrorKind;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";
const CL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// 4 節が `none` の関数。
fn func(sig: &str, body: &str) -> String {
    format!("fn {sig}\n{CL}{{\n{body}\n}}\n")
}

fn prog(items: &str) -> String {
    format!("{HEAD}{items}")
}

fn kind(items: &str) -> ErrorKind {
    let src = prog(items);
    match check(&src) {
        Ok(()) => panic!("型エラーになるはずが成功した:\n{src}"),
        Err(e) => e.kind,
    }
}

fn ok(items: &str) {
    let src = prog(items);
    if let Err(e) = check(&src) {
        panic!("{e}\n{src}");
    }
}

/// 関数 1 つの本体だけを検査する。
fn body_kind(sig: &str, body: &str) -> ErrorKind {
    kind(&func(sig, body))
}

fn body_ok(sig: &str, body: &str) {
    ok(&func(sig, body));
}

const ACCOUNT: &str = "struct Account {\n  balance: Int,\n  owner: String,\n}\n";
const PORT: &str = "type Port = Int where 1 <= self && self <= 65535;\n";

// ---- ADR 0011 の「型エラーになる」----

#[test]
fn int_plus_float() {
    assert_eq!(
        body_kind("f(a: Int, b: Float) -> Int", "  return a + b;"),
        ErrorKind::OperandTypes
    );
}

#[test]
fn int_literal_out_of_range() {
    assert_eq!(
        body_kind("f() -> Int", "  return 9223372036854775808;"),
        ErrorKind::IntLiteralRange
    );
    body_ok("f() -> Int", "  return 9223372036854775807;");
}

#[test]
fn assign_through_shared_ref() {
    assert_eq!(
        body_kind("f(n: &Int)", "  n = 1;"),
        ErrorKind::AssignThroughSharedRef
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f(a: &Account)", "  a.balance = 1;")
        )),
        ErrorKind::AssignThroughSharedRef
    );
}

#[test]
fn assign_to_by_value_param_and_immutable_let() {
    assert_eq!(
        body_kind("f(n: Int)", "  n = 1;"),
        ErrorKind::AssignToImmutable
    );
    assert_eq!(
        body_kind("f()", "  let x = 1;\n  x = 2;"),
        ErrorKind::AssignToImmutable
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f(a: Account)", "  a.balance = 1;")
        )),
        ErrorKind::AssignToImmutable
    );
}

#[test]
fn let_mut_of_reference() {
    assert_eq!(
        body_kind("f(n: Int)", "  let mut r = &n;"),
        ErrorKind::LetMutRef
    );
    body_ok("f(n: Int)", "  let r = &n;");
}

#[test]
fn old_of_struct() {
    let items = format!(
        "{ACCOUNT}fn f(a: &Account)\n  intent \"x\"\n  requires none\n  ensures a.balance == a.balance && old(a) == old(a)\n  effects none\n{{\n  return;\n}}\n"
    );
    assert_eq!(kind(&items), ErrorKind::OldNotCopyable);
}

#[test]
fn old_of_int_ok() {
    let items = format!(
        "{ACCOUNT}fn f(a: &mut Account)\n  intent \"x\"\n  requires none\n  ensures a.balance == old(a.balance) + 1\n  effects none\n{{\n  a.balance = a.balance + 1;\n}}\n"
    );
    ok(&items);
}

#[test]
fn eq_on_non_equatable() {
    assert_eq!(
        kind(&func(
            "f(g: fn(Int) -> Int, h: fn(Int) -> Int) -> Bool",
            "  return g == h;"
        )),
        ErrorKind::NotEquatable
    );
}

#[test]
fn redundant_let_annotation() {
    assert_eq!(
        body_kind("f()", "  let x: Int = 1;"),
        ErrorKind::RedundantAnnotation
    );
    // 篩型と基の型の間を移すときは必要
    ok(&format!(
        "{PORT}{}",
        func("f(p: Port)", "  let n: Int = p;\n  let q: Port = 8080;")
    ));
}

#[test]
fn fn_type_positions() {
    assert_eq!(
        kind("struct S {\n  f: fn(Int) -> Int,\n}\n"),
        ErrorKind::FnTypePosition
    );
    assert_eq!(
        kind("enum E {\n  A(fn(Int)),\n}\n"),
        ErrorKind::FnTypePosition
    );
    assert_eq!(
        kind(&func("f() -> fn(Int) -> Int", "  return f;")),
        ErrorKind::FnTypePosition
    );
    assert_eq!(
        kind(&func(
            "f(g: fn(Int) -> Int)",
            "  let h: fn(Int) -> Int = g;"
        )),
        ErrorKind::FnTypePosition
    );
}

// ---- ADR 0011 の「通る」----

#[test]
fn mut_ref_param_assignment() {
    body_ok("bump(n: &mut Int)", "  n = n + 1;");
}

#[test]
fn refinement_value_passes_as_base() {
    ok(&format!(
        "{PORT}{}{}",
        func("show(n: Int) -> Int", "  return n;"),
        func("f(p: Port) -> Int", "  return show(p) + p;")
    ));
}

#[test]
fn constant_to_refinement() {
    ok(&format!(
        "{PORT}{}{}",
        func("take(p: Port)", "  return;"),
        func("f()", "  take(8080);\n  let p: Port = 80;")
    ));
}

#[test]
fn base_to_refinement_needs_literal() {
    assert_eq!(
        kind(&format!(
            "{PORT}{}",
            func("f(n: Int) -> Port", "  return n;")
        )),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn tuple_destructuring() {
    body_ok(
        "f(t: (Int, String)) -> Int",
        "  let (a, b) = t;\n  let (_, (c, d)) = (1, (2, 3));\n  return a + c + d;",
    );
    assert_eq!(
        body_kind("f(t: (Int, Int, Int))", "  let (a, b) = t;"),
        ErrorKind::DestructureMismatch
    );
    assert_eq!(
        body_kind("f(t: Int)", "  let (a, b) = t;"),
        ErrorKind::DestructureMismatch
    );
}

#[test]
fn method_with_mut_receiver() {
    let src = format!(
        "{ACCOUNT}impl Account {{\n  fn deposit(self: &mut Account, n: Int)\n{CL}  {{\n    self.balance = self.balance + n;\n  }}\n}}\n{}",
        func(
            "f()",
            "  let mut a = Account { balance: 1, owner: \"x\" };\n  a.deposit(2);"
        )
    );
    ok(&src);
}

#[test]
fn mut_receiver_needs_mutable_variable() {
    let src = format!(
        "{ACCOUNT}impl Account {{\n  fn deposit(self: &mut Account, n: Int)\n{CL}  {{\n    self.balance = self.balance + n;\n  }}\n}}\n{}",
        func(
            "f()",
            "  let a = Account { balance: 1, owner: \"x\" };\n  a.deposit(2);"
        )
    );
    assert_eq!(kind(&src), ErrorKind::BorrowMutOfImmutable);
}

#[test]
fn method_reads_and_static_fn() {
    let src = format!(
        "{ACCOUNT}impl Account {{\n  fn open(owner: String) -> Account\n{CL}  {{\n    return Account {{ balance: 0, owner: owner }};\n  }}\n  fn total(self: &Account) -> Int\n{CL}  {{\n    return self.balance;\n  }}\n}}\n{}",
        func(
            "f(a: &Account) -> Int",
            "  let b = Account.open(\"x\");\n  return a.total() + b.total();"
        )
    );
    ok(&src);
}

#[test]
fn impl_errors() {
    let m = |sig: &str| {
        format!("{ACCOUNT}impl Account {{\n  fn {sig}\n{CL}  {{\n    return;\n  }}\n}}\n")
    };
    assert_eq!(
        kind(&m("balance(self: &Account)")),
        ErrorKind::MemberConflict
    );
    assert_eq!(kind(&m("m(self: Int)")), ErrorKind::TypeMismatch);
    let dup = format!(
        "{ACCOUNT}impl Account {{\n  fn m()\n{CL}  {{\n    return;\n  }}\n  fn m()\n{CL}  {{\n    return;\n  }}\n}}\n"
    );
    assert_eq!(kind(&dup), ErrorKind::DuplicateDefinition);
}

#[test]
fn borrow_passes() {
    ok(&format!(
        "{ACCOUNT}{}{}",
        func("peek(a: &Account) -> Int", "  return a.balance;"),
        func(
            "f(a: &mut Account) -> Int",
            "  a.balance = 5;\n  return peek(&a);"
        )
    ));
    // 借用は呼び出しの位置で & を書く
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}{}",
            func("peek(a: &Account) -> Int", "  return a.balance;"),
            func("f(a: &Account) -> Int", "  return peek(a);")
        )),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}{}",
            func("w(a: &mut Account)", "  return;"),
            func("f(a: &Account)", "  w(&mut a);")
        )),
        ErrorKind::BorrowMutOfImmutable
    );
}

#[test]
fn enum_values() {
    ok(&format!(
        "enum Shape {{\n  Circle(Int),\n  Empty,\n}}\n{}{}",
        func("mk(r: Int) -> Shape", "  return Shape.Circle(r);"),
        func("e() -> Shape", "  return Shape.Empty;")
    ));
    assert_eq!(
        kind(&format!(
            "enum Shape {{\n  Circle(Int),\n}}\n{}",
            func("mk() -> Shape", "  return Shape.Square(1);")
        )),
        ErrorKind::NoSuchMember
    );
}

#[test]
fn function_values_as_args() {
    ok(&format!(
        "{}{}{}",
        func("inc(x: Int) -> Int", "  return x + 1;"),
        func("apply(g: fn(Int) -> Int, v: Int) -> Int", "  return g(v);"),
        func("main2() -> Int", "  return apply(inc, 1);")
    ));
}

// ---- 基本の規則 ----

#[test]
fn operators() {
    body_ok(
        "f(a: Int, b: Int, x: Float, s: String) -> Bool",
        "  let c = a % b + a / b - a * b + -a;\n  let y = x * 2.5 + -x;\n  return !(a < b) && (x >= y || s == \"a\") && s <= \"b\";",
    );
    assert_eq!(
        body_kind("f(x: Float, y: Float) -> Float", "  return x % y;"),
        ErrorKind::OperandTypes
    );
    assert_eq!(
        body_kind("f(a: Int, b: Float) -> Bool", "  return a == b;"),
        ErrorKind::OperandTypes
    );
    assert_eq!(
        body_kind("f(a: Bool, b: Bool) -> Bool", "  return a < b;"),
        ErrorKind::NotComparable
    );
    assert_eq!(
        body_kind("f(a: Int) -> Bool", "  return a && true;"),
        ErrorKind::OperandTypes
    );
    assert_eq!(
        body_kind("f(a: Int) -> Int", "  return !a;"),
        ErrorKind::OperandTypes
    );
    assert_eq!(
        body_kind("f(a: String, b: String) -> String", "  return a + b;"),
        ErrorKind::OperandTypes
    );
}

#[test]
fn refinement_value_is_base_in_operators() {
    ok(&format!(
        "{PORT}{}",
        func("f(p: Port) -> Int", "  let n = p + 1;\n  return n;")
    ));
    // 結果は基の型。篩型の戻り値にはそのまま返せない
    assert_eq!(
        kind(&format!(
            "{PORT}{}",
            func("f(p: Port) -> Port", "  return p + 1;")
        )),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn structural_equality_is_derived() {
    ok(&format!(
        "{ACCOUNT}{}",
        func("f(a: &Account, b: &Account) -> Bool", "  return a == b;")
    ));
    assert_eq!(
        kind(&format!(
            "struct H {{\n  g: fn(Int),\n}}\n{}",
            func("f() -> Int", "  return 1;")
        )),
        ErrorKind::FnTypePosition
    );
}

#[test]
fn names_and_calls() {
    assert_eq!(
        body_kind("f() -> Int", "  return y;"),
        ErrorKind::UnknownName
    );
    assert_eq!(
        body_kind("f() -> Int", "  return g(1);"),
        ErrorKind::UnknownName
    );
    assert_eq!(
        kind(&func("f(x: Foo)", "  return;")),
        ErrorKind::UnknownType
    );
    assert_eq!(
        kind(&format!(
            "{}{}",
            func("g(a: Int) -> Int", "  return a;"),
            func("f() -> Int", "  return g(1, 2);")
        )),
        ErrorKind::ArgCount
    );
    assert_eq!(
        kind(&format!(
            "{}{}",
            func("g(a: Int) -> Int", "  return a;"),
            func("f() -> Int", "  return g(\"s\");")
        )),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        body_kind("f(x: Int) -> Int", "  return x(1);"),
        ErrorKind::NotCallable
    );
}

#[test]
fn returns() {
    assert_eq!(
        body_kind("f(x: Int) -> Int", "  let y = x;"),
        ErrorKind::MissingReturn
    );
    assert_eq!(
        body_kind("f(x: Int) -> Int", "  if x > 0 {\n    return 1;\n  }"),
        ErrorKind::MissingReturn
    );
    body_ok(
        "f(x: Int) -> Int",
        "  if x > 0 {\n    return 1;\n  } else if x < 0 {\n    return 2;\n  } else {\n    return 3;\n  }",
    );
    assert_eq!(
        body_kind("f(x: Int)", "  return x;"),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        body_kind("f(x: Int) -> Int", "  return;"),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        body_kind("f(x: Int) -> Int", "  return \"a\";"),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn control_flow_conditions() {
    body_ok(
        "f(x: Int)",
        "  let mut i = 0;\n  while i < x {\n    i = i + 1;\n  }\n  if i == x {\n    return;\n  }",
    );
    assert_eq!(
        body_kind("f(x: Int)", "  if x {\n    return;\n  }"),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        body_kind("f(x: Int)", "  while x + 1 {\n    return;\n  }"),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn struct_literals() {
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f() -> Account", "  return Account { balance: 1 };")
        )),
        ErrorKind::FieldSet
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func(
                "f() -> Account",
                "  return Account { balance: 1, owner: \"x\", extra: 2 };"
            )
        )),
        ErrorKind::FieldSet
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func(
                "f() -> Account",
                "  return Account { balance: \"x\", owner: \"x\" };"
            )
        )),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f(a: Account) -> Int", "  return a.nope;")
        )),
        ErrorKind::NoSuchMember
    );
}

#[test]
fn tuples_have_no_index_access() {
    // `t.0` は構文エラー。型検査まで来ない
    assert!(check(&prog(&func("f(t: (Int, Int)) -> Int", "  return t.0;"))).is_err());
}

#[test]
fn definitions() {
    assert_eq!(
        kind(&format!(
            "{}{}",
            func("f()", "  return;"),
            func("f()", "  return;")
        )),
        ErrorKind::DuplicateDefinition
    );
    assert_eq!(
        kind("struct A {\n  x: Int,\n}\nstruct A {\n  y: Int,\n}\n"),
        ErrorKind::DuplicateDefinition
    );
    assert_eq!(
        kind(&func("f(a: Int, a: Int)", "  return;")),
        ErrorKind::DuplicateDefinition
    );
    assert_eq!(
        kind("struct A {\n  x: Int,\n  x: Int,\n}\n"),
        ErrorKind::DuplicateDefinition
    );
}

#[test]
fn direct_recursive_types() {
    assert_eq!(kind("struct A {\n  a: A,\n}\n"), ErrorKind::RecursiveType);
    assert_eq!(
        kind("enum T {\n  Node(T, T),\n  Leaf,\n}\n"),
        ErrorKind::RecursiveType
    );
    assert_eq!(
        kind("struct A {\n  b: B,\n}\nstruct B {\n  t: (Int, A),\n}\n"),
        ErrorKind::RecursiveType
    );
    ok("struct A {\n  x: Int,\n}\nstruct B {\n  a: A,\n}\n");
}

// ---- 篩型の宣言 (ADR 0011 の決定 8) ----

#[test]
fn refinement_declarations() {
    ok(&format!(
        "{}type Short = String where len(self) < 10;\n{}",
        func("len(s: String) -> Int", "  return 1;"),
        PORT
    ));
    assert_eq!(kind("type A = Int where self;\n"), ErrorKind::TypeMismatch);
    assert_eq!(
        kind("type A = Int where self == 1;\ntype B = A where self == 1;\n"),
        ErrorKind::RefinementBase
    );
    assert_eq!(
        kind("type A = &Int where self == 1;\n"),
        ErrorKind::RefinementBase
    );
    assert_eq!(
        kind(&format!(
            "{}type A = Int where g(self);\n",
            func("g(n: Int) -> Bool", "  return true;").replace("effects none", "effects console")
        )),
        ErrorKind::RefinementEffect
    );
    assert_eq!(
        kind(&format!(
            "{}type A = Int where self > limit;\n",
            func("g(limit: Int)", "  return;")
        )),
        ErrorKind::RefinementName
    );
    assert_eq!(
        kind("type A = Int where self == 1.5;\n"),
        ErrorKind::OperandTypes
    );
}

// ---- 範囲外（黙って通さない） ----

#[test]
fn unsupported_is_explicit() {
    let is_unsupported = |items: String| assert_eq!(kind(&items), ErrorKind::Unsupported);
    is_unsupported(func("f<T>(x: Int)", "  return;"));
    is_unsupported("struct S<T> {\n  x: Int,\n}\n".into());
    is_unsupported(func("f(x: List<Int>)", "  return;"));
    is_unsupported(func("f(x: Option<Int>)", "  return;"));
    is_unsupported(func(
        "f(x: Int) -> Int",
        "  let y = match x { 1 => 2, _ => 3 };\n  return y;",
    ));
    is_unsupported(func(
        "f(x: Int)",
        "  match x {\n    _ => {\n      return;\n    }\n  }",
    ));
    is_unsupported(func("f(x: Int) -> Int", "  let g = |a| a;\n  return x;"));
    is_unsupported(func("f(x: String) -> Int", "  return x.len();"));
    is_unsupported(func("f() -> Int", "  let x = Some(1);\n  return 1;"));
}

#[test]
fn error_display_is_japanese_with_type_label() {
    let src = prog(&func("f(a: Int, b: Float) -> Int", "  return a + b;"));
    let e = check(&src).unwrap_err();
    let s = e.to_string();
    assert!(s.contains("型エラー"), "{s}");
    assert!(s.contains("Int") && s.contains("Float"), "{s}");
    assert_eq!(e.span.line, 11, "{s}");
}

#[test]
fn syntax_errors_come_first() {
    let e = check("module").unwrap_err();
    assert_ne!(e.kind.phase(), sigil::Phase::Check);
}

#[test]
fn examples_stop_at_unsupported_not_at_a_false_type_error() {
    // 5 つのサンプルは総称・標準ライブラリの型・`match` を使うので、この版ではまだ通らない。
    // ただし誤った型エラーではなく、範囲外（`Unsupported`）で止まること。
    for p in [
        "examples/01-average/c-brace.sigil",
        "examples/02-read-port/c-brace.sigil",
        "examples/03-ownership/ownership.sigil",
        "examples/04-effects/effects.sigil",
        "examples/05-control/control.sigil",
    ] {
        let src = std::fs::read_to_string(p).unwrap();
        let e = check(&src).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported, "{p}: {e}");
    }
}

// ---- PR #22 のレビュー指摘 ----

#[test]
fn not_and_or_see_through_refs_and_refinements() {
    let flag = "type Flag = Bool where self || !self;\n";
    // 篩型の Bool と &Bool は、`!`・`&&`・`||` に使える。
    ok(&format!(
        "{flag}{}",
        func(
            "f(a: Flag, b: &Bool, c: Bool) -> Bool",
            "  let x = !a;\n  let y = !b;\n  let z = a && b;\n  let w = b || a;\n  return x && y && z && w && !c;"
        )
    ));
    // Bool でないものは相変わらずエラー。
    assert_eq!(
        body_kind("f(a: &Int) -> Bool", "  return !a;"),
        ErrorKind::OperandTypes
    );
    assert_eq!(
        body_kind("f(a: &Int, b: Bool) -> Bool", "  return a && b;"),
        ErrorKind::OperandTypes
    );
}

#[test]
fn shadowing_is_an_error() {
    // 引数と同じ名前の let
    assert_eq!(
        body_kind("f(x: Int) -> Int", "  let x = 1;\n  return x;"),
        ErrorKind::ShadowedName
    );
    // 外側のスコープと同じ名前
    assert_eq!(
        body_kind(
            "f(a: Int) -> Int",
            "  let y = 1;\n  if a > 0 {\n    let y = 2;\n    return y;\n  }\n  return y;"
        ),
        ErrorKind::ShadowedName
    );
    // 同じ let の分解の中
    assert_eq!(
        body_kind("f(t: (Int, Int)) -> Int", "  let (a, a) = t;\n  return a;"),
        ErrorKind::ShadowedName
    );
    // 兄弟のブロックでの再利用は隠蔽ではない
    body_ok(
        "f(a: Int) -> Int",
        "  if a > 0 {\n    let y = 1;\n    return y;\n  }\n  if a < 0 {\n    let y = 2;\n    return y;\n  }\n  return 0;",
    );
    // 別の関数の同名の変数は関係ない
    ok(&format!(
        "{}{}",
        func("f(a: Int) -> Int", "  let y = a;\n  return y;"),
        func("g(a: Int) -> Int", "  let y = a;\n  return y;")
    ));
}

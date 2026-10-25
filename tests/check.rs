//! 型検査（M1 の最初の一区切り。ADR 0011）のテスト。

use sigil::check::{check, check_with};
use sigil::effects::{EffectTable, Options, Primitive};
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

fn prim(sig: &str, effects: &[&str]) -> Primitive {
    Primitive {
        signature: sig.to_string(),
        effects: effects.iter().map(|e| e.to_string()).collect(),
    }
}

/// 標準ライブラリが入るまでの、効果のある仮の関数。
fn emit_options() -> Options {
    Options {
        table: EffectTable::standard(),
        primitives: vec![
            prim("emit(n: Int) -> Bool", &["console"]),
            prim("read_file(p: String) -> String", &["fs.read"]),
        ],
    }
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
        body_kind("f(xs: &List<Int>)", "  let mut r = xs.get(0);"),
        ErrorKind::LetMutRef
    );
    body_ok("f(xs: &List<Int>)", "  let r = xs.get(0);");
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
        func("apply(g: fn(Int) -> Int, v: Int) -> Int", "  return g(v);")
            .replace("effects none", "effects of(g)"),
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
    // 効果のある関数は篩型の条件で呼べない（仮の関数 `emit` は `console` を起こす）
    let src = prog("type A = Int where emit(self);\n");
    assert_eq!(
        check_with(&src, &emit_options()).unwrap_err().kind,
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

// ---- 標準ライブラリ（仮の範囲。src/stdlib.rs） ----

#[test]
fn std_methods_and_functions_typecheck() {
    body_ok("f(x: String) -> Int", "  return x.len();");
    body_ok("f(x: &List<Int>) -> Int", "  return x.len();");
    body_ok("f() -> List<Int>", "  return List.new();");
    body_ok(
        "f(xs: &mut List<Int>)",
        "  xs.push(1);\n  let n = xs.len();\n  let g = xs.get(0);\n  let e = xs.is_empty();",
    );
    body_ok(
        "f(o: Option<Int>) -> Int",
        "  let a = o.is_some();\n  return o.unwrap_or(0);",
    );
    body_ok(
        "f(r: Result<Int, String>) -> Option<Int>",
        "  let a = r.is_ok();\n  return r.ok();",
    );
    body_ok(
        "f(s: String) -> Result<Int, ParseError>",
        "  return s.trim().parse_int();",
    );
}

#[test]
fn std_method_type_errors() {
    assert_eq!(
        body_kind("f(x: &mut List<Int>)", "  x.push(1.5);"),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        body_kind("f(x: String) -> Int", "  return x.nothing();"),
        ErrorKind::NoSuchMember
    );
    assert_eq!(
        body_kind("f() -> Int", "  return List.nothing();"),
        ErrorKind::NoSuchMember
    );
}

#[test]
fn std_free_fn_names_are_reserved() {
    // 標準の自由関数 `to_float` は予約名（ADR 0012 の決定 5）。上書きは構文の段階で弾かれる
    assert_eq!(
        kind(&func("to_float(x: Int) -> Float", "  return 1.0;")),
        ErrorKind::ReservedNameDefined
    );
    for name in [
        "Displayable",
        "Path",
        "IoError",
        "ParseError",
        "Shared",
        "Cell",
        "Set",
        "Map",
    ] {
        assert_eq!(
            kind(&format!("struct {name} {{\n  x: Int,\n}}\n")),
            ErrorKind::ReservedNameDefined,
            "{name}"
        );
    }
    // `to_float` は呼べる
    body_ok("f(n: Int) -> Float", "  return to_float(n);");
    // 標準の自由関数でない名前は、これまでどおり定義できる
    ok(&format!(
        "{}{}",
        func("len(x: Int) -> Int", "  return x;"),
        func("g() -> Int", "  return len(3);")
    ));
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
fn all_five_examples_check() {
    for p in [
        "examples/01-average/c-brace.sigil",
        "examples/02-read-port/c-brace.sigil",
        "examples/03-ownership/ownership.sigil",
        "examples/04-effects/effects.sigil",
        "examples/05-control/control.sigil",
    ] {
        let src = std::fs::read_to_string(p).unwrap();
        if let Err(e) = check(&src) {
            panic!("{p}: {e}");
        }
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

// ======================================================================
// PR2: 総称・性質・Option/Result/?・match・クロージャ・推論・T.check
// ======================================================================

const PAIR: &str = "struct Pair<A, B> {\n  first: A,\n  second: B,\n}\n";
const SHAPE: &str = "enum Shape {\n  Circle(Int),\n  Rect(Int, Int),\n  Empty,\n}\n";

// ---- 総称の関数 ----

#[test]
fn generic_fn_identity_and_inference() {
    ok(&format!(
        "{}{}",
        func("id<T>(x: T) -> T", "  return x;"),
        func(
            "g() -> Int",
            "  let a = id(5);\n  let b = id(\"s\");\n  return a;"
        )
    ));
    // 型引数に合わない使い方
    assert_eq!(
        kind(&format!(
            "{}{}",
            func("id<T>(x: T) -> T", "  return x;"),
            func("g() -> String", "  return id(5);")
        )),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn generic_fn_type_params_are_rigid() {
    assert_eq!(
        body_kind("f<T>(x: T) -> Int", "  return x;"),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        body_kind("f<T, U>(x: T, y: U) -> T", "  return y;"),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn unification_ties_type_args_together() {
    let two = func("two<T>(a: T, b: T) -> T", "  return a;");
    ok(&format!(
        "{two}{}",
        func("g() -> Int", "  return two(1, 2);")
    ));
    assert_eq!(
        kind(&format!(
            "{two}{}",
            func("g() -> Int", "  return two(1, \"x\");")
        )),
        ErrorKind::TypeMismatch
    );
}

// ---- 性質と制約 ----

#[test]
fn bounds_are_checked_at_the_call() {
    let max = func(
        "biggest<T: Comparable>(a: T, b: T) -> T",
        "  if a > b {\n    return a;\n  }\n  return b;",
    );
    ok(&format!(
        "{max}{}",
        func("g() -> Int", "  return biggest(1, 2);")
    ));
    // 構造体は Comparable でない
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{max}{}",
            func(
                "g(a: Account, b: Account) -> Account",
                "  return biggest(a, b);"
            )
        )),
        ErrorKind::ConstraintUnsatisfied
    );
}

#[test]
fn unbounded_type_param_has_no_operators() {
    assert_eq!(
        body_kind("f<T>(a: T, b: T) -> Bool", "  return a == b;"),
        ErrorKind::NotEquatable
    );
    assert_eq!(
        body_kind("f<T>(a: T, b: T) -> Bool", "  return a < b;"),
        ErrorKind::NotComparable
    );
    assert_eq!(
        body_kind("f<T>(a: T, b: T) -> T", "  return a + b;"),
        ErrorKind::OperandTypes
    );
    body_ok("f<T: Equatable>(a: T, b: T) -> Bool", "  return a == b;");
    body_ok("f<T: Comparable>(a: T, b: T) -> Bool", "  return a <= b;");
    body_ok("f<T: Numeric>(a: T, b: T) -> T", "  return a + b * a;");
    // Numeric は Comparable を含む
    body_ok("f<T: Numeric>(a: T, b: T) -> Bool", "  return a < b;");
}

#[test]
fn derived_properties_follow_the_adr_table() {
    let eq = func("same<T: Equatable>(a: T, b: T) -> Bool", "  return a == b;");
    let send = func("go<T: Sendable>(a: T)", "  return;");
    let copy = func("dup<T: Copyable>(a: T) -> T", "  return a;");
    let cmp = func("cmp<T: Comparable>(a: T, b: T) -> Bool", "  return a < b;");
    // Equatable: 構造体・列挙型・組・List は中身が Equatable なら Equatable
    ok(&format!(
        "{ACCOUNT}{SHAPE}{eq}{}",
        func(
            "g(a: Account, b: Account, s: Shape, t: Shape, l: List<Int>, m: List<Int>)",
            "  let x = same(a, b);\n  let y = same(s, t);\n  let z = same(l, m);\n  let w = same((1, \"a\"), (2, \"b\"));"
        )
    ));
    // Equatable でない: 関数を持つ型、&mut など。List<関数> は作れないので組で確かめる
    assert_eq!(
        kind(&format!(
            "{eq}{}",
            func("g(f: fn(Int) -> Int)", "  let x = same(f, f);")
        )),
        ErrorKind::ConstraintUnsatisfied
    );
    // Sendable: 参照は送れない
    ok(&format!(
        "{ACCOUNT}{send}{}",
        func("g(a: Account)", "  go(a);")
    ));
    // 参照を型引数に渡せる（ADR 0012 の決定 1 の 4）が、参照は Sendable でない
    assert_eq!(
        kind(&format!("{send}{}", func("g(a: Int)", "  go(&a);"))),
        ErrorKind::ConstraintUnsatisfied
    );
    // Copyable は導出されない: 構造体・列挙型は Copyable でない
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{copy}{}",
            func("g(a: Account)", "  let b = dup(a);")
        )),
        ErrorKind::ConstraintUnsatisfied
    );
    ok(&format!(
        "{copy}{}",
        func(
            "g(a: Int, b: (Int, Bool))",
            "  let x = dup(a);\n  let y = dup(b);"
        )
    ));
    // Comparable は導出されない: 組も構造体も比べられない
    assert_eq!(
        kind(&format!(
            "{cmp}{}",
            func("g()", "  let x = cmp((1, 2), (3, 4));")
        )),
        ErrorKind::ConstraintUnsatisfied
    );
    ok(&format!(
        "{cmp}{}",
        func(
            "g()",
            "  let x = cmp(\"a\", \"b\");\n  let y = cmp(1.5, 2.5);"
        )
    ));
}

#[test]
fn refinement_types_take_their_base_properties() {
    let cmp = func("cmp<T: Comparable>(a: T, b: T) -> Bool", "  return a < b;");
    ok(&format!(
        "{PORT}{cmp}{}",
        func("g(a: Port, b: Port) -> Bool", "  return cmp(a, b);")
    ));
    // Numeric は篩型には付かない（算術で範囲が崩れるため）
    let num = func("add<T: Numeric>(a: T, b: T) -> T", "  return a + b;");
    assert_eq!(
        kind(&format!(
            "{PORT}{num}{}",
            func("g(a: Port, b: Port) -> Port", "  return add(a, b);")
        )),
        ErrorKind::ConstraintUnsatisfied
    );
}

#[test]
fn unknown_property_and_bad_bound_positions() {
    assert_eq!(
        kind(&func("f<T: Showable>(x: T)", "  return;")),
        ErrorKind::UnknownProperty
    );
    assert_eq!(
        kind("struct S<T: Equatable> {\n  x: T,\n}\n"),
        ErrorKind::TypeParamBound
    );
    assert_eq!(
        kind("enum E<T: Equatable> {\n  A(T),\n}\n"),
        ErrorKind::TypeParamBound
    );
    // 複数の制約
    body_ok(
        "f<T: Equatable + Sendable>(a: T, b: T) -> Bool",
        "  return a == b;",
    );
}

#[test]
fn type_arg_count_is_checked() {
    assert_eq!(
        kind(&func("f(x: List<Int, Int>)", "  return;")),
        ErrorKind::TypeArgCount
    );
    assert_eq!(
        kind(&func("f(x: List)", "  return;")),
        ErrorKind::TypeArgCount
    );
    assert_eq!(
        kind(&format!("{PAIR}{}", func("f(x: Pair<Int>)", "  return;"))),
        ErrorKind::TypeArgCount
    );
    assert_eq!(
        kind(&func("f(x: Int<Int>)", "  return;")),
        ErrorKind::TypeArgCount
    );
}

#[test]
fn type_param_names_cannot_clash() {
    assert_eq!(
        kind(&func("f<T, T>(x: T)", "  return;")),
        ErrorKind::DuplicateDefinition
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f<Account>(x: Account)", "  return;")
        )),
        ErrorKind::DuplicateDefinition
    );
    // 標準の前置きの名前は型に使えない
    assert_eq!(
        kind("struct Option {\n  x: Int,\n}\n"),
        ErrorKind::ReservedNameDefined
    );
    assert_eq!(
        kind("enum Foo {\n  Some,\n}\nenum Some {\n  A,\n}\n"),
        ErrorKind::ReservedNameDefined
    );
}

// ---- 総称の構造体・列挙型・impl ----

#[test]
fn generic_struct_literal_and_field() {
    ok(&format!(
        "{PAIR}{}",
        func(
            "g() -> Int",
            "  let p = Pair { first: 1, second: \"a\" };\n  let s = p.second;\n  return p.first;"
        )
    ));
    assert_eq!(
        kind(&format!(
            "{PAIR}{}",
            func(
                "g() -> String",
                "  let p = Pair { first: 1, second: \"a\" };\n  return p.first;"
            )
        )),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        kind(&format!(
            "{PAIR}{}",
            func(
                "g(p: Pair<Int, String>) -> Pair<String, Int>",
                "  return p;"
            )
        )),
        ErrorKind::TypeMismatch
    );
}

#[test]
fn generic_enum_variants() {
    let maybe = "enum Maybe<T> {\n  Just(T),\n  Nothing,\n}\n";
    ok(&format!(
        "{maybe}{}",
        func("g() -> Maybe<Int>", "  let a = Maybe.Just(3);\n  return a;")
    ));
    // 型引数が決まらない
    assert_eq!(
        kind(&format!(
            "{maybe}{}",
            func("g()", "  let a = Maybe.Nothing;")
        )),
        ErrorKind::CannotInfer
    );
    ok(&format!(
        "{maybe}{}",
        func("g() -> Maybe<Int>", "  return Maybe.Nothing;")
    ));
    // 値としての構築子（関数の値）
    ok(&format!(
        "{maybe}{}{}",
        func("apply<T, U>(x: T, f: fn(T) -> U) -> U", "  return f(x);")
            .replace("effects none", "effects of(f)"),
        func("g() -> Maybe<Int>", "  return apply(4, Maybe.Just);")
    ));
}

#[test]
fn generic_impl_methods() {
    let stack = format!(
        "struct Stack<T> {{\n  top: T,\n  size: Int,\n}}\nimpl<T: Copyable> Stack<T> {{\n  fn peek(self: &Stack<T>) -> T\n{CL}  {{\n    return self.top;\n  }}\n  fn single(x: T) -> Stack<T>\n{CL}  {{\n    return Stack {{ top: x, size: 1 }};\n  }}\n}}\n"
    );
    let src = format!(
        "{stack}{}",
        func(
            "f() -> Int",
            "  let s = Stack.single(3);\n  return s.peek();"
        )
    );
    ok(&src);
    let bad = format!(
        "{stack}{}",
        func(
            "f() -> Bool",
            "  let s = Stack.single(3);\n  return s.peek();"
        )
    );
    assert_eq!(kind(&bad), ErrorKind::TypeMismatch);
}

// ---- Option / Result と `?` ----

#[test]
fn option_and_result_values() {
    body_ok(
        "f() -> Option<Int>",
        "  let a = Some(1);\n  let b: Option<Int> = None;\n  return a;",
    );
    body_ok(
        "f() -> Result<Int, String>",
        "  let r: Result<Int, String> = Err(\"x\");\n  return Ok(1);",
    );
    assert_eq!(
        body_kind("f() -> Option<Int>", "  return Some(\"a\");"),
        ErrorKind::TypeMismatch
    );
    assert_eq!(body_kind("f()", "  let x = None;"), ErrorKind::CannotInfer);
}

#[test]
fn try_operator() {
    let g = func("g() -> Option<Int>", "  return Some(1);");
    ok(&format!(
        "{g}{}",
        func(
            "f() -> Option<Int>",
            "  let x = g()?;\n  return Some(x + 1);"
        )
    ));
    let h = func("h() -> Result<Int, String>", "  return Ok(1);");
    ok(&format!(
        "{h}{}",
        func(
            "f() -> Result<Int, String>",
            "  let x = h()?;\n  return Ok(x);"
        )
    ));
    // 失敗の型が違う
    assert_eq!(
        kind(&format!(
            "{h}{}",
            func(
                "f() -> Result<Int, Int>",
                "  let x = h()?;\n  return Ok(x);"
            )
        )),
        ErrorKind::TryOperand
    );
    // Option の中で Result は使えない
    assert_eq!(
        kind(&format!(
            "{h}{}",
            func("f() -> Option<Int>", "  let x = h()?;\n  return Some(x);")
        )),
        ErrorKind::TryOperand
    );
    // 関数が Option / Result を返さない
    assert_eq!(
        kind(&format!(
            "{g}{}",
            func("f() -> Int", "  let x = g()?;\n  return x;")
        )),
        ErrorKind::TryOperand
    );
    // 値が Option / Result でない
    assert_eq!(
        body_kind("f() -> Option<Int>", "  let x = 1?;\n  return Some(x);"),
        ErrorKind::TryOperand
    );
}

// ---- match と網羅性 ----

#[test]
fn match_expression_and_statement() {
    ok(&format!(
        "{SHAPE}{}",
        func(
            "area(s: Shape) -> Int",
            "  return match s {\n    Shape.Circle(r) => r * r * 3,\n    Shape.Rect(w, h) => w * h,\n    Shape.Empty => 0,\n  };"
        )
    ));
    body_ok(
        "f(o: Option<Int>) -> Int",
        "  match o {\n    Some(x) => {\n      return x;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;",
    );
    body_ok(
        "f(n: Int) -> String",
        "  return match n {\n    0 => \"zero\",\n    -1 => \"neg\",\n    _ => \"other\",\n  };",
    );
    body_ok(
        "f(t: (Bool, Bool)) -> Int",
        "  return match t {\n    (true, _) => 1,\n    (false, true) => 2,\n    (false, false) => 3,\n  };",
    );
}

#[test]
fn match_exhaustiveness() {
    assert_eq!(
        kind(&format!(
            "{SHAPE}{}",
            func(
                "f(s: Shape) -> Int",
                "  return match s {\n    Shape.Circle(r) => r,\n    Shape.Empty => 0,\n  };"
            )
        )),
        ErrorKind::NotExhaustive
    );
    assert_eq!(
        body_kind(
            "f(b: Bool) -> Int",
            "  return match b {\n    true => 1,\n  };"
        ),
        ErrorKind::NotExhaustive
    );
    assert_eq!(
        body_kind(
            "f(o: Option<Int>) -> Int",
            "  return match o {\n    Some(x) => x,\n  };"
        ),
        ErrorKind::NotExhaustive
    );
    assert_eq!(
        body_kind(
            "f(n: Int) -> Int",
            "  return match n {\n    0 => 1,\n    1 => 2,\n  };"
        ),
        ErrorKind::NotExhaustive
    );
    assert_eq!(
        body_kind(
            "f(t: (Bool, Bool)) -> Int",
            "  return match t {\n    (true, _) => 1,\n    (false, true) => 2,\n  };"
        ),
        ErrorKind::NotExhaustive
    );
}

#[test]
fn match_pattern_errors() {
    assert_eq!(
        body_kind(
            "f(n: Int) -> Int",
            "  return match n {\n    \"a\" => 1,\n    _ => 2,\n  };"
        ),
        ErrorKind::PatternMismatch
    );
    assert_eq!(
        body_kind(
            "f(o: Option<Int>) -> Int",
            "  return match o {\n    Some(x, y) => 1,\n    None => 2,\n  };"
        ),
        ErrorKind::PatternMismatch
    );
    // 腕で束縛した名前も、外の名前と重ねられない
    assert_eq!(
        body_kind(
            "f(o: Option<Int>) -> Int",
            "  return match o {\n    Some(o) => 1,\n    None => 2,\n  };"
        ),
        ErrorKind::ShadowedName
    );
    // 腕の値の型はそろえる
    assert_eq!(
        body_kind(
            "f(b: Bool) -> Int",
            "  return match b {\n    true => 1,\n    false => \"a\",\n  };"
        ),
        ErrorKind::TypeMismatch
    );
}

// ---- クロージャ ----

#[test]
fn closures_get_types_from_context() {
    let max_by = func(
        "max_by<T>(xs: List<T>, key: fn(T) -> Int) -> Int",
        "  return 0;",
    );
    ok(&format!(
        "{ACCOUNT}{max_by}{}",
        func(
            "f(xs: List<Account>) -> Int",
            "  return max_by(xs, |a| a.balance);"
        )
    ));
    ok(&format!(
        "{max_by}{}",
        func(
            "f(xs: List<Int>, k: Int) -> Int",
            "  return max_by(xs, |a| a + k);"
        )
    ));
    ok(&format!(
        "{}{}",
        func("apply(g: fn(Int) -> Int, v: Int) -> Int", "  return g(v);")
            .replace("effects none", "effects of(g)"),
        func("f(n: Int) -> Int", "  return apply(|y| y + n, 1);")
    ));
    assert_eq!(
        kind(&format!(
            "{max_by}{}",
            func(
                "f(xs: List<Int>) -> Int",
                "  return max_by(xs, |a| a == \"x\");"
            )
        )),
        ErrorKind::OperandTypes
    );
}

#[test]
fn closure_needs_known_types() {
    assert_eq!(
        body_kind("f()", "  let g = move |a| a;"),
        ErrorKind::CannotInfer
    );
    assert_eq!(
        kind(&format!(
            "{ACCOUNT}{}",
            func("f()", "  let g = move |a| a.balance;")
        )),
        ErrorKind::CannotInfer
    );
}

// ---- T.check ----

#[test]
fn refinement_check_returns_option() {
    ok(&format!(
        "{PORT}{}",
        func("f(n: Int) -> Option<Port>", "  return Port.check(n);")
    ));
    assert_eq!(
        kind(&format!(
            "{PORT}{}",
            func("f(n: Int) -> Port", "  return Port.check(n);")
        )),
        ErrorKind::TypeMismatch
    );
    assert_eq!(
        kind(&format!(
            "{PORT}{}",
            func("f(n: Int) -> Option<Port>", "  return Port.check(\"a\");")
        )),
        ErrorKind::TypeMismatch
    );
}

// ---- 注釈の冗長さ・&mut から & ----

#[test]
fn redundant_annotation_vs_needed() {
    body_ok("f()", "  let x: Option<Int> = None;");
    assert_eq!(
        body_kind("f()", "  let x: Int = 5;"),
        ErrorKind::RedundantAnnotation
    );
    assert_eq!(
        body_kind("f()", "  let x: Option<Int> = Some(1);"),
        ErrorKind::RedundantAnnotation
    );
    ok(&format!("{PORT}{}", func("f()", "  let p: Port = 80;")));
}

#[test]
fn no_implicit_mut_to_shared() {
    let r = func("read(n: &Int) -> Int", "  return n;");
    assert_eq!(
        kind(&format!(
            "{r}{}",
            func("f(n: &mut Int) -> Int", "  return read(&mut n);")
        )),
        ErrorKind::TypeMismatch
    );
    ok(&format!(
        "{r}{}",
        func("f(n: &mut Int) -> Int", "  return read(&n);")
    ));
}

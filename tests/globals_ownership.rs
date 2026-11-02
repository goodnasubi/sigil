//! ADR 0025 補足 A-1 / A-7 の 3-4d: 大域は移動の元にならない。
//! 大域（構造体・組・そのフィールドを含む）を値で読むのは複製で、何度読んでも使用後移動にならない。
//! 借用を関数の外へ出すことも拒否される。局所の構造体の移動は従来どおり。

use sigil::check::check_with;
use sigil::effects::Options;
use sigil::error::ErrorKind;

fn run(items: &str) -> Result<(), sigil::error::Error> {
    let src = format!("module m.test\n  targets server\n  trust base\n\n{items}");
    let opts = Options {
        trust_base: vec!["m.test".to_string()],
        ..Default::default()
    };
    check_with(&src, &opts)
}

fn fn_def(name: &str, params: &str, ret: &str, body: &str) -> String {
    format!(
        "fn {name}({params}){ret}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n\n"
    )
}

fn defs() -> String {
    let mut s = String::from(
        "struct P {\n  a: Int,\n  b: Int,\n}\n\nstruct Q {\n  p: P,\n  c: Int,\n}\n\nglobal mut G: P = P { a: 1, b: 2 }\nglobal mut H: Q = Q { p: P { a: 1, b: 2 }, c: 3 }\nglobal mut T: (Int, Int) = (1, 2)\nglobal mut N: Int = 0\n\n",
    );
    s += &fn_def("take", "p: P", " -> Int", "  return p.a;");
    s += &fn_def("peek", "p: &P", " -> Int", "  return p.a;");
    s += &fn_def("poke", "p: &mut P", " -> Int", "  p.a = 1;\n  return 0;");
    s += &fn_def("pick", "a: &Int", " -> &Int", "  return a;");
    s
}

fn check_body(body: &str) -> Result<(), sigil::error::Error> {
    run(&format!("{}{}", defs(), fn_def("f", "", " -> Int", body)))
}

#[test]
fn struct_global_read_repeatedly_is_copy() {
    for b in [
        "  let a = G;\n  let b = G;\n  return a.a + b.b;",
        "  let n = take(G) + take(G);\n  return n;",
        "  let a = take(G);\n  return a + take(G) + G.a;",
        "  let a = H.p;\n  let b = H.p;\n  return a.a + b.a;",
        "  let a = H;\n  let b = H.p;\n  let c = H;\n  return a.c + b.a + c.c;",
        "  let a = T;\n  let b = T;\n  return 0;",
        "  let a = G;\n  G = P { a: 5, b: 6 };\n  let b = G;\n  return a.a + b.a;",
        "  let mut i = 0;\n  while i < 3 {\n    let a = G;\n    i = i + 1;\n  }\n  return 0;",
        "  H.p = G;\n  H.p = G;\n  return 0;",
        "  let g = move || take(G);\n  return g() + g();",
        "  return peek(&G) + poke(&mut G) + take(G);",
        "  return take(H.p) + take(H.p);",
    ] {
        if let Err(e) = check_body(b) {
            panic!("通るはず: {b}\n{e:?}");
        }
    }
}

#[test]
fn local_struct_still_moves() {
    let e = check_body(
        "  let a = P { a: 1, b: 2 };\n  let x = take(a);\n  let y = take(a);\n  return x + y;",
    )
    .expect_err("局所の構造体は移動する");
    assert_eq!(e.kind, ErrorKind::UseAfterMove);
    // 大域から複製した局所は、その局所が移動の元になる
    let e = check_body("  let p = G;\n  let x = take(p);\n  return x + take(p);")
        .expect_err("複製した局所は移動する");
    assert_eq!(e.kind, ErrorKind::UseAfterMove);
}

#[test]
fn borrow_of_global_cannot_leave_the_function() {
    let escape = |sig: (&str, &str), body: &str| {
        let src = format!("{}{}", defs(), fn_def("g", sig.0, sig.1, body));
        run(&src).expect_err(body).kind
    };
    // 引数に借用がなければ、参照を返す関数自体が書けない
    let k = escape(("", " -> &Int"), "  return pick(&N);");
    assert_eq!(k, ErrorKind::RefReturnBorrowCount);
    // 借用の引数があっても、大域を借りた参照は返せない
    for body in ["  return pick(&N);", "  return pick(&G.a);"] {
        let k = escape(("k: &Int", " -> &Int"), body);
        assert_eq!(k, ErrorKind::ReturnBorrowRoot, "{body}");
    }
    // 変数に持つこともできない
    let k = escape(("k: &Int", " -> &Int"), "  let r = pick(&N);\n  return r;");
    assert_eq!(k, ErrorKind::TempBorrow);
}

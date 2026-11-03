//! ADR 0025 補足 G の 3-5e-i（比較 `==`/`<` と `List.min/max/max_by/min_by` を、codegen の生成関数
//! `sigil_eq_<k>`/`sigil_cmp_<k>` へ切り替える）。
//!
//! 評価器とネイティブが一致すること（`common::run` が両方を走らせて突き合わせる）に加え、Rust の模型と突き合わせる。
//! 規則（G-1）: Float は IEEE（NaN は比べられない）、String は位置の順、一覧は辞書順の後に長さ、
//! 組は先頭から（先に決まれば後ろは見ない・比べられない要素は全体を比べられなくする）、Addr は番地の値（G-10）。

mod common;
use common::run;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

fn prog(defs: &str, body: &str) -> String {
    format!("{HEAD}{defs}{}", func("t() -> Int", body))
}

fn val(src: &str) -> String {
    match run(src, "t", &[]) {
        Ok(o) => o.value,
        Err(e) => panic!("{e:?}\n{src}"),
    }
}

#[test]
fn scalars() {
    let src = prog(
        "",
        r#"let mut r = 0;
if 1 < 2 { r = r + 1; }
if 2 == 2 { r = r + 2; }
if "ab" < "b" { r = r + 4; }
if "a" == "a" { r = r + 8; }
if 1.5 < 2.5 { r = r + 16; }
if true == true { r = r + 32; }
return r;"#,
    );
    assert_eq!(val(&src), "63");
}

/// 1 件 = (宣言（`a` と `b` を作る）, 期待する比べ方: -1/0/1、比べられない = 2)。
type Case = (&'static str, i64);

/// 件ごとの関数 `c<i>() -> Int`（食い違った性質の bit: 1 `==`、2 `<`、4 `>`、8 `<=`、16 `>=`）を作り、
/// `t` は最初に食い違った件の `添字 * 32 + bit` を返す（全部合えば 0）。
fn cases_src(defs: &str, cases: &[Case], ordered: bool) -> String {
    let mut s = format!("{HEAD}{defs}");
    let mut t = String::new();
    for (i, (decl, want)) in cases.iter().enumerate() {
        let (eq, lt, gt) = (*want == 0, *want == -1, *want == 1);
        let mut b = String::from(*decl);
        b.push_str("\nlet mut r = 0;\n");
        let mut chk = |bit: i64, op: &str, exp: bool| {
            b.push_str(&format!("if (a {op} b) != {exp} {{ r = r + {bit}; }}\n"));
        };
        chk(1, "==", eq);
        if ordered {
            chk(2, "<", lt);
            chk(4, ">", gt);
            chk(8, "<=", lt || eq);
            chk(16, ">=", gt || eq);
        }
        b.push_str("return r;");
        s.push_str(&func(&format!("c{i}() -> Int"), &b));
        t.push_str(&format!(
            "let r{i} = c{i}();\nif r{i} != 0 {{ return {i} * 32 + r{i}; }}\n"
        ));
    }
    t.push_str("return 0;");
    s.push_str(&func("t() -> Int", &t));
    s
}

fn check(defs: &str, cases: &[Case], ordered: bool) {
    let src = cases_src(defs, cases, ordered);
    assert_eq!(val(&src), "0", "最初に食い違った件 = 値 / 32\n{src}");
}

#[test]
fn int_bool_float_string() {
    let c: &[Case] = &[
        ("let a = 1; let b = 1;", 0),
        ("let a = -5; let b = 3;", -1),
        (
            "let a = 9223372036854775807; let b = -9223372036854775807;",
            1,
        ),
        ("let a = 0.0; let b = 0.0;", 0),
        ("let a = 1.5; let b = 2.5;", -1),
        ("let a = 0.0; let b = 0.0 * (0.0 - 1.0);", 0),
        ("let a = 1.0 / 0.0; let b = 1.0;", 1),
        ("let a = 0.0 / 0.0; let b = 1.0;", 2),
        ("let a = 0.0 / 0.0; let b = 0.0 / 0.0;", 2),
        ("let a = \"\"; let b = \"\";", 0),
        ("let a = \"\"; let b = \"a\";", -1),
        ("let a = \"ab\"; let b = \"a\";", 1),
        ("let a = \"ab\"; let b = \"b\";", -1),
        ("let a = \"あい\"; let b = \"あう\";", -1),
        ("let a = \"é\"; let b = \"z\";", 1),
        ("let a = \"abc\"; let b = \"abc\";", 0),
    ];
    check("", c, true);
}

const DEFS: &str = r#"struct P {
  k: Int,
  name: String,
}

enum Ev {
  A,
  B(Int),
  C(String, Float),
}

enum L {
  Nil,
  Cons(Int, L),
}

"#;

#[test]
fn lists_and_tuples() {
    let c: &[Case] = &[
        ("let a: List<Int> = List.new(); let b = List.new();", 0),
        ("let a = List.new(); let mut b = List.new(); b.push(1);", -1),
        ("let mut a = List.new(); a.push(1); a.push(2); let mut b = List.new(); b.push(1);", 1),
        ("let mut a = List.new(); a.push(1); a.push(2); let mut b = List.new(); b.push(1); b.push(3);", -1),
        ("let mut a = List.new(); a.push(5); let mut b = List.new(); b.push(1); b.push(3);", 1),
        ("let mut a = List.new(); a.push(\"x\"); a.push(\"y\"); let mut b = List.new(); b.push(\"x\"); b.push(\"y\");", 0),
        ("let mut a = List.new(); a.push(\"x\"); a.push(\"y\"); let mut b = List.new(); b.push(\"x\"); b.push(\"yy\");", -1),
        ("let mut a = List.new(); a.push(1.0); let mut b = List.new(); b.push(0.0 / 0.0);", 2),
        ("let mut a = List.new(); a.push(1.0); a.push(0.0 / 0.0); let mut b = List.new(); b.push(2.0); b.push(1.0);", -1),
        ("let mut a = List.new(); a.push(1.0); a.push(0.0 / 0.0); let mut b = List.new(); b.push(1.0); b.push(1.0);", 2),
        ("let mut x = List.new(); x.push(1); let mut y = List.new(); y.push(1); let mut a = List.new(); a.push(x); let mut b = List.new(); b.push(y);", 0),
        ("let a = (1, 2); let b = (1, 2);", 0),
        ("let a = (1, 2); let b = (1, 3);", -1),
        ("let a = (2, 0.0 / 0.0); let b = (1, 1.0);", 1),
        ("let a = (1, 0.0 / 0.0); let b = (1, 1.0);", 2),
        ("let a = (1.0, 0.0 / 0.0); let b = (2.0, 1.0);", -1),
        ("let a = (0.0 / 0.0, 1); let b = (0.0 / 0.0, 1);", 2),
        ("let a = (1, \"a\", 2.0); let b = (1, \"a\", 2.0);", 0),
        ("let a = ((1, 2), \"b\"); let b = ((1, 2), \"c\");", -1),
        ("let a = ((1, 0.0 / 0.0), 1); let b = ((1, 0.0 / 0.0), 2);", 2),
    ];
    check(DEFS, c, false);
}

#[test]
fn structs_enums_options() {
    let c: &[Case] = &[
        (
            "let a = P { k: 1, name: \"x\" }; let b = P { k: 1, name: \"x\" };",
            0,
        ),
        (
            "let a = P { k: 1, name: \"x\" }; let b = P { k: 1, name: \"y\" };",
            2,
        ),
        (
            "let a = P { k: 1, name: \"x\" }; let b = P { k: 2, name: \"x\" };",
            2,
        ),
        ("let a = Ev.A; let b = Ev.A;", 0),
        ("let a = Ev.A; let b = Ev.B(1);", 2),
        ("let a = Ev.B(1); let b = Ev.B(1);", 0),
        ("let a = Ev.B(1); let b = Ev.B(2);", 2),
        ("let a = Ev.C(\"q\", 1.5); let b = Ev.C(\"q\", 1.5);", 0),
        ("let a = Ev.C(\"q\", 1.5); let b = Ev.C(\"q\", 2.5);", 2),
        (
            "let a = Ev.C(\"q\", 0.0 / 0.0); let b = Ev.C(\"q\", 0.0 / 0.0);",
            2,
        ),
        ("let a = Ev.B(1); let b = Ev.C(\"q\", 1.5);", 2),
        ("let a = Some(1); let b = Some(1);", 0),
        ("let a = Some(1); let b = None;", 2),
        ("let a: Option<Int> = None; let b = None;", 0),
        ("let a = Ok(1); let b = Err(\"e\");", 2),
        (
            "let a: Result<Int, String> = Err(\"e\"); let b = Err(\"e\");",
            0,
        ),
        (
            "let a = L.Cons(1, L.Cons(2, L.Nil)); let b = L.Cons(1, L.Cons(2, L.Nil));",
            0,
        ),
        (
            "let a = L.Cons(1, L.Cons(2, L.Nil)); let b = L.Cons(1, L.Cons(3, L.Nil));",
            2,
        ),
    ];
    check(DEFS, c, false);
}

/// G-10: 番地は、組や構造体の中でも、番地の値で比べる（評価器と同じ）。
/// 検査器は `Addr` の `==` を許さないので、`Int` の組・構造体を SIR に落としてから、型の表の `Int` を `Addr` に
/// 書き換えて、ネイティブだけを走らせる（語は同じなので、評価器の値の比較と同じ結果になるはず）。
#[test]
fn addr_compares_by_value() {
    use sigil::ir::{Ty, TypeKind};
    let src = format!(
        "{HEAD}struct S {{\n  a: Int,\n  b: Int,\n}}\n\n{}",
        func(
            "t() -> Int",
            r#"let mut r = 0;
let a = (5, 1);
let b = (5, 1);
if a == b { r = r + 1; }
let c = (6, 1);
if a == c { r = r + 10; }
let d = (5, 2);
if a == d { r = r + 100; }
let s = S { a: 7, b: 1 };
let u = S { a: 7, b: 1 };
if s == u { r = r + 1000; }
let v = S { a: 8, b: 1 };
if s == v { r = r + 10000; }
return r;"#
        )
    );
    assert_eq!(val(&src), "1001");
    let mut p = sigil::ir_eval::lower_sources(&[&src], "t").unwrap();
    let mut hit = 0;
    for f in &mut p.funcs {
        for l in &mut f.locals {
            if l.ty == Ty::Tuple(vec![Ty::Int, Ty::Int]) {
                l.ty = Ty::Tuple(vec![Ty::Addr, Ty::Int]);
                hit += 1;
            }
        }
    }
    for t in &mut p.types {
        if let TypeKind::Struct { fields } = &mut t.kind {
            if fields.len() == 2 && fields[0].1 == Ty::Int {
                fields[0].1 = Ty::Addr;
                hit += 1;
            }
        }
    }
    assert!(hit >= 2, "書き換える場所が見つからない: {hit}\n{p}");
    let dir = common::native::scratch("val35e-addr");
    let exe = dir.join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let got = common::native::exec_env(&exe, &[], &[]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout.trim(), "1001", "ネイティブ（Addr）");
}

#[test]
fn bytes_shared_cell_refs() {
    let c: &[Case] = &[
        ("let a = Bytes.new(); let b = Bytes.new();", 0),
        (
            "let a = Bytes.new(); let mut b = Bytes.new(); b.push(0);",
            2,
        ),
        (
            "let mut a = Bytes.new(); a.push(1); let mut b = Bytes.new(); b.push(1);",
            0,
        ),
        (
            "let mut a = Bytes.new(); a.push(255); let mut b = Bytes.new(); b.push(0);",
            2,
        ),
        (
            "let mut a = Bytes.new(); a.push(1); a.push(2); let mut b = Bytes.new(); b.push(1);",
            2,
        ),
        ("let a = Shared.new(1); let b = Shared.new(1);", 0),
        ("let a = Shared.new(1); let b = Shared.new(2);", 2),
        ("let a = Shared.new(1); let b = a.clone();", 0),
        ("let a = Cell.new(1); let b = Cell.new(1);", 0),
        ("let a = Cell.new(\"x\"); let b = Cell.new(\"y\");", 2),
        (
            "let a = Shared.new(Cell.new(1)); let b = Shared.new(Cell.new(1));",
            0,
        ),
    ];
    check(DEFS, c, false);
}

fn refs_defs() -> String {
    let f = |sig: &str, op: &str| func(sig, &format!("return a {op} b;"));
    [
        f("eqt(a: &(Int, String), b: &(Int, String)) -> Bool", "=="),
        f("eqf(a: &Float, b: &Float) -> Bool", "=="),
        f("ltf(a: &Float, b: &Float) -> Bool", "<"),
        f("lts(a: &String, b: &String) -> Bool", "<"),
        f("eql(a: &List<Float>, b: &List<Float>) -> Bool", "=="),
    ]
    .concat()
}

#[test]
fn refs_are_compared_through_the_pointee() {
    let body = r#"let mut r = 0;
let s = (1, "a");
let t = (1, "a");
let u = (1, "b");
if eqt(&s, &t) { r = r + 1; }
if eqt(&s, &u) { r = r + 10; }
let n = 0.0 / 0.0;
if eqf(&n, &n) { r = r + 100; }
let x = 1.5;
let y = 1.5;
if eqf(&x, &y) { r = r + 1000; }
if ltf(&x, &n) { r = r + 10000; }
if ltf(&x, &2.5) { r = r + 100000; }
if lts(&"ab", &"b") { r = r + 1000000; }
let mut p = List.new();
p.push(1.0);
let mut q = List.new();
q.push(1.0);
if eql(&p, &q) { r = r + 10000000; }
q.push(n);
if eql(&p, &q) { r = r + 100000000; }
return r;"#;
    assert_eq!(val(&prog(&refs_defs(), body)), "11101001");
}

/// `List.min/max/min_by/max_by` は生成した `sigil_cmp_<k>` で比べる。同順位は先頭・比べられない値は今の約束どおり。
#[test]
fn list_min_max_family() {
    let body = r#"let mut r = 0;
let mut xs = List.new();
xs.push(3);
xs.push(-7);
xs.push(9);
xs.push(-7);
if xs.min() == -7 { r = r + 1; }
if xs.max() == 9 { r = r + 2; }
let mut ss = List.new();
ss.push("pear");
ss.push("apple");
ss.push("apricot");
ss.push("あ");
if ss.min() == "apple" { r = r + 4; }
if ss.max() == "あ" { r = r + 8; }
let mut fl = List.new();
fl.push(2.5);
fl.push(-0.5);
fl.push(1.0 / 0.0);
if fl.min() == -0.5 { r = r + 16; }
if fl.max() == 1.0 / 0.0 { r = r + 32; }
let mut ws = List.new();
ws.push("bb");
ws.push("a");
ws.push("ccc");
ws.push("dd");
let lo = ws.min_by(|w| w.len());
let hi = ws.max_by(|w| w.len());
match lo {
    Some(w) => { if w == "a" { r = r + 64; } }
    None => {}
}
match hi {
    Some(w) => { if w == "ccc" { r = r + 128; } }
    None => {}
}
let by_s = ws.max_by(|w| w.clone());
match by_s {
    Some(w) => { if w == "dd" { r = r + 256; } }
    None => {}
}
let by_f = fl.max_by(|x| 0.0 - x);
match by_f {
    Some(w) => { if w == -0.5 { r = r + 512; } }
    None => {}
}
return r;"#;
    assert_eq!(val(&prog("", body)), "1023");
}

/// 入れ子の深い再帰型の比較（反復で書いた 1000 段の鎖）。
#[test]
fn deep_enum_chain_equality() {
    let body = r#"let mut a = L.Nil;
let mut b = L.Nil;
let mut c = L.Nil;
let mut i = 0;
while i < 1000 {
    a = L.Cons(i, a);
    b = L.Cons(i, b);
    let mut k = i;
    if i == 0 { k = 5; }
    c = L.Cons(k, c);
    i = i + 1;
}
let mut r = 0;
if a == b { r = r + 1; }
if a == c { r = r + 10; }
return r;"#;
    assert_eq!(val(&prog(DEFS, body)), "1");
}

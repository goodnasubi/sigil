use sigil::fmt::{check, format};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn fnsrc(effects: &str, body: &str) -> String {
    format!(
        "{HEAD}fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}}}\n"
    )
}

fn body(stmts: &str) -> String {
    fnsrc("none", stmts)
}

fn fmt(src: &str) -> String {
    match format(src) {
        Ok(s) => s,
        Err(e) => panic!("{e}\n{src}"),
    }
}

/// 整形結果が `want` に一致し、冪等で、`want` 自体は違反なしであること。
fn expect(src: &str, want: &str) {
    let got = fmt(src);
    assert_eq!(got, want, "入力:\n{src}");
    assert_eq!(fmt(&got), got, "冪等でない");
    assert!(check(&got).unwrap().is_none());
    assert!(check(src).unwrap().is_some(), "違反として検出されない");
}

fn unchanged(src: &str) {
    assert_eq!(fmt(src), src);
    assert!(check(src).unwrap().is_none());
}

#[test]
fn examples_are_canonical() {
    for p in [
        "examples/01-average/c-brace.sigil",
        "examples/02-read-port/c-brace.sigil",
        "examples/03-ownership/ownership.sigil",
        "examples/04-effects/effects.sigil",
        "examples/05-control/control.sigil",
    ] {
        let s = std::fs::read_to_string(p).unwrap();
        assert!(check(&s).unwrap().is_none(), "{p} が正規形でない");
        assert_eq!(fmt(&s), s, "{p}");
    }
}

#[test]
fn trailing_commas() {
    expect(&body("  return f(1, 2,);\n"), &body("  return f(1, 2);\n"));
    expect(
        &format!("{HEAD}struct P {{ x: Int, y: Int, }}\n"),
        &format!("{HEAD}struct P {{\n  x: Int,\n  y: Int,\n}}\n"),
    );
    expect(
        &fnsrc("fs.read,", "  return x;\n"),
        &fnsrc("fs.read", "  return x;\n"),
    );
}

#[test]
fn redundant_parens() {
    expect(&body("  return (x);\n"), &body("  return x;\n"));
    expect(
        &body("  return (x + 1) + 2;\n"),
        &body("  return x + 1 + 2;\n"),
    );
    expect(&body("  return x * (y);\n"), &body("  return x * y;\n"));
}

#[test]
fn needed_parens_stay() {
    unchanged(&body("  return (x + 1) * 2;\n"));
    unchanged(&body("  return x - (y - z);\n"));
    unchanged(&body("  return (a || b) && c;\n"));
    unchanged(&body("  return (1).abs();\n"));
    unchanged(&body("  return (&(&x)).len();\n"));
}

#[test]
fn comment_spacing() {
    expect(
        &body("  //x\n  return x;\n"),
        &body("  // x\n  return x;\n"),
    );
    expect(&body("  return x; // c\n"), &body("  return x;  // c\n"));
    expect(
        &body("  return x;   //c   \n"),
        &body("  return x;  // c\n"),
    );
    unchanged(&body("  //\n  return x;\n"));
}

#[test]
fn return_unit_removed() {
    let src = format!(
        "{HEAD}fn g()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let a = 1;\n  return;\n}}\n"
    );
    let want = src.replace("  return;\n", "");
    expect(&src, &want);
}

#[test]
fn closure_single_return() {
    expect(
        &body("  let c = move |y| { return y + 1; };\n  return x;\n"),
        &body("  let c = move |y| y + 1;\n  return x;\n"),
    );
}

#[test]
fn stmt_match_single_return() {
    expect(
        &body("  match x {\n    1 => { return 1; }\n    _ => { return 2; }\n  }\n"),
        &body("  return match x {\n    1 => 1,\n    _ => 2,\n  };\n"),
    );
}

#[test]
fn value_match_final_comma() {
    expect(
        &body("  let y = match x {\n    1 => 2,\n    _ => 3\n  };\n  return y;\n"),
        &body("  let y = match x {\n    1 => 2,\n    _ => 3,\n  };\n  return y;\n"),
    );
}

#[test]
fn effects_order_and_fold() {
    expect(
        &fnsrc("fs.write, console, fs.read", "  return x;\n"),
        &fnsrc("console, fs", "  return x;\n"),
    );
    expect(
        &fnsrc("console, console", "  return x;\n"),
        &fnsrc("console", "  return x;\n"),
    );
    expect(
        &fnsrc("fs, fs.read", "  return x;\n"),
        &fnsrc("fs", "  return x;\n"),
    );
    unchanged(&fnsrc("console, fs.read", "  return x;\n"));
}

#[test]
fn effects_of_first() {
    let src = format!(
        "{HEAD}fn g(f: fn(Int) -> Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects console, of(f)\n{{\n  return 1;\n}}\n"
    );
    let want = src.replace("console, of(f)", "of(f), console");
    expect(&src, &want);
}

#[test]
fn literals() {
    expect(&body("  return 1.50;\n"), &body("  return 1.5;\n"));
    unchanged(&body("  return 1.0;\n"));
    expect(
        &body("  return \"a\\u{9}b\";\n"),
        &body("  return \"a\\tb\";\n"),
    );
}

#[test]
fn layout() {
    expect(&body("    return x;\n"), &body("  return x;\n"));
    expect(&body("  return x;  \n"), &body("  return x;\n"));
    let s = body("  return x;\n");
    expect(s.trim_end_matches('\n'), &s);
}

#[test]
fn blank_lines() {
    unchanged(&body("  let a = 1;\n\n  return a;\n"));
    expect(
        &body("  let a = 1;\n\n\n  return a;\n"),
        &body("  let a = 1;\n\n  return a;\n"),
    );
}

#[test]
fn struct_lit_in_condition_keeps_parens() {
    let src = body("  if (P { x: 1 }) == p {\n    return 1;\n  }\n  return 2;\n");
    unchanged(&src);
}

#[test]
fn idempotent_on_messy_input() {
    let src = std::fs::read_to_string("tests/data/messy.sigil").unwrap();
    let once = fmt(&src);
    assert_ne!(once, src);
    assert_eq!(fmt(&once), once);
}

#[test]
fn check_reports_line() {
    let src = body("  //x\n  return x;\n");
    let d = check(&src).unwrap().unwrap();
    assert_eq!(d.found, "  //x");
    assert_eq!(d.expected, "  // x");
    assert_eq!(d.line, src.lines().position(|l| l == "  //x").unwrap() + 1);
}

#[test]
fn parse_error_is_error() {
    assert!(format("fn").is_err());
    assert!(check("fn").is_err());
}

// ---- レビュー指摘の回帰テスト ----

#[test]
fn stmt_start_match_keeps_parens() {
    let s = body("  (match x { 1 => 2, _ => 3 }).g();\n  return x;\n");
    let want = body("  (match x {\n    1 => 2,\n    _ => 3,\n  }).g();\n  return x;\n");
    expect(&s, &want);
    unchanged(&body(
        "  (match x {\n    1 => 2,\n    _ => 3,\n  }).g().h();\n  return x;\n",
    ));
    // 先頭でなければ括弧は不要
    expect(
        &body("  let y = (match x { 1 => 2, _ => 3 }).g();\n  return y;\n"),
        &body("  let y = match x {\n    1 => 2,\n    _ => 3,\n  }.g();\n  return y;\n"),
    );
}

#[test]
fn doc_comments_kept() {
    unchanged(&body("  /// doc\n  return x;\n"));
    unchanged(&body("  //// four\n  return x;\n"));
    unchanged(&body("  //  two spaces\n  return x;\n"));
    expect(
        &body("  //!x\n  return x;\n"),
        &body("  // !x\n  return x;\n"),
    );
    expect(
        &body("  return x;  ///d   \n"),
        &body("  return x;  ///d\n"),
    );
}

#[test]
fn deep_output_is_explicit_error() {
    // 入力が上限内で通る最大の入れ子について、成功するか明示エラーであること。
    let ok_depth = |mk: &dyn Fn(usize) -> String| {
        (1..=256)
            .rev()
            .find(|&n| sigil::parse(&body(&format!("  return {};\n", mk(n)))).is_ok())
            .unwrap()
    };
    // 不要な括弧は消えるので、出力は浅くなって成功する。
    let mk = |n: usize| format!("{}x{}", "(".repeat(n), ")".repeat(n));
    let n = ok_depth(&mk);
    assert!(n >= 50, "{n}");
    assert_eq!(
        format(&body(&format!("  return {};\n", mk(n)))).unwrap(),
        body("  return x;\n")
    );
    // 必要な括弧は残る。成功するなら再整形で変わらず、失敗なら FmtVerify か TooDeep。
    let mk = |n: usize| format!("{}1{}", "(1 + ".repeat(n), ")".repeat(n));
    let n = ok_depth(&mk);
    match format(&body(&format!("  return {};\n", mk(n)))) {
        Ok(out) => assert_eq!(fmt(&out), out),
        Err(err) => assert!(
            err.kind == sigil::ErrorKind::TooDeep || err.kind == sigil::ErrorKind::FmtVerify
        ),
    }
}

/// 入力がパースできる最大の n（境界）。
fn max_ok(mk: &dyn Fn(usize) -> String) -> usize {
    (1..=1000)
        .rev()
        .find(|&n| sigil::parse(&mk(n)).is_ok())
        .expect("パースできる n がない")
}

/// 境界直下の入力で、format が成功して冪等か、TooDeep / FmtVerify のどちらかであること。
fn check_boundary(mk: &dyn Fn(usize) -> String) {
    let n = max_ok(mk);
    assert!(n >= 50, "境界が浅すぎる: {n}");
    assert!(sigil::parse(&mk(n)).is_ok());
    assert!(
        sigil::parse(&mk(n + 1)).is_err(),
        "n+1 でパースできてしまう"
    );
    // n-1 は必ず整形でき、冪等でなければならない。
    let below = format(&mk(n - 1)).expect("n-1 の整形に失敗");
    assert_eq!(fmt(&below), below, "n-1 の整形結果が冪等でない");
    // n は境界そのもの。整形できるなら冪等、できないなら深さ系のエラーだけを許す。
    {
        let src = mk(n);
        match format(&src) {
            Ok(out) => assert_eq!(fmt(&out), out),
            Err(e) => assert!(
                e.kind == sigil::ErrorKind::TooDeep || e.kind == sigil::ErrorKind::FmtVerify,
                "{e}"
            ),
        }
    }
}

#[test]
fn long_chains_do_not_break_verification() {
    check_boundary(&|n| body(&format!("  return x{};\n", " + 1".repeat(n))));
    check_boundary(&|n| body(&format!("  return x{};\n", ".a".repeat(n))));
    check_boundary(&|n| {
        let mut s = String::from("  if a {\n    return 1;\n  }");
        for _ in 0..n {
            s.push_str(" else if a {\n    return 1;\n  }");
        }
        s.push('\n');
        body(&s)
    });
}

#[test]
fn trailing_comment_on_removed_line_not_moved_to_neighbour() {
    let src = format!(
        "{HEAD}fn g()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let a = 1;\n  return;  // bye\n}}\n"
    );
    let want = format!(
        "{HEAD}fn g()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let a = 1;\n  // bye\n}}\n"
    );
    expect(&src, &want);
}

#[test]
fn trailing_comments_not_concatenated() {
    let src = fnsrc("fs.read,  // a\n    fs.write  // b", "  return x;\n");
    let out = fmt(&src);
    assert_eq!(out.matches("//").count(), 2, "{out}");
    assert!(!out.lines().any(|l| l.matches("//").count() > 1), "{out}");
    assert_eq!(fmt(&out), out);
}

#[test]
fn fold_keeps_comment_count() {
    let src = fnsrc("fs.read, fs.write", "  return x;  // r\n");
    let out = fmt(&src);
    assert!(out.contains("return x;  // r"));
}

#[cfg(unix)]
#[test]
fn cli_writes_atomically_and_keeps_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("sigil-fmt-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.sigil");
    std::fs::write(&f, body("  //x\n  return x;\n")).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o640)).unwrap();
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["fmt", f.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(st.success());
    assert_eq!(
        std::fs::read_to_string(&f).unwrap(),
        body("  // x\n  return x;\n")
    );
    assert_eq!(
        std::fs::metadata(&f).unwrap().permissions().mode() & 0o777,
        0o640
    );
    // 一時ファイルが残らない
    let n = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(n, 1);
    // 構文エラーのファイルは変更しない
    let g = dir.join("b.sigil");
    std::fs::write(&g, "fn").unwrap();
    let st = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["fmt", g.to_str().unwrap()])
        .status()
        .unwrap();
    assert!(!st.success());
    assert_eq!(std::fs::read_to_string(&g).unwrap(), "fn");
    std::fs::remove_dir_all(&dir).unwrap();
}

// ---- M1: ADR 0011 の構文の拡張と `-> Unit` の除去 ----

#[test]
fn messy_has_expected_output() {
    let src = std::fs::read_to_string("tests/data/messy.sigil").unwrap();
    let want = std::fs::read_to_string("tests/data/messy.expected.sigil").unwrap();
    assert_eq!(fmt(&src), want);
    assert!(!want.contains("-> Unit"));
    assert!(check(&want).unwrap().is_none());
}

#[test]
fn unit_return_type_is_removed() {
    let with = format!(
        "{HEAD}fn g()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{}}\n"
    );
    let src = with.replace("fn g()", "fn g() -> Unit");
    expect(&src, &with);
    // 末尾の `return;` も取り除かれる
    let src2 = src.replace("{}", "{\n  return;\n}");
    expect(&src2, &with);
    // 関数の型
    expect(
        &format!("{HEAD}fn a(f: fn(Int) -> Unit)\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(f)\n{{}}\n"),
        &format!("{HEAD}fn a(f: fn(Int))\n  intent \"x\"\n  requires none\n  ensures none\n  effects of(f)\n{{}}\n"),
    );
    // 型引数の `Unit` は残る
    unchanged(&format!(
        "{HEAD}fn s() -> Result<Unit, E>\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return Ok(Unit);\n}}\n"
    ));
}

#[test]
fn constraints_are_sorted_and_deduplicated() {
    let f = |c: &str| {
        format!("{HEAD}fn a<T: {c}>(x: T)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{}}\n")
    };
    expect(&f("Sendable + Equatable"), &f("Equatable + Sendable"));
    expect(
        &f("Sendable + Equatable + Sendable"),
        &f("Equatable + Sendable"),
    );
    unchanged(&f("Comparable + Equatable"));
}

#[test]
fn let_annotation_and_destructuring() {
    unchanged(&body("  let a: Int = 1;\n  let mut b: Float = 1.5;\n  let (c, d) = (1, 2);\n  let (_, (e, f)) = (1, (2, 3));\n  return x;\n"));
    expect(
        &body("  let a:Int=1;\n  let ( c ,d )=t;\n  return x;\n"),
        &body("  let a: Int = 1;\n  let (c, d) = t;\n  return x;\n"),
    );
}

#[test]
fn impl_blocks() {
    let m = "  fn get(self: &Account) -> Int\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {\n    return self.balance;\n  }\n";
    unchanged(&format!("{HEAD}impl Account {{\n{m}}}\n"));
    unchanged(&format!("{HEAD}impl<T> Box<T> {{\n{m}}}\n"));
    expect(
        &format!("{HEAD}impl Account {{\n  fn get(self: &Account) -> Int\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n  {{ return self.balance; }}\n}}\n"),
        &format!("{HEAD}impl Account {{\n{m}}}\n"),
    );
}

#[test]
fn unchecked_clause_round_trips() {
    let src = format!(
        "{HEAD}fn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"xs は確保した領域を指す\"\n{{\n  return x;\n}}\n"
    );
    assert_eq!(fmt(&src), src);
    assert!(check(&src).unwrap().is_none());
    // 字下げの崩れは直る。
    let messy = src.replace("\n  unchecked", "\n      unchecked");
    expect(&messy, &src);
}

use sigil::spec::spec;

const CASES: &[&str] = &[
    "examples/01-average/c-brace",
    "examples/02-read-port/c-brace",
    "examples/03-ownership/ownership",
    "examples/04-effects/effects",
    "examples/05-control/control",
];

#[test]
fn examples_match_expected_output() {
    for c in CASES {
        let src = std::fs::read_to_string(format!("{c}.sigil")).unwrap();
        let want = std::fs::read_to_string(format!("{c}.spec")).unwrap();
        assert_eq!(spec(&src).unwrap(), want, "{c}");
    }
}

#[test]
fn adr0008_decision7_example() {
    // ADR 0008 決定 7 のコードブロックと一致する。
    let src = std::fs::read_to_string("examples/03-ownership/ownership.sigil").unwrap();
    let out = spec(&src).unwrap();
    let want = "モジュール examples.ownership
  出力先: ブラウザ, サーバー
  信頼基盤: いいえ

関数 deposit
  意図: 口座に入金する
  引数: account を書き換える（Account）、amount を受け取る（Int）
  戻り値: なし
  事前条件: amount > 0
  事後条件: account.balance == old(account.balance) + amount
  効果: なし
";
    let module = want.split("\n\n").next().unwrap();
    let deposit = want.split("\n\n").nth(1).unwrap();
    assert!(out.starts_with(module));
    assert!(out.contains(deposit));
}

#[test]
fn body_is_not_included() {
    let src = std::fs::read_to_string("examples/03-ownership/ownership.sigil").unwrap();
    let out = spec(&src).unwrap();
    assert!(!out.contains("return"));
    assert!(!out.contains("let "));
}

#[test]
fn trust_base_and_generics_and_expression_forms() {
    let src = "module m.a\n  targets browser\n  trust base\n\nfn f<T: Sendable>(xs: &mut List<T>, g: fn(Int) -> Int) -> Option<T>\n  intent \"a\\nb\"\n  requires ((xs.len()) > 0)\n  ensures old(xs.len()) == result.len()\n  effects task.spawn, of(g), task.suspend, task.share, fs.read\n{\n  return None;\n}\n";
    let out = spec(src).unwrap();
    assert!(out.contains("出力先: ブラウザ\n  信頼基盤: はい"));
    assert!(out.contains("関数 f<T: Sendable>"));
    assert!(out.contains("意図: a\\nb"));
    assert!(out.contains("xs を書き換える（List<T>）、g を呼ぶ（fn(Int) -> Int）"));
    // 式は正規形の表記（冗長な括弧は落ちる）、old を含む
    assert!(out.contains("事前条件: xs.len() > 0"));
    assert!(out.contains("事後条件: old(xs.len()) == result.len()"));
    // 効果は正規形の順と畳み込み（of 先頭、task は畳む）
    assert!(
        out.contains("効果: g の効果、ファイルを読む、タスクを起動する・中断しうる・値を共有する")
    );
}

#[test]
fn syntax_error_is_error() {
    assert!(spec("fn").is_err());
}

#[test]
fn intent_line_separators_and_format_chars_are_escaped() {
    // U+2028/U+2029（改行扱いの空白）、U+200B・U+202E（書式文字）、U+00A0（空白）は生で出さない。
    let src = "module m.a\n  targets server\n  trust none\n\nfn f()\n  intent \"a\u{2028}b\u{2029}c\u{200B}d\u{202E}e\u{A0}f\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\n";
    let out = spec(src).unwrap();
    assert!(
        out.contains("意図: a\\u{2028}b\\u{2029}c\\u{200B}d\\u{202E}e\\u{A0}f"),
        "{out}"
    );
    for c in ['\u{2028}', '\u{2029}', '\u{200B}', '\u{202E}', '\u{A0}'] {
        assert!(!out.contains(c), "生の U+{:X} が残っている", c as u32);
    }
    // 通常の文字（日本語・通常の空白）はそのまま。
    let src = src.replace("a\u{2028}b\u{2029}c\u{200B}d\u{202E}e\u{A0}f", "日本語 ok");
    assert!(spec(&src).unwrap().contains("意図: 日本語 ok"));
}

#[test]
fn impl_methods_and_constraints_in_spec() {
    let src = "module m.a\n  targets server\n  trust none\n\nstruct Account {\n  balance: Int,\n}\n\nimpl Account {\n  fn deposit(self: &mut Account, n: Int)\n    intent \"入金する\"\n    requires n > 0\n    ensures none\n    effects none\n  {}\n}\n\nfn eq<T: Sendable + Equatable>(a: T)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{}\n";
    let out = spec(src).unwrap();
    assert!(out.contains("メソッド Account.deposit"), "{out}");
    assert!(out.contains("戻り値: なし"), "{out}");
    assert!(out.contains("Equatable + Sendable"), "{out}");
    let with_unit = src.replace(
        "deposit(self: &mut Account, n: Int)",
        "deposit(self: &mut Account, n: Int) -> Unit",
    );
    assert_eq!(spec(&with_unit).unwrap(), out);
}

#[test]
fn spec_shows_unchecked_premise_and_count() {
    let src = "module m.a\n  targets server\n  trust base\n\nfn f(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"領域は確保済み\"\n{\n  return x;\n}\n\nfn g(x: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return x;\n}\n";
    let out = spec(src).unwrap();
    assert!(
        out.contains("信頼基盤: はい（検査を外す関数 1 個）"),
        "{out}"
    );
    assert!(
        out.contains("  効果: なし\n  検査を外す: 領域は確保済み\n"),
        "{out}"
    );
    // 節のない関数には出ない。
    assert_eq!(out.matches("検査を外す:").count(), 1);
    let none = spec(&src.replace("trust base", "trust none")).unwrap();
    assert!(none.contains("信頼基盤: いいえ\n"));
}

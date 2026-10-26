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

// ---- ADR 0016 の決定 6（PR2: 使う原始操作・推論できない効果） ----

fn raw_opts() -> sigil::effects::Options {
    use sigil::effects::RawOp;
    use sigil::stdlib::{Layer, DELEGATED};
    let op = |layer, sig: &str, effects: &str, premise: &str| RawOp {
        layer,
        signature: sig.to_string(),
        effects: effects.to_string(),
        requires: "none".to_string(),
        premise: premise.to_string(),
    };
    sigil::effects::Options {
        raw_ops: vec![
            op(
                Layer::Core,
                "alloc(size: Int) -> Addr",
                "state",
                "確保器が有効",
            ),
            op(
                Layer::Core,
                "read_int(a: &Addr) -> Int",
                "none",
                "領域が生きている",
            ),
            op(Layer::Os, "syscall(n: Int) -> Int", DELEGATED, "番号が有効"),
        ],
        ..Default::default()
    }
}

fn ufn(name: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {name}(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n  unchecked \"領域は確保済み\"\n{{\n{body}\n}}\n\n"
    )
}

#[test]
fn spec_shows_unchecked_premise_and_raw_ops() {
    let body = "  let a = raw.alloc(n);\n  let b = raw.alloc(n);\n  return raw.read_int(&a) + raw.read_int(&b);";
    let src = format!(
        "module m.a\n  targets server\n  trust base\n\n{}",
        ufn("f", "state", body)
    );
    let out = sigil::spec::spec_with(&src, &raw_opts()).unwrap();
    // 名前の順で、重複なく、前提つき。
    assert!(
        out.contains("  検査を外す: 領域は確保済み\n  使う原始操作: raw.alloc（前提: 確保器が有効）, raw.read_int（前提: 領域が生きている）\n"),
        "{out}"
    );
    // 宣言に委ねる操作を呼ばないので、推論できない効果の行は出ない。
    assert!(!out.contains("推論できない効果"), "{out}");
    assert!(
        out.contains("信頼基盤: はい（検査を外す関数 1 個）"),
        "{out}"
    );
}

#[test]
fn spec_shows_effects_not_inferred() {
    let o = raw_opts();
    let mk = |effects: &str| {
        format!(
            "module m.a\n  targets server\n  trust base\n\n{}",
            ufn("f", effects, "  return raw.syscall(n);")
        )
    };
    let out = sigil::spec::spec_with(&mk("fs.read, fs.write"), &o).unwrap();
    assert!(
        out.contains("  使う原始操作: raw.syscall（前提: 番号が有効）\n  推論できない効果: fs.read, fs.write\n"),
        "{out}"
    );
    let out = sigil::spec::spec_with(&mk("none"), &o).unwrap();
    assert!(out.contains("  推論できない効果: なし\n"), "{out}");
    // 検査が失敗したファイル。
    let bad = mk("none").replace("raw.syscall(n)", "nope");
    let out = sigil::spec::spec_with(&bad, &o).unwrap();
    assert!(
        out.contains("  使う原始操作: （検査のエラーのため表示できない）\n  推論できない効果: （検査のエラーのため表示できない）\n"),
        "{out}"
    );
    // `unchecked` の関数がなければ検査器は呼ばず、行も出ない。
    let plain = "module m.a\n  targets server\n  trust none\n\nfn g(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n";
    let out = sigil::spec::spec_with(plain, &o).unwrap();
    assert!(
        !out.contains("原始操作") && !out.contains("推論できない"),
        "{out}"
    );
}

#[test]
fn spec_without_raw_ops_says_none() {
    let src = format!(
        "module m.a\n  targets server\n  trust base\n\n{}",
        ufn("f", "none", "  return n;")
    );
    let out = spec(&src).unwrap();
    assert!(
        out.contains("  検査を外す: 領域は確保済み\n  使う原始操作: なし\n"),
        "{out}"
    );
}

#[test]
fn spec_counts_unchecked_in_impl_methods() {
    // `impl` のメソッドに節を書いたものも、モジュールの行の数に入る。
    let m = |unchecked: bool| {
        format!(
            "fn {}(self: &S) -> Int\n    intent \"x\"\n    requires none\n    ensures none\n    effects none\n{}  {{\n    return 1;\n  }}\n",
            "get",
            if unchecked { "    unchecked \"前提\"\n" } else { "" }
        )
    };
    let src = format!(
        "module m.a\n  targets server\n  trust base\n\nstruct S {{\n  n: Int\n}}\n\nimpl S {{\n  {}  {}}}\n\n{}",
        m(true),
        m(true).replace("get", "get2"),
        ufn("f", "none", "  return n;")
    );
    let out = spec(&src).unwrap();
    assert!(
        out.contains("信頼基盤: はい（検査を外す関数 3 個）"),
        "{out}"
    );
    assert_eq!(out.matches("  検査を外す:").count(), 3, "{out}");
    // 節のないメソッドは数えない。
    let src2 = src.replace("    unchecked \"前提\"\n", "");
    let out = spec(&src2).unwrap();
    assert!(
        out.contains("信頼基盤: はい（検査を外す関数 1 個）"),
        "{out}"
    );
}

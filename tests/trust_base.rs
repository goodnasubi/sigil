//! ADR 0016 の PR2: 信頼基盤の許可・`unchecked` の検査・原始操作。
//!
//! 原始操作の表（`stdlib::RAW_OPS`）は PR2 では空なので、テストは `Options.raw_ops` に
//! テスト用の原始操作（`alloc`・`read_int`・`write_int`・`syscall`）を足して検査する。

use sigil::check::check_with;
use sigil::effects::{Options, RawOp};
use sigil::stdlib::{Layer, DELEGATED};
use sigil::ErrorKind;

fn op(layer: Layer, sig: &str, effects: &str) -> RawOp {
    RawOp {
        layer,
        signature: sig.to_string(),
        effects: effects.to_string(),
        requires: "none".to_string(),
        premise: format!("{sig} の前提"),
    }
}

fn opts(trust_base: &[&str]) -> Options {
    Options {
        trust_base: trust_base.iter().map(|s| s.to_string()).collect(),
        raw_ops: vec![
            op(Layer::Core, "alloc(size: Int) -> Addr", "state"),
            op(Layer::Core, "read_int(a: &Addr) -> Int", "none"),
            op(Layer::Core, "write_int(a: &Addr, v: Int)", "state"),
            op(Layer::Os, "syscall(n: Int) -> Int", DELEGATED),
        ],
        ..Options::default()
    }
}

fn module(name: &str, targets: &str, trust: &str, items: &str) -> String {
    format!("module {name}\n  targets {targets}\n  trust {trust}\n\n{items}")
}

/// `unchecked` つきの関数。
fn ufn(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n  unchecked \"前提\"\n{{\n{body}\n}}\n\n"
    )
}

fn fnn(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

fn kind(src: &str, o: &Options) -> ErrorKind {
    match check_with(src, o) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => e.kind,
    }
}

fn ok(src: &str, o: &Options) {
    if let Err(e) = check_with(src, o) {
        panic!("{e}\n{src}");
    }
}

const BODY_OK: &str =
    "  let a = raw.alloc(n);\n  raw.write_int(&a, n);\n  return raw.read_int(&a);";

fn base(items: &str) -> String {
    module("m.a", "server", "base", items)
}

#[test]
fn unchecked_outside_trust_base_rejected() {
    let src = module(
        "m.a",
        "server",
        "none",
        &ufn("f(n: Int) -> Int", "none", "  return n;"),
    );
    // 許可の名前を挙げても、`trust none` のモジュールでは節を書けない。
    assert_eq!(kind(&src, &opts(&[])), ErrorKind::UncheckedOutsideTrustBase);
    assert_eq!(
        kind(&src, &opts(&["m.a"])),
        ErrorKind::UncheckedOutsideTrustBase
    );
}

#[test]
fn trust_base_not_allowed_without_option() {
    let src = base(&ufn("f(n: Int) -> Int", "state", BODY_OK));
    assert_eq!(kind(&src, &opts(&[])), ErrorKind::TrustBaseNotAllowed);
    // 別のモジュール名を挙げても許可にならない。
    assert_eq!(kind(&src, &opts(&["m.b"])), ErrorKind::TrustBaseNotAllowed);
    // `unchecked` がなくても、宣言だけで許可が要る。
    let plain = base(&fnn("f(n: Int) -> Int", "none", "  return n;"));
    assert_eq!(kind(&plain, &opts(&[])), ErrorKind::TrustBaseNotAllowed);
}

#[test]
fn trust_base_allowed_with_option() {
    let src = base(&ufn("f(n: Int) -> Int", "state", BODY_OK));
    ok(&src, &opts(&["m.a"]));
    ok(&src, &opts(&["m.b", "m.a"]));
}

#[test]
fn user_std_module_name_rejected() {
    for name in ["std", "std.x", "std.core.list"] {
        let src = module(
            name,
            "server",
            "none",
            &fnn("f(n: Int) -> Int", "none", "  return n;"),
        );
        assert_eq!(
            kind(&src, &opts(&[])),
            ErrorKind::ReservedModuleName,
            "{name}"
        );
        // 許可に挙げても予約は外れない。
        let b = module(
            name,
            "server",
            "base",
            &fnn("f(n: Int) -> Int", "none", "  return n;"),
        );
        assert_eq!(
            kind(&b, &opts(&[name])),
            ErrorKind::ReservedModuleName,
            "{name}"
        );
    }
    // `stdx` のような名前は予約ではない。
    ok(
        &module(
            "stdx.a",
            "server",
            "none",
            &fnn("f(n: Int) -> Int", "none", "  return n;"),
        ),
        &opts(&[]),
    );
}

#[test]
fn raw_outside_unchecked_rejected() {
    // 節のない関数。
    let src = base(&fnn("f(n: Int) -> Int", "state", BODY_OK));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
    // `trust none` のモジュールでも同じ（節がないので `RawOutsideUnchecked`）。
    let src = module(
        "m.a",
        "server",
        "none",
        &fnn("f(n: Int) -> Int", "state", BODY_OK),
    );
    assert_eq!(kind(&src, &opts(&[])), ErrorKind::RawOutsideUnchecked);
}

#[test]
fn raw_in_closure_rejected() {
    let body = "  let ys = xs.map(|x| {\n    return raw.syscall(x);\n  });\n  return n;";
    let src = base(&ufn("f(n: Int, xs: &List<Int>) -> Int", "none", body));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
}

#[test]
fn raw_in_contract_rejected() {
    let src = base(
        "fn f(a: &Addr) -> Int\n  intent \"x\"\n  requires raw.read_int(a) > 0\n  ensures none\n  effects none\n  unchecked \"前提\"\n{\n  return 1;\n}\n",
    );
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
    let src = base(
        "fn f(a: &Addr) -> Int\n  intent \"x\"\n  requires none\n  ensures raw.read_int(a) > 0\n  effects none\n  unchecked \"前提\"\n{\n  return 1;\n}\n",
    );
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
    let src = base(&format!(
        "type Pos = Int where raw.syscall(1) > 0 && self > 0;\n\n{}",
        ufn("f(n: Int) -> Int", "none", "  return n;")
    ));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
}

#[test]
fn unchecked_on_impl_method() {
    let impl_src = |trust: &str, body: &str| {
        module(
            "m.a",
            "server",
            trust,
            &format!(
                "struct S {{\n  n: Int\n}}\n\nimpl S {{\n  fn get(self: &S) -> Int\n    intent \"x\"\n    requires none\n    ensures none\n    effects state\n    unchecked \"前提\"\n  {{\n{body}\n  }}\n}}\n"
            ),
        )
    };
    let good = impl_src(
        "base",
        "    let a = raw.alloc(1);\n    return raw.read_int(&a);",
    );
    ok(&good, &opts(&["m.a"]));
    // メソッドの節も `trust none` では UncheckedOutsideTrustBase。
    let none = impl_src("none", "    return 1;");
    assert_eq!(
        kind(&none, &opts(&[])),
        ErrorKind::UncheckedOutsideTrustBase
    );
}

#[test]
fn raw_as_function_value_rejected() {
    let src = base(&ufn(
        "f(n: Int) -> Int",
        "state",
        "  let g = raw.alloc;\n  return n;",
    ));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
    let src = base(&format!(
        "{}{}",
        fnn(
            "apply(g: fn(Int) -> Addr) -> Int",
            "of(g)",
            "  let a = g(1);\n  return 1;"
        ),
        ufn("f(n: Int) -> Int", "state", "  return apply(raw.alloc);")
    ));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
}

#[test]
fn raw_in_generic_unchecked_fn() {
    let src = base(&ufn("f<T>(x: T, n: Int) -> Int", "state", BODY_OK));
    ok(&src, &opts(&["m.a"]));
}

#[test]
fn raw_via_generic_callee_rejected() {
    // 総称の関数に `unchecked` はなく、そこから原始操作は呼べない。呼び出し側の `unchecked` でも救われない。
    let src = base(&format!(
        "{}{}",
        fnn("g<T>(x: T, n: Int) -> Int", "state", BODY_OK),
        ufn("f(n: Int) -> Int", "state", "  return g(n, n);")
    ));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::RawOutsideUnchecked);
}

#[test]
fn error_priority() {
    // ReservedModuleName > TrustBaseNotAllowed > 関数ごと（UncheckedOutsideTrustBase など）。
    let bad = ufn("f(n: Int) -> Int", "none", "  return undefined_name;");
    let r = module("std.x", "server", "base", &bad);
    assert_eq!(kind(&r, &opts(&[])), ErrorKind::ReservedModuleName);
    let t = module("m.a", "server", "base", &bad);
    assert_eq!(kind(&t, &opts(&[])), ErrorKind::TrustBaseNotAllowed);
    // 関数ごと: 同じ関数では節の検査が本体の名前の解決より先。
    let u = module("m.a", "server", "none", &bad);
    assert_eq!(kind(&u, &opts(&[])), ErrorKind::UncheckedOutsideTrustBase);
    // 関数ごとはソースの順。先の関数の誤りが出る。
    let two = module(
        "m.a",
        "server",
        "none",
        &format!(
            "{}{}",
            fnn("a(n: Int) -> Int", "none", "  return nope;"),
            ufn("b(n: Int) -> Int", "none", "  return n;")
        ),
    );
    assert_eq!(kind(&two, &opts(&[])), ErrorKind::UnknownName);
    let two = module(
        "m.a",
        "server",
        "none",
        &format!(
            "{}{}",
            ufn("b(n: Int) -> Int", "none", "  return n;"),
            fnn("a(n: Int) -> Int", "none", "  return nope;")
        ),
    );
    assert_eq!(kind(&two, &opts(&[])), ErrorKind::UncheckedOutsideTrustBase);
}

#[test]
fn unchecked_keeps_move_and_borrow_check() {
    // 移動済みの値の再利用。
    let body = "  let a = raw.alloc(n);\n  let t = s;\n  let u = s;\n  return n;";
    let src = base(&ufn("f(n: Int, s: String) -> Int", "state", body));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::UseAfterMove);
    // 型の不一致。
    let src = base(&ufn("f(n: Int) -> Int", "state", "  return true;"));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::TypeMismatch);
}

#[test]
fn unchecked_effects_exact_without_delegated_op() {
    // 宣言に委ねない原始操作（効果 state）は、効果が厳密に照合される。
    let src = base(&ufn("f(n: Int) -> Int", "none", BODY_OK));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::EffectMissing);
    // 過大な宣言も（委ねる操作を呼ばない限り）誤り。
    let src = base(&ufn("f(n: Int) -> Int", "state, fs.read", BODY_OK));
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::EffectExtra);
}

const OS_BODY: &str = "  return raw.syscall(n);";

#[test]
fn delegated_op_allows_declared_superset() {
    let o = opts(&["m.a"]);
    // 推論された効果（なし）より多く宣言してよい。
    ok(
        &base(&ufn("f(n: Int) -> Int", "fs.read, fs.write", OS_BODY)),
        &o,
    );
    ok(&base(&ufn("f(n: Int) -> Int", "none", OS_BODY)), &o);
    // 推論された効果より少ない宣言は誤り。
    let body = "  console.log(&n);\n  return raw.syscall(n);";
    let src = base(&ufn("f(n: Int) -> Int", "fs.read", body));
    assert_eq!(kind(&src, &o), ErrorKind::EffectMissing);
    ok(
        &base(&ufn("f(n: Int) -> Int", "console, fs.read", body)),
        &o,
    );
}

#[test]
fn raw_os_op_in_bare_rejected() {
    let src = module(
        "m.a",
        "bare",
        "base",
        &ufn("f(n: Int) -> Int", "none", OS_BODY),
    );
    assert_eq!(kind(&src, &opts(&["m.a"])), ErrorKind::OsItemInCore);
    // 核の層の原始操作は bare でも使える。
    let src = module(
        "m.a",
        "bare",
        "base",
        &ufn("f(n: Int) -> Int", "state", BODY_OK),
    );
    ok(&src, &opts(&["m.a"]));
}

#[test]
fn raw_op_without_premise_rejected() {
    // 前提の文が空（空白だけを含む）の原始操作は、登録の時点で誤りになる（ADR 0016 の決定 1）。
    for premise in ["", "  "] {
        let mut o = opts(&["m.a"]);
        o.raw_ops[0].premise = premise.to_string();
        let src = base(&ufn("f(n: Int) -> Int", "state", BODY_OK));
        let e = check_with(&src, &o).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Unsupported);
        assert!(e.message.contains("前提"), "{}", e.message);
    }
}

#[test]
fn duplicate_raw_op_rejected() {
    let mut o = opts(&["m.a"]);
    o.raw_ops
        .push(op(Layer::Core, "alloc(size: Int) -> Addr", "state"));
    let src = base(&ufn("f(n: Int) -> Int", "state", BODY_OK));
    assert_eq!(kind(&src, &o), ErrorKind::DuplicateDefinition);
}

// ---- コマンドライン（`--trust-base`） ----

fn sigil_bin(args: &[&str]) -> (i32, String, String) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(args)
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn temp_file(name: &str, src: &str) -> String {
    let dir = std::env::temp_dir().join(format!("sigil-trust-base-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p.to_string_lossy().into_owned()
}

#[test]
fn trust_base_allowed_with_option_cli() {
    let src = base(&ufn("f(n: Int) -> Int", "none", "  return n + 1;"));
    let p = temp_file("a.sigil", &src);
    let (code, _, err) = sigil_bin(&["check", &p]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("trust base"), "{err}");
    let (code, out, err) = sigil_bin(&["check", "--trust-base", "m.a", &p]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("ok"), "{out}");
    let (code, out, err) = sigil_bin(&["run", "--trust-base", "m.a", &p, "f", "41"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "42");
    let (code, _, _) = sigil_bin(&["run", &p, "f", "41"]);
    assert_eq!(code, 1);
    // 値のない `--trust-base` は使い方の誤り。
    let (code, _, _) = sigil_bin(&["check", "--trust-base"]);
    assert_eq!(code, 2);
}

#[test]
fn user_std_module_name_rejected_cli() {
    let src = module(
        "std.x",
        "server",
        "base",
        &ufn("f(n: Int) -> Int", "none", "  return n;"),
    );
    let p = temp_file("b.sigil", &src);
    let (code, _, err) = sigil_bin(&["check", "--trust-base", "std.x", &p]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("std"), "{err}");
}

#[test]
fn trust_base_allowed_with_option_build_cli() {
    let src = base(&ufn("f(n: Int) -> Int", "none", "  return n + 1;"));
    let p = temp_file("c.sigil", &src);
    let out = temp_file("c.out", "");
    let (code, _, err) = sigil_bin(&["build", &p, "f", "-o", &out]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("trust base"), "{err}");
    let (code, _, err) = sigil_bin(&["build", "--trust-base", "m.a", &p, "f", "-o", &out]);
    assert_eq!(code, 0, "{err}");
    let o = std::process::Command::new(&out).arg("41").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "42");
}

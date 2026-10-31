//! ADR 0022 の PR 2-4c: トレイトの検査（適合・コヒーレンス）と実行（静的解決・単相化）。
//! 期待値は決め打ち（エラーの種類と位置）。実行は `common::run` で評価器とネイティブを突き合わせる。

mod common;

use sigil::check::check;
use sigil::{fmt, ErrorKind};

const HEAD: &str = "module m.t\n  targets server\n  trust none\n\n";

const SPEC4: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// トレイト `Describe`（メソッド 2 つ）と型 `Box2` の組。
fn describe(extra_impl: &str, extra: &str) -> String {
    format!(
        "{HEAD}trait Describe {{\n  fn label(self: &Self) -> String\n    intent \"ラベル\"\n    requires none\n    ensures none\n    effects none;\n\n  fn size(self: &Self) -> Int\n    intent \"大きさ\"\n    requires none\n    ensures result >= 0\n    effects none;\n}}\n\nstruct Box2 {{\n  w: Int,\n}}\n\nimpl Describe for Box2 {{\n  fn label(self: &Box2) -> String\n    intent \"ラベル\"\n    requires none\n    ensures none\n    effects none\n  {{\n    return \"box\";\n  }}\n\n  fn size(self: &Box2) -> Int\n    intent \"大きさ\"\n    requires none\n    ensures result >= 0\n    effects none\n  {{\n    return self.w;\n  }}\n{extra_impl}}}\n{extra}"
    )
}

fn err(src: &str) -> (ErrorKind, u32) {
    match check(src) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => (e.kind, e.span.line),
    }
}

fn run_int(src: &str, func: &str) -> String {
    common::run(src, func, &[]).unwrap().value
}

// ---- 正常系: 呼び出しと実行 ----

#[test]
fn trait_method_call_runs() {
    let src = describe(
        "",
        "fn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{\n  let b = Box2 { w: 40 };\n  return b.size() + 2;\n}\n\nfn lab() -> String\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{\n  let b = Box2 { w: 1 };\n  return b.label();\n}\n",
    );
    check(&src).unwrap();
    assert_eq!(run_int(&src, "main"), "42");
    assert_eq!(run_int(&src, "lab"), "\"box\"");
}

#[test]
fn generic_bound_method_call_runs() {
    // 型引数の制約に書いたトレイトは、その関数の中で見える（ADR 0022 決定 2）。
    let src = describe(
        "",
        "fn total<T: Describe>(x: T) -> Int\n  intent \"合計\"\n  requires none\n  ensures none\n  effects none\n{\n  return x.size();\n}\n\nfn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{\n  return total(Box2 { w: 7 });\n}\n",
    );
    check(&src).unwrap();
    assert_eq!(run_int(&src, "main"), "7");
}

#[test]
fn generic_bound_without_impl_is_unsatisfied() {
    // 制約を満たさない型を渡すと `ConstraintUnsatisfied`。
    let src = describe(
        "",
        "fn total<T: Describe>(x: T) -> Int\n  intent \"合計\"\n  requires none\n  ensures none\n  effects none\n{\n  return x.size();\n}\n\nfn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{\n  return total(3);\n}\n",
    );
    assert_eq!(err(&src).0, ErrorKind::ConstraintUnsatisfied);
}

#[test]
fn typefn_and_param_typefn_run() {
    // `型.f()`（受け手のない関数）と、総称の中の `T.f()`（ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}trait Zero {{\n  fn zero() -> Self\n    intent \"ゼロ\"\n    requires none\n    ensures none\n    effects none;\n}}\n\nimpl Zero for Int {{\n  fn zero() -> Int\n    intent \"ゼロ\"\n    requires none\n    ensures none\n    effects none\n  {{\n    return 0;\n  }}\n}}\n\nfn make<T: Zero>() -> T\n  intent \"作る\"\n  requires none\n  ensures none\n  effects none\n{{\n  return T.zero();\n}}\n\nfn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{{\n  let a = make();\n  return a + Int.zero() + 5;\n}}\n"
    );
    check(&src).unwrap();
    assert_eq!(run_int(&src, "main"), "5");
}

#[test]
fn qualified_call_disambiguates() {
    // 見えている 2 つのトレイトが同じ名前の関数を与えたら、呼び出し箇所で曖昧（`UseConflict`）。
    // `トレイト.m(x)` の形で選べる（ADR 0022 決定 2）。
    let base = format!(
        "{HEAD}trait A {{\n  fn who(self: &Self) -> Int\n{SPEC4};\n}}\n\ntrait B {{\n  fn who(self: &Self) -> Int\n{SPEC4};\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl A for S {{\n  fn who(self: &S) -> Int\n{SPEC4}  {{\n    return 1;\n  }}\n}}\n\nimpl B for S {{\n  fn who(self: &S) -> Int\n{SPEC4}  {{\n    return 2;\n  }}\n}}\n"
    );
    let ambiguous = format!(
        "{base}\nfn main() -> Int\n{SPEC4}{{\n  let s = S {{ v: 0 }};\n  return s.who();\n}}\n"
    );
    assert_eq!(err(&ambiguous).0, ErrorKind::UseConflict);
    let chosen = format!(
        "{base}\nfn main() -> Int\n{SPEC4}{{\n  let s = S {{ v: 0 }};\n  return A.who(s) * 10 + B.who(s);\n}}\n"
    );
    check(&chosen).unwrap();
    assert_eq!(run_int(&chosen, "main"), "12");
}

#[test]
fn generic_impl_for_runs() {
    // `impl<T> Show for Tree<T>`（ADR 0022 決定 2 の一意の規則）。
    let src = format!(
        "{HEAD}struct Tree<T> {{\n  v: T,\n}}\n\ntrait Show {{\n  fn show(self: &Self) -> String\n{SPEC4};\n}}\n\nimpl<T> Show for Tree<T> {{\n  fn show(self: &Tree<T>) -> String\n{SPEC4}  {{\n    return \"tree\";\n  }}\n}}\n\nfn main() -> String\n{SPEC4}{{\n  let t = Tree {{ v: 3 }};\n  return t.show();\n}}\n"
    );
    check(&src).unwrap();
    assert_eq!(run_int(&src, "main"), "\"tree\"");
}

#[test]
fn constraint_accepts_trait_and_fmt_sorts() {
    // 制約に性質とトレイトを並べて書ける。並びは名前の文字順（fmt が直す。ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}trait Describe {{\n  fn size(self: &Self) -> Int\n{SPEC4};\n}}\n\nfn f<T: Describe + Copy>(x: T) -> Int\n{SPEC4}{{\n  return x.size();\n}}\n"
    );
    let out = fmt::format(&src).unwrap();
    assert!(out.contains("<T: Copy + Describe>"), "{out}");
}

#[test]
fn effects_of_generic_call_come_from_trait() {
    // 総称の `T: トレイト` 経由の呼び出しは、トレイトの効果宣言を使う（ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}trait Loud {{\n  fn shout(self: &Self) -> Int\n    intent \"叫ぶ\"\n    requires none\n    ensures none\n    effects console;\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl Loud for S {{\n  fn shout(self: &S) -> Int\n    intent \"叫ぶ\"\n    requires none\n    ensures none\n    effects console\n  {{\n    console.log(&\"x\");\n    return self.v;\n  }}\n}}\n\nfn go<T: Loud>(x: T) -> Int\n  intent \"行く\"\n  requires none\n  ensures none\n  effects none\n{{\n  return x.shout();\n}}\n"
    );
    // `go` は効果なしなのに、トレイトの効果 `console` が伝わるので合わない。
    assert_eq!(err(&src).0, ErrorKind::EffectMissing);
}

// ---- 適合検査（ADR 0022 決定 2） ----

#[test]
fn missing_fn_is_mismatch() {
    let src = format!(
        "{HEAD}trait Describe {{\n  fn label(self: &Self) -> String\n{SPEC4};\n\n  fn size(self: &Self) -> Int\n{SPEC4};\n}}\n\nstruct Box2 {{\n  w: Int,\n}}\n\nimpl Describe for Box2 {{\n  fn label(self: &Box2) -> String\n{SPEC4}  {{\n    return \"box\";\n  }}\n}}\n"
    );
    let (k, line) = err(&src);
    assert_eq!(k, ErrorKind::TraitImplMismatch);
    // 位置は impl の行。
    assert_eq!(
        line,
        src.lines().position(|l| l.starts_with("impl")).unwrap() as u32 + 1
    );
}

#[test]
fn extra_fn_is_mismatch() {
    let src = describe(
        "\n  fn extra(self: &Box2) -> Int\n    intent \"余分\"\n    requires none\n    ensures none\n    effects none\n  {\n    return 0;\n  }\n",
        "",
    );
    assert_eq!(err(&src).0, ErrorKind::TraitImplMismatch);
}

#[test]
fn requires_alpha_equivalence() {
    // 引数の名前の違いは位置で対応する（α同値）ので通る。式の並べ替えは落ちる。
    let mk = |t_req: &str, i_req: &str, i_name: &str| {
        format!(
            "{HEAD}trait Calc {{\n  fn get(self: &Self, x: Int) -> Int\n    intent \"取る\"\n    requires {t_req}\n    ensures none\n    effects none;\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl Calc for S {{\n  fn get(self: &S, {i_name}: Int) -> Int\n    intent \"取る\"\n    requires {i_req}\n    ensures none\n    effects none\n  {{\n    return {i_name};\n  }}\n}}\n"
        )
    };
    check(&mk("x > 0", "y > 0", "y")).unwrap();
    assert_eq!(
        err(&mk("x > 0", "y >= 0", "y")).0,
        ErrorKind::TraitImplMismatch
    );
    assert_eq!(
        err(&mk("x > 0 && x < 10", "y < 10 && y > 0", "y")).0,
        ErrorKind::TraitImplMismatch
    );
}

#[test]
fn ensures_strengthening_only() {
    // ensures はトレイトの条件を足すだけ許可（削除は不可。ADR 0022 決定 2）。
    let mk = |i_ens: &str| {
        format!(
            "{HEAD}trait Calc {{\n  fn get(self: &Self) -> Int\n    intent \"取る\"\n    requires none\n    ensures result >= 0\n    effects none;\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl Calc for S {{\n  fn get(self: &S) -> Int\n    intent \"取る\"\n    requires none\n    ensures {i_ens}\n    effects none\n  {{\n    return self.v;\n  }}\n}}\n"
        )
    };
    check(&mk("result >= 0 && result < 100")).unwrap();
    assert_eq!(err(&mk("none")).0, ErrorKind::TraitImplMismatch);
}

#[test]
fn effects_subset_rules() {
    // effects は ⊑（カテゴリは階層で覆う。`fs.read` は `fs` を覆わない。ADR 0022 決定 2・ADR 0004 追記）。
    // 本体は宣言と一致する効果を実際に起こす（宣言と本体の一致は通常の検査の規則）。
    let mk = |t_fx: &str, i_fx: &str, body: &str| {
        format!(
            "{HEAD}trait Rd {{\n  fn rd(self: &Self, p: Path) -> Int\n    intent \"読む\"\n    requires none\n    ensures none\n    effects {t_fx};\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl Rd for S {{\n  fn rd(self: &S, p: Path) -> Int\n    intent \"読む\"\n    requires none\n    ensures none\n    effects {i_fx}\n  {{\n{body}    return self.v;\n  }}\n}}\n"
        )
    };
    let read = "    let a = fs.read(&p);\n";
    let both = "    let a = fs.read(&p);\n    let b = fs.write(&p, &\"x\");\n";
    let log = "    console.log(&\"x\");\n";
    check(&mk("fs", "fs.read", read)).unwrap();
    assert_eq!(
        err(&mk("fs.read", "fs", both)).0,
        ErrorKind::TraitImplMismatch
    );
    assert_eq!(
        err(&mk("none", "console", log)).0,
        ErrorKind::TraitImplMismatch
    );
}

#[test]
fn effects_of_positional() {
    // `of(p)` は引数の位置で対応する（ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}trait Ap {{\n  fn ap(self: &Self, f: fn(Int) -> Int) -> Int\n    intent \"当てる\"\n    requires none\n    ensures none\n    effects of(f);\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl Ap for S {{\n  fn ap(self: &S, g: fn(Int) -> Int) -> Int\n    intent \"当てる\"\n    requires none\n    ensures none\n    effects of(g)\n  {{\n    return g(1);\n  }}\n}}\n"
    );
    check(&src).unwrap();
}

#[test]
fn inherent_conflict_is_mismatch() {
    // 固有メソッドとトレイトの関数が同じ名前なら、impl の宣言時に落ちる（ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}trait Describe {{\n  fn size(self: &Self) -> Int\n{SPEC4};\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl S {{\n  fn size(self: &S) -> Int\n{SPEC4}  {{\n    return self.v;\n  }}\n}}\n\nimpl Describe for S {{\n  fn size(self: &S) -> Int\n{SPEC4}  {{\n    return self.v;\n  }}\n}}\n"
    );
    assert_eq!(err(&src).0, ErrorKind::TraitImplMismatch);
}

// ---- コヒーレンス ----

#[test]
fn duplicate_impl_is_rejected() {
    let one = format!(
        "{HEAD}trait Describe {{\n  fn size(self: &Self) -> Int\n{SPEC4};\n}}\n\nstruct S {{\n  v: Int,\n}}\n\nimpl Describe for S {{\n  fn size(self: &S) -> Int\n{SPEC4}  {{\n    return self.v;\n  }}\n}}\n"
    );
    let src = format!("{one}\nimpl Describe for S {{\n  fn size(self: &S) -> Int\n{SPEC4}  {{\n    return self.v;\n  }}\n}}\n");
    let (k, line) = err(&src);
    assert_eq!(k, ErrorKind::TraitImplDuplicate);
    let last_impl = src
        .lines()
        .collect::<Vec<_>>()
        .iter()
        .rposition(|l| l.starts_with("impl"))
        .unwrap() as u32
        + 1;
    assert_eq!(line, last_impl);
}

#[test]
fn headless_impl_is_rejected() {
    // 頭のない実装（`for T`）は認めない（ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}trait Describe {{\n  fn size(self: &Self) -> Int\n{SPEC4};\n}}\n\nimpl<T> Describe for T {{\n  fn size(self: &T) -> Int\n{SPEC4}  {{\n    return 0;\n  }}\n}}\n"
    );
    assert_eq!(err(&src).0, ErrorKind::TraitImplMismatch);
}

// ---- 単相化の上限 ----

#[test]
fn polymorphic_recursion_is_rejected() {
    // 多相再帰は検査時に `PolymorphicRecursion` で拒否する（ADR 0022 決定 2）。
    let src = format!(
        "{HEAD}fn f<T>(x: List<T>) -> Int\n{SPEC4}{{\n  let mut ys = List.new();\n  ys.push(x);\n  return f(ys);\n}}\n\nfn main() -> Int\n{SPEC4}{{\n  let mut xs = List.new();\n  xs.push(1);\n  return f(xs);\n}}\n"
    );
    assert_eq!(err(&src).0, ErrorKind::PolymorphicRecursion);
}

// ---- モジュールをまたぐ規則（配置・見える範囲・重複） ----

use sigil::effects::Options;
use sigil::project::{check_project, ProjectError};
use std::path::PathBuf;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sigil-2-4c-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d.canonicalize().unwrap())
    }
    fn write(&self, rel: &str, src: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, src).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn proj_err(t: &Tmp, entry: &str) -> Result<(), (ErrorKind, String, u32)> {
    match check_project(&t.0, &t.0.join(entry), &Options::default()) {
        Ok(()) => Ok(()),
        Err(ProjectError::Diag { file, error }) => Err((
            error.kind,
            file.strip_prefix(&t.0).unwrap().display().to_string(),
            error.span.line,
        )),
        Err(ProjectError::Read { file, msg }) => panic!("読めない {}: {msg}", file.display()),
    }
}

fn module(name: &str, uses: &str, items: &str) -> String {
    format!("module {name}\n  targets server\n  trust none\n{uses}\n{items}")
}

const TYPES: &str = "pub struct S {\n  v: Int,\n}\n\npub fn mk(v: Int) -> S\n  intent \"作る\"\n  requires none\n  ensures none\n  effects none\n{\n  return S { v: v };\n}\n";
const TRAIT: &str = "pub trait Tr {\n  fn get(self: &Self) -> Int\n    intent \"取る\"\n    requires none\n    ensures none\n    effects none;\n}\n";
const IMPL: &str = "impl Tr for S {\n  fn get(self: &S) -> Int\n    intent \"取る\"\n    requires none\n    ensures none\n    effects none\n  {\n    return self.v;\n  }\n}\n";

#[test]
fn placement_rules_across_modules() {
    // トレイトのモジュールに置く: 可（型は他のモジュール）。
    let t = Tmp::new("place-trait");
    t.write("m/types.sigil", &module("m.types", "", TYPES));
    t.write(
        "m/traits.sigil",
        &module(
            "m.traits",
            "\nuse m.types.S;\n",
            &format!("{TRAIT}\n{IMPL}"),
        ),
    );
    let main = module(
        "m.main",
        "\nuse m.traits.Tr;\nuse m.types.mk;\n",
        "fn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{\n  return mk(1).get();\n}\n",
    );
    let e = t.write("m/main.sigil", &main);
    proj_err(&t, "m/main.sigil").unwrap();
    let _ = e;

    // 型のモジュールに置く: 可。
    let t = Tmp::new("place-type");
    t.write(
        "m/types.sigil",
        &module(
            "m.types",
            "\nuse m.traits.Tr;\n",
            &format!("{TYPES}\n{IMPL}"),
        ),
    );
    t.write("m/traits.sigil", &module("m.traits", "", TRAIT));
    t.write("m/main.sigil", &main);
    proj_err(&t, "m/main.sigil").unwrap();

    // どちらでもないモジュールに置く: `TraitImplPlacement`。
    let t = Tmp::new("place-other");
    t.write("m/types.sigil", &module("m.types", "", TYPES));
    t.write("m/traits.sigil", &module("m.traits", "", TRAIT));
    t.write(
        "m/other.sigil",
        &module("m.other", "\nuse m.traits.Tr;\nuse m.types.S;\n", IMPL),
    );
    t.write("m/main.sigil", &module("m.main", "\nuse m.other.x;\n", ""));
    // 入口は other を辿るモジュールにする（other 自体を入口にして検査する）。
    let (k, f, _) = proj_err(&t, "m/other.sigil").unwrap_err();
    assert_eq!(k, ErrorKind::TraitImplPlacement);
    assert_eq!(f, "m/other.sigil");
}

#[test]
fn placement_std_type_only_in_trait_module() {
    // 標準の型への impl は、トレイトのモジュールでのみ置ける（ADR 0022 決定 2）。
    let t = Tmp::new("place-std");
    t.write(
        "m/traits.sigil",
        &module(
            "m.traits",
            "",
            &format!(
                "{TRAIT}\nimpl Tr for Int {{\n  fn get(self: &Int) -> Int\n    intent \"取る\"\n    requires none\n    ensures none\n    effects none\n  {{\n    return 0;\n  }}\n}}\n"
            ),
        ),
    );
    proj_err(&t, "m/traits.sigil").unwrap();

    let t = Tmp::new("place-std-ng");
    t.write("m/traits.sigil", &module("m.traits", "", TRAIT));
    t.write(
        "m/other.sigil",
        &module(
            "m.other",
            "\nuse m.traits.Tr;\n",
            "impl Tr for Int {\n  fn get(self: &Int) -> Int\n    intent \"取る\"\n    requires none\n    ensures none\n    effects none\n  {\n    return 0;\n  }\n}\n",
        ),
    );
    let (k, _, _) = proj_err(&t, "m/other.sigil").unwrap_err();
    assert_eq!(k, ErrorKind::TraitImplPlacement);
}

#[test]
fn duplicate_impl_across_modules() {
    // 同じ（トレイト, 型）の組の impl は全体で 1 つ（ADR 0022 決定 2）。
    let t = Tmp::new("dup-x");
    t.write(
        "m/defs.sigil",
        &module("m.defs", "", &format!("{TYPES}\n{TRAIT}\n{IMPL}")),
    );
    t.write(
        "m/other.sigil",
        &module("m.other", "\nuse m.defs.S;\nuse m.defs.Tr;\n", IMPL),
    );
    let (k, f, _) = proj_err(&t, "m/other.sigil").unwrap_err();
    assert_eq!(k, ErrorKind::TraitImplDuplicate);
    assert_eq!(f, "m/other.sigil");
}

const DEFS: &str = "pub struct S {\n  v: Int,\n}\n\npub fn mk(v: Int) -> S\n  intent \"作る\"\n  requires none\n  ensures none\n  effects none\n{\n  return S { v: v };\n}\n\npub trait Tr {\n  fn get(self: &Self) -> Int\n    intent \"取る\"\n    requires none\n    ensures none\n    effects none;\n}\n\nimpl Tr for S {\n  fn get(self: &S) -> Int\n    intent \"取る\"\n    requires none\n    ensures none\n    effects none\n  {\n    return self.v;\n  }\n}\n";

#[test]
fn hidden_trait_method_is_not_callable() {
    // トレイトが見えていないモジュールでは、そのトレイトのメソッドを呼べない（ADR 0022 決定 2）。
    let main = |uses: &str| {
        format!(
            "module m.main\n  targets server\n  trust none\n{uses}\nfn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{{\n  let s = mk(3);\n  return s.get();\n}}\n"
        )
    };
    let hidden = main("\nuse m.defs.mk;\n");
    let defs = format!("module m.defs\n  targets server\n  trust none\n\n{DEFS}");
    let f = sigil::ir_eval::run_sources(&[&hidden, &defs], "main", &[]).unwrap_err();
    let e = match &f.error {
        sigil::exec::RunError::Module { error, .. } => error,
        sigil::exec::RunError::Check(e) => e,
        other => panic!("型エラーの診断のはず: {other}"),
    };
    assert_eq!(e.kind, ErrorKind::NoSuchMember);
    assert!(e.message.contains("見えていない"), "{}", e.message);
    // トレイトを取り込めば呼べて、実行もできる（評価器とネイティブの突き合わせつき）。
    let shown = main("\nuse m.defs.Tr;\nuse m.defs.mk;\n");
    assert_eq!(
        common::run_sources(&[&shown, &defs], "main", &[])
            .unwrap()
            .value,
        "3"
    );
}

#[test]
fn lsp_and_check_find_the_same_impl() {
    // ADR 0022 決定 2 の決め打ち: LSP と `sigil check` で実装を探した結果が同じ。
    // `S.zero()` の呼び出しについて、check は通り、hover は実装（impl の中の関数）の仕様を示す。
    let t = Tmp::new("lsp-same");
    let defs = "pub struct S {\n  v: Int,\n}\n\npub trait Zero {\n  fn zero() -> Self\n    intent \"ゼロの宣言\"\n    requires none\n    ensures none\n    effects none;\n}\n\nimpl Zero for S {\n  fn zero() -> S\n    intent \"ゼロの実装\"\n    requires none\n    ensures none\n    effects none\n  {\n    return S { v: 0 };\n  }\n}\n";
    t.write("m/defs.sigil", &module("m.defs", "", defs));
    let main_src = module(
        "m.main",
        "\nuse m.defs.S;\nuse m.defs.Zero;\n",
        "fn main() -> Int\n  intent \"入口\"\n  requires none\n  ensures none\n  effects none\n{\n  let s = S.zero();\n  return s.v;\n}\n",
    );
    let pm = t.write("m/main.sigil", &main_src);
    proj_err(&t, "m/main.sigil").unwrap();

    // 最小の LSP クライアント（枠は `sigil::lsp::read_frame` を使う）。
    use sigil::json::{obj, Value};
    use sigil::lsp::{path_to_uri, read_frame, Frame, MAX_BODY};
    use std::io::{BufReader, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut w = child.stdin.take().unwrap();
    let mut rd = BufReader::new(child.stdout.take().unwrap());
    let send = |w: &mut std::process::ChildStdin, v: Value| {
        let body = v.to_json();
        w.write_all(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes())
            .unwrap();
        w.flush().unwrap();
    };
    send(
        &mut w,
        obj(vec![
            ("jsonrpc", "2.0".into()),
            ("id", 1i64.into()),
            ("method", "initialize".into()),
            (
                "params",
                obj(vec![
                    ("capabilities", obj(vec![])),
                    ("rootUri", path_to_uri(&t.0).into()),
                ]),
            ),
        ]),
    );
    // initialize の応答を読む（通知が先に来たら読み飛ばす）。
    let mut hover_result = None;
    let mut sent_open = false;
    let mut sent_hover = false;
    let line0 = main_src
        .lines()
        .position(|l| l.contains("S.zero()"))
        .unwrap() as i64;
    let ch0 = main_src
        .lines()
        .find(|l| l.contains("S.zero()"))
        .unwrap()
        .find("zero")
        .unwrap() as i64;
    for _ in 0..40 {
        match read_frame(&mut rd, MAX_BODY) {
            Frame::Body(b) => {
                let v = sigil::json::parse(&String::from_utf8_lossy(&b)).unwrap();
                if v.get("id") == Some(&Value::Int(1)) && !sent_open {
                    send(
                        &mut w,
                        obj(vec![
                            ("jsonrpc", "2.0".into()),
                            ("method", "initialized".into()),
                            ("params", obj(vec![])),
                        ]),
                    );
                    send(
                        &mut w,
                        obj(vec![
                            ("jsonrpc", "2.0".into()),
                            ("method", "textDocument/didOpen".into()),
                            (
                                "params",
                                obj(vec![(
                                    "textDocument",
                                    obj(vec![
                                        ("uri", path_to_uri(&pm).into()),
                                        ("languageId", "sigil".into()),
                                        ("version", 1i64.into()),
                                        ("text", main_src.clone().into()),
                                    ]),
                                )]),
                            ),
                        ]),
                    );
                    sent_open = true;
                } else if v.get("method").and_then(|m| m.as_str())
                    == Some("textDocument/publishDiagnostics")
                    && sent_open
                    && !sent_hover
                {
                    send(
                        &mut w,
                        obj(vec![
                            ("jsonrpc", "2.0".into()),
                            ("id", 7i64.into()),
                            ("method", "textDocument/hover".into()),
                            (
                                "params",
                                obj(vec![
                                    ("textDocument", obj(vec![("uri", path_to_uri(&pm).into())])),
                                    (
                                        "position",
                                        obj(vec![
                                            ("line", line0.into()),
                                            ("character", ch0.into()),
                                        ]),
                                    ),
                                ]),
                            ),
                        ]),
                    );
                    sent_hover = true;
                } else if v.get("id") == Some(&Value::Int(7)) {
                    hover_result = Some(v);
                    break;
                }
            }
            _ => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let v = hover_result.expect("hover の応答が来ない");
    let text = v
        .get("result")
        .and_then(|r| r.get("contents"))
        .and_then(|c| c.get("value"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    // hover はトレイトの宣言ではなく、実装の関数の仕様（intent「ゼロの実装」）を示す。
    assert!(text.contains("ゼロの実装"), "{text}");
    assert!(!text.contains("ゼロの宣言"), "{text}");
}

#[test]
fn instance_limit_depth() {
    // 型引数の入れ子の深さ 32 が上限（ADR 0022 決定 2）。33 で `InstanceLimit`。
    let mk = |d: usize| {
        let mut lets = String::from("  let a0 = 1;\n");
        for i in 1..=d {
            lets.push_str(&format!("  let a{i} = Wrap {{ v: a{} }};\n", i - 1));
        }
        format!(
            "{HEAD}struct Wrap<T> {{\n  v: T,\n}}\n\nfn id<T>(x: T) -> T\n{SPEC4}{{\n  return x;\n}}\n\nfn main() -> Int\n{SPEC4}{{\n{lets}  let v = id(a{d});\n  return 0;\n}}\n"
        )
    };
    // 深さ 32 は通る。
    sigil::ir_eval::run(&mk(32), "main", &[]).unwrap();
    // 深さ 33 は上限で止まる。
    let e = sigil::ir_eval::run(&mk(33), "main", &[]).unwrap_err();
    assert!(format!("{e}").contains("InstanceLimit"), "{e}");
}

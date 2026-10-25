//! 効果の検査（ADR 0004・ADR 0006 の条件 4〜6・11・12）と、`match` の到達できない腕（ADR 0011）のテスト。
//!
//! 標準ライブラリが入るまで、効果のある関数は仮の関数（`Primitive`）として差し込む。

use sigil::check::check_with;
use sigil::effects::{EffectTable, Options, Primitive, Targets};
use sigil::ErrorKind;

fn prim(sig: &str, effects: &[&str]) -> Primitive {
    Primitive {
        signature: sig.to_string(),
        effects: effects.iter().map(|e| e.to_string()).collect(),
    }
}

fn std_options() -> Options {
    Options {
        copy_override: vec![],
        table: EffectTable::standard(),
        primitives: vec![
            prim("emit(n: Int) -> Int", &["console"]),
            prim("read_file(p: String) -> String", &["fs.read"]),
            prim("write_file(p: String) -> Int", &["fs.write"]),
            prim("tick() -> Int", &["state"]),
            prim("touch_dom(n: Int) -> Int", &["dom"]),
        ],
    }
}

fn head(target: &str) -> String {
    format!("module m.a\n  targets {target}\n  trust none\n\n")
}

/// 効果の節を指定した関数。
fn fx(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n"
    )
}

const APPLY_ALL: &str = "apply_all(f: fn(Int) -> Int, v: Int) -> Int";

fn apply_all() -> String {
    fx(APPLY_ALL, "of(f)", "  return f(v);")
}

fn run(opts: &Options, target: &str, items: &str) -> Result<(), ErrorKind> {
    let src = format!("{}{items}", head(target));
    check_with(&src, opts).map_err(|e| e.kind)
}

fn kind(items: &str) -> ErrorKind {
    match run(&std_options(), "server", items) {
        Ok(()) => panic!("エラーになるはずが成功した:\n{items}"),
        Err(k) => k,
    }
}

fn ok(items: &str) {
    if let Err(k) = run(&std_options(), "server", items) {
        panic!("{k:?}\n{items}");
    }
}

// ---- 宣言と推論の一致 ----

#[test]
fn declared_effects_match_inferred() {
    ok(&fx("f(n: Int) -> Int", "console", "  return emit(n);"));
    ok(&fx("f(n: Int) -> Int", "none", "  return n + 1;"));
    ok(&fx(
        "f(p: String) -> Int",
        "console, fs.read",
        "  let t = read_file(p);\n  return emit(1);",
    ));
    // 宣言の順は問わない
    ok(&fx(
        "f(p: String) -> Int",
        "fs.read, console",
        "  let t = read_file(p);\n  return emit(1);",
    ));
}

#[test]
fn missing_effect_is_an_error() {
    // 04-effects の bad_missing に相当
    assert_eq!(
        kind(&fx(
            "f(p: String) -> String",
            "none",
            "  return read_file(p);"
        )),
        ErrorKind::EffectMissing
    );
    assert_eq!(
        kind(&fx(
            "f(p: String) -> Int",
            "console",
            "  let t = read_file(p);\n  return emit(1);"
        )),
        ErrorKind::EffectMissing
    );
}

#[test]
fn extra_effect_is_an_error() {
    // 04-effects の bad_extra に相当
    assert_eq!(
        kind(&fx("f(x: Int) -> Int", "console", "  return x * 2;")),
        ErrorKind::EffectExtra
    );
    assert_eq!(
        kind(&fx(
            "f(n: Int) -> Int",
            "console, fs.read",
            "  return emit(n);"
        )),
        ErrorKind::EffectExtra
    );
}

#[test]
fn effects_propagate_to_callers() {
    let callee = fx("log(n: Int) -> Int", "console", "  return emit(n);");
    assert_eq!(
        kind(&format!(
            "{callee}{}",
            fx("f(n: Int) -> Int", "none", "  return log(n);")
        )),
        ErrorKind::EffectMissing
    );
    ok(&format!(
        "{callee}{}",
        fx("f(n: Int) -> Int", "console", "  return log(n);")
    ));
}

#[test]
fn effects_inside_branches_and_loops_count() {
    assert_eq!(
        kind(&fx(
            "f(n: Int) -> Int",
            "none",
            "  if n > 0 {\n    let k = emit(n);\n  }\n  return n;"
        )),
        ErrorKind::EffectMissing
    );
    assert_eq!(
        kind(&fx(
            "f(n: Int) -> Int",
            "none",
            "  return match n {\n    0 => emit(1),\n    _ => 2,\n  };"
        )),
        ErrorKind::EffectMissing
    );
}

#[test]
fn methods_carry_effects() {
    let imp = "struct S {\n  v: Int,\n}\nimpl S {\n  fn show(self: &S) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects console\n  {\n    return emit(self.v);\n  }\n}\n";
    ok(&format!(
        "{imp}{}",
        fx("f(s: &S) -> Int", "console", "  return s.show();")
    ));
    assert_eq!(
        kind(&format!(
            "{imp}{}",
            fx("f(s: &S) -> Int", "none", "  return s.show();")
        )),
        ErrorKind::EffectMissing
    );
}

// ---- 階層 ----

#[test]
fn parent_category_includes_children() {
    let both = "  let a = read_file(p);\n  return write_file(a);";
    ok(&fx("f(p: String) -> Int", "fs", both));
    ok(&fx("f(p: String) -> Int", "fs.read, fs.write", both));
    // 親を書くと下位すべてを含むので、片方だけ起こす本体には余分
    assert_eq!(
        kind(&fx(
            "f(p: String) -> String",
            "fs",
            "  return read_file(p);"
        )),
        ErrorKind::EffectExtra
    );
    assert_eq!(
        kind(&fx("f(p: String) -> Int", "fs.read", both)),
        ErrorKind::EffectMissing
    );
}

// ---- 名前の検査（ADR 0010 決定 12） ----

#[test]
fn unknown_effect_names() {
    for name in ["foo", "fs.exec", "browser", "console.log", "task.stop"] {
        assert_eq!(
            kind(&fx("f()", name, "  return;")),
            ErrorKind::UnknownEffect,
            "{name}"
        );
    }
}

#[test]
fn folded_parent_names_are_valid() {
    // `fs`・`net`・`task` は下位を束ねる名前として書ける（本体が起こさないので余分になる）
    for name in ["fs", "net", "task"] {
        assert_eq!(
            kind(&fx("f()", name, "  return;")),
            ErrorKind::EffectExtra,
            "{name}"
        );
    }
}

#[test]
fn of_must_name_a_function_parameter() {
    assert_eq!(
        kind(&fx("f(n: Int) -> Int", "of(z)", "  return n;")),
        ErrorKind::OfNotParam
    );
    assert_eq!(
        kind(&fx("f(n: Int) -> Int", "of(n)", "  return n;")),
        ErrorKind::OfNotParam
    );
}

// ---- of(f) ----

#[test]
fn of_f_declared_and_inferred() {
    ok(&apply_all());
    // f を呼ばない関数に of(f) は余分
    assert_eq!(
        kind(&fx(APPLY_ALL, "of(f)", "  return v;")),
        ErrorKind::EffectExtra
    );
    // f を呼ぶのに of(f) がない
    assert_eq!(
        kind(&fx(APPLY_ALL, "none", "  return f(v);")),
        ErrorKind::EffectMissing
    );
}

#[test]
fn of_f_is_replaced_by_the_actual_effect_at_the_call() {
    // 04-effects の with_tax に相当: 効果なしの関数を渡せば none
    ok(&format!(
        "{}{}",
        apply_all(),
        fx(
            "with_tax(v: Int) -> Int",
            "none",
            "  return apply_all(|p| p + 10, v);"
        )
    ));
    // 効果のある関数を渡せば、呼び出し側にその効果が現れる
    let a = apply_all();
    assert_eq!(
        kind(&format!(
            "{a}{}",
            fx(
                "g(v: Int) -> Int",
                "none",
                "  return apply_all(|p| emit(p), v);"
            )
        )),
        ErrorKind::EffectMissing
    );
    ok(&format!(
        "{a}{}",
        fx(
            "g(v: Int) -> Int",
            "console",
            "  return apply_all(|p| emit(p), v);"
        )
    ));
    // 関数の名前を渡す場合も同じ
    ok(&format!(
        "{a}{}",
        fx(
            "g(v: Int) -> Int",
            "console",
            "  return apply_all(emit, v);"
        )
    ));
    assert_eq!(
        kind(&format!(
            "{a}{}",
            fx("g(v: Int) -> Int", "none", "  return apply_all(emit, v);")
        )),
        ErrorKind::EffectMissing
    );
    // 余分な宣言も同じく誤り
    assert_eq!(
        kind(&format!(
            "{a}{}",
            fx(
                "g(v: Int) -> Int",
                "console",
                "  return apply_all(|p| p + 1, v);"
            )
        )),
        ErrorKind::EffectExtra
    );
}

#[test]
fn of_f_passes_through_a_parameter() {
    let a = apply_all();
    let wrap = "wrap(g: fn(Int) -> Int, v: Int) -> Int";
    ok(&format!(
        "{a}{}",
        fx(wrap, "of(g)", "  return apply_all(g, v);")
    ));
    assert_eq!(
        kind(&format!(
            "{a}{}",
            fx(wrap, "none", "  return apply_all(g, v);")
        )),
        ErrorKind::EffectMissing
    );
    // クロージャの中で g を呼んでも、g の効果として数える
    ok(&format!(
        "{a}{}",
        fx(wrap, "of(g)", "  return apply_all(|p| g(p) + 1, v);")
    ));
}

#[test]
fn let_bound_closures_carry_their_effects() {
    ok(&fx(
        "f(n: Int) -> Int",
        "console",
        "  let h = move |x| emit(x);\n  return h(n);",
    ));
    assert_eq!(
        kind(&fx(
            "f(n: Int) -> Int",
            "none",
            "  let h = move |x| emit(x);\n  return h(n);"
        )),
        ErrorKind::EffectMissing
    );
    let a = apply_all();
    assert_eq!(
        kind(&format!(
            "{a}{}",
            fx(
                "g(n: Int) -> Int",
                "none",
                "  let h = move |x| emit(x);\n  return apply_all(h, n);"
            )
        )),
        ErrorKind::EffectMissing
    );
    // 呼ばれない・渡されないクロージャは効果を起こさない
    ok(&fx(
        "f(n: Int) -> Int",
        "none",
        "  let h = move |x| emit(x);\n  return n;",
    ));
}

// ---- 実行対象（ADR 0004・ADR 0006 の条件 11） ----

#[test]
fn effects_are_checked_against_targets() {
    let o = std_options();
    // 04-effects の bad_target: ブラウザ向けモジュールで fs.read
    let read = fx(
        "f(p: String) -> String",
        "fs.read",
        "  return read_file(p);",
    );
    assert_eq!(run(&o, "browser", &read), Err(ErrorKind::EffectTarget));
    assert_eq!(run(&o, "server", &read), Ok(()));
    // 両方を対象にするなら両方で使えるカテゴリだけ
    let src = format!("module m.a\n  targets browser, server\n  trust none\n\n{read}");
    assert_eq!(
        check_with(&src, &o).unwrap_err().kind,
        ErrorKind::EffectTarget
    );
    let dom = fx("f(n: Int) -> Int", "dom", "  return touch_dom(n);");
    assert_eq!(run(&o, "server", &dom), Err(ErrorKind::EffectTarget));
    assert_eq!(run(&o, "browser", &dom), Ok(()));
    let both = fx("f(n: Int) -> Int", "console", "  return emit(n);");
    assert_eq!(run(&o, "browser", &both), Ok(()));
}

#[test]
fn declared_only_effects_are_also_checked_against_targets() {
    // 本体が起こさなくても、宣言したカテゴリが対象で使えなければ対象のエラー
    let o = std_options();
    let f = fx("f()", "fs.read", "  return;");
    assert_eq!(run(&o, "browser", &f), Err(ErrorKind::EffectTarget));
}

// ---- 表の拡張（ADR 0006 の条件 4・5・6・11） ----

fn gpu_options(targets: Targets) -> Options {
    Options {
        copy_override: vec![],
        table: EffectTable::standard().with("gpu.draw", targets),
        primitives: vec![prim("draw(n: Int) -> Int", &["gpu.draw"])],
    }
}

#[test]
fn adding_a_table_row_is_enough() {
    // 条件 4: 表に 1 行足すだけで、宣言・推論・照合が働く
    let o = gpu_options(Targets::ALL);
    let good = fx("f(n: Int) -> Int", "gpu.draw", "  return draw(n);");
    assert_eq!(run(&o, "server", &good), Ok(()));
    let missing = fx("f(n: Int) -> Int", "none", "  return draw(n);");
    assert_eq!(run(&o, "server", &missing), Err(ErrorKind::EffectMissing));
    let extra = fx("f(n: Int) -> Int", "gpu.draw", "  return n;");
    assert_eq!(run(&o, "server", &extra), Err(ErrorKind::EffectExtra));
    // 表にない間は、名前が誤り
    assert_eq!(
        run(&std_options(), "server", &good),
        Err(ErrorKind::UnknownEffect)
    );
}

#[test]
fn of_f_works_for_an_added_category() {
    // 条件 5
    let o = gpu_options(Targets::ALL);
    let a = apply_all();
    let call = |eff: &str| {
        format!(
            "{a}{}",
            fx(
                "g(v: Int) -> Int",
                eff,
                "  return apply_all(|p| draw(p), v);"
            )
        )
    };
    assert_eq!(run(&o, "server", &call("gpu.draw")), Ok(()));
    assert_eq!(
        run(&o, "server", &call("none")),
        Err(ErrorKind::EffectMissing)
    );
}

#[test]
fn state_subcategory_is_included_in_state() {
    // 条件 6: state の下位を足すと、`effects state` はそれを含む
    let o = Options {
        copy_override: vec![],
        table: EffectTable::standard().with("state.cell", Targets::ALL),
        primitives: vec![
            prim("tick() -> Int", &["state"]),
            prim("bump() -> Int", &["state.cell"]),
        ],
    };
    let both = "  let a = tick();\n  return bump();";
    assert_eq!(run(&o, "server", &fx("f() -> Int", "state", both)), Ok(()));
    assert_eq!(
        run(&o, "server", &fx("f() -> Int", "state, state.cell", both)),
        Ok(())
    );
    assert_eq!(
        run(
            &o,
            "server",
            &fx("f() -> Int", "state.cell", "  return bump();")
        ),
        Ok(())
    );
    // state.cell だけの本体に `state` は余分（親は下位に加えて自分も含む）
    assert_eq!(
        run(&o, "server", &fx("f() -> Int", "state", "  return bump();")),
        Err(ErrorKind::EffectExtra)
    );
}

#[test]
fn added_category_can_be_server_only() {
    // 条件 11: 許す実行対象を server にすると、ブラウザ向けのモジュールでの使用が誤り
    let o = gpu_options(Targets::SERVER);
    let f = fx("f(n: Int) -> Int", "gpu.draw", "  return draw(n);");
    assert_eq!(run(&o, "server", &f), Ok(()));
    assert_eq!(run(&o, "browser", &f), Err(ErrorKind::EffectTarget));
}

#[test]
fn primitive_names_must_be_unique_and_valid() {
    let dup = Options {
        copy_override: vec![],
        table: EffectTable::standard(),
        primitives: vec![prim("emit(n: Int) -> Int", &["console"]); 2],
    };
    assert_eq!(
        run(&dup, "server", &fx("f()", "none", "  return;")),
        Err(ErrorKind::DuplicateDefinition)
    );
    let bad = Options {
        copy_override: vec![],
        table: EffectTable::standard(),
        primitives: vec![prim("emit(n: Int) -> Int", &["nope"])],
    };
    assert_eq!(
        run(&bad, "server", &fx("f()", "none", "  return;")),
        Err(ErrorKind::UnknownEffect)
    );
}

// ---- 大域の値（ADR 0006 の条件 12） ----

#[test]
fn global_values_are_errors() {
    for src in [
        "let counter = 0;\n",
        "let mut counter = 0;\n",
        "fn f()\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return;\n}\nlet x = 1;\n",
    ] {
        assert_eq!(
            run(&std_options(), "server", src),
            Err(ErrorKind::GlobalValue),
            "{src}"
        );
    }
}

// ---- 到達できない腕（ADR 0011 の決定 9 の追記） ----

const SHAPE: &str = "enum Shape {\n  Circle(Int),\n  Rect(Int, Int),\n  Empty,\n}\n";

fn arms_kind(sig: &str, body: &str) -> ErrorKind {
    kind(&format!("{SHAPE}{}", fx(sig, "none", body)))
}

fn arms_ok(sig: &str, body: &str) {
    ok(&format!("{SHAPE}{}", fx(sig, "none", body)));
}

#[test]
fn arm_after_wildcard_is_unreachable() {
    assert_eq!(
        arms_kind(
            "f(n: Int) -> Int",
            "  return match n {\n    _ => 1,\n    0 => 2,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
    // 束縛の名前も全部に当たる
    assert_eq!(
        arms_kind(
            "f(n: Int) -> Int",
            "  return match n {\n    k => k,\n    0 => 2,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
}

#[test]
fn duplicate_arms_are_unreachable() {
    assert_eq!(
        arms_kind(
            "f(n: Int) -> Int",
            "  return match n {\n    0 => 1,\n    0 => 2,\n    _ => 3,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
    assert_eq!(
        arms_kind(
            "f(s: Shape) -> Int",
            "  return match s {\n    Shape.Empty => 0,\n    Shape.Empty => 1,\n    Shape.Circle(r) => r,\n    Shape.Rect(w, h) => w,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
}

#[test]
fn arms_covered_by_earlier_arms_are_unreachable() {
    // 列挙型の全部の値の後の `_`
    assert_eq!(
        arms_kind(
            "f(s: Shape) -> Int",
            "  return match s {\n    Shape.Circle(r) => r,\n    Shape.Rect(w, h) => w,\n    Shape.Empty => 0,\n    _ => 9,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
    // Bool の両方の後の `_`
    assert_eq!(
        arms_kind(
            "f(b: Bool) -> Int",
            "  return match b {\n    true => 1,\n    false => 2,\n    _ => 3,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
    // 組: (_, _) の後の腕
    assert_eq!(
        arms_kind(
            "f(t: (Bool, Bool)) -> Int",
            "  return match t {\n    (_, _) => 1,\n    (true, false) => 2,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
    // 組: 前の 2 つが (true, _) を覆う
    assert_eq!(
        arms_kind(
            "f(t: (Bool, Bool)) -> Int",
            "  return match t {\n    (true, true) => 1,\n    (true, false) => 2,\n    (true, _) => 3,\n    (false, _) => 4,\n  };"
        ),
        ErrorKind::UnreachableArm
    );
}

#[test]
fn unreachable_arm_in_match_statement() {
    assert_eq!(
        arms_kind(
            "f(o: Option<Int>) -> Int",
            "  match o {\n    Some(x) => {\n      return x;\n    }\n    Some(y) => {\n      return y;\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;"
        ),
        ErrorKind::UnreachableArm
    );
}

#[test]
fn reachable_arms_still_pass() {
    arms_ok(
        "f(s: Shape) -> Int",
        "  return match s {\n    Shape.Circle(0) => 0,\n    Shape.Circle(r) => r,\n    Shape.Rect(w, h) => w * h,\n    _ => 9,\n  };",
    );
    arms_ok(
        "f(t: (Bool, Bool)) -> Int",
        "  return match t {\n    (true, _) => 1,\n    (false, true) => 2,\n    (false, false) => 3,\n  };",
    );
    arms_ok(
        "f(n: Int) -> Int",
        "  return match n {\n    0 => 1,\n    1 => 2,\n    _ => 3,\n  };",
    );
    arms_ok(
        "f(o: Option<Int>) -> Int",
        "  return match o {\n    Some(0) => 1,\n    Some(x) => x,\n    None => 0,\n  };",
    );
}

// ---------- 関数の値の別名と入れ物（ADR 0011 決定 10） ----------

#[test]
fn alias_of_closure_keeps_effects() {
    // let d = c; d(1)
    let f = fx(
        "f() -> Int",
        "none",
        "  let c = move |x| emit(x);\n  let d = c;\n  return d(1);",
    );
    assert_eq!(kind(&f), ErrorKind::EffectMissing);
}

#[test]
fn alias_of_function_item_keeps_effects() {
    // let c = emit; c(1)
    let f = fx("f() -> Int", "none", "  let c = emit;\n  return c(1);");
    assert_eq!(kind(&f), ErrorKind::EffectMissing);
    let g = fx("f() -> Int", "console", "  let c = emit;\n  return c(1);");
    ok(&g);
}

#[test]
fn alias_of_parameter_inherits_of() {
    let bad = fx(
        "k(f: fn(Int) -> Int) -> Int",
        "none",
        "  let g = f;\n  return g(1);",
    );
    assert_eq!(kind(&bad), ErrorKind::EffectMissing);
    let good = fx(
        "k(f: fn(Int) -> Int) -> Int",
        "of(f)",
        "  let g = f;\n  return g(1);",
    );
    ok(&good);
}

#[test]
fn function_in_tuple_is_rejected() {
    let f = fx(
        "f() -> Int",
        "none",
        "  let c = move |x| emit(x);\n  let t = (c, 1);\n  let (h, n) = t;\n  return h(n);",
    );
    assert_eq!(kind(&f), ErrorKind::FnTypePosition);
}

#[test]
fn function_in_option_is_rejected() {
    let f = fx(
        "f() -> Int",
        "none",
        "  let c = move |x| emit(x);\n  let o = Some(c);\n  match o {\n    Some(h) => {\n      return h(1);\n    }\n    None => {\n      return 0;\n    }\n  }\n  return 1;",
    );
    assert_eq!(kind(&f), ErrorKind::FnTypePosition);
}

#[test]
fn function_returned_from_generic_call_is_rejected() {
    let id = fx("id<T>(x: T) -> T", "none", "  return x;");
    let f = fx(
        "f() -> Int",
        "none",
        "  let c = move |x| emit(x);\n  let d = id(c);\n  return d(1);",
    );
    assert_eq!(kind(&format!("{id}\n{f}")), ErrorKind::FnTypePosition);
}

#[test]
fn function_variable_cannot_be_let_mut() {
    let f = fx(
        "f() -> Int",
        "none",
        "  let mut c = move |x| x;\n  return c(1);",
    );
    assert_eq!(kind(&f), ErrorKind::FnValueReassign);
}

#[test]
fn function_value_cannot_be_reassigned() {
    let a = fx(
        "f() -> Int",
        "none",
        "  let mut c = move |x| x;\n  c = emit;\n  return c(1);",
    );
    assert_eq!(kind(&a), ErrorKind::FnValueReassign);
    let b = fx(
        "k(f: fn(Int) -> Int) -> Int",
        "none",
        "  let c = move |x| x;\n  c = f;\n  return c(1);",
    );
    assert_eq!(kind(&b), ErrorKind::FnValueReassign);
    let p = fx(
        "k(f: fn(Int) -> Int) -> Int",
        "none",
        "  f = emit;\n  return f(1);",
    );
    assert_eq!(kind(&p), ErrorKind::FnValueReassign);
    let g = format!(
        "{}\n{}",
        apply_all(),
        fx(
            "f() -> Int",
            "none",
            "  let mut c = move |x| x;\n  c = emit;\n  return apply_all(c, 1);"
        )
    );
    assert_eq!(kind(&g), ErrorKind::FnValueReassign);
}

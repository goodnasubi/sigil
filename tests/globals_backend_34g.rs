//! ADR 0025 補足 A-1 / A-7 の 3-4g: 大域の lowering・SIR の評価器・ネイティブ。
//!
//! - lowering: `Item::Global` が `Program.globals` に出て、参照が `GlobalRef` + `Deref` に下がる。
//!   `GlobalRef` の受け先の局所の型は、不変の大域なら `Ref<T>`、`global mut` なら `MutRef<T>`（可変性を固定する）。
//! - 差分試験: 読み・書き・構造体/組の大域・複数の関数からの共有を、評価器とネイティブで同じ結果にする。
//!   大域は単一スレッド前提（補足 A-5）。

mod common;

use sigil::exec::Options;
use sigil::ir::{GlobalInit, Operand, Place, Program, Proj, Rvalue, Stmt, Term, Ty};

const MODS: &[&str] = &["m.test"];

fn fn_def(name: &str, ret: &str, body: &str) -> String {
    format!(
        "fn {name}() -> {ret}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"単一スレッド\"\n{{\n{body}\n}}\n\n"
    )
}

fn module(items: &str) -> String {
    format!("module m.test\n  targets server\n  trust base\n\n{items}")
}

fn lower(src: &str, entry: &str) -> Program {
    let tb: Vec<String> = MODS.iter().map(|s| s.to_string()).collect();
    let p = sigil::ir_eval::lower_sources_with(&[src], entry, &tb)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    p
}

/// 評価器とネイティブで走らせ、同じ値になることを確かめて、その値を返す。
fn both(src: &str, entry: &str) -> String {
    lower(src, entry);
    let opts = Options {
        trust_base: MODS.iter().map(|s| s.to_string()).collect(),
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(&[src], entry, &[], &opts);
    common::native::check(&[src], entry, &[], &opts, &r);
    match r {
        Ok(o) => o.value,
        Err(f) => panic!("評価器が失敗: {:?}", f.error),
    }
}

// ---- lowering ----

/// 関数 `name` の中の `GlobalRef` の代入（受け先の局所の型, 大域の番号）。
fn global_refs(p: &Program, name: &str) -> Vec<(Ty, usize)> {
    let f = p.funcs.iter().find(|f| f.name == name).expect("関数がある");
    let mut out = vec![];
    for b in &f.blocks {
        for s in &b.stmts {
            if let Stmt::Assign(d, Rvalue::GlobalRef(g)) = s {
                assert!(d.proj.is_empty());
                out.push((f.locals[d.base].ty.clone(), *g));
            }
        }
    }
    out
}

/// 関数 `name` の中で、`Deref` を通る場所（`GlobalRef` の受け先の局所の先）を読み書きしているか。
fn uses_deref_of(p: &Program, name: &str, local_ty: &Ty) -> (bool, bool) {
    let f = p.funcs.iter().find(|f| f.name == name).unwrap();
    let is_g =
        |pl: &Place| pl.proj.first() == Some(&Proj::Deref) && &f.locals[pl.base].ty == local_ty;
    let (mut read, mut write) = (false, false);
    for b in &f.blocks {
        if let Term::CheckedOp { a, b, .. } = &b.term {
            for o in [a, b] {
                if let Operand::Copy(pl) | Operand::Move(pl) = o {
                    read |= is_g(pl);
                }
            }
        }
        for s in &b.stmts {
            if let Stmt::Assign(d, r) = s {
                if is_g(d) {
                    write = true;
                }
                let ops: Vec<&Operand> = match r {
                    Rvalue::Use(o) => vec![o],
                    Rvalue::Binary(_, a, b) => vec![a, b],
                    _ => vec![],
                };
                for o in ops {
                    if let Operand::Copy(pl) | Operand::Move(pl) = o {
                        if is_g(pl) {
                            read = true;
                        }
                    }
                }
            }
        }
    }
    (read, write)
}

#[test]
fn lowering_emits_globals_table() {
    let src = module(&format!(
        "struct Pt {{\n  x: Int,\n  y: Int,\n}}\n\nglobal A: Int = -5\nglobal mut B: Bool = true\nglobal mut P: Pt = Pt {{ y: 2, x: 1 }}\nglobal mut T: (Int, Pt) = (3, Pt {{ x: 4, y: 5 }})\nglobal mut N: Addr = raw.addr_null()\n\n{}",
        fn_def("main", "Int", "  return A;")
    ));
    let p = lower(&src, "main");
    let names: Vec<&str> = p.globals.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["A", "B", "P", "T", "N"]);
    let muts: Vec<bool> = p.globals.iter().map(|g| g.mutable).collect();
    assert_eq!(muts, [false, true, true, true, true]);
    assert_eq!(p.globals[0].ty, Ty::Int);
    assert_eq!(p.globals[0].init, GlobalInit::Int(-5));
    assert_eq!(p.globals[1].init, GlobalInit::Bool(true));
    // 構造体の初期値は宣言の順（書いた順 y, x ではなく x, y）。
    assert_eq!(
        p.globals[2].init,
        GlobalInit::Struct(vec![GlobalInit::Int(1), GlobalInit::Int(2)])
    );
    assert!(matches!(p.globals[2].ty, Ty::Adt(_)));
    assert_eq!(
        p.globals[3].init,
        GlobalInit::Tuple(vec![
            GlobalInit::Int(3),
            GlobalInit::Struct(vec![GlobalInit::Int(4), GlobalInit::Int(5)])
        ])
    );
    assert_eq!(p.globals[4].ty, Ty::Addr);
    assert_eq!(p.globals[4].init, GlobalInit::AddrNull);
}

#[test]
fn lowering_global_ref_mutability_is_fixed() {
    let src = module(&format!(
        "global R: Int = 1\nglobal mut W: Int = 2\n\n{}{}{}",
        fn_def("read_imm", "Int", "  return R;"),
        fn_def("read_mut", "Int", "  return W;"),
        fn_def(
            "main",
            "Int",
            "  W = W + R;\n  return read_imm() + read_mut();"
        )
    ));
    let p = lower(&src, "main");
    // 不変の大域は Ref<T>、global mut は MutRef<T>（mutable なのに Ref になっていないこと）。
    assert_eq!(
        global_refs(&p, "read_imm"),
        [(Ty::Ref(Box::new(Ty::Int)), 0)]
    );
    assert_eq!(
        global_refs(&p, "read_mut"),
        [(Ty::MutRef(Box::new(Ty::Int)), 1)]
    );
    // 読みは Deref 経由。
    assert_eq!(
        uses_deref_of(&p, "read_imm", &Ty::Ref(Box::new(Ty::Int))),
        (true, false)
    );
    assert_eq!(
        uses_deref_of(&p, "read_mut", &Ty::MutRef(Box::new(Ty::Int))),
        (true, false)
    );
    // 代入は MutRef 経由（Deref の先への書き込み）。不変の大域への GlobalRef は書き込みの先にならない。
    let refs = global_refs(&p, "main");
    let w_ty = Ty::MutRef(Box::new(Ty::Int));
    assert!(refs.contains(&(w_ty.clone(), 1)), "{refs:?}");
    assert!(refs.contains(&(Ty::Ref(Box::new(Ty::Int)), 0)), "{refs:?}");
    assert_eq!(uses_deref_of(&p, "main", &w_ty), (true, true));
    assert_eq!(
        uses_deref_of(&p, "main", &Ty::Ref(Box::new(Ty::Int))),
        (true, false)
    );
}

// ---- 評価器とネイティブの差分 ----

#[test]
fn read_immutable_and_mutable() {
    let src = module(&format!(
        "global A: Int = 42\nglobal mut B: Int = -7\nglobal F: Bool = true\n\n{}",
        fn_def(
            "main",
            "Int",
            "  let mut r = A + B;\n  if F {\n    r = r + 1000;\n  }\n  return r;"
        )
    ));
    assert_eq!(both(&src, "main"), "1035");
}

#[test]
fn write_and_overwrite() {
    let src = module(&format!(
        "global mut C: Int = 5\n\n{}",
        fn_def(
            "main",
            "Int",
            "  C = C + 1;\n  C = C * 2;\n  let mut i = 0;\n  while i < 3 {\n    C = C + i;\n    i = i + 1;\n  }\n  return C;"
        )
    ));
    assert_eq!(both(&src, "main"), "15");
}

#[test]
fn struct_global_read_write_fields_and_whole() {
    let src = module(&format!(
        "struct Pt {{\n  x: Int,\n  y: Int,\n}}\n\nglobal mut P: Pt = Pt {{ y: 2, x: 1 }}\n\n{}",
        fn_def(
            "main",
            "Int",
            "  P.x = P.x + 10;\n  let q = P;\n  P = Pt { x: 7, y: 8 };\n  P.y = P.y + 1;\n  return q.x * 1000 + q.y * 100 + P.x * 10 + P.y;"
        )
    ));
    assert_eq!(both(&src, "main"), "11279");
}

#[test]
fn tuple_and_nested_globals() {
    let src = module(&format!(
        "struct Pt {{\n  x: Int,\n  y: Int,\n}}\n\nglobal mut T: (Int, Bool) = (3, true)\nglobal mut N: (Int, Pt) = (4, Pt {{ x: 5, y: 6 }})\n\n{}",
        fn_def(
            "main",
            "Int",
            "  let (t0, t1) = T;\n  T = (t0 + 1, false);\n  let (m0, mp) = N;\n  N = (m0, Pt { x: mp.x, y: mp.y + 100 });\n  let (n0, np) = N;\n  N = (0, Pt { x: 1, y: 2 });\n  let (a0, ap) = N;\n  let (u0, u1) = T;\n  let mut r = u0 * 10000 + n0 * 1000 + np.x * 100 + np.y + ap.y + a0;\n  if u1 {\n    r = r + 1;\n  }\n  if t1 {\n    r = r + 5000000;\n  }\n  return r;"
        )
    ));
    // u0=4, n=(4,(5,106)), ap.y=2, t1=true
    assert_eq!(both(&src, "main"), "5044608");
}

#[test]
fn shared_across_functions() {
    let src = module(&format!(
        "struct St {{\n  hits: Int,\n  last: Int,\n}}\n\nglobal mut COUNT: Int = 0\nglobal mut S: St = St {{ hits: 0, last: 0 }}\nglobal LIMIT: Int = 3\n\n{}{}{}{}",
        fn_def(
            "bump",
            "Int",
            "  COUNT = COUNT + 1;\n  S.hits = S.hits + 1;\n  S.last = COUNT;\n  return COUNT;"
        ),
        fn_def(
            "twice",
            "Int",
            "  let a = bump();\n  let b = bump();\n  return a + b;"
        ),
        fn_def(
            "peek",
            "Int",
            "  return S.hits * 100 + S.last * 10 + COUNT;"
        ),
        fn_def(
            "main",
            "Int",
            "  let mut total = twice();\n  while COUNT < LIMIT + 2 {\n    total = total + bump();\n  }\n  return total * 1000 + peek();"
        )
    ));
    // twice = 1+2, loop adds 3+4+5 => 15, peek = 5*100 + 5*10 + 5 = 555
    assert_eq!(both(&src, "main"), "15555");
}

#[test]
fn closure_reads_global_without_capturing_it() {
    let src = module(&format!(
        "global mut K: Int = 10\n\n{}",
        fn_def(
            "main",
            "Int",
            "  let f = move |n| n + K;\n  let a = f(1);\n  K = 20;\n  return a * 100 + f(2);"
        )
    ));
    assert_eq!(both(&src, "main"), "1122");
}

// ---- 終了時の解放 ----

/// `trust base` を許可してネイティブの実行ファイルを作って走らせる。
fn run_native(src: &str, trace_drops: bool, env: &[(&str, &str)]) -> common::native::Ran {
    let dir = common::native::scratch("gexit");
    let exe = dir.join("prog");
    let o = sigil::codegen::Opts {
        trace_drops,
        trust_base: MODS.iter().map(|s| s.to_string()).collect(),
    };
    sigil::native::build(&[src], "main", &o, &exe).unwrap_or_else(|e| panic!("{e}"));
    let got = common::native::exec_env(&exe, &[], env);
    let _ = std::fs::remove_dir_all(&dir);
    got
}

/// 構造体・組の大域を上書きして終わるプログラム。上書きで古い値が、終了時に最後の値が解放される。
fn overwrite_src() -> String {
    module(&format!(
        "struct Pt {{\n  x: Int,\n  y: Int,\n}}\n\nglobal mut P: Pt = Pt {{ x: 1, y: 2 }}\nglobal mut T: (Int, Pt) = (3, Pt {{ x: 4, y: 5 }})\n\n{}",
        fn_def(
            "main",
            "Int",
            "  P = Pt { x: 10, y: 20 };\n  T = (6, Pt { x: 7, y: 8 });\n  P = Pt { x: 30, y: 40 };\n  return 0;"
        )
    ))
}

#[test]
fn overwritten_globals_drop_order_matches_evaluator() {
    let src = overwrite_src();
    lower(&src, "main");
    let opts = Options {
        trust_base: MODS.iter().map(|s| s.to_string()).collect(),
        trace_drops: true,
        ..Options::captured()
    };
    let r = sigil::ir_eval::run_sources_with(&[&src], "main", &[], &opts);
    common::native::check(&[&src], "main", &[], &opts, &r);
    let ran = run_native(&src, true, &[]);
    let drops: Vec<&str> = ran
        .stderr
        .lines()
        .filter_map(|l| l.strip_prefix("drop: "))
        .collect();
    assert_eq!(ran.code, Some(0), "{}", ran.stderr);
    // 上書きの 2 回に加えて、終了時に P の最後の値が落ちる（T の組の中の Pt も）。
    assert!(drops.len() >= 3, "終了時の解放が記録されない: {drops:?}");
}

#[test]
fn globals_leave_no_live_blocks_at_exit() {
    let src = overwrite_src();
    let got = run_native(&src, false, &[("SIGIL_RT_LIVE", "1")]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    let line = got
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {}", got.stderr));
    assert!(
        line.contains("strs=0") && line.contains("blocks=0"),
        "大域の塊が残っている: {line}"
    );
}

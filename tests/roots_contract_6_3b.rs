//! ADR 0031 決定 4-1 / PR 6-3b（残り）: 契約変更 `Builtin::RootsScope`・`RootReserve`・`RootStart` の追加を固定する。
//! lowering は名前を登録するだけで、検査器の登録・評価器・ネイティブの実体は PR 6-3d で入る。
mod common;
use common::run_with;
use sigil::exec::Options;
use sigil::ir::{Builtin, Callee};

#[test]
fn roots_builtins_are_distinct_in_the_sir_dump() {
    let bs = [
        (Builtin::RootsScope, "Builtin(RootsScope)"),
        (Builtin::RootReserve, "Builtin(RootReserve)"),
        (Builtin::RootStart, "Builtin(RootStart)"),
    ];
    for (b, n) in bs {
        let c = Callee::Builtin(b);
        assert_eq!(format!("{c:?}"), n);
        assert_ne!(c, Callee::Builtin(Builtin::TaskScope));
        assert_ne!(c, Callee::Builtin(Builtin::ScopeSpawn));
    }
    assert_ne!(Builtin::RootsScope, Builtin::RootReserve);
    assert_ne!(Builtin::RootReserve, Builtin::RootStart);
}

#[test]
fn existing_builtin_dump_names_are_unchanged() {
    for (b, n) in [
        (Builtin::IoWaitFd, "IoWaitFd"),
        (Builtin::TaskScope, "TaskScope"),
        (Builtin::ScopeSpawn, "ScopeSpawn"),
        (Builtin::AtomicStore, "AtomicStore"),
    ] {
        assert_eq!(format!("{b:?}"), n);
    }
}

/// 検査器はまだ `task.roots` を登録しない。呼び出しは、これまでどおり既存のエラーで止まる。
#[test]
fn calling_task_roots_still_fails_with_the_existing_error() {
    let src = "module m.t\n  targets server\n  trust none\n\nfn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects task.suspend\n{\n  return task.roots(1, |g| 0);\n}\n";
    let e = run_with(src, "t", &[], &Options::captured()).expect_err("未登録の呼び出しは通らない");
    let d = format!("{e:?}");
    assert!(d.contains("UnknownName") && d.contains("`task`"), "{d}");
}

/// `RootGroup`・`RootSlot` は検査器の前置き型にまだ無いので、利用者が同じ名前の構造体を宣言できる。
/// lowering の型の写し（番号）は、利用者の宣言を優先しなければならない（PR #394 の指摘）。
#[test]
fn user_structs_named_root_group_and_root_slot_still_work() {
    let src = "module m.t\n  targets server\n  trust none\n\nstruct RootGroup {\n  n: Int,\n}\n\nstruct RootSlot {\n  g: RootGroup,\n  k: Int,\n}\n\nfn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let s = RootSlot { g: RootGroup { n: 40 }, k: 2 };\n  return s.g.n + s.k;\n}\n";
    let r =
        run_with(src, "t", &[], &Options::captured()).expect("利用者の構造体はこれまでどおり動く");
    assert_eq!(r.value, "42");
}

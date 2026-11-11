//! ADR 0031 決定 4-1 / PR 6-3b（残り）: 契約変更 `Builtin::RootsScope`・`RootReserve`・`RootStart` の追加を固定する。
//! 検査器の登録と評価器の実体は PR 6-3d（`tests/task_roots_6_3d.rs`）。
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

/// PR 6-3d で `RootGroup`・`RootSlot` を前置き型にした（ADR 0031 決定 4-1）。`Scope` と同じく、
/// 利用者は同じ名前の型を宣言できない（PR #394 で持ち越した扱い）。
#[test]
fn user_structs_named_root_group_or_root_slot_are_rejected() {
    for name in ["RootGroup", "RootSlot"] {
        let src = format!("module m.t\n  targets server\n  trust none\n\nstruct {name} {{\n  n: Int,\n}}\n\nfn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  let s = {name} {{ n: 40 }};\n  return s.n;\n}}\n");
        let e = run_with(&src, "t", &[], &Options::captured()).expect_err("前置きの名前は使えない");
        let d = format!("{e:?}");
        assert!(d.contains("DuplicateDefinition") && d.contains(name), "{d}");
    }
}

//! ADR 0031 決定 1-1 / PR 6-3b: 契約変更 `Builtin::IoWaitFd` の追加だけを固定する。
//! lowering・検査器・評価器・ネイティブの実体は後続の PR（6-3c 以降）で入る。
mod common;
use common::run_with;
use sigil::exec::Options;
use sigil::ir::{Builtin, Callee};

#[test]
fn io_wait_fd_is_a_distinct_builtin_in_the_sir_dump() {
    let c = Callee::Builtin(Builtin::IoWaitFd);
    assert_eq!(format!("{c:?}"), "Builtin(IoWaitFd)");
    assert_ne!(c, Callee::Builtin(Builtin::TaskYield));
    assert_eq!(c.clone(), c);
}

#[test]
fn existing_builtin_dump_names_are_unchanged() {
    for (b, n) in [
        (Builtin::RawAtomicFence, "RawAtomicFence"),
        (Builtin::TaskSleep, "TaskSleep"),
        (Builtin::AtomicStore, "AtomicStore"),
    ] {
        assert_eq!(format!("{b:?}"), n);
    }
}

/// lowering はまだ `io.wait_fd` を登録しない。呼ぶ原始操作は、これまでどおり既存のエラーで止まる。
#[test]
fn calling_io_wait_fd_still_fails_with_the_existing_error() {
    let src = "module m.t\n  targets server\n  trust base\n\nfn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"試験\"\n{\n  return io.wait_fd(0, 1);\n}\n";
    let o = Options {
        trust_base: vec!["m.t".to_string()],
        ..Options::captured()
    };
    let e = run_with(src, "t", &[], &o).expect_err("未登録の呼び出しは通らない");
    let d = format!("{e:?}");
    assert!(d.contains("UnknownName") && d.contains("`io`"), "{d}");
}

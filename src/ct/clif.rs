//! 定数時間検査の CLIF 層（ADR 0020 決定 1・ADR 0061 決定 4 の段階 2）。
//!
//! ネイティブ生成の途中の中間表現（`SIGIL_CLIF_DUMP`）に、除算命令が現れていないか、条件分岐が
//! SIR の構造から説明できる数を超えていないかを確かめる。現在のネイティブ生成は Cranelift の
//! 最適化水準が `none` で、組み立てた CLIF がそのまま機械語になる。最適化水準を上げる変更を
//! 入れる場合は、この検査の前提（組み立てた CLIF＝最終形）が崩れる。

use crate::ir::*;
use std::collections::BTreeSet;

/// CLIF 出力から数えた、注目する命令の数。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClifCounts {
    /// 条件分岐（brif・br_table）。
    pub branch: usize,
    /// 除算・剰余（sdiv・udiv・srem・urem）。
    pub div: usize,
    /// 書き込み（store）。ゼロ化の消去を見るときの材料。
    pub store: usize,
}

pub fn count_clif(dump: &str) -> ClifCounts {
    let mut c = ClifCounts::default();
    for line in dump.lines() {
        let line = line.trim_start();
        if line.contains(" brif ") || line.starts_with("brif ") {
            c.branch += 1;
        }
        if line.contains(" br_table ") || line.starts_with("br_table ") {
            c.branch += 1;
        }
        for op in [" sdiv.", " udiv.", " srem.", " urem."] {
            if line.contains(op) {
                c.div += 1;
            }
        }
        if line.contains(" store ") || line.starts_with("store ") {
            c.store += 1;
        }
    }
    c
}

/// SIR の項のうち、ネイティブ生成で条件分岐になりうるもの数。
/// 分岐・振り分けに加えて、検査つき演算・表明・呼び出しは、異常停止へ
/// 進む分岐（桁あふれ・契約違反・スタックの検査）を 1 つまで伴いうる。
pub fn sir_branch_budget(func: &Func) -> usize {
    func.blocks
        .iter()
        .map(|b| match b.term {
            Term::Branch(..)
            | Term::Switch(..)
            | Term::CheckedOp { .. }
            | Term::Assert { .. }
            | Term::Call { .. } => 1,
            _ => 0,
        })
        .sum()
}

/// 入口の関数から `Callee::Func` の呼び出しで推移的に届く関数の番号（入口を含む）。
pub fn reachable_funcs(program: &Program, entry: &str) -> Vec<usize> {
    let root = program
        .funcs
        .iter()
        .position(|f| f.name == entry)
        .unwrap_or_else(|| panic!("{entry} がコーパスにない"));
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        for b in &program.funcs[id].blocks {
            if let Term::Call {
                func: Callee::Func(callee),
                ..
            } = &b.term
            {
                stack.push(*callee);
            }
            for st in &b.stmts {
                if let Stmt::Assign(_, Rvalue::MakeClosure(callee, _)) = st {
                    stack.push(*callee);
                }
            }
        }
    }
    seen.into_iter().collect()
}

/// 届く関数すべての、SIR の項から説明できる条件分岐の数の上限。関数ごとに入口の公開
/// スタック境界分岐が 1 つ、分岐・検査つき演算・表明は 1 つ、呼び出しは異常停止の
/// 伝播を含めて 2 つ、破棄は 1 つ（`src/codegen/func/drop.rs`）。
pub fn reach_branch_budget(program: &Program, reach: &[usize]) -> usize {
    reach
        .iter()
        .map(|id| {
            1 + program.funcs[*id]
                .blocks
                .iter()
                .map(|b| {
                    let drops = b
                        .stmts
                        .iter()
                        .filter(|s| matches!(s, Stmt::Drop(_)))
                        .count();
                    drops
                        + match &b.term {
                            Term::Branch(..)
                            | Term::Switch(..)
                            | Term::CheckedOp { .. }
                            | Term::Assert { .. } => 1,
                            // 標準ライブラリの部品（`List` の読み書きなど）は、呼び出し先の本体が
                            // CLIF へ展開され、添字の範囲検査と拡張の分岐を伴う。
                            Term::Call {
                                func: Callee::Builtin(_),
                                ..
                            } => 4,
                            Term::Call { .. } => 2,
                            _ => 0,
                        }
                })
                .sum::<usize>()
        })
        .sum()
}

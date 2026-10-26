//! SIR の検証器（ADR 0018 の決定 4、PR 1-4）の試験。
//! 正しい SIR は通り、lowering の誤りを模した書き換え（破棄の落とし・二重の破棄・移動後の使用・型の取り違えなど）は落ちる。
//! 破棄の順序の入れ替えは検証器では見つからない（各経路で 1 回ずつという数と、移動の前後だけを見る）。それは評価器と
//! 実行ファイルの突き合わせで見つける。

use sigil::ir::*;

const SRC: &str = "module m.a
  targets server
  trust none

struct Pair {
  name: String,
  dir: Path,
}

enum Tag {
  Named(String),
  Num(Int),
}

fn f(p: Pair, n: Int) -> Int
  intent \"x\"
  requires none
  ensures none
  effects none
{
  let s = p.name.clone();
  let t = s;
  if n > 1 {
    return t.len();
  }
  return n + 1;
}

fn g(t: Tag) -> Int
  intent \"x\"
  requires none
  ensures none
  effects none
{
  return match t {
    Tag.Named(x) => x.len(),
    Tag.Num(k) => k,
  };
}
";

fn lowered(func: &str) -> Program {
    sigil::ir_eval::lower_sources(&[SRC], func).unwrap_or_else(|e| panic!("{e:?}"))
}

fn errs(p: &Program) -> Vec<String> {
    sigil::verify::verify(p).err().unwrap_or_default()
}

/// 書き換えが当たり（`true`）、検証器が落ちることを確かめる。
fn caught(func: &str, what: &str, f: impl FnOnce(&mut Program) -> bool) {
    let mut p = lowered(func);
    assert!(errs(&p).is_empty(), "書き換える前に落ちた: {:?}", errs(&p));
    assert!(f(&mut p), "{what}: 書き換える場所が見つからない\n{p}");
    assert!(!errs(&p).is_empty(), "{what}: 検証器が見つけなかった\n{p}");
}

fn owned(p: &Program, f: usize, l: LocalId) -> bool {
    !matches!(
        p.funcs[f].locals[l].ty,
        Ty::Int | Ty::Bool | Ty::Unit | Ty::Float | Ty::Ref(_) | Ty::MutRef(_)
    )
}

#[test]
fn well_formed_programs_pass() {
    for f in ["f", "g"] {
        assert_eq!(errs(&lowered(f)), Vec::<String>::new());
    }
}

#[test]
fn missing_drop_on_a_path_is_caught() {
    caught("f", "破棄を落とす", |p| {
        for b in &mut p.funcs[0].blocks {
            if b.term == Term::Return {
                if let Some(i) = b.stmts.iter().position(|s| matches!(s, Stmt::Drop(_))) {
                    b.stmts.remove(i);
                    return true;
                }
            }
        }
        false
    });
}

#[test]
fn early_return_that_skips_drops_is_caught() {
    caught("f", "早期の戻りで破棄を飛ばす", |p| {
        let f = &mut p.funcs[0];
        let ret: Vec<usize> = (0..f.blocks.len())
            .filter(|&i| f.blocks[i].term == Term::Return && !f.blocks[i].stmts.is_empty())
            .collect();
        for b in &mut f.blocks {
            if let Term::Goto(t) = b.term {
                if ret.contains(&t) && b.stmts.iter().any(|s| matches!(s, Stmt::Assign(..))) {
                    b.term = Term::Return;
                    return true;
                }
            }
        }
        false
    });
}

#[test]
fn double_drop_is_caught() {
    caught("f", "破棄を 2 回出す", |p| {
        for fi in 0..p.funcs.len() {
            for bi in 0..p.funcs[fi].blocks.len() {
                let hit = p.funcs[fi].blocks[bi].stmts.iter().position(
                    |s| matches!(s, Stmt::Drop(pl) if pl.proj.is_empty() && owned(p, fi, pl.base)),
                );
                if let Some(i) = hit {
                    let s = p.funcs[fi].blocks[bi].stmts[i].clone();
                    p.funcs[fi].blocks[bi].stmts.insert(i + 1, s);
                    return true;
                }
            }
        }
        false
    });
}

#[test]
fn use_after_move_is_caught() {
    caught("f", "移動の後で使う", |p| {
        for b in &mut p.funcs[0].blocks {
            let hit = b.stmts.iter().position(|s| {
                matches!(s, Stmt::Assign(_, Rvalue::Use(Operand::Move(pl))) if pl.proj.is_empty())
            });
            if let Some(i) = hit {
                let s = b.stmts[i].clone();
                b.stmts.insert(i + 1, s);
                return true;
            }
        }
        false
    });
}

#[test]
fn struct_field_index_shift_is_caught() {
    caught("f", "構造体の欄の番号をずらす", |p| {
        for b in &mut p.funcs[0].blocks {
            for s in &mut b.stmts {
                if let Stmt::Assign(_, Rvalue::Clone(Operand::Copy(pl), _)) = s {
                    if let Some(Proj::Field(i)) = pl.proj.first_mut() {
                        *i = 1;
                        return true;
                    }
                }
            }
        }
        false
    });
}

#[test]
fn match_binding_mixup_is_caught() {
    caught("g", "match の腕の束縛の値を取り違える", |p| {
        for b in &mut p.funcs[0].blocks {
            for s in &mut b.stmts {
                if let Stmt::Assign(_, Rvalue::Use(Operand::Move(pl) | Operand::Copy(pl))) = s {
                    if let Some(Proj::Downcast(v)) = pl.proj.first_mut() {
                        *v = 1 - *v;
                        return true;
                    }
                }
            }
        }
        false
    });
}

#[test]
fn type_and_index_errors_are_caught() {
    caught("f", "局所の型を変える", |p| {
        let l = p.funcs[0]
            .locals
            .iter()
            .position(|l| l.ty == Ty::Int)
            .unwrap();
        p.funcs[0].locals[l].ty = Ty::Str;
        true
    });
    caught("f", "飛び先が範囲外", |p| {
        p.funcs[0].blocks[0].term = Term::Goto(999);
        true
    });
    caught("f", "分岐の条件が Bool でない", |p| {
        for b in &mut p.funcs[0].blocks {
            if let Term::Branch(c, ..) = &mut b.term {
                *c = Operand::Const(Const::Int(1));
                return true;
            }
        }
        false
    });
}

/// 分岐の腕を、何もしない `Return` だけの基本ブロックに付け替える。破棄の落としが「来ない腕」に紛れないことを確かめる。
#[test]
fn branch_straight_to_return_is_caught() {
    caught("f", "分岐の腕を直接の戻りにする", |p| {
        let f = &mut p.funcs[0];
        let ret = f.blocks.len();
        for bi in 0..ret {
            if let Term::Branch(_, _, e) = &mut f.blocks[bi].term {
                *e = ret;
                f.blocks.push(Block {
                    stmts: vec![],
                    term: Term::Return,
                });
                return true;
            }
        }
        false
    });
    caught("g", "Switch の腕を直接の戻りにする", |p| {
        let f = &mut p.funcs[0];
        let ret = f.blocks.len();
        for bi in 0..ret {
            if let Term::Switch(_, arms, _) = &mut f.blocks[bi].term {
                arms[0].1 = ret;
                f.blocks.push(Block {
                    stmts: vec![],
                    term: Term::Return,
                });
                return true;
            }
        }
        false
    });
}

/// 片方の腕だけで破棄し、合流の後でもう一度破棄する（破棄済みかもしれない）。
#[test]
fn conditional_double_drop_after_join_is_caught() {
    caught("f", "片方の腕だけ先に破棄する", |p| {
        let f = &mut p.funcs[0];
        let Some(&param) = f.params.iter().find(|&&l| owned_ty(&f.locals[l].ty)) else {
            return false;
        };
        for bi in 0..f.blocks.len() {
            if let Term::Branch(_, a, _) = f.blocks[bi].term {
                f.blocks[a].stmts.insert(0, Stmt::Drop(Place::local(param)));
                return true;
            }
        }
        false
    });
}

fn owned_ty(t: &Ty) -> bool {
    !matches!(
        t,
        Ty::Int | Ty::Bool | Ty::Unit | Ty::Float | Ty::Ref(_) | Ty::MutRef(_)
    )
}

#[test]
fn unreachable_arms_are_not_errors() {
    // 腕がすべて並んだ `match`（g）の既定の腕は `Unreachable` で、誤りにならない。
    let p = lowered("g");
    assert!(
        p.funcs
            .iter()
            .any(|f| f.blocks.iter().any(|b| b.term == Term::Unreachable)),
        "Unreachable の腕がない\n{p}"
    );
    assert_eq!(errs(&p), Vec::<String>::new());
}

/// `Goto`・`Call` の `next`・`cleanup` などから `Unreachable` に落ちる形は、分岐の腕ではないので誤り。
#[test]
fn edge_into_unreachable_other_than_a_branch_arm_is_caught() {
    caught("f", "Goto の先を Unreachable にする", |p| {
        let f = &mut p.funcs[0];
        let u = f.blocks.len();
        for bi in 0..u {
            if let Term::Goto(t) = &mut f.blocks[bi].term {
                *t = u;
                f.blocks.push(Block {
                    stmts: vec![],
                    term: Term::Unreachable,
                });
                return true;
            }
        }
        false
    });
    caught("f", "Call の cleanup を Unreachable にする", |p| {
        let f = &mut p.funcs[0];
        let u = f.blocks.len();
        for bi in 0..u {
            if let Term::Call { cleanup, .. }
            | Term::CheckedOp { cleanup, .. }
            | Term::Assert { cleanup, .. } = &mut f.blocks[bi].term
            {
                *cleanup = u;
                f.blocks.push(Block {
                    stmts: vec![],
                    term: Term::Unreachable,
                });
                return true;
            }
        }
        false
    });
}

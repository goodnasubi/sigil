//! ADR 0050 段1（#608）: `?` を含む `if`／`while` の条件で、分岐が読む値を破棄より先に取り出す。
//!
//! 報告形（#602）、真側・失敗側、`else if`、短絡評価、`while`、`Option` を、評価器の明示した期待値と
//! 突き合わせる。SIR は、分岐の条件が容器の内側を指さず、破棄済みの局所を読まないことを構造で確かめる。
//! ネイティブとの突き合わせは段3（新規統合試験）の範囲。段1の採取結果は PR に記録する。

use crate::exec::Options;
use crate::ir::{Operand, Program, Stmt, Term};

const VALUES: &str = r#"module main
  targets server
  trust none

fn ok(b: Bool) -> Result<Bool, String>
  intent "成功"
  requires none
  ensures none
  effects none
{
  return Ok(b);
}

fn bad() -> Result<Bool, String>
  intent "失敗"
  requires none
  ensures none
  effects none
{
  return Err("x");
}

fn some(b: Bool) -> Option<Bool>
  intent "Some"
  requires none
  ensures none
  effects none
{
  return Some(b);
}

fn get_none() -> Option<Bool>
  intent "None"
  requires none
  ensures none
  effects none
{
  return None;
}

fn direct(b: Bool) -> Result<Int, String>
  intent "if の条件に ? を直接置く"
  requires none
  ensures none
  effects none
{
  if ok(b)? {
    return Ok(1);
  }
  return Ok(0);
}

fn direct_bad() -> Result<Int, String>
  intent "失敗側"
  requires none
  ensures none
  effects none
{
  if bad()? {
    return Ok(1);
  }
  return Ok(0);
}

fn chain(a: Bool, b: Bool) -> Result<Int, String>
  intent "else if"
  requires none
  ensures none
  effects none
{
  if ok(a)? {
    return Ok(1);
  } else if ok(b)? {
    return Ok(2);
  }
  return Ok(0);
}

fn shortc(a: Bool, b: Bool) -> Result<Int, String>
  intent "短絡 a && f()?"
  requires none
  ensures none
  effects none
{
  if a && ok(b)? {
    return Ok(1);
  }
  return Ok(0);
}

fn shortc_bad(a: Bool) -> Result<Int, String>
  intent "短絡で失敗に到達するか"
  requires none
  ensures none
  effects none
{
  if a && bad()? {
    return Ok(1);
  }
  return Ok(0);
}

fn shortor(a: Bool, b: Bool) -> Result<Int, String>
  intent "短絡 a || f()?"
  requires none
  ensures none
  effects none
{
  if a || ok(b)? {
    return Ok(1);
  }
  return Ok(0);
}

fn looped(n: Int) -> Result<Int, String>
  intent "while の条件に ?"
  requires none
  ensures none
  effects none
{
  let mut i = 0;
  while ok(i < n)? {
    i = i + 1;
  }
  return Ok(i);
}

fn opt(b: Bool) -> Option<Int>
  intent "Option の条件"
  requires none
  ensures none
  effects none
{
  if some(b)? {
    return Some(1);
  }
  return Some(0);
}

fn opt_none() -> Option<Int>
  intent "None 側"
  requires none
  ensures none
  effects none
{
  if get_none()? {
    return Some(1);
  }
  return Some(0);
}

fn code(r: Result<Int, String>) -> Int
  intent "結果を符号に"
  requires none
  ensures none
  effects none
{
  match r {
    Ok(v) => { return v; }
    Err(_) => { return 9; }
  }
}

fn ocode(r: Option<Int>) -> Int
  intent "Option を符号に"
  requires none
  ensures none
  effects none
{
  match r {
    Some(v) => { return v; }
    None => { return 9; }
  }
}

pub fn main() -> Int
  intent "各形を 1 つの数にまとめる"
  requires none
  ensures none
  effects none
{
  let mut acc = 0;
  acc = acc * 10 + code(direct(true));
  acc = acc * 10 + code(direct(false));
  acc = acc * 10 + code(direct_bad());
  acc = acc * 10 + code(chain(false, true));
  acc = acc * 10 + code(chain(true, false));
  acc = acc * 10 + code(shortc(false, true));
  acc = acc * 10 + code(shortc(true, true));
  acc = acc * 10 + code(shortc_bad(false));
  acc = acc * 10 + code(shortc_bad(true));
  acc = acc * 10 + code(shortor(true, false));
  acc = acc * 10 + code(shortor(false, true));
  acc = acc * 10 + code(looped(3));
  acc = acc * 10 + ocode(opt(true));
  acc = acc * 10 + ocode(opt_none());
  return acc;
}
"#;

const DROPS: &str = r#"module main
  targets server
  trust none

struct Probe {
  id: Int,
}

fn bad() -> Result<Bool, String>
  intent "失敗"
  requires none
  ensures none
  effects none
{
  return Err("x");
}

fn ok(b: Bool) -> Result<Bool, String>
  intent "成功"
  requires none
  ensures none
  effects none
{
  return Ok(b);
}

fn fail_path() -> Result<Int, String>
  intent "失敗側で局所が 1 回だけ破棄される"
  requires none
  ensures none
  effects none
{
  let g = Probe { id: 7 };
  if bad()? {
    return Ok(g.id);
  }
  return Ok(0);
}

fn ok_path(b: Bool) -> Result<Int, String>
  intent "成功側は分岐の後の脱出で局所を 1 回破棄する"
  requires none
  ensures none
  effects none
{
  let g = Probe { id: 8 };
  if ok(b)? {
    return Ok(g.id);
  }
  return Ok(0);
}

fn skipped(a: Bool) -> Result<Int, String>
  intent "短絡で右辺に到達しない"
  requires none
  ensures none
  effects none
{
  let g = Probe { id: 9 };
  if a && bad()? {
    return Ok(1);
  }
  return Ok(g.id);
}

pub fn main() -> Int
  intent "入口"
  requires none
  ensures none
  effects none
{
  match fail_path() {
    Err(_) => { return 1; }
    Ok(v) => { return v; }
  }
}

pub fn m2() -> Int
  intent "成功側"
  requires none
  ensures none
  effects none
{
  match ok_path(true) {
    Err(_) => { return 1; }
    Ok(v) => { return v; }
  }
}

pub fn m3() -> Int
  intent "短絡"
  requires none
  ensures none
  effects none
{
  match skipped(false) {
    Err(_) => { return 1; }
    Ok(v) => { return v; }
  }
}
"#;

fn run_main(src: &str, func: &str) -> crate::exec::Outcome {
    let mut opts = Options::captured();
    opts.trace_drops = true;
    crate::ir_eval::run_with(src, func, &[], &opts).expect("評価器で実行できる")
}

/// 報告形の各経路を 1 つの数にまとめた値。桁は main の呼び出し順で、期待は次のとおり。
/// direct(true)=1, direct(false)=0, 失敗=9, else if (false,true)=2, (true,false)=1,
/// `a && f()?` (false,_)=0, (true,true)=1, 失敗に到達しない=0, 到達する=9,
/// `a || f()?` (true,_)=1, (false,true)=1, while=3, Option の Some(true)=1, None=9。
#[test]
fn conditions_with_question_mark_take_the_expected_branch() {
    let out = run_main(VALUES, "main");
    assert_eq!(out.value, "10921010911319");
}

/// 失敗側では局所を 1 回だけ破棄し、成功側では分岐の後の脱出で 1 回、短絡で右辺に到達しない経路では
/// 初期化されていない `?` の一時値を破棄せず、局所だけを 1 回破棄する。
#[test]
fn drops_happen_once_per_initialised_value() {
    let fail = run_main(DROPS, "main");
    assert_eq!(fail.value, "1");
    assert_eq!(fail.drops, vec!["Probe { id: 7 }"]);
    let ok = run_main(DROPS, "m2");
    assert_eq!(ok.value, "8");
    assert_eq!(ok.drops, vec!["Probe { id: 8 }"]);
    let skipped = run_main(DROPS, "m3");
    assert_eq!(skipped.value, "9");
    assert_eq!(skipped.drops, vec!["Probe { id: 9 }"]);
}

fn lowered(src: &str, func: &str) -> Program {
    crate::ir_eval::lower_sources(&[src], func).expect("SIR にできる")
}

/// 分岐の条件は、容器の内側（射影）を指さず、同じ基本ブロックで破棄された局所も読まない。
#[test]
fn branch_conditions_do_not_read_through_a_dropped_container() {
    for (src, func) in [
        (VALUES, "main"),
        (DROPS, "main"),
        (DROPS, "m2"),
        (DROPS, "m3"),
    ] {
        let program = lowered(src, func);
        let mut seen = 0;
        for f in &program.funcs {
            for (bi, b) in f.blocks.iter().enumerate() {
                let Term::Branch(Operand::Copy(p) | Operand::Move(p), ..) = &b.term else {
                    continue;
                };
                seen += 1;
                assert!(
                    p.proj.is_empty(),
                    "{}: ブロック {bi} の分岐が射影つきの場所を読む: {p:?}",
                    f.name
                );
                let dropped = b
                    .stmts
                    .iter()
                    .any(|s| matches!(s, Stmt::Drop(d) if d.base == p.base));
                assert!(
                    !dropped,
                    "{}: ブロック {bi} が破棄した局所を分岐で読む",
                    f.name
                );
            }
        }
        assert!(seen > 0);
    }
}

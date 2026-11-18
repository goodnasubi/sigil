//! ADR 0050 段3（Issue #610）: `?` を含む条件・対象式の新規統合試験。
//!
//! 評価器（`ir_eval`）とネイティブ（`codegen`）の両方を `common::run` / `common::run_with`
//! で走らせ、実比較の一致を突き合わせる。
//!
//! 検証対象（ADR 0050「突き合わせの追加」節）:
//! 1. `if` 条件の直接形と `let` 束縛の対照（`Ok(true)` の真側、`Ok(false)` の偽側、`Err(e)` の値と後続処理の不実行）。
//! 2. `else if` 連鎖と、短絡（`&&`、`||`）で `?` に到達しない場合／到達する場合。
//! 3. `while` の条件における真から偽への再評価と反復回数、反復途中の失敗による早期リターン。
//! 4. `match` 文／式の対象における成功値による腕の選択と、失敗時に照合へ進まない早期リターン。
//! 5. `Option` の `Some`／`None` による条件・ループ・照合。
//! 6. 破棄記録（`trace_drops`）による、分岐前の一時値と失敗時の局所の逆順破棄。

mod common;

use common::{run, run_with};
use sigil::exec::Options;

/// 1. if 条件の直接形と let 束縛の対照（Ok(true)、Ok(false)、Err(e) と後続不実行）
const IF_CONTRAST_SRC: &str = r#"module app.try_cond
  targets server
  trust none

fn ok(b: Bool) -> Result<Bool, String>
  intent "成功の真偽値を返す"
  requires none
  ensures none
  effects none
{
  return Ok(b);
}

fn bad() -> Result<Bool, String>
  intent "エラーを返す"
  requires none
  ensures none
  effects none
{
  return Err("if_err_token");
}

pub fn entry_if_direct_true() -> Result<Int, String>
  intent "if 直接形: Ok(true) で真側"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  if ok(true)? {
    side_effect = 10;
    return Ok(side_effect);
  } else {
    side_effect = 20;
    return Ok(side_effect);
  }
}

pub fn entry_if_let_true() -> Result<Int, String>
  intent "let 束縛形: Ok(true) で真側"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  let cond = ok(true)?;
  if cond {
    side_effect = 10;
    return Ok(side_effect);
  } else {
    side_effect = 20;
    return Ok(side_effect);
  }
}

pub fn entry_if_direct_false() -> Result<Int, String>
  intent "if 直接形: Ok(false) で偽側"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  if ok(false)? {
    side_effect = 10;
    return Ok(side_effect);
  } else {
    side_effect = 20;
    return Ok(side_effect);
  }
}

pub fn entry_if_let_false() -> Result<Int, String>
  intent "let 束縛形: Ok(false) で偽側"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  let cond = ok(false)?;
  if cond {
    side_effect = 10;
    return Ok(side_effect);
  } else {
    side_effect = 20;
    return Ok(side_effect);
  }
}

pub fn entry_if_direct_err() -> Result<Int, String>
  intent "if 直接形: Err(e) で早期リターン、後続不実行"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  if bad()? {
    side_effect = 10;
    return Ok(side_effect);
  }
  // 早期リターンのため到達しない
  side_effect = 999;
  return Ok(side_effect);
}

pub fn entry_if_let_err() -> Result<Int, String>
  intent "let 束縛形: Err(e) で早期リターン、後続不実行"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  let cond = bad()?;
  if cond {
    side_effect = 10;
    return Ok(side_effect);
  }
  // 早期リターンのため到達しない
  side_effect = 999;
  return Ok(side_effect);
}
"#;

#[test]
fn test_if_direct_and_let_contrast() {
    // Ok(true): 直接形と let 束縛形で同じ真側の結果
    let d_true = run(IF_CONTRAST_SRC, "entry_if_direct_true", &[]).unwrap();
    let l_true = run(IF_CONTRAST_SRC, "entry_if_let_true", &[]).unwrap();
    assert_eq!(d_true.value, "Ok(10)");
    assert_eq!(l_true.value, "Ok(10)");

    // Ok(false): 直接形と let 束縛形で同じ偽側の結果
    let d_false = run(IF_CONTRAST_SRC, "entry_if_direct_false", &[]).unwrap();
    let l_false = run(IF_CONTRAST_SRC, "entry_if_let_false", &[]).unwrap();
    assert_eq!(d_false.value, "Ok(20)");
    assert_eq!(l_false.value, "Ok(20)");

    // Err(e): 直接形と let 束縛形で同じエラー値、後続処理は実行されない
    let d_err = run(IF_CONTRAST_SRC, "entry_if_direct_err", &[]).unwrap();
    let l_err = run(IF_CONTRAST_SRC, "entry_if_let_err", &[]).unwrap();
    assert_eq!(d_err.value, "Err(\"if_err_token\")");
    assert_eq!(l_err.value, "Err(\"if_err_token\")");
}

/// 2. else if 連鎖と短絡評価（&&, ||）
const CHAIN_AND_SHORT_CIRCUIT_SRC: &str = r#"module app.try_cond
  targets server
  trust none

fn ok(b: Bool) -> Result<Bool, String>
  intent "成功の真偽値を返す"
  requires none
  ensures none
  effects none
{
  return Ok(b);
}

fn bad() -> Result<Bool, String>
  intent "エラーを返す"
  requires none
  ensures none
  effects none
{
  return Err("chain_err_token");
}

pub fn entry_else_if_first_true() -> Result<Int, String>
  intent "else if: 第1条件が真"
  requires none
  ensures none
  effects none
{
  if ok(true)? {
    return Ok(1);
  } else if ok(false)? {
    return Ok(2);
  }
  return Ok(3);
}

pub fn entry_else_if_second_true() -> Result<Int, String>
  intent "else if: 第2条件が真"
  requires none
  ensures none
  effects none
{
  if ok(false)? {
    return Ok(1);
  } else if ok(true)? {
    return Ok(2);
  }
  return Ok(3);
}

pub fn entry_else_if_both_false() -> Result<Int, String>
  intent "else if: 両方偽で else 側"
  requires none
  ensures none
  effects none
{
  if ok(false)? {
    return Ok(1);
  } else if ok(false)? {
    return Ok(2);
  }
  return Ok(3);
}

pub fn entry_else_if_first_err() -> Result<Int, String>
  intent "else if: 第1条件で Err"
  requires none
  ensures none
  effects none
{
  if bad()? {
    return Ok(1);
  } else if ok(true)? {
    return Ok(2);
  }
  return Ok(3);
}

pub fn entry_else_if_second_err() -> Result<Int, String>
  intent "else if: 第2条件で Err"
  requires none
  ensures none
  effects none
{
  if ok(false)? {
    return Ok(1);
  } else if bad()? {
    return Ok(2);
  }
  return Ok(3);
}

pub fn entry_shortc_and_skip() -> Result<Int, String>
  intent "短絡 a && f()?: 左辺が偽なら右辺の ? に到達しない"
  requires none
  ensures none
  effects none
{
  if false && bad()? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_and_eval_err() -> Result<Int, String>
  intent "短絡 a && f()?: 左辺が真なら右辺の ? に到達し Err"
  requires none
  ensures none
  effects none
{
  if true && bad()? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_and_eval_ok_true() -> Result<Int, String>
  intent "短絡 a && f()?: 左辺真、右辺 Ok(true) で真側"
  requires none
  ensures none
  effects none
{
  if true && ok(true)? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_and_eval_ok_false() -> Result<Int, String>
  intent "短絡 a && f()?: 左辺真、右辺 Ok(false) で偽側"
  requires none
  ensures none
  effects none
{
  if true && ok(false)? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_or_skip() -> Result<Int, String>
  intent "短絡 a || f()?: 左辺が真なら右辺の ? に到達しない"
  requires none
  ensures none
  effects none
{
  if true || bad()? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_or_eval_err() -> Result<Int, String>
  intent "短絡 a || f()?: 左辺が偽なら右辺の ? に到達し Err"
  requires none
  ensures none
  effects none
{
  if false || bad()? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_or_eval_ok_true() -> Result<Int, String>
  intent "短絡 a || f()?: 左辺偽、右辺 Ok(true) で真側"
  requires none
  ensures none
  effects none
{
  if false || ok(true)? {
    return Ok(1);
  }
  return Ok(0);
}

pub fn entry_shortc_or_eval_ok_false() -> Result<Int, String>
  intent "短絡 a || f()?: 左辺偽、右辺 Ok(false) で偽側"
  requires none
  ensures none
  effects none
{
  if false || ok(false)? {
    return Ok(1);
  }
  return Ok(0);
}
"#;

#[test]
fn test_else_if_chain() {
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_else_if_first_true", &[])
            .unwrap()
            .value,
        "Ok(1)"
    );
    assert_eq!(
        run(
            CHAIN_AND_SHORT_CIRCUIT_SRC,
            "entry_else_if_second_true",
            &[]
        )
        .unwrap()
        .value,
        "Ok(2)"
    );
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_else_if_both_false", &[])
            .unwrap()
            .value,
        "Ok(3)"
    );
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_else_if_first_err", &[])
            .unwrap()
            .value,
        "Err(\"chain_err_token\")"
    );
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_else_if_second_err", &[])
            .unwrap()
            .value,
        "Err(\"chain_err_token\")"
    );
}

#[test]
fn test_short_circuit_and_or() {
    // a && f()?
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_shortc_and_skip", &[])
            .unwrap()
            .value,
        "Ok(0)"
    );
    assert_eq!(
        run(
            CHAIN_AND_SHORT_CIRCUIT_SRC,
            "entry_shortc_and_eval_err",
            &[]
        )
        .unwrap()
        .value,
        "Err(\"chain_err_token\")"
    );
    assert_eq!(
        run(
            CHAIN_AND_SHORT_CIRCUIT_SRC,
            "entry_shortc_and_eval_ok_true",
            &[]
        )
        .unwrap()
        .value,
        "Ok(1)"
    );
    assert_eq!(
        run(
            CHAIN_AND_SHORT_CIRCUIT_SRC,
            "entry_shortc_and_eval_ok_false",
            &[]
        )
        .unwrap()
        .value,
        "Ok(0)"
    );

    // a || f()?
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_shortc_or_skip", &[])
            .unwrap()
            .value,
        "Ok(1)"
    );
    assert_eq!(
        run(CHAIN_AND_SHORT_CIRCUIT_SRC, "entry_shortc_or_eval_err", &[])
            .unwrap()
            .value,
        "Err(\"chain_err_token\")"
    );
    assert_eq!(
        run(
            CHAIN_AND_SHORT_CIRCUIT_SRC,
            "entry_shortc_or_eval_ok_true",
            &[]
        )
        .unwrap()
        .value,
        "Ok(1)"
    );
    assert_eq!(
        run(
            CHAIN_AND_SHORT_CIRCUIT_SRC,
            "entry_shortc_or_eval_ok_false",
            &[]
        )
        .unwrap()
        .value,
        "Ok(0)"
    );
}

/// 3. while 条件の再評価と反復回数、反復途中の失敗による早期リターン
const WHILE_SRC: &str = r#"module app.try_cond
  targets server
  trust none

fn ok(b: Bool) -> Result<Bool, String>
  intent "真偽値を返す"
  requires none
  ensures none
  effects none
{
  return Ok(b);
}

pub fn entry_while_loop_count() -> Result<Int, String>
  intent "while 条件の再評価と反復回数の確認"
  requires none
  ensures none
  effects none
{
  let mut count = 0;
  while ok(count < 5)? {
    count = count + 1;
  }
  return Ok(count);
}

fn step_fail(i: Int) -> Result<Bool, String>
  intent "i >= 3 でエラーを返す"
  requires none
  ensures none
  effects none
{
  if i >= 3 {
    return Err("while_mid_fail");
  }
  return Ok(true);
}

pub fn entry_while_early_fail() -> Result<Int, String>
  intent "while 反復途中の失敗による早期リターンと後続不実行"
  requires none
  ensures none
  effects none
{
  let mut i = 0;
  let mut side_effect = 0;
  while step_fail(i)? {
    i = i + 1;
    side_effect = side_effect + 1;
  }
  // 早期リターンのため到達しない
  side_effect = 999;
  return Ok(side_effect);
}
"#;

#[test]
fn test_while_loop() {
    assert_eq!(
        run(WHILE_SRC, "entry_while_loop_count", &[]).unwrap().value,
        "Ok(5)"
    );
    assert_eq!(
        run(WHILE_SRC, "entry_while_early_fail", &[]).unwrap().value,
        "Err(\"while_mid_fail\")"
    );
}

/// 4. match 文／式の対象における成功値の腕の選択と、失敗時の照合不実行
const MATCH_SRC: &str = r#"module app.try_cond
  targets server
  trust none

fn ok_val(v: Int) -> Result<Int, String>
  intent "成功整数"
  requires none
  ensures none
  effects none
{
  return Ok(v);
}

fn bad_val() -> Result<Int, String>
  intent "失敗"
  requires none
  ensures none
  effects none
{
  return Err("match_fail_token");
}

pub fn entry_match_stmt_ok() -> Result<Int, String>
  intent "match 文: 成功値で腕を選択"
  requires none
  ensures none
  effects none
{
  match ok_val(2)? {
    1 => { return Ok(10); }
    2 => { return Ok(20); }
    _ => { return Ok(30); }
  }
}

pub fn entry_match_stmt_bad() -> Result<Int, String>
  intent "match 文: 失敗時に腕に入らず早期リターン"
  requires none
  ensures none
  effects none
{
  let mut reached = 0;
  match bad_val()? {
    _ => {
      reached = 1;
      return Ok(10);
    }
  }
}

pub fn entry_match_expr_ok() -> Result<Int, String>
  intent "match 式: 成功値で腕を選択"
  requires none
  ensures none
  effects none
{
  let ans = match ok_val(1)? {
    1 => 100,
    2 => 200,
    _ => 300,
  };
  return Ok(ans);
}

pub fn entry_match_expr_bad() -> Result<Int, String>
  intent "match 式: 失敗時に腕に入らず早期リターン"
  requires none
  ensures none
  effects none
{
  let mut reached = 0;
  let ans = match bad_val()? {
    _ => 100,
  };
  reached = 1;
  return Ok(ans);
}
"#;

#[test]
fn test_match_target() {
    assert_eq!(
        run(MATCH_SRC, "entry_match_stmt_ok", &[]).unwrap().value,
        "Ok(20)"
    );
    assert_eq!(
        run(MATCH_SRC, "entry_match_stmt_bad", &[]).unwrap().value,
        "Err(\"match_fail_token\")"
    );
    assert_eq!(
        run(MATCH_SRC, "entry_match_expr_ok", &[]).unwrap().value,
        "Ok(100)"
    );
    assert_eq!(
        run(MATCH_SRC, "entry_match_expr_bad", &[]).unwrap().value,
        "Err(\"match_fail_token\")"
    );
}

/// 5. Option の Some／None による if・while・match
const OPTION_SRC: &str = r#"module app.try_cond
  targets server
  trust none

fn some_b(b: Bool) -> Option<Bool>
  intent "Some(Bool)"
  requires none
  ensures none
  effects none
{
  return Some(b);
}

fn none_b() -> Option<Bool>
  intent "None"
  requires none
  ensures none
  effects none
{
  return None;
}

pub fn entry_opt_if_true() -> Option<Int>
  intent "Option if: Some(true) で真側"
  requires none
  ensures none
  effects none
{
  if some_b(true)? {
    return Some(1);
  } else {
    return Some(0);
  }
}

pub fn entry_opt_if_false() -> Option<Int>
  intent "Option if: Some(false) で偽側"
  requires none
  ensures none
  effects none
{
  if some_b(false)? {
    return Some(1);
  } else {
    return Some(0);
  }
}

pub fn entry_opt_if_none() -> Option<Int>
  intent "Option if: None で早期リターン、後続不実行"
  requires none
  ensures none
  effects none
{
  let mut side_effect = 0;
  if none_b()? {
    side_effect = 10;
    return Some(side_effect);
  }
  // 早期リターンのため到達しない
  side_effect = 999;
  return Some(side_effect);
}

fn opt_step(i: Int) -> Option<Bool>
  intent "Option while 条件"
  requires none
  ensures none
  effects none
{
  if i >= 3 {
    return Some(false);
  }
  return Some(true);
}

fn opt_step_none(i: Int) -> Option<Bool>
  intent "Option while 途中 None"
  requires none
  ensures none
  effects none
{
  if i >= 2 {
    return None;
  }
  return Some(true);
}

pub fn entry_opt_while_count() -> Option<Int>
  intent "Option while: 再評価と反復回数"
  requires none
  ensures none
  effects none
{
  let mut i = 0;
  while opt_step(i)? {
    i = i + 1;
  }
  return Some(i);
}

pub fn entry_opt_while_none() -> Option<Int>
  intent "Option while: 途中 None による早期リターン"
  requires none
  ensures none
  effects none
{
  let mut i = 0;
  while opt_step_none(i)? {
    i = i + 1;
  }
  return Some(999);
}

fn some_i(n: Int) -> Option<Int>
  intent "Some(Int)"
  requires none
  ensures none
  effects none
{
  return Some(n);
}

fn none_i() -> Option<Int>
  intent "None"
  requires none
  ensures none
  effects none
{
  return None;
}

pub fn entry_opt_match_some() -> Option<Int>
  intent "Option match: Some で腕を選択"
  requires none
  ensures none
  effects none
{
  match some_i(2)? {
    1 => { return Some(10); }
    2 => { return Some(20); }
    _ => { return Some(30); }
  }
}

pub fn entry_opt_match_none() -> Option<Int>
  intent "Option match: None で早期リターン、照合へ進まない"
  requires none
  ensures none
  effects none
{
  match none_i()? {
    _ => { return Some(999); }
  }
}
"#;

#[test]
fn test_option_conditions() {
    assert_eq!(
        run(OPTION_SRC, "entry_opt_if_true", &[]).unwrap().value,
        "Some(1)"
    );
    assert_eq!(
        run(OPTION_SRC, "entry_opt_if_false", &[]).unwrap().value,
        "Some(0)"
    );
    assert_eq!(
        run(OPTION_SRC, "entry_opt_if_none", &[]).unwrap().value,
        "None"
    );
    assert_eq!(
        run(OPTION_SRC, "entry_opt_while_count", &[]).unwrap().value,
        "Some(3)"
    );
    assert_eq!(
        run(OPTION_SRC, "entry_opt_while_none", &[]).unwrap().value,
        "None"
    );
    assert_eq!(
        run(OPTION_SRC, "entry_opt_match_some", &[]).unwrap().value,
        "Some(20)"
    );
    assert_eq!(
        run(OPTION_SRC, "entry_opt_match_none", &[]).unwrap().value,
        "None"
    );
}

/// 6. 破棄記録（trace_drops）: 分岐前の一時値と失敗時の局所の逆順破棄
const DROPS_SRC: &str = r#"module app.try_cond
  targets server
  trust none

struct Probe {
  id: Int,
}

fn probe_bad(p: Probe) -> Result<Bool, String>
  intent "一時値を受け取って失敗"
  requires none
  ensures none
  effects none
{
  return Err("probe_err");
}

fn probe_ok(p: Probe, b: Bool) -> Result<Bool, String>
  intent "一時値を受け取って成功"
  requires none
  ensures none
  effects none
{
  return Ok(b);
}

pub fn entry_drop_fail_order() -> Result<Int, String>
  intent "失敗時: 分岐前の一時値の破棄と、失敗時の局所の逆順破棄"
  requires none
  ensures none
  effects none
{
  let a = Probe { id: 1 };
  let b = Probe { id: 2 };
  if probe_bad(Probe { id: 3 })? {
    return Ok(10);
  }
  return Ok(0);
}

pub fn entry_drop_ok_order() -> Result<Int, String>
  intent "成功時: 分岐前の一時値の破棄と、分岐脱出時の局所の破棄"
  requires none
  ensures none
  effects none
{
  let a = Probe { id: 1 };
  let b = Probe { id: 2 };
  if probe_ok(Probe { id: 3 }, true)? {
    let c = Probe { id: 4 };
    return Ok(c.id);
  }
  return Ok(0);
}

pub fn entry_drop_short_circuit_skip() -> Result<Int, String>
  intent "短絡で右辺に到達しないときの一時値未初期化と局所の破棄"
  requires none
  ensures none
  effects none
{
  let a = Probe { id: 10 };
  let b = Probe { id: 20 };
  if false && probe_bad(Probe { id: 30 })? {
    return Ok(1);
  }
  return Ok(0);
}
"#;

#[test]
fn test_drops_order_and_cleanup() {
    let mut opts = Options::captured();
    opts.trace_drops = true;

    // 失敗側: probe_bad 内で引数 Probe 3 が破棄され、? で早期リターンする際に
    // 局所変数 b (Probe 2)、a (Probe 1) が宣言の逆順で破棄される。
    let fail = run_with(DROPS_SRC, "entry_drop_fail_order", &[], &opts).unwrap();
    assert_eq!(fail.value, "Err(\"probe_err\")");
    assert_eq!(
        fail.drops,
        vec![
            "Probe { id: 3 }".to_string(),
            "Probe { id: 2 }".to_string(),
            "Probe { id: 1 }".to_string(),
        ]
    );

    // 成功側: probe_ok 内で引数 Probe 3 が破棄され、分岐先で c (Probe 4) が作られ、
    // 関数脱出時に c (4), b (2), a (1) が逆順で破棄される。
    let ok = run_with(DROPS_SRC, "entry_drop_ok_order", &[], &opts).unwrap();
    assert_eq!(ok.value, "Ok(4)");
    assert_eq!(
        ok.drops,
        vec![
            "Probe { id: 3 }".to_string(),
            "Probe { id: 4 }".to_string(),
            "Probe { id: 2 }".to_string(),
            "Probe { id: 1 }".to_string(),
        ]
    );

    // 短絡スキップ: 右辺が評価されないため Probe 30 は作られず破棄もされない。
    // 関数脱出時に局所変数 b (20), a (10) だけが逆順で破棄される。
    let skip = run_with(DROPS_SRC, "entry_drop_short_circuit_skip", &[], &opts).unwrap();
    assert_eq!(skip.value, "Ok(0)");
    assert_eq!(
        skip.drops,
        vec![
            "Probe { id: 20 }".to_string(),
            "Probe { id: 10 }".to_string(),
        ]
    );
}

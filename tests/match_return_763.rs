//! 借用した match の腕で束縛した Int を Result に入れて返す（#763）。

mod common;

#[test]
fn returns_int_bound_in_borrowed_match_arm() {
    let src = r#"
module m.match_return
  targets server
  trust none

fn select_group(group: Int) -> Result<Int, Int>
  intent "候補のグループを返す"
  requires none
  ensures none
  effects none
{
  let candidate = Some(group);
  match &candidate {
    Some(g) => { return Ok(g); }
    None => { return Err(1); }
  }
}
"#;
    let result = common::run(src, "select_group", &[sigil::exec::Arg::Int(42)]).unwrap();
    assert_eq!(result.value, "Ok(42)");
}

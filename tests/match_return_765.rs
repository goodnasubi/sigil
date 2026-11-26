//! 借用した match の腕で束縛した Int を直接 return する（#765）。

mod common;

#[test]
fn returns_int_bound_in_borrowed_match_arm_directly() {
    let src = r#"
module m.match_return_direct
  targets server
  trust none

fn select_group(group: Int) -> Int
  intent "候補のグループを返す"
  requires none
  ensures none
  effects none
{
  let candidate = Some(group);
  match &candidate {
    Some(g) => { return g; }
    None => { return 0; }
  }
}
"#;
    let result = common::run(src, "select_group", &[sigil::exec::Arg::Int(42)]).unwrap();
    assert_eq!(result.value, "42");
}

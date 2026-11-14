//! ADR 0034 §4-1: 所有移動・置換・排他的借用の正負試験。
mod common;
const HEAD: &str = "module app.main\n  targets server\n  trust none\n";
const TAIL: &str =
    "\n  intent \"所有移動の試験\"\n  requires none\n  ensures none\n  effects none\n";

#[test]
fn take_moves_fields_nested_and_noncopy_values() {
    let src = format!(
        "{HEAD}struct Boxed {{ item: Option<Option<Bytes>>, }}\npub fn main() -> Int{TAIL}{{
  let mut box = Boxed {{ item: Some(Some(\"abc\".as_bytes())) }};
  let taken = box.item.take();
  if !box.item.is_none() {{ return 1; }}
  if !box.item.take().is_none() {{ return 2; }}
  box.item = Some(None);
  let nested = box.item.take();
  match nested {{ Some(None) => {{}} _ => {{ return 3; }} }}
  match taken {{ Some(Some(bytes)) => {{ if bytes.len() != 3 {{ return 4; }} }} _ => {{ return 5; }} }}
  let mut n = Some(7);
  let a = n.take();
  n = Some(9);
  if a.unwrap_or(0) != 7 || n.take().unwrap_or(0) != 9 || !n.is_none() {{ return 6; }}
  return 0;
}}"
    );
    assert_eq!(common::run(&src, "main", &[]).unwrap().value, "0");
}

fn rejects(body: &str) {
    let src = format!("{HEAD}pub fn main() -> Int{TAIL}{{ {body} }}");
    assert!(sigil::ir_eval::lower_sources(&[&src], "main").is_err());
}

#[test]
fn take_requires_exclusive_mutable_access() {
    rejects("let x = Some(1); let y = x.take(); return 0;");
    rejects("let mut x = Some(1); let y = &x; let z = y.take(); return 0;");
    rejects("let mut x = Some(1); let y = &x; let z = x.take(); return y.unwrap_or(0);");
    rejects("let mut x = Some(Bytes.new()); let y = x; let z = x.take(); return 0;");
}

#[test]
fn moved_payload_is_dropped_once_on_return_and_abnormal_stop() {
    for stop in [false, true] {
        let end = if stop {
            "return 1 / zero;"
        } else {
            "return zero;"
        };
        let src = format!(
            "{HEAD}struct Probe {{ id: Int, }}\npub fn main(zero: Int) -> Int{TAIL}{{
  let mut original = Some(Probe {{ id: 42 }});
  let taken = original.take();
  original = None;
  {end}
}}"
        );
        let opts = sigil::exec::Options {
            trace_drops: true,
            ..sigil::exec::Options::captured()
        };
        let outcome = common::run_with(&src, "main", &[sigil::exec::Arg::Int(0)], &opts);
        let drops = if stop {
            let failure = outcome.unwrap_err();
            assert!(
                matches!(failure.error, sigil::exec::RunError::Stop(ref s) if s.kind == sigil::exec::StopKind::DivisionByZero)
            );
            failure.drops.to_vec()
        } else {
            outcome.unwrap().drops
        };
        assert_eq!(
            drops
                .iter()
                .filter(|s| s.as_str() == "Probe { id: 42 }")
                .count(),
            1,
            "{drops:?}"
        );
    }
}

#[test]
fn taking_a_socket_does_not_make_it_cloneable_or_shareable() {
    let clone = format!(
        "{HEAD}fn forbidden(socket: TcpClient){TAIL}{{
  let mut original = Some(socket); let taken = original.take(); let copy = taken.clone();
}}\npub fn main() -> Int{TAIL}{{ return 0; }}"
    );
    let error = sigil::check::check(&clone).unwrap_err();
    assert!(
        matches!(
            error.kind,
            sigil::ErrorKind::ConstraintUnsatisfied | sigil::ErrorKind::NoSuchMember
        ),
        "{error:?}"
    );
    let share = format!("{HEAD}fn require_share<T: Shareable>(value: T) -> Int{TAIL}{{ return 0; }}\nfn forbidden(socket: TcpClient){TAIL}{{
  let mut original = Some(socket); let taken = original.take(); let shared = require_share(taken);
}}\npub fn main() -> Int{TAIL}{{ return 0; }}");
    let error = sigil::check::check(&share).unwrap_err();
    assert_eq!(error.kind, sigil::ErrorKind::ConstraintUnsatisfied);
}

//! ADR 0028（Issue #291）: 効果 `task.suspend` の bare 許可の検証テスト。
//!
//! - bare を対象に含むモジュールでも、`task.yield`（効果 `task.suspend`）が
//!   宣言・利用できる（ADR 0024 決定 2 の PBKDF2 の協調点の前提）。
//! - `task.spawn`・`task.share` は従来どおり bare では使えない（広げたのは
//!   `task.suspend` だけ、という ADR 0028 の線引き）。
//! - `task.yield` を含むプログラムが、SIR 評価器とネイティブ出力の両方で
//!   同じ結果になる（ネイティブ出力にはスケジューラが無いため、yield は
//!   bare の意味づけどおり no-op になる）。

mod common;

use sigil::check::check;
use sigil::error::ErrorKind;
use sigil::exec::Arg;

fn check_bare(body: &str) -> Result<(), sigil::error::Error> {
    let src = format!("module m.a\n  targets bare\n  trust none\n\n{body}\n");
    check(&src)
}

#[test]
fn yield_is_allowed_on_bare() {
    check_bare(
        r#"
fn pause()
  intent "p"
  requires none
  ensures none
  effects task.suspend
{
  task.yield();
}
"#,
    )
    .expect("task.suspend は bare でも宣言できるはず");
}

#[test]
fn spawn_and_share_are_still_rejected_on_bare() {
    for effects in ["task.spawn, task.suspend", "task.share, task.suspend"] {
        let body = format!(
            r#"
fn pause()
  intent "p"
  requires none
  ensures none
  effects {effects}
{{
  task.yield();
}}
"#
        );
        let e = check_bare(&body).expect_err("task.spawn / task.share は bare では落ちるはず");
        assert_eq!(e.kind, ErrorKind::EffectTarget, "{effects}: {}", e.message);
    }
}

#[test]
fn yield_runs_the_same_on_eval_and_native() {
    // bare 対象のモジュールで、ループの中で yield しながら数える。
    // 評価器ではスケジューラが協調点として処理し、ネイティブ出力では
    // no-op になるが、結果は一致しなければならない（common::run が
    // 両者を突き合わせる）。
    let src = "module yield_bare.t\n  targets bare\n  trust none\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects task.suspend\n{\n  let mut acc = 0;\n  let mut i = 0;\n  while i < 5000 {\n    if i % 1024 == 0 {\n      task.yield();\n    }\n    acc = acc + 1;\n    i = i + 1;\n  }\n  return acc;\n}\n";
    let r = common::run(src, "main", &[] as &[Arg]).unwrap();
    assert_eq!(r.value, "5000");
}

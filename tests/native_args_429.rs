//! CLI のリテラルに戻せない値も、元の関数の実行結果と突き合わせる（#429）。
mod common;
use sigil::exec::Arg;

const HEAD: &str = "module m.args\n  targets server\n  trust none\n\n";

#[test]
fn strings_keep_their_kind_and_embedded_nul() {
    let src = format!(
        "{HEAD}fn echo(s: &String) -> String\n  intent \"値を返す\"\n  requires none\n  ensures none\n  effects none\n{{\n  return s.clone();\n}}\n"
    );
    for s in ["21", "true", "[1,2]", "a\0b"] {
        let got = common::run(&src, "echo", &[Arg::Str(s.into())]).unwrap();
        assert_eq!(got.value, sigil::shared_text::quote(s));
    }
    assert_checked("echo", 4);
}

#[test]
fn empty_list_of_structs_reaches_the_original_function() {
    let src = format!(
        "{HEAD}struct P {{\n  k: Int,\n}}\n\nfn empty(xs: &List<P>) -> Bool\n  intent \"空を調べる\"\n  requires none\n  ensures none\n  effects none\n{{\n  return xs.is_empty();\n}}\n"
    );
    let got = common::run(&src, "empty", &[Arg::List(vec![])]).unwrap();
    assert_eq!(got.value, "true");
    assert_checked("empty", 1);
}

/// 評価器の成功だけでなく、飛ばさず native-report に記録したことを確認する。
fn assert_checked(func: &str, count: usize) {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "native-report-native_args_429-{}.tsv",
        std::process::id()
    ));
    let report = std::fs::read_to_string(path).unwrap();
    assert_eq!(
        report
            .lines()
            .filter(|line| *line == format!("checked\t\t{func}"))
            .count(),
        count
    );
    assert!(!report
        .lines()
        .any(|line| line.starts_with("skip\t") && line.ends_with(&format!("\t{func}"))));
}

//! `src/borrow.rs` の単体テストと `tests/borrow_pairs.rs` のテストが、同じ名前の対になっていることの確認。
//! 検査自身は別のファイルに置くので、対の側の名前の一覧から何かを除く必要がない。

/// `#[test]` の直後の関数名を集める（補助の関数は `#[test]` が付かないので、除外の一覧は要らない）。
fn test_names(src: &str) -> Vec<String> {
    let mut names = vec![];
    let mut lines = src.lines().map(str::trim_start);
    while let Some(l) = lines.next() {
        if l != "#[test]" {
            continue;
        }
        for next in lines.by_ref() {
            if let Some(rest) = next.strip_prefix("fn ") {
                names.push(rest.split('(').next().unwrap_or(rest).to_string());
                break;
            }
        }
    }
    names.sort();
    names
}

/// `src/borrow.rs` の単体テストと、この枠のテストの名前が一致していること。
/// 片方だけ増減すると対が崩れるので、ここで落とす。
#[test]
fn unit_tests_and_pair_tests_have_the_same_names() {
    let unit_src = include_str!("../src/borrow.rs");
    let from = unit_src.find("mod tests {").expect("mod tests");
    let unit = test_names(&unit_src[from..]);
    let pair = test_names(include_str!("borrow_pairs.rs"));
    assert!(!unit.is_empty());
    assert_eq!(unit, pair);
}

#[test]
fn the_name_collector_finds_only_tests() {
    let src =
        "fn helper() {}\n#[test]\nfn a() {}\n// c\n#[test]\n// x\nfn b(x: i32) {}\nfn other() {}\n";
    assert_eq!(test_names(src), ["a", "b"]);
}

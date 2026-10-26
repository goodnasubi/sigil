//! ADR 0018 の決定 5（A）: 文字列とパスを解放する。作っては捨てる繰り返しで、生きている塊の数が
//! 繰り返しの回数に比例して増えないことを、実行ファイルの `SIGIL_RT_LIVE`（終了時に数を標準エラーへ出す）で確かめる。
//! 二重解放は、数が負になる・解放済みの印が残る塊をもう一度解放する、のいずれかでランタイムが異常終了して見つかる
//! （どのテストも終了コード 0 を求めるので、二重解放があれば落ちる）。

mod common;
use common::native::{exec_env, scratch, try_build};

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(sig: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    )
}

const DEFS: &str = "struct Pair {\n  name: String,\n  dir: Path,\n}\n\nenum Tag {\n  Named(String),\n  Empty,\n}\n\n";

/// 文字列・パスを作って捨てる経路を一通り通る。
fn churn() -> String {
    let mut s = String::from(HEAD);
    s.push_str(DEFS);
    s.push_str(&func(
        "mk(n: Int) -> Pair",
        "let s = \"name\";\nlet p = Path.new(&\"/etc/app\");\nreturn Pair { name: s, dir: p };",
    ));
    s.push_str(&func(
        "pick(t: Tag) -> Int",
        "match t {\n    Tag.Named(x) => {\n        let y = x.clone();\n        return y.len();\n    }\n    Tag.Empty => {\n        return 0;\n    }\n}",
    ));
    s.push_str(&func("echo(s: String) -> String", "return s;"));
    s.push_str(&func(
        "run(n: Int) -> Int",
        "let mut i = 0;\nlet mut total = 0;\nwhile i < n {\n    let a = \"  abc  \";\n    let b = a.trim();\n    let c = b.clone();\n    let d = echo(c);\n    total = total + d.len();\n    let p = Path.new(&\"/etc/app\");\n    let q = p.join(&\"conf.toml\");\n    let r = q.file_name().unwrap_or(\"\");\n    total = total + r.len();\n    let k = mk(i);\n    let k2 = k.name.clone();\n    total = total + k2.len();\n    let t = Tag.Named(\"zz\");\n    total = total + pick(t);\n    let l = \"a\\nb\".lines();\n    total = total + l.len();\n    let mut xs = List.new();\n    xs.push(\"ab\");\n    xs.push(\"c\");\n    let w = xs.get(0).map(|x| x.len()).unwrap_or(100);\n    total = total + w;\n    i = i + 1;\n}\nreturn total;",
    ));
    s
}

fn live(src: &str, n: i64) -> (i64, i64, String) {
    let dir = scratch("live");
    let exe = dir.join("prog");
    try_build(&[src], "run", &exe, false).unwrap_or_else(|e| panic!("{e}\n{src}"));
    let got = exec_env(&exe, &[n.to_string()], &[("SIGIL_RT_LIVE", "1")]);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(got.code, Some(0), "終了コード\n{}\n{src}", got.stderr);
    let line = got
        .stderr
        .lines()
        .find_map(|l| l.strip_prefix("live: "))
        .unwrap_or_else(|| panic!("live の行がない: {}", got.stderr))
        .to_string();
    let num = |key: &str| -> i64 {
        line.split(' ')
            .find_map(|w| w.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{key} がない: {line}"))
    };
    (num("strs="), num("blocks="), got.stdout)
}

#[test]
fn create_and_discard_does_not_grow_with_the_iteration_count() {
    let src = churn();
    let (s1, b1, out1) = live(&src, 1_000);
    assert!(out1.trim_end().ends_with("22000"), "{out1}");
    let (s2, b2, out) = live(&src, 100_000);
    assert!(out.trim_end().ends_with("2200000"), "{out}");
    // 回数に比例しない: 100 倍の繰り返しでも、生きている数は変わらない（出力の組み立てなど一定の分だけ）。
    assert_eq!(
        (s1, b1),
        (s2, b2),
        "1000 回と 100000 回で生きている数が違う"
    );
    assert!(
        s2 < 100 && b2 < 100,
        "生きている数が多い: strs={s2} blocks={b2}"
    );
}

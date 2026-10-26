//! ADR 0020 の決定 2（PR 1-6）: 構文解析の入れ子の深さの上限（128）で、全段がスタックを使い切らない。
//!
//! 受け入れられる最も深い入れ子（上限ちょうど）の入力を、式・型・ブロック・`match` ごとに作り、
//! `check`・`fmt`・lowering・`run`（SIR の評価器）・`build`（ネイティブ）を、2 MiB のスタックの
//! スレッド（Rust のテストの既定）で通す。上限の 1 つ上は `TooDeep` になる。

use sigil::interp::{Arg, Options};
use sigil::{parse, ErrorKind};

const HEAD: &str = "module m.d\n  targets server\n  trust none\n\n";
const CLAUSES: &str = "  intent \"d\"\n  requires none\n  ensures none\n  effects none\n";
const STACK: usize = 2 * 1024 * 1024;

fn func(params: &str, body: &str) -> String {
    format!("fn f({params}) -> Int\n{CLAUSES}{{\n{body}\n}}\n")
}

/// 入れ子の深さ n の入力を作る関数の一覧。
type Shape = Box<dyn Fn(usize) -> String + Sync>;

fn shapes() -> Vec<(&'static str, Shape)> {
    vec![
        (
            "paren",
            Box::new(|n| {
                format!(
                    "{HEAD}{}",
                    func(
                        "x: Int",
                        &format!("  return {}x{};", "(".repeat(n), ")".repeat(n))
                    )
                )
            }),
        ),
        (
            "unary",
            Box::new(|n| {
                format!(
                    "{HEAD}{}",
                    func("x: Int", &format!("  return {}x;", "- ".repeat(n)))
                )
            }),
        ),
        (
            "type",
            Box::new(|n| {
                let ty = format!("{}Int{}", "List<".repeat(n), ">".repeat(n));
                format!(
                    "{HEAD}fn t(p: {ty}) -> Int\n{CLAUSES}{{\n  return 0;\n}}\n\n{}",
                    func("x: Int", "  return x;")
                )
            }),
        ),
        (
            "block",
            Box::new(|n| {
                let body = format!(
                    "{}  return x;\n{}",
                    "if true {\n".repeat(n),
                    "}\n".repeat(n)
                );
                format!("{HEAD}{}", func("x: Int", &format!("{body}  return x;")))
            }),
        ),
        (
            "match",
            Box::new(|n| {
                // 内側の腕に 2 文を置き、`return match ...;` への整形の畳み込みが起きない形にする。
                let open = "match x {\n  0 => {\n".repeat(n);
                let close = "  }\n  _ => {\n    let y = 1;\n    return y;\n  }\n}\n".repeat(n);
                format!(
                    "{HEAD}{}",
                    func("x: Int", &format!("{open}  return 2;\n{close}  return x;"))
                )
            }),
        ),
    ]
}

fn accepted(src: &str) -> bool {
    match parse(src) {
        Ok(_) => true,
        Err(e) => {
            assert_eq!(e.kind, ErrorKind::TooDeep, "{}", e);
            false
        }
    }
}

/// 受け入れられる最大の n。
fn limit(make: &dyn Fn(usize) -> String) -> usize {
    let (mut lo, mut hi) = (1, 1000);
    assert!(accepted(&make(lo)));
    assert!(!accepted(&make(hi)));
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if accepted(&make(mid)) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn on_small_stack<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .stack_size(STACK)
            .spawn_scoped(s, f)
            .unwrap()
            .join()
            .unwrap()
    })
}

#[test]
fn depth_limit_all_phases() {
    let exe_dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    for (name, make) in shapes() {
        on_small_stack(|| {
            let n = limit(&*make);
            let src = make(n);
            assert!(!accepted(&make(n + 1)), "{name}: 上限の 1 つ上");
            sigil::check::check(&src).unwrap_or_else(|e| panic!("{name} n={n} check: {e}"));
            let out = sigil::fmt::format(&src).unwrap_or_else(|e| panic!("{name} fmt: {e}"));
            assert_eq!(sigil::fmt::format(&out).unwrap(), out, "{name}: fmt の冪等");
            sigil::ir_eval::lower_sources(&[&src], "f")
                .unwrap_or_else(|e| panic!("{name} lower: {e}"));
            let opts = Options {
                stack_bytes: STACK,
                ..Options::captured()
            };
            let r = sigil::ir_eval::run_with(&src, "f", &[Arg::Int(1)], &opts)
                .unwrap_or_else(|e| panic!("{name} run: {e}"));
            let exe = exe_dir.join(format!("depth-{name}-{}", std::process::id()));
            sigil::native::build(&[&src], "f", &sigil::codegen::Opts::default(), &exe)
                .unwrap_or_else(|e| panic!("{name} build: {e}"));
            let o = std::process::Command::new(&exe).arg("1").output().unwrap();
            let _ = std::fs::remove_file(&exe);
            assert!(o.status.success(), "{name}: 実行ファイル");
            assert!(
                String::from_utf8_lossy(&o.stdout)
                    .trim_end()
                    .ends_with(&r.value.to_string()),
                "{name}: 値の一致"
            );
        });
    }
}

/// すべての腕が `return` だけの入れ子の `match` は、整形が式の `match` に畳むので、式の側の深さの数え方で
/// 上限を超えることがある。その場合は、スタックを使い切らず、整形のエラー（`FmtVerify`）になる。
#[test]
fn depth_limit_fmt_fold_is_an_error_not_a_crash() {
    let make = |n: usize| {
        let open = "match x {\n  0 => {\n".repeat(n);
        let close = "  }\n  _ => {\n    return 1;\n  }\n}\n".repeat(n);
        format!(
            "{HEAD}{}",
            func("x: Int", &format!("{open}  return 2;\n{close}  return x;"))
        )
    };
    on_small_stack(|| {
        let n = limit(&make);
        match sigil::fmt::format(&make(n)) {
            Ok(out) => assert_eq!(sigil::fmt::format(&out).unwrap(), out),
            Err(e) => assert_eq!(e.kind, ErrorKind::FmtVerify, "{e}"),
        }
        sigil::check::check(&make(n)).unwrap();
    });
}

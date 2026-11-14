//! Sigil 製 std.json の短い fuzz と評価器／ネイティブの突き合わせ（#455）。
//! 長い実行: SIGIL_FUZZ_ITERS=20000 SIGIL_FUZZ_SEED=7 cargo test --release --test fuzz_app_json
//! fuzz.yml への長い実行の追加は人間依頼。本変更では workflow を触らない。

mod common;
#[path = "common/fuzz.rs"]
mod fuzz;

use sigil::exec::{Arg, Options};
use std::process::Command;

const DRIVER: &str = include_str!("fuzz/app_json_driver.sigil");

fn arg(bytes: &[u8]) -> Arg {
    Arg::List(bytes.iter().map(|&b| Arg::Int(i64::from(b))).collect())
}

fn roots(function: &str, sets: &[Vec<Arg>]) -> Vec<String> {
    sigil::verify::verify_sources(&[DRIVER], function).unwrap();
    let mut opts = Options::captured();
    opts.stack_bytes = 512 * 1024;
    sigil::ir_eval::run_roots_with(&[DRIVER], function, sets, &opts)
        .unwrap_or_else(|e| panic!("{function}: {e:?}"))
        .roots
        .into_iter()
        .map(|r| r.unwrap_or_else(|e| panic!("{function}: {e:?}")))
        .collect()
}

fn decoded(s: &str) -> String {
    // 評価器の表示は Rust Debug の \u{...} を含むため、JSON でなく Sigil の文字列として読む。
    let lexed = sigil::lexer::lex(s).unwrap();
    match &lexed.tokens[0].tok {
        sigil::lexer::Tok::Str(s) => s.clone(),
        other => panic!("文字列でない: {other:?}"),
    }
}

fn nested(depth: usize, object: bool) -> Vec<u8> {
    if object {
        format!("{}0{}", "{\"k\":".repeat(depth), "}".repeat(depth)).into_bytes()
    } else {
        format!("{}0{}", "[".repeat(depth), "]".repeat(depth)).into_bytes()
    }
}

fn cases() -> Vec<(Vec<u8>, bool)> {
    let valid: &[&[u8]] = &[
        b"null",
        b"true",
        b"false",
        b"0",
        b"-0",
        b"9223372036854775807",
        b"-9223372036854775808",
        b"1.25",
        b"-0.0",
        b"1e0",
        b"1E+2",
        b"1e-1000000",
        b"0e999999999999999999999999999",
        b"[]",
        b"{}",
        b"[[],{},true,null]",
        b"{\"a\":1,\"b\":[2,3]}",
        b"\"\\u0000\\b\\f\\n\\r\\t\\/\\\\\\\"\"",
        b"\"\\ud83d\\ude00\"",
        b"\"\\uDBFF\\uDFFF\"",
        "\"日本語😀\"".as_bytes(),
        b" \r\n\t[0] \r\n\t",
        b"1.7976931348623157e308",
        b"5e-324",
        b"2.2250738585072014e-308",
    ];
    let invalid: &[&[u8]] = &[
        b"",
        b" ",
        b"[",
        b"{",
        b"nullx",
        b"true false",
        b"+1",
        b"01",
        b"-01",
        b".1",
        b"1.",
        b"1e",
        b"1e+",
        b"--1",
        b"NaN",
        b"Infinity",
        b"1e309",
        b"1.7976931348623159e308",
        b"9223372036854775808",
        b"-9223372036854775809",
        b"[1,]",
        b"[,1]",
        b"[1 2]",
        b"{1:0}",
        b"{\"a\" 0}",
        b"{\"a\":0,}",
        b"{\"a\":0,\"a\":1}",
        b"{\"a\":0,\"\\u0061\":1}",
        b"\"\\ud800\"",
        b"\"\\udc00\"",
        b"\"\\ud800\\u0061\"",
        b"\"\\uD800\\uD800\"",
        b"\"\\u12xz\"",
        b"\"\\x00\"",
        b"\"\n\"",
        b"\"\xff\"",
        b"\"\xc0\x80\"",
        b"\"\xed\xa0\x80\"",
        b"\"\xf4\x90\x80\x80\"",
        b"\"\xe3\x81\"",
        b"\xef\xbb\xbfnull",
        b"\x0cnull",
        b"null\0",
    ];
    let mut out: Vec<_> = valid
        .iter()
        .map(|b| (b.to_vec(), true))
        .chain(invalid.iter().map(|b| (b.to_vec(), false)))
        .collect();
    for object in [false, true] {
        out.push((nested(64, object), true));
        out.push((nested(65, object), false));
        out.push((nested(10_000, object), false));
    }
    out.push((
        format!("{}{}", "[".repeat(64), "]".repeat(64)).into_bytes(),
        true,
    ));
    out.push((
        format!("{}{}", "[".repeat(65), "]".repeat(65)).into_bytes(),
        false,
    ));
    out
}

#[test]
fn syntax_roundtrip_and_depth_boundaries() {
    let inputs = cases();
    let sets: Vec<_> = inputs.iter().map(|(b, _)| vec![arg(b)]).collect();
    for ((input, valid), got) in inputs.iter().zip(roots("probe", &sets)) {
        let got = decoded(&got);
        assert!(!got.starts_with("NG"), "{input:?}: {got}");
        assert_eq!(got != "E400", *valid, "{input:?}: {got}");
    }
}

#[test]
fn public_flat_tables_and_deep_clone() {
    let cases = [
        (64, 0, 200),
        (65, 0, 400),
        (10_000, 0, 400),
        (0, 1, 400),
        (0, 2, 400),
        (0, 3, 400),
        (0, 4, 400),
        (0, 5, 400),
        (0, 6, 400),
        (0, 7, 400),
        (0, 8, 400),
        (0, 9, 200),
        (65, 10, 200),
    ];
    let sets: Vec<_> = cases
        .iter()
        .map(|&(depth, bad, _)| vec![Arg::Int(depth), Arg::Int(bad)])
        .collect();
    for ((depth, bad, expected), got) in cases.iter().zip(roots("table", &sets)) {
        assert_eq!(got, expected.to_string(), "depth={depth}, bad={bad}");
    }
    // 1万段を構築・clone・破棄してから400。ネイティブでも同じ入口を確認する。
    let mut opts = Options::captured();
    opts.stack_bytes = 512 * 1024;
    assert_eq!(
        common::run_with(DRIVER, "table", &[Arg::Int(10_000), Arg::Int(0)], &opts)
            .unwrap()
            .value,
        "400"
    );
}

#[test]
fn decimal_float_matches_rust_rounding() {
    let mut inputs = vec![
        "0.0".to_owned(),
        "-0.0".into(),
        "1e-324".into(),
        "5e-324".into(),
        "2.2250738585072014e-308".into(),
        "2.225073858507201e-308".into(),
        "1.7976931348623157e308".into(),
        "1.7976931348623158e308".into(),
        "9007199254740993.0".into(),
        "0.1000000000000000055511151231257827021181583404541015625".into(),
        "1.00000000000000011102230246251565404236316680908203125".into(),
        format!(
            "1.00000000000000011102230246251565404236316680908203125{}1",
            "0".repeat(1200)
        ),
    ];
    let mut rng = fuzz::Rng::new(fuzz::DEFAULT_SEED ^ 455);
    for _ in 0..80 {
        let bits = rng.next_u64();
        let x = f64::from_bits(bits);
        if x.is_finite() {
            inputs.push(format!("{x:e}"));
        }
    }
    let sets: Vec<_> = inputs.iter().map(|s| vec![arg(s.as_bytes())]).collect();
    for (input, got) in inputs.iter().zip(roots("numeric", &sets)) {
        let expected = input.parse::<f64>().unwrap().to_bits() as i64;
        assert_eq!(got, expected.to_string(), "{input}");
    }
}

fn generated() -> Vec<Vec<u8>> {
    let corpus: Vec<_> = cases()
        .into_iter()
        .filter(|(b, _)| b.len() < 1024)
        .map(|(b, _)| b)
        .collect();
    let mut all = corpus.clone();
    let mut rng = fuzz::Rng::new(fuzz::seed());
    for i in 0..fuzz::iters(120) {
        let b = match i % 3 {
            0 => fuzz::random_bytes(&mut rng),
            1 => {
                let base = rng.pick(&corpus).clone();
                fuzz::mutate(&mut rng, &base, &corpus)
            }
            _ => format!(
                "{{\"k{}\":[{},true,\"\\u0061\",null,{}.{}e{}]}}",
                rng.below(10),
                rng.below(10000),
                rng.below(1000),
                rng.below(100),
                rng.below(10)
            )
            .into_bytes(),
        };
        all.push(b);
    }
    all
}

#[test]
fn fuzz_matches_eval_and_native() {
    let ins = generated();
    let sets: Vec<_> = ins.iter().map(|b| vec![arg(b)]).collect();
    let dir = common::native::scratch("app-json-fuzz");
    std::fs::create_dir_all(&dir).unwrap();
    let failures =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/fuzz-failures/app_json");
    std::fs::create_dir_all(&failures).unwrap();
    let saved = failures.join("current.hex");
    let text: String = ins
        .iter()
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>() + "\n")
        .collect();
    std::fs::write(&saved, text).unwrap();
    for (batch, inputs) in sets.chunks(64).zip(ins.chunks(64)) {
        for (input, got) in inputs.iter().zip(roots("probe", batch)) {
            let rec = decoded(&got);
            if rec.starts_with("NG") {
                let path = failures.join(format!("fail-{}", fuzz::seed()));
                std::fs::write(&path, input).unwrap();
                panic!("種 {}: {}: {got}", fuzz::seed(), path.display());
            }
            if rec != "E400" {
                sigil::json::parse(&rec)
                    .unwrap_or_else(|e| panic!("出力はJSONであること: {rec}: {e:?}"));
            }
        }
    }
    // CLI の同じ main を1回ずつ動かす。空行は lines が落とすため、空入力は上の roots で検証。
    let exe = dir.join("json");
    sigil::native::build(&[DRIVER], "main", &sigil::codegen::Opts::default(), &exe).unwrap();
    std::fs::create_dir_all(dir.join("app")).unwrap();
    let source = dir.join("app/main.sigil");
    std::fs::write(&source, DRIVER).unwrap();
    let eval = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(["run", "--root"])
        .arg(&dir)
        .arg(&source)
        .arg("main")
        .arg(&saved)
        .env("SIGIL_RT_LIVE", "1")
        .output()
        .unwrap();
    assert!(
        eval.status.success(),
        "{}",
        String::from_utf8_lossy(&eval.stderr)
    );
    assert!(String::from_utf8_lossy(&eval.stderr).contains("live: frames=0 chunks=0"));
    for workers in [1, 4] {
        let native = Command::new(&exe)
            .arg(&saved)
            .env("SIGIL_RT_LIVE", "1")
            .env("SIGIL_WORKERS", workers.to_string())
            .output()
            .unwrap();
        assert!(
            native.status.success(),
            "{}",
            String::from_utf8_lossy(&native.stderr)
        );
        assert!(String::from_utf8_lossy(&native.stderr).contains("live: sigil_blocks=0"));
        assert_eq!(native.stdout, eval.stdout, "workers={workers}");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

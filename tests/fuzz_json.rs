//! JSON の読み取り・書き出しのファジング（ADR 0020 の決定 2）。枠は `tests/common/fuzz.rs`。
//!
//! 性質: 落ちない／スタックを使い切らない。読めた値を書き出して読み直すと同じ値になる。
//! 入れ子が `MAX_DEPTH` を超える入力は（落ちずに）エラーになる。

#[path = "common/fuzz.rs"]
mod fuzz;

use fuzz::Rng;
use sigil::json::{self, Value};

fn check(input: &[u8]) {
    let src = String::from_utf8_lossy(input);
    if let Ok(v) = json::parse(&src) {
        let out = v.to_json();
        let again = json::parse(&out).unwrap_or_else(|e| panic!("書き出しが読めない: {e}: {out}"));
        assert_eq!(again, v, "往復で値が変わった: {out}");
        assert_eq!(again.to_json(), out, "書き出しが固定点でない");
    }
}

fn gen_str(rng: &mut Rng, out: &mut String) {
    out.push('"');
    for _ in 0..rng.below(6) {
        match rng.below(8) {
            0 => {
                out.push_str(rng.pick(&["\\n", "\\\"", "\\\\", "\\/", "\\t", "\\b", "\\f", "\\r"]))
            }
            1 => out.push_str(rng.pick(&[
                "\\u0041",
                "\\u00e9",
                "\\ud83d\\ude00",
                "\\ud83d",
                "\\ude00",
                "\\u12",
            ])),
            2 => out.push(*rng.pick(&['あ', '😀', 'é', '\u{7f}'])),
            _ => out.push((b'a' + rng.below(26) as u8) as char),
        }
    }
    out.push('"');
}

fn gen_num(rng: &mut Rng, out: &mut String) {
    out.push_str(rng.pick(&[
        "0",
        "-0",
        "1",
        "-1",
        "12",
        "1.5",
        "-0.25",
        "1e3",
        "1E-3",
        "2.5e+10",
        "9223372036854775807",
        "-9223372036854775808",
        "9223372036854775808",
        "1e999",
        "01",
        "1.",
        ".5",
        "+1",
        "-",
        "1e",
    ]));
}

fn gen_value(rng: &mut Rng, depth: usize, out: &mut String) {
    let ws = |rng: &mut Rng, out: &mut String| {
        if rng.chance(1, 4) {
            out.push_str(rng.pick(&[" ", "\n", "\t", "\r\n"]));
        }
    };
    ws(rng, out);
    match if depth >= 6 {
        rng.below(5)
    } else {
        rng.below(7)
    } {
        0 => out.push_str("null"),
        1 => out.push_str(rng.pick(&["true", "false", "tru", "nul"])),
        2 | 3 => gen_num(rng, out),
        4 => gen_str(rng, out),
        5 => {
            out.push('[');
            for i in 0..rng.below(4) {
                if i > 0 {
                    out.push(',');
                }
                gen_value(rng, depth + 1, out);
            }
            out.push(']');
        }
        _ => {
            out.push('{');
            for i in 0..rng.below(4) {
                if i > 0 {
                    out.push(',');
                }
                gen_str(rng, out);
                out.push(':');
                gen_value(rng, depth + 1, out);
            }
            out.push('}');
        }
    }
    ws(rng, out);
}

fn grammar(rng: &mut Rng) -> Vec<u8> {
    let mut s = String::new();
    if rng.chance(1, 40) {
        // 深い入れ子（上限の前後）。
        let n = json::MAX_DEPTH - 2 + rng.below(6);
        s.push_str(&"[".repeat(n));
        s.push_str(&"]".repeat(n));
    } else {
        gen_value(rng, 0, &mut s);
    }
    let mut b = s.into_bytes();
    // ときどき壊す。
    if rng.chance(1, 5) && !b.is_empty() {
        let i = rng.below(b.len());
        b.truncate(i);
    }
    b
}

#[test]
fn fuzz_json() {
    fuzz::run("json", 3000, fuzz::corpus("json", &[]), grammar, check);
}

#[test]
fn json_depth_limit_is_an_error() {
    let nest =
        |n: usize, open: &str, close: &str| format!("{}1{}", open.repeat(n), close.repeat(n));
    for (open, close) in [("[", "]"), ("{\"a\":", "}")] {
        // 上限の深さまでは読める。
        let ok = json::parse(&nest(json::MAX_DEPTH, open, close));
        assert!(matches!(ok, Ok(Value::Arr(_) | Value::Obj(_))), "{open}");
        // 超えたらエラー（スタックを使い切らない）。
        for n in [json::MAX_DEPTH + 1, 1000, 200_000] {
            assert!(json::parse(&nest(n, open, close)).is_err(), "{open} n={n}");
        }
    }
}

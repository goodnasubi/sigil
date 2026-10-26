//! 字句解析・構文解析・検査・整形のファジング（ADR 0020 の決定 2）。枠は `tests/common/fuzz.rs`。
//!
//! 性質: どの段も落ちない／スタックを使い切らない。構文解析が通れば、整形の出力も構文解析でき、
//! もう一度整形しても変わらない（整形は固定点）。

#[path = "common/fuzz.rs"]
mod fuzz;

use fuzz::Rng;

fn check(input: &[u8]) {
    let src = String::from_utf8_lossy(input);
    let _ = sigil::lexer::lex(&src);
    let parsed = sigil::parse(&src);
    let _ = sigil::check::check(&src);
    if parsed.is_ok() {
        match sigil::fmt::format(&src) {
            Ok(out) => {
                assert!(sigil::parse(&out).is_ok(), "整形の出力が構文解析できない");
                assert_eq!(
                    sigil::fmt::format(&out).as_deref(),
                    Ok(out.as_str()),
                    "整形が固定点でない"
                );
            }
            Err(e) => panic!("構文解析が通る入力の整形が失敗: {e}"),
        }
    }
}

fn ident(rng: &mut Rng) -> &'static str {
    rng.pick(&["x", "y", "z", "n", "acc", "xs"])
}

fn expr(rng: &mut Rng, d: usize, out: &mut String) {
    let leaf = d >= 4 || rng.chance(1, 3);
    if leaf {
        match rng.below(6) {
            0 => out.push_str(&rng.below(1000).to_string()),
            1 => out.push_str(rng.pick(&["true", "false", "1.5", "\"s\"", "\"あ\\n\""])),
            _ => out.push_str(ident(rng)),
        }
        return;
    }
    match rng.below(10) {
        0..=2 => {
            expr(rng, d + 1, out);
            out.push_str(rng.pick(&[" + ", " - ", " * ", " / ", " % "]));
            expr(rng, d + 1, out);
        }
        8 => {
            // 比較は連鎖できず、`&&` と `||` は混ぜられないので、葉どうしの比較を `&&` か `||` で 1 種類だけつなぐ。
            let cmp = |rng: &mut Rng, out: &mut String| {
                expr(rng, 4, out);
                out.push_str(rng.pick(&[" == ", " < ", " <= ", " != "]));
                expr(rng, 4, out);
            };
            cmp(rng, out);
            if rng.chance(1, 2) {
                out.push_str(rng.pick(&[" && ", " || "]));
                cmp(rng, out);
            }
        }
        3 => {
            out.push_str(rng.pick(&["-", "!", "- ", "&"]));
            expr(rng, d + 1, out);
        }
        4 => {
            out.push('(');
            expr(rng, d + 1, out);
            out.push(')');
        }
        5 => {
            out.push_str(ident(rng));
            out.push('(');
            for i in 0..rng.below(3) {
                if i > 0 {
                    out.push_str(", ");
                }
                expr(rng, d + 1, out);
            }
            out.push(')');
        }
        6 => {
            out.push_str(ident(rng));
            out.push('.');
            out.push_str(rng.pick(&["len", "get", "f"]));
            if rng.chance(1, 2) {
                out.push_str("()");
            }
            if rng.chance(1, 5) {
                out.push('?');
            }
        }
        7 if d == 0 => {
            out.push_str(rng.pick(&["|a| ", "move |a, b| "]));
            expr(rng, d + 1, out);
        }
        _ => expr(rng, 4, out),
    }
}

fn pad(out: &mut String, indent: usize) {
    out.push_str(&"  ".repeat(indent));
}

fn block(rng: &mut Rng, d: usize, indent: usize, out: &mut String) {
    for _ in 0..rng.below(4) {
        if rng.chance(1, 8) {
            pad(out, indent);
            out.push_str("// 注釈\n");
        }
        if rng.chance(1, 10) {
            out.push('\n');
        }
        pad(out, indent);
        match if d >= 3 { rng.below(5) } else { rng.below(9) } {
            0 => {
                out.push_str(rng.pick(&["let ", "let mut "]));
                out.push_str(ident(rng));
                out.push_str(" = ");
                expr(rng, 0, out);
                out.push(';');
            }
            1 => {
                out.push_str(ident(rng));
                out.push_str(rng.pick(&[" = ", ".f = "]));
                expr(rng, 0, out);
                out.push(';');
            }
            2 | 3 => {
                out.push_str("return ");
                expr(rng, 0, out);
                out.push(';');
            }
            4 => {
                out.push_str(ident(rng));
                out.push('(');
                expr(rng, 1, out);
                out.push_str(")?;");
                if rng.chance(1, 4) {
                    out.push_str(" // 行末");
                }
            }
            5 | 6 => {
                let is_if = rng.chance(2, 3);
                out.push_str(if is_if { "if " } else { "while " });
                expr(rng, 2, out);
                out.push_str(" {\n");
                block(rng, d + 1, indent + 1, out);
                pad(out, indent);
                out.push('}');
                if is_if && rng.chance(1, 3) {
                    out.push_str(" else ");
                    if rng.chance(1, 2) {
                        out.push_str("if ");
                        expr(rng, 2, out);
                        out.push(' ');
                    }
                    out.push_str("{\n");
                    block(rng, d + 1, indent + 1, out);
                    pad(out, indent);
                    out.push('}');
                }
            }
            _ => {
                out.push_str("match ");
                expr(rng, 2, out);
                out.push_str(" {\n");
                for pat in ["0", "1", "_"].iter().take(1 + rng.below(3)) {
                    pad(out, indent + 1);
                    out.push_str(pat);
                    out.push_str(" => {\n");
                    block(rng, d + 1, indent + 2, out);
                    pad(out, indent + 1);
                    out.push_str("}\n");
                }
                pad(out, indent);
                out.push('}');
            }
        }
        out.push('\n');
    }
}

fn grammar(rng: &mut Rng) -> Vec<u8> {
    let mut s = String::from("module m.f\n  targets server\n  trust none\n\n");
    if rng.chance(1, 4) {
        s.push_str("struct P {\n  a: Int,\n  b: List<Int>,\n}\n\n");
    }
    for k in 0..1 + rng.below(2) {
        s.push_str(&format!(
            "fn f{k}(x: Int, xs: &List<Int>) -> Int\n  intent \"f\"\n  requires {}\n  ensures none\n  effects none\n{{\n",
            rng.pick(&["none", "x > 0"])
        ));
        block(rng, 0, 1, &mut s);
        s.push_str("  return x;\n}\n\n");
    }
    let mut b = s.into_bytes();
    // ときどき壊す（行の削除・字句の差し替え）。
    if rng.chance(1, 4) && !b.is_empty() {
        let at = rng.below(b.len());
        match rng.below(3) {
            0 => b.truncate(at),
            1 => {
                b.remove(at);
            }
            _ => {
                let t: &[u8] = rng.pick(&[b"{".as_slice(), b"}", b")", b";", b"=>", b"\"", b"/*"]);
                b.splice(at..at, t.iter().copied());
            }
        }
    }
    b
}

#[test]
fn fuzz_parse() {
    let corpus = fuzz::corpus(
        "parse",
        &[
            "examples/01-average",
            "examples/02-read-port",
            "examples/04-effects",
        ],
    );
    fuzz::run("parse", 1500, corpus, grammar, check);
}

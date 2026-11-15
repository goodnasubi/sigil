//! ADR 0037 6-6b: Python 標準 hmac で独立計算した固定値を両実行系で照合。
mod common;

use sigil::exec::Arg;

const VECTORS: &str = include_str!("data/bearer_507.tsv");
const HEAD: &str = "module bearer507\n  targets server\n  trust none\n";
const CONTRACT: &str =
    "\n  intent \"純粋検証の固定ベクタ\"\n  requires none\n  ensures none\n  effects none\n";

fn bytes(name: &str, hex: &str) -> String {
    let mut out = format!("let mut {name} = Bytes.new();\n");
    for i in (0..hex.len()).step_by(2) {
        let b = u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        out.push_str(&format!("{name}.push({b});\n"));
    }
    out
}

fn case_body(cols: &[&str], expected: &str) -> String {
    format!(
        "{}let key = Secret.from_bytes(kb);\n{}\nmatch web.verify_bearer(&key, &\"{}\", &token, {}) {{\n Ok(identity) => {{ if identity.subject() == \"{}\" {{ return 0; }} return 1; }}\n Err(reason) => {{ if \"{}\" == \"ERR\" && reason == \"unauthorized\" {{ return 0; }} return 1; }}\n}}",
        bytes("kb", cols[1]), bytes("token", cols[6]), cols[7], cols[8], expected, expected,
    )
}

#[test]
fn fixed_public_vectors_eval_native() {
    let mut source = HEAD.to_string();
    let mut main = String::from("let mut bad = 0;\n");
    for (i, line) in VECTORS.lines().filter(|l| !l.starts_with('#')).enumerate() {
        let cols: Vec<_> = line.split('\t').collect();
        assert_eq!(cols.len(), 13);
        source.push_str(&format!(
            "fn case_{i}() -> Int{CONTRACT}{{\n{}\n}}\n",
            case_body(&cols, cols[9])
        ));
        main.push_str(&format!("if case_{i}() != 0 {{ return {}; }}\n", i + 1));
    }
    source.push_str(&format!(
        "fn main() -> Int{CONTRACT}{{\n{main}return bad;\n}}\n"
    ));
    let result = common::run(&source, "main", &[] as &[Arg]).unwrap();
    assert_eq!(result.value, "0", "失敗行は TSV のデータ行番号（1 起点）");
}

#[test]
fn claims_are_unreachable_until_signature_matches_eval_native() {
    // 本物の核の意味解釈入口だけを標識へ置換する。無効署名なら標識へ
    // 到達せず、正しい署名の不正 payload なら到達することを両実行系で観測。
    let web = include_str!("../src/stdlib/web.sigil");
    let start = web.find("fn bearer_claims(").unwrap();
    let signature = &web[start..web[start..].find('{').unwrap() + start + 1];
    let instrumented = format!(
        "{}{} return Ok(AuthIdentity.new(\"claims-reached\")); }}\n",
        &web[..start],
        signature
    );
    let mut source = HEAD.to_string();
    let mut main = String::new();
    for (i, name) in ["signed_short_payload", "wrong_tag_short_payload"]
        .iter()
        .enumerate()
    {
        let line = VECTORS
            .lines()
            .find(|l| l.starts_with(&format!("{name}\t")))
            .unwrap();
        let cols: Vec<_> = line.split('\t').collect();
        let want = if i == 0 { "claims-reached" } else { "ERR" };
        source.push_str(&format!(
            "fn case_{i}() -> Int{CONTRACT}{{ {} }}\n",
            case_body(&cols, want)
        ));
        main.push_str(&format!("if case_{i}() != 0 {{ return {}; }}\n", i + 1));
    }
    source.push_str(&format!("fn main() -> Int{CONTRACT}{{ {main} return 0; }}"));
    let options = sigil::exec::Options {
        embedded_extra: vec![("std.web".into(), instrumented.clone())],
        ..sigil::exec::Options::captured()
    };
    let result = sigil::ir_eval::run_with(&source, "main", &[], &options).unwrap();
    assert_eq!(result.value, "0");
    // common の native 比較は embedded_extra を build に渡さないため、
    // 同じ差替えを含む SIR を明示的に build_program へ渡す。
    let lower_options = sigil::effects::Options {
        embedded_extra: vec![("std.web".into(), instrumented)],
        ..Default::default()
    };
    let program = sigil::ir_eval::lower_sources_opts(&[&source], "main", &lower_options).unwrap();
    let dir = std::path::PathBuf::from(format!("target/bearer507-observe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let executable = dir.join("probe");
    sigil::native::build_program(&program, &Default::default(), &executable).unwrap();
    let native = std::process::Command::new(&executable).output().unwrap();
    std::fs::remove_dir_all(dir).unwrap();
    assert!(native.status.success());
    assert_eq!(
        String::from_utf8(native.stdout).unwrap(),
        format!("{}\n", result.value)
    );
}

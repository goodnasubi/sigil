//! ADR 0037 決定5の全上限。独立固定ベクタを、両実行系でまとめて検証する。
mod common;
const VECTORS: &str = include_str!("data/bearer_507.tsv");
const CONTRACT: &str = "intent \"上限の成功と失敗\" requires none ensures none effects none";
fn bytes(name: &str, hex: &str) -> String {
    let mut code = format!("let mut {name} = Bytes.new();\n");
    for i in (0..hex.len()).step_by(2) {
        code.push_str(&format!(
            "{name}.push({});\n",
            u8::from_str_radix(&hex[i..i + 2], 16).unwrap()
        ));
    }
    code
}
#[test]
fn limit_token_payload_audience_subject_key_lifetime_eval_native() {
    // 590バイトの正規最大値は成功、1024/1025は内側形式不正で拒否。
    let names = [
        "valid_max_payload_404_token_590",
        "token_1024",
        "token_1025",
        "payload_405",
        "long_audience_129",
        "long_subject_257",
        "key_31",
        "valid_iat_equals_now_lifetime_3600",
        "key_33",
        "lifetime_3601",
    ];
    let mut source = "module test.limits512\n targets server\n trust none\n".to_string();
    let mut main = String::new();
    for (i, name) in names.iter().enumerate() {
        let row = VECTORS
            .lines()
            .find(|r| r.starts_with(&format!("{name}\t")))
            .unwrap();
        let c: Vec<_> = row.split('\t').collect();
        source.push_str(&format!("fn case_{i}() -> Int {CONTRACT} {{\n{}{}let key = Secret.from_bytes(kb);\nmatch web.verify_bearer(&key, &\"{}\", &token, {}) {{ Ok(identity) => {{ if identity.subject() == \"{}\" {{ return 0; }} return 1; }} Err(e) => {{ if \"{}\" == \"ERR\" && e == \"unauthorized\" {{ return 0; }} return 1; }} }} }}\n", bytes("kb", c[1]), bytes("token",c[6]),c[7],c[8],c[9],c[9]));
        main.push_str(&format!("if case_{i}() != 0 {{ return {}; }}\n", i + 1));
    }
    source.push_str(&format!(
        "fn main() -> Int {CONTRACT} {{ {main} return 0; }}"
    ));
    assert_eq!(
        common::run(&source, "main", &[]).unwrap().value,
        "0",
        "失敗値はnamesの1起点番号"
    );
}
#[test]
fn limit_key_hex_31_32_33_and_invalid_32() {
    let source = r#"module test.keylimits512
 targets server
 trust none
fn probe() -> String
 intent "設定値を表示せず固定分類だけを返す"
 requires none ensures none effects env
{
 match web.load_bearer_config() {
  Ok(_) => { return "ok"; }
  Err(e) => { return e.name().as_bytes().concat(&":".as_bytes()).concat(&e.kind().as_bytes()).to_string_utf8().unwrap_or(""); }
 }
}
"#;
    let dir = common::native::scratch("bearer512-keylimits");
    let exe = dir.join("probe");
    sigil::native::build(&[source], "probe", &Default::default(), &exe).unwrap();
    for (key, want) in [
        ("ab".repeat(31), "SIGIL_AUTH_HMAC_KEY_HEX:invalid"),
        ("ab".repeat(32), "ok"),
        ("ab".repeat(33), "SIGIL_AUTH_HMAC_KEY_HEX:invalid"),
        ("g".repeat(64), "SIGIL_AUTH_HMAC_KEY_HEX:invalid"),
    ] {
        let opts = sigil::exec::Options {
            env_vars: Some(std::collections::BTreeMap::from([
                ("SIGIL_AUTH_HMAC_KEY_HEX".into(), key.clone()),
                ("SIGIL_AUTH_AUDIENCE".into(), "sigil.api".into()),
            ])),
            ..sigil::exec::Options::captured()
        };
        let out = sigil::ir_eval::run_with(source, "probe", &[], &opts).unwrap();
        assert_eq!(out.value, format!("{want:?}"));
        assert!(out.output.is_empty());
        let native = std::process::Command::new(&exe)
            .env_clear()
            .env("SIGIL_AUTH_HMAC_KEY_HEX", key)
            .env("SIGIL_AUTH_AUDIENCE", "sigil.api")
            .output()
            .unwrap();
        assert!(native.status.success());
        assert!(native.stderr.is_empty());
        assert_eq!(
            String::from_utf8(native.stdout).unwrap(),
            format!("{want:?}\n")
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

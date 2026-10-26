//! ADR 0019 の PR1: `sigil check --json`（診断の機械向けの形式）。

use sigil::json::{parse, Value};

fn sigil_bin(args: &[&str]) -> (i32, String, String) {
    // 根の既定は cwd（ADR 0021 2-2c）。試験の置き場所を根にする。
    let mut full: Vec<String> = vec![args[0].to_string(), "--root".into(), tmp_root()];
    full.extend(args[1..].iter().map(|a| a.to_string()));
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(full)
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn tmp_root() -> String {
    std::env::temp_dir()
        .join(format!("sigil-m6-check-json-{}", std::process::id()))
        .to_string_lossy()
        .into_owned()
}

/// `m/<名前>.sigil` に書く。`module m.a` / `module m.b` は、パスに合わせて `m.<名前>` にする。
fn temp_file(name: &str, src: &str) -> String {
    let stem = name.trim_end_matches(".sigil");
    let src = src
        .replacen("module m.a\n", &format!("module m.x_{stem}\n"), 1)
        .replacen("module m.b\n", &format!("module m.x_{stem}\n"), 1);
    let dir = std::path::PathBuf::from(tmp_root()).join("m");
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("x_{name}"));
    std::fs::write(&p, src).unwrap();
    p.to_string_lossy().into_owned()
}

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

fn func(body: &str) -> String {
    format!(
        "{HEAD}fn f(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n"
    )
}

#[test]
fn check_json_kinds() {
    let cases: Vec<(&str, String, &str, &str, i64, i64)> = vec![
        // 字句: 本体の中のタブ
        ("lex.sigil", func("\treturn n;"), "Tab", "lex", 11, 1),
        // 構文: `;` がない
        ("syntax.sigil", func("  return n"), "Unexpected", "syntax", 12, 1),
        // 型: 戻り値の型が違う
        ("type.sigil", func("  return true;"), "TypeMismatch", "check", 11, 3),
        // 移動: 移動の後の使用
        (
            "move.sigil",
            format!(
                "{HEAD}struct S\n{{\n  a: Int,\n}}\n\nfn take(s: S)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return;\n}}\n\nfn f(s: S) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  take(s);\n  return s.a;\n}}\n"
            ),
            "UseAfterMove",
            "check",
            26,
            3,
        ),
    ];
    let mut paths = vec![];
    for (name, src, kind, phase, line, col) in &cases {
        let p = temp_file(name, src);
        let (code, out, err) = sigil_bin(&["check", "--json", &p]);
        let (code0, out0, err0) = sigil_bin(&["check", &p]);
        // 終了コードは `--json` のないときと同じ。
        assert_eq!(code, code0, "{name}");
        assert_eq!(code, 1, "{name}");
        assert_eq!(err, "", "{name}");
        // `--json` のないときの標準エラーの文が、`message` と位置に出ている。
        assert_eq!(out0, "", "{name}");
        let v = parse(&out).unwrap_or_else(|e| panic!("{name}: {e}\n{out}"));
        assert_eq!(v.get("format").unwrap().as_i64(), Some(1));
        assert_eq!(v.get("ok").unwrap().as_bool(), Some(false));
        let ds = v.get("diagnostics").unwrap().as_array().unwrap();
        assert_eq!(ds.len(), 1, "{name}");
        let d = &ds[0];
        assert_eq!(d.get("file").unwrap().as_str(), Some(p.as_str()));
        assert_eq!(
            d.get("kind").unwrap().as_str(),
            Some(*kind),
            "{name}: {out}"
        );
        assert_eq!(d.get("phase").unwrap().as_str(), Some(*phase), "{name}");
        assert_eq!(
            d.get("line").unwrap().as_i64(),
            Some(*line),
            "{name}: {out}"
        );
        assert_eq!(d.get("col").unwrap().as_i64(), Some(*col), "{name}: {out}");
        let msg = d.get("message").unwrap().as_str().unwrap();
        assert!(!msg.is_empty());
        assert!(err0.contains(msg), "{name}: {err0}");
        assert!(
            err0.starts_with(&format!("{p}:{line}:{col}: ")),
            "{name}: {err0}"
        );
        paths.push(p);
    }
    // 成功: 終了コード 0、`ok` が真で診断は空。標準出力は JSON だけ。
    let ok = temp_file("ok.sigil", &func("  return n;"));
    let (code, out, err) = sigil_bin(&["check", "--json", &ok]);
    assert_eq!((code, err.as_str()), (0, ""));
    assert_eq!(out, "{\"format\":1,\"ok\":true,\"diagnostics\":[]}\n");
    // 複数のファイル: 誤りのあるファイルだけが診断になり、終了コードは 1。
    let (code, out, _) = sigil_bin(&["check", "--json", &ok, &paths[2], &paths[0]]);
    assert_eq!(code, 1);
    let v = parse(&out).unwrap();
    let ds = v.get("diagnostics").unwrap().as_array().unwrap();
    let kinds: Vec<&str> = ds
        .iter()
        .map(|d| d.get("kind").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["TypeMismatch", "Tab"]);
    // `--trust-base` と並べても読める（順序は問わない）。
    let tb = temp_file(
        "tb.sigil",
        "module m.b\n  targets server\n  trust base\n\nfn g(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n",
    );
    let (code, out, _) = sigil_bin(&["check", "--json", &tb]);
    assert_eq!(code, 1);
    assert_eq!(
        parse(&out)
            .unwrap()
            .get("diagnostics")
            .unwrap()
            .as_array()
            .unwrap()[0]
            .get("kind"),
        Some(&Value::from("TrustBaseNotAllowed"))
    );
    for args in [
        ["check", "--json", "--trust-base", "m.x_tb", tb.as_str()],
        ["check", "--trust-base", "m.x_tb", "--json", tb.as_str()],
    ] {
        let (code, out, _) = sigil_bin(&args);
        assert_eq!(code, 0, "{args:?}");
        assert_eq!(out, "{\"format\":1,\"ok\":true,\"diagnostics\":[]}\n");
    }
    // 読めないファイルは ReadError の診断（`check_json_read_error` が確かめる）。
}

#[test]
fn check_json_message_with_escapes() {
    // 診断の文に引用符や改行があっても、JSON として読める。
    let p = temp_file("esc.sigil", "module m.a\n  targets server\n  trust none\n\nfn f(n: Int) -> Int\n  intent \"a\"\n  requires none\n  ensures none\n  effects none\n{\n  let s = \"\\q\";\n  return n;\n}\n");
    let (code, out, _) = sigil_bin(&["check", "--json", &p]);
    assert_eq!(code, 1);
    let v = parse(&out).unwrap();
    let d = &v.get("diagnostics").unwrap().as_array().unwrap()[0];
    assert_eq!(d.get("kind").unwrap().as_str(), Some("BadEscape"));
    assert_eq!(d.get("phase").unwrap().as_str(), Some("lex"));
}

#[test]
fn check_json_read_error() {
    let ok = temp_file("read_ok.sigil", &func("  return n;"));
    let bad = temp_file("read_bad.sigil", "");
    std::fs::write(&bad, [0xff, 0xfe, 0x00]).unwrap(); // UTF-8 でない
    let missing = format!("{ok}.missing");
    let (code, out, err) = sigil_bin(&["check", "--json", &missing, &ok, &bad]);
    assert_eq!(code, 1);
    assert_eq!(err, "", "--json では標準エラーに出さない");
    let v = parse(out.trim()).unwrap();
    assert_eq!(v.get("ok"), Some(&Value::Bool(false)));
    let d = v.get("diagnostics").and_then(Value::as_array).unwrap();
    assert_eq!(d.len(), 2);
    for (x, file) in d.iter().zip([&missing, &bad]) {
        assert_eq!(x.get("file").and_then(Value::as_str), Some(file.as_str()));
        assert_eq!(x.get("kind").and_then(Value::as_str), Some("ReadError"));
        assert_eq!(x.get("phase").and_then(Value::as_str), Some("read"));
        assert_eq!(x.get("line"), Some(&Value::Null));
        assert_eq!(x.get("col"), Some(&Value::Null));
        assert!(x
            .get("message")
            .and_then(Value::as_str)
            .unwrap()
            .starts_with("読めない: "));
    }
    // 人間向けの出力は従来どおり標準エラー
    let (code, out, err) = sigil_bin(&["check", &missing]);
    assert_eq!((code, out.as_str()), (1, ""));
    assert!(err.contains("読めない"));
}

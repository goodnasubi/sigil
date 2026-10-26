//! ADR 0019 の PR1: `sigil spec --json`（仕様の機械向けの形式）。

mod common;
use sigil::json::{parse, Value};
use sigil::spec::{extract, extract_with, spec, spec_with};

const CASES: &[&str] = &[
    "examples/01-average/c-brace",
    "examples/02-read-port/c-brace",
    "examples/03-ownership/ownership",
    "examples/04-effects/effects",
    "examples/05-control/control",
];

fn sigil_bin(args: &[&str]) -> (i32, String, String) {
    sigil_bin_in(None, args)
}

/// 根を明示して走らせる（既定は cwd。ADR 0021 2-2c）。
fn sigil_bin_in(root: Option<&std::path::Path>, args: &[&str]) -> (i32, String, String) {
    let mut full: Vec<String> = vec![args[0].to_string()];
    if let Some(r) = root {
        full.push("--root".into());
        full.push(r.to_string_lossy().into_owned());
    }
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

/// 連結の規則: `module` の `text` と各 `items` の `text` を、改行 1 つを挟んでつなぐ。
fn joined_text(v: &Value) -> String {
    let mut blocks = vec![v
        .get("module")
        .unwrap()
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()];
    for it in v.get("items").unwrap().as_array().unwrap() {
        blocks.push(it.get("text").unwrap().as_str().unwrap());
    }
    blocks.join("\n")
}

#[test]
fn spec_json_text_matches_spec() {
    for c in CASES {
        let path = format!("{c}.sigil");
        let want = std::fs::read_to_string(format!("{c}.spec")).unwrap();
        // CLI の `--json` の出力から取り出した `text` をつないだものが、`sigil spec` の出力と一致する。
        let (root, staged) = common::stage_file("spec", &path);
        let staged = staged.to_string_lossy().into_owned();
        let (code, plain, _) = sigil_bin_in(Some(&root), &["spec", &staged]);
        assert_eq!(code, 0, "{c}");
        assert_eq!(plain, want, "{c}");
        let (code, json, err) = sigil_bin_in(Some(&root), &["spec", "--json", &staged]);
        assert_eq!(code, 0, "{c}: {err}");
        assert!(
            json.ends_with('\n') && !json[..json.len() - 1].contains('\n'),
            "{c}"
        );
        let v = parse(&json).unwrap_or_else(|e| panic!("{c}: {e}"));
        assert_eq!(v.get("format").unwrap().as_i64(), Some(1));
        assert_eq!(joined_text(&v), want, "{c}");
        // 各項目の `text` は改行 1 つで終わり、空行で終わらない。
        for it in v.get("items").unwrap().as_array().unwrap() {
            let t = it.get("text").unwrap().as_str().unwrap();
            assert!(t.ends_with('\n') && !t.ends_with("\n\n"), "{c}: {t:?}");
        }
        // ライブラリの値も同じ。
        let src = std::fs::read_to_string(&path).unwrap();
        assert_eq!(extract(&src).unwrap().text(), want, "{c}");
        assert_eq!(extract(&src).unwrap().to_json(), v, "{c}");
    }
}

#[test]
fn spec_json_text_matches_spec_all_kinds() {
    // 型・構造体・列挙型・総称・メソッド・複数行の条件・エスケープする意図を含む。
    let src = "module m.a\n  targets browser, server\n  trust none\n\ntype Port = Int where self >= 1 && self <= 65535;\n\nstruct Pair<T: Equatable + Sendable>\n{\n  a: T,\n  b: T,\n}\n\nenum Shape\n{\n  Dot,\n  Circle(Int),\n  Rect(Int, Int),\n}\n\nfn id<T>(x: T) -> T\n  intent \"恒等\\n\"\n  requires none\n  ensures result == x\n  effects none\n{\n  return x;\n}\n\nimpl Pair<Int>\n{\n  fn first(p: &Pair<Int>) -> Int\n    intent \"最初\"\n    requires none\n    ensures none\n    effects none\n  {\n    return p.a;\n  }\n\n  fn swap(p: &mut Pair<Int>)\n    intent \"入れ替える\"\n    requires none\n    ensures none\n    effects none\n  {\n    return;\n  }\n}\n";
    let s = extract(src).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.text(), spec(src).unwrap());
    let v = s.to_json();
    assert_eq!(joined_text(&v), spec(src).unwrap());
    let kinds: Vec<&str> = v
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.get("kind").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["type", "struct", "enum", "fn", "method", "method"]);
    let names: Vec<&str> = v
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i.get("name").unwrap().as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Port",
            "Pair",
            "Shape",
            "id",
            "Pair<Int>.first",
            "Pair<Int>.swap"
        ]
    );
    let items = v.get("items").unwrap().as_array().unwrap();
    // 型・構造体・列挙型の欄
    assert_eq!(items[0].get("base").unwrap().as_str(), Some("Int"));
    assert_eq!(
        items[0].get("cond").unwrap().as_str(),
        Some("self >= 1 && self <= 65535")
    );
    assert_eq!(items[1].get("fields").unwrap().as_array().unwrap().len(), 2);
    let g = &items[1].get("generics").unwrap().as_array().unwrap()[0];
    assert_eq!(g.get("name").unwrap().as_str(), Some("T"));
    assert_eq!(
        g.get("bounds").unwrap().to_json(),
        "[\"Equatable\",\"Sendable\"]"
    );
    let vars = items[2].get("variants").unwrap().as_array().unwrap();
    assert_eq!(
        vars[2].get("fields").unwrap().to_json(),
        "[\"Int\",\"Int\"]"
    );
    // 関数の欄: 意図は書かれたまま（エスケープしない）、`text` はエスケープする
    assert_eq!(items[3].get("intent").unwrap().as_str(), Some("恒等\n"));
    assert_eq!(
        items[3].get("ensures").unwrap().as_str(),
        Some("result == x")
    );
    assert_eq!(items[3].get("requires"), Some(&Value::Null));
    // メソッド
    assert_eq!(items[5].get("returns"), Some(&Value::Null));
    assert_eq!(
        items[5].get("params").unwrap().as_array().unwrap()[0]
            .get("mode")
            .unwrap()
            .as_str(),
        Some("mut")
    );
    // モジュール
    let m = v.get("module").unwrap();
    assert_eq!(m.get("kind").unwrap().as_str(), Some("module"));
    assert_eq!(m.get("name").unwrap().as_str(), Some("m.a"));
    assert_eq!(
        m.get("targets").unwrap().to_json(),
        "[\"browser\",\"server\"]"
    );
    assert_eq!(m.get("trust_base").unwrap().as_bool(), Some(false));
}

#[test]
fn spec_json_fields() {
    let src = std::fs::read_to_string("examples/03-ownership/ownership.sigil").unwrap();
    let v = extract(&src).unwrap().to_json();
    let deposit = v
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i.get("name").unwrap().as_str() == Some("deposit"))
        .unwrap();
    // ADR 0019 決定 1 の例（`text` は別に確かめる）
    let mut want = parse(
        r#"{"kind": "fn", "name": "deposit", "span": {"line": 26, "col": 1},
 "intent": "口座に入金する",
 "params": [{"name": "account", "mode": "mut", "type": "Account"},
            {"name": "amount", "mode": "own", "type": "Int"}],
 "returns": null, "requires": "amount > 0",
 "ensures": "account.balance == old(account.balance) + amount",
 "effects": [], "generics": [], "unchecked": null,
 "raw_ops": [], "uninferred_effects": []}"#,
    )
    .unwrap();
    let Value::Obj(fs) = &mut want else { panic!() };
    // 欄の追加は `format` を上げない（unchecked の関数でないので、解決できたかの欄は偽）。
    fs.push(("unchecked_unresolved".into(), false.into()));
    fs.push(("visibility".into(), "private".into()));
    fs.push(("text".into(), deposit.get("text").unwrap().clone()));
    let Value::Obj(mut got) = deposit.clone() else {
        panic!()
    };
    let mut w = fs.clone();
    got.sort_by(|a, b| a.0.cmp(&b.0));
    w.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(got, w);
    assert!(deposit
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()
        .starts_with("関数 deposit（非公開）\n  意図: 口座に入金する\n"));
}

fn raw_opts() -> sigil::effects::Options {
    use sigil::effects::RawOp;
    use sigil::stdlib::{Layer, DELEGATED};
    let op = |layer, sig: &str, effects: &str, premise: &str| RawOp {
        layer,
        signature: sig.to_string(),
        effects: effects.to_string(),
        requires: "none".to_string(),
        premise: premise.to_string(),
    };
    sigil::effects::Options {
        raw_ops: vec![
            op(
                Layer::Core,
                "alloc(size: Int) -> Addr",
                "state",
                "確保器が有効",
            ),
            op(
                Layer::Core,
                "read_int(a: &Addr) -> Int",
                "none",
                "領域が生きている",
            ),
            op(Layer::Os, "syscall(n: Int) -> Int", DELEGATED, "番号が有効"),
        ],
        ..Default::default()
    }
}

fn ufn(name: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {name}(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n  unchecked \"領域は確保済み\"\n{{\n{body}\n}}\n\n"
    )
}

fn fn_json(src: &str, o: &sigil::effects::Options) -> Value {
    let s = extract_with(src, o).unwrap();
    assert_eq!(s.text(), spec_with(src, o).unwrap());
    let v = s.to_json();
    assert_eq!(joined_text(&v), spec_with(src, o).unwrap());
    v.get("items").unwrap().as_array().unwrap()[0].clone()
}

#[test]
fn spec_json_unchecked_fields() {
    let o = raw_opts();
    let head = "module m.a\n  targets server\n  trust base\n\n";
    // 原始操作を使う（名前の順・重複なし）
    let body = "  let a = raw.alloc(n);\n  let b = raw.alloc(n);\n  return raw.read_int(&a) + raw.read_int(&b);";
    let f = fn_json(&format!("{head}{}", ufn("f", "state", body)), &o);
    assert_eq!(f.get("unchecked").unwrap().as_str(), Some("領域は確保済み"));
    assert_eq!(
        f.get("raw_ops").unwrap().to_json(),
        r#"[{"name":"raw.alloc","premise":"確保器が有効"},{"name":"raw.read_int","premise":"領域が生きている"}]"#
    );
    assert_eq!(f.get("uninferred_effects").unwrap().to_json(), "[]");
    assert_eq!(
        f.get("unchecked_unresolved").unwrap().as_bool(),
        Some(false)
    );
    let m = extract_with(&format!("{head}{}", ufn("f", "state", body)), &o)
        .unwrap()
        .to_json();
    assert_eq!(
        m.get("module")
            .unwrap()
            .get("trust_base")
            .unwrap()
            .as_bool(),
        Some(true)
    );
    assert_eq!(
        m.get("module")
            .unwrap()
            .get("unchecked_fns")
            .unwrap()
            .as_i64(),
        Some(1)
    );
    // 宣言に委ねる操作: 推論できない効果
    let f = fn_json(
        &format!(
            "{head}{}",
            ufn("f", "fs.read, fs.write", "  return raw.syscall(n);")
        ),
        &o,
    );
    assert_eq!(
        f.get("uninferred_effects").unwrap().to_json(),
        r#"["fs.read","fs.write"]"#
    );
    assert_eq!(f.get("effects").unwrap().to_json(), r#"["fs"]"#);
    // 検査が失敗: 空の配列と区別する欄が真
    let bad = format!("{head}{}", ufn("f", "none", "  return nope;"));
    let f = fn_json(&bad, &o);
    assert_eq!(f.get("raw_ops").unwrap().to_json(), "[]");
    assert_eq!(f.get("unchecked_unresolved").unwrap().as_bool(), Some(true));
    assert!(f
        .get("text")
        .unwrap()
        .as_str()
        .unwrap()
        .contains("（検査のエラーのため表示できない）"));
}

#[test]
fn spec_json_error_exit() {
    let p = std::env::temp_dir().join(format!("sigil-m6-spec-json-{}.sigil", std::process::id()));
    std::fs::write(&p, "module").unwrap();
    let path = p.to_string_lossy().into_owned();
    let (c1, o1, e1) = sigil_bin(&["spec", &path]);
    let (c2, o2, e2) = sigil_bin(&["spec", "--json", &path]);
    assert_eq!((c1, c2), (1, 1));
    assert_eq!((o1.as_str(), o2.as_str()), ("", ""));
    assert_eq!(e1, e2);
    let _ = std::fs::remove_file(&p);
}

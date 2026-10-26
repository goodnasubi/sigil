//! ADR 0021 の PR 2-2a: `use`・`pub` の構文、`--root` とパス→モジュール名の対応、
//! `sigil spec` の `use` 一覧と公開の別。期待値は決め打ち（エラー種別と位置）。

use sigil::{check, fmt, parse, ErrorKind};
use std::path::{Path, PathBuf};

const FN_TAIL: &str =
    "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n";

fn module(name: &str, uses: &str, items: &str) -> String {
    format!("module {name}\n  targets server\n  trust none\n{uses}\n{items}")
}

fn pub_fn(name: &str) -> String {
    format!("pub fn {name}(n: Int) -> Int\n{FN_TAIL}")
}

fn err_at(src: &str) -> (ErrorKind, u32, u32) {
    match parse(src) {
        Ok(_) => panic!("エラーになるはずが成功した:\n{src}"),
        Err(e) => (e.kind, e.span.line, e.span.col),
    }
}

// ---- 構文 ----

#[test]
fn use_and_pub_parse() {
    let src = module(
        "app.http.server",
        "\nuse app.http.client.send;\nuse app.util.Clamp;\n",
        &format!(
            "{}\nstruct S {{\n  a: Int,\n}}\n\npub struct T {{\n  a: Int,\n}}\n\nimpl S {{\n  pub fn m(self: &S) -> Int\n{FN_TAIL}\n  fn p(self: &S) -> Int\n{FN_TAIL}}}\n",
            pub_fn("handle")
        ),
    );
    let f = parse(&src).unwrap();
    let uses: Vec<String> = f.uses.iter().map(|u| u.path.join(".")).collect();
    assert_eq!(uses, ["app.http.client.send", "app.util.Clamp"]);
    assert_eq!(f.uses[0].span.line, 5);
}

#[test]
fn use_errors_with_positions() {
    // 重複: 2 つ目の `use`（6 行目）
    let dup = module("a.b", "\nuse x.y.f;\nuse x.y.f;\n", "");
    assert_eq!(err_at(&dup), (ErrorKind::UseDuplicate, 6, 1));
    // 順の違反: `x.y.a` は `x.y.f` より前
    let order = module("a.b", "\nuse x.y.f;\nuse x.y.a;\n", "");
    assert_eq!(err_at(&order), (ErrorKind::UseUnsorted, 6, 1));
    // `module` の宣言の後（宣言の後）の `use`
    let late = module("a.b", "", &format!("{}\nuse x.y.f;\n", pub_fn("g")));
    assert_eq!(err_at(&late).0, ErrorKind::UseMisplaced);
    // 名前が 1 つだけ（モジュール名がない）
    let one = module("a.b", "\nuse f;\n", "");
    assert_eq!(err_at(&one), (ErrorKind::UseForm, 5, 1));
    // まとめ書き・ワイルドカード・別名は持たない
    for bad in ["use x.y.{f, g};", "use x.y.*;", "use x.y.f as g;"] {
        let src = module("a.b", &format!("\n{bad}\n"), "");
        assert!(parse(&src).is_err(), "{bad}");
    }
}

#[test]
fn pub_on_impl_is_rejected() {
    let src = module("a.b", "", "pub impl S {\n}\n");
    assert_eq!(err_at(&src), (ErrorKind::PubTarget, 5, 1));
}

#[test]
fn use_pub_are_no_longer_future_reserved() {
    // `use`・`pub` は予約語から構文の語になった。識別子としては引き続き使えない。
    let src = module("a.b", "", &format!("fn pub(n: Int) -> Int\n{FN_TAIL}"));
    assert_eq!(err_at(&src).0, ErrorKind::ReservedWordAsName);
}

// ---- 整形 ----

#[test]
fn fmt_use_pub_canonical() {
    let canonical = module(
        "app.http.server",
        "\nuse app.http.client.send;\nuse app.util.clamp;\n",
        &format!(
            "// 公開\n{}\nstruct S {{\n  a: Int,\n}}\n",
            pub_fn("handle")
        ),
    );
    assert_eq!(fmt::check(&canonical).unwrap(), None);
    assert_eq!(fmt::format(&canonical).unwrap(), canonical);
    // 空行の違いは正規形に直る（`use` は互いに空行を置かない）。
    let uneven = canonical.replace(
        "use app.http.client.send;\n",
        "use app.http.client.send;\n\n",
    );
    assert_eq!(fmt::format(&uneven).unwrap(), canonical);
}

#[test]
fn fmt_is_idempotent_on_pub_methods() {
    let src = module(
        "a.b",
        "",
        &format!(
            "pub struct Account {{\n  a: Int,\n}}\n\nimpl Account {{\n  pub fn m(self: &Account) -> Int\n{}}}\n",
            FN_TAIL.replace("n;", "self.a;")
        ),
    );
    let once = fmt::format(&src).unwrap();
    assert_eq!(fmt::format(&once).unwrap(), once);
    assert!(once.contains("  pub fn m("), "{once}");
}

// ---- spec ----

#[test]
fn spec_shows_uses_and_visibility() {
    let src = module(
        "app.http.server",
        "\nuse app.http.client.send;\nuse app.util.clamp;\n",
        &format!("{}\nfn helper(n: Int) -> Int\n{FN_TAIL}", pub_fn("handle")),
    );
    let text = sigil::spec::spec(&src).unwrap();
    assert!(
        text.starts_with("モジュール app.http.server\n  出力先: サーバー\n  信頼基盤: いいえ\n  使う名前: app.http.client.send、app.util.clamp\n"),
        "{text}"
    );
    assert!(text.contains("関数 handle（公開）\n"), "{text}");
    assert!(text.contains("関数 helper（非公開）\n"), "{text}");
    // `use` がないモジュールには `使う名前` の行を足さない。
    let plain = sigil::spec::spec(&module("a.b", "", &pub_fn("f"))).unwrap();
    assert!(!plain.contains("使う名前"), "{plain}");

    let v = sigil::spec::extract(&src).unwrap().to_json();
    assert_eq!(v.get("format").unwrap().as_i64(), Some(1));
    let m = v.get("module").unwrap();
    let uses: Vec<&str> = m
        .get("uses")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap())
        .collect();
    assert_eq!(uses, ["app.http.client.send", "app.util.clamp"]);
    assert!(m.get("visibility").is_none());
    let vis: Vec<(&str, &str)> = v
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i.get("name").unwrap().as_str().unwrap(),
                i.get("visibility").unwrap().as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(vis, [("handle", "pub"), ("helper", "private")]);
}

#[test]
fn spec_visibility_of_each_kind() {
    let src = module(
        "a.b",
        "",
        &format!(
            "pub type Port = Int where self > 0;\n\ntype Q = Int where self > 0;\n\npub enum E {{\n  A,\n}}\n\nenum F {{\n  B,\n}}\n\npub struct S {{\n  a: Int,\n}}\n\nimpl S {{\n  pub fn m(self: &S) -> Int\n{}\n  fn p(self: &S) -> Int\n{}}}\n",
            FN_TAIL.replace("n;", "self.a;"),
            FN_TAIL.replace("n;", "self.a;"),
        ),
    );
    let v = sigil::spec::extract(&src).unwrap().to_json();
    let got: Vec<(String, String)> = v
        .get("items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i.get("name").unwrap().as_str().unwrap().to_string(),
                i.get("visibility").unwrap().as_str().unwrap().to_string(),
            )
        })
        .collect();
    let want: Vec<(String, String)> = [
        ("Port", "pub"),
        ("Q", "private"),
        ("E", "pub"),
        ("F", "private"),
        ("S", "pub"),
        ("S.m", "pub"),
        ("S.p", "private"),
    ]
    .iter()
    .map(|(a, b)| (a.to_string(), b.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn check_accepts_pub_and_rejects_use_without_root() {
    // `pub` は `--root` なしでも通る。`use` の先は根がないと決まらないので、2-2b では明示的に拒否する。
    check::check(&module("a.b", "", &pub_fn("g"))).unwrap();
    let e = check::check(&module("a.b", "\nuse x.y.f;\n", &pub_fn("g"))).unwrap_err();
    assert_eq!((e.kind, e.span.line), (ErrorKind::Unsupported, 5));
}

// ---- パス→モジュール名（--root） ----

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("sigil-2a-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Tmp(d)
    }
    fn write(&self, rel: &str, src: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, src).unwrap();
        p
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn verify(root: &Path, file: &Path) -> Result<(), (ErrorKind, u32)> {
    let src = std::fs::read_to_string(file).unwrap();
    sigil::modpath::verify(&src, root, file).map_err(|e| (e.kind, e.span.line))
}

#[test]
fn path_to_module_name_matches() {
    let t = Tmp::new("ok");
    let f = t.write(
        "app/http/server.sigil",
        &module("app.http.server", "", &pub_fn("f")),
    );
    assert_eq!(verify(&t.0, &f), Ok(()));
    // 根は末尾の `/`・`.` を含んでもよく、引数のパスの `./` と `..` は根の下に畳まれる
    let root2 = t.0.join("app/..");
    assert_eq!(verify(&root2, &t.0.join("app/./http/server.sigil")), Ok(()));
    assert_eq!(
        sigil::modpath::module_name_for(&t.0, &f).unwrap(),
        ["app", "http", "server"]
    );
}

#[test]
fn module_path_mismatch_is_reported_at_module_decl() {
    let t = Tmp::new("mismatch");
    // 宣言は app.http.client だがパスは app/http/server.sigil。位置は `module` の行（1 行目）。
    let f = t.write(
        "app/http/server.sigil",
        &module("app.http.client", "", &pub_fn("f")),
    );
    assert_eq!(verify(&t.0, &f), Err((ErrorKind::ModulePathMismatch, 1)));
    // コメントの後なら、宣言の行（3 行目）
    let g = t.write(
        "a.sigil",
        &format!("// 1\n// 2\n{}", module("b", "", &pub_fn("f"))),
    );
    assert_eq!(verify(&t.0, &g), Err((ErrorKind::ModulePathMismatch, 3)));
}

#[test]
fn module_path_unusable_names_and_outside_root() {
    let t = Tmp::new("bad");
    // 拡張子が `.sigil` でない
    let f = t.write("a.txt", &module("a", "", &pub_fn("f")));
    assert_eq!(verify(&t.0, &f), Err((ErrorKind::ModulePathMismatch, 1)));
    // 識別子にならない部分（大文字・`-`・語頭が数字・予約語）
    for name in ["Foo.sigil", "my-mod.sigil", "1a.sigil", "fn.sigil"] {
        let f = t.write(name, &module("a", "", &pub_fn("f")));
        assert_eq!(
            verify(&t.0, &f),
            Err((ErrorKind::ModulePathMismatch, 1)),
            "{name}"
        );
    }
    // 根の外（`..`）
    let outside = Tmp::new("outside");
    let o = outside.write("x.sigil", &module("x", "", &pub_fn("f")));
    assert_eq!(verify(&t.0, &o), Err((ErrorKind::ModulePathMismatch, 1)));
    let via_dotdot = t.0.join("../").join(
        o.strip_prefix(o.parent().unwrap().parent().unwrap())
            .unwrap(),
    );
    assert_eq!(
        verify(&t.0, &via_dotdot),
        Err((ErrorKind::ModulePathMismatch, 1))
    );
}

#[cfg(unix)]
#[test]
fn symlink_named_by_apparent_location_and_must_stay_inside_root() {
    let t = Tmp::new("sym");
    let real = t.write("impl/real.sigil", &module("alias", "", &pub_fn("f")));
    // 根の中を指す symlink は、見かけの位置（alias.sigil）で名前が決まる。
    let link = t.0.join("alias.sigil");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(verify(&t.0, &link), Ok(()));
    // 根の外を指す symlink は拒否する。
    let outside = Tmp::new("sym-out");
    let target = outside.write("t.sigil", &module("esc", "", &pub_fn("f")));
    let esc = t.0.join("esc.sigil");
    std::os::unix::fs::symlink(&target, &esc).unwrap();
    assert_eq!(verify(&t.0, &esc), Err((ErrorKind::ModulePathMismatch, 1)));
}

#[test]
fn case_only_difference_is_an_error() {
    let t = Tmp::new("case");
    let a = t.write("util.sigil", &module("util", "", &pub_fn("f")));
    // 大小だけが違うファイル（大小を区別しないファイルシステムでは同じ場所になる）
    if std::fs::write(t.0.join("Util.sigil"), module("util", "", &pub_fn("g"))).is_ok()
        && std::fs::read_dir(&t.0).unwrap().count() == 2
    {
        assert_eq!(verify(&t.0, &a), Err((ErrorKind::ModulePathCase, 1)));
    } else {
        // 大小を区別しないファイルシステムでは 2 つ目が同じファイルになる（この環境では検証できない）。
        eprintln!("skip: case-insensitive filesystem");
    }
}

#[test]
fn reserved_std_names_rejected_by_verify_too() {
    let t = Tmp::new("std");
    // 宣言とパスが一致していても std.x は ReservedModuleName（verify 側で拒否。spec --root もここを通る）
    let f = t.write("std/x.sigil", &module("std.x", "", &pub_fn("f")));
    assert_eq!(verify(&t.0, &f), Err((ErrorKind::ReservedModuleName, 1)));
    let src = std::fs::read_to_string(&f).unwrap();
    let e = check::check(&src).unwrap_err();
    assert_eq!((e.kind, e.span.line), (ErrorKind::ReservedModuleName, 1));
    // パスと食い違う std.x も ModulePathMismatch ではなく ReservedModuleName
    let g = t.write("app/x.sigil", &module("std.x", "", &pub_fn("f")));
    assert_eq!(verify(&t.0, &g), Err((ErrorKind::ReservedModuleName, 1)));
}

#[cfg(unix)]
#[test]
fn intermediate_directory_symlink_escaping_root_is_rejected() {
    // root/app -> 根の外のディレクトリ。見かけの名前は app.x で宣言と一致するが、実体は根の外。
    let t = Tmp::new("dirsym");
    let outside = Tmp::new("dirsym-out");
    outside.write("x.sigil", &module("app.x", "", &pub_fn("f")));
    std::os::unix::fs::symlink(&outside.0, t.0.join("app")).unwrap();
    assert_eq!(
        verify(&t.0, &t.0.join("app/x.sigil")),
        Err((ErrorKind::ModulePathMismatch, 1))
    );
}

#[cfg(unix)]
#[test]
fn dotdot_through_symlink_is_rejected() {
    // d/link -> 根の外の other/sub。`d/link/../top.sigil` は字面では d/top.sigil だが、
    // OS は link の実体の親（other/top.sigil）を開く。
    let t = Tmp::new("dotdot");
    t.write("d/top.sigil", &module("d.top", "", &pub_fn("f")));
    let outside = Tmp::new("dotdot-out");
    std::fs::create_dir_all(outside.0.join("sub")).unwrap();
    outside.write("top.sigil", &module("d.top", "", &pub_fn("f")));
    std::os::unix::fs::symlink(outside.0.join("sub"), t.0.join("d/link")).unwrap();
    let via = t.0.join("d/link/../top.sigil");
    assert_eq!(verify(&t.0, &via), Err((ErrorKind::ModulePathMismatch, 1)));
    // 正規の d/top.sigil は通る
    assert_eq!(verify(&t.0, &t.0.join("d/top.sigil")), Ok(()));
    // 実体が根の中でも、見かけ（d/top.sigil）と実体（x/top.sigil）が違えば拒否
    std::fs::create_dir_all(t.0.join("x/sub")).unwrap();
    t.write("x/top.sigil", &module("d.top", "", &pub_fn("f")));
    std::os::unix::fs::symlink(t.0.join("x/sub"), t.0.join("d/in")).unwrap();
    let via_in = t.0.join("d/in/../top.sigil");
    assert_eq!(
        verify(&t.0, &via_in),
        Err((ErrorKind::ModulePathMismatch, 1))
    );
}

#[test]
fn run_rejects_use_explicitly() {
    let src = module("a.b", "\nuse x.y.f;\n", &pub_fn("g"));
    let e = sigil::ir_eval::run(&src, "g", &[]).unwrap_err();
    match e.error {
        sigil::exec::RunError::Check(e) => {
            assert_eq!((e.kind, e.span.line), (ErrorKind::Unsupported, 5));
        }
        other => panic!("Check(Unsupported) のはず: {other:?}"),
    }
}

// ---- CLI ----

fn bin(args: &[&str]) -> (i32, String, String) {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_sigil"))
        .args(args)
        .output()
        .unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn cli_check_and_spec_with_root() {
    let t = Tmp::new("cli");
    let good = t.write("app/ok.sigil", &module("app.ok", "", &pub_fn("f")));
    let bad = t.write("app/bad.sigil", &module("app.other", "", &pub_fn("f")));
    let std_x = t.write("std/x.sigil", &module("std.x", "", &pub_fn("f")));
    let root = t.0.to_str().unwrap();
    let (g, bd, sx) = (
        good.to_str().unwrap(),
        bad.to_str().unwrap(),
        std_x.to_str().unwrap(),
    );

    let (c, out, _) = bin(&["check", "--root", root, g]);
    assert_eq!((c, out.trim()), (0, format!("{g}: ok").as_str()));

    let (c, out, _) = bin(&["check", "--json", "--root", root, bd]);
    assert_eq!(c, 1);
    let v = sigil::json::parse(&out).unwrap();
    let d = &v.get("diagnostics").unwrap().as_array().unwrap()[0];
    assert_eq!(d.get("kind").unwrap().as_str(), Some("ModulePathMismatch"));
    assert_eq!(d.get("phase").unwrap().as_str(), Some("check"));
    assert_eq!(d.get("line").unwrap().as_i64(), Some(1));

    let (c, out, _) = bin(&["check", "--json", "--root", root, sx]);
    assert_eq!(c, 1);
    let v = sigil::json::parse(&out).unwrap();
    let d = &v.get("diagnostics").unwrap().as_array().unwrap()[0];
    assert_eq!(d.get("kind").unwrap().as_str(), Some("ReservedModuleName"));

    // --root がなければ従来どおり（パスは見ない）
    let (c, _, _) = bin(&["check", bd]);
    assert_eq!(c, 0);

    // spec も同じ検査をする
    let (c, _, err) = bin(&["spec", "--root", root, bd]);
    assert_eq!(c, 1, "{err}");
    let (c, out, _) = bin(&["spec", "--json", "--root", root, g]);
    assert_eq!(c, 0);
    assert!(out.contains("\"visibility\":\"pub\""), "{out}");
    let (c, _, _) = bin(&["spec", "--root"]);
    assert_eq!(c, 2);
    // spec --root でも std.* は拒否
    let (c, _, err) = bin(&["spec", "--root", root, sx]);
    assert_eq!(c, 1, "{err}");
    assert!(err.contains("標準ライブラリの名前"), "{err}");
}

#[test]
fn cli_run_and_build_reject_use() {
    let t = Tmp::new("cli-use");
    let f = t.write("a.sigil", &module("a", "\nuse x.y.f;\n", &pub_fn("g")));
    let p = f.to_str().unwrap();
    let (c, _, err) = bin(&["run", p, "g", "1"]);
    assert_eq!(c, 1, "{err}");
    assert!(err.contains("`use` を含むプログラムの実行"), "{err}");
    let out = t.0.join("out.bin");
    let (c, _, err) = bin(&["build", p, "g", "-o", out.to_str().unwrap()]);
    assert_eq!(c, 1, "{err}");
    assert!(err.contains("`use` を含むプログラムの実行"), "{err}");
    assert!(!out.exists());
}

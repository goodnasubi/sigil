//! `check.rs` 分割（docs/CHECK_SPLIT_PLAN.md）の前後で、検査と仕様の出力が変わっていないことを固定するスナップショット。
//!
//! `examples/` と `tests/data/` のすべての `.sigil` に `check`・`spec` を通し、結果を
//! `tests/snapshots/check_snapshot.txt` と比べる。出力が変わったら失敗する。
//! 意図した変更のときだけ、`SIGIL_UPDATE_SNAPSHOT=1 cargo test --test check_snapshot` で更新する。
//! （更新が仕様の変更になる場合は、人間のレビューが必要。）

use sigil::check::check;
use sigil::spec::spec;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|x| x == "sigil") {
            out.push(p);
        }
    }
}

fn render() -> String {
    let mut files = Vec::new();
    collect(&root().join("examples"), &mut files);
    collect(&root().join("tests").join("data"), &mut files);
    let mut rels: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    rels.sort();

    let mut s = String::new();
    for rel in rels {
        // 作業ツリーの改行（CRLF）に左右されないようにする。
        let src = fs::read_to_string(root().join(&rel))
            .unwrap()
            .replace("\r\n", "\n");
        s.push_str(&format!("=== {rel}\n"));
        match check(&src) {
            Ok(()) => s.push_str("check: ok\n"),
            Err(e) => s.push_str(&format!("check: {e}\n")),
        }
        match spec(&src) {
            Ok(t) => s.push_str(&format!("spec:\n{t}\n")),
            Err(e) => s.push_str(&format!("spec: {e}\n")),
        }
    }
    s
}

#[test]
fn check_and_spec_output_is_unchanged() {
    let path = root().join("tests/snapshots/check_snapshot.txt");
    let now = render();
    if std::env::var("SIGIL_UPDATE_SNAPSHOT").is_ok() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &now).unwrap();
        return;
    }
    let want = fs::read_to_string(&path)
        .expect("スナップショットが無い。SIGIL_UPDATE_SNAPSHOT=1 で作る")
        .replace("\r\n", "\n");
    assert!(
        want == now,
        "check/spec の出力がスナップショットと違う。意図した変更なら SIGIL_UPDATE_SNAPSHOT=1 で更新する"
    );
}

#[test]
fn snapshot_covers_passing_programs() {
    // 空振り（全件がエラー）になっていないことの確認。
    let now = render();
    let ok = now.matches("check: ok\n").count();
    assert!(ok >= 5, "型検査を通るサンプルが少なすぎる: {ok}");
}

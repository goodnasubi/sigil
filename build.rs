//! 暫定ランタイム（`runtime/core`・`runtime/os`）をビルドして、`.a` を `sigil` に埋め込む準備をする。
//! ADR 0015 の決定 6。
//!
//! - `cargo rustc --release --lib --crate-type staticlib --manifest-path runtime/<名前>/Cargo.toml --target-dir $OUT_DIR/rt` で 2 つの staticlib を作る。
//! - `rustc --print native-static-libs` の出力を読んで、リンク時に `cc` へ渡す `-l` の並びを `native_libs.txt` に書く。
//!   `-lgcc_s`・`-lc` は静的リンク（`cc -static`）では渡せない・要らないので除く（gcc が自分で足す）。
//! - 埋め込みは `src/runtime_lib.rs` が `include_bytes!` で行う。
//!
//! 外部のクレートは使わない（標準ライブラリだけ）。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 静的リンクで渡せない・要らない `-l`。
const SKIP_LIBS: [&str; 3] = ["gcc_s", "gcc_eh", "c"];

fn rerun_if_changed_dir(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            rerun_if_changed_dir(&p);
        } else {
            println!("cargo:rerun-if-changed={}", p.display());
        }
    }
}

fn cargo_cmd(manifest: &Path, target_dir: &Path) -> Command {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut c = Command::new(cargo);
    c.arg("rustc")
        .arg("--release")
        .arg("--lib")
        .arg("--crate-type")
        .arg("staticlib")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--target-dir")
        .arg(target_dir)
        // 親の cargo の指定（clippy のラッパーなど）を、ランタイムのビルドへ持ち込まない。
        .env("CARGO_TERM_COLOR", "never")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    c
}

fn build_one(root: &Path, name: &str, out: &Path) -> String {
    let manifest = root.join("runtime").join(name).join("Cargo.toml");
    let target_dir = out.join("rt");
    let output = cargo_cmd(&manifest, &target_dir)
        .arg("--")
        .arg("--print")
        .arg("native-static-libs")
        .output()
        .unwrap_or_else(|e| panic!("cargo を起動できない: {e}"));
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        panic!("runtime/{name} のビルドに失敗した:\n{stderr}");
    }
    let lib = target_dir
        .join("release")
        .join(format!("libsigil_rt_{name}.a"));
    fs::copy(&lib, out.join(format!("{name}.a")))
        .unwrap_or_else(|e| panic!("{} を写せない: {e}", lib.display()));
    stderr
}

/// `note: native-static-libs: -lgcc_s -lutil ...` の行から、渡してよい `-l` を取り出す。
fn native_libs(stderr: &str) -> Vec<String> {
    let Some(line) = stderr.lines().find(|l| l.contains("native-static-libs:")) else {
        return Vec::new();
    };
    let list = line.split("native-static-libs:").nth(1).unwrap_or("");
    list.split_whitespace()
        .filter(|t| {
            t.strip_prefix("-l")
                .is_some_and(|n| !SKIP_LIBS.contains(&n))
        })
        .map(str::to_string)
        .collect()
}

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed=build.rs");
    rerun_if_changed_dir(&root.join("runtime"));
    rerun_if_changed_dir(&root.join("src").join("shared"));

    let core_log = build_one(&root, "core", &out);
    let os_log = build_one(&root, "os", &out);
    // 2 つの staticlib はそれぞれ std を持つので、`-l` の和集合を重複なく取る。
    let mut libs = native_libs(&core_log);
    for l in native_libs(&os_log) {
        if !libs.contains(&l) {
            libs.push(l);
        }
    }
    fs::write(out.join("native_libs.txt"), libs.join(" ")).expect("native_libs.txt を書けない");
}

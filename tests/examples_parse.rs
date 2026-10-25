use sigil::parse;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn check(rel: &str) {
    let src = fs::read_to_string(root().join(rel)).unwrap();
    if let Err(e) = parse(&src) {
        panic!("{rel}: {e}");
    }
}

#[test]
fn example_01_brace() {
    check("examples/01-average/c-brace.sigil");
}

#[test]
fn example_02_brace() {
    check("examples/02-read-port/c-brace.sigil");
}

#[test]
fn example_03_ownership() {
    check("examples/03-ownership/ownership.sigil");
}

#[test]
fn example_04_effects() {
    check("examples/04-effects/effects.sigil");
}

#[test]
fn example_05_control() {
    check("examples/05-control/control.sigil");
}

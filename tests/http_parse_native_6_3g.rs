//! `std.http` の解釈器の、評価器とネイティブの突き合わせ（ADR 0032 決定 8、ADR 0030 補足 D の方針、PR 6-3g-b）。
//!
//! - 入力をまとめた 1 つのファイル（各行が 16 進の入力）を、1 回のビルドと 1 回の実行で処理する（ADR 0020 決定 2）。
//!   入口は `tests/data/http_driver.sigil` の `main`。各入力を、既定の上限と小さい上限で、全体・1 バイトずつ・
//!   分けて与えた記録（`Ok` の中身か `Err` の状態コードと消費したバイト数、保持の最大、往復）を 1 つの値で返す。
//! - 評価器（`sigil run`）とネイティブ（ワーカー 1 と 4）の値が同じで、どれも正常に終わり、漏れが無いこと
//!   （`SIGIL_RT_LIVE`: 評価器は `frames=0 chunks=0`、ネイティブは `sigil_blocks=0`）を確かめる。
//! - 値は入口の戻り値で返す（標準出力の行の混ざりに依らない。解釈器はタスクを作らない）。

mod common;
#[path = "common/fuzz.rs"]
mod fuzz;
#[path = "common/http.rs"]
mod http;

use common::native::scratch;
use std::process::Command;

fn inputs() -> Vec<Vec<u8>> {
    let dir = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fuzz/corpus/http_request"
    );
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "hex"))
        .collect();
    paths.sort();
    let corpus: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| http::unhex(&std::fs::read_to_string(p).unwrap()))
        .collect();
    let mut all = corpus.clone();
    let mut rng = fuzz::Rng::new(fuzz::DEFAULT_SEED ^ 0x6_3a7b);
    for i in 0..120 {
        if i % 4 == 0 {
            all.push(fuzz::random_bytes(&mut rng));
        } else {
            let base = rng.pick(&corpus).clone();
            all.push(fuzz::mutate(&mut rng, &base, &corpus));
        }
    }
    // 空の入力は行にできないので除く（空の行は `lines` で落ちる）。
    all.retain(|b| !b.is_empty());
    all
}

struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(mut c: Command) -> Ran {
    let o = c.env("SIGIL_RT_LIVE", "1").output().unwrap();
    Ran {
        code: o.status.code(),
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
    }
}

#[test]
fn http_parse_matches_between_eval_and_native() {
    let src = http::driver();
    let dir = scratch("http-parse");
    std::fs::create_dir_all(dir.join("app")).unwrap();
    let file = dir.join("app").join("main.sigil");
    std::fs::write(&file, &src).unwrap();
    let exe = dir.join("prog");
    sigil::native::build(&[&src], "main", &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    let data = dir.join("inputs.hex");
    let ins = inputs();
    let text: String = ins.iter().map(|b| http::hex(b) + "\n").collect();
    std::fs::write(&data, text).unwrap();

    let mut eval = Command::new(env!("CARGO_BIN_EXE_sigil"));
    eval.arg("run")
        .arg("--root")
        .arg(&dir)
        .arg(&file)
        .arg("main")
        .arg(&data);
    let e = run(eval);
    assert_eq!(e.code, Some(0), "評価器: {}", e.stderr);
    assert!(
        e.stderr.contains("live: frames=0 chunks=0"),
        "評価器に漏れがある: {}",
        e.stderr
    );
    // 入力ごとの記録が `/` で区切られて全部ある。
    let value = e.stdout.trim_end();
    assert_eq!(value.matches('/').count(), ins.len(), "{value}");
    assert!(!value.contains("NG"), "往復で違う要求になった: {value}");

    for w in [1, 4] {
        let mut c = Command::new(&exe);
        c.arg(&data).env("SIGIL_WORKERS", w.to_string());
        let n = run(c);
        assert_eq!(n.code, Some(0), "ネイティブ（ワーカー {w}）: {}", n.stderr);
        assert!(
            n.stderr.contains("live: sigil_blocks=0"),
            "ネイティブ（ワーカー {w}）に漏れがある: {}",
            n.stderr
        );
        assert_eq!(
            n.stdout.trim_end(),
            value,
            "評価器とネイティブ（ワーカー {w}）の結果が違う"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

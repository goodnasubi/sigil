//! ADR 0032 PR 6-3g-d: HTTP サーバーのサンプル `examples/10-http-server` の検証。
//!
//! 構文・正規形・検査・仕様の出力を固定し、評価器・ネイティブ（ワーカー 1・4）で動かして、同じ要求に同じ応答を返すことを確かめる。
mod common;

use sigil::fmt::format;
use sigil::spec::spec;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const PATH: &str = "examples/10-http-server/http_server";

fn src() -> String {
    std::fs::read_to_string(format!("{PATH}.sigil")).unwrap()
}

#[test]
fn example_parses_checks_and_is_canonical() {
    let s = src();
    sigil::parse(&s).unwrap();
    // 暗黙に結ぶ標準ライブラリを使うので、根の下に名前どおりのパスで置いて検査する（examples/README.md の「実行のしかた」）。
    let (root, file) = common::stage_src("ex-http-check", &s);
    let out = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("check")
        .arg("--root")
        .arg(&root)
        .arg(&file)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(format(&s).unwrap(), s);
}

#[test]
fn spec_matches_expected_output() {
    let want = std::fs::read_to_string(format!("{PATH}.spec")).unwrap();
    assert_eq!(spec(&src()).unwrap(), want);
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Eval,
    Native(u32),
}

struct Guard(Child);

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// サンプルを動かして、待ち受けの番号を `client` に渡し、終わりの値を返す。
fn serve<T>(mode: Mode, n: i64, client: impl FnOnce(u16) -> T) -> (T, String) {
    let s = src();
    let (root, file) = common::stage_src("ex-http-run", &s);
    let mut cmd = match mode {
        Mode::Eval => {
            let mut c = Command::new(env!("CARGO_BIN_EXE_sigil"));
            c.arg("run")
                .arg("--root")
                .arg(&root)
                .arg(&file)
                .arg("serve");
            c
        }
        Mode::Native(w) => {
            let exe: PathBuf = root.join("prog");
            sigil::native::build(
                &[s.as_str()],
                "serve",
                &sigil::codegen::Opts::default(),
                &exe,
            )
            .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
            let mut c = Command::new(exe);
            c.env("SIGIL_WORKERS", w.to_string());
            c
        }
    };
    cmd.args([n.to_string(), "4".to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut g = Guard(cmd.spawn().unwrap());
    let mut out = BufReader::new(g.0.stdout.take().unwrap());
    let mut first = String::new();
    out.read_line(&mut first).unwrap();
    let port: u16 = first
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("{mode:?}: 待ち受けの番号が出なかった: {first:?}"));
    let got = client(port);
    let mut rest = String::new();
    out.read_to_string(&mut rest).unwrap();
    let status = g.0.wait().unwrap();
    let _ = std::fs::remove_dir_all(&root);
    assert!(status.success(), "{mode:?}: {status:?}");
    (
        got,
        rest.trim_end().rsplit('\n').next().unwrap().to_string(),
    )
}

fn exchange(port: u16, payload: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(payload.as_bytes()).unwrap();
    s.shutdown(Shutdown::Write).unwrap();
    let mut got = vec![];
    s.read_to_end(&mut got).unwrap();
    String::from_utf8_lossy(&got).into_owned()
}

fn get(path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n")
}

/// 挨拶・本文のそのままの返信・404 を、評価器・ネイティブ（ワーカー 1・4）で同じ応答にする。
#[test]
fn example_serves_the_same_responses_in_every_mode() {
    let post = "POST /echo HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello";
    for mode in [Mode::Eval, Mode::Native(1), Mode::Native(4)] {
        let (got, last) = serve(mode, 3, |port| {
            (
                exchange(port, &get("/")),
                exchange(port, post),
                // 持続接続で 2 件続けて答え、最後の応答の後で閉じる。
                exchange(port, &format!("{}{}", get("/nope"), get("/"))),
            )
        });
        assert_eq!(last, "3", "{mode:?}");
        assert_eq!(
            got.0, "HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello",
            "{mode:?}"
        );
        assert_eq!(
            got.1,
            "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: 5\r\n\r\nhello",
            "{mode:?}"
        );
        assert_eq!(
            got.2,
            "HTTP/1.1 404 Not Found\r\ncontent-length: 9\r\n\r\nnot foundHTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello",
            "{mode:?}"
        );
    }
}

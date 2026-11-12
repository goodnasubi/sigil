//! HTTP の要求の増分の解釈器（`std.http` の `http.parser`・`HttpParser.feed`）のファジング（ADR 0020 決定 2、
//! ADR 0032 決定 7、PR 6-3g-b）。対象は Sigil で書いた解釈器で、SIR の評価器で動かす（「Sigil の対象」）。
//!
//! - 性質（`tests/common/http.rs` の `check_probe`）: どの入力にも停止せずに `Ok`／`Err` を返す。消費したバイト数が入力の
//!   長さを超えない。全体・1 バイトずつ・任意の位置で分けて与えた結果が同じ。保持が上限の和を超えない。往復で同じ要求。
//! - 入力: 回帰（`tests/fuzz/regress/http_request/`）、種（`tests/fuzz/corpus/http_request/`。R1〜R12 と密輸の既知の形）、
//!   ランダム・種の変異・文法からの組み立て（`tests/common/fuzz.rs` の枠）。上限は既定値か、小さい乱数の値。
//! - 種と回帰のファイルは 16 進のテキスト（`.hex`）。CRLF を git の改行の正規化から守るため（`.gitattributes` は LF に揃える）。
//! - 評価器の lowering を入力ごとにしないよう、`BATCH` 件をまとめて 1 回の `run_roots_with` で動かす（1 件 = 1 つのルート）。
//!   処理の前に、まとめた入力を `target/fuzz-failures/http_request/current` に書き出す（プロセスが落ちたときの再現用）。
//!   性質が破れた入力は `fail-<種>-<番号>`（生のバイト列）に書き出し、種と番号を出して失敗にする。
//! - 長く回すとき: `SIGIL_FUZZ_ITERS=20000 SIGIL_FUZZ_SEED=7 cargo test --release --test fuzz_http_request -- --nocapture`。

#[path = "common/fuzz.rs"]
mod fuzz;
#[path = "common/http.rs"]
mod http;

use fuzz::Rng;
use sigil::exec::Arg;
use std::path::PathBuf;

const TARGET: &str = "http_request";
/// 1 回の評価器の実行でまとめて動かす入力の数。
const BATCH: usize = 64;
/// 既定の生成の回数（CI の増分を 60 秒以内にする。ADR 0020 決定 2。PR で測った値は PR 本文に記録する）。
const DEFAULT_ITERS: usize = 600;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_hex_dir(dir: &str) -> Vec<Vec<u8>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root().join(dir))
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "hex"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|p| http::unhex(&std::fs::read_to_string(p).unwrap()))
        .collect()
}

const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "HEAD", "OPTIONS", "DELETE", "CONNECT", "TRACE", "get", "G(T", "",
];
const TARGETS: &[&str] = &[
    "/",
    "/a/b?c=d",
    "*",
    "/?",
    "http://a/",
    "a:80",
    "/%20x",
    "/a b",
    "/\u{e9}",
    "",
];
const VERSIONS: &[&str] = &[
    "HTTP/1.1",
    "HTTP/1.1",
    "HTTP/1.0",
    "HTTP/2.0",
    "http/1.1",
    "HTTP/1.10",
    "HTTP/1",
];
const HEADERS: &[&str] = &[
    "Host: a",
    "Host: a",
    "host:b",
    "User-Agent: x y",
    "X-Empty:",
    "Expect: 100-continue",
    "Expect: nope",
    "Connection: close",
    " folded",
    "Bad Name: v",
    "X: \u{1}",
    "X: \t\u{80}",
    "Content-Length: 0",
    "Transfer-Encoding: Chunked",
    "Transfer-Encoding: gzip",
    "Transfer-Encoding: chunked ",
];
const EOLS: &[&[u8]] = &[b"\r\n", b"\r\n", b"\r\n", b"\r\n", b"\n", b"\r"];

/// HTTP の要求の骨組みを作り、ところどころを壊す（ADR 0020 決定 2 の「文法からの組み立て」）。
fn request(rng: &mut Rng, out: &mut Vec<u8>) {
    let eol = |rng: &mut Rng| *rng.pick(EOLS);
    out.extend_from_slice(rng.pick(METHODS).as_bytes());
    out.push(b' ');
    out.extend_from_slice(rng.pick(TARGETS).as_bytes());
    if rng.chance(1, 20) {
        out.push(b' ');
    }
    out.push(b' ');
    out.extend_from_slice(rng.pick(VERSIONS).as_bytes());
    out.extend_from_slice(eol(rng));
    for _ in 0..rng.below(4) {
        out.extend_from_slice(rng.pick(HEADERS).as_bytes());
        out.extend_from_slice(eol(rng));
    }
    let body: Vec<u8> = (0..rng.below(20)).map(|i| b'a' + (i % 26) as u8).collect();
    match rng.below(4) {
        0 => {}
        1 => {
            let n = match rng.below(5) {
                0 => format!("{}", body.len() + rng.below(3)),
                1 => format!("0{}", body.len()),
                2 => format!("{}, {}", body.len(), body.len()),
                3 => "99999999999999999999".to_string(),
                _ => format!("{}", body.len()),
            };
            out.extend_from_slice(format!("Content-Length: {n}").as_bytes());
            out.extend_from_slice(eol(rng));
            if rng.chance(1, 6) {
                out.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
            }
            out.extend_from_slice(eol(rng));
            out.extend_from_slice(&body);
            return;
        }
        _ => {
            out.extend_from_slice(b"Transfer-Encoding: chunked");
            out.extend_from_slice(eol(rng));
            if rng.chance(1, 6) {
                out.extend_from_slice(b"Content-Length: 3\r\n");
            }
            out.extend_from_slice(eol(rng));
            let mut rest = &body[..];
            while !rest.is_empty() {
                let k = 1 + rng.below(rest.len());
                let size = match rng.below(8) {
                    0 => format!("0{k:x}"),
                    1 => format!("{k:x};e=1"),
                    2 => format!("{k:X}"),
                    3 => format!("{k:x} "),
                    _ => format!("{k:x}"),
                };
                out.extend_from_slice(size.as_bytes());
                out.extend_from_slice(eol(rng));
                out.extend_from_slice(&rest[..k]);
                out.extend_from_slice(eol(rng));
                rest = &rest[k..];
            }
            out.extend_from_slice(if rng.chance(1, 8) { b"00" } else { b"0" });
            out.extend_from_slice(eol(rng));
            if rng.chance(1, 3) {
                out.extend_from_slice(rng.pick(HEADERS).as_bytes());
                out.extend_from_slice(eol(rng));
            }
            out.extend_from_slice(eol(rng));
            return;
        }
    }
    out.extend_from_slice(eol(rng));
}

fn grammar(rng: &mut Rng) -> Vec<u8> {
    let mut out = vec![];
    for _ in 0..1 + rng.below(3) {
        request(rng, &mut out);
    }
    if rng.chance(1, 6) && !out.is_empty() {
        let i = rng.below(out.len());
        out.truncate(i);
    }
    out
}

/// 分ける位置（0〜4 個、昇順）と上限（既定値か、小さい乱数の値）。
fn shape(rng: &mut Rng, n: usize) -> (Vec<i64>, Vec<i64>) {
    let mut cuts: Vec<i64> = (0..rng.below(5)).map(|_| rng.below(n + 1) as i64).collect();
    cuts.sort();
    let lim = if rng.chance(1, 2) {
        vec![]
    } else {
        vec![
            4 + rng.below(80) as i64,
            1 + rng.below(120) as i64,
            1 + rng.below(6) as i64,
            1 + rng.below(40) as i64,
            1 + rng.below(12) as i64,
            1 + rng.below(80) as i64,
        ]
    };
    (cuts, lim)
}

#[test]
fn fuzz_http_request() {
    let n = fuzz::iters(DEFAULT_ITERS);
    let seed = fuzz::seed();
    let regress = read_hex_dir("tests/fuzz/regress/http_request");
    let corpus = read_hex_dir("tests/fuzz/corpus/http_request");
    assert!(corpus.len() >= 30, "種が足りない（R1〜R12 と密輸の形）");
    let mut rng = Rng::new(seed);
    // (由来, 番号, 入力, 分ける位置, 上限)
    let mut cases: Vec<(&str, usize, Vec<u8>, Vec<i64>, Vec<i64>)> = vec![];
    for (i, b) in regress.iter().enumerate() {
        let len = b.len() as i64;
        cases.push(("回帰", i, b.clone(), vec![1, len / 2], vec![]));
    }
    for (i, b) in corpus.iter().enumerate() {
        let len = b.len() as i64;
        cases.push(("種", i, b.clone(), vec![1, len / 2], vec![]));
        let (c, l) = shape(&mut rng, b.len());
        cases.push(("種（小さい上限）", i, b.clone(), c, l));
    }
    for i in 0..n {
        let input = match i % 3 {
            0 => fuzz::random_bytes(&mut rng),
            1 => {
                let base = rng.pick(&corpus).clone();
                fuzz::mutate(&mut rng, &base, &corpus)
            }
            _ => grammar(&mut rng),
        };
        let (c, l) = shape(&mut rng, input.len());
        cases.push(("生成", i, input, c, l));
    }
    let dir = root().join("target/fuzz-failures").join(TARGET);
    let _ = std::fs::create_dir_all(&dir);
    let current = dir.join("current");
    let mut total = 0;
    for chunk in cases.chunks(BATCH) {
        let text: String = chunk
            .iter()
            .map(|(_, _, b, _, _)| http::hex(b) + "\n")
            .collect();
        let _ = std::fs::write(&current, text);
        let sets: Vec<Vec<Arg>> = chunk
            .iter()
            .map(|(_, _, b, c, l)| {
                vec![
                    Arg::List(b.iter().map(|&x| Arg::Int(x as i64)).collect()),
                    Arg::List(c.iter().map(|&x| Arg::Int(x)).collect()),
                    Arg::List(l.iter().map(|&x| Arg::Int(x)).collect()),
                ]
            })
            .collect();
        let got = http::roots("probe", &sets);
        for ((label, i, b, _, l), r) in chunk.iter().zip(got) {
            let res = r.and_then(|rec| http::check_probe(b, l, &rec).map(|_| ()));
            if let Err(e) = res {
                let f = dir.join(format!("fail-{seed}-{i}"));
                let _ = std::fs::write(&f, b);
                panic!(
                    "ファジング {TARGET} が失敗（{label}。種 {seed}、{i} 番目。上限 {l:?}）: {e}\n入力: {}",
                    f.display()
                );
            }
            total += 1;
        }
    }
    let _ = std::fs::remove_file(&current);
    eprintln!("fuzz {TARGET}: {total} 件（種 {seed}）");
}

//! 自前のファジングの枠（ADR 0020 の決定 2。依存を増やさない）。
//!
//! - 乱数は splitmix64（種から決まる。同じ種なら同じ入力列）。
//! - 入力は 3 通りで作る: ランダムなバイト列、コーパスの変異、文法からの生成（順に回す）。
//! - 既定は種固定・短時間（`cargo test` の中で動く）。長く回すときは環境変数で指定する。
//!   `SIGIL_FUZZ_ITERS=200000 SIGIL_FUZZ_SEED=7 cargo test --test fuzz_parse -- --nocapture`
//! - スタックあふれは `catch_unwind` で捕まえられない。そのため（1）各入力を処理する前に
//!   `target/fuzz-failures/<対象>/current` へ書き出し（落ちたらこのファイルが再現入力）、
//!   （2）全体を固定サイズ（2 MiB）のスレッドで回す。
//! - 失敗した入力は `target/fuzz-failures/<対象>/fail-<種>-<番号>` に書き、
//!   直したら `tests/fuzz/regress/<対象>/` に移して回帰試験にする（毎回先に流れる）。
#![allow(dead_code)]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};

/// 試験の既定の種。
pub const DEFAULT_SEED: u64 = 0x5167_11fa_c0de_0001;
/// 処理するスレッドのスタック（Rust のテストのスレッドと同じ 2 MiB）。
pub const STACK_BYTES: usize = 2 * 1024 * 1024;

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// 0 以上 n 未満（n = 0 なら 0）。
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next_u64() % n as u64) as usize
        }
    }
    /// 確率 num/den で true。
    pub fn chance(&mut self, num: usize, den: usize) -> bool {
        self.below(den) < num
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

/// 変異に使う境界値と字句の断片。
const BOUNDARY: &[&[u8]] = &[
    b"0",
    b"-1",
    b"255",
    b"256",
    b"9223372036854775807",
    b"-9223372036854775808",
    b"9223372036854775808",
    b"\r\n",
    b"\r",
    b"\n",
    b"\t",
    b"\0",
    b"\"",
    b"\\",
    b"//",
    b"/*",
    b"{",
    b"}",
    b"(",
    b")",
    b"[",
    b"]",
    b"<",
    b">",
    b"->",
    b"=>",
    "あ".as_bytes(),
    b"\xff",
    b"\xe3\x81",
    b"\xed\xa0\x80",
];

/// コーパスの 1 つを変異させる（回数は 1〜4 回）。
pub fn mutate(rng: &mut Rng, base: &[u8], corpus: &[Vec<u8>]) -> Vec<u8> {
    let mut v = base.to_vec();
    for _ in 0..1 + rng.below(4) {
        match rng.below(8) {
            0 if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] ^= 1 << rng.below(8);
            }
            1 => {
                let i = rng.below(v.len() + 1);
                v.insert(i, rng.next_u64() as u8);
            }
            2 if !v.is_empty() => {
                let i = rng.below(v.len());
                v.remove(i);
            }
            3 if !v.is_empty() => {
                // 範囲の削除
                let a = rng.below(v.len());
                let b = (a + 1 + rng.below(32)).min(v.len());
                v.drain(a..b);
            }
            4 if !v.is_empty() => {
                // 範囲の繰り返し
                let a = rng.below(v.len());
                let b = (a + 1 + rng.below(32)).min(v.len());
                let chunk = v[a..b].to_vec();
                let times = 1 + rng.below(40);
                let at = rng.below(v.len() + 1);
                for _ in 0..times {
                    v.splice(at..at, chunk.iter().copied());
                }
            }
            5 => {
                // 他のコーパスとの継ぎ合わせ
                let o = rng.pick(corpus);
                if !o.is_empty() {
                    let a = rng.below(o.len());
                    let b = (a + 1 + rng.below(64)).min(o.len());
                    let at = rng.below(v.len() + 1);
                    v.splice(at..at, o[a..b].iter().copied());
                }
            }
            6 => {
                let s = *rng.pick(BOUNDARY);
                let at = rng.below(v.len() + 1);
                v.splice(at..at, s.iter().copied());
            }
            _ if !v.is_empty() => {
                let i = rng.below(v.len());
                v[i] = *rng.pick(&[0u8, 1, 0x7f, 0x80, 0xff, b'\n', b' ', b'"']);
            }
            _ => {}
        }
    }
    v
}

/// ランダムなバイト列（長さは 0〜96）。
pub fn random_bytes(rng: &mut Rng) -> Vec<u8> {
    let n = rng.below(96);
    if rng.chance(1, 2) {
        // 印字できる ASCII に寄せる（字句の層を通りやすくする）。
        (0..n).map(|_| 0x20 + rng.below(0x5f) as u8).collect()
    } else {
        (0..n).map(|_| rng.next_u64() as u8).collect()
    }
}

/// 反復回数（`SIGIL_FUZZ_ITERS`。なければ既定）。
pub fn iters(default: usize) -> usize {
    std::env::var("SIGIL_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// 種（`SIGIL_FUZZ_SEED`。なければ固定値）。
pub fn seed() -> u64 {
    std::env::var("SIGIL_FUZZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SEED)
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_dir_files(dir: &Path) -> Vec<Vec<u8>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    paths.sort();
    paths.iter().filter_map(|p| std::fs::read(p).ok()).collect()
}

/// `tests/fuzz/corpus/<対象>/` と、追加のディレクトリのファイルを読む。
pub fn corpus(target: &str, extra: &[&str]) -> Vec<Vec<u8>> {
    let mut c = read_dir_files(&root().join("tests/fuzz/corpus").join(target));
    for d in extra {
        c.extend(read_dir_files(&root().join(d)));
    }
    c
}

fn fail_dir(target: &str) -> PathBuf {
    root().join("target/fuzz-failures").join(target)
}

/// 枠の本体。`check` は 1 つの入力について性質を確かめる（破れたら panic する）。
/// `grammar` は文法から 1 つ入力を作る。
pub fn run(
    target: &'static str,
    default_iters: usize,
    corpus: Vec<Vec<u8>>,
    grammar: fn(&mut Rng) -> Vec<u8>,
    check: fn(&[u8]),
) {
    let n = iters(default_iters);
    let seed = seed();
    let regress = read_dir_files(&root().join("tests/fuzz/regress").join(target));
    let dir = fail_dir(target);
    let _ = std::fs::create_dir_all(&dir);
    let handle = std::thread::Builder::new()
        .name(format!("fuzz-{target}"))
        .stack_size(STACK_BYTES)
        .spawn(move || {
            let current = dir.join("current");
            let mut rng = Rng::new(seed);
            let mut inputs = 0usize;
            let mut step = |label: &str, i: usize, input: Vec<u8>| {
                // スタックあふれで落ちたときの再現用。処理の前に書く。
                let _ = std::fs::write(&current, &input);
                if catch_unwind(AssertUnwindSafe(|| check(&input))).is_err() {
                    let f = dir.join(format!("fail-{seed}-{i}"));
                    let _ = std::fs::write(&f, &input);
                    panic!(
                        "ファジング {target} が失敗（{label}。種 {seed}、{i} 番目）。入力: {}",
                        f.display()
                    );
                }
                inputs += 1;
            };
            for (i, input) in regress.iter().enumerate() {
                step("回帰", i, input.clone());
            }
            for (i, input) in corpus.iter().enumerate() {
                step("コーパス", i, input.clone());
            }
            for i in 0..n {
                let input = match i % 3 {
                    0 => random_bytes(&mut rng),
                    1 if !corpus.is_empty() => {
                        let base = rng.pick(&corpus).clone();
                        mutate(&mut rng, &base, &corpus)
                    }
                    _ => grammar(&mut rng),
                };
                step("生成", i, input);
            }
            let _ = std::fs::remove_file(&current);
            eprintln!("fuzz {target}: {inputs} 件（種 {seed}）");
        })
        .unwrap();
    if let Err(e) = handle.join() {
        std::panic::resume_unwind(e);
    }
}

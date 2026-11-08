//! ADR 0025 補足 C-7 の 3-6c（補足 L）: `env` の Sigil 実装（`std.os.env`）。
//!
//! 言語に `env` を読む関数はまだなく（評価器の組み込みもない）ので、駆動モジュール（`std.os.t`）から
//! `std.os.env` の関数を直接呼ぶ。
//! - `lookup`（`envp` の配列を引数に取る）は評価器とネイティブの両方で、`raw` の操作で組み立てた模型の `envp` に対して走らせ、
//!   Rust の参照実装（最初の `=` で割る・最初の一致・`from_utf8_lossy`）と突き合わせる。
//! - `get`（大域 `ENVP` を読む）はネイティブだけ。起動の処理が書いた本物の `envp` を、子プロセスの環境で確かめる。
mod common;
use common::native::{scratch, Ran};
use sigil::effects::Options as COpts;
use sigil::exec::{Arg, Options};
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::process::Command;

const TAIL: &str = "  intent \"x\"\n  requires none\n  ensures none\n  effects none\n";

/// 環境の項（`NAME=VALUE` のバイト列。NUL は含まない）。`=` のない項も入れる。
const ENTRIES: &[&[u8]] = &[
    b"A=1",
    b"B=",
    b"C=x=y=z",
    b"D=first",
    b"D=second",
    b"E=ok\xff\xfe\xc3(\xe3\x81",
    b"F=\xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e",
    b"NOEQ",
    b"=EMPTYNAME",
    b"AB=ab",
    b"A_=under",
    b"G==lead",
    b"H=\xed\xa0\x80\xf4\x90\x80\x80",
];

/// 引く名前。`k` 番目。
const NAMES: &[&[u8]] = &[
    b"A",
    b"B",
    b"C",
    b"D",
    b"E",
    b"F",
    b"NOEQ",
    b"",
    b"Z",
    b"AB",
    b"a",
    b"A=1",
    b"DD",
    b"ABC",
    b"A_",
    b"G",
    b"H",
    b"=EMPTYNAME",
    b"NOE",
    b"AB=",
];

/// 参照実装: 最初の一致の値を `from_utf8_lossy` にしたもの。
fn reference(entries: &[&[u8]], name: &[u8]) -> Option<String> {
    if name.is_empty() || name.contains(&b'=') || name.contains(&0) {
        return None;
    }
    entries.iter().find_map(|e| {
        let i = e.iter().position(|&b| b == b'=')?;
        (&e[..i] == name).then(|| String::from_utf8_lossy(&e[i + 1..]).into_owned())
    })
}

/// 駆動と同じ指紋: 無ければ -1、あれば `h = len; h = (h * 131 + b) % P` を各バイトで。
fn fingerprint(v: &Option<String>) -> i64 {
    match v {
        None => -1,
        Some(s) => {
            let mut h = s.len() as i64;
            for b in s.bytes() {
                h = (h * 131 + b as i64) % 1_000_000_007;
            }
            h
        }
    }
}

fn store_bytes(var: &str, b: &[u8]) -> String {
    b.iter()
        .enumerate()
        .map(|(i, c)| format!("  raw.store_u8(raw.offset({var}, {i}), {c});\n"))
        .collect()
}

/// `name_block` は `k` から名前の番地 `np` と長さ `nl` を決める断片。`call` は結果 `r` を得る式。
fn driver(entries: Option<&[&[u8]]>, call_get: bool) -> String {
    let mut body = String::new();
    // 引く名前を `k` で選ぶ。
    body.push_str("  let mut np = raw.addr_null();\n  let mut nl = 0;\n");
    for (k, n) in NAMES.iter().enumerate() {
        if n.is_empty() {
            body.push_str(&format!("  if k == {k} {{\n    nl = 0;\n  }}\n"));
        } else {
            body.push_str(&format!(
                "  if k == {k} {{\n  np = raw.alloc({0}, 1);\n{1}  nl = {0};\n  }}\n",
                n.len(),
                store_bytes("np", n)
            ));
        }
    }
    let mut free = String::new();
    let call = if call_get {
        "  let r = get(np, nl);\n".to_string()
    } else {
        let es = entries.unwrap();
        let n = es.len();
        body.push_str(&format!("  let envp = raw.alloc({}, 8);\n", 8 * (n + 1)));
        for (i, e) in es.iter().enumerate() {
            body.push_str(&format!(
                "  let e{i} = raw.alloc({}, 1);\n{}  raw.store_u8(raw.offset(e{i}, {}), 0);\n  raw.store_addr(raw.offset(envp, {}), e{i});\n",
                e.len() + 1,
                store_bytes(&format!("e{i}"), e),
                e.len(),
                8 * i
            ));
            free.push_str(&format!("  raw.free(e{i}, {}, 1);\n", e.len() + 1));
        }
        body.push_str(&format!(
            "  raw.store_addr(raw.offset(envp, {}), raw.addr_null());\n",
            8 * n
        ));
        free.push_str(&format!("  raw.free(envp, {}, 8);\n", 8 * (n + 1)));
        "  let r = lookup(envp, np, nl);\n".to_string()
    };
    body.push_str(&call);
    body.push_str(
        "  let mut h = -1;\n  if raw.addr_to_int(r) != 0 {\n    let n = bytes(r);\n    let d = data(r);\n    h = n;\n    let mut i = 0;\n    while i < n {\n      h = (h * 131 + raw.load_u8(raw.offset(d, i))) % 1000000007;\n      i = i + 1;\n    }\n    free(r);\n  }\n",
    );
    body.push_str("  if nl > 0 {\n    raw.free(np, nl, 1);\n  }\n");
    body.push_str(&free);
    body.push_str("  return h;\n");
    let f = if call_get { "get" } else { "lookup" };
    format!(
        "module std.os.t\n  targets server\n  trust base\n\nuse std.core.string.bytes;\nuse std.core.string.data;\nuse std.core.string.free;\nuse std.os.env.{f};\n\npub fn run(k: Int) -> Int\n{TAIL}  unchecked \"単一スレッド\"\n{{\n{body}}}\n"
    )
}

fn user_entry() -> String {
    "module app.main\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 7;\n}\n"
        .to_string()
}

fn copts(drv: String) -> COpts {
    COpts {
        embedded_extra: vec![("std.os.t".to_string(), drv)],
        embedded_roots: vec!["std.os.t".into()],
        ..Default::default()
    }
}

fn eval(drv: String, k: i64) -> Result<String, String> {
    let c = copts(drv);
    let o = Options {
        embedded_extra: c.embedded_extra,
        embedded_roots: c.embedded_roots,
        ..Options::captured()
    };
    match sigil::ir_eval::run_sources_with(&[&user_entry()], "std.os.t.run", &[Arg::Int(k)], &o) {
        Ok(o) => Ok(o.value),
        Err(f) => Err(format!("{:?}", f.error)),
    }
}

fn build(drv: String) -> std::path::PathBuf {
    let c = copts(drv);
    let p = sigil::ir_eval::lower_sources_opts(&[&user_entry()], "std.os.t.run", &c)
        .unwrap_or_else(|e| panic!("lowering に失敗: {e}"));
    if let Err(es) = sigil::verify::verify(&p) {
        panic!("SIR の検証に失敗:\n{}", es.join("\n"));
    }
    let exe = scratch("env36c").join("a.out");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe)
        .unwrap_or_else(|e| panic!("ネイティブのビルドに失敗: {e}"));
    exe
}

/// 子プロセスの環境を `env`（バイト列の `NAME=VALUE`）だけにして走らせる。
fn exec_envp(exe: &std::path::Path, k: i64, env: &[&[u8]]) -> Ran {
    let mut cmd = Command::new(exe);
    cmd.arg(k.to_string()).env_clear();
    for e in env {
        let i = e.iter().position(|&b| b == b'=').unwrap();
        cmd.env(
            OsString::from_vec(e[..i].to_vec()),
            OsString::from_vec(e[i + 1..].to_vec()),
        );
    }
    let o = cmd.output().unwrap();
    Ran {
        stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        code: o.status.code(),
    }
}

#[test]
fn lookup_matches_the_reference_on_evaluator_and_native() {
    let drv = driver(Some(ENTRIES), false);
    let exe = build(drv.clone());
    for (k, name) in NAMES.iter().enumerate() {
        let want = fingerprint(&reference(ENTRIES, name)).to_string();
        let got = eval(drv.clone(), k as i64).unwrap_or_else(|e| panic!("name {name:?}: {e}"));
        assert_eq!(
            got,
            want,
            "評価器: name {:?}",
            String::from_utf8_lossy(name)
        );
        let r = exec_envp(&exe, k as i64, &[]);
        assert_eq!(r.code, Some(0), "{}", r.stderr);
        assert_eq!(
            r.stdout.trim(),
            want,
            "ネイティブ: name {:?}",
            String::from_utf8_lossy(name)
        );
    }
}

#[test]
fn lookup_reference_decisions() {
    // 設定された変数・無い変数・空の値・値に '=' を含む変数・不正な UTF-8 の置換、最初の一致、'=' なしの項。
    assert_eq!(reference(ENTRIES, b"A").as_deref(), Some("1"));
    assert_eq!(reference(ENTRIES, b"Z"), None);
    assert_eq!(reference(ENTRIES, b"B").as_deref(), Some(""));
    assert_eq!(reference(ENTRIES, b"C").as_deref(), Some("x=y=z"));
    assert_eq!(reference(ENTRIES, b"D").as_deref(), Some("first"));
    assert_eq!(
        reference(ENTRIES, b"E").as_deref(),
        Some("ok\u{fffd}\u{fffd}\u{fffd}(\u{fffd}")
    );
    assert_eq!(reference(ENTRIES, b"NOEQ"), None);
    assert_eq!(reference(ENTRIES, b""), None);
    assert_eq!(reference(ENTRIES, b"G").as_deref(), Some("=lead"));
}

#[test]
fn empty_and_null_envp() {
    let drv = driver(Some(&[]), false);
    for k in 0..NAMES.len() as i64 {
        assert_eq!(eval(drv.clone(), k).unwrap(), "-1");
    }
}

#[test]
fn get_reads_the_real_envp_written_by_start() {
    let drv = driver(None, true);
    let exe = build(drv);
    // 子の環境は `ENTRIES` のうち '=' を持つ項（同名は後の項が先の項を置き換えるので、重複は除く）。
    let env: Vec<&[u8]> = ENTRIES
        .iter()
        .copied()
        .filter(|e| e.contains(&b'=') && !e.starts_with(b"=") && *e != b"D=second")
        .collect();
    for (k, name) in NAMES.iter().enumerate() {
        let want = fingerprint(&reference(&env, name)).to_string();
        let r = exec_envp(&exe, k as i64, &env);
        assert_eq!(r.code, Some(0), "{}", r.stderr);
        assert_eq!(
            r.stdout.trim(),
            want,
            "name {:?}",
            String::from_utf8_lossy(name)
        );
    }
    // 環境が空でも止まらない（envp は NULL だけの配列）。
    let r = exec_envp(&exe, 0, &[]);
    assert_eq!(r.code, Some(0), "{}", r.stderr);
    assert_eq!(r.stdout.trim(), "-1");
}

#[test]
fn evaluator_get_sees_a_null_envp() {
    // 評価器は ENVP を書かない（C-4）ので、`get` は常に空番地（-1）。
    let drv = driver(None, true);
    assert_eq!(eval(drv, 0).unwrap(), "-1");
}

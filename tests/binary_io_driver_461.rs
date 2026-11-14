//! ADR 0036 の内部駆動。本番と同じ Sigil 本体の syscall と一時確保だけを試験用に差し替える。
//! 不正な番地・解放サイズ・二重解放は評価器のチャンク表、漏れは native の live で検出する。
mod common;
use common::native::{exec_env, scratch};
use sigil::effects::Options as CheckOptions;
use sigil::exec::Options;

#[derive(Clone)]
struct Step {
    nr: i64,
    result: i64,
    bytes: Vec<u8>,
    request: Option<i64>,
    stat: Option<(i64, i64)>,
}
fn step(nr: i64, result: i64) -> Step {
    Step {
        nr,
        result,
        bytes: vec![],
        request: None,
        stat: None,
    }
}
fn stat(size: i64) -> Step {
    Step {
        stat: Some((32768, size)),
        ..step(5, 0)
    }
}
fn read(count: i64, bytes: &[u8]) -> Step {
    Step {
        bytes: bytes.to_vec(),
        request: Some(count),
        ..step(0, bytes.len() as i64)
    }
}
fn write(count: i64, bytes: &[u8]) -> Step {
    Step {
        bytes: bytes.to_vec(),
        request: Some(count),
        ..step(1, bytes.len() as i64)
    }
}
struct Case {
    writing: bool,
    max: i64,
    path_len: i64,
    data_len: i64,
    steps: Vec<Step>,
    // 成功 -1、失敗は BinaryIoError の腕番号。Io のときは二番目の番号も検査。
    error: i64,
    io: i64,
    output: Vec<u8>,
    peak: i64,
    allocations: i64,
}
fn rc(max: i64, steps: Vec<Step>, error: i64, io: i64, output: &[u8]) -> Case {
    Case {
        writing: false,
        max,
        path_len: 1,
        data_len: 0,
        steps,
        error,
        io,
        output: output.to_vec(),
        peak: 144.max(max + 1),
        allocations: 4,
    }
}
fn wc(steps: Vec<Step>, error: i64, io: i64) -> Case {
    Case {
        writing: true,
        max: 3,
        path_len: 1,
        data_len: 3,
        steps,
        error,
        io,
        output: vec![],
        peak: 144,
        allocations: 2,
    }
}
fn cases() -> Vec<Case> {
    let mut v = vec![
        rc(
            4,
            vec![
                step(257, 7),
                stat(4),
                read(4, &[0, 255]),
                read(2, &[]),
                step(3, 0),
            ],
            -1,
            0,
            &[0, 255],
        ),
        rc(
            2,
            vec![
                step(257, 7),
                stat(1),
                read(2, &[0, 1]),
                read(1, &[2]),
                step(3, 0),
            ],
            2,
            0,
            &[],
        ),
        rc(
            2,
            vec![
                step(257, 7),
                stat(2),
                read(2, &[0, 255]),
                read(1, &[]),
                step(3, 0),
            ],
            -1,
            0,
            &[0, 255],
        ),
        rc(
            0,
            vec![step(257, 7), stat(0), read(1, &[]), step(3, 0)],
            -1,
            0,
            &[],
        ),
        rc(
            0,
            vec![step(257, 7), stat(0), read(1, &[1]), step(3, 0)],
            2,
            0,
            &[],
        ),
        rc(
            2,
            vec![
                step(257, -4),
                step(257, 7),
                step(5, -4),
                stat(2),
                Step {
                    request: Some(2),
                    ..step(0, -4)
                },
                read(2, &[0]),
                read(1, &[255]),
                read(1, &[]),
                step(3, 0),
            ],
            -1,
            0,
            &[0, 255],
        ),
        rc(
            3,
            vec![
                step(257, 7),
                stat(3),
                read(3, &[0]),
                Step {
                    request: Some(2),
                    ..step(0, -13)
                },
                step(3, -2),
            ],
            4,
            1,
            &[],
        ),
        rc(
            1,
            vec![step(257, 7), stat(0), read(1, &[]), step(3, -4)],
            4,
            2,
            &[],
        ),
        rc(1, vec![step(257, 7), step(5, -2), step(3, -13)], 4, 0, &[]),
        rc(
            1,
            vec![
                step(257, 7),
                Step {
                    stat: Some((8192, 0)),
                    ..step(5, 0)
                },
                step(3, 0),
            ],
            3,
            0,
            &[],
        ),
        rc(4, vec![step(257, 7), stat(5), step(3, 0)], 2, 0, &[]),
        rc(1, vec![step(257, -2)], 4, 0, &[]),
        wc(
            vec![
                step(257, -4),
                step(257, 7),
                step(5, -4),
                stat(0),
                Step {
                    request: Some(3),
                    ..step(1, -4)
                },
                write(3, &[0]),
                write(2, &[1, 2]),
                step(3, 0),
            ],
            -1,
            0,
        ),
        wc(
            vec![
                step(257, 7),
                stat(0),
                Step {
                    request: Some(3),
                    ..step(1, 0)
                },
                step(3, 0),
            ],
            4,
            2,
        ),
        wc(
            vec![step(257, 7), stat(0), write(3, &[0, 1, 2]), step(3, -4)],
            4,
            2,
        ),
        wc(
            vec![
                step(257, 7),
                stat(0),
                Step {
                    request: Some(3),
                    ..step(1, -13)
                },
                step(3, -2),
            ],
            4,
            1,
        ),
    ];
    // fstat で拒否した経路では本体や probe の確保を禁止する。
    v[8].allocations = 2;
    v[9].allocations = 2;
    v[10].allocations = 2;
    v[11].allocations = 1;
    for writing in [false, true] {
        for (max, path_len, data_len, error) in
            [(-1, 1, 3, 0), (134217729, 1, 3, 0), (1, 4096, 3, 1)]
        {
            v.push(Case {
                writing,
                max,
                path_len,
                data_len,
                steps: vec![],
                error,
                io: 0,
                output: vec![],
                peak: 0,
                allocations: 0,
            });
        }
    }
    v.push(Case {
        writing: true,
        max: 2,
        path_len: 1,
        data_len: 3,
        steps: vec![],
        error: 2,
        io: 0,
        output: vec![],
        peak: 0,
        allocations: 0,
    });
    v
}
const TAIL: &str = "  intent \"故障注入駆動\"\n  requires none\n  ensures none\n";
fn syscall_mock(cases: &[Case]) -> String {
    let mut body = String::from("  STEP = STEP + 1;\n");
    for (ci, case) in cases.iter().enumerate() {
        body += &format!("  if CASE == {ci} {{\n");
        for (si, s) in case.steps.iter().enumerate() {
            body += &format!(
                "    if STEP == {} {{\n      if n != {} {{ BAD = BAD + 1; }}\n",
                si + 1,
                s.nr
            );
            if s.nr == 257 {
                let flags = if case.writing { 526913 } else { 526336 };
                body += &format!("      if a != -100 || c != {flags} {{ BAD = BAD + 1; }}\n");
            } else {
                body += "      if a != 7 { BAD = BAD + 1; }\n";
            }
            if let Some(count) = s.request {
                body += &format!("      if c != {count} {{ BAD = BAD + 1; }}\n");
            }
            if let Some((mode, size)) = s.stat {
                body += &format!("      let p = raw.int_to_addr(b);\n      raw.store_i64(raw.offset(p, 24), {mode});\n      raw.store_i64(raw.offset(p, 48), {size});\n");
            }
            for (i, byte) in s.bytes.iter().enumerate() {
                if s.nr == 0 {
                    body += &format!(
                        "      raw.store_u8(raw.offset(raw.int_to_addr(b), {i}), {byte});\n"
                    );
                } else {
                    body += &format!("      if raw.load_u8(raw.offset(raw.int_to_addr(b), {i})) != {byte} {{ BAD = BAD + 1; }}\n");
                }
            }
            body += &format!("      return {};\n    }}\n", s.result);
        }
        body += "  }\n";
    }
    // 到達時は失敗。raw.syscall は元の fs 効果を保つためだけの到達不能分岐に残す。
    body += "  BAD = BAD + 1;\n  if n == -999 { return raw.syscall(n, a, b, c, d, e, f); }\n  return -5;\n";
    format!("fn injected(n: Int, a: Int, b: Int, c: Int, d: Int, e: Int, f: Int) -> Int\n{TAIL}  effects fs\n  unchecked \"試験用。スクリプトのread/fstatだけが本体の要求した可書込み範囲に書く\"\n{{\n{body}}}\n")
}
fn driver(cases: &[Case]) -> String {
    let mut body = String::from("  let path = raw.alloc(1, 1);\n  raw.store_u8(path, 112);\n  let kind = raw.alloc(16, 8);\n  let input = raw.alloc(3, 1);\n  raw.store_u8(input, 0);\n  raw.store_u8(raw.offset(input, 1), 1);\n  raw.store_u8(raw.offset(input, 2), 2);\n  let mut failures = 0;\n");
    for (i, case) in cases.iter().enumerate() {
        body += &format!("  CASE = {i};\n  STEP = 0;\n  CURRENT = 0;\n  PEAK = 0;\n  ALLOCS = 0;\n  raw.store_i64(kind, -9);\n  raw.store_i64(raw.offset(kind, 8), -9);\n");
        if case.writing {
            body += &format!(
                "  let result{i} = write_bytes(path, {}, input, {}, {}, kind);\n",
                case.path_len, case.data_len, case.max
            );
            let want = if case.error < 0 { 0 } else { 1 };
            body += &format!("  if result{i} != {want} {{ failures = failures + 1; }}\n");
        } else {
            body += &format!(
                "  let result{i} = read_bytes(path, {}, {}, kind);\n",
                case.path_len, case.max
            );
            if case.error < 0 {
                body += &format!("  if raw.addr_to_int(result{i}) == 0 {{ failures = failures + 1; }} else {{\n    if len(result{i}) != {} {{ failures = failures + 1; }}\n", case.output.len());
                for (j, byte) in case.output.iter().enumerate() {
                    body += &format!("    if raw.load_u8(raw.offset(data(result{i}), {j})) != {byte} {{ failures = failures + 1; }}\n");
                }
                body += &format!("    free(result{i});\n  }}\n");
            } else {
                body += &format!("  if raw.addr_to_int(result{i}) != 0 {{ free(result{i}); failures = failures + 1; }}\n");
            }
        }
        if case.error >= 0 {
            body += &format!(
                "  if raw.load_i64(kind) != {} {{ failures = failures + 1; }}\n",
                case.error
            );
            if case.error == 4 {
                body += &format!(
                    "  if raw.load_i64(raw.offset(kind, 8)) != {} {{ failures = failures + 1; }}\n",
                    case.io
                );
            }
        }
        body += &format!("  if STEP != {} || CURRENT != 0 || PEAK > {} || ALLOCS > {} {{ failures = failures + 1; }}\n", case.steps.len(), case.peak, case.allocations);
    }
    body += "  raw.free(path, 1, 1);\n  raw.free(kind, 16, 8);\n  raw.free(input, 3, 1);\n  return failures + BAD;\n";
    format!("pub fn run() -> Int\n{TAIL}  effects fs\n  unchecked \"試験用。入力/kindを単独所有し、所有Bytesを成功時だけ検査して一度解放する\"\n{{\n{body}}}\n")
}
fn options() -> CheckOptions {
    let cases = cases();
    let source = include_str!("../src/stdlib/os_fs.sigil")
        .replace("raw.syscall(", "injected(")
        .replace("raw.alloc(", "tracked_alloc(")
        .replace("raw.free(", "tracked_free(");
    let helper = format!(
        r#"
global mut CASE: Int = 0
global mut STEP: Int = 0
global mut BAD: Int = 0
global mut CURRENT: Int = 0
global mut PEAK: Int = 0
global mut ALLOCS: Int = 0

fn tracked_alloc(n: Int, a: Int) -> Addr
{TAIL}  effects none
  unchecked "試験用。確保サイズを追跡する"
{{
  ALLOCS = ALLOCS + 1;
  CURRENT = CURRENT + n;
  if CURRENT > PEAK {{ PEAK = CURRENT; }}
  return raw.alloc(n, a);
}}
fn tracked_free(p: Addr, n: Int, a: Int)
{TAIL}  effects none
  unchecked "試験用。元の割当てと一致する解放を評価器が検査する"
{{
  CURRENT = CURRENT - n;
  raw.free(p, n, a);
}}
"#
    );
    let source = source
        .replacen("use std.core.bytes.from_raw;", "use std.core.bytes.data;\nuse std.core.bytes.free;\nuse std.core.bytes.from_raw;\nuse std.core.bytes.len;", 1)
        .replacen("use std.core.string.valid_utf8;", &format!("use std.core.string.valid_utf8;\n{helper}"), 1);
    CheckOptions {
        embedded_extra: vec![(
            "std.os.fs".into(),
            format!("{source}\n{}\n{}", syscall_mock(&cases), driver(&cases)),
        )],
        ..CheckOptions::default()
    }
}
const USER: &str = "module app.main\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"未使用\"\n  requires none\n  ensures none\n  effects none\n{ return 0; }\n";

#[test]
fn injected_io_contract_and_allocation_bounds_in_evaluator() {
    let c = options();
    let o = Options {
        embedded_extra: c.embedded_extra,
        ..Options::captured()
    };
    let result = sigil::ir_eval::run_sources_with(&[USER], "std.os.fs.run", &[], &o)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(result.value, "0");
}

#[test]
fn injected_io_contract_and_allocation_bounds_in_native() {
    let c = options();
    let p = sigil::ir_eval::lower_sources_opts(&[USER], "std.os.fs.run", &c).unwrap();
    sigil::verify::verify(&p).unwrap();
    let dir = scratch("binary-driver");
    let exe = dir.join("driver");
    sigil::native::build_program(&p, &sigil::codegen::Opts::default(), &exe).unwrap();
    let got = exec_env(&exe, &[], &[("SIGIL_RT_LIVE", "1")]);
    assert_eq!(got.code, Some(0), "{}", got.stderr);
    assert_eq!(got.stdout.trim(), "0");
    assert!(
        got.stderr.lines().any(|s| s == "live: sigil_blocks=0"),
        "{}",
        got.stderr
    );
    std::fs::remove_dir_all(dir).unwrap();
}

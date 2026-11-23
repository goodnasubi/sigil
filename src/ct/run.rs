//! `sigil test --ct` の本体（ADR 0061 決定 3・4 の段階 2）。
//!
//! `Sigil.toml` の `[testing.constant_time]` に書かれた外部モジュールの関数それぞれについて、
//! 静的検査（R1〜R6）と CLIF 検査を行う。
//!
//! - 対象の `name` は `<モジュール名>.<関数名>`。モジュールは `--root` からの相対パス
//!   （`a.b` → `<root>/a/b.sigil`）。
//! - 宣言した関数がモジュールにない、`public_args` に関数の仮引数にない名前がある、のはエラー。
//! - 静的検査は、対象を入口にして lowering した SIR（入口から届く関数だけ）の上で、対象の本体に
//!   テイント解析を行う。`public_args` に書かれていない仮引数は秘密。ほかの対象と標準ライブラリの
//!   定数時間の関数（`stdlib::STDLIB_TARGETS`）は、秘密を渡してよい呼び出し先として扱う。
//! - CLIF 検査は、対象を呼ぶだけの入口を SIR で足したプログラムと、何も呼ばない入口だけの
//!   プログラムをネイティブ生成し（自分自身を子として起動して `SIGIL_CLIF_DUMP` の出力を得る）、
//!   CLIF の命令数の差を、対象から届く関数すべての SIR から説明できる数と突き合わせる
//!   （方法と限界は `tests/ct_rules.rs` の冒頭を参照）。

use crate::ast::Item;
use crate::ct::clif::{count_clif, reach_branch_budget, reachable_funcs, ClifCounts};
use crate::ct::manifest::{Manifest, Target};
use crate::ct::rules::{analyze_with_table, Violation};
use crate::ct::stdlib::STDLIB_TARGETS;
use crate::ir::*;
use std::path::{Path, PathBuf};

/// 対象 1 つの検査の結果。
#[derive(Debug, Default)]
pub struct TargetReport {
    pub name: String,
    /// 検査を始められなかった理由（モジュール・関数がない、`public_args` の名前違いなど）。
    pub errors: Vec<String>,
    /// 静的検査の違反。
    pub violations: Vec<Violation>,
    /// CLIF 検査の違反の説明。
    pub clif_failures: Vec<String>,
    /// CLIF 検査の測定値（検査をしたとき）。
    pub clif: Option<ClifCounts>,
    /// CLIF 検査を飛ばした（`--no-clif`）。
    pub clif_skipped: bool,
}

impl TargetReport {
    pub fn ok(&self) -> bool {
        self.errors.is_empty() && self.violations.is_empty() && self.clif_failures.is_empty()
    }
}

/// モジュール名（`a.b`）に当たるファイル。
pub fn module_file(root: &Path, module: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    let parts: Vec<&str> = module.split('.').collect();
    for (i, seg) in parts.iter().enumerate() {
        if i + 1 == parts.len() {
            p.push(format!("{seg}.sigil"));
        } else {
            p.push(seg);
        }
    }
    p
}

/// 対象を入口にしたときの SIR 上の関数名。入口のモジュールの関数は裸の名前、ほかのモジュールは
/// `<モジュール>.<関数>`（ADR 0021 の 2-2c）。
fn sir_name(entry_module: &str, module: &str, func: &str) -> String {
    if module == entry_module {
        func.to_string()
    } else {
        format!("{module}.{func}")
    }
}

/// 対象を入口にして、`use` で届くモジュールを結んで lowering する。
/// 宣言（関数と仮引数）の確認も、ここで行う。
fn lower_target(root: &Path, t: &Target) -> Result<Program, Vec<String>> {
    let (module, func) = t.split();
    let file = module_file(root, module);
    if !file.is_file() {
        return Err(vec![format!(
            "モジュール `{module}` のファイル {} がない（`--root` からの相対パスで探す）",
            file.display()
        )]);
    }
    let opts = crate::effects::Options::default();
    let linked = crate::project::link_project(root, &file, &opts)
        .map_err(|e| vec![crate::exec::link_error(e, false).to_string()])?;
    let decl = linked.items.iter().find_map(|i| match i {
        Item::Fn(d) if d.name == func => Some(d),
        _ => None,
    });
    let Some(decl) = decl else {
        return Err(vec![format!(
            "関数 `{func}` がモジュール `{module}` にない（`Sigil.toml` の対象 `{}`）",
            t.name
        )]);
    };
    let params: Vec<&str> = decl.params.iter().map(|p| p.name.as_str()).collect();
    let unknown: Vec<String> = t
        .public_args
        .iter()
        .filter(|a| !params.contains(&a.as_str()))
        .map(|a| {
            format!(
                "`public_args` の `{a}` は関数 `{}` の仮引数にない（仮引数: {}）",
                t.name,
                if params.is_empty() {
                    "なし".to_string()
                } else {
                    params.join(", ")
                }
            )
        })
        .collect();
    if !unknown.is_empty() {
        return Err(unknown);
    }
    crate::ir_eval::lower_project(root, &file, func, &[]).map_err(|e| vec![e.to_string()])
}

/// 静的検査の対象表: 標準ライブラリの定数時間の関数と、マニフェストの対象すべて。
/// `entry_module` は、いま入口にしているモジュール（SIR の名前の付き方が変わる）。
fn static_table(
    manifest: &Manifest,
    entry_module: &str,
    shadowed: &[String],
) -> Vec<(String, Vec<String>)> {
    let mut table: Vec<(String, Vec<String>)> = STDLIB_TARGETS
        .iter()
        // 入口のモジュールが同じ裸の名前の関数を自分で定義しているなら、標準ライブラリの
        // メソッドの名前とは別物なので、表に入れない。
        .filter(|(n, _)| !shadowed.iter().any(|s| s == n))
        .map(|(n, p)| (n.to_string(), p.iter().map(|s| s.to_string()).collect()))
        .collect();
    for t in &manifest.targets {
        let (m, f) = t.split();
        table.push((sir_name(entry_module, m, f), t.public_args.clone()));
    }
    table
}

/// 静的検査（R1〜R6）。
fn static_check(root: &Path, manifest: &Manifest, t: &Target, program: &Program) -> Vec<Violation> {
    let (module, func) = t.split();
    let own = sir_name(module, module, func);
    // 入口のモジュールの最上位の関数と衝突する標準ライブラリの裸の名前を外す。
    let shadowed = top_level_fn_names(root, module);
    let table = static_table(manifest, module, &shadowed);
    let refs: Vec<(&str, Vec<&str>)> = table
        .iter()
        .map(|(n, p)| (n.as_str(), p.iter().map(|s| s.as_str()).collect()))
        .collect();
    let slices: Vec<(&str, &[&str])> = refs.iter().map(|(n, p)| (*n, p.as_slice())).collect();
    analyze_with_table(program, &slices)
        .into_iter()
        .filter(|(f, _)| *f == own)
        .map(|(_, v)| v)
        .collect()
}

fn top_level_fn_names(root: &Path, module: &str) -> Vec<String> {
    let file = module_file(root, module);
    let Ok(src) = std::fs::read_to_string(&file) else {
        return vec![];
    };
    match crate::parse(&src) {
        Ok(f) => f
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Fn(d) => Some(d.name.clone()),
                _ => None,
            })
            .collect(),
        Err(_) => vec![],
    }
}

// ---------------------------------------------------------------------------
// CLIF 検査
// ---------------------------------------------------------------------------

/// 何も呼ばない入口だけのプログラム（CLIF の差分の基準）。
const BASELINE_SRC: &str = "module ctbase\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 0;\n}\n";

/// 対象を呼ぶだけの入口 `main` を足す。引数は未初期化の局所をそのまま渡す（命令数だけを見るので
/// 実行はしない）。基準のプログラムの入口と同じ形にして、差分が対象の分だけになるようにする。
fn with_call_entry(mut p: Program, target: &str) -> Result<Program, String> {
    let tid = p
        .funcs
        .iter()
        .position(|f| f.name == target)
        .ok_or_else(|| format!("lowering の結果に {target} がない"))?;
    let callee = p.funcs[tid].clone();
    let mut locals = vec![Local {
        name: "_ret".into(),
        ty: Ty::Int,
    }];
    let mut args = vec![];
    for &pi in &callee.params {
        let l = &callee.locals[pi];
        locals.push(Local {
            name: l.name.clone(),
            ty: l.ty.clone(),
        });
        let place = Place {
            base: locals.len() - 1,
            proj: vec![],
        };
        args.push(
            if matches!(l.ty, Ty::Int | Ty::Float | Ty::Bool | Ty::Addr)
                || matches!(l.ty, Ty::Ref(_) | Ty::MutRef(_))
            {
                Operand::Copy(place)
            } else {
                Operand::Move(place)
            },
        );
    }
    locals.push(Local {
        name: "_dest".into(),
        ty: callee.ret.clone(),
    });
    let dest = Place {
        base: locals.len() - 1,
        proj: vec![],
    };
    p.stop_sites.push(StopSite {
        kind: crate::exec::StopKind::StackExhausted,
        func: "main".into(),
        line: 5,
        during: None,
        detail: String::new(),
    });
    let stop = p.stop_sites.len() - 1;
    let entry = Func {
        name: "main".into(),
        params: vec![],
        ret: Ty::Int,
        locals,
        blocks: vec![
            Block {
                stmts: vec![Stmt::Assign(
                    Place {
                        base: 0,
                        proj: vec![],
                    },
                    Rvalue::Use(Operand::Const(Const::Int(0))),
                )],
                term: Term::Call {
                    func: Callee::Func(tid),
                    args,
                    dest,
                    next: 1,
                    cleanup: 2,
                    stop,
                },
            },
            Block {
                stmts: vec![],
                term: Term::Return,
            },
            Block {
                stmts: vec![],
                term: Term::Goto(1),
            },
        ],
    };
    p.funcs.push(entry);
    p.entry = p.funcs.len() - 1;
    crate::verify::verify(&p)
        .map_err(|e| format!("入口の SIR が検証器を通らない: {}", e.join("; ")))?;
    Ok(p)
}

/// 子のプロセス（`sigil test --ct --emit-clif …`）の本体。CLIF は `SIGIL_CLIF_DUMP` で標準エラーに出る。
/// `target` が `None` なら基準のプログラム。
pub fn emit_clif(root: &Path, target: Option<&Target>) -> Result<(), String> {
    let program = match target {
        None => {
            crate::ir_eval::lower_sources(&[BASELINE_SRC], "main").map_err(|e| e.to_string())?
        }
        Some(t) => {
            let p = lower_target(root, t).map_err(|e| e.join("; "))?;
            let (m, f) = t.split();
            with_call_entry(p, &sir_name(m, m, f))?
        }
    };
    crate::codegen::compile(&program, &crate::codegen::Opts::default())
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))
}

fn child_dump(root: &Path, target: Option<&Target>) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("自分自身の場所を得られない: {e}"))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["test", "--ct", "--emit-clif", "--root"])
        .arg(root)
        .arg(target.map(|t| t.name.as_str()).unwrap_or("-"))
        .env("SIGIL_CLIF_DUMP", "1");
    let out = cmd
        .output()
        .map_err(|e| format!("CLIF 検査の子プロセスを起動できない: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stdout).into_owned();
        return Err(format!("CLIF の生成に失敗: {}", msg.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
}

fn clif_check(
    root: &Path,
    t: &Target,
    program: &Program,
    base: &ClifCounts,
    rep: &mut TargetReport,
) {
    let dump = match child_dump(root, Some(t)) {
        Ok(d) => d,
        Err(e) => {
            rep.errors.push(e);
            return;
        }
    };
    let with = count_clif(&dump);
    let delta = ClifCounts {
        branch: with.branch.saturating_sub(base.branch),
        div: with.div.saturating_sub(base.div),
        store: with.store.saturating_sub(base.store),
    };
    let (m, f) = t.split();
    let own = sir_name(m, m, f);
    let reach = reachable_funcs(program, &own);
    let sir_div = reach
        .iter()
        .flat_map(|id| program.funcs[*id].blocks.iter())
        .filter(|b| {
            matches!(
                b.term,
                Term::CheckedOp {
                    op: ArithOp::Div | ArithOp::Rem,
                    ..
                }
            )
        })
        .count();
    // 対象を呼ぶ入口の呼び出し（異常停止の伝播を含めて 2 つ）の分。
    let budget = reach_branch_budget(program, &reach) + 2;
    rep.clif = Some(delta);
    if delta.div > sir_div {
        rep.clif_failures.push(format!(
            "CLIF の除算・剰余命令 {} が SIR の除算・剰余 {sir_div} を超えた（生成が足した除算がある）",
            delta.div
        ));
    }
    if delta.branch > budget {
        rep.clif_failures.push(format!(
            "CLIF の条件分岐 {} が SIR から説明できる数 {budget} を超えた（秘密に依存する分岐が生成された疑い）",
            delta.branch
        ));
    }
}

/// マニフェストの対象すべてを検査する。`clif` が偽なら CLIF 検査を飛ばす。
pub fn run(root: &Path, manifest: &Manifest, clif: bool) -> Vec<TargetReport> {
    let mut base: Option<Result<ClifCounts, String>> = None;
    let mut out = vec![];
    for t in &manifest.targets {
        let mut rep = TargetReport {
            name: t.name.clone(),
            clif_skipped: !clif,
            ..Default::default()
        };
        match lower_target(root, t) {
            Err(es) => rep.errors.extend(es),
            Ok(program) => {
                rep.violations = static_check(root, manifest, t, &program);
                if clif {
                    let b =
                        base.get_or_insert_with(|| child_dump(root, None).map(|d| count_clif(&d)));
                    match b {
                        Ok(b) => clif_check(root, t, &program, &b.clone(), &mut rep),
                        Err(e) => rep.errors.push(e.clone()),
                    }
                }
            }
        }
        out.push(rep);
    }
    out
}

/// 結果を利用者向けの行にする。
pub fn render(reports: &[TargetReport]) -> String {
    let mut s = String::new();
    for r in reports {
        if r.ok() {
            s.push_str(&format!("{}: ok", r.name));
            if r.clif_skipped {
                s.push_str("（静的検査のみ）");
            } else if let Some(c) = r.clif {
                s.push_str(&format!("（CLIF: 分岐 {}・除算 {}）", c.branch, c.div));
            }
            s.push('\n');
            continue;
        }
        s.push_str(&format!("{}: 失敗\n", r.name));
        for e in &r.errors {
            s.push_str(&format!("  エラー: {e}\n"));
        }
        for v in &r.violations {
            s.push_str(&format!(
                "  {} 基本ブロック {}: {}\n",
                v.rule.code(),
                v.block,
                v.detail
            ));
        }
        for f in &r.clif_failures {
            s.push_str(&format!("  CLIF: {f}\n"));
        }
    }
    s
}

/// `sigil test --ct`（別名 `sigil verify --ct`）の引数の解釈と実行。`args` は `test`／`verify` の
/// 次から。終了コードは、すべて通れば 0、検査の失敗・エラーなら 1、使い方の誤りなら 2。
pub fn cli(args: &[String]) -> u8 {
    const USAGE: &str = "使い方: sigil test --ct [--root <根>] [--manifest <Sigil.toml>] [--no-clif]\n  Sigil.toml の [testing.constant_time] の対象を、静的検査（R1〜R6）と CLIF 検査にかける。--root の既定は現在のディレクトリ、--manifest の既定は <根>/Sigil.toml。--no-clif は CLIF 検査を飛ばす";
    let mut ct = false;
    let mut clif = true;
    let mut root: Option<PathBuf> = None;
    let mut manifest: Option<PathBuf> = None;
    let mut emit: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ct" => ct = true,
            "--no-clif" => clif = false,
            "--root" if i + 1 < args.len() => {
                root = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            "--manifest" if i + 1 < args.len() => {
                manifest = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
            // 内部用: CLIF 検査の子プロセス。`--root` の次に対象名（基準は `-`）。
            "--emit-clif" => emit = Some(String::new()),
            a if emit.as_deref() == Some("") && !a.starts_with("--") => emit = Some(a.to_string()),
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
        i += 1;
    }
    if !ct {
        eprintln!("{USAGE}");
        return 2;
    }
    let root =
        root.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let path = manifest.unwrap_or_else(|| root.join("Sigil.toml"));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: 読めない: {e}", path.display());
            return 1;
        }
    };
    let m = match crate::ct::manifest::parse(&text) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if let Some(name) = emit {
        let target = m.targets.iter().find(|t| t.name == name);
        if name != "-" && target.is_none() {
            println!("対象 {name} がマニフェストにない");
            return 1;
        }
        return match emit_clif(&root, target) {
            Ok(()) => 0,
            Err(e) => {
                println!("{e}");
                1
            }
        };
    }
    if m.targets.is_empty() {
        eprintln!(
            "{}: [testing.constant_time] の targets がない（検査する対象がない）",
            path.display()
        );
        return 1;
    }
    let reports = run(&root, &m, clif);
    print!("{}", render(&reports));
    let bad = reports.iter().filter(|r| !r.ok()).count();
    if bad == 0 {
        println!("定数時間の検査: {} 件すべて通った", reports.len());
        0
    } else {
        eprintln!("定数時間の検査: {} 件中 {bad} 件が失敗", reports.len());
        1
    }
}

//! 定数時間検査の土台（ADR 0024 決定 3、ADR 0020 決定 1。PR 6-1a）。
//!
//! 暗号の部品は、秘密の値によって実行時間が変わらないこと（定数時間）が要る。
//! このファイルは、その性質を機械的に確かめる 3 層のうち、テストスイートで
//! 常時動かす 2 層を実装する。
//!
//! 1. **対象表と漏れの検査**: 定数時間であるべき関数は `CT_FUNCTIONS` に
//!    明示する。ADR 0024 決定 3 の包括基準（`ct_` 接頭辞／`Secret` を含む
//!    署名／`std.crypto` の関数）に当たるのに対象表にない関数があれば、
//!    テストを失敗にする（載せ忘れの防止）。
//! 2. **静的検査 `ct_rules_static`**: lowering 後の SIR の上で、秘密の値の
//!    流れ（テイント）を追い、ADR 0020 決定 1 の規則 R1〜R6 への違反を
//!    検出する。
//! 3. **CLIF 検査 `ct_rules_clif`**: ネイティブ生成の途中の中間表現
//!    （`SIGIL_CLIF_DUMP`）に、除算命令が現れていないか、条件分岐が SIR の
//!    構造から説明できる数を超えていないかを確かめる。
//!
//! 統計的な計測（第 3 層）は `tests/ct_measure.rs`（`#[ignore]`、手動実行）。
//!
//! ## テイントの付け方の約束（モデリング）
//!
//! - 対象表で「公開」と書かれていない引数は、すべて秘密とする。
//! - **長さは公開情報**とする。`len`・`is_empty` の結果は、秘密の領域に
//!   対しても公開の値として扱う（ADR 0020 決定 1 の対象表のとおり）。
//! - **領域を経由する流れ**（ADR 0020 の必須事項）: 秘密の値を書いた
//!   `Bytes`・一覧・構造体などの領域から読んだ値は、読んだ位置によらず
//!   秘密とする。位置ごとに区別して追うことはしない。実装の上では、
//!   局所（領域の根）単位で「秘密を含む」を伝播させる。
//! - **raw 層の約束**: 標準ライブラリの `Bytes` の頭は「データ番地＋長さ」
//!   の 2 ワードで、`raw.load_i64`・`raw.load_addr` はその頭のワード
//!   （長さと番地＝公開の情報）を読むために使う。したがって、この 2 つの
//!   結果は公開とする。一方 `raw.load_u8` は中身のバイト列を読むので、
//!   秘密の領域から読んだ結果は秘密とする。将来、i64 の並び自体を秘密の
//!   中身として `load_i64` で読む関数が現れたら、この約束の見直しが要る。
//! - 比較の結果（`ct_eq` の戻り値など）は公開の値とする（ADR 0020）。
//!   ただし定数時間の関数の**中**で秘密由来の比較結果を分岐に使うことは、
//!   R1 として扱う（公開になるのは関数の外へ出た後）。
//!
//! ## CLIF 検査の方法と限界
//!
//! 現在のネイティブ生成は Cranelift の最適化水準が `none` で、組み立てた
//! CLIF がそのまま機械語になる。つまり「最適化後の IR」は現状では
//! 「組み立てた CLIF」と同じものである。CLIF の出力には関数名が付かない
//! ため、対象関数の寄与は差分で切り分ける: 入口が対象関数を呼ぶプログラム
//! と、呼ばないプログラムの両方を組み立て、命令数の差を対象関数の分と
//! みなす。対象関数がユーザー定義の関数を呼ばない場合にだけ、この切り
//! 分けは正確になる（このファイルの fixture はその形にしてある）。
//! 標準ライブラリの関数（`std.core.bytes.ct_equal`）については、静的検査
//! で全面を覆い、CLIF 検査は入口から直接呼ぶ形のプログラムで差分が
//! 1 関数分に収まることを確かめたうえで適用する。
//! 最適化水準を上げる変更を入れる場合は、この検査の前提（組み立てた
//! CLIF＝最終形）が崩れるので、ADR 0020 が検討項目とした「分岐の条件の
//! 出所を最適化後の IR で追う」方法への置き換えを検討すること。

use sigil::ir::*;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command;

// ---------------------------------------------------------------------------
// 対象表（ADR 0024 決定 3）
// ---------------------------------------------------------------------------

/// 定数時間であるべき関数の表。(関数名, 公開の引数名の一覧)。
/// ここにない引数はすべて秘密として扱う。
const CT_FUNCTIONS: &[(&str, &[&str])] = &[
    // 既にある実装（ADR 0023）。引数 a・b の中身が秘密、長さは公開。
    ("std.core.bytes.ct_equal", &[]),
    // `Secret` の入口（PR 6-1b）。メソッドの SIR 名は裸の名前になる。
    // `from_bytes` の引数は取り込む秘密、`expose` は明示的な取り出し
    // （秘密を返すこと自体は違反ではない）、`len` は長さ（公開）だけを見る。
    ("from_bytes", &[]),
    ("expose", &[]),
    ("len", &[]),
    // SHA-256（PR 6-1c で実装）。ワンショット `sha256` の引数 `data` の
    // 中身は秘密、長さは公開（解析のモデリングで BytesLen は公開になる）。
    // 圧縮 `sha256_compress` は、公開なのはブロックの数 `block_count`
    // だけ（ループ回数・添字はそこから派生する。ADR 0024 決定 3 の表）。
    ("std.crypto.sha256", &[]),
    ("std.crypto.sha256_compress", &["block_count"]),
    // ADR 0024 の部品（PR 6-1d で実装される。先に載せておき、実装が
    // 現れた時点で自動で検査の対象になる）。
    // なお std.crypto の比較は言語の自由関数 `ct_eq` が担い（ADR 0024 決定 1）、
    // 実体は上の `std.core.bytes.ct_equal` なので、ここには別に載せない。
    ("std.crypto.hmac_sha256", &[]),
    ("std.crypto.verify_hmac_sha256", &[]),
    // 塩・回数・出力の長さは公開（ADR 0020 決定 1 の対象表）。
    (
        "std.crypto.pbkdf2_sha256",
        &["salt", "iterations", "dk_len"],
    ),
];

/// 対象表のうち、まだ実装が存在しない行（先行登録）。
/// 実装が現れたら、ここから外して通常の行として扱う。
const CT_AHEAD: &[&str] = &[
    "std.crypto.hmac_sha256",
    "std.crypto.verify_hmac_sha256",
    "std.crypto.pbkdf2_sha256",
];

// ---------------------------------------------------------------------------
// 違反の表現
// ---------------------------------------------------------------------------

/// ADR 0020 決定 1 の静的な規則。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    /// R1 分岐: 秘密の値を分岐・条件に使う。
    R1Branch,
    /// R2 番地: 秘密の値を添字・番地・大きさの計算に使う。
    R2Index,
    /// R3 割り算: 秘密の値を `/`・`%` の左右に置く。
    R3Div,
    /// R4 呼び出し: 定数時間でない関数へ秘密の値を渡す。
    R4Call,
    /// R5 演算: 許された演算以外の演算に秘密の値を使う。
    R5Op,
    /// R6 外への出力: 秘密の値を出力・記録に渡す。
    R6Output,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Violation {
    rule: Rule,
    block: usize,
    detail: String,
}

fn viol(rule: Rule, block: usize, detail: impl Into<String>) -> Violation {
    Violation {
        rule,
        block,
        detail: detail.into(),
    }
}

// ---------------------------------------------------------------------------
// テイント解析（ct_rules_static の中核）
// ---------------------------------------------------------------------------

/// 値の出所が秘密かどうかを、オペランドについて答える。
fn operand_taint(state: &[bool], op: &Operand) -> bool {
    match op {
        Operand::Copy(p) | Operand::Move(p) => place_taint(state, p),
        Operand::Const(_) => false,
    }
}

/// 場所（局所＋射影）の読みは、根の局所が秘密を含めば秘密とする。
fn place_taint(state: &[bool], p: &Place) -> bool {
    state.get(p.base).copied().unwrap_or(false)
}

fn any_taint(state: &[bool], ops: &[Operand]) -> bool {
    ops.iter().any(|o| operand_taint(state, o))
}

/// `program` の中の対象表にある関数をすべて解析し、違反を集める。
/// `table` は (関数名, 公開の引数名) の列。表にあってプログラムにない
/// 関数は対象にしない（先行登録の行のため）。
fn analyze_with_table(program: &Program, table: &[(&str, &[&str])]) -> Vec<(String, Violation)> {
    let ct_names: BTreeSet<&str> = table.iter().map(|(name, _)| *name).collect();
    let mut out = Vec::new();
    for func in &program.funcs {
        let Some((_, public)) = table.iter().find(|(name, _)| *name == func.name) else {
            continue;
        };
        let violations = analyze_func(program, func, public, &ct_names);
        out.extend(violations.into_iter().map(|v| (func.name.clone(), v)));
    }
    out
}

fn analyze_func(
    program: &Program,
    func: &Func,
    public: &[&str],
    ct_names: &BTreeSet<&str>,
) -> Vec<Violation> {
    let n = func.locals.len();
    // 引数の初期テイント: 公開と書かれていない引数は秘密。
    let mut init = vec![false; n];
    for &param in &func.params {
        let name = &func.locals[param].name;
        if !public.contains(&name.as_str()) {
            init[param] = true;
        }
    }
    // 基本ブロック入口の状態を、伝播が止まるまで回す前向き解析。
    let mut entry: Vec<Vec<bool>> = func.blocks.iter().map(|_| vec![false; n]).collect();
    if !func.blocks.is_empty() {
        entry[0] = init;
    }
    let mut violations: Vec<Violation> = Vec::new();
    for _ in 0..64 {
        let mut changed = false;
        let mut round: Vec<Violation> = Vec::new();
        for (bi, block) in func.blocks.iter().enumerate() {
            let mut state = entry[bi].clone();
            for stmt in &block.stmts {
                apply_stmt(&mut state, stmt, bi, &mut round);
            }
            apply_term(
                &Ctx { program, ct_names },
                &mut state,
                &block.term,
                bi,
                &mut round,
            );
            for succ in block.term.succs() {
                if succ < entry.len() {
                    for i in 0..n {
                        if state[i] && !entry[succ][i] {
                            entry[succ][i] = true;
                            changed = true;
                        }
                    }
                }
            }
        }
        violations = round;
        if !changed {
            break;
        }
    }
    violations.sort_by_key(|v| (v.block, v.rule));
    violations.dedup();
    violations
}

fn apply_stmt(state: &mut [bool], stmt: &Stmt, bi: usize, out: &mut Vec<Violation>) {
    match stmt {
        Stmt::Assign(place, rvalue) => {
            let t = rvalue_taint(state, rvalue, bi, out);
            set_place_taint(state, place, t);
        }
        Stmt::Drop(_) => {}
    }
}

/// 代入先のテイントを決める。射影つき（領域への書き込み）は根に足し、
/// 局所そのものへの代入は置き換える。
fn set_place_taint(state: &mut [bool], place: &Place, t: bool) {
    if place.base >= state.len() {
        return;
    }
    if place.proj.is_empty() {
        state[place.base] = t;
    } else if t {
        state[place.base] = true;
    }
}

fn rvalue_taint(state: &[bool], rv: &Rvalue, bi: usize, out: &mut Vec<Violation>) -> bool {
    match rv {
        Rvalue::Use(op) => operand_taint(state, op),
        Rvalue::Unary(op, x) => {
            let t = operand_taint(state, x);
            if t && matches!(op, UnOp::NegFloat) {
                out.push(viol(Rule::R5Op, bi, "秘密の値への浮動小数点の符号反転"));
            }
            t
        }
        Rvalue::Binary(op, a, b) => {
            let (ta, tb) = (operand_taint(state, a), operand_taint(state, b));
            if matches!(op, BinOp::Div | BinOp::Rem) && (ta || tb) {
                out.push(viol(Rule::R3Div, bi, "秘密の値の割り算・剰余"));
            }
            ta || tb
        }
        Rvalue::Aggregate(_, ops) => any_taint(state, ops),
        Rvalue::Ref(p) | Rvalue::RefMut(p) => place_taint(state, p),
        Rvalue::Clone(op, _) => operand_taint(state, op),
        Rvalue::Discriminant(op) => operand_taint(state, op),
        Rvalue::MakeClosure(_, ops) => any_taint(state, ops),
        Rvalue::GlobalRef(_) => false,
    }
}

/// 解析の読み取り専用の入力（プログラムと対象表の名前集合）。
struct Ctx<'a> {
    program: &'a Program,
    ct_names: &'a BTreeSet<&'a str>,
}

fn apply_term(ctx: &Ctx, state: &mut [bool], term: &Term, bi: usize, out: &mut Vec<Violation>) {
    match term {
        Term::Goto(_) | Term::Return | Term::Resume | Term::Unreachable => {}
        Term::Branch(cond, _, _) | Term::Assert { cond, .. } => {
            if operand_taint(state, cond) {
                out.push(viol(Rule::R1Branch, bi, "秘密の値による分岐・表明"));
            }
        }
        Term::Switch(cond, _, _) => {
            if operand_taint(state, cond) {
                out.push(viol(Rule::R1Branch, bi, "秘密の値による振り分け"));
            }
        }
        Term::CheckedOp { op, a, b, dest, .. } => {
            let (ta, tb) = (operand_taint(state, a), operand_taint(state, b));
            if matches!(op, ArithOp::Div | ArithOp::Rem) && (ta || tb) {
                out.push(viol(
                    Rule::R3Div,
                    bi,
                    "秘密の値の割り算・剰余（検査つき演算）",
                ));
            }
            set_place_taint(state, dest, ta || tb);
        }
        Term::Call {
            func: callee,
            args,
            dest,
            ..
        } => apply_call(ctx, state, callee, args, dest, bi, out),
    }
}

fn apply_call(
    ctx: &Ctx,
    state: &mut [bool],
    callee: &Callee,
    args: &[Operand],
    dest: &Place,
    bi: usize,
    out: &mut Vec<Violation>,
) {
    let arg_taint: Vec<bool> = args.iter().map(|a| operand_taint(state, a)).collect();
    let any = arg_taint.iter().any(|t| *t);
    match callee {
        Callee::Builtin(b) => apply_builtin(state, *b, args, &arg_taint, dest, bi, out),
        Callee::Func(id) => {
            let name = &ctx.program.funcs[*id].name;
            let is_ct = ctx.ct_names.contains(name.as_str());
            if any && !is_ct {
                out.push(viol(
                    Rule::R4Call,
                    bi,
                    format!("定数時間でない関数 {name} へ秘密の値を渡した"),
                ));
            }
            set_place_taint(state, dest, any);
        }
        Callee::Indirect(op) => {
            if any {
                out.push(viol(
                    Rule::R4Call,
                    bi,
                    "関数の値への呼び出しに秘密の値を渡した",
                ));
            }
            set_place_taint(state, dest, any || operand_taint(state, op));
        }
    }
}

/// 組み込み操作の分類。戻り値は「結果が秘密かどうか」。
fn apply_builtin(
    state: &mut [bool],
    b: Builtin,
    args: &[Operand],
    arg_taint: &[bool],
    dest: &Place,
    bi: usize,
    out: &mut Vec<Violation>,
) {
    let any = arg_taint.iter().any(|t| *t);
    // 添字・番地・大きさに秘密を使っていないか（R2）を先に確かめるための道具。
    let index_check = |idx: &[bool], out: &mut Vec<Violation>| {
        if idx.iter().any(|t| *t) {
            out.push(viol(
                Rule::R2Index,
                bi,
                "秘密の値を添字・番地の計算に使った",
            ));
        }
    };
    fn tail(t: &[bool], from: usize) -> &[bool] {
        if from >= t.len() {
            &[]
        } else {
            &t[from..]
        }
    }
    let region_root = |op: &Operand| -> Option<usize> {
        match op {
            Operand::Copy(p) | Operand::Move(p) => Some(p.base),
            Operand::Const(_) => None,
        }
    };
    match b {
        // 長さ・有無の問い合わせ: 結果は公開（長さは公開情報）。
        Builtin::ListLen
        | Builtin::ListIsEmpty
        | Builtin::BytesLen
        | Builtin::BytesIsEmpty
        | Builtin::StrLen
        | Builtin::StrIsEmpty
        | Builtin::OptIsSome
        | Builtin::OptIsNone
        | Builtin::ResIsOk
        | Builtin::ResIsErr
        | Builtin::IntMinValue => set_place_taint(state, dest, false),
        // 定数時間の比較そのもの: 結果は公開。
        Builtin::CtEq => set_place_taint(state, dest, false),
        // 領域からの読み: 添字が秘密なら R2。結果は領域の秘密を引き継ぐ。
        Builtin::ListGet | Builtin::ListGetMut | Builtin::BytesGet | Builtin::BytesReadU32Be => {
            index_check(tail(arg_taint, 1), out);
            set_place_taint(state, dest, arg_taint.first().copied().unwrap_or(false));
        }
        Builtin::BytesSlice => {
            index_check(tail(arg_taint, 1), out);
            set_place_taint(state, dest, arg_taint.first().copied().unwrap_or(false));
        }
        // 領域への書き込み: 値が秘密なら領域が秘密を含むようになる。
        // 添字が秘密なら R2。
        Builtin::ListPush | Builtin::BytesPush => {
            if arg_taint.get(1).copied().unwrap_or(false) {
                if let Some(root) = args.first().and_then(region_root) {
                    if root < state.len() {
                        state[root] = true;
                    }
                }
            }
            set_place_taint(state, dest, false);
        }
        Builtin::BytesSet => {
            if arg_taint.get(1).copied().unwrap_or(false) {
                out.push(viol(Rule::R2Index, bi, "秘密の値を添字の計算に使った"));
            }
            if arg_taint.get(2).copied().unwrap_or(false) {
                if let Some(root) = args.first().and_then(region_root) {
                    if root < state.len() {
                        state[root] = true;
                    }
                }
            }
            set_place_taint(state, dest, false);
        }
        Builtin::BytesExtend | Builtin::BytesConcat => {
            set_place_taint(state, dest, any);
            if any {
                if let Some(root) = args.first().and_then(region_root) {
                    if root < state.len() {
                        state[root] = true;
                    }
                }
            }
        }
        Builtin::BytesPushU16Be
        | Builtin::BytesPushU32Be
        | Builtin::BytesPushU64Be
        | Builtin::BytesPushU32Le => {
            if arg_taint.get(1).copied().unwrap_or(false) {
                if let Some(root) = args.first().and_then(region_root) {
                    if root < state.len() {
                        state[root] = true;
                    }
                }
            }
            set_place_taint(state, dest, false);
        }
        // ビット演算・変換の部品: 秘密を引き継ぐ（R5 で許された演算）。
        Builtin::IntRotl
        | Builtin::IntRotr
        | Builtin::IntRotr32
        | Builtin::IntAshr
        | Builtin::ToFloat
        | Builtin::ToInt
        | Builtin::FloatToBits
        | Builtin::FloatFromBits
        | Builtin::StrAsBytes
        | Builtin::OptUnwrapOr
        | Builtin::OptMap
        | Builtin::ResUnwrapOr
        | Builtin::ResOk
        | Builtin::ResMap
        | Builtin::ResMapErr
        | Builtin::OptOkOr
        | Builtin::Compare(_) => set_place_taint(state, dest, any),
        // 外への出力: 秘密を渡したら R6。
        Builtin::ConsoleLog | Builtin::FsWrite => {
            if any {
                out.push(viol(Rule::R6Output, bi, "秘密の値を出力・書き込みに渡した"));
            }
            set_place_taint(state, dest, false);
        }
        // raw 層（番地の計算と読み書き）。頭の約束はファイル冒頭の説明のとおり。
        Builtin::RawOffset => {
            index_check(tail(arg_taint, 1), out);
            set_place_taint(state, dest, arg_taint.first().copied().unwrap_or(false));
        }
        Builtin::RawLoadU8 => {
            // 中身のバイト: 番地の指す領域が秘密なら結果も秘密。
            set_place_taint(state, dest, arg_taint.first().copied().unwrap_or(false));
        }
        Builtin::RawLoadI64 => {
            // 頭のワード（長さ）: 公開。
            set_place_taint(state, dest, false);
        }
        Builtin::RawLoadAddr => {
            // 頭のワード（データ番地）。指す先は元の領域なので、領域の
            // 秘密は引き継ぐ（この番地で load_u8 した結果が秘密になる）。
            set_place_taint(state, dest, arg_taint.first().copied().unwrap_or(false));
        }
        Builtin::RawStoreU8 | Builtin::RawStoreI64 | Builtin::RawStoreAddr => {
            set_place_taint(state, dest, false);
        }
        Builtin::RawCopy | Builtin::RawSet => {
            // 大きさ（第 3 引数）が秘密なら R2。
            index_check(tail(arg_taint, 2), out);
            if arg_taint.get(1).copied().unwrap_or(false) {
                if let Some(root) = args.first().and_then(region_root) {
                    if root < state.len() {
                        state[root] = true;
                    }
                }
            }
            set_place_taint(state, dest, false);
        }
        Builtin::RawAlloc => {
            index_check(arg_taint, out);
            set_place_taint(state, dest, false);
        }
        Builtin::RawSyscall => {
            if any {
                out.push(viol(Rule::R6Output, bi, "秘密の値を syscall に渡した"));
            }
            set_place_taint(state, dest, false);
        }
        Builtin::RawAddrNull | Builtin::RawAddrToInt | Builtin::RawIntToAddr => {
            set_place_taint(state, dest, any);
        }
        // 委ねる（`task.yield` は引数なしで R4 に当たらない。ADR 0020）。
        Builtin::TaskYield => set_place_taint(state, dest, false),
        // それ以外: 秘密を渡すのは R4（ホワイトリストにない呼び出し）。
        _ => {
            if any {
                out.push(viol(
                    Rule::R4Call,
                    bi,
                    format!("組み込み操作 {b:?} へ秘密の値を渡した"),
                ));
            }
            set_place_taint(state, dest, any);
        }
    }
}

// ---------------------------------------------------------------------------
// 対象表の検査（漏れの防止）
// ---------------------------------------------------------------------------

/// 標準ライブラリを含むプログラム（コーパス）を 1 つ組み立てる。
/// 入口から届かない関数も含めて、標準ライブラリの関数がすべて SIR に現れる。
fn corpus() -> Program {
    let src = "module ctprobe\n  targets server\n  trust none\n\n\
               fn main() -> Int\n  intent \"探査\"\n  requires none\n  ensures none\n  effects none\n{\n  \
               let a = Bytes.new();\n  let b = Bytes.new();\n  \
               let s = Secret.from_bytes(a);\n  let inner = s.expose();\n  \
               let d = sha256(&b);\n  \
               if ct_eq(&inner, &b) {\n    return s.len() + d.len();\n  }\n  return 0;\n}\n";
    sigil::ir_eval::lower_sources(&[src], "main").expect("コーパスの組み立てに失敗")
}

/// ADR 0024 決定 3 の包括基準に当たる関数かどうか。
fn is_ct_candidate(program: &Program, func: &Func) -> bool {
    let last = func.name.rsplit('.').next().unwrap_or(&func.name);
    if last.starts_with("ct_") {
        return true;
    }
    if func.name.starts_with("std.crypto.") {
        return true;
    }
    // 引数・戻り値に `Secret` 型を含む。
    let mentions_secret = |ty: &Ty| -> bool {
        let mut t = ty;
        loop {
            match t {
                Ty::Adt(id) => {
                    // 秘密かどうかは lower が付けた同一性の印で見る
                    // （名前で比べると利用者の同名の型を拾ってしまう）。
                    return program.types.get(*id).map(|d| d.secret).unwrap_or(false);
                }
                Ty::Ref(inner) | Ty::MutRef(inner) => t = inner,
                _ => return false,
            }
        }
    };
    func.params
        .iter()
        .any(|&l| mentions_secret(&func.locals[l].ty))
        || mentions_secret(&func.ret)
}

#[test]
fn table_is_well_formed() {
    let names: Vec<&str> = CT_FUNCTIONS.iter().map(|(n, _)| *n).collect();
    let set: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), set.len(), "対象表に関数名の重なりがある");
    for ahead in CT_AHEAD {
        assert!(set.contains(ahead), "先行登録 {ahead} が対象表にない");
    }
}

#[test]
fn completeness_no_missing_targets() {
    let program = corpus();
    let table: BTreeSet<&str> = CT_FUNCTIONS.iter().map(|(n, _)| *n).collect();
    let mut missing = Vec::new();
    for func in &program.funcs {
        if is_ct_candidate(&program, func) && !table.contains(func.name.as_str()) {
            missing.push(func.name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "包括基準に当たるのに対象表にない関数がある: {missing:?}"
    );
    // 対象表の行のうち、実装が存在するものは、公開の引数名が実在することを確かめる。
    for (name, public) in CT_FUNCTIONS {
        if CT_AHEAD.contains(name) {
            continue;
        }
        let func = program
            .funcs
            .iter()
            .find(|f| f.name == *name)
            .unwrap_or_else(|| panic!("対象表の {name} がコーパスに見つからない"));
        let params: BTreeSet<&str> = func
            .params
            .iter()
            .map(|&l| func.locals[l].name.as_str())
            .collect();
        for p in *public {
            assert!(params.contains(p), "{name} の公開引数 {p} が実在しない");
        }
    }
}

// ---------------------------------------------------------------------------
// 静的検査: 実在の定数時間関数
// ---------------------------------------------------------------------------

#[test]
fn static_real_targets_are_clean() {
    let program = corpus();
    let violations = analyze_with_table(&program, CT_FUNCTIONS);
    assert!(
        violations.is_empty(),
        "定数時間の関数に規則違反がある: {violations:?}"
    );
}

// ---------------------------------------------------------------------------
// 静的検査: fixture（違反を埋め込んだ関数と、正しい関数）
// ---------------------------------------------------------------------------

const FIX_HEAD: &str = "module ctfix\n  targets server\n  trust none\n\n";

fn fx(sig: &str, body: &str) -> String {
    fx_fx(sig, "none", body)
}

fn fx_fx(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"fixture\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

/// fixture のプログラムを組み立てる。lowering は入口から届く関数だけを
/// 残すので、`main_body` から対象の関数を呼ぶ形にすること。
fn fixture_program(defs: &str, main_body: &str) -> Program {
    fixture_program_fx(defs, main_body, "none")
}

fn fixture_program_fx(defs: &str, main_body: &str, main_effects: &str) -> Program {
    let src = format!(
        "{FIX_HEAD}{defs}{}",
        fx_fx("main() -> Int", main_effects, main_body)
    );
    sigil::ir_eval::lower_sources(&[&src], "main")
        .unwrap_or_else(|e| panic!("fixture の組み立てに失敗: {e}\n{src}"))
}

fn rules_of(violations: &[(String, Violation)], fname: &str) -> Vec<Rule> {
    violations
        .iter()
        .filter(|(n, _)| n == fname)
        .map(|(_, v)| v.rule)
        .collect()
}

#[test]
fn static_fixture_clean_functions_pass() {
    // 正しい形: 長さ（公開）で回し、バイトを XOR で積む。分岐は長さだけ。
    let defs = format!(
        "{}{}",
        fx(
            "ct_ok(secret: Bytes) -> Int",
            "  let mut acc = 0;\n  let mut i = 0;\n  while i < secret.len() {\n    acc = acc ^ secret.get(i).unwrap_or(0);\n    i = i + 1;\n  }\n  return acc;"
        ),
        fx(
            "ct_wrap(secret: Bytes) -> Bool",
            "  return ct_eq(&secret, &secret);"
        ),
    );
    let program = fixture_program(
        &defs,
        "  let a = ct_ok(Bytes.new());
  let b = ct_wrap(Bytes.new());
  return a;",
    );
    let table: &[(&str, &[&str])] = &[("ct_ok", &[]), ("ct_wrap", &[])];
    let violations = analyze_with_table(&program, table);
    assert!(
        violations.is_empty(),
        "正しい fixture に違反が出た: {violations:?}"
    );
}

#[test]
fn static_fixture_violations_are_caught() {
    let defs = format!(
        "{}{}{}{}{}{}",
        fx(
            "ct_bad_branch(secret: Bytes) -> Int",
            "  if secret.get(0).unwrap_or(0) == 1 {\n    return 1;\n  }\n  return 0;"
        ),
        fx(
            "ct_bad_region(secret: Bytes) -> Int",
            "  let mut copy = Bytes.new();\n  let mut i = 0;\n  while i < secret.len() {\n    copy.push(secret.get(i).unwrap_or(0));\n    i = i + 1;\n  }\n  if copy.get(0).unwrap_or(0) == 1 {\n    return 1;\n  }\n  return 0;"
        ),
        fx(
            "ct_bad_index(secret: Bytes, table: Bytes) -> Int",
            "  return table.get(secret.get(0).unwrap_or(0)).unwrap_or(0);"
        ),
        fx(
            "ct_bad_rem(secret: Int, m: Int) -> Int",
            "  return secret % m;"
        ),
        fx(
            "leak(x: Int) -> Int",
            "  return x + 1;"
        ),
        fx(
            "ct_bad_call(secret: Int) -> Int",
            "  return leak(secret);"
        ),
    );
    let defs = format!(
        "{defs}{}",
        fx_fx(
            "ct_bad_log(secret: Bytes) -> Int",
            "console",
            "  console.log(&secret.get(0).unwrap_or(0));\n  return 0;"
        )
    );
    let program = fixture_program_fx(
        &defs,
        "  let a = ct_bad_branch(Bytes.new());
  let b = ct_bad_region(Bytes.new());
  let c = ct_bad_index(Bytes.new(), Bytes.new());
  let d = ct_bad_rem(7, 3);
  let e = ct_bad_call(7);
  let f = ct_bad_log(Bytes.new());
  return a + b + c + d + e + f;",
        "console",
    );
    let table: &[(&str, &[&str])] = &[
        ("ct_bad_branch", &[]),
        ("ct_bad_region", &[]),
        ("ct_bad_index", &[]),
        ("ct_bad_rem", &["m"]),
        ("ct_bad_call", &[]),
        ("ct_bad_log", &[]),
    ];
    let v = analyze_with_table(&program, table);
    assert!(
        rules_of(&v, "ct_bad_branch").contains(&Rule::R1Branch),
        "R1 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_region").contains(&Rule::R1Branch),
        "領域経由の R1 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_index").contains(&Rule::R2Index),
        "R2 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_rem").contains(&Rule::R3Div),
        "R3 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_call").contains(&Rule::R4Call),
        "R4 を検出できない: {v:?}"
    );
    assert!(
        rules_of(&v, "ct_bad_log").contains(&Rule::R6Output),
        "R6 を検出できない: {v:?}"
    );
    // 違反のない関数 leak 自体は対象外なので、違反は報告されない。
    assert!(rules_of(&v, "leak").is_empty());
}

// ---------------------------------------------------------------------------
// CLIF 検査（ct_rules_clif）
// ---------------------------------------------------------------------------

/// CLIF 出力から数えた、注目する命令の数。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ClifCounts {
    /// 条件分岐（brif・br_table）。
    branch: usize,
    /// 除算・剰余（sdiv・udiv・srem・urem）。
    div: usize,
    /// 書き込み（store）。ゼロ化の消去を見るときの材料。
    store: usize,
}

fn count_clif(dump: &str) -> ClifCounts {
    let mut c = ClifCounts::default();
    for line in dump.lines() {
        let line = line.trim_start();
        if line.contains(" brif ") || line.starts_with("brif ") {
            c.branch += 1;
        }
        if line.contains(" br_table ") || line.starts_with("br_table ") {
            c.branch += 1;
        }
        for op in [" sdiv.", " udiv.", " srem.", " urem."] {
            if line.contains(op) {
                c.div += 1;
            }
        }
        if line.contains(" store ") || line.starts_with("store ") {
            c.store += 1;
        }
    }
    c
}

static CLIF_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `sigil build` を `SIGIL_CLIF_DUMP` つきで実行し、CLIF 出力を得る。
/// ソースはリポジトリの根の下に置かないと組み立てられないため、
/// `target/` の下に一時置き場を作る（モジュール名はパスから決まる）。
fn build_dump(tag: &str, module: &str, src: &str) -> String {
    let seq = CLIF_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = PathBuf::from(format!(
        "target/ct_rules_{}_{}_{}",
        std::process::id(),
        tag,
        seq
    ));
    std::fs::create_dir_all(&dir).expect("一時置き場を作れない");
    let file = dir.join(format!("{module}.sigil"));
    let full = src.replace(
        "{MODULE}",
        &format!(
            "target.{}.{module}",
            dir.file_name().unwrap().to_string_lossy()
        ),
    );
    std::fs::write(&file, full).expect("fixture を書けない");
    let out = dir.join("prog");
    let output = Command::new(env!("CARGO_BIN_EXE_sigil"))
        .arg("build")
        .arg(&file)
        .arg("main")
        .arg("-o")
        .arg(&out)
        .env("SIGIL_CLIF_DUMP", "1")
        .output()
        .expect("sigil build を実行できない");
    let dump = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        output.status.success(),
        "sigil build が失敗: {}",
        dump.chars().take(400).collect::<String>()
    );
    let _ = std::fs::remove_dir_all(&dir);
    dump
}

/// 対象関数を呼ぶプログラムと呼ばないプログラムの CLIF の差分。
/// 対象関数がユーザー定義の関数を呼ばない場合、その差分が対象関数の
/// 命令数になる（ファイル冒頭の「方法と限界」を参照）。
fn clif_delta(tag: &str, defs: &str, call: &str) -> ClifCounts {
    let with = format!(
        "module {{MODULE}}\n  targets server\n  trust none\n\n{defs}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return {call};\n}}\n"
    );
    let without = format!(
        "module {{MODULE}}\n  targets server\n  trust none\n\n{defs}fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return 0;\n}}\n"
    );
    let a = count_clif(&build_dump(tag, "a", &with));
    let b = count_clif(&build_dump(tag, "b", &without));
    ClifCounts {
        branch: a.branch.saturating_sub(b.branch),
        div: a.div.saturating_sub(b.div),
        store: a.store.saturating_sub(b.store),
    }
}

/// SIR の項のうち、ネイティブ生成で条件分岐になりうるもの数。
/// 分岐・振り分けに加えて、検査つき演算・表明・呼び出しは、異常停止へ
/// 進む分岐（桁あふれ・契約違反・スタックの検査）を 1 つまで伴いうる。
fn sir_branch_budget(func: &Func) -> usize {
    func.blocks
        .iter()
        .map(|b| match b.term {
            Term::Branch(..)
            | Term::Switch(..)
            | Term::CheckedOp { .. }
            | Term::Assert { .. }
            | Term::Call { .. } => 1,
            _ => 0,
        })
        .sum()
}

#[test]
fn clif_fixture_ct_ok_is_clean() {
    let defs = "fn ct_ok(secret: Bytes) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let mut acc = 0;\n  let mut i = 0;\n  while i < secret.len() {\n    acc = acc ^ secret.get(i).unwrap_or(0);\n    i = i + 1;\n  }\n  return acc;\n}\n\n";
    let delta = clif_delta("ctok", defs, "ct_ok(Bytes.new())");
    assert_eq!(delta.div, 0, "定数時間の fixture に除算命令がある");
    // SIR 側の分岐予算と比較する（対象関数の SIR を組み立て直して数える）。
    let src = format!(
        "{FIX_HEAD}{defs}{}",
        fx("main() -> Int", "  return ct_ok(Bytes.new());")
    );
    let program = sigil::ir_eval::lower_sources(&[&src], "main").expect("組み立て失敗");
    let f = program.funcs.iter().find(|f| f.name == "ct_ok").unwrap();
    let budget = sir_branch_budget(f);
    assert!(
        delta.branch >= 1,
        "差分が取れていない（帰属の前提が崩れた）: {delta:?}"
    );
    assert!(
        delta.branch <= budget,
        "CLIF の分岐 {0} が SIR の予算 {budget} を超えた: {delta:?}",
        delta.branch
    );
}

#[test]
fn clif_detects_division() {
    let defs = "fn with_rem(a: Int, b: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return a % b;\n}\n\n";
    let delta = clif_delta("rem", defs, "with_rem(7, 3)");
    assert!(delta.div >= 1, "除算命令を検出できない: {delta:?}");
}

#[test]
fn clif_real_ct_equal_is_clean() {
    // 標準ライブラリの ct_equal（ct_eq の実体）を入口から呼ぶプログラムで、
    // 差分がその関数の分だけになることを使って確かめる。
    let defs = "";
    let delta = clif_delta(
        "cteq",
        defs,
        "match ct_eq(&Bytes.new(), &Bytes.new()) { true => 1, false => 0 }",
    );
    assert_eq!(delta.div, 0, "ct_equal に除算命令がある: {delta:?}");
    let program = corpus();
    let f = program
        .funcs
        .iter()
        .find(|f| f.name == "std.core.bytes.ct_equal")
        .unwrap();
    let budget = sir_branch_budget(f);
    assert!(
        delta.branch <= budget,
        "ct_equal の CLIF 分岐 {0} が SIR の予算 {budget} を超えた: {delta:?}",
        delta.branch
    );
}

/// `Secret` の記述子が「秘密の種類」（17）で出ることを、組み立てた
/// オブジェクトのデータで確かめる（PR 6-1b の完了条件の 1 つ）。
/// CLIF のテキストは関数の命令だけを持ち、記述子の内容は含まないため、
/// ここではオブジェクトを語単位で走査し、`[種類=17, 名前への参照, 名前の
/// 長さ, フィールド数=1]` の並びを探す。通常の構造体（種類 9）では
/// 現れないことも対照で確かめる。ゼロ化そのものの振る舞いは
/// ランタイム（runtime/core）の単体テストが担保する。
/// 記述子は、型ごとの生成関数へ移った設計（ADR 0025 補足 G）では
/// 必要になったときにだけ出る。`--trace-drops` は破棄の表示のために
/// 記述子を要する経路なので、両方のプログラムとも有効にして組む。
#[test]
fn clif_secret_descriptor_is_zeroizing_kind() {
    fn object_of(src: &str) -> Vec<i64> {
        let p = sigil::ir_eval::lower_sources(&[src], "main").expect("組み立てに失敗");
        let opts = sigil::codegen::Opts {
            trace_drops: true,
            ..Default::default()
        };
        let obj = sigil::codegen::compile(&p, &opts).expect("オブジェクト生成に失敗");
        obj.as_chunks::<8>()
            .0
            .iter()
            .map(|c| i64::from_le_bytes(*c))
            .collect()
    }
    fn has_secret_desc(words: &[i64]) -> bool {
        words
            .windows(4)
            .any(|w| w[0] == 17 && (w[2] == 6 || w[2] == 17) && w[3] == 1)
    }
    let secret_src = "module dprobe\n  targets server\n  trust none\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let s = Secret.from_bytes(Bytes.new());\n  return s.len();\n}\n";
    assert!(
        has_secret_desc(&object_of(secret_src)),
        "Secret の記述子（種類 17）がオブジェクトにない"
    );
    let plain_src = "module dprobe\n  targets server\n  trust none\n\nstruct W {\n  data: Bytes,\n}\n\nfn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  let w = W { data: Bytes.new() };\n  return w.data.len();\n}\n";
    assert!(
        !has_secret_desc(&object_of(plain_src)),
        "通常の構造体に秘密の種類（17）が出てしまった"
    );
}

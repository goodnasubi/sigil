//! 定数時間検査の静的層（ADR 0020 決定 1・ADR 0024 決定 3・ADR 0061 決定 4 の段階 2）。
//!
//! `tests/ct_rules.rs` にあった検査エンジンを `src/` へ移したもの。テスト（標準ライブラリの
//! 対象表）と `sigil test --ct`（`Sigil.toml` の `[testing.constant_time]`）が同じ検査器を使う。
//!
//! ## テイントの付け方の約束（モデリング）
//!
//! - 対象で「公開」と書かれていない引数は、すべて秘密とする。
//! - **長さは公開情報**とする。`len`・`is_empty` の結果は、秘密の領域に対しても公開の値として
//!   扱う（ADR 0020 決定 1 の対象表のとおり）。
//! - **領域を経由する流れ**（ADR 0020 の必須事項）: 秘密の値を書いた領域から読んだ値は、読んだ
//!   位置によらず秘密とする。局所（領域の根）単位で「秘密を含む」を伝播させる。
//! - **raw 層の約束**: `raw.load_i64`・`raw.load_addr` は `Bytes` の頭のワード（長さと番地＝
//!   公開の情報）を読むので結果は公開。`raw.load_u8` は中身を読むので、秘密の領域から読んだ結果は
//!   秘密とする。
//! - 比較の結果（`ct_eq` の戻り値など）は公開の値とする（ADR 0020）。ただし定数時間の関数の
//!   **中**で秘密由来の比較結果を分岐に使うことは R1 として扱う。

use crate::ir::*;
use std::collections::BTreeSet;

// ADR 0020 / 0037: 定数時間照合の Bool だけを関数外で公開する。
// 一般の Bool 戻り値や検証器全体を除外しない。関数本体は引き続き検査する。
pub const PUBLIC_COMPARISONS: &[&str] = &[
    "std.crypto.verify_hmac_sha256",
    "std.crypto.aes128_gcm_tag_matches",
    "std.crypto.aes256_gcm_tag_matches",
    "std.crypto.chacha20_poly1305_tag_matches",
    "std.crypto.x25519_is_zero",
    "std.web.ct_bearer_signature",
];

/// ADR 0020 決定 1 の静的な規則。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rule {
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
pub struct Violation {
    pub rule: Rule,
    pub block: usize,
    pub detail: String,
}

pub(crate) fn viol(rule: Rule, block: usize, detail: impl Into<String>) -> Violation {
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
pub fn analyze_with_table(
    program: &Program,
    table: &[(&str, &[&str])],
) -> Vec<(String, Violation)> {
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
            // `Secret::len` / `Secret::byte_len`（対象表の `len` / `byte_len`。
            // メソッドなので SIR 名は裸の名前）の結果は長さであり、長さは公開の情報
            // （ADR 0020 決定 1、対象表の `len` の行の注記）。組み込みの *Len と同じ扱いに
            // しないと、`requires key.byte_len() >= 32` のような「秘密の長さを
            // 見る契約」が、中身による分岐として誤検出される。
            if is_ct
                && (name == "len"
                    || name == "byte_len"
                    || PUBLIC_COMPARISONS.contains(&name.as_str()))
            {
                set_place_taint(state, dest, false);
            } else {
                set_place_taint(state, dest, any);
            }
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
        | Builtin::StrByteLen
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

impl Rule {
    /// 利用者向けの規則名（`R1`〜`R6`）。
    pub fn code(self) -> &'static str {
        match self {
            Rule::R1Branch => "R1(分岐)",
            Rule::R2Index => "R2(番地)",
            Rule::R3Div => "R3(割り算)",
            Rule::R4Call => "R4(呼び出し)",
            Rule::R5Op => "R5(演算)",
            Rule::R6Output => "R6(外への出力)",
        }
    }
}

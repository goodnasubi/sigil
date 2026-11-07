//! SIR から Cranelift でオブジェクトを作る（ADR 0015 の決定 4・6・7、PR4）。
//!
//! 値の持ち方（`runtime/core/src/val.rs` と同じ）: どの値も 8 バイトの語。`Int`・`Bool` はそのまま、`Float` は
//! 64 ビットの並び、`String`・`Path` は `RtStr*`、一覧は `RtList*`（要素は語）、組・構造体は語の塊（フィールド `i` は `8*i`）、
//! 列挙型は `[腕の番号][フィールド…]`（フィールド `i` は `8+8*i`、塊は少なくとも 16 バイト）、`Cell`・`Shared` は中身の語を
//! 持つ塊への番地、参照は指す先の語の番地、関数の値は `[関数の番地][環境の語（なければ 0）]` の塊への番地。
//!
//! 関数は `(語…) -> (値, 停止の符号)` の形で呼ぶ（符号が 0 でなければ停止中で、`Resume` が呼び出し元の `cleanup` へ返す）。
//! 局所は 8 バイトの stack slot に置き、基本ブロックの境界をまたぐ値は slot を通す（ブロック引数は使わない）。
//! 表示は型の記述子（読み取り専用のデータ）をランタイムへ渡して行う。等しさ・順序・複写・破棄は型ごとに生成する関数で行う:
//! `sigil_eq_<k>`・`sigil_cmp_<k>`（補足 G-1）、`sigil_clone_<k>`・`sigil_shadow_<k>`、破棄の入口 `sigil_drop_<k>` と 1 段 `sigil_dstep_<k>`
//! （補足 G-2。破棄は作業の列で反復し、再帰しない）。塊は Sigil 製の確保器から取り、頭は `[大きさ | 黙った印][参照の数]`（補足 G-4）。
//! `--trace-drops` では構造体の破棄を順に標準エラーへ出す（補足 G-3）。文字列・パスも同じ道で解放する（ADR 0018 の決定 5）。
//! 既知の制限: クロージャの破棄の時期と停止の経路での漏れは SIR の評価器と厳密には揃えない。
//!
//! 出せない構文・組み込みはない（SIR の不変条件が崩れたときだけ `CgError::Internal`）。

mod func;

use crate::exec::StopKind;
use crate::ir::{Func, GlobalInit, Program, Ty};
use cranelift_codegen::ir::{types, AbiParam, Function, Signature, UserFuncName};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_codegen::Context;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, DataDescription, DataId, FuncId, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use func::Fx;
use std::collections::HashMap;
use std::fmt;

#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// `--trace-drops`（破棄の記録を標準エラーに出す）。
    pub trace_drops: bool,
    /// `--trust-base`（`trust base` を許すモジュール名。ADR 0016）。
    pub trust_base: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum CgError {
    /// 入口の指定の誤り（引数の型がコマンドラインから受け取れないなど）。
    Entry(String),
    /// 生成の失敗（Cranelift の検証の失敗など。不具合）。
    Internal(String),
}

impl fmt::Display for CgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CgError::Entry(m) => write!(f, "{m}"),
            CgError::Internal(m) => write!(f, "内部エラー: {m}"),
        }
    }
}

fn internal(e: impl fmt::Display) -> CgError {
    CgError::Internal(e.to_string())
}

/// 記述子・表のデータの語。
pub(crate) enum W {
    I(i64),
    D(DataId),
}

/// 参照の入れ子を 1 段にまとめる（`&&T` と `&mut &T` は 1 つの番地）。`erase_fn` なら関数の型の中身を消す（記述子の鍵）。
pub(crate) fn norm_with(ty: &Ty, erase_fn: bool) -> Ty {
    match ty {
        Ty::Ref(i) | Ty::MutRef(i) => {
            let mut j = &**i;
            while let Ty::Ref(k) | Ty::MutRef(k) = j {
                j = k;
            }
            Ty::Ref(Box::new(norm_with(j, erase_fn)))
        }
        Ty::List(t) => Ty::List(Box::new(norm_with(t, erase_fn))),
        Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(|t| norm_with(t, erase_fn)).collect()),
        Ty::Shared(t) => Ty::Shared(Box::new(norm_with(t, erase_fn))),
        Ty::Cell(t) => Ty::Cell(Box::new(norm_with(t, erase_fn))),
        Ty::FnRef(ps, r) => {
            if erase_fn {
                Ty::FnRef(vec![], Box::new(Ty::Unit))
            } else {
                Ty::FnRef(
                    ps.iter().map(|t| norm_with(t, false)).collect(),
                    Box::new(norm_with(r, false)),
                )
            }
        }
        other => other.clone(),
    }
}

pub(crate) fn norm(ty: &Ty) -> Ty {
    norm_with(ty, false)
}

/// 生成コードの中で定義する文字列の関数（`std.core.string` を呼ぶシム。補足 D-3）の、呼び出し側から見た引数と戻りの形。
/// Rust ランタイムの関数ではないので `rt_spec` には置かない。形は `rt_spec` と同じ（`i` の戻りは 32 ビット）。
pub(crate) fn shim_spec(name: &str) -> Option<(&'static str, char)> {
    Some(match name {
        "str_new" | "str_concat" => ("pp", 'p'),
        "str_clone" | "str_data" | "str_bytes" | "str_len" | "str_trim" | "str_lines" => ("p", 'p'),
        "str_free" => ("p", 'v'),
        "str_is_empty" => ("p", 'i'),
        // 等しさ（0/1）と順序（-1・0・1。`Int` の値なので `'p'`）。生成する `sigil_eq_<k>`・`sigil_cmp_<k>` が呼ぶ（補足 G-1）
        "str_eq" | "bytes_eq" => ("pp", 'i'),
        "str_cmp" | "bytes_cmp" => ("pp", 'p'),
        "str_parse_int" => ("pp", 'i'),
        "str_from_utf8" => ("ppp", 'p'),
        // `Int` の回転（`std.core.int`。補足 F-3）。`Int` の値を返すので `'p'`（`'i'` は 32 ビットへ切り詰める。F-0・F-4）
        "int_rotl" | "int_rotr" => ("ppp", 'p'),
        "int_rotr32" => ("pp", 'p'),
        // 一覧の表現（`std.core.list`。補足 E-3）
        "list_new" | "list_len" => ("p", 'p'),
        "list_get" => ("pp", 'p'),
        "list_push" => ("pp", 'v'),
        "list_free" => ("p", 'v'),
        "list_sum_int" => ("pp", 'i'),
        // 戻りは `Float` のビット列の語（E-9 の 6）。生成コードの `Float` の値も語なので、そのまま使える
        "list_sum_float" => ("p", 'p'),
        // バイト列の表現（`std.core.bytes`。補足 F-4）。`Int` を返すものは `'p'`、`Bool`・小さい状態（0..=2）は `'i'`
        "bytes_new" => ("", 'p'),
        "bytes_with_capacity" | "bytes_clone" | "bytes_data" | "bytes_len" => ("p", 'p'),
        "bytes_from_raw" | "bytes_concat" => ("pp", 'p'),
        "bytes_slice" => ("ppp", 'p'),
        "bytes_push" | "bytes_extend" | "bytes_push_u16_be" | "bytes_push_u32_be"
        | "bytes_push_u64_be" | "bytes_push_u32_le" => ("pp", 'v'),
        "bytes_free" => ("p", 'v'),
        "bytes_is_empty" => ("p", 'i'),
        "bytes_ct_eq" => ("pp", 'i'),
        "bytes_get" | "bytes_read_u32_be" | "bytes_set" => ("ppp", 'i'),
        _ => return None,
    })
}

/// ランタイム関数の引数と戻りの形（`p` 語・`i` 32 ビット整数・`f` 倍精度・`v` なし）。
pub(crate) fn rt_spec(name: &str) -> Option<(&'static str, char)> {
    Some(match name {
        "memmove" | "memset" | "raw_free" => ("ppp", 'v'),
        "raw_alloc" => ("pp", 'p'),
        "set_sigil_live" => ("p", 'v'),
        "set_str_ops" | "set_list_ops" | "set_bytes_ops" => ("ppppp", 'v'),
        "raw_syscall" => ("ppppppp", 'p'),
        "enter" => ("pp", 'i'),
        "console_log" | "os_buf_free" => ("pp", 'v'),
        // `Secret` の中身のゼロ化（ADR 0024、PR 6-1b）: Bytes を取り、データ領域をゼロで上書きする。生成した破棄の段が呼ぶ。
        "zeroize_bytes" => ("p", 'v'),
        "stack_limit" => ("", 'p'),
        "fs_read" | "fs_write" => ("ppppp", 'i'),
        "path_join" => ("pppppp", 'v'),
        "path_file_name" | "path_parent" => ("pppp", 'i'),
        "fmod" => ("ff", 'f'),
        _ => return None,
    })
}

pub(crate) struct Cg<'p> {
    pub p: &'p Program,
    pub m: ObjectModule,
    pub cc: CallConv,
    fe: cranelift_codegen::isa::TargetFrontendConfig,
    rt: HashMap<&'static str, FuncId>,
    bytes: HashMap<Vec<u8>, DataId>,
    fnblocks: HashMap<usize, DataId>,
    msgs: Vec<String>,
    msg_idx: HashMap<String, usize>,
    pub trace: bool,
    pub fids: Vec<FuncId>,
    pub msg_table: DataId,
    /// 大域ごとの静的データ（8 バイトの語。`Program.globals` と同じ添字）。
    pub gids: Vec<DataId>,
    /// 生存数のシム `sigil_rt_sigil_live_blocks`（確保器を結んだときだけ。補足 B-5）。
    pub live_shim: Option<FuncId>,
    /// 文字列の関数表に登録するシム（new・free・clone・data・bytes。補足 D-3）。
    pub str_ops: Option<[FuncId; 5]>,
    /// 一覧の関数表に登録するシム（create・free・push・get・len。補足 E-3）。
    pub list_ops: Option<[FuncId; 5]>,
    /// バイト列の関数表に登録するシム（from_raw・free・clone・data・len。補足 F-4）。
    pub bytes_ops: Option<[FuncId; 5]>,
    /// 型ごとに生成する関数（補足 G-1）。鍵は（種類, 型）で、型は `norm_with(strip(ty), true)`。通し番号 `<k>` は `gen_n` から取る。
    gen: HashMap<(Gk, Ty), FuncId>,
    /// まだ本体を作っていない生成の関数（本体が新しい鍵を求めるので、空になるまで回す）。
    gen_q: Vec<(FuncId, Gk, Ty, usize)>,
    /// 生成の関数の通し番号の次の値（`sigil_<種類>_<k>` の `<k>` と、関数の番号の添字に使う）。
    gen_n: usize,
    /// 作業の列の静的な領域（補足 G-2）。`None` なら、この生成では使っていない。
    ws: Option<DataId>,
    /// 0 でなければ塊を解放する `sigil_blkfree_nz`（どのプログラムにも置く。子を持たない値の破棄が呼ぶ。補足 G-2）。
    pub blkfree_nz: Option<FuncId>,
    /// 表示の補助（補足 H）。宣言したもの。
    show_fns: HashMap<SFn, FuncId>,
    /// まだ本体を作っていない表示の補助（本体が別の補助を求めることがあるので、空になるまで回す）。
    show_q: Vec<(FuncId, SFn)>,
    /// 表示バッファの静的な領域（補足 H-2）。`None` なら、この生成では使っていない。
    show_buf: Option<DataId>,
    /// 引用で逃がす符号点の範囲のデータ（補足 H-4）。`None` なら未作成で、値は（データ, 範囲の数）。
    quote: Option<(DataId, usize)>,
    /// 合わせ込みの失敗の内訳（補足 I-4）: `[要る型の名前の番地, 長さ, 渡された値の名前の番地, 長さ]`。
    /// `sigil_coerce_<k>` が失敗の箇所で書き、thunk が誤りの文言を組み立てるときに読む。
    coerce_err: Option<DataId>,
}

/// 型ごとに生成する関数の種類（補足 G-1・G-2）。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Gk {
    /// `sigil_eq_<k>`: 等しさ（1 なら等しい）
    Eq,
    /// `sigil_cmp_<k>`: 順序（−1・0・1。比べられなければ 2）
    Cmp,
    /// `sigil_clone_<k>`: `clone`（`Cell` は独立した複製）
    Clone,
    /// `sigil_shadow_<k>`: 影（`Copy` の複写。共有するものは共有のまま）
    Shadow,
    /// `sigil_drop_<k>`: 破棄の入口（作業の列を回す）
    Drop,
    /// `sigil_dstep_<k>`: 破棄の 1 段（子を積み、自分の塊を解放する）
    Dstep,
    /// `sigil_show_<k>`: 表示（共有バッファへ追記。補足 H-1）
    Show,
    /// `sigil_coerce_<k>`: 入口の引数の合わせ込み（リテラル値を型の語に変える。補足 I-4）。
    /// 引数は（リテラル値の塊, 語を書く先）で、戻りは成否（1/0）。
    Coerce,
}

impl Gk {
    fn prefix(self) -> &'static str {
        match self {
            Gk::Eq => "eq",
            Gk::Cmp => "cmp",
            Gk::Clone => "clone",
            Gk::Shadow => "shadow",
            Gk::Drop => "drop",
            Gk::Dstep => "dstep",
            Gk::Show => "show",
            Gk::Coerce => "coerce",
        }
    }

    /// 引数の数と戻りの印（`shim_sig` の形）。
    pub(crate) fn arity(self) -> (usize, ShimRet) {
        match self {
            Gk::Eq | Gk::Cmp => (2, 'p'),
            Gk::Clone | Gk::Shadow => (1, 'p'),
            Gk::Drop | Gk::Dstep => (1, 'v'),
            Gk::Show => (2, 'v'),
            Gk::Coerce => (2, 'p'),
        }
    }
}

/// 表示の補助の種類（補足 H-1・H-2・H-3・H-4・H-6）と停止の報告（補足 I-6）。型ごとではなく、プログラムに 1 組だけ生成する。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum SFn {
    /// `sigil_show_begin()`: バッファを空にする。
    Begin,
    /// `sigil_show_end()`: バッファを片付ける（伸ばした領域を返し、長さを 0 に戻す）。
    End,
    /// `sigil_show_end_str() -> p`: バッファの中身から文字列を作って片付ける。
    EndStr,
    /// `sigil_show_push(p, p)`: バッファへバイト列を足す。
    Push,
    /// `sigil_show_int(p)`: `Int` の 10 進の並びを足す。
    Int,
    /// `sigil_show_float(f)`: `Float` の最短表現を足す。
    Float,
    /// `sigil_show_quote(p, p)`: 文字列の中身の引用（`"` で囲んだ並び）を足す。
    Quote,
    /// `sigil_write_all(p, p, p)`: ファイル記述子へ全部書く。
    WriteAll,
    /// `sigil_report_stop(p, p)`: 停止の報告を fd 2 へ出し、終了コード 70 で終わる（補足 I-6）。戻らない。
    ReportStop,
    /// `sigil_arg_parse(p, p, p) -> p`: コマンドラインの引数 1 つをリテラル値の塊に読む（補足 I-2）。
    /// 引数は（バイト列の番地, 長さ, 誤りの {s} を書く先 2 語）で、戻りは塊の番地（誤りなら 0）。
    ArgParse,
    /// `sigil_arg_float(p, p, p) -> p`: バイト列を `Float` として読めるか確かめ、ビット列を書く（補足 I-3）。
    /// 引数は（バイト列の番地, 長さ, ビット列を書く先）で、戻りは成否（1/0）。
    ArgFloat,
}

impl SFn {
    fn name(self) -> &'static str {
        match self {
            SFn::Begin => "sigil_show_begin",
            SFn::End => "sigil_show_end",
            SFn::EndStr => "sigil_show_end_str",
            SFn::Push => "sigil_show_push",
            SFn::Int => "sigil_show_int",
            SFn::Float => "sigil_show_float",
            SFn::Quote => "sigil_show_quote",
            SFn::WriteAll => "sigil_write_all",
            SFn::ReportStop => "sigil_report_stop",
            SFn::ArgParse => "sigil_arg_parse",
            SFn::ArgFloat => "sigil_arg_float",
        }
    }

    fn sig(self, cc: CallConv) -> Signature {
        let mut s = Signature::new(cc);
        match self {
            SFn::Begin | SFn::End => {}
            SFn::EndStr => s.returns.push(AbiParam::new(types::I64)),
            SFn::Push | SFn::Quote | SFn::ReportStop => {
                s.params.push(AbiParam::new(types::I64));
                s.params.push(AbiParam::new(types::I64));
            }
            SFn::Int => s.params.push(AbiParam::new(types::I64)),
            SFn::Float => s.params.push(AbiParam::new(types::F64)),
            SFn::WriteAll | SFn::ArgParse | SFn::ArgFloat => {
                for _ in 0..3 {
                    s.params.push(AbiParam::new(types::I64));
                }
                if !matches!(self, SFn::WriteAll) {
                    s.returns.push(AbiParam::new(types::I64));
                }
            }
        }
        s
    }
}

/// 表示バッファの静的な領域の大きさ（バイト）。補足 H-2。
pub(crate) const SB_INLINE_BYTES: usize = 4096;
/// 表示バッファのデータの大きさ（先頭の 24 バイトは状態: `[長さ][確保した領域][その容量]`）。
pub(crate) const SB_BYTES: usize = 24 + SB_INLINE_BYTES;

/// 作業の列の静的な領域の大きさ（項の数）。項は 16 バイト（`(段の関数の番地, 語)`）。補足 G-2。
pub(crate) const WS_INLINE_ITEMS: usize = 4096;
/// 作業の列の静的な領域の 1 つ分の大きさ（バイト）。先頭の 24 バイトは `[項の数][確保した領域][その項の数]`。
pub(crate) const WS_BYTES: usize = 24 + WS_INLINE_ITEMS * 16;

impl<'p> Cg<'p> {
    pub fn sig(&self, n: usize) -> Signature {
        let mut s = Signature::new(self.cc);
        for _ in 0..n {
            s.params.push(AbiParam::new(types::I64));
        }
        s.returns.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        s
    }

    pub fn rt_id(&mut self, name: &'static str) -> Result<FuncId, CgError> {
        if let Some(id) = self.rt.get(name) {
            return Ok(*id);
        }
        let Some((ps, r)) = rt_spec(name).or_else(|| shim_spec(name)) else {
            return Err(internal(format!("ランタイム関数 {name} の形がない")));
        };
        let mut s = Signature::new(self.cc);
        for c in ps.chars() {
            s.params.push(AbiParam::new(match c {
                'p' => types::I64,
                'i' => types::I32,
                _ => types::F64,
            }));
        }
        match r {
            'p' => s.returns.push(AbiParam::new(types::I64)),
            'i' => s.returns.push(AbiParam::new(types::I32)),
            'f' => s.returns.push(AbiParam::new(types::F64)),
            _ => {}
        }
        // `memmove`・`memset` は生成コードの中で定義する（`mem_fns`。補足 C-3）ので、ここには来ない。
        let sym = if name == "fmod" {
            name.to_string()
        } else {
            format!("sigil_rt_{name}")
        };
        let id = self
            .m
            .declare_function(&sym, Linkage::Import, &s)
            .map_err(internal)?;
        self.rt.insert(name, id);
        Ok(id)
    }

    /// 型 `ty`（`norm_with(strip(..), true)` で正規化済み）の等しさ・順序の関数（補足 G-1）。
    pub fn eqcmp_fn(&mut self, is_cmp: bool, ty: &Ty) -> Result<FuncId, CgError> {
        self.gen_fn(if is_cmp { Gk::Cmp } else { Gk::Eq }, ty)
    }

    /// 種類 `k` と型 `ty`（正規化済み）の生成の関数。鍵になければ宣言して表に入れ、本体は待ち行列に積む
    /// （補足 G-1。再帰する型でも宣言が先なので終わる）。
    pub fn gen_fn(&mut self, k: Gk, ty: &Ty) -> Result<FuncId, CgError> {
        if let Some(id) = self.gen.get(&(k, ty.clone())) {
            return Ok(*id);
        }
        let n = self.gen_n;
        self.gen_n += 1;
        let name = format!("sigil_{}_{n}", k.prefix());
        let (np, r) = k.arity();
        let id = self
            .m
            .declare_function(&name, Linkage::Local, &shim_sig(self.cc, np, r))
            .map_err(internal)?;
        self.gen.insert((k, ty.clone()), id);
        self.gen_q.push((id, k, ty.clone(), n));
        Ok(id)
    }

    /// 作業の列の静的な領域のデータ（補足 G-2）。初めて使うときに宣言し、最後に 0 で定義する。
    pub fn ws_data(&mut self) -> Result<DataId, CgError> {
        if let Some(d) = self.ws {
            return Ok(d);
        }
        let d = self
            .m
            .declare_anonymous_data(true, false)
            .map_err(internal)?;
        self.ws = Some(d);
        Ok(d)
    }

    /// 表示の補助 `k`（補足 H）。宣言がなければ宣言して表に入れ、本体は待ち行列に積む。
    pub fn show_fn(&mut self, k: SFn) -> Result<FuncId, CgError> {
        if let Some(id) = self.show_fns.get(&k) {
            return Ok(*id);
        }
        let id = self
            .m
            .declare_function(k.name(), Linkage::Local, &k.sig(self.cc))
            .map_err(internal)?;
        self.show_fns.insert(k, id);
        self.show_q.push((id, k));
        Ok(id)
    }

    /// 表示バッファの静的な領域のデータ（補足 H-2）。初めて使うときに宣言し、最後に 0 で定義する。
    pub fn show_buf_data(&mut self) -> Result<DataId, CgError> {
        if let Some(d) = self.show_buf {
            return Ok(d);
        }
        let d = self
            .m
            .declare_anonymous_data(true, false)
            .map_err(internal)?;
        self.show_buf = Some(d);
        Ok(d)
    }

    /// 引用で逃がす符号点の範囲のデータ（補足 H-4）。値は（データ, 範囲の数）。
    /// 範囲は `[下限, 上限]` の 2 語ずつで、上限を含む。
    pub fn quote_data(&mut self) -> Result<(DataId, usize), CgError> {
        if let Some(v) = self.quote {
            return Ok(v);
        }
        let ranges = quote_escape_ranges();
        let mut ws: Vec<W> = Vec::with_capacity(ranges.len() * 2);
        for &(lo, hi) in ranges {
            ws.push(W::I(lo));
            ws.push(W::I(hi));
        }
        let d = self.data_anon()?;
        self.write_words(d, &ws)?;
        let v = (d, ranges.len());
        self.quote = Some(v);
        Ok(v)
    }

    fn data_anon(&mut self) -> Result<DataId, CgError> {
        self.m
            .declare_anonymous_data(false, false)
            .map_err(internal)
    }

    fn write_words(&mut self, id: DataId, ws: &[W]) -> Result<(), CgError> {
        let mut bytes = Vec::with_capacity(ws.len() * 8);
        for w in ws {
            match w {
                W::I(n) => bytes.extend_from_slice(&n.to_le_bytes()),
                W::D(_) => bytes.extend_from_slice(&[0u8; 8]),
            }
        }
        let mut dd = DataDescription::new();
        dd.define(bytes.into_boxed_slice());
        dd.set_align(8);
        for (i, w) in ws.iter().enumerate() {
            if let W::D(d) = w {
                let gv = self.m.declare_data_in_data(*d, &mut dd);
                dd.write_data_addr((i * 8) as u32, gv, 0);
            }
        }
        self.m.define_data(id, &dd).map_err(internal)
    }

    /// 読み取り専用のバイト列（空でも 1 バイト持つ）。
    pub fn bytes_data(&mut self, b: &[u8]) -> Result<DataId, CgError> {
        if let Some(d) = self.bytes.get(b) {
            return Ok(*d);
        }
        let id = self.data_anon()?;
        let mut dd = DataDescription::new();
        dd.define(if b.is_empty() {
            vec![0u8].into_boxed_slice()
        } else {
            b.to_vec().into_boxed_slice()
        });
        self.m.define_data(id, &dd).map_err(internal)?;
        self.bytes.insert(b.to_vec(), id);
        Ok(id)
    }

    fn name_words(&mut self, s: &str) -> Result<[W; 2], CgError> {
        if s.is_empty() {
            return Ok([W::I(0), W::I(0)]);
        }
        let d = self.bytes_data(s.as_bytes())?;
        Ok([W::D(d), W::I(s.len() as i64)])
    }

    /// 合わせ込みの失敗の内訳を置く領域（補足 I-4）。4 語（要る型の名前 2 語＋
    /// 渡された値の名前 2 語）で、最初はゼロ。`sigil_coerce_<k>` が失敗の箇所で書き、
    /// thunk が誤りの文言を組み立てるときに読む。
    pub fn coerce_err_data(&mut self) -> DataId {
        if let Some(d) = self.coerce_err {
            return d;
        }
        let d = self
            .m
            .declare_data("sigil_coerce_err", Linkage::Local, true, false)
            .expect("coerce err decl");
        let mut dd = DataDescription::new();
        dd.define_zeroinit(32);
        self.m.define_data(d, &dd).expect("coerce err def");
        self.coerce_err = Some(d);
        d
    }

    /// 環境を持たない関数の値（`[関数の番地][0]`）の静的な塊。
    pub fn fnblock(&mut self, f: usize) -> Result<DataId, CgError> {
        if let Some(d) = self.fnblocks.get(&f) {
            return Ok(*d);
        }
        let id = self.data_anon()?;
        let mut dd = DataDescription::new();
        dd.define(vec![0u8; 16].into_boxed_slice());
        dd.set_align(8);
        let fr = self.m.declare_func_in_data(self.fids[f], &mut dd);
        dd.write_function_addr(0, fr);
        self.m.define_data(id, &dd).map_err(internal)?;
        self.fnblocks.insert(f, id);
        Ok(id)
    }

    /// 停止の報告の文字列の番号 + 1（0 は停止なし）。
    pub fn code_for(&mut self, stop: usize, kind: Option<StopKind>, detail: Option<&str>) -> i64 {
        let s = &self.p.stop_sites[stop];
        let text = crate::report::stop_message(
            kind.unwrap_or(s.kind),
            &s.func,
            s.line,
            s.during.as_ref().map(|(f, c)| (f.as_str(), *c)),
            detail.unwrap_or(&s.detail),
        );
        if let Some(i) = self.msg_idx.get(&text) {
            return *i as i64 + 1;
        }
        let i = self.msgs.len();
        self.msg_idx.insert(text.clone(), i);
        self.msgs.push(text);
        i as i64 + 1
    }

    fn define(
        &mut self,
        id: FuncId,
        idx: u32,
        sig: Signature,
        lt: Vec<Ty>,
        body: impl FnOnce(&mut Fx<'_, 'p>),
    ) -> Result<(), CgError> {
        let mut func = Function::with_name_signature(UserFuncName::user(0, idx), sig);
        let mut fctx = FunctionBuilderContext::new();
        let fe = self.fe;
        let err = {
            let b = FunctionBuilder::new(&mut func, &mut fctx);
            let mut fx = Fx::new(self, b, lt);
            body(&mut fx);
            let Fx { mut b, err, .. } = fx;
            b.seal_all_blocks();
            b.finalize(fe);
            err
        };
        if let Some(e) = err {
            return Err(e);
        }
        let dump = std::env::var_os("SIGIL_CLIF_DUMP").is_some();
        let text = if dump {
            Some(func.display().to_string())
        } else {
            None
        };
        if let Some(t) = &text {
            eprintln!("{t}");
        }
        let mut ctx = Context::for_function(func);
        self.m.define_function(id, &mut ctx).map_err(|e| {
            let clif = text.unwrap_or_else(|| ctx.func.display().to_string());
            internal(format!("{e:?}\n{clif}"))
        })
    }
}

fn supported_param(t: &Ty) -> bool {
    match t {
        Ty::Int | Ty::Float | Ty::Bool | Ty::Addr | Ty::Str | Ty::Path => true,
        Ty::List(e) => supported_param(e),
        Ty::Ref(i) | Ty::MutRef(i) => supported_param(i),
        _ => false,
    }
}

/// `p` をオブジェクトのバイト列にする。
pub fn compile(p: &Program, opts: &Opts) -> Result<Vec<u8>, CgError> {
    let entry: &Func = &p.funcs[p.entry];
    for &pi in &entry.params {
        if !supported_param(&entry.locals[pi].ty) {
            return Err(CgError::Entry(format!(
                "入口の関数 {} の引数 {} の型は、コマンドラインから受け取れない（Int・Float・Bool・String・Path とその List だけ）",
                entry.name, entry.locals[pi].name
            )));
        }
    }

    let mut fb = settings::builder();
    fb.set("opt_level", "none").map_err(internal)?;
    fb.set("is_pic", "false").map_err(internal)?;
    fb.set("enable_verifier", "true").map_err(internal)?;
    // 大きな枠でもライブラリ関数を呼ばない（静的リンクに `__rust_probestack` 相当を足さない）。
    let _ = fb.set("probestack_strategy", "inline");
    let flags = settings::Flags::new(fb);
    let triple = if cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else {
        "x86_64-unknown-linux-gnu"
    };
    let isa = cranelift_codegen::isa::lookup_by_name(triple)
        .map_err(internal)?
        .finish(flags)
        .map_err(internal)?;
    let cc = isa.default_call_conv();
    let fe = isa.frontend_config();
    let ob = ObjectBuilder::new(isa, "sigil", default_libcall_names()).map_err(internal)?;
    let mut m = ObjectModule::new(ob);

    let mut fids = Vec::with_capacity(p.funcs.len());
    // 入口でないモジュールの関数の名前は `<モジュール>.<関数>`（ADR 0021 の 2-2c）。
    for (i, f) in p.funcs.iter().enumerate() {
        let mut s = Signature::new(cc);
        for _ in 0..f.params.len() {
            s.params.push(AbiParam::new(types::I64));
        }
        s.returns.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I64));
        fids.push(
            m.declare_function(&func_symbol(i, &f.name), Linkage::Local, &s)
                .map_err(internal)?,
        );
    }
    let msg_table = m.declare_anonymous_data(false, false).map_err(internal)?;
    // 大域: 語 1 つの静的データ。スカラーは初期値をそのまま置き、組・構造体は起動時に作る塊への番地を
    // 入れる場所（0 で始める。入口の thunk が初期化する）。単一スレッド前提（ADR 0025 補足 A-5）。
    let mut gids = Vec::with_capacity(p.globals.len());
    for g in &p.globals {
        let agg = matches!(g.init, GlobalInit::Tuple(_) | GlobalInit::Struct(_));
        let id = m
            .declare_anonymous_data(g.mutable || agg, false)
            .map_err(internal)?;
        let word: i64 = match &g.init {
            GlobalInit::Int(n) => *n,
            GlobalInit::Bool(b) => *b as i64,
            GlobalInit::AddrNull | GlobalInit::Tuple(_) | GlobalInit::Struct(_) => 0,
        };
        let mut dd = DataDescription::new();
        dd.define(word.to_le_bytes().to_vec().into_boxed_slice());
        dd.set_align(8);
        m.define_data(id, &dd).map_err(internal)?;
        gids.push(id);
    }
    let mut cg = Cg {
        p,
        m,
        cc,
        fe,
        rt: HashMap::new(),
        bytes: HashMap::new(),
        fnblocks: HashMap::new(),
        msgs: vec![],
        msg_idx: HashMap::new(),
        trace: opts.trace_drops,
        fids,
        msg_table,
        gids,
        live_shim: None,
        str_ops: None,
        list_ops: None,
        bytes_ops: None,
        gen: HashMap::new(),
        gen_q: vec![],
        gen_n: 0,
        ws: None,
        blkfree_nz: None,
        show_fns: HashMap::new(),
        show_q: vec![],
        show_buf: None,
        quote: None,
        coerce_err: None,
    };

    // Sigil 製の確保器のシム（ADR 0025 補足 B-2・B-5）。確保器の関数を結んだときだけ公開する。
    let mut shims = alloc_shims(&mut cg)?;
    // 文字列の表現（`std.core.string`）のシム（補足 D-3）。常に必要で、結ばれていなければ内部エラーで止める。
    shims.extend(str_shims(&mut cg)?);
    // 一覧の表現（`std.core.list`）のシム（補足 E-3）。同じく常に必要。
    shims.extend(list_shims(&mut cg)?);
    shims.extend(int_shims(&mut cg)?);
    // バイト列の表現（`std.core.bytes`）のシム（補足 F-4）。同じく常に必要。
    shims.extend(bytes_shims(&mut cg)?);
    // `memmove`・`memset`（補足 C-3）。libc のものは使わず、同じオブジェクトの中で定義する。
    let mem = mem_fns(&mut cg)?;
    // 0 でなければ塊を解放する共通の関数（補足 G-2）。使わないプログラムでも置く（比べる試験の分岐の数を揃えるため）。
    let blkfree =
        cg.m.declare_function("sigil_blkfree_nz", Linkage::Local, &shim_sig(cc, 1, 'v'))
            .map_err(internal)?;
    cg.blkfree_nz = Some(blkfree);

    for (i, f) in p.funcs.iter().enumerate() {
        let sig = cg.sig(f.params.len());
        let lt: Vec<Ty> = f.locals.iter().map(|l| norm(&l.ty)).collect();
        let id = cg.fids[i];
        cg.define(id, i as u32 + 1, sig, lt, |fx| fx.run(f))?;
    }

    for (k, &(id, target, np, ret)) in shims.iter().enumerate() {
        let sig = shim_sig(cc, np, ret);
        cg.define(id, p.funcs.len() as u32 + 3 + k as u32, sig, vec![], |fx| {
            fx.alloc_shim(target, ret)
        })?;
    }

    for (k, &(id, kind)) in mem.iter().enumerate() {
        let mut sig = Signature::new(cc);
        for _ in 0..3 {
            sig.params.push(AbiParam::new(types::I64));
        }
        let idx = p.funcs.len() as u32 + 3 + shims.len() as u32 + k as u32;
        cg.define(id, idx, sig, vec![], |fx| match kind {
            MemFn::Move => fx.mem_move(),
            MemFn::Set => fx.mem_set(),
        })?;
    }

    // 入口: スレッドの本体（引数を読み、入口の関数を呼び、値を表示する）と C の main。
    let thunk =
        cg.m.declare_function("sigil_thunk", Linkage::Local, &{
            let mut s = Signature::new(cc);
            s.params.push(AbiParam::new(types::I64));
            s.returns.push(AbiParam::new(types::I32));
            s
        })
        .map_err(internal)?;
    cg.define(
        thunk,
        p.funcs.len() as u32 + 1,
        {
            let mut s = Signature::new(cc);
            s.params.push(AbiParam::new(types::I64));
            s.returns.push(AbiParam::new(types::I32));
            s
        },
        vec![],
        |fx| fx.thunk(),
    )?;
    let main_sig = {
        let mut s = Signature::new(cc);
        s.params.push(AbiParam::new(types::I32));
        s.params.push(AbiParam::new(types::I64));
        s.returns.push(AbiParam::new(types::I32));
        s
    };
    let main =
        cg.m.declare_function("main", Linkage::Export, &main_sig)
            .map_err(internal)?;
    cg.define(main, p.funcs.len() as u32 + 2, main_sig, vec![], |fx| {
        fx.c_main(thunk)
    })?;

    // 型ごとの生成の関数の本体（補足 G-1・G-2）。本体が新しい鍵を求めるので、待ち行列が空になるまで回す。
    let base = p.funcs.len() as u32 + 3 + shims.len() as u32 + mem.len() as u32;
    while let Some((id, k, ty, n)) = cg.gen_q.pop() {
        let (np, r) = k.arity();
        let sig = shim_sig(cc, np, r);
        cg.define(id, base + n as u32, sig, vec![], |fx| fx.gen_body(k, &ty))?;
    }

    // `sigil_blkfree_nz` の本体（補足 G-2）。番号は生成の関数の後に続ける。
    let blkfree_idx = base + cg.gen_n as u32;
    cg.gen_n += 1;
    cg.define(blkfree, blkfree_idx, shim_sig(cc, 1, 'v'), vec![], |fx| {
        fx.blkfree_nz_body()
    })?;

    // 表示の補助の本体（補足 H）。本体が別の補助を求めることがあるので、列が空になるまで回す。
    while let Some((id, k)) = cg.show_q.pop() {
        let idx = base + cg.gen_n as u32;
        cg.gen_n += 1;
        cg.define(id, idx, k.sig(cc), vec![], |fx| fx.show_body(k))?;
    }

    // 作業の列の静的な領域（補足 G-2）。語は 0 で始まり、項の領域は後から確保した領域に替わる。
    if let Some(d) = cg.ws {
        let mut dd = DataDescription::new();
        dd.define(vec![0u8; WS_BYTES].into_boxed_slice());
        dd.set_align(8);
        cg.m.define_data(d, &dd).map_err(internal)?;
    }

    // 表示バッファの静的な領域（補足 H-2）。語は 0 で始まり、中身の領域は後から確保した領域に替わる。
    if let Some(d) = cg.show_buf {
        let mut dd = DataDescription::new();
        dd.define(vec![0u8; SB_BYTES].into_boxed_slice());
        dd.set_align(8);
        cg.m.define_data(d, &dd).map_err(internal)?;
    }

    // 停止の報告の表（16 バイトずつ: 文字列の番地, 長さ）。
    let msgs = std::mem::take(&mut cg.msgs);
    let mut ws: Vec<W> = vec![];
    for t in &msgs {
        ws.extend(cg.name_words(t)?);
    }
    if ws.is_empty() {
        ws.push(W::I(0));
        ws.push(W::I(0));
    }
    let table = cg.msg_table;
    cg.write_words(table, &ws)?;

    let product = cg.m.finish();
    product.emit().map_err(internal)
}

/// シムの戻り（`p` 語・`i` 32 ビット整数・`v` なし）。`rt_spec` の戻りの印と同じ。
type ShimRet = char;

/// シム 1 つ（公開する関数, 呼ぶ Sigil の関数の番号, 引数の数, 戻り）。
type Shim = (FuncId, usize, usize, ShimRet);

/// 引用で `\u{...}` に逃がす符号点の範囲（両端含む、補足 H-4）。
/// `str` の `Debug`（Rust の `char::escape_debug` のうち、引用符とバックスラッシュ以外）と同じ判定を
/// 全符号点について調べて作る。コンパイラと評価器が同じ Rust に従うので、判定は一致する。
/// プロセスで 1 回だけ作る。名前つきの逃がし（`\t`・`\n`・`\r`・`\\`・`\"`）は生成コードが直接見るので含めない。
fn quote_escape_ranges() -> &'static [(i64, i64)] {
    use std::sync::OnceLock;
    static RANGES: OnceLock<Vec<(i64, i64)>> = OnceLock::new();
    RANGES.get_or_init(|| {
        let mut out: Vec<(i64, i64)> = vec![];
        for cp in 0..0x110000u32 {
            let Some(c) = char::from_u32(cp) else {
                continue;
            };
            if matches!(c, '\t' | '\n' | '\r' | '\\' | '"' | '\'') {
                continue;
            }
            // `'` は `char::escape_debug` が逃がすが `str` の `Debug` は逃がさないので、上で除いた。
            // それ以外は両者の判定が一致する（逃がすなら先頭が `\` になる）。
            let mut it = c.escape_debug();
            let escaped = it.next() == Some('\\');
            if escaped {
                match out.last_mut() {
                    Some((_, hi)) if *hi + 1 == cp as i64 => *hi = cp as i64,
                    _ => out.push((cp as i64, cp as i64)),
                }
            }
        }
        out
    })
}

pub(crate) fn shim_sig(cc: CallConv, np: usize, ret: ShimRet) -> Signature {
    let mut s = Signature::new(cc);
    for _ in 0..np {
        s.params.push(AbiParam::new(types::I64));
    }
    match ret {
        'p' => s.returns.push(AbiParam::new(types::I64)),
        'i' => s.returns.push(AbiParam::new(types::I32)),
        _ => {}
    }
    s
}

/// 確保器（`std.os.alloc`）の関数が SIR にあれば、C の呼び出し規約のシムを宣言して公開する（補足 B-2・B-5）。
/// 生成コードの呼び出し先の名前（`sigil_rt_raw_alloc`/`sigil_rt_raw_free`）は変えない（補足 A-2）。
fn alloc_shims(cg: &mut Cg<'_>) -> Result<Vec<Shim>, CgError> {
    use crate::stdlib::embedded::{ALLOC_FN, FREE_FN, LIVE_FN};
    let pos = |n: &str| cg.p.funcs.iter().position(|f| f.name == n);
    let (Some(a), Some(f), Some(l)) = (pos(ALLOC_FN), pos(FREE_FN), pos(LIVE_FN)) else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for (sym, target, np, ret) in [
        ("sigil_rt_raw_alloc", a, 2, 'p'),
        ("sigil_rt_raw_free", f, 3, 'v'),
        ("sigil_rt_sigil_live_blocks", l, 0, 'p'),
    ] {
        let id =
            cg.m.declare_function(sym, Linkage::Export, &shim_sig(cg.cc, np, ret))
                .map_err(internal)?;
        out.push((id, target, np, ret));
    }
    cg.rt.insert("raw_alloc", out[0].0);
    cg.rt.insert("raw_free", out[1].0);
    cg.live_shim = Some(out[2].0);
    Ok(out)
}

/// 文字列の表現（`std.core.string`）の関数を呼ぶ C の呼び出し規約のシムを宣言し、`rt` に登録する（補足 D-3）。
/// 生成コードの呼び出し先の名前と形は変えない。シムは公開せず（`Linkage::Local`）、番地を関数表で Rust ランタイムに渡す。
fn str_shims(cg: &mut Cg<'_>) -> Result<Vec<Shim>, CgError> {
    use crate::stdlib::embedded::{
        STR_BYTES_FN, STR_CLONE_FN, STR_CMP_FN, STR_CONCAT_FN, STR_DATA_FN, STR_EQ_FN, STR_FREE_FN,
        STR_FROM_UTF8_FN, STR_IS_EMPTY_FN, STR_LEN_FN, STR_LINES_FN, STR_NEW_LOSSY_FN,
        STR_PARSE_INT_FN, STR_TRIM_FN,
    };
    let mut out = vec![];
    let mut ids = vec![];
    for (name, target_fn, np, ret) in [
        ("str_new", STR_NEW_LOSSY_FN, 2, 'p'),
        ("str_free", STR_FREE_FN, 1, 'v'),
        ("str_clone", STR_CLONE_FN, 1, 'p'),
        ("str_data", STR_DATA_FN, 1, 'p'),
        ("str_bytes", STR_BYTES_FN, 1, 'p'),
        ("str_len", STR_LEN_FN, 1, 'p'),
        ("str_is_empty", STR_IS_EMPTY_FN, 1, 'i'),
        ("str_trim", STR_TRIM_FN, 1, 'p'),
        ("str_concat", STR_CONCAT_FN, 2, 'p'),
        ("str_parse_int", STR_PARSE_INT_FN, 2, 'i'),
        ("str_lines", STR_LINES_FN, 1, 'p'),
        ("str_from_utf8", STR_FROM_UTF8_FN, 3, 'p'),
        ("str_eq", STR_EQ_FN, 2, 'i'),
        ("str_cmp", STR_CMP_FN, 2, 'p'),
    ] {
        let Some(target) = cg.p.funcs.iter().position(|f| f.name == target_fn) else {
            return Err(internal("std.core.string が結ばれていない".to_string()));
        };
        let id =
            cg.m.declare_function(
                &format!("sigil_rt_{name}"),
                Linkage::Local,
                &shim_sig(cg.cc, np, ret),
            )
            .map_err(internal)?;
        cg.rt.insert(name, id);
        ids.push(id);
        out.push((id, target, np, ret));
    }
    cg.str_ops = Some([ids[0], ids[1], ids[2], ids[3], ids[4]]);
    Ok(out)
}

/// 一覧の表現（`std.core.list`）の関数を呼ぶ C の呼び出し規約のシムを宣言し、`rt` に登録する（補足 E-3）。
/// 生成コードの呼び出し先の名前と形は変えない。シムは公開せず（`Linkage::Local`）、番地を関数表で Rust ランタイムに渡す。
fn list_shims(cg: &mut Cg<'_>) -> Result<Vec<Shim>, CgError> {
    use crate::stdlib::embedded::{
        LIST_CREATE_FN, LIST_FREE_FN, LIST_GET_FN, LIST_LEN_FN, LIST_PUSH_FN, LIST_SUM_FLOAT_FN,
        LIST_SUM_INT_FN,
    };
    let mut out = vec![];
    let mut ids = vec![];
    // 関数表の順（create・free・push・get・len）。
    for (name, target_fn, np, ret) in [
        ("list_new", LIST_CREATE_FN, 1, 'p'),
        ("list_free", LIST_FREE_FN, 1, 'v'),
        ("list_push", LIST_PUSH_FN, 2, 'v'),
        ("list_get", LIST_GET_FN, 2, 'p'),
        ("list_len", LIST_LEN_FN, 1, 'p'),
        ("list_sum_int", LIST_SUM_INT_FN, 2, 'i'),
        ("list_sum_float", LIST_SUM_FLOAT_FN, 1, 'p'),
    ] {
        let Some(target) = cg.p.funcs.iter().position(|f| f.name == target_fn) else {
            return Err(internal("std.core.list が結ばれていない".to_string()));
        };
        let id =
            cg.m.declare_function(
                &format!("sigil_rt_{name}"),
                Linkage::Local,
                &shim_sig(cg.cc, np, ret),
            )
            .map_err(internal)?;
        cg.rt.insert(name, id);
        ids.push(id);
        out.push((id, target, np, ret));
    }
    cg.list_ops = Some([ids[0], ids[1], ids[2], ids[3], ids[4]]);
    Ok(out)
}

/// バイト列の表現（`std.core.bytes`）の関数を呼ぶ C の呼び出し規約のシムを宣言し、`rt` に登録する（補足 F-4）。
/// `bytes_set`・`bytes_slice` は形が変わる（`Err` の文字列は呼び出し側が作る。F-2）。シムは公開しない。
fn bytes_shims(cg: &mut Cg<'_>) -> Result<Vec<Shim>, CgError> {
    use crate::stdlib::embedded::*;
    let mut out = vec![];
    // 先頭の 5 つは関数表の順（from_raw・free・clone・data・len）。
    for (name, target_fn, np, ret) in [
        ("bytes_from_raw", BYTES_FROM_RAW_FN, 2, 'p'),
        ("bytes_free", BYTES_FREE_FN, 1, 'v'),
        ("bytes_clone", BYTES_CLONE_FN, 1, 'p'),
        ("bytes_data", BYTES_DATA_FN, 1, 'p'),
        ("bytes_len", BYTES_LEN_FN, 1, 'p'),
        ("bytes_new", BYTES_EMPTY_FN, 0, 'p'),
        ("bytes_with_capacity", BYTES_CREATE_FN, 1, 'p'),
        ("bytes_is_empty", BYTES_IS_EMPTY_FN, 1, 'i'),
        ("bytes_get", BYTES_GET_FN, 3, 'i'),
        ("bytes_set", BYTES_SET_FN, 3, 'i'),
        ("bytes_push", BYTES_PUSH_FN, 2, 'v'),
        ("bytes_slice", BYTES_SLICE_FN, 3, 'p'),
        ("bytes_concat", BYTES_CONCAT_FN, 2, 'p'),
        ("bytes_extend", BYTES_EXTEND_FN, 2, 'v'),
        ("bytes_push_u16_be", BYTES_PUSH_U16_BE_FN, 2, 'v'),
        ("bytes_push_u32_be", BYTES_PUSH_U32_BE_FN, 2, 'v'),
        ("bytes_push_u64_be", BYTES_PUSH_U64_BE_FN, 2, 'v'),
        ("bytes_push_u32_le", BYTES_PUSH_U32_LE_FN, 2, 'v'),
        ("bytes_read_u32_be", BYTES_READ_U32_BE_FN, 3, 'i'),
        ("bytes_ct_eq", BYTES_CT_EQ_FN, 2, 'i'),
        ("bytes_eq", BYTES_EQ_FN, 2, 'i'),
        ("bytes_cmp", BYTES_CMP_FN, 2, 'p'),
    ] {
        let Some(target) = cg.p.funcs.iter().position(|f| f.name == target_fn) else {
            return Err(internal("std.core.bytes が結ばれていない".to_string()));
        };
        let id =
            cg.m.declare_function(
                &format!("sigil_rt_{name}"),
                Linkage::Local,
                &shim_sig(cg.cc, np, ret),
            )
            .map_err(internal)?;
        cg.rt.insert(name, id);
        out.push((id, target, np, ret));
    }
    cg.bytes_ops = Some([out[0].0, out[1].0, out[2].0, out[3].0, out[4].0]);
    Ok(out)
}

/// `Int` の回転（`std.core.int`）の関数を呼ぶ C の呼び出し規約のシムを宣言し、`rt` に登録する（補足 F-3）。
/// 生成コードの呼び出し先の名前と形は変えない。関数表は持たない（Rust の消費者がない）。
fn int_shims(cg: &mut Cg<'_>) -> Result<Vec<Shim>, CgError> {
    use crate::stdlib::embedded::{INT_ROTL_FN, INT_ROTR32_FN, INT_ROTR_FN};
    let mut out = vec![];
    for (name, target_fn, np) in [
        ("int_rotl", INT_ROTL_FN, 3),
        ("int_rotr", INT_ROTR_FN, 3),
        ("int_rotr32", INT_ROTR32_FN, 2),
    ] {
        let Some(target) = cg.p.funcs.iter().position(|f| f.name == target_fn) else {
            return Err(internal("std.core.int が結ばれていない".to_string()));
        };
        let id =
            cg.m.declare_function(
                &format!("sigil_rt_{name}"),
                Linkage::Local,
                &shim_sig(cg.cc, np, 'p'),
            )
            .map_err(internal)?;
        cg.rt.insert(name, id);
        out.push((id, target, np, 'p'));
    }
    Ok(out)
}

/// 生成コードの中で定義する関数（`memmove`・`memset`）。
#[derive(Clone, Copy)]
enum MemFn {
    Move,
    Set,
}

/// `memmove`・`memset` を宣言して `rt` に登録する（呼び出しは `rt("memmove"|"memset", ..)` のまま）。
/// 本体は `Fx::mem_move`・`Fx::mem_set`。シンボルは公開しない（libc のものと衝突しない）。
fn mem_fns(cg: &mut Cg<'_>) -> Result<Vec<(FuncId, MemFn)>, CgError> {
    let mut out = vec![];
    for (name, sym, kind) in [
        ("memmove", "sigil_memmove", MemFn::Move),
        ("memset", "sigil_memset", MemFn::Set),
    ] {
        let mut s = Signature::new(cg.cc);
        for _ in 0..3 {
            s.params.push(AbiParam::new(types::I64));
        }
        let id =
            cg.m.declare_function(sym, Linkage::Local, &s)
                .map_err(internal)?;
        cg.rt.insert(name, id);
        out.push((id, kind));
    }
    Ok(out)
}

/// 関数のシンボル名。番号で一意になり、名前（モジュールをまたぐ関数は `<モジュール>.<関数>`）を添える
/// （デバッガ・シンボル表で、どのモジュールの関数かが分かる）。
pub(crate) fn func_symbol(i: usize, name: &str) -> String {
    let n: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("sigil_f{i}_{n}")
}

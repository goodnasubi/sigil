//! SIR から Cranelift でオブジェクトを作る（ADR 0015 の決定 4・6・7、PR4）。
//!
//! 値の持ち方（`runtime/core/src/val.rs` と同じ）: どの値も 8 バイトの語。`Int`・`Bool` はそのまま、`Float` は
//! 64 ビットの並び、`String`・`Path` は `RtStr*`、一覧は `RtList*`（要素は語）、組・構造体は語の塊（フィールド `i` は `8*i`）、
//! 列挙型は `[腕の番号][フィールド…]`（フィールド `i` は `8+8*i`、塊は少なくとも 16 バイト）、`Cell`・`Shared` は中身の語を
//! 持つ塊への番地、参照は指す先の語の番地、関数の値は `[関数の番地][環境の語（なければ 0）]` の塊への番地。
//!
//! 関数は `(語…) -> (値, 停止の符号)` の形で呼ぶ（符号が 0 でなければ停止中で、`Resume` が呼び出し元の `cleanup` へ返す）。
//! 局所は 8 バイトの stack slot に置き、基本ブロックの境界をまたぐ値は slot を通す（ブロック引数は使わない）。
//! 表示・比較・複写は型の記述子（読み取り専用のデータ）をランタイムへ渡して行う。破棄は SIR の `Stmt::Drop`・`Stmt::Assign` の旧値で、型の記述子を使う `sigil_rt_val_drop` を呼んで行う（ヘッダつきの塊は参照数で解放し、`--trace-drops` では構造体の破棄を順に標準エラーへ出す）。文字列・パスも同じ道で解放する（ADR 0018 の決定 5）。既知の制限: クロージャの破棄の時期と停止の経路での漏れは SIR の評価器と厳密には揃えない。
//!
//! 出せない構文・組み込みはない（SIR の不変条件が崩れたときだけ `CgError::Internal`）。

mod func;

use crate::exec::StopKind;
use crate::ir::{Func, GlobalInit, Program, Ty, TypeKind};
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
        "str_clone" | "str_data" | "str_bytes" | "str_len" | "str_trim" => ("p", 'p'),
        "str_free" => ("p", 'v'),
        "str_is_empty" => ("p", 'i'),
        "str_parse_int" => ("pp", 'i'),
        _ => return None,
    })
}

/// ランタイム関数の引数と戻りの形（`p` 語・`i` 32 ビット整数・`f` 倍精度・`v` なし）。
pub(crate) fn rt_spec(name: &str) -> Option<(&'static str, char)> {
    Some(match name {
        "alloc" | "list_get" | "val_shadow" | "val_clone" => ("pp", 'p'),
        "blk_new" | "str_lines" | "list_len" | "list_new" => ("p", 'p'),
        "bytes_new" => ("", 'p'),
        "bytes_with_capacity" | "bytes_clone" | "bytes_len" | "str_as_bytes" => ("p", 'p'),
        "bytes_concat" => ("pp", 'p'),
        "bytes_push" | "bytes_extend" | "bytes_push_u16_be" | "bytes_push_u32_be"
        | "bytes_push_u64_be" | "bytes_push_u32_le" => ("pp", 'v'),
        "bytes_free" => ("p", 'v'),
        "bytes_is_empty" => ("p", 'i'),
        "bytes_ct_eq" | "int_rotr32" => ("pp", 'i'),
        "bytes_get" | "bytes_read_u32_be" | "bytes_from_list" | "bytes_to_string_utf8" => {
            ("ppp", 'i')
        }
        "int_rotl" | "int_rotr" => ("ppp", 'i'),
        "bytes_set" => ("pppp", 'i'),
        "bytes_slice" => ("ppppp", 'i'),
        "memmove" | "memset" | "raw_free" => ("ppp", 'v'),
        "raw_alloc" => ("pp", 'p'),
        "set_sigil_live" => ("p", 'v'),
        "set_str_ops" => ("ppppp", 'v'),
        "raw_syscall" => ("ppppppp", 'p'),
        "list_sum_int" | "enter" => ("pp", 'i'),
        "list_push" | "console_log" | "os_buf_free" | "stop" | "val_drop" => ("pp", 'v'),
        "trace_on" => ("", 'v'),
        "print_result" => ("p", 'v'),
        "stack_limit" => ("", 'p'),
        "val_show" => ("ppi", 'p'),
        "val_eq" | "val_cmp" => ("ppp", 'i'),
        "list_sum_float" => ("p", 'f'),
        "entry_args" => ("ippppppp", 'i'),
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
    descs: HashMap<Ty, DataId>,
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
}

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

    /// 型の記述子のデータ。
    pub fn desc(&mut self, ty: &Ty) -> Result<DataId, CgError> {
        self.desc_n(&norm_with(ty, true))
    }

    fn desc_n(&mut self, ty: &Ty) -> Result<DataId, CgError> {
        if let Some(d) = self.descs.get(ty) {
            return Ok(*d);
        }
        let id = self.data_anon()?;
        self.descs.insert(ty.clone(), id);
        let p = self.p;
        let mut ws: Vec<W> = vec![];
        match ty {
            Ty::Unit => ws.push(W::I(0)),
            Ty::Int => ws.push(W::I(1)),
            Ty::Float => ws.push(W::I(2)),
            Ty::Bool => ws.push(W::I(3)),
            Ty::Str => ws.push(W::I(4)),
            Ty::Bytes => ws.push(W::I(15)),
            Ty::Addr => ws.push(W::I(16)),
            Ty::Path => ws.push(W::I(5)),
            Ty::ParseError => ws.push(W::I(6)),
            Ty::List(e) => {
                ws.push(W::I(7));
                ws.push(W::D(self.desc(e)?));
            }
            Ty::Tuple(ts) => {
                ws.push(W::I(8));
                ws.push(W::I(ts.len() as i64));
                for t in ts {
                    ws.push(W::D(self.desc(t)?));
                }
            }
            Ty::Adt(t) => {
                let def = &p.types[*t];
                match &def.kind {
                    TypeKind::Struct { fields } => {
                        ws.push(W::I(9));
                        ws.extend(self.name_words(&def.name)?);
                        ws.push(W::I(fields.len() as i64));
                        for (n, ft) in fields {
                            ws.extend(self.name_words(n)?);
                            ws.push(W::D(self.desc(ft)?));
                        }
                    }
                    TypeKind::Enum { variants } => {
                        ws.push(W::I(10));
                        ws.extend(self.name_words(&def.name)?);
                        ws.push(W::I((def.name == "Option" || def.name == "Result") as i64));
                        ws.push(W::I(variants.len() as i64));
                        for v in variants {
                            ws.extend(self.name_words(&v.name)?);
                            ws.push(W::I(v.fields.len() as i64));
                            for ft in &v.fields {
                                ws.push(W::D(self.desc(ft)?));
                            }
                        }
                    }
                }
            }
            Ty::Ref(i) | Ty::MutRef(i) => {
                ws.push(W::I(11));
                ws.push(W::D(self.desc(i)?));
            }
            Ty::Shared(i) => {
                ws.push(W::I(12));
                ws.push(W::D(self.desc(i)?));
            }
            Ty::Cell(i) => {
                ws.push(W::I(13));
                ws.push(W::D(self.desc(i)?));
            }
            Ty::FnRef(..) => ws.push(W::I(14)),
        }
        self.write_words(id, &ws)?;
        Ok(id)
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
        descs: HashMap::new(),
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
    };

    // Sigil 製の確保器のシム（ADR 0025 補足 B-2・B-5）。確保器の関数を結んだときだけ公開する。
    let mut shims = alloc_shims(&mut cg)?;
    // 文字列の表現（`std.core.string`）のシム（補足 D-3）。常に必要で、結ばれていなければ内部エラーで止める。
    shims.extend(str_shims(&mut cg)?);
    // `memmove`・`memset`（補足 C-3）。libc のものは使わず、同じオブジェクトの中で定義する。
    let mem = mem_fns(&mut cg)?;

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

fn shim_sig(cc: CallConv, np: usize, ret: ShimRet) -> Signature {
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
        STR_BYTES_FN, STR_CLONE_FN, STR_CONCAT_FN, STR_DATA_FN, STR_FREE_FN, STR_IS_EMPTY_FN,
        STR_LEN_FN, STR_NEW_LOSSY_FN, STR_PARSE_INT_FN, STR_TRIM_FN,
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

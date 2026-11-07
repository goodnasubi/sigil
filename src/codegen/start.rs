//! Linux x86_64 の起動用の短い機械語（ADR 0025 C-4）。
use super::*;
use cranelift_codegen::binemit::Reloc;
use cranelift_module::{ModuleReloc, ModuleRelocTarget};

// mov rdi,rsp; and rsp,-16; call sigil_boot; ud2
pub(super) const START: &[u8] = &[
    0x48, 0x89, 0xe7, 0x48, 0x83, 0xe4, 0xf0, 0xe8, 0, 0, 0, 0, 0x0f, 0x0b,
];
// SysV の (番号,rdi,rsi,rdx,r10,r8,r9) へ並べ替え。第 7 引数は戻り番地の次。
const SYSCALL: &[u8] = &[
    0x48, 0x89, 0xf8, 0x48, 0x89, 0xf7, 0x48, 0x89, 0xd6, 0x48, 0x89, 0xca, 0x4d, 0x89, 0xc2, 0x4d,
    0x89, 0xc8, 0x4c, 0x8b, 0x4c, 0x24, 0x08, 0x0f, 0x05, 0xc3,
];
// mov rsp,rdi; mov rdi,rdx; call rsi; ud2（戻らない）。上端は 16B 境界。
const SWITCH: &[u8] = &[0x48, 0x89, 0xfc, 0x48, 0x89, 0xd7, 0xff, 0xd6, 0x0f, 0x0b];

pub(super) fn syscall(cg: &mut Cg<'_>) -> Result<(), CgError> {
    let id =
        cg.m.declare_function("sigil_syscall6", Linkage::Export, &shim_sig(cg.cc, 7, 'p'))
            .map_err(internal)?;
    cg.m.define_function_bytes(id, 1, SYSCALL, &[])
        .map_err(internal)?;
    cg.rt.insert("syscall6", id);
    Ok(())
}

pub(super) fn prepare(cg: &mut Cg<'_>) -> Result<DataId, CgError> {
    let limit = cg.m.declare_anonymous_data(true, false).map_err(internal)?;
    let mut data = DataDescription::new();
    data.define_zeroinit(8);
    data.set_align(8);
    cg.m.define_data(limit, &data).map_err(internal)?;
    let lim =
        cg.m.declare_function(
            "sigil_stack_limit",
            Linkage::Local,
            &shim_sig(cg.cc, 0, 'p'),
        )
        .map_err(internal)?;
    cg.rt.insert("stack_limit", lim);
    // 番号は宣言された FuncId を使う（通常の SIR 関数の番号より後）。
    cg.define(lim, lim.as_u32(), shim_sig(cg.cc, 0, 'p'), vec![], |fx| {
        fx.start_limit(limit)
    })?;
    Ok(limit)
}

pub(super) fn define(cg: &mut Cg<'_>, thunk: FuncId, limit: DataId) -> Result<(), CgError> {
    let run =
        cg.m.declare_function("sigil_run", Linkage::Local, &shim_sig(cg.cc, 1, 'v'))
            .map_err(internal)?;
    cg.define(run, run.as_u32(), shim_sig(cg.cc, 1, 'v'), vec![], |fx| {
        fx.run_start(thunk)
    })?;
    let switch =
        cg.m.declare_function(
            "sigil_switch_stack",
            Linkage::Local,
            &shim_sig(cg.cc, 3, 'v'),
        )
        .map_err(internal)?;
    cg.m.define_function_bytes(switch, 1, SWITCH, &[])
        .map_err(internal)?;
    let mut entry_sig = Signature::new(cg.cc);
    entry_sig.params = vec![
        AbiParam::new(types::I32),
        AbiParam::new(types::I64),
        AbiParam::new(types::I64),
    ];
    entry_sig.returns.push(AbiParam::new(types::I32));
    let entry =
        cg.m.declare_function("sigil_start", Linkage::Local, &entry_sig)
            .map_err(internal)?;
    cg.define(entry, entry.as_u32(), entry_sig, vec![], |fx| {
        fx.start_stack(limit, run, switch)
    })?;
    // -nostartfiles で外れる crt の空のフック。静的・非 PIE なので自己再配置も不要。
    for symbol in ["_init", "_fini", "_dl_relocate_static_pie"] {
        let sig = shim_sig(cg.cc, 0, 'v');
        let id =
            cg.m.declare_function(symbol, Linkage::Export, &sig)
                .map_err(internal)?;
        cg.define(id, id.as_u32(), sig, vec![], |fx| fx.start_noop())?;
    }
    // 3-7c まで glibc を残す。TLS・IFUNC・Rust の消費者を初期化してから Sigil の起動へ入る。
    let libc =
        cg.m.declare_function(
            "__libc_start_main",
            Linkage::Import,
            &shim_sig(cg.cc, 7, 'i'),
        )
        .map_err(internal)?;
    let boot =
        cg.m.declare_function("sigil_boot", Linkage::Local, &shim_sig(cg.cc, 1, 'v'))
            .map_err(internal)?;
    cg.define(boot, boot.as_u32(), shim_sig(cg.cc, 1, 'v'), vec![], |fx| {
        fx.start_boot(entry, libc)
    })?;
    let start =
        cg.m.declare_function("_start", Linkage::Export, &shim_sig(cg.cc, 0, 'v'))
            .map_err(internal)?;
    cg.m.define_function_bytes(
        start,
        16,
        START,
        &[ModuleReloc {
            offset: 8,
            kind: Reloc::X86CallPCRel4,
            name: ModuleRelocTarget::user(0, boot.as_u32()),
            addend: -4,
        }],
    )
    .map_err(internal)?;
    Ok(())
}

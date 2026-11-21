//! Issue #645 / ADR 0054 決定 2・7: x86-64 freestanding 出力と入口 ABI のテスト。
//!
//! 確かめる項目:
//! - `targets bare` の機能制約（ADR 0014）と独立に `Target::X86_64Freestanding`（`native::build_freestanding`）を
//!   選択でき、確保を使わない Sigil プログラムを静的 ELF として出力できる。
//! - 生成 ELF が起動契約（`sigil-os` の `docs/boot-contract.md`・`boot/contract.h`・`tests/check-probe.sh`）を満たす:
//!   - ELF64 little-endian・System V OSABI (`ELFOSABI_NONE`)・`ET_EXEC`・`EM_X86_64`
//!   - ファイルサイズ 2 MiB 以下（`<= 0x00200000`）
//!   - `1 <= e_phnum <= 32`
//!   - すべての `PT_LOAD` が `[0x02000000, 0x02200000)` の範囲内で `p_vaddr == p_paddr`、互いに重ならない
//!   - 入口アドレス `e_entry` が実行フラグ（`PF_X`）を持つ `PT_LOAD` のファイルデータ内にある
//!   - 動的リンク（`PT_INTERP`・`PT_DYNAMIC`・`.interp`・`.dynamic`・`NEEDED`）・再配置・TLS を含まない
//!   - 未解決シンボル（`nm -u`）が不在
//!   - 生成コードと同梱ランタイムに確保器（`alloc`・`free`・`std.os.alloc`）や `std.os`・`syscall` の参照がない
//!   - 逆アセンブリにホスト OS システムコール命令（`syscall`・`sysenter`・`int`）がない
//! - 入口 ABI が起動契約どおりである:
//!   - 入口は `RDI` に起動情報 v1（40 バイト `struct boot_info`）へのポインタを受け取る
//!   - Linux の `_start` の前提（スタック上の `argc`/`argv`、`mmap`/`mprotect` によるスタック確保、Linux の終了システムコール）を使わない
//!   - 起動情報 v1 の読み取り・大域状態（`.data` / `.bss`）の読み書き・`raw.*` 原始操作・純粋な関数呼び出しと契約検査が正しく生成される
//! - 確保や OS 機能を使うプログラムを freestanding 出力で指定した場合は、エラーで拒否され、出力ファイルが残らない。
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod common;

use common::native::scratch;
use sigil::codegen::{CgError, Opts, Target};
use sigil::exec::RunError;
use sigil::native::{self, BuildError, FREESTANDING_KERNEL_BASE, FREESTANDING_KERNEL_WINDOW};
use std::path::{Path, PathBuf};
use std::process::Command;

fn tool(name: &str, args: &[&str], file: &Path) -> String {
    let o = Command::new(name).args(args).arg(file).output().unwrap();
    assert!(
        o.status.success(),
        "{name} {:?}: {}",
        args,
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// `sigil-os` の `boot/loader.c` および `tests/check-probe.sh` と同じ契約で ELF を検査する。
fn assert_boot_contract_elf(elf: &Path) {
    let bytes = std::fs::read(elf).unwrap();
    let size = bytes.len() as u64;
    assert!(size >= 64, "ELF ヘッダより小さい: {size}");
    assert!(
        size <= FREESTANDING_KERNEL_WINDOW,
        "ファイルサイズが 2 MiB を超えている: {size}"
    );

    // ELF64 little-endian, EV_CURRENT, ELFOSABI_NONE (0)
    assert_eq!(&bytes[0..4], b"\x7fELF", "ELF マジック");
    assert_eq!(bytes[4], 2, "ELFCLASS64");
    assert_eq!(bytes[5], 1, "ELFDATA2LSB");
    assert_eq!(bytes[6], 1, "EV_CURRENT");
    assert_eq!(bytes[7], 0, "ELFOSABI_NONE");

    let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let u64at = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().unwrap());

    let e_type = u16at(16);
    let e_machine = u16at(18);
    let e_version = u32at(20);
    let e_entry = u64at(24);
    let e_phoff = u64at(32) as usize;
    let e_ehsize = u16at(52);
    let e_phentsize = u16at(54) as usize;
    let e_phnum = u16at(56) as usize;

    assert_eq!(e_type, 2, "ET_EXEC でない: {e_type}");
    assert_eq!(e_machine, 62, "EM_X86_64 でない: {e_machine}");
    assert_eq!(e_version, 1, "e_version");
    assert_eq!(e_ehsize, 64, "e_ehsize");
    assert_eq!(e_phentsize, 56, "e_phentsize");
    assert!(
        (1..=32).contains(&e_phnum),
        "プログラムヘッダ数が 1..=32 の範囲外: {e_phnum}"
    );
    assert!(
        e_phoff >= 64 && e_phoff + e_phnum * e_phentsize <= bytes.len(),
        "プログラムヘッダ表がファイル範囲外"
    );

    const PT_NULL: u32 = 0;
    const PT_LOAD: u32 = 1;
    const PT_DYNAMIC: u32 = 2;
    const PT_INTERP: u32 = 3;
    const PT_NOTE: u32 = 4;
    const PT_PHDR: u32 = 6;
    const PT_TLS: u32 = 7;
    const PT_GNU_STACK: u32 = 0x6474_e551;
    const PF_X: u32 = 1;

    let mut loads: Vec<(u64, u64)> = Vec::new();
    let mut entry_in_text = false;

    for i in 0..e_phnum {
        let base = e_phoff + i * e_phentsize;
        let p_type = u32at(base);
        let p_flags = u32at(base + 4);
        let p_offset = u64at(base + 8);
        let p_vaddr = u64at(base + 16);
        let p_paddr = u64at(base + 24);
        let p_filesz = u64at(base + 32);
        let p_memsz = u64at(base + 40);
        let p_align = u64at(base + 48);

        assert!(
            !matches!(p_type, PT_DYNAMIC | PT_INTERP | PT_TLS),
            "禁止されたセグメント種別が含まれている: {p_type}"
        );
        if matches!(p_type, PT_NULL | PT_NOTE | PT_PHDR | PT_GNU_STACK) {
            continue;
        }
        assert_eq!(p_type, PT_LOAD, "契約外のプログラムヘッダ種別: {p_type}");
        assert_eq!(
            p_vaddr, p_paddr,
            "PT_LOAD の仮想アドレスと物理アドレスが一致しない: {p_vaddr:#x} != {p_paddr:#x}"
        );
        assert!(
            (FREESTANDING_KERNEL_BASE..FREESTANDING_KERNEL_BASE + FREESTANDING_KERNEL_WINDOW)
                .contains(&p_vaddr),
            "PT_LOAD の開始番地が [0x02000000, 0x02200000) の外: {p_vaddr:#x}"
        );
        assert!(
            p_memsz <= FREESTANDING_KERNEL_BASE + FREESTANDING_KERNEL_WINDOW - p_vaddr,
            "PT_LOAD のメモリ範囲が 0x02200000 を超える: vaddr={p_vaddr:#x} memsz={p_memsz:#x}"
        );
        assert!(
            p_filesz <= p_memsz,
            "p_filesz > p_memsz: {p_filesz} > {p_memsz}"
        );
        assert!(
            p_offset <= size && p_filesz <= size - p_offset,
            "PT_LOAD のファイル範囲がファイル外"
        );
        if p_align > 1 {
            assert!(
                p_align.is_power_of_two(),
                "p_align が 2 の冪でない: {p_align}"
            );
            assert_eq!(
                p_vaddr % p_align,
                p_offset % p_align,
                "p_vaddr と p_offset の整列剰余が一致しない"
            );
        }
        if p_memsz == 0 {
            continue;
        }
        let end = p_vaddr + p_memsz;
        for &(other_start, other_end) in &loads {
            assert!(
                !(p_vaddr < other_end && other_start < end),
                "PT_LOAD セグメントが重なっている: [{p_vaddr:#x}, {end:#x}) と [{other_start:#x}, {other_end:#x})"
            );
        }
        if (p_flags & PF_X) != 0 && e_entry >= p_vaddr && e_entry - p_vaddr < p_filesz {
            entry_in_text = true;
        }
        loads.push((p_vaddr, end));
    }
    assert!(!loads.is_empty(), "PT_LOAD セグメントが存在しない");
    assert!(
        entry_in_text,
        "入口アドレス {e_entry:#x} が PF_X を持つ PT_LOAD のファイルデータ内にない"
    );

    // readelf / nm / objdump による検査（sigil-os/tests/check-probe.sh と同条件）
    let headers = tool("readelf", &["-hlWd"], elf);
    assert!(headers.contains("EXEC"), "ET_EXEC でない:\n{headers}");
    for bad in ["INTERP", "DYNAMIC", "NEEDED", "TLS"] {
        assert!(!headers.contains(bad), "{bad} が含まれている:\n{headers}");
    }
    let sections = tool("readelf", &["-SW"], elf);
    for bad in [".interp", ".dynamic", ".rela.", ".rel.", ".tbss", ".tdata"] {
        assert!(
            !sections.contains(bad),
            "不要なセクション {bad} が残っている:\n{sections}"
        );
    }

    let undefined = tool("nm", &["-u"], elf);
    assert!(
        undefined.trim().is_empty(),
        "未解決シンボルがある:\n{undefined}"
    );

    let symbols = tool("nm", &[], elf);
    for line in symbols.lines() {
        let name = line.split_whitespace().last().unwrap_or("");
        let lower = name.to_ascii_lowercase();
        assert!(
            !lower.contains("alloc"),
            "確保器のシンボルが残っている: {line}"
        );
        assert!(
            !lower.contains("syscall"),
            "システムコールのシンボルが残っている: {line}"
        );
        assert!(
            !(lower == "free"
                || lower.starts_with("free")
                || lower.contains("_free")
                || lower.contains(".free")
                || lower.contains("blkfree")),
            "解放関連のシンボルが残っている: {line}"
        );
        assert!(
            !lower.contains("std.os."),
            "std.os のシンボルが残っている: {line}"
        );
        for linux_boot in [
            "sigil_start",
            "sigil_run",
            "sigil_thunk",
            "sigil_switch_stack",
            "sigil_stack_limit",
            "sigil_thread_entry",
        ] {
            assert_ne!(
                name, linux_boot,
                "Linux 起動用のシンボルが残っている: {name}"
            );
        }
    }

    let disasm = tool("objdump", &["-d", "--no-show-raw-insn"], elf);
    for line in disasm.lines() {
        // 命令列に syscall / sysenter / int が含まれないことを確かめる
        let tokens: Vec<&str> = line.split_whitespace().collect();
        for tok in tokens {
            let t = tok.to_ascii_lowercase();
            assert!(
                !matches!(t.as_str(), "syscall" | "sysenter" | "int"),
                "ホスト ABI / 割り込み命令が生成されている: {line}"
            );
        }
    }
}

fn build_fs(srcs: &[&str], entry: &str, trust_base: &[&str], tag: &str) -> PathBuf {
    let dir = scratch(tag);
    let elf = dir.join("kernel.elf");
    let opts = Opts {
        trust_base: trust_base.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    native::build_freestanding(srcs, entry, &opts, &elf)
        .unwrap_or_else(|e| panic!("freestanding ビルドに失敗 ({tag}): {e}"));
    elf
}

#[test]
fn target_independent_of_targets_bare_and_meets_boot_contract() {
    // ADR 0054 決定 2: `targets bare` は機能制約であり、出力選択と独立に選べる。
    // `targets bare` でも `targets server` でも、確保と OS 機能を使わないプログラムなら freestanding ELF を出力できる。
    for (i, targets) in ["bare", "server", "bare, browser, server"]
        .iter()
        .enumerate()
    {
        let src = format!(
            "module m.kernel\n  targets {targets}\n  trust none\n\n\
             fn helper(x: Int) -> Int\n  intent \"純粋な補助計算\"\n  requires x >= 0\n  ensures result > x\n  effects none\n{{\n  return x + 1;\n}}\n\n\
             fn kernel_entry(boot_info: Int) -> Int\n  intent \"起動契約の入口\"\n  requires none\n  ensures none\n  effects none\n{{\n  if boot_info <= 0 {{\n    return 0;\n  }}\n  return helper(41);\n}}\n"
        );
        let elf = build_fs(&[&src], "kernel_entry", &[], &format!("p6-targets-{i}"));
        assert_boot_contract_elf(&elf);
        let _ = std::fs::remove_dir_all(elf.parent().unwrap());
    }
}

#[test]
fn entry_abi_reads_boot_info_v1_and_uses_globals_and_raw_ops() {
    // sigil-os の probe.c と同じ起動情報 v1（40 バイト）の検査・初期化済み大域（.data / .bss）・
    // raw.* 原始操作・Int 回転を Sigil で書き、freestanding ELF にコンパイルできることを確かめる。
    let src = r#"module kernel.boot
  targets bare
  trust base

global INITIALIZED: Int = 0x12345678
global mut CLEARED: Int = 0

fn read_u32_le(base: Addr, off: Int) -> Int
  intent "リトルエンディアンの 32 ビット符号なし整数を読む"
  requires off >= 0
  ensures result >= 0
  effects none
  unchecked "呼び出し側が 40 バイトの起動情報 v1 の範囲内を渡す"
{
  let p = raw.offset(base, off);
  let b0 = raw.load_u8(p);
  let b1 = raw.load_u8(raw.offset(p, 1));
  let b2 = raw.load_u8(raw.offset(p, 2));
  let b3 = raw.load_u8(raw.offset(p, 3));
  return b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
}

fn check_boot_info(info: Addr) -> Bool
  intent "起動情報 v1（40 バイト）の版・サイズ・予約領域・メモリマップの整合を検査する"
  requires none
  ensures none
  effects none
  unchecked "UEFI ローダーが渡した const struct boot_info * を読み取り専用で検査する"
{
  if raw.addr_to_int(info) == 0 {
    return false;
  }
  let version = read_u32_le(info, 0);
  let size = read_u32_le(info, 4);
  if version != 1 || size != 40 {
    return false;
  }
  let mmap = raw.load_i64(raw.offset(info, 8));
  let mmap_size = raw.load_i64(raw.offset(info, 16));
  let desc_size = raw.load_i64(raw.offset(info, 24));
  let desc_ver = read_u32_le(info, 32);
  let reserved = read_u32_le(info, 36);
  if reserved != 0 || mmap <= 0 || mmap % 8 != 0 {
    return false;
  }
  if desc_ver != 1 || desc_size < 40 || desc_size % 8 != 0 {
    return false;
  }
  if mmap_size < desc_size || mmap_size % desc_size != 0 {
    return false;
  }
  if INITIALIZED != 0x12345678 || CLEARED != 0 {
    return false;
  }
  CLEARED = version.rotl(4, 64);
  return CLEARED == 16;
}

pub fn kernel_entry(info: Addr)
  intent "カーネルの freestanding 入口"
  requires none
  ensures none
  effects none
  unchecked "起動情報を受け取り検査する"
{
  let ok = check_boot_info(info);
  if !ok {
    return;
  }
}
"#;
    let elf = build_fs(&[src], "kernel_entry", &["kernel.boot"], "p6-boot-info");
    assert_boot_contract_elf(&elf);

    // 入口 `_start` の機械語を逆アセンブルして、RDI を上書きせず `sigil_boot` を呼び、
    // 復帰時は `cli; hlt; jmp` で停止することを確かめる。
    let disasm = tool("objdump", &["-d", "--disassemble=_start"], &elf);
    assert!(
        !disasm.contains("mov    %rsp,%rdi") && !disasm.contains("mov %rsp,%rdi"),
        "Linux 用の mov rdi, rsp が含まれている:\n{disasm}"
    );
    assert!(
        disasm.contains("cli") && disasm.contains("hlt"),
        "復帰阻止の cli / hlt が含まれていない:\n{disasm}"
    );
    let _ = std::fs::remove_dir_all(elf.parent().unwrap());
}

#[test]
fn raw_copy_and_set_work_without_allocator_or_libc() {
    let src = r#"module kernel.mem
  targets bare
  trust base

pub fn kernel_entry(info: Addr) -> Int
  intent "自前の memmove / memset を使う freestanding プログラム"
  requires none
  ensures none
  effects none
  unchecked "テスト用の番地計算と複写"
{
  if raw.addr_to_int(info) != 0 {
    raw.copy(info, info, 0);
    raw.set(info, 0, 0);
  }
  return 0;
}
"#;
    let elf = build_fs(&[src], "kernel_entry", &["kernel.mem"], "p6-mem");
    assert_boot_contract_elf(&elf);
    let _ = std::fs::remove_dir_all(elf.parent().unwrap());
}

fn assert_rejected(src: &str, entry: &str, trust_base: &[&str], want_substr: &str) {
    let dir = scratch("p6-reject");
    let out = dir.join("kernel.elf");
    let opts = Opts {
        trust_base: trust_base.iter().map(|s| (*s).to_string()).collect(),
        ..Default::default()
    };
    let res = native::build_freestanding(&[src], entry, &opts, &out);
    let exists = out.exists();
    let _ = std::fs::remove_dir_all(&dir);
    let err = res.expect_err(&format!(
        "拒否されるべきプログラムが通った ({want_substr}):\n{src}"
    ));
    assert!(
        !exists,
        "ビルド拒否時に出力ファイルが残っている ({want_substr})"
    );
    let msg = err.to_string();
    assert!(
        msg.contains(want_substr),
        "エラー文言に {want_substr:?} が含まれない: {msg}"
    );
    assert!(
        matches!(
            err,
            BuildError::Run(RunError::Unsupported(_) | RunError::Entry(_))
        ),
        "期待したエラー種別でない: {err:?}"
    );
}

#[test]
fn rejects_allocation_and_os_features_in_freestanding_mode() {
    // 1. raw.alloc / raw.free の直接使用
    let raw_alloc_src = "module m.a\n  targets bare\n  trust base\n\n\
        fn main(info: Addr) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"x\"\n{\n\
          let p = raw.alloc(16, 8);\n  raw.free(p, 16, 8);\n  return raw.addr_to_int(info);\n}\n";
    assert_rejected(raw_alloc_src, "main", &["m.a"], "raw.alloc");

    // 2. raw.syscall の直接使用
    let raw_syscall_src = "module m.a\n  targets server\n  trust base\n\n\
        fn main(info: Addr) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n  unchecked \"x\"\n{\n\
          return raw.syscall(231, raw.addr_to_int(info), 0, 0, 0, 0, 0);\n}\n";
    assert_rejected(raw_syscall_src, "main", &["m.a"], "raw.syscall");

    // 3. String（確保を要する型）
    let str_src = "module m.a\n  targets bare\n  trust none\n\n\
        fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n\
          let s = \"hello\";\n  return s.char_count();\n}\n";
    assert_rejected(str_src, "main", &[], "確保");

    // 4. List（確保を要する型）
    let list_src = "module m.a\n  targets bare\n  trust none\n\n\
        fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n\
          let mut xs = List.new();\n  xs.push(1);\n  return xs.len();\n}\n";
    assert_rejected(list_src, "main", &[], "確保");

    // 5. Bytes（確保を要する型）
    let bytes_src = "module m.a\n  targets bare\n  trust none\n\n\
        fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n\
          let b = Bytes.new();\n  return b.byte_len();\n}\n";
    assert_rejected(bytes_src, "main", &[], "確保");

    // 6. console.log（OS 機能）
    let console_src = "module m.a\n  targets server\n  trust none\n\n\
        fn main()\n  intent \"x\"\n  requires none\n  ensures none\n  effects console\n{\n\
          console.log(&42);\n}\n";
    assert_rejected(console_src, "main", &[], "OS 機能");

    // 7. 入口の引数が不正（2 個以上、または Addr/Int 以外）
    let bad_entry_arity = "module m.a\n  targets bare\n  trust none\n\n\
        fn main(a: Int, b: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n\
          return a + b;\n}\n";
    assert_rejected(bad_entry_arity, "main", &[], "freestanding 入口");

    let bad_entry_ty = "module m.a\n  targets bare\n  trust none\n\n\
        fn main(a: Bool) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n\
          if a { return 1; }\n  return 0;\n}\n";
    assert_rejected(bad_entry_ty, "main", &[], "freestanding 入口");

    // 8. --trace-drops の指定は freestanding では拒否
    let ok_src = "module m.a\n  targets bare\n  trust none\n\n\
        fn main(info: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n\
          return info;\n}\n";
    let p = sigil::ir_eval::lower_sources(&[ok_src], "main").unwrap();
    let err = sigil::codegen::compile_for(
        &p,
        &Opts {
            trace_drops: true,
            ..Default::default()
        },
        Target::X86_64Freestanding,
    )
    .expect_err("--trace-drops が拒否されるべき");
    assert!(matches!(err, CgError::Unsupported(_)), "{err:?}");
}

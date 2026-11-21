//! ネイティブ実行ファイルの出力（ADR 0015 の決定 4・6・7、ADR 0054 の決定 2・7）。
//!
//! SIR を `codegen` でオブジェクトにし、一時ディレクトリに書き出してリンクする（ADR 0025 補足 C-7 の 3-7c）。
//! Linux 出力は `cc -nostdlib -static -no-pie` で結び、`_start` からシステムコールまで生成コードが持つ。
//! freestanding 出力は専用のリンカスクリプト（`[0x02000000, 0x02200000)` の `ET_EXEC`、仮想番地と物理番地が一致）で
//! 静的に結び、ホスト ABI・確保器・`std.os` に依存しない ELF を作る。一時ディレクトリは終わりに自分で消す。

use crate::codegen::{self, CgError, Opts, Target};
use crate::exec::RunError;
use crate::ir::Program;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// P6-1 起動契約（`boot/contract.h`・`docs/boot-contract.md`）のアドレス範囲とリンカスクリプト。
pub const FREESTANDING_KERNEL_BASE: u64 = 0x0200_0000;
pub const FREESTANDING_KERNEL_WINDOW: u64 = 0x0020_0000;

const FREESTANDING_LDSCRIPT: &str = "\
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)
PHDRS {
    text PT_LOAD FLAGS(5);
    data PT_LOAD FLAGS(6);
}
SECTIONS {
    . = 0x02000000;
    .text : { *(.text*) *(.rodata*) } :text
    . = ALIGN(0x1000);
    .data : { *(.data*) } :data
    .bss : { *(.bss*) *(COMMON) } :data
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
";

#[derive(Debug)]
pub enum BuildError {
    /// 構文・型検査のエラー、入口の指定の誤りなど。
    Run(RunError),
    /// `cc` がない・リンクに失敗した。
    Toolchain(String),
    /// 生成の不具合・ファイルの入出力の失敗。
    Internal(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Run(e) => write!(f, "{e}"),
            BuildError::Toolchain(m) | BuildError::Internal(m) => write!(f, " {m}"),
        }
    }
}

/// ソースの並び（先頭が入口のモジュール。`run_sources` と同じ結び方）から実行ファイルを作る。
pub fn build(srcs: &[&str], func: &str, opts: &Opts, out: &Path) -> Result<(), BuildError> {
    build_for(srcs, func, opts, Target::LinuxX86_64, out)
}

/// ソースの並びから指定した出力先 `target` の実行ファイルを作る。
pub fn build_for(
    srcs: &[&str],
    func: &str,
    opts: &Opts,
    target: Target,
    out: &Path,
) -> Result<(), BuildError> {
    let p = crate::ir_eval::lower_sources_with(srcs, func, &opts.trust_base)
        .map_err(BuildError::Run)?;
    build_program_for(&p, opts, target, out)
}

/// ソースの並びから x86-64 freestanding の静的 ELF を作る（ADR 0054 決定 2・7）。
pub fn build_freestanding(
    srcs: &[&str],
    func: &str,
    opts: &Opts,
    out: &Path,
) -> Result<(), BuildError> {
    build_for(srcs, func, opts, Target::X86_64Freestanding, out)
}

/// 入口のファイルから `use` を `root` で辿って結び、実行ファイルを作る（`sigil build`）。
pub fn build_project(
    root: &Path,
    entry: &Path,
    func: &str,
    opts: &Opts,
    out: &Path,
) -> Result<(), BuildError> {
    build_project_for(root, entry, func, opts, Target::LinuxX86_64, out)
}

/// 入口のファイルから `use` を `root` で辿って結び、指定した出力先 `target` の実行ファイルを作る。
pub fn build_project_for(
    root: &Path,
    entry: &Path,
    func: &str,
    opts: &Opts,
    target: Target,
    out: &Path,
) -> Result<(), BuildError> {
    let p = crate::ir_eval::lower_project(root, entry, func, &opts.trust_base)
        .map_err(BuildError::Run)?;
    build_program_for(&p, opts, target, out)
}

/// 入口のファイルから `use` を `root` で辿って結び、x86-64 freestanding の静的 ELF を作る。
pub fn build_project_freestanding(
    root: &Path,
    entry: &Path,
    func: &str,
    opts: &Opts,
    out: &Path,
) -> Result<(), BuildError> {
    build_project_for(root, entry, func, opts, Target::X86_64Freestanding, out)
}

/// SIR から実行ファイルを作る（Linux x86-64 向け）。
pub fn build_program(p: &Program, opts: &Opts, out: &Path) -> Result<(), BuildError> {
    build_program_for(p, opts, Target::LinuxX86_64, out)
}

/// SIR から指定した出力先 `target` の実行ファイルを作る。
pub fn build_program_for(
    p: &Program,
    opts: &Opts,
    target: Target,
    out: &Path,
) -> Result<(), BuildError> {
    if !cfg!(all(target_arch = "x86_64", target_os = "linux")) {
        return Err(BuildError::Toolchain(
            "ネイティブ出力は Linux x86_64 のみ対応".into(),
        ));
    }
    let obj = codegen::compile_for(p, opts, target).map_err(|e| match e {
        CgError::Entry(m) => BuildError::Run(RunError::Entry(m)),
        CgError::Unsupported(m) => BuildError::Run(RunError::Unsupported(m)),
        CgError::Internal(m) => BuildError::Internal(m),
    })?;
    let dir = fresh_dir()
        .map_err(|e| BuildError::Internal(format!("作業ディレクトリを作れない: {e}")))?;
    let r = link(&dir, &obj, target, out);
    let _ = std::fs::remove_dir_all(&dir);
    r
}

fn fresh_dir() -> std::io::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // 同じプロセスの複数のスレッドが同じ時刻を読むことがあるので、連番も付ける。
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir =
        std::env::temp_dir().join(format!("sigil-build-{}-{nanos}-{seq}", std::process::id()));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

fn link(dir: &Path, obj: &[u8], target: Target, out: &Path) -> Result<(), BuildError> {
    let o = dir.join("out.o");
    std::fs::write(&o, obj)
        .map_err(|e| BuildError::Internal(format!("オブジェクトを書けない: {e}")))?;
    // 暫定ランタイム（`core.a`・`os.a`）も glibc も結ばない。`_start` から必要な最小コードまで生成コードが持つ
    // （ADR 0025 補足 C-4・C-7 の 3-7c、ADR 0054 決定 2）。`cc` はリンカの起動役にだけ使う。
    let mut cmd = Command::new("cc");
    cmd.arg("-nostdlib").arg("-static").arg("-no-pie");
    if target == Target::X86_64Freestanding {
        let ld = dir.join("freestanding.ld");
        std::fs::write(&ld, FREESTANDING_LDSCRIPT)
            .map_err(|e| BuildError::Internal(format!("リンカスクリプトを書けない: {e}")))?;
        cmd.arg("-Wl,--no-undefined")
            .arg("-Wl,--build-id=none")
            .arg("-Wl,-T")
            .arg(&ld);
    }
    cmd.arg("-o").arg(out).arg(&o);
    let res = cmd.output();
    match res {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(BuildError::Toolchain(
            "リンカ cc が見つからない。ビルドには cc（gcc か clang）が要る".into(),
        )),
        Err(e) => Err(BuildError::Toolchain(format!("cc を実行できない: {e}"))),
        Ok(o) if !o.status.success() => Err(BuildError::Toolchain(format!(
            "リンクに失敗した:\n{}",
            String::from_utf8_lossy(&o.stderr)
        ))),
        Ok(_) => {
            if target == Target::X86_64Freestanding {
                verify_freestanding_elf(out)?;
            }
            Ok(())
        }
    }
}

/// 生成した freestanding ELF が起動契約（2 MiB 以下など）を満たすか確かめる。
fn verify_freestanding_elf(out: &Path) -> Result<(), BuildError> {
    let meta = std::fs::metadata(out)
        .map_err(|e| BuildError::Internal(format!("出力 ELF を確認できない: {e}")))?;
    if meta.len() > FREESTANDING_KERNEL_WINDOW {
        let _ = std::fs::remove_file(out);
        return Err(BuildError::Internal(format!(
            "freestanding ELF の大きさが上限（2 MiB）を超えている: {} バイト",
            meta.len()
        )));
    }
    Ok(())
}

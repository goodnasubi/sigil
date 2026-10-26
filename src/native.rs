//! ネイティブ実行ファイルの出力（ADR 0015 の決定 4・6・7）。
//!
//! SIR を `codegen` でオブジェクトにし、埋め込みのランタイム（`runtime_lib`）を一時ディレクトリに
//! 書き出して、`cc -static -no-pie` でリンクする。一時ディレクトリは終わりに自分で消す。

use crate::codegen::{self, CgError, Opts};
use crate::interp::RunError;
use crate::ir::Program;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

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

/// ソース（複数ファイルは `run_sources` と同じく結合する）から実行ファイルを作る。
pub fn build(srcs: &[&str], func: &str, opts: &Opts, out: &Path) -> Result<(), BuildError> {
    let p = crate::ir_eval::lower_sources(srcs, func).map_err(BuildError::Run)?;
    build_program(&p, opts, out)
}

/// SIR から実行ファイルを作る。
pub fn build_program(p: &Program, opts: &Opts, out: &Path) -> Result<(), BuildError> {
    let obj = codegen::compile(p, opts).map_err(|e| match e {
        CgError::Entry(m) => BuildError::Run(RunError::Entry(m)),
        CgError::Internal(m) => BuildError::Internal(m),
    })?;
    let dir = fresh_dir()
        .map_err(|e| BuildError::Internal(format!("作業ディレクトリを作れない: {e}")))?;
    let r = link(&dir, &obj, p.core_only, out);
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

fn link(dir: &Path, obj: &[u8], core_only: bool, out: &Path) -> Result<(), BuildError> {
    let io = |what: &str, e: std::io::Error| BuildError::Internal(format!("{what}: {e}"));
    let o = dir.join("out.o");
    let core = dir.join("core.a");
    std::fs::write(&o, obj).map_err(|e| io("オブジェクトを書けない", e))?;
    std::fs::write(&core, crate::runtime_lib::core_a()).map_err(|e| io("core.a を書けない", e))?;
    let mut cmd = Command::new("cc");
    cmd.arg("-static")
        .arg("-no-pie")
        .arg("-o")
        .arg(out)
        .arg(&o)
        .arg(&core);
    if !core_only {
        let os = dir.join("os.a");
        std::fs::write(&os, crate::runtime_lib::os_a()).map_err(|e| io("os.a を書けない", e))?;
        cmd.arg(&os);
    }
    cmd.args(crate::runtime_lib::native_libs());
    let res = cmd.output();
    match res {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(BuildError::Toolchain(
            "リンカ cc が見つからない。ビルドには cc（gcc か clang）と glibc の静的ライブラリ（libc.a）が要る".into(),
        )),
        Err(e) => Err(BuildError::Toolchain(format!("cc を実行できない: {e}"))),
        Ok(o) if !o.status.success() => Err(BuildError::Toolchain(format!(
            "リンクに失敗した（glibc の静的ライブラリ libc.a が要る）:\n{}",
            String::from_utf8_lossy(&o.stderr)
        ))),
        Ok(_) => Ok(()),
    }
}

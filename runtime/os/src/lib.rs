//! Sigil の暫定ランタイムの OS 部（ADR 0015 の決定 6）。
//!
//! `console.log`・`fs.read`・`fs.write`・`Path` は Sigil 実装（`std.os.console`・`std.os.fs`・`std.core.path`）へ移った
//! （ADR 0025 補足 C-7、3-6a・3-6b）。ここに残るのは生のシステムコールの入口 `sigil_rt_raw_syscall` だけ
//! （3-7b で `_start` とともに生成コードへ移る）。

/// # Safety
/// システムコール番号と引数がカーネル ABI に合致し、ポインタ引数は有効な領域を指す。
#[no_mangle]
pub unsafe extern "C" fn sigil_rt_raw_syscall(
    n: i64,
    a0: i64,
    a1: i64,
    a2: i64,
    a3: i64,
    a4: i64,
    a5: i64,
) -> i64 {
    let ret: i64;
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        std::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            in("r10") a3,
            in("r8") a4,
            in("r9") a5,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack)
        );
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (n, a0, a1, a2, a3, a4, a5);
        ret = -1;
    }
    ret
}

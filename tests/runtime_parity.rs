//! ADR 0015 の決定 6（PR3）: 暫定ランタイム（`runtime/core`・`runtime/os`）のリンクの試験。
//! `Path` と `fs` は Sigil 実装へ移った（ADR 0025 補足 C-7、3-6b）ので、その突き合わせは `tests/fs_path_36b.rs` が担う。
//! （値の表示は生成したコードへ移ったので、突き合わせは `tests/show_35f.rs` が担う。ADR 0025 補足 H。）

mod common;
use sigil::runtime_lib;
use std::path::PathBuf;
use std::process::Command;

/// 作業ディレクトリ（`CARGO_TARGET_TMPDIR` の下。`/tmp` には残さない）。同じ名前のものがあれば作り直す。
fn work_dir(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sigil-rt-parity-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// `cc` があるか。なければ理由を標準エラーに出す。
fn have_cc() -> bool {
    let ok = Command::new("cc")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("cc がないので、このテストは飛ばす（ランタイムのリンクの試験は実行されない）");
    }
    ok
}

#[test]
fn core_alone_links_without_os() {
    if !have_cc() {
        return;
    }
    let dir = work_dir("core");
    std::fs::write(dir.join("core.a"), runtime_lib::core_a()).unwrap();
    // 文字列の関数表（補足 D-3）が要る。ここでは最小の表（24B の頭を malloc で作る）を登録する。
    let src = r#"#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef struct RtStr RtStr;
int32_t sigil_rt_enter(int32_t (*)(uint8_t*), uint8_t*);
void sigil_rt_set_str_ops(RtStr* (*)(const uint8_t*, int64_t), void (*)(RtStr*), RtStr* (*)(const RtStr*),
                          const uint8_t* (*)(const RtStr*), int64_t (*)(const RtStr*));
typedef struct { uint8_t* ptr; int64_t len; int64_t cap; } Head;
static RtStr* s_new(const uint8_t* p, int64_t n) {
  Head* h = malloc(sizeof(Head));
  h->ptr = n > 0 ? malloc((size_t)n) : NULL;
  if (n > 0) memcpy(h->ptr, p, (size_t)n);
  h->len = n; h->cap = n;
  return (RtStr*)h;
}
static void s_free(RtStr* s) { Head* h = (Head*)s; if (!h) return; free(h->ptr); free(h); }
static RtStr* s_clone(const RtStr* s) { const Head* h = (const Head*)s; return s_new(h->ptr, h->len); }
static const uint8_t* s_data(const RtStr* s) { return ((const Head*)s)->ptr; }
static int64_t s_bytes(const RtStr* s) { return ((const Head*)s)->len; }
static int32_t entry(uint8_t* arg) {
  (void)arg;
  return 42;
}
int main(void){
  sigil_rt_set_str_ops(s_new, s_free, s_clone, s_data, s_bytes);
  printf("%lld\n", (long long)sigil_rt_enter(entry, NULL));
  return 0;
}
"#;
    std::fs::write(dir.join("d.c"), src).unwrap();
    let o = Command::new("cc")
        .args(["-static", "-no-pie", "d.c", "core.a"])
        .args(runtime_lib::native_libs())
        .args(["-o", "d"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r = Command::new(dir.join("d")).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&r.stdout), "42\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn runtime_crates_own_tests_pass() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("rt-tests");
    for name in ["core", "os"] {
        let o = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["test", "--manifest-path"])
            .arg(root.join("runtime").join(name).join("Cargo.toml"))
            .arg("--target-dir")
            .arg(&target)
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "runtime/{name}: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

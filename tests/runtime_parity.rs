//! ADR 0015 の決定 6（PR3）: 暫定ランタイム（`runtime/core`・`runtime/os`）の文字列・パスの関数が、
//! SIR の評価器と同じ結果を返すこと。
//! （値の表示は生成したコードへ移ったので、突き合わせは `tests/show_35f.rs` が担う。ADR 0025 補足 H。）
//!
//! 埋め込んだ `.a`（`sigil::runtime_lib`）を、C の小さなドライバと `cc -static -no-pie` でリンクして走らせ、
//! 同じ入力を SIR の評価器で評価した結果と突き合わせる。`cc` がなければ飛ばす。
//! 加えて、2 つのランタイムの crate 自身の単体テスト（`cargo test --manifest-path`）も走らせる。

mod common;
use common::run;
use sigil::runtime_lib;
use std::path::PathBuf;
use std::process::Command;

const DRIVER: &str = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef struct RtStr RtStr;
void sigil_rt_set_str_ops(RtStr* (*)(const uint8_t*, int64_t), void (*)(RtStr*), RtStr* (*)(const RtStr*),
                          const uint8_t* (*)(const RtStr*), int64_t (*)(const RtStr*));
void sigil_rt_path_join(const uint8_t*, size_t, const uint8_t*, size_t, uint8_t**, size_t*);
int32_t sigil_rt_path_file_name(const uint8_t*, size_t, uint8_t**, size_t*);
int32_t sigil_rt_path_parent(const uint8_t*, size_t, uint8_t**, size_t*);
void sigil_rt_os_buf_free(uint8_t*, size_t);

/* 文字列の関数表（補足 D-3）。本物は Sigil 製（std.core.string）で、ここでは同じ配置の 24B の頭を malloc で作る。 */
typedef struct { uint8_t* ptr; int64_t len; int64_t cap; } Head;
static RtStr* sigil_rt_str_new(const uint8_t* p, int64_t n) {
  Head* h = malloc(sizeof(Head));
  h->ptr = n > 0 ? malloc((size_t)n) : NULL;
  if (n > 0) memcpy(h->ptr, p, (size_t)n);
  h->len = n; h->cap = n;
  return (RtStr*)h;
}
static void sigil_rt_str_free(RtStr* s) { Head* h = (Head*)s; if (!h) return; free(h->ptr); free(h); }
static RtStr* sigil_rt_str_clone(const RtStr* s) { const Head* h = (const Head*)s; return sigil_rt_str_new(h->ptr, h->len); }
static const uint8_t* sigil_rt_str_data(const RtStr* s) { return ((const Head*)s)->ptr; }
static int64_t sigil_rt_str_bytes(const RtStr* s) { return ((const Head*)s)->len; }

static void put(RtStr* s) { fwrite(sigil_rt_str_data(s), 1, sigil_rt_str_bytes(s), stdout); sigil_rt_str_free(s); }

/* 引用つきで出す（このドライバの入力は `"` と `\` を含まないパスだけなので、その 2 つを逃がせば評価器の表示と一致する）。 */
static void putq(RtStr* s) {
  const uint8_t* p = sigil_rt_str_data(s); int64_t n = sigil_rt_str_bytes(s);
  putchar('"');
  for (int64_t i = 0; i < n; i++) { if (p[i] == '"' || p[i] == '\\') putchar('\\'); putchar(p[i]); }
  putchar('"');
  sigil_rt_str_free(s);
}

int main(int argc, char** argv) {
  const char* cmd = argv[1];
  const char* a = argc > 2 ? argv[2] : "";
  const char* b = argc > 3 ? argv[3] : "";
  sigil_rt_set_str_ops(sigil_rt_str_new, sigil_rt_str_free, sigil_rt_str_clone, sigil_rt_str_data, sigil_rt_str_bytes);
  if (!strcmp(cmd, "join")) {
    uint8_t* p; size_t n; sigil_rt_path_join((const uint8_t*)a, strlen(a), (const uint8_t*)b, strlen(b), &p, &n);
    RtStr* t = sigil_rt_str_new(p, (int64_t)n); sigil_rt_os_buf_free(p, n); putq(t);
  } else if (!strcmp(cmd, "file_name") || !strcmp(cmd, "parent")) {
    uint8_t* p; size_t n;
    int32_t some = cmd[0] == 'f' ? sigil_rt_path_file_name((const uint8_t*)a, strlen(a), &p, &n)
                                 : sigil_rt_path_parent((const uint8_t*)a, strlen(a), &p, &n);
    if (some) { RtStr* t = sigil_rt_str_new(p, (int64_t)n); sigil_rt_os_buf_free(p, n); printf("Some("); putq(t); printf(")"); }
    else printf("None");
  } else { return 2; }
  printf("\n");
  return 0;
}
"#;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

/// 1 つの式を SIR の評価器で評価して表示を返す。
fn ev(ty: &str, body: &str) -> String {
    let src = format!(
        "{HEAD}fn f() -> {ty}\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n\n"
    );
    match run(&src, "f", &[]) {
        Ok(o) => o.value,
        Err(e) => panic!("{e}\n{src}"),
    }
}

/// Sigil の文字列リテラルにする（`\n`・`\r`・`\t`・`\"`・`\\`・`\u{..}`）。
fn lit(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                o.push_str(&format!("\\u{{{:x}}}", c as u32))
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `cc` があるか。なければ理由を標準エラーに出す（どのテストでも、飛ばしたことが分かるように）。
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

/// 作業ディレクトリ（`CARGO_TARGET_TMPDIR` の下。`/tmp` には残さない）。同じ名前のものがあれば作り直す。
fn work_dir(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sigil-rt-parity-{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 埋め込んだ `.a` で C のドライバをリンクして、実行ファイルの場所を返す。`order_os_first` で `.a` の順序を替える。
fn build_driver(tag: &str, order_os_first: bool) -> PathBuf {
    let dir = work_dir(tag);
    std::fs::write(dir.join("core.a"), runtime_lib::core_a()).unwrap();
    std::fs::write(dir.join("os.a"), runtime_lib::os_a()).unwrap();
    std::fs::write(dir.join("d.c"), DRIVER).unwrap();
    let mut c = Command::new("cc");
    c.args(["-static", "-no-pie", "d.c"]);
    if order_os_first {
        c.args(["os.a", "core.a"]);
    } else {
        c.args(["core.a", "os.a"]);
    }
    c.args(runtime_lib::native_libs()).args(["-o", "d"]);
    let o = c.current_dir(&dir).output().unwrap();
    assert!(
        o.status.success(),
        "リンクに失敗: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    dir.join("d")
}

/// 作業ディレクトリを消す。
fn cleanup(exe: &std::path::Path) {
    if let Some(d) = exe.parent() {
        let _ = std::fs::remove_dir_all(d);
    }
}

fn rt(exe: &PathBuf, args: &[&str]) -> String {
    let o = Command::new(exe).args(args).output().unwrap();
    assert!(o.status.success(), "{args:?}");
    let s = String::from_utf8(o.stdout).unwrap();
    s.strip_suffix('\n').unwrap_or(&s).to_string()
}

fn check_all(exe: &PathBuf) {
    for (a, b) in [
        ("/etc/app", "conf.toml"),
        ("a", "b"),
        ("a/", "b"),
        ("", "x"),
        ("a", "/abs"),
        ("a/b", "../c"),
    ] {
        let want = ev(
            "String",
            &format!(
                "let q = Path.new(&{}).join(&{});\nreturn q.file_name().unwrap_or(\"?\");",
                lit(a),
                lit(b)
            ),
        );
        let joined = rt(exe, &["join", a, b]);
        // join の結果の末尾の名前が、SIR の評価器の file_name と一致する。
        let name = rt(exe, &["file_name", joined.trim_matches('"')]);
        assert_eq!(
            name,
            format!("Some({want})").replace("Some(\"?\")", "None"),
            "join {a:?} {b:?}"
        );
    }
    for p in [
        "/",
        "/etc/app/conf.toml",
        "a",
        "a/b/",
        "",
        ".",
        "..",
        "a/..",
    ] {
        let l = lit(p);
        let name = ev(
            "String",
            &format!("return Path.new(&{l}).file_name().unwrap_or(\"?\");"),
        );
        let want = if name == "\"?\"" {
            "None".to_string()
        } else {
            format!("Some({name})")
        };
        assert_eq!(rt(exe, &["file_name", p]), want, "file_name {p:?}");
        let par = ev(
            "String",
            &format!("return match Path.new(&{l}).parent() {{\n  Some(q) => q.file_name().unwrap_or(\"/\"),\n  None => \"?none\",\n}};"),
        );
        let got = rt(exe, &["parent", p]);
        assert_eq!(
            got == "None",
            par == "\"?none\"",
            "parent {p:?}: {got} / {par}"
        );
    }
}

#[test]
fn runtime_matches_sir_eval_core_then_os() {
    if !have_cc() {
        return;
    }
    let exe = build_driver("a", false);
    check_all(&exe);
    cleanup(&exe);
}

#[test]
fn runtime_matches_sir_eval_os_then_core() {
    if !have_cc() {
        return;
    }
    let exe = build_driver("b", true);
    check_all(&exe);
    cleanup(&exe);
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

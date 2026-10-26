//! ADR 0015 の決定 6（PR3）: 暫定ランタイム（`runtime/core`・`runtime/os`）の表示・文字列・パスの関数が、
//! インタープリタと同じ結果を返すこと。
//!
//! 埋め込んだ `.a`（`sigil::runtime_lib`）を、C の小さなドライバと `cc -static -no-pie` でリンクして走らせ、
//! 同じ入力をインタープリタで評価した結果と突き合わせる。`cc` がなければ飛ばす。
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
typedef struct RtList RtList;
RtStr* sigil_rt_str_new(const uint8_t*, size_t);
void sigil_rt_str_free(RtStr*);
const uint8_t* sigil_rt_str_data(const RtStr*);
size_t sigil_rt_str_bytes(const RtStr*);
int64_t sigil_rt_str_len(const RtStr*);
RtStr* sigil_rt_str_trim(const RtStr*);
int32_t sigil_rt_str_parse_int(const RtStr*, int64_t*);
RtList* sigil_rt_str_lines(const RtStr*);
int64_t sigil_rt_list_len(const RtList*);
uint8_t* sigil_rt_list_get(const RtList*, int64_t);
RtStr* sigil_rt_show_int(int64_t);
RtStr* sigil_rt_show_float(double);
RtStr* sigil_rt_show_str(const RtStr*);
void sigil_rt_print_result(const RtStr*);
void sigil_rt_path_join(const uint8_t*, size_t, const uint8_t*, size_t, uint8_t**, size_t*);
int32_t sigil_rt_path_file_name(const uint8_t*, size_t, uint8_t**, size_t*);
int32_t sigil_rt_path_parent(const uint8_t*, size_t, uint8_t**, size_t*);
void sigil_rt_os_buf_free(uint8_t*, size_t);
void sigil_rt_console_log(const uint8_t*, size_t);
void sigil_rt_stop(const uint8_t*, size_t);

static void put(RtStr* s) { fwrite(sigil_rt_str_data(s), 1, sigil_rt_str_bytes(s), stdout); sigil_rt_str_free(s); }

int main(int argc, char** argv) {
  const char* cmd = argv[1];
  const char* a = argc > 2 ? argv[2] : "";
  const char* b = argc > 3 ? argv[3] : "";
  RtStr* s = sigil_rt_str_new((const uint8_t*)a, strlen(a));
  if (!strcmp(cmd, "float")) {
    put(sigil_rt_show_float(strtod(a, NULL)));
  } else if (!strcmp(cmd, "len")) {
    put(sigil_rt_show_int(sigil_rt_str_len(s)));
  } else if (!strcmp(cmd, "trim")) {
    RtStr* t = sigil_rt_str_trim(s); put(sigil_rt_show_str(t)); sigil_rt_str_free(t);
  } else if (!strcmp(cmd, "parse_int")) {
    int64_t n; if (sigil_rt_str_parse_int(s, &n)) { printf("Ok("); put(sigil_rt_show_int(n)); printf(")"); } else printf("Err");
  } else if (!strcmp(cmd, "lines")) {
    RtList* l = sigil_rt_str_lines(s); printf("[");
    for (int64_t i = 0; i < sigil_rt_list_len(l); i++) {
      if (i) printf(", ");
      put(sigil_rt_show_str(*(RtStr**)sigil_rt_list_get(l, i)));
    }
    printf("]");
  } else if (!strcmp(cmd, "join")) {
    uint8_t* p; size_t n; sigil_rt_path_join((const uint8_t*)a, strlen(a), (const uint8_t*)b, strlen(b), &p, &n);
    RtStr* t = sigil_rt_str_new(p, n); sigil_rt_os_buf_free(p, n); put(sigil_rt_show_str(t));
  } else if (!strcmp(cmd, "file_name") || !strcmp(cmd, "parent")) {
    uint8_t* p; size_t n;
    int32_t some = cmd[0] == 'f' ? sigil_rt_path_file_name((const uint8_t*)a, strlen(a), &p, &n)
                                 : sigil_rt_path_parent((const uint8_t*)a, strlen(a), &p, &n);
    if (some) { RtStr* t = sigil_rt_str_new(p, n); sigil_rt_os_buf_free(p, n); printf("Some("); put(sigil_rt_show_str(t)); printf(")"); }
    else printf("None");
  } else if (!strcmp(cmd, "log")) {
    sigil_rt_console_log((const uint8_t*)a, strlen(a)); return 0;
  } else if (!strcmp(cmd, "stop")) {
    printf("before"); sigil_rt_stop((const uint8_t*)a, strlen(a));
  } else { return 2; }
  printf("\n");
  return 0;
}
"#;

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";

/// 1 つの式をインタープリタで評価して表示を返す。
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
    for f in [
        "1.5",
        "3.0",
        "-0.25",
        "0.1",
        "100000.0",
        "2.5",
        "1234567.125",
    ] {
        assert_eq!(
            rt(exe, &["float", f]),
            ev("Float", &format!("return {f};")),
            "float {f}"
        );
    }
    let strs = [
        "",
        "abc",
        "  a b\t\n",
        "\u{3000}x\u{3000}",
        "héllo",
        "日本語",
        "a\nb",
        "a\r\nb\r\n",
        "a\n\nb\n",
        "\n",
        "a\r\r\n",
        "a\rb",
        "q\"uote\\",
    ];
    for s in strs {
        let l = lit(s);
        assert_eq!(
            rt(exe, &["len", s]),
            ev("Int", &format!("return {l}.len();")),
            "len {s:?}"
        );
        assert_eq!(
            rt(exe, &["trim", s]),
            ev("String", &format!("return {l}.trim();")),
            "trim {s:?}"
        );
        assert_eq!(
            rt(exe, &["lines", s]),
            ev("List<String>", &format!("return {l}.lines();")),
            "lines {s:?}"
        );
    }
    for s in [
        "0",
        "42",
        "-7",
        "+1",
        " 1",
        "1_0",
        "",
        "-",
        "9223372036854775807",
        "9223372036854775808",
        "abc",
    ] {
        let want = ev(
            "Result<Int, ParseError>",
            &format!("return {}.parse_int();", lit(s)),
        );
        let want = if want.starts_with("Ok(") {
            want
        } else {
            "Err".to_string()
        };
        assert_eq!(rt(exe, &["parse_int", s]), want, "parse_int {s:?}");
    }
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
        // join の結果の末尾の名前が、インタープリタの file_name と一致する。
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
fn runtime_matches_interpreter_core_then_os() {
    if !have_cc() {
        return;
    }
    let exe = build_driver("a", false);
    check_all(&exe);
    cleanup(&exe);
}

#[test]
fn runtime_matches_interpreter_os_then_core() {
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
    let src = "#include <stdint.h>\n#include <stddef.h>\ntypedef struct RtStr RtStr;\nRtStr* sigil_rt_show_int(int64_t);\nvoid sigil_rt_print_result(const RtStr*);\nint main(void){ sigil_rt_print_result(sigil_rt_show_int(42)); return 0; }\n";
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
fn console_log_and_stop_exit_code() {
    if !have_cc() {
        return;
    }
    let exe = build_driver("d", false);
    let o = Command::new(&exe)
        .args(["stop", "停止: x"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(70));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "before");
    assert_eq!(String::from_utf8_lossy(&o.stderr), "停止: x\n");
    cleanup(&exe);
    console_log_only();
}

fn console_log_only() {
    if !have_cc() {
        return;
    }
    let exe = build_driver("c", false);
    assert_eq!(rt(&exe, &["log", "こんにちは"]), "こんにちは");
    cleanup(&exe);
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

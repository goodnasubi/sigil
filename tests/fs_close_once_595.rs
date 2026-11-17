//! Issue #595: `File::close`／`Dir::close` の後に、同じ fd が破棄フックでもう一度 close されない。
//!
//! 原因: メソッドの本体の `self`（`Expr::SelfValue`）を値として使う（`let mut f = self;`）と、検査器は移動を記録するのに、
//! lowering が常に `Operand::Copy` にしていた。複写された `f` は破棄の記録も fd の close もしない「写し」になり、
//! 元の `self` が元の fd 番号のまま関数の終わりで破棄されて、2 回目の close を発行していた。
//! lowering は評価器とネイティブで共通なので、`common::run_with` は両方の結果を突き合わせる。

mod common;
use common::run_with;
use sigil::exec::{Arg, Options};
use std::path::PathBuf;

const HEAD: &str = "module app.close_once\n  targets server\n  trust none\n\n";

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-close-once-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(src: &str, func: &str, args: &[Arg], opts: &Options) -> sigil::exec::Outcome {
    run_with(&format!("{HEAD}{src}"), func, args, opts).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

/// 値を消費するメソッドの `let mut f = self;` は移動であり、元の値は破棄されない（破棄の記録は 1 回で、書き換えた後の値）。
#[test]
fn consuming_method_moves_self_instead_of_copying() {
    let src = r#"
pub struct Handle {
  fd: Int,
}

fn unit() -> Unit
  intent "unit"
  requires none
  ensures none
  effects none
{
}

impl Handle {
  pub fn close(self: Handle) -> Result<Unit, IoError>
    intent "fd を -1 にして消費する"
    requires none
    ensures none
    effects none
  {
    let mut h = self;
    h.fd = -1;
    return Ok(unit());
  }
}

fn make() -> Result<Handle, IoError>
  intent "作る"
  requires none
  ensures none
  effects none
{
  return Ok(Handle { fd: 4 });
}

pub fn t() -> Int
  intent "match で束縛して close"
  requires none
  ensures none
  effects none
{
  match make() {
    Err(_) => { return 1; }
    Ok(h) => { let _ = h.close(); }
  }
  return 0;
}
"#;
    let opts = Options {
        trace_drops: true,
        ..Options::default()
    };
    let out = run(src, "t", &[], &opts);
    assert_eq!(out.value, "0");
    // 破棄は 1 回だけで、消費する側で書き換えた後の値（元の fd: 4 のままの値が破棄されない）。
    assert_eq!(
        out.drops,
        vec!["Handle { fd: -1 }".to_string()],
        "{:?}",
        out.drops
    );
}

/// 本題: fs A の `File::close`／`Dir::close` の後、破棄されるのは fd を `-1` に書き換えた後の値だけで、元の fd 番号のままの値は破棄されない
/// （破棄の時にその fd を閉じるので、元の番号のままだと 2 回目の close になり、番号が再利用されていれば他のハンドルの fd を閉じる）。
/// 2 回目の close は `close()` の中で 1 回目の直後に起きるため、単一スレッドの fd の再利用では検出できない。破棄の記録の fd で確かめる。
fn fd_of_logged_drops(drops: &[String], ty: &str) -> Vec<i64> {
    drops
        .iter()
        .filter_map(|d| d.strip_prefix(&format!("{ty} {{ fd: ")))
        .map(|rest| {
            rest.split(',')
                .next()
                .unwrap()
                .trim()
                .parse::<i64>()
                .unwrap()
        })
        .collect()
}

#[test]
fn file_close_drops_only_the_closed_value() {
    let dir = scratch("file");
    let a = dir.join("a.txt");
    std::fs::write(&a, "a").unwrap();
    let src = r#"
pub fn t(path_a: String) -> String
  intent "開いて明示的に閉じる"
  requires none
  ensures none
  effects fs
{
  let pa = Path.new(&path_a);
  let rw = fs.open_options().write(true);
  match rw.open(&pa) {
    Err(_) => { return "err_open"; }
    Ok(f) => {
      match f.close() { Err(_) => { return "err_close"; } Ok(_) => {} }
    }
  }
  return "ok";
}
"#;
    let opts = Options {
        trace_drops: true,
        ..Options::default()
    };
    let out = run(src, "t", &[Arg::Str(a.to_string_lossy().into())], &opts);
    assert_eq!(out.value, "\"ok\"");
    let fds = fd_of_logged_drops(&out.drops, "std.os.fs.File");
    assert_eq!(fds, vec![-1], "破棄された File の fd: {:?}", out.drops);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn dir_close_drops_only_the_closed_value() {
    let dir = scratch("dir");
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let src = r#"
pub fn t(path_a: String) -> String
  intent "ディレクトリを開いて明示的に閉じる"
  requires none
  ensures none
  effects fs
{
  let pa = Path.new(&path_a);
  match fs.open_dir(&pa) {
    Err(_) => { return "err_open"; }
    Ok(d) => {
      match d.close() { Err(_) => { return "err_close"; } Ok(_) => {} }
    }
  }
  return "ok";
}
"#;
    let opts = Options {
        trace_drops: true,
        ..Options::default()
    };
    // Dir の破棄の記録には確保した列挙バッファの番地が入り、評価器とネイティブで必ず違うので、評価器だけで確かめる。
    let out = sigil::ir_eval::run_with(
        &format!("{HEAD}{src}"),
        "t",
        &[Arg::Str(sub.to_string_lossy().into())],
        &opts,
    )
    .unwrap_or_else(|e| panic!("{e}\n{src}"));
    assert_eq!(out.value, "\"ok\"");
    let fds = fd_of_logged_drops(&out.drops, "std.os.fs.Dir");
    assert_eq!(fds, vec![-1], "破棄された Dir の fd: {:?}", out.drops);
    let _ = std::fs::remove_dir_all(&dir);
}

/// 明示的に閉じなかった値は、破棄で 1 回だけ閉じる（従来どおり）。
#[test]
fn file_dropped_without_close_is_closed_by_the_drop_hook_once() {
    let dir = scratch("nodrop");
    let a = dir.join("a.txt");
    std::fs::write(&a, "a").unwrap();
    let src = r#"
pub fn t(path_a: String) -> String
  intent "開いて閉じずに破棄する"
  requires none
  ensures none
  effects fs
{
  let pa = Path.new(&path_a);
  let rw = fs.open_options().write(true);
  match rw.open(&pa) {
    Err(_) => { return "err_open"; }
    Ok(f) => { let _ = f.try_lock_exclusive(); }
  }
  return "ok";
}
"#;
    let opts = Options {
        trace_drops: true,
        ..Options::default()
    };
    let out = run(src, "t", &[Arg::Str(a.to_string_lossy().into())], &opts);
    assert_eq!(out.value, "\"ok\"");
    let fds = fd_of_logged_drops(&out.drops, "std.os.fs.File");
    assert_eq!(fds.len(), 1, "{:?}", out.drops);
    assert!(
        fds[0] >= 0,
        "閉じていない値は元の fd のまま破棄される: {fds:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

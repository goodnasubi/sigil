//! ADR 0019 の PR2: `sigil lsp`（診断）と、ADR 0020 の枠の上限。

use sigil::json::{obj, parse, Value};
use sigil::lsp::{path_to_uri, read_frame, Frame, MAX_BODY, MAX_HEADER_LINE};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    out: Receiver<Frame>,
}

const WAIT: Duration = Duration::from_secs(10);

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Client {
    fn start(env: &[(&str, &str)]) -> Client {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_sigil"));
        cmd.arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let stdin = child.stdin.take();
        // 読み取りは別のスレッドで行い、待ちすぎたらテストを失敗させる（hang させない）。
        let mut rd = BufReader::new(child.stdout.take().unwrap());
        let (tx, out) = channel();
        std::thread::spawn(move || loop {
            let f = read_frame(&mut rd, MAX_BODY);
            let end = matches!(f, Frame::Eof | Frame::Broken(_));
            if tx.send(f).is_err() || end {
                break;
            }
        });
        Client { child, stdin, out }
    }

    fn raw(&mut self, bytes: &[u8]) {
        let w = self.stdin.as_mut().unwrap();
        w.write_all(bytes).unwrap();
        w.flush().unwrap();
    }

    fn send(&mut self, v: Value) {
        let body = v.to_json();
        self.raw(format!("Content-Length: {}\r\n\r\n{body}", body.len()).as_bytes());
    }

    fn recv(&mut self) -> Value {
        match self.out.recv_timeout(WAIT) {
            Ok(Frame::Body(b)) => parse(std::str::from_utf8(&b).unwrap()).unwrap(),
            f => panic!("本文が来ない: {f:?}"),
        }
    }

    /// 終了を待って終了コードを返す（待ちすぎたら失敗）。
    fn exit_code(&mut self) -> i32 {
        let t = Instant::now();
        loop {
            if let Some(st) = self.child.try_wait().unwrap() {
                return st.code().unwrap_or(-1);
            }
            assert!(t.elapsed() < WAIT, "終了しない");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn request(&mut self, id: i64, method: &str, params: Value) {
        self.send(obj(vec![
            ("jsonrpc", "2.0".into()),
            ("id", id.into()),
            ("method", method.into()),
            ("params", params),
        ]));
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(obj(vec![
            ("jsonrpc", "2.0".into()),
            ("method", method.into()),
            ("params", params),
        ]));
    }

    fn initialize(&mut self, encodings: &[&str]) -> Value {
        let general = obj(vec![(
            "positionEncodings",
            Value::Arr(encodings.iter().map(|e| (*e).into()).collect()),
        )]);
        let caps = obj(vec![("general", general)]);
        self.request(1, "initialize", obj(vec![("capabilities", caps)]));
        let r = self.recv();
        self.notify("initialized", obj(vec![]));
        r
    }

    /// ワークスペースの根（`rootUri`）つきの `initialize`。
    fn initialize_root(&mut self, root: &Path) {
        let caps = obj(vec![("general", obj(vec![]))]);
        let params = obj(vec![
            ("capabilities", caps),
            ("rootUri", path_to_uri(root).into()),
        ]);
        self.request(1, "initialize", params);
        self.recv();
        self.notify("initialized", obj(vec![]));
    }

    fn open(&mut self, path: &Path, text: &str) {
        let doc = obj(vec![
            ("uri", path_to_uri(path).into()),
            ("languageId", "sigil".into()),
            ("version", 1i64.into()),
            ("text", text.into()),
        ]);
        self.notify("textDocument/didOpen", obj(vec![("textDocument", doc)]));
    }

    fn change(&mut self, path: &Path, text: &str) {
        let doc = obj(vec![("uri", path_to_uri(path).into())]);
        let change = obj(vec![("text", text.into())]);
        self.notify(
            "textDocument/didChange",
            obj(vec![
                ("textDocument", doc),
                ("contentChanges", Value::Arr(vec![change])),
            ]),
        );
    }

    fn close(&mut self, path: &Path) {
        let doc = obj(vec![("uri", path_to_uri(path).into())]);
        self.notify("textDocument/didClose", obj(vec![("textDocument", doc)]));
    }

    /// 次の `publishDiagnostics` の (URI, 診断の配列)。
    fn publish(&mut self) -> (String, Vec<Value>) {
        let m = self.recv();
        assert_eq!(
            m.get("method").and_then(Value::as_str),
            Some("textDocument/publishDiagnostics"),
            "{m}"
        );
        let p = m.get("params").unwrap();
        let uri = p.get("uri").and_then(Value::as_str).unwrap().to_string();
        let d = p.get("diagnostics").and_then(Value::as_array).unwrap();
        (uri, d.to_vec())
    }

    /// `n` 件の `publishDiagnostics` を受けて、URI の順に並べる。
    fn publishes(&mut self, n: usize) -> Vec<(String, Vec<Value>)> {
        let mut v: Vec<_> = (0..n).map(|_| self.publish()).collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// `shutdown` の応答までに届いた通知を集め、`exit` して終了コードを返す。
    fn finish(mut self) -> (Vec<(String, Vec<Value>)>, i32) {
        self.request(99, "shutdown", Value::Null);
        let mut got = vec![];
        loop {
            let m = self.recv();
            if m.get("id").and_then(Value::as_i64) == Some(99) {
                break;
            }
            let p = m.get("params").unwrap();
            let uri = p.get("uri").and_then(Value::as_str).unwrap().to_string();
            let d = p.get("diagnostics").and_then(Value::as_array).unwrap();
            got.push((uri, d.to_vec()));
        }
        self.notify("exit", Value::Null);
        drop(self.stdin.take());
        (got, self.exit_code())
    }
}

fn tmpdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sigil-m6-lsp-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, text).unwrap();
    p
}

fn example(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join(rel)
}

/// ソースを、`module` の宣言の名前に対応するパス（`<根>/a/b.sigil`）に置く。
fn stage(root: &Path, text: &str) -> PathBuf {
    let name = text
        .lines()
        .find_map(|l| l.strip_prefix("module "))
        .unwrap()
        .trim()
        .replace('.', "/");
    let p = root.join(format!("{name}.sigil"));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, text).unwrap();
    p
}

/// 例 04 の組（入口, `pub` にした取り込まれる側）を根の下に置く。(入口の位置, 取り込まれる側の位置, 入口の中身, 取り込まれる側の中身)。
fn effects_staged(root: &Path) -> (PathBuf, PathBuf, String, String) {
    let eff = std::fs::read_to_string(example("04-effects/effects.sigil"))
        .unwrap()
        .replace("\nfn count_up", "\npub fn count_up")
        .replace("\nfn with_tax", "\npub fn with_tax");
    let main = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/effects_main.sigil"),
    )
    .unwrap();
    (stage(root, &main), stage(root, &eff), main, eff)
}

const HEAD: &str = "module m\n  targets server\n  trust none\n\n";
const FN_HEAD: &str =
    "fn f(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n";

fn pos(d: &Value) -> (i64, i64, i64, i64) {
    let r = d.get("range").unwrap();
    let g = |k: &str, f: &str| r.get(k).unwrap().get(f).and_then(Value::as_i64).unwrap();
    (
        g("start", "line"),
        g("start", "character"),
        g("end", "line"),
        g("end", "character"),
    )
}

fn code(d: &Value) -> &str {
    d.get("code").and_then(Value::as_str).unwrap()
}

#[test]
fn lsp_samples_no_diagnostics() {
    // 例は `module` の名前がリポジトリの配置と合わないので、根の下に名前どおりのパスで複製して開く。
    let root = tmpdir("samples");
    let mut files: Vec<PathBuf> = [
        "01-average/c-brace.sigil",
        "02-read-port/c-brace.sigil",
        "03-ownership/ownership.sigil",
        "05-control/control.sigil",
    ]
    .iter()
    .map(|f| {
        let p = example(f);
        stage(&root, &std::fs::read_to_string(p).unwrap())
    })
    .collect();
    let (main_p, eff_p, main, eff) = effects_staged(&root);
    let mut c = Client::start(&[]);
    let r = c.initialize(&[]);
    let caps = r.get("result").unwrap().get("capabilities").unwrap();
    assert_eq!(
        caps.get("positionEncoding").and_then(Value::as_str),
        Some("utf-16")
    );
    for p in &files {
        c.open(p, &std::fs::read_to_string(p).unwrap());
    }
    c.open(&eff_p, &eff);
    c.open(&main_p, &main);
    files.push(eff_p);
    files.push(main_p);
    let (got, code) = c.finish();
    assert_eq!(code, 0);
    for (uri, d) in &got {
        assert!(d.is_empty(), "{uri}: {d:?}");
    }
    for p in &files {
        assert!(
            got.iter().any(|(u, _)| *u == path_to_uri(p)),
            "{} に診断が送られない",
            p.display()
        );
    }
}

fn move_src(second_use: &str) -> String {
    format!("{HEAD}struct S\n{{\n  a: Int,\n}}\n\nfn take(s: S)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return;\n}}\n\nfn f(s: S) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  take(s);\n{second_use}  return 1;\n}}\n")
}

#[test]
fn lsp_error_diagnostic() {
    let dir = tmpdir("error");
    let p = write(&dir, "m.sigil", &move_src(""));
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&p, &move_src("  take(s);\n"));
    let (uri, d) = c.publish();
    assert_eq!(uri, path_to_uri(&p));
    assert_eq!(d.len(), 1);
    assert_eq!(code(&d[0]), "UseAfterMove");
    // 26 行目の `take`（0 始まりの行 25、文字 2〜6）
    assert_eq!(pos(&d[0]), (25, 2, 25, 6));
    assert_eq!(d[0].get("severity").and_then(Value::as_i64), Some(1));
    assert_eq!(d[0].get("source").and_then(Value::as_str), Some("sigil"));
    assert!(d[0]
        .get("message")
        .and_then(Value::as_str)
        .unwrap()
        .contains("移動"));
    c.change(&p, &move_src(""));
    let (_, d) = c.publish();
    assert!(d.is_empty());
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_utf16_positions() {
    let dir = tmpdir("utf16");
    let p = write(&dir, "m.sigil", "");
    // 12 行目: `  let s = "あ😀"; let t: Int = true;`。誤りは 2 つ目の `let`（文字の位置 16）
    let src = format!("{HEAD}{FN_HEAD}  let s = \"あ😀\"; let t: Int = true;\n  return n;\n}}\n");
    for (enc, expect) in [("utf-16", (10, 17, 10, 20)), ("utf-32", (10, 16, 10, 19))] {
        let mut c = Client::start(&[]);
        let r = c.initialize(&["utf-8", enc]);
        let caps = r.get("result").unwrap().get("capabilities").unwrap();
        assert_eq!(
            caps.get("positionEncoding").and_then(Value::as_str),
            Some(enc)
        );
        c.open(&p, &src);
        let (_, d) = c.publish();
        assert_eq!(code(&d[0]), "TypeMismatch");
        assert_eq!(pos(&d[0]), expect, "{enc}");
        c.finish();
    }
}

#[test]
fn lsp_shutdown() {
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    assert_eq!(c.finish().1, 0);
    // `shutdown` なしの `exit` は 1
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.notify("exit", Value::Null);
    drop(c.stdin.take());
    assert_eq!(c.exit_code(), 1);
    // `initialize` の前の要求と、`shutdown` の後の要求は拒む
    let mut c = Client::start(&[]);
    c.request(5, "shutdown", Value::Null);
    let e = c.recv();
    assert_eq!(
        e.get("error").unwrap().get("code").and_then(Value::as_i64),
        Some(-32002)
    );
    c.initialize(&[]);
    c.request(6, "foo/bar", Value::Null);
    let e = c.recv();
    assert_eq!(
        e.get("error").unwrap().get("code").and_then(Value::as_i64),
        Some(-32601)
    );
    c.request(7, "shutdown", Value::Null);
    assert!(c.recv().get("result").is_some());
    c.request(8, "foo/bar", Value::Null);
    let e = c.recv();
    assert_eq!(
        e.get("error").unwrap().get("code").and_then(Value::as_i64),
        Some(-32600)
    );
    c.notify("exit", Value::Null);
    drop(c.stdin.take());
    assert_eq!(c.exit_code(), 0);
}

#[test]
fn lsp_internal_error_contained() {
    let dir = tmpdir("panic");
    let bad_text = "module bad\n  targets server\n  trust none\n\n// PANIC_HERE\n";
    let ok_text = format!(
        "{}{FN_HEAD}  return n;\n}}\n",
        "module ok\n  targets server\n  trust none\n\n"
    );
    let bad = write(&dir, "bad.sigil", bad_text);
    let ok = write(&dir, "ok.sigil", &ok_text);
    let mut c = Client::start(&[("SIGIL_LSP_TEST_PANIC", "PANIC_HERE")]);
    c.initialize(&[]);
    c.open(&bad, bad_text);
    let (uri, d) = c.publish();
    assert_eq!(uri, path_to_uri(&bad));
    assert_eq!((d.len(), code(&d[0])), (1, "InternalError"));
    assert_eq!(pos(&d[0]), (0, 0, 0, 0));
    assert!(d[0]
        .get("message")
        .and_then(Value::as_str)
        .unwrap()
        .contains("注入された panic"));
    // 続けて次の要求に応える（状態は検査の前のまま。別のモジュールなので配信は開いた文書だけ）
    c.open(&ok, &ok_text);
    let (uri, d) = c.publish();
    assert_eq!((uri, d.is_empty()), (path_to_uri(&ok), true));
    c.change(&bad, &bad_text.replace("PANIC_HERE", "直した"));
    let (uri, d) = c.publish();
    assert_eq!((uri, d.is_empty()), (path_to_uri(&bad), true));
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_lex_error_ranges() {
    // (名前, 中身, 期待する種類, 範囲)
    let body = |line: &str| format!("{HEAD}{FN_HEAD}{line}\n  return n;\n}}\n");
    type Range = (i64, i64, i64, i64);
    let cases: Vec<(&str, String, &str, Range)> = vec![
        // 日本語の文字列の後ろの絵文字（UTF-16 で 2 単位）
        (
            "emoji",
            body("  let s = \"あ\"; let t = 😀;"),
            "NonAsciiOutsideLiteral",
            (10, 23, 10, 25),
        ),
        ("tab", body("\tlet s = 1;"), "Tab", (10, 0, 10, 1)),
        // CRLF: 1 行目の行末の CR
        (
            "crlf",
            "module m.a\r\n  targets server\r\n".into(),
            "CarriageReturn",
            (0, 10, 0, 11),
        ),
        ("bom", format!("\u{FEFF}{HEAD}"), "Bom", (0, 0, 0, 1)),
        // 末尾の改行がなく、ファイルの終わりで起きる構文エラーは長さ 0
        ("eof", format!("{HEAD}fn f("), "Unexpected", (4, 5, 4, 5)),
    ];
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    for (name, text, kind, range) in cases {
        let p = tmpdir(&format!("lex-{name}")).join("m.sigil");
        c.open(&p, &text);
        let (_, d) = c.publish();
        assert_eq!((d.len(), code(&d[0])), (1, kind), "{name}");
        assert_eq!(pos(&d[0]), range, "{name}");
    }
    c.finish();
}

#[test]
fn lsp_use_publishes_across_modules() {
    // 入口が `use` で取り込むモジュールの誤りは、取り込まれる側のファイルに診断として配信される。
    let root = tmpdir("multi");
    let (pn, pe, main, eff) = effects_staged(&root);
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&pe, &eff);
    let (u, d) = c.publish();
    assert_eq!((u, d.is_empty()), (path_to_uri(&pe), true));
    c.open(&pn, &main);
    let (u, d) = c.publish();
    assert_eq!((u, d.is_empty()), (path_to_uri(&pn), true));
    // 取り込まれる側の関数の名前を変えると、開いている入口の `use` が解決できなくなる
    c.change(&pe, &eff.replace("fn with_tax", "fn with_tax2"));
    let got = c.publishes(2);
    assert_eq!(got[0].0.clone(), path_to_uri(&pe).min(path_to_uri(&pn)));
    let by = |u: &str| got.iter().find(|g| g.0 == u).unwrap().1.clone();
    assert!(by(&path_to_uri(&pe)).is_empty());
    let d = by(&path_to_uri(&pn));
    assert_eq!((d.len(), code(&d[0])), (1, "UnresolvedUse"), "{d:?}");
    // 戻すと、空の配列で消える
    c.change(&pe, &eff);
    assert!(c.publishes(2).iter().all(|(_, d)| d.is_empty()));
    // 取り込まれる側の本文の型の誤りは、その文書に 1 件だけ（入口からも自分自身からも見えるが、重ねない）
    let bad = eff.replacen("return ", "let z: Int = true;\n  return ", 1);
    c.change(&pe, &bad);
    let got = c.publishes(2);
    let d = by_uri(&got, &pe);
    assert_eq!((d.len(), code(&d[0])), (1, "TypeMismatch"), "{got:?}");
    assert!(by_uri(&got, &pn).is_empty());
    // 閉じると、ディスクの中身（正しい）で入口を検査し直す
    std::fs::write(&pe, &eff).unwrap();
    c.close(&pe);
    let got = c.publishes(2);
    assert!(got.iter().all(|(_, d)| d.is_empty()), "{got:?}");
    assert_eq!(c.finish().1, 0);
}

fn by_uri(got: &[(String, Vec<Value>)], p: &Path) -> Vec<Value> {
    got.iter()
        .find(|g| g.0 == path_to_uri(p))
        .unwrap()
        .1
        .clone()
}

#[test]
fn lsp_error_in_unopened_module_goes_to_its_file() {
    let root = tmpdir("unopened");
    let (pn, pe, main, eff) = effects_staged(&root);
    std::fs::write(
        &pe,
        eff.replacen("return ", "let z: Int = true;\n  return ", 1),
    )
    .unwrap();
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&pn, &main);
    let got = c.publishes(2);
    assert!(by_uri(&got, &pn).is_empty());
    let d = by_uri(&got, &pe);
    assert_eq!((d.len(), code(&d[0])), (1, "TypeMismatch"), "{d:?}");
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_path_must_match_module_name() {
    // `module m` のファイルが `other.sigil` にある。名前とパスが合わない（CLI と同じ ModulePathMismatch）。
    let dir = tmpdir("mismatch");
    let p = write(
        &dir,
        "other.sigil",
        &format!("{HEAD}{FN_HEAD}  return n;\n}}\n"),
    );
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&p, &format!("{HEAD}{FN_HEAD}  return n;\n}}\n"));
    let (_, d) = c.publish();
    assert_eq!((d.len(), code(&d[0])), (1, "ModulePathMismatch"), "{d:?}");
    assert_eq!(c.finish().1, 0);
}

#[cfg(unix)]
#[test]
fn lsp_follows_use_only_inside_the_workspace_root() {
    // 根の中の symlink が根の外のファイルを指していると、`use` で辿れない（`modpath::load` の確認を使う）。
    let base = tmpdir("escape");
    let root = base.join("ws");
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let lib = "module lib.a\n  targets server\n  trust none\n\npub fn step(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n";
    std::fs::write(outside.join("a.sigil"), lib).unwrap();
    std::fs::create_dir_all(root.join("lib")).unwrap();
    std::os::unix::fs::symlink(outside.join("a.sigil"), root.join("lib/a.sigil")).unwrap();
    let main = "module app\n  targets server\n  trust none\n\nuse lib.a.step;\n\nfn f(n: Int) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return step(n);\n}\n";
    let pm = write(&root, "app.sigil", main);
    let mut c = Client::start(&[]);
    c.initialize_root(&root);
    c.open(&pm, main);
    let got = c.publishes(2);
    let d = by_uri(&got, &root.join("lib/a.sigil"));
    assert_eq!((d.len(), code(&d[0])), (1, "ModulePathMismatch"), "{d:?}");
    assert_eq!(c.finish().1, 0);
}

fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
fn lsp_frame_limits_in_process() {
    let rd = |bytes: &[u8], max| read_frame(&mut std::io::Cursor::new(bytes.to_vec()), max);
    assert_eq!(rd(&frame("{}"), 10), Frame::Body(b"{}".to_vec()));
    // 上限を超える本文は読まずに捨て、次の枠から続ける
    let mut two = b"Content-Length: 20\r\n\r\n".to_vec();
    two.extend_from_slice(&[b'x'; 20]);
    two.extend_from_slice(&frame("{}"));
    let mut cur = std::io::Cursor::new(two);
    assert!(matches!(read_frame(&mut cur, 10), Frame::Skipped(_)));
    assert_eq!(read_frame(&mut cur, 10), Frame::Body(b"{}".to_vec()));
    assert_eq!(read_frame(&mut cur, 10), Frame::Eof);
    // 長すぎる見出しの行（Content-Length 以外）は、その枠を捨てて続ける
    let mut long = format!("X-Long: {}\r\n", "a".repeat(MAX_HEADER_LINE + 1)).into_bytes();
    long.extend_from_slice(&frame("{}"));
    long.extend_from_slice(&frame("[]"));
    let mut cur = std::io::Cursor::new(long);
    assert!(matches!(read_frame(&mut cur, 10), Frame::Skipped(_)));
    assert_eq!(read_frame(&mut cur, 10), Frame::Body(b"[]".to_vec()));
    // ちょうど上限の長さの行は受ける
    let ok = format!("X-Long: {}\r\n", "a".repeat(MAX_HEADER_LINE - 9));
    let mut b = ok.into_bytes();
    b.extend_from_slice(&frame("{}"));
    assert_eq!(rd(&b, 10), Frame::Body(b"{}".to_vec()));
    // 続けられないもの: 長さが読めない、長さがない、数でない、食い違う重複、途中で終わる
    let long_len = format!(
        "Content-Length: {}\r\n\r\n",
        "1".repeat(MAX_HEADER_LINE + 1)
    );
    for bad in [
        long_len.as_str(),
        "Content-Type: x\r\n\r\n{}",
        "Content-Length: abc\r\n\r\n{}",
        "Content-Length: -2\r\n\r\n{}",
        "Content-Length: 2\r\nContent-Length: 3\r\n\r\n{}x",
        "Content-Length: 5\r\n\r\n{}",
        "Content-Length: 2\r\n",
        "no colon\r\n\r\n",
    ] {
        assert!(matches!(rd(bad.as_bytes(), 10), Frame::Broken(_)), "{bad}");
    }
    // 同じ値の重複は受ける
    assert_eq!(
        rd(b"content-length: 2\r\nContent-Length: 2\r\n\r\n{}", 10),
        Frame::Body(b"{}".to_vec())
    );
}

#[test]
fn lsp_frame_limits_on_the_wire() {
    // 64 MiB を超える本文は読まずに捨て、エラーの応答を返して続ける
    let mut c = Client::start(&[]);
    let n = MAX_BODY + 1;
    c.raw(format!("Content-Length: {n}\r\n\r\n").as_bytes());
    let chunk = vec![b' '; 1 << 20];
    let mut left = n as usize;
    while left > 0 {
        let k = left.min(chunk.len());
        c.raw(&chunk[..k]);
        left -= k;
    }
    let e = c.recv();
    assert_eq!(
        e.get("error").unwrap().get("code").and_then(Value::as_i64),
        Some(-32600)
    );
    assert_eq!(e.get("id"), Some(&Value::Null));
    c.initialize(&[]);
    // 長すぎる見出しの行でも続ける
    let mut b = format!("X-Long: {}\r\n", "a".repeat(MAX_HEADER_LINE + 1)).into_bytes();
    b.extend_from_slice(&frame("{}"));
    c.raw(&b);
    assert!(c.recv().get("error").is_some());
    // JSON として読めない本文は -32700 で、続ける
    c.raw(&frame("{"));
    let e = c.recv();
    assert_eq!(
        e.get("error").unwrap().get("code").and_then(Value::as_i64),
        Some(-32700)
    );
    assert_eq!(c.finish().1, 0);
    // 枠が崩れたら終了コード 1 で終わる
    let mut c = Client::start(&[]);
    c.raw(b"Content-Length: abc\r\n\r\n");
    assert_eq!(c.exit_code(), 1);
}

impl Client {
    /// `hover` の結果。`null` は `None`。
    fn hover(&mut self, path: &Path, line: i64, ch: i64) -> Option<String> {
        let pos = obj(vec![("line", line.into()), ("character", ch.into())]);
        let doc = obj(vec![("uri", path_to_uri(path).into())]);
        self.request(
            7,
            "textDocument/hover",
            obj(vec![("textDocument", doc), ("position", pos)]),
        );
        let r = self.recv();
        assert_eq!(r.get("id").and_then(Value::as_i64), Some(7), "{r}");
        let v = r.get("result").unwrap();
        if *v == Value::Null {
            return None;
        }
        let c = v.get("contents").unwrap();
        assert_eq!(c.get("kind").and_then(Value::as_str), Some("plaintext"));
        Some(c.get("value").and_then(Value::as_str).unwrap().to_string())
    }
}

#[test]
fn lsp_hover_matches_spec() {
    let p = example("03-ownership/ownership.sigil");
    let src = std::fs::read_to_string(&p).unwrap();
    let spec = std::fs::read_to_string(example("03-ownership/ownership.spec")).unwrap();
    // `sigil spec` の出力から `deposit` の塊（空行で区切られる）を取り出す。
    let chunk = spec
        .split("\n\n")
        .find(|c| c.starts_with("関数 deposit（非公開）\n"))
        .map(|c| format!("{}\n", c.trim_end()))
        .unwrap();
    let line_of = |needle: &str| src.lines().position(|l| l.contains(needle)).unwrap() as i64;
    let mut c = Client::start(&[]);
    let caps = c.initialize(&[]);
    let caps = caps.get("result").unwrap().get("capabilities").unwrap();
    assert_eq!(caps.get("hoverProvider"), Some(&Value::Bool(true)));
    c.open(&p, &src);
    c.publish();
    let decl = line_of("fn deposit(");
    // 宣言: 名前の上、引数の行、4 節の行。
    for (l, ch) in [(decl, 4), (decl, 0), (decl + 1, 4), (decl + 3, 10)] {
        assert_eq!(
            c.hover(&p, l, ch).as_deref(),
            Some(chunk.as_str()),
            "{l}:{ch}"
        );
    }
    // 本体の `{` の行より後ろは返さない。
    assert_eq!(c.hover(&p, decl + 7, 4), None);
    // 呼び出しの箇所。
    let call = line_of("deposit(&mut a");
    assert_eq!(c.hover(&p, call, 3).as_deref(), Some(chunk.as_str()));
    // 型の宣言の名前。
    let st = line_of("struct Account");
    let got = c.hover(&p, st, 8).unwrap();
    assert!(
        spec.contains(&got) && got.starts_with("構造体 Account"),
        "{got}"
    );
    // 標準ライブラリの関数・呼び出しでない名前・空白は返さない。
    assert_eq!(c.hover(&p, line_of("console.log(&b)"), 11), None);
    assert_eq!(c.hover(&p, call, 12), None);
    assert_eq!(c.hover(&p, call, 0), None);
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_hover_scope_and_methods() {
    let dir = tmpdir("hover");
    let head = format!("{HEAD}struct S {{\n  v: Int,\n}}\n\n");
    let g = "fn g(n: Int) -> Int\n  intent \"g\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n\n";
    let imp = "impl S {\n  fn m(self: &S) -> Int\n    intent \"m\"\n    requires none\n    ensures none\n    effects none\n  {\n    return self.v;\n  }\n}\n\n";
    let user = "fn u(g: Int, s: S) -> Int\n  intent \"u\"\n  requires none\n  ensures none\n  effects none\n{\n  let a = g(1);\n  return a;\n}\n\n\
fn w(s: S) -> Int\n  intent \"w\"\n  requires none\n  ensures none\n  effects none\n{\n  let a = g(1);\n  let b = S.m(&s);\n  let c = s.m();\n  return a;\n}\n";
    let src = format!("{head}{g}{imp}{user}");
    let p = write(&dir, "m.sigil", &src);
    let line_of = |needle: &str| src.lines().position(|l| l.contains(needle)).unwrap() as i64;
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&p, &src);
    c.publish();
    let gl = line_of("fn g(");
    let gtext = c.hover(&p, gl, 3).unwrap();
    assert!(gtext.starts_with("関数 g（非公開）\n"), "{gtext}");
    // 引数 g が関数 g を隠している
    assert_eq!(c.hover(&p, line_of("let a = g(1)"), 10), None);
    // 隠していない呼び出しは返す。メソッドは `型.名前(...)` の形だけ
    let w = src.lines().position(|l| l.contains("fn w(")).unwrap() as i64;
    let ws: Vec<i64> = (w..w + 12).collect();
    let find = |s: &str| {
        *ws.iter()
            .find(|l| src.lines().nth(**l as usize).unwrap().contains(s))
            .unwrap()
    };
    assert_eq!(c.hover(&p, find("let a = g(1)"), 10), Some(gtext));
    let m = c.hover(&p, find("S.m(&s)"), 12).unwrap();
    assert!(m.starts_with("メソッド S.m"), "{m}");
    assert_eq!(c.hover(&p, find("s.m()"), 12), None);
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_hover_cross_file_and_utf16() {
    // 開いたのは入口だけ。呼び出し先 `with_tax` は `use` で取り込んだモジュールにある。
    let root = tmpdir("hoverx");
    let (p, _, src, other) = effects_staged(&root);
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&p, &src);
    c.publish();
    let l = src
        .lines()
        .position(|l| l.contains("with_tax(&xs)"))
        .unwrap() as i64;
    let got = c.hover(&p, l, 12).unwrap();
    assert!(got.starts_with("関数 with_tax（公開）\n"), "{got}");
    assert!(sigil::spec::spec(&other).unwrap().contains(&got));
    assert_eq!(c.finish().1, 0);

    // 日本語の文字列（UTF-16 で 1 単位、UTF-8 で 3 バイト）と絵文字（UTF-16 で 2 単位）の後ろの呼び出し。
    let dir = tmpdir("hover16");
    let g = "fn g(n: Int) -> Int\n  intent \"g\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n\n";
    let u = "fn u() -> Int\n  intent \"u\"\n  requires none\n  ensures none\n  effects none\n{\n  let s = \"日本語😀\"; let a = g(1);\n  return a;\n}\n";
    let src = format!("{HEAD}{g}{u}");
    let p = write(&dir, "m.sigil", &src);
    let l = src.lines().position(|l| l.contains("let s =")).unwrap() as i64;
    let line = src.lines().nth(l as usize).unwrap();
    let chars_before: Vec<char> = line.chars().take_while(|_| true).collect();
    let at = chars_before
        .iter()
        .collect::<String>()
        .find("g(1)")
        .unwrap();
    let c_idx = line[..at].chars().count() as i64;
    let u16_idx = line[..at].encode_utf16().count() as i64;
    assert_eq!(u16_idx, c_idx + 1);
    for (enc, ch) in [(&[][..], u16_idx), (&["utf-32"][..], c_idx)] {
        let mut c = Client::start(&[]);
        c.initialize(enc);
        c.open(&p, &src);
        c.publish();
        let got = c.hover(&p, l, ch).unwrap_or_default();
        assert!(got.starts_with("関数 g（非公開）\n"), "{enc:?} {got}");
        // 1 単位ずれると名前の外
        assert_eq!(c.hover(&p, l, ch - 3), None, "{enc:?}");
        assert_eq!(c.finish().1, 0);
    }
}

#[test]
fn lsp_hover_edges() {
    let dir = tmpdir("hoveredge");
    // 1 行の本体: `{` より右は宣言ではない（呼び出しの名前は返す）。
    let g = "fn g(n: Int) -> Int\n  intent \"g\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n\n";
    let one = "fn h(n: Int) -> Int\n  intent \"h\"\n  requires none\n  ensures none\n  effects none\n{ return n; }\n\n";
    let two = "fn k(n: Int) -> Int\n  intent \"k\"\n  requires none\n  ensures none\n  effects none\n{ return g(n); }\n";
    let src = format!("{HEAD}{g}{one}{two}");
    let p = write(&dir, "m.sigil", &src);
    let line_of = |needle: &str| src.lines().position(|l| l.contains(needle)).unwrap() as i64;
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&p, &src);
    c.publish();
    let hl = line_of("{ return n; }");
    assert_eq!(c.hover(&p, hl, 0), None);
    assert_eq!(c.hover(&p, hl, 3), None);
    assert_eq!(c.hover(&p, hl, 9), None);
    let kl = line_of("{ return g(n); }");
    assert_eq!(c.hover(&p, kl, 1), None);
    let got = c.hover(&p, kl, 9).unwrap();
    assert!(got.starts_with("関数 g（非公開）\n"), "{got}");
    // 範囲外の位置（行・列）
    assert_eq!(c.hover(&p, 10_000, 0), None);
    assert_eq!(c.hover(&p, kl, 10_000), None);
    assert_eq!(c.hover(&p, -0, 10_000), None);
    // 開いていない文書・空の文書
    let q = write(&dir, "e.sigil", "");
    c.open(&q, "");
    c.publish();
    assert_eq!(c.hover(&q, 0, 0), None);
    assert_eq!(c.hover(&q, 5, 5), None);
    assert_eq!(c.hover(&dir.join("nothing.sigil"), 0, 0), None);
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_hover_match_arm_and_closure_shadow() {
    let dir = tmpdir("hovershadow");
    let g = "fn g(n: Int) -> Int\n  intent \"g\"\n  requires none\n  ensures none\n  effects none\n{\n  return n;\n}\n\n";
    let u = "fn u(n: Int) -> Int\n  intent \"u\"\n  requires none\n  ensures none\n  effects none\n{\n  let a = g(1);\n  match n {\n    g => {\n      let z = g(2);\n    }\n  }\n  let b = g(3);\n  return match n {\n    g => g(4),\n  };\n}\n\n\
fn v(n: Int) -> Int\n  intent \"v\"\n  requires none\n  ensures none\n  effects none\n{\n  let k = |g| g(5);\n  let c = g(6);\n  return n;\n}\n";
    let src = format!("{HEAD}{g}{u}");
    let p = write(&dir, "m.sigil", &src);
    let line_of = |needle: &str| src.lines().position(|l| l.contains(needle)).unwrap() as i64;
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&p, &src);
    c.publish();
    let hit = |c: &mut Client, needle: &str| {
        let l = line_of(needle);
        let col = src.lines().nth(l as usize).unwrap().find("g(").unwrap() as i64;
        c.hover(&p, l, col)
    };
    assert!(hit(&mut c, "let a = g(1)").is_some());
    // 文の match の腕の束縛は、腕から match の終わりまで名前を隠す
    assert_eq!(hit(&mut c, "let z = g(2)"), None);
    // match の後ろでは隠さない
    assert!(hit(&mut c, "let b = g(3)").is_some());
    // 式の match の腕の束縛は、その腕で隠す
    assert_eq!(hit(&mut c, "g => g(4)"), None);
    // クロージャの引数は関数の全体で隠す（保守側）
    assert_eq!(hit(&mut c, "|g| g(5)"), None);
    assert_eq!(hit(&mut c, "let c = g(6)"), None);
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_hover_panic_contained() {
    let dir = tmpdir("hoverpanic");
    let src = format!("{HEAD}{FN_HEAD}  return n;\n}}\n");
    let p = write(&dir, "ok.sigil", &src);
    let mut c = Client::start(&[("SIGIL_LSP_TEST_PANIC", "return n;")]);
    c.initialize(&[]);
    // 注入は本文に印がある文書。診断は InternalError になるが、hover は null で、サーバーは生きている。
    c.open(&p, &src);
    c.publish();
    assert_eq!(c.hover(&p, 0, 0), None);
    assert_eq!(c.hover(&p, 4, 4), None);
    assert_eq!(c.finish().1, 0);
}

#[cfg(unix)]
#[test]
fn lsp_disk_read_rejects_fifo_device_and_huge_file() {
    // 開いていない文書の hover はディスクから読む。FIFO・`/dev/zero`・巨大ファイルで固まらず、応答は null。
    let dir = tmpdir("saferead");
    let fifo = dir.join("pipe.sigil");
    let made = std::process::Command::new("mkfifo").arg(&fifo).status();
    let zero = dir.join("z.sigil");
    std::os::unix::fs::symlink("/dev/zero", &zero).unwrap();
    let big = dir.join("big.sigil");
    std::fs::write(&big, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    if matches!(made, Ok(s) if s.success()) {
        assert_eq!(c.hover(&fifo, 0, 0), None);
    }
    assert_eq!(c.hover(&zero, 0, 0), None);
    assert_eq!(c.hover(&big, 0, 0), None);
    assert_eq!(c.finish().1, 0);
}

/// 例 04 の組を根の下に置き、入口を開いて、`with_tax` の呼び出しの位置を返す。
fn hover_setup(c: &mut Client, root: &Path) -> (PathBuf, PathBuf, String, i64) {
    let (p, pe, src, eff) = effects_staged(root);
    c.open(&p, &src);
    let l = src
        .lines()
        .position(|l| l.contains("with_tax(&xs)"))
        .unwrap() as i64;
    (p, pe, eff, l)
}

#[test]
fn lsp_hover_uses_check_time_body_not_disk() {
    // ADR 0022 決定 4: 検査のあとにディスクの依存を書き換えても、hover は検査した本文に基づく。
    let root = tmpdir("hover-fresh");
    let mut c = Client::start(&[]);
    c.initialize_root(&root);
    let (p, pe, eff, l) = hover_setup(&mut c, &root);
    c.publish();
    let want = sigil::spec::spec(&eff).unwrap();
    let before = c.hover(&p, l, 12).unwrap();
    assert!(want.contains(&before), "{before}");
    // 1. 同じ場所の中身を、署名の違うものに書き換える（通知なし）。
    std::fs::write(&pe, eff.replace("fn with_tax(", "fn with_tax_renamed(")).unwrap();
    assert_eq!(c.hover(&p, l, 12), Some(before.clone()));
    // 2. 根の外のファイルを指す symlink に差し替える。外の中身は読まない。
    let outside = tmpdir("hover-fresh-out");
    let out = write(
        &outside,
        "evil.sigil",
        &eff.replace("fn with_tax(", "fn with_tax_evil("),
    );
    std::fs::remove_file(&pe).unwrap();
    std::os::unix::fs::symlink(&out, &pe).unwrap();
    assert_eq!(c.hover(&p, l, 12), Some(before));
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_dependency_body_cap_drops_hover_and_moves_diagnostic_to_use_line() {
    // 保持量の上限を超えた依存は、本文を持たない（hover を出さず、ディスクも読み直さない）。診断は `use` の行に出す。
    let root = tmpdir("depcap");
    let (pn, pe, main, eff) = effects_staged(&root);
    std::fs::write(
        &pe,
        eff.replacen("return ", "let z: Int = true;\n  return ", 1),
    )
    .unwrap();
    let mut c = Client::start(&[(sigil::lsp::DEP_CAP_ENV, "1")]);
    c.initialize_root(&root);
    c.open(&pn, &main);
    let got = c.publishes(1);
    assert_eq!(got[0].0, path_to_uri(&pn));
    let d = &got[0].1;
    assert_eq!((d.len(), code(&d[0])), (1, "TypeMismatch"), "{d:?}");
    let line = main
        .lines()
        .position(|l| l.starts_with("use examples.effect_check"))
        .unwrap() as i64;
    let len = main.lines().nth(line as usize).unwrap().chars().count() as i64;
    assert_eq!(pos(&d[0]), (line, 0, line, len));
    // 依存の本文を持たないので hover は出さない。
    let l = main
        .lines()
        .position(|l| l.contains("with_tax(&xs)"))
        .unwrap() as i64;
    assert_eq!(c.hover(&pn, l, 12), None);
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_dependency_body_is_shared_between_entries() {
    // 同じパス・同じ中身の依存の本文は入口の間で 1 つだけ持つ（上限は 1 つ分）。2 つ目の入口も hover が出る。
    let root = tmpdir("depshare");
    let (p1, _, main, eff) = effects_staged(&root);
    let main2 = main.replace("effect_main", "effect_main2");
    let p2 = stage(&root, &main2);
    let cap = (eff.len() + 10).to_string();
    let mut c = Client::start(&[(sigil::lsp::DEP_CAP_ENV, &cap)]);
    c.initialize_root(&root);
    c.open(&p1, &main);
    c.publish();
    c.open(&p2, &main2);
    c.publish();
    let l = main
        .lines()
        .position(|l| l.contains("with_tax(&xs)"))
        .unwrap() as i64;
    let a = c.hover(&p1, l, 12).unwrap();
    assert!(a.starts_with("関数 with_tax"), "{a}");
    assert_eq!(c.hover(&p2, l, 12), Some(a));
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_dependency_cap_boundary_exact_and_one_below() {
    // 上限がちょうど本文の大きさなら持ち、1 バイト足りなければ持たない。
    let root = tmpdir("capedge");
    let (pn, _, main, eff) = effects_staged(&root);
    let l = main
        .lines()
        .position(|l| l.contains("with_tax(&xs)"))
        .unwrap() as i64;
    for (cap, kept) in [(eff.len(), true), (eff.len() - 1, false)] {
        let cap = cap.to_string();
        let mut c = Client::start(&[(sigil::lsp::DEP_CAP_ENV, &cap)]);
        c.initialize_root(&root);
        c.open(&pn, &main);
        c.publish();
        assert_eq!(c.hover(&pn, l, 12).is_some(), kept, "cap={cap}");
        assert_eq!(c.finish().1, 0);
    }
}

#[test]
fn lsp_dropped_chain_reports_at_entry_head_with_note() {
    // 入口 -> a -> b。a も b も本文を持たない（上限 1）。b の誤りは、取り込む `use` の行が見つからないので
    // 入口の先頭に注記つきで出す。
    let root = tmpdir("chain");
    let h = "  targets server\n  trust none\n\n";
    let f = |n: &str, body: &str| {
        format!("fn {n}() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  {body}\n}}\n")
    };
    let b = format!(
        "module chain.b\n{h}pub {}",
        f("fb", "let z: Int = true;\n  return 1;")
    );
    let a = format!(
        "module chain.a\n{h}use chain.b.fb;\n\npub {}",
        f("fa", "return fb();")
    );
    let m = format!(
        "module chain.m\n{h}use chain.a.fa;\n\n{}",
        f("main2", "return fa();")
    );
    stage(&root, &b);
    stage(&root, &a);
    let pm = stage(&root, &m);
    let mut c = Client::start(&[(sigil::lsp::DEP_CAP_ENV, "1")]);
    c.initialize_root(&root);
    c.open(&pm, &m);
    let (u, d) = c.publish();
    assert_eq!(u, path_to_uri(&pm));
    assert_eq!((d.len(), code(&d[0])), (1, "TypeMismatch"), "{d:?}");
    assert_eq!(pos(&d[0]), (0, 0, 0, 0));
    let msg = d[0].get("message").and_then(Value::as_str).unwrap();
    assert!(msg.contains("use") && msg.contains("b.sigil"), "{msg}");
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_dropped_dependency_recovers_when_held_bytes_shrink() {
    // 上限は 1 つ分。入口 1 が依存 x を持つ間、入口 2 の依存 y は持てない。入口 1 を閉じて入口 2 を検査し直すと持てる。
    let root = tmpdir("recover");
    let h = "  targets server\n  trust none\n\n";
    let f = "pub fn g() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n";
    let x = format!("module rec.x\n{h}{f}");
    let y = format!("module rec.y\n{h}{f}\n");
    let ma = format!("module rec.ma\n{h}use rec.x.g;\n\nfn t() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return g();\n}}\n");
    let mb = ma.replace("rec.ma", "rec.mb").replace("rec.x", "rec.y");
    stage(&root, &x);
    stage(&root, &y);
    let (pa, pb) = (stage(&root, &ma), stage(&root, &mb));
    let l = ma.lines().position(|l| l.contains("return g()")).unwrap() as i64;
    let cap = (y.len().max(x.len())).to_string();
    let mut c = Client::start(&[(sigil::lsp::DEP_CAP_ENV, &cap)]);
    c.initialize_root(&root);
    c.open(&pa, &ma);
    c.publish();
    c.open(&pb, &mb);
    c.publish();
    assert!(c.hover(&pa, l, 9).is_some());
    assert_eq!(c.hover(&pb, l, 9), None);
    c.close(&pa);
    c.publish();
    c.change(&pb, &mb);
    c.publish();
    assert!(c.hover(&pb, l, 9).is_some());
    assert_eq!(c.finish().1, 0);
}

#[test]
fn lsp_dependency_diagnostic_range_from_check_time_body() {
    // 開いていない依存の誤りの範囲は、検査した本文の行（ディスクを書き換えても変わらない）。
    let root = tmpdir("range");
    let (pn, pe, main, eff) = effects_staged(&root);
    let bad = eff.replacen("return ", "let z: Int = true;\n  return ", 1);
    std::fs::write(&pe, &bad).unwrap();
    let want = bad
        .lines()
        .position(|l| l.contains("let z: Int = true"))
        .unwrap() as i64;
    let mut c = Client::start(&[]);
    c.initialize_root(&root);
    c.open(&pn, &main);
    let got = c.publishes(2);
    let d = by_uri(&got, &pe);
    assert_eq!(pos(&d[0]).0, want, "{d:?}");
    std::fs::write(&pe, format!("\n\n\n{bad}")).unwrap();
    assert!(c.hover(&pn, 0, 0).is_none());
    assert_eq!(c.finish().1, 0);
}

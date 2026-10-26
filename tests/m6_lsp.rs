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

const HEAD: &str = "module m.a\n  targets server\n  trust none\n\n";
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
    let files = [
        "01-average/c-brace.sigil",
        "02-read-port/c-brace.sigil",
        "03-ownership/ownership.sigil",
        "04-effects/effects.sigil",
        "05-control/control.sigil",
        "04-effects/entry.sigil",
    ];
    let mut c = Client::start(&[]);
    let r = c.initialize(&[]);
    let caps = r.get("result").unwrap().get("capabilities").unwrap();
    assert_eq!(
        caps.get("positionEncoding").and_then(Value::as_str),
        Some("utf-16")
    );
    for f in files {
        let p = example(f);
        c.open(&p, &std::fs::read_to_string(&p).unwrap());
    }
    let (got, code) = c.finish();
    assert_eq!(code, 0);
    for (uri, d) in &got {
        assert!(d.is_empty(), "{uri}: {d:?}");
    }
    for f in files {
        assert!(
            got.iter().any(|(u, _)| *u == path_to_uri(&example(f))),
            "{f} に診断が送られない"
        );
    }
    // 04-effects は 2 つのファイルを合わせて検査する（開いていない方にも送る）。
    assert!(got
        .iter()
        .any(|(u, _)| u.ends_with("/04-effects/entry.sigil")));
}

fn move_src(second_use: &str) -> String {
    format!("{HEAD}struct S\n{{\n  a: Int,\n}}\n\nfn take(s: S)\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  return;\n}}\n\nfn f(s: S) -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{{\n  take(s);\n{second_use}  return 1;\n}}\n")
}

#[test]
fn lsp_error_diagnostic() {
    let dir = tmpdir("error");
    let p = dir.join("m.sigil");
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
    let p = dir.join("m.sigil");
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
    let ok_text = format!("{HEAD}{FN_HEAD}  return n;\n}}\n");
    let bad = write(&dir, "bad.sigil", &format!("{HEAD}// PANIC_HERE\n"));
    let ok = write(&dir, "ok.sigil", &ok_text);
    let mut c = Client::start(&[("SIGIL_LSP_TEST_PANIC", "PANIC_HERE")]);
    c.initialize(&[]);
    c.open(&bad, &format!("{HEAD}// PANIC_HERE\n"));
    let (uri, d) = c.publish();
    assert_eq!(uri, path_to_uri(&bad));
    assert_eq!((d.len(), code(&d[0])), (1, "InternalError"));
    assert_eq!(pos(&d[0]), (0, 0, 0, 0));
    assert!(d[0]
        .get("message")
        .and_then(Value::as_str)
        .unwrap()
        .contains("注入された panic"));
    // 続けて次の要求に応える（状態は検査の前のまま）
    c.open(&ok, &format!("{HEAD}{FN_HEAD}  return n;\n}}\n"));
    // 同じディレクトリ・同じモジュールなので両方に配信される（注入は開いた文書の本文だけ）
    let ps = c.publishes(2);
    assert!(ps.iter().all(|(_, d)| d.is_empty()));
    c.change(&bad, &format!("{HEAD}// 直した\n"));
    assert!(c.publishes(2).iter().all(|(_, d)| d.is_empty()));
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
fn lsp_multi_file_publish() {
    let dir = tmpdir("multi");
    let eff = std::fs::read_to_string(example("04-effects/effects.sigil")).unwrap();
    let entry = std::fs::read_to_string(example("04-effects/entry.sigil")).unwrap();
    let (pe, pn) = (
        write(&dir, "effects.sigil", &eff),
        write(&dir, "entry.sigil", &entry),
    );
    let mut c = Client::start(&[]);
    c.initialize(&[]);
    c.open(&pe, &eff);
    let got = c.publishes(2);
    assert!(got.iter().all(|(_, d)| d.is_empty()));
    // 開いた方の関数の名前を変えると、開いていない方（entry）の呼び出しが解決できなくなる
    c.change(&pe, &eff.replace("fn with_tax", "fn with_tax2"));
    let got = c.publishes(2);
    assert_eq!(got[0].0, path_to_uri(&pe));
    assert!(got[0].1.is_empty());
    assert_eq!(got[1].0, path_to_uri(&pn));
    assert_eq!(got[1].1.len(), 1, "{:?}", got[1].1);
    // 戻すと、空の配列で消える
    c.change(&pe, &eff);
    assert!(c.publishes(2).iter().all(|(_, d)| d.is_empty()));
    // モジュールの宣言を変えて組から外すと、外れた方に空の配列を送る
    c.change(
        &pe,
        &eff.replace("module examples.effect_check", "module examples.other"),
    );
    let got = c.publishes(2);
    assert_eq!(got.iter().filter(|(_, d)| !d.is_empty()).count(), 0);
    assert!(got.iter().any(|(u, _)| *u == path_to_uri(&pn)));
    // 閉じると、ディスクの中身で検査し直す
    c.close(&pe);
    c.publishes(1);
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

#[test]
fn lsp_group_split_rechecks_remaining() {
    // a と c は同じ関数 f を持ち（重複）、b は別の関数。3 つとも m.a。
    let dir = tmpdir("split");
    let f = |name: &str| {
        format!(
            "{HEAD}{}  return n;\n}}\n",
            FN_HEAD.replace("fn f(", &format!("fn {name}("))
        )
    };
    let (a, b, c) = (
        write(&dir, "a.sigil", &f("f")),
        write(&dir, "b.sigil", &f("g")),
        write(&dir, "c.sigil", &f("f")),
    );
    let mut cl = Client::start(&[]);
    cl.initialize(&[]);
    cl.open(&b, &f("g"));
    let first = cl.publishes(3);
    assert_eq!(
        first.iter().map(|p| p.1.len()).sum::<usize>(),
        1,
        "重複が 1 件"
    );
    // b が別のモジュールへ移る。a と c の組は、b を抜けた後も検査し直されて重複が残る。
    cl.change(&b, &f("g").replace("module m.a", "module m.b"));
    let (got, code) = cl.finish();
    assert_eq!(code, 0);
    let last = |p: &Path| {
        got.iter()
            .rev()
            .find(|g| g.0 == path_to_uri(p))
            .map(|g| g.1.len())
    };
    assert_eq!(last(&b), Some(0));
    assert_eq!([last(&a), last(&c)].iter().flatten().sum::<usize>(), 1);
    // 閉じる（ディスクの中身は m.a）と b は元の組へ戻り、3 つのファイルのどれかに重複が 1 件残る。
    let mut cl = Client::start(&[]);
    cl.initialize(&[]);
    cl.open(&b, &f("g"));
    cl.publishes(3);
    cl.change(&b, &f("g").replace("module m.a", "module m.b"));
    cl.close(&b);
    let (got, _) = cl.finish();
    let n: usize = [&a, &b, &c]
        .iter()
        .map(|p| {
            got.iter()
                .rev()
                .find(|g| g.0 == path_to_uri(p))
                .unwrap()
                .1
                .len()
        })
        .sum();
    assert_eq!(n, 1);
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
        .find(|c| c.starts_with("関数 deposit\n"))
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
    assert!(gtext.starts_with("関数 g\n"), "{gtext}");
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

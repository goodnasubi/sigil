//! 言語サーバー（ADR 0019 の決定 3・4・6。ADR 0020 の上限）。標準入出力で LSP（JSON-RPC）を話す。
//!
//! 依存は足さず、枠（`Content-Length`）・JSON-RPC・診断を自前で持つ。送る通知は `publishDiagnostics` だけ。

mod hover;

use crate::json::{obj, parse, Value};
use crate::lexer::{lex, Tok};
use crate::{Error, Phase};
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

/// `Content-Length` の上限（ADR 0020）。超えた本文は読まずに捨てる。
pub const MAX_BODY: u64 = 64 * 1024 * 1024;
/// 見出しの 1 行の長さの上限（ADR 0020）。
pub const MAX_HEADER_LINE: usize = 1024;
/// 検査に panic を起こさせるテスト用の差し込み口（この環境変数の値を含む文書で panic する）。
pub const INJECT_PANIC_ENV: &str = "SIGIL_LSP_TEST_PANIC";

/// 枠を 1 つ読んだ結果。
#[derive(Debug, PartialEq)]
pub enum Frame {
    Body(Vec<u8>),
    /// 本文を捨てて続けられる（理由つき）。
    Skipped(String),
    /// 入力の終わり（枠の境目で）。
    Eof,
    /// 枠が崩れて続けられない。
    Broken(String),
}

/// 1 行を読む。(行の先頭 `MAX_HEADER_LINE` バイトまで, 長すぎたか, 改行まで読めたか)。
fn read_line(r: &mut impl BufRead) -> std::io::Result<(Vec<u8>, bool, bool)> {
    let (mut line, mut over) = (vec![], false);
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok((line, over, false));
        }
        let found = buf.iter().position(|&b| b == b'\n');
        let n = found.map_or(buf.len(), |i| i + 1);
        let chunk = &buf[..found.unwrap_or(buf.len())];
        if line.len() + chunk.len() > MAX_HEADER_LINE {
            over = true;
        } else {
            line.extend_from_slice(chunk);
        }
        r.consume(n);
        if found.is_some() {
            return Ok((line, over, true));
        }
    }
}

/// 見出しと本文を 1 組読む。大きすぎる本文は読まずに捨てる（メモリを確保しない）。
pub fn read_frame(r: &mut impl BufRead, max_body: u64) -> Frame {
    let (mut len, mut long_line, mut first) = (None::<u64>, false, true);
    loop {
        let (mut line, over, complete) = match read_line(r) {
            Ok(x) => x,
            Err(e) => return Frame::Broken(format!("入力を読めない: {e}")),
        };
        if !complete {
            let empty = line.is_empty() && !over;
            return if first && empty {
                Frame::Eof
            } else {
                Frame::Broken("見出しの途中で入力が終わった".into())
            };
        }
        first = false;
        if over {
            long_line = true;
            continue;
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            break;
        }
        let text = String::from_utf8_lossy(&line);
        let Some((name, value)) = text.split_once(':') else {
            return Frame::Broken("見出しの行に `:` がない".into());
        };
        if name.eq_ignore_ascii_case("content-length") {
            let v = value.trim();
            let n = match v.parse::<u64>() {
                Ok(n) if !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()) => n,
                _ => return Frame::Broken("Content-Length が数でない".into()),
            };
            if len.is_some_and(|old| old != n) {
                return Frame::Broken("Content-Length が食い違って重複している".into());
            }
            len = Some(n);
        }
    }
    let Some(n) = len else {
        return Frame::Broken(if long_line {
            "Content-Length を読めない（見出しの行が長すぎる）".into()
        } else {
            "Content-Length がない".into()
        });
    };
    if n > max_body || long_line {
        let skipped = std::io::copy(&mut Read::take(r.by_ref(), n), &mut std::io::sink());
        if skipped.ok() != Some(n) {
            return Frame::Broken("捨てる本文の途中で入力が終わった".into());
        }
        return Frame::Skipped(if long_line {
            format!("見出しの行が {MAX_HEADER_LINE} バイトを超えている")
        } else {
            format!("Content-Length {n} が上限 {max_body} を超えている")
        });
    }
    let mut body = vec![];
    match Read::take(r.by_ref(), n).read_to_end(&mut body) {
        Ok(got) if got as u64 == n => Frame::Body(body),
        _ => Frame::Broken("本文の途中で入力が終わった".into()),
    }
}

/// `file:` の URI をパスにする（`%XX` を戻す）。`file:` でなければ `None`。
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let (b, mut out, mut i) = (rest.as_bytes(), vec![], 0);
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let h = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(out).ok()?))
}

pub fn path_to_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for &b in p.to_string_lossy().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

/// 行頭から始まる字句の長さ（文字数）。字句が取れなければ `None`。
fn token_len(rest: &[char]) -> Option<usize> {
    let lexed = lex(&rest.iter().collect::<String>()).ok()?;
    let first = lexed.tokens.first()?;
    if first.tok == Tok::Eof || first.span.col != 1 {
        return None;
    }
    let mut end = lexed
        .tokens
        .get(1)
        .map_or(rest.len() + 1, |t| t.span.col as usize);
    if let Some(c) = lexed.comments.first() {
        end = end.min(c.span.col as usize);
    }
    let mut n = (end - 1).min(rest.len());
    while n > 1 && rest[n - 1].is_whitespace() {
        n -= 1;
    }
    Some(n)
}

fn utf16(cs: &[char]) -> usize {
    cs.iter().map(|c| c.len_utf16()).sum()
}

pub struct Server<W: Write> {
    out: W,
    opts: crate::effects::Options,
    utf32: bool,
    initialized: bool,
    shutdown: bool,
    /// 開いている文書（URI -> 中身）。
    docs: HashMap<String, String>,
    /// ワークスペースの根（`initialize` の `rootUri`・`workspaceFolders`）。モジュールの名前とパスの対応の根。
    root: Option<PathBuf>,
    /// 開いている文書ごとの、最後の検査の結果（その文書を入口にして `use` を辿ったもの）。
    results: HashMap<String, Checked>,
    inject: Option<String>,
}

type Diags = Vec<(String, Vec<Value>)>;

/// 1 つの入口の検査の結果。
struct Checked {
    /// 読んだファイル（入口を含む。`use` で辿れたもの）。
    deps: Vec<PathBuf>,
    /// 診断（URI -> 診断）。入口の URI は、診断がなくても入れる。
    diags: Diags,
}

impl<W: Write> Server<W> {
    pub fn new(out: W, trust_base: Vec<String>) -> Self {
        Server {
            out,
            opts: crate::effects::Options {
                trust_base,
                ..Default::default()
            },
            utf32: false,
            initialized: false,
            shutdown: false,
            docs: HashMap::new(),
            root: None,
            results: HashMap::new(),
            inject: std::env::var(INJECT_PANIC_ENV)
                .ok()
                .filter(|s| !s.is_empty()),
        }
    }

    fn send(&mut self, v: Value) {
        let body = v.to_json();
        let _ = write!(self.out, "Content-Length: {}\r\n\r\n{body}", body.len());
        let _ = self.out.flush();
    }

    fn respond(&mut self, id: Value, result: Value) {
        self.send(obj(vec![
            ("jsonrpc", "2.0".into()),
            ("id", id),
            ("result", result),
        ]));
    }

    fn respond_err(&mut self, id: Value, code: i64, message: &str) {
        let error = obj(vec![("code", code.into()), ("message", message.into())]);
        self.send(obj(vec![
            ("jsonrpc", "2.0".into()),
            ("id", id),
            ("error", error),
        ]));
    }

    fn publish(&mut self, uri: &str, diagnostics: Vec<Value>) {
        let params = obj(vec![
            ("uri", uri.into()),
            ("diagnostics", Value::Arr(diagnostics)),
        ]);
        self.send(obj(vec![
            ("jsonrpc", "2.0".into()),
            ("method", "textDocument/publishDiagnostics".into()),
            ("params", params),
        ]));
    }

    /// 1 つの枠を処理する。終了するなら終了コードを返す。
    pub fn on_frame(&mut self, frame: Frame) -> Option<i32> {
        match frame {
            Frame::Body(b) => self.on_message(&b),
            Frame::Skipped(why) => {
                self.respond_err(Value::Null, -32600, &why);
                None
            }
            Frame::Eof => Some(if self.shutdown { 0 } else { 1 }),
            Frame::Broken(why) => {
                eprintln!("sigil lsp: 枠が崩れたので終わる: {why}");
                Some(1)
            }
        }
    }

    fn on_message(&mut self, body: &[u8]) -> Option<i32> {
        let Some(msg) = std::str::from_utf8(body).ok().and_then(|s| parse(s).ok()) else {
            self.respond_err(Value::Null, -32700, "JSON として読めない");
            return None;
        };
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            if msg.get("result").is_none() && msg.get("error").is_none() {
                self.respond_err(id.unwrap_or(Value::Null), -32600, "method がない");
            }
            return None;
        };
        let null = Value::Null;
        let params = msg.get("params").unwrap_or(&null);
        if method == "exit" {
            return Some(if self.shutdown { 0 } else { 1 });
        }
        if method != "initialize" && !self.initialized || self.shutdown {
            if let Some(id) = id {
                let (code, why) = if self.shutdown {
                    (-32600, "shutdown の後は exit だけを受ける")
                } else {
                    (-32002, "initialize の前")
                };
                self.respond_err(id, code, why);
            }
            return None;
        }
        match (method, id) {
            ("initialize", Some(id)) => {
                let encs = params
                    .get("capabilities")
                    .and_then(|c| c.get("general"))
                    .and_then(|g| g.get("positionEncodings"))
                    .and_then(Value::as_array)
                    .unwrap_or(&[]);
                self.root = params
                    .get("rootUri")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        params
                            .get("workspaceFolders")
                            .and_then(Value::as_array)
                            .and_then(|w| w.first())
                            .and_then(|f| f.get("uri"))
                            .and_then(Value::as_str)
                    })
                    .and_then(uri_to_path);
                self.utf32 = encs.iter().any(|e| e.as_str() == Some("utf-32"));
                self.initialized = true;
                let enc = if self.utf32 { "utf-32" } else { "utf-16" };
                let sync = obj(vec![("openClose", true.into()), ("change", 1i64.into())]);
                let caps = obj(vec![
                    ("positionEncoding", enc.into()),
                    ("textDocumentSync", sync),
                    ("hoverProvider", true.into()),
                ]);
                let info = obj(vec![
                    ("name", "sigil".into()),
                    ("version", env!("CARGO_PKG_VERSION").into()),
                ]);
                self.respond(id, obj(vec![("capabilities", caps), ("serverInfo", info)]));
            }
            ("shutdown", Some(id)) => {
                self.shutdown = true;
                self.respond(id, Value::Null);
            }
            ("textDocument/didOpen", None) => {
                let doc = params.get("textDocument");
                let uri = doc.and_then(|d| d.get("uri")).and_then(Value::as_str);
                let text = doc.and_then(|d| d.get("text")).and_then(Value::as_str);
                if let (Some(uri), Some(text)) = (uri, text) {
                    self.docs.insert(uri.into(), text.into());
                    self.recheck(uri);
                }
            }
            ("textDocument/didChange", None) => {
                let uri = params.get("textDocument").and_then(|d| d.get("uri"));
                let text = params
                    .get("contentChanges")
                    .and_then(Value::as_array)
                    .and_then(|c| c.last())
                    .and_then(|c| c.get("text"))
                    .and_then(Value::as_str);
                if let (Some(uri), Some(text)) = (uri.and_then(Value::as_str), text) {
                    if self.docs.contains_key(uri) {
                        self.docs.insert(uri.into(), text.into());
                        self.recheck(uri);
                    }
                }
            }
            ("textDocument/didClose", None) => {
                if let Some(uri) = params
                    .get("textDocument")
                    .and_then(|d| d.get("uri"))
                    .and_then(Value::as_str)
                {
                    self.close(uri);
                }
            }
            ("textDocument/hover", Some(id)) => {
                // 内部の不具合（panic）は null にして、サーバーを落とさない。
                let r = std::panic::catch_unwind(AssertUnwindSafe(|| self.hover(params)))
                    .ok()
                    .flatten()
                    .unwrap_or(Value::Null);
                self.respond(id, r);
            }
            (_, Some(id)) => self.respond_err(id, -32601, "未対応の要求"),
            _ => {}
        }
        None
    }

    /// hover の結果（`MarkupContent` の `plaintext`）。返さないときは `None`（応答は `null`）。
    fn hover(&self, params: &Value) -> Option<Value> {
        let uri = params.get("textDocument")?.get("uri")?.as_str()?;
        let pos = params.get("position")?;
        let line = pos.get("line")?.as_i64()? as usize;
        let ch = pos.get("character")?.as_i64()? as usize;
        let text = self.text_of(uri)?;
        if let Some(m) = &self.inject {
            if text.contains(m.as_str()) {
                panic!("注入された panic（{m}）");
            }
        }
        let cs: Vec<char> = text.split('\n').nth(line)?.chars().collect();
        // 位置の単位（UTF-16 の符号単位か文字数）から、文字の添字へ。
        let (mut col, mut units) = (0, 0);
        while col < cs.len() && units < ch {
            units += if self.utf32 { 1 } else { cs[col].len_utf16() };
            col += 1;
        }
        let own = crate::spec::extract_with(&text, &self.opts).ok()?;
        // 呼び出し先は、この文書から `use` で辿れるモジュール（最後の検査で読んだファイル）にある。
        let mut all = vec![own.clone()];
        let me = uri_to_path(uri);
        for d in self.results.get(uri).map_or(&[][..], |r| &r.deps[..]) {
            if Some(d) != me.as_ref() {
                let t = self.text_of(&self.uri_for(d));
                all.extend(t.and_then(|t| crate::spec::extract_with(&t, &self.opts).ok()));
            }
        }
        let value = hover::hover(&text, line as u32 + 1, col as u32 + 1, &own, &all)?;
        let md = obj(vec![("kind", "plaintext".into()), ("value", value.into())]);
        Some(obj(vec![("contents", md)]))
    }

    /// 文書の中身。開いていなければディスクの中身。
    fn text_of(&self, uri: &str) -> Option<String> {
        self.docs
            .get(uri)
            .cloned()
            .or_else(|| uri_to_path(uri).and_then(|p| std::fs::read_to_string(p).ok()))
    }

    /// 閉じた文書は入口でなくなる。その文書に出していた診断を消し、その文書を読んでいた入口は検査し直す。
    fn close(&mut self, uri: &str) {
        if self.docs.remove(uri).is_none() {
            return;
        }
        let old = self.results.remove(uri);
        let mut affected: Vec<String> = old
            .iter()
            .flat_map(|r| r.diags.iter().map(|d| d.0.clone()))
            .collect();
        affected.push(uri.to_string());
        self.refresh(uri, &mut affected);
    }

    /// 検査し直して配信する。変わった文書と、それを `use` で読んでいる開いた文書を入口として検査し直す。
    fn recheck(&mut self, uri: &str) {
        let mut affected = vec![uri.to_string()];
        self.refresh(uri, &mut affected);
    }

    /// `changed` が変わった（開いていれば入口にもする。閉じたときは入口にしない）。依存する入口と合わせて検査し直し、
    /// 診断が変わりうる URI（`affected` に足す）へ、すべての入口の診断を合わせて送る。
    fn refresh(&mut self, changed: &str, affected: &mut Vec<String>) {
        let changed_path = uri_to_path(changed);
        let mut entries: Vec<String> = vec![];
        if self.docs.contains_key(changed) {
            entries.push(changed.to_string());
        }
        let mut deps: Vec<String> = self
            .results
            .iter()
            .filter(|(u, r)| {
                u.as_str() != changed && changed_path.as_ref().is_some_and(|p| r.deps.contains(p))
            })
            .map(|(u, _)| u.clone())
            .collect();
        deps.sort();
        entries.extend(deps);
        for e in entries {
            let Some(t) = self.docs.get(&e).cloned() else {
                continue;
            };
            let r = self.guarded(&e, &t);
            affected.extend(r.diags.iter().map(|d| d.0.clone()));
            if let Some(old) = self.results.insert(e, r) {
                affected.extend(old.diags.into_iter().map(|d| d.0));
            }
        }
        let mut seen: Vec<String> = vec![];
        for u in affected.drain(..) {
            if seen.contains(&u) {
                continue;
            }
            // すべての入口が出した、この文書の診断（同じものは 1 つ）。
            let mut ds: Vec<Value> = vec![];
            let mut keys: Vec<&String> = self.results.keys().collect();
            keys.sort();
            for k in keys {
                for (x, d) in &self.results[k].diags {
                    if *x == u {
                        for v in d {
                            if !ds.contains(v) {
                                ds.push(v.clone());
                            }
                        }
                    }
                }
            }
            self.publish(&u, ds);
            seen.push(u);
        }
    }

    fn guarded(&self, uri: &str, text: &str) -> Checked {
        match std::panic::catch_unwind(AssertUnwindSafe(|| self.compute(uri, text))) {
            Ok(x) => x,
            Err(p) => {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "原因不明".into());
                let d = self.diag_at(
                    0,
                    0,
                    0,
                    0,
                    "InternalError",
                    &format!("処理系の不具合（panic）: {msg}"),
                );
                Checked {
                    deps: uri_to_path(uri).into_iter().collect(),
                    diags: vec![(uri.to_string(), vec![d])],
                }
            }
        }
    }

    /// `uri` の文書を入口にして、`use` で辿れるモジュールを検査する（`project`。ファイルの読み込みと
    /// 根の外・パスと名前の対応の確認は `modpath::load` と同じ）。状態は書き換えない。
    fn compute(&self, uri: &str, text: &str) -> Checked {
        if let Some(m) = &self.inject {
            if text.contains(m.as_str()) {
                panic!("注入された panic（{m}）");
            }
        }
        let own = (uri.to_string(), vec![]);
        let Some(path) = uri_to_path(uri) else {
            // ファイルでない文書: 単独で検査する（`use` は辿れない）。
            let err =
                crate::parse(text).and_then(|f| crate::check::check_file_with(&f, &self.opts));
            let d = err.err().map(|e| self.diag(&e, text)).into_iter().collect();
            return Checked {
                deps: vec![],
                diags: vec![(uri.to_string(), d)],
            };
        };
        let root = self.root_for(&path, text);
        let open: HashMap<PathBuf, &String> = self
            .docs
            .iter()
            .filter_map(|(u, t)| uri_to_path(u).map(|p| (p, t)))
            .collect();
        let mut deps: Vec<PathBuf> = vec![];
        let mut overlay = |p: &Path| {
            if !deps.iter().any(|d| d == p) {
                deps.push(p.to_path_buf());
            }
            open.get(p).map(|t| t.to_string())
        };
        let r = crate::project::check_project_overlay(&root, &path, &self.opts, &mut overlay);
        let mut diags: Diags = vec![own];
        match r {
            Ok(()) => {}
            Err(crate::project::ProjectError::Diag { file, error }) => {
                let u = self.uri_for(&file);
                let t = if file == path {
                    Some(text.to_string())
                } else {
                    self.text_of(&u)
                };
                let d = match t {
                    Some(t) => self.diag(&error, &t),
                    None => self.diag_at(0, 0, 0, 0, &format!("{:?}", error.kind), &error.message),
                };
                match diags.iter_mut().find(|x| x.0 == u) {
                    Some(x) => x.1.push(d),
                    None => diags.push((u, vec![d])),
                }
            }
            Err(crate::project::ProjectError::Read { file, msg }) => {
                let d = self.diag_at(
                    0,
                    0,
                    0,
                    0,
                    "Read",
                    &format!("`{}` を読めない: {msg}", file.display()),
                );
                diags[0].1.push(d);
            }
        }
        Checked { deps, diags }
    }

    /// モジュールの名前とパスの対応の根。ワークスペースの根の下にあればそれ、なければ
    /// 文書の `module` の宣言の名前の分だけパスの末尾をはがしたもの（合わなければ置かれたディレクトリ。
    /// 対応の誤りとして `modpath` が報告する）。
    fn root_for(&self, path: &Path, text: &str) -> PathBuf {
        if let Some(r) = self.root.as_ref().filter(|r| path.starts_with(r)) {
            return r.clone();
        }
        let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let Ok(f) = crate::parse(text) else {
            return parent;
        };
        let mut want = std::path::PathBuf::new();
        for seg in &f.module.name {
            want.push(seg);
        }
        want.set_extension("sigil");
        let mut root = path.to_path_buf();
        for _ in want.components() {
            root.pop();
        }
        if path.ends_with(&want) {
            root
        } else {
            parent
        }
    }

    fn uri_for(&self, p: &Path) -> String {
        let open = self
            .docs
            .keys()
            .find(|u| uri_to_path(u).as_deref() == Some(p));
        open.cloned().unwrap_or_else(|| path_to_uri(p))
    }

    /// 決定 6 の範囲を持つ診断。
    fn diag(&self, e: &Error, text: &str) -> Value {
        let lines: Vec<Vec<char>> = text.split('\n').map(|l| l.chars().collect()).collect();
        let li = (e.span.line as usize)
            .saturating_sub(1)
            .min(lines.len() - 1);
        let line = &lines[li];
        let c0 = (e.span.col as usize).saturating_sub(1).min(line.len());
        let on_char = c0 < line.len();
        let at_eof = !on_char && li == lines.len() - 1;
        let n = if at_eof {
            0
        } else if e.kind.phase() == Phase::Lex || !on_char {
            1
        } else {
            token_len(&line[c0..]).unwrap_or(1)
        };
        // 行末の改行（1 文字）に当たるときは、行の長さを超える分をそのまま足す。
        let conv = |k: usize| {
            let m = k.min(line.len());
            (if self.utf32 { m } else { utf16(&line[..m]) }) + (k - m)
        };
        let (s, t) = (conv(c0), conv(c0 + n));
        let kind = format!("{:?}", e.kind);
        self.diag_at(li, s, li, t, &kind, &e.message)
    }

    fn diag_at(
        &self,
        l1: usize,
        c1: usize,
        l2: usize,
        c2: usize,
        code: &str,
        message: &str,
    ) -> Value {
        let pos = |l: usize, c: usize| {
            obj(vec![
                ("line", (l as i64).into()),
                ("character", (c as i64).into()),
            ])
        };
        obj(vec![
            (
                "range",
                obj(vec![("start", pos(l1, c1)), ("end", pos(l2, c2))]),
            ),
            ("severity", 1i64.into()),
            ("code", code.into()),
            ("source", "sigil".into()),
            ("message", message.into()),
        ])
    }
}

/// 標準入出力などで言語サーバーを動かし、終了コードを返す。
pub fn serve(mut input: impl BufRead, output: impl Write, trust_base: Vec<String>) -> i32 {
    let mut server = Server::new(output, trust_base);
    loop {
        let frame = read_frame(&mut input, MAX_BODY);
        if let Some(code) = server.on_frame(frame) {
            return code;
        }
    }
}

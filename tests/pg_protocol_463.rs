//! ADR 0034 決定 2 の純粋な部品。評価器とネイティブを同じ Sigil 本体で照合する。
mod common;
#[path = "common/fuzz.rs"]
mod fuzz;
use sigil::exec::Options;
const PG: &str = include_str!("../src/stdlib/pg.sigil");
const TAIL: &str = "\n  intent \"試験\"\n  requires none\n  ensures none\n  effects none\n";
fn source(sig: &str, body: &str) -> String {
    // 純粋なフレーム部を試験する。接続部と埋込み専用 import は外部認証試験が担う。
    let pure = PG
        .split("// 接続は fd を一つだけ所有する。")
        .next()
        .unwrap();
    let pure = pure
        .lines()
        .filter(|line| !line.starts_with("use std.pg_scram."))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "{}\npub fn {sig}{TAIL}{{\n{body}\n}}\n",
        pure.replacen("module std.pg", "module app.pg", 1)
    )
    .replace("PgError", "ProtocolTestError")
}
fn convert(body: &str) -> String {
    let mut b = body.replace(".unwrap()", "?").replace(r"\0", r"\u{0}");
    for n in 0..=20 {
        b = b.replace(&format!("return {n};"), &format!("return Ok({n});"));
    }
    b
}
fn run(body: &str) {
    let s = source("main() -> Result<Int, PgError>", &convert(body));
    assert_eq!(
        common::run(&s, "main", &[])
            .unwrap_or_else(|e| panic!("{e:?}"))
            .value,
        "Ok(0)"
    );
}
#[test]
fn embedded_is_trust_none() {
    assert!(!PG.contains("unchecked"));
    assert!(!PG.contains("raw."));
    let opts = Options {
        embedded_roots: vec!["std.pg".into()],
        ..Options::captured()
    };
    let src = "module app.main\n  targets server\n  trust none\npub fn main() -> Int\n  intent \"試験\"\n  requires none\n  ensures none\n  effects none\n{ return 0; }";
    sigil::ir_eval::run_with(src, "main", &[], &opts).unwrap();
}
#[test]
fn types_and_frontend() {
    run(r#"
  let mut ps = List.new();
  ps.push(param(16, Some(PgValue.Boolean(true))).unwrap());
  ps.push(param(21, Some(PgValue.Integer(0 - 32768))).unwrap());
  ps.push(param(23, Some(PgValue.Integer(2147483647))).unwrap());
  ps.push(param(20, Some(PgValue.Integer((0 - 9223372036854775807) - 1))).unwrap());
  ps.push(param(25, Some(PgValue.Text("日本語"))).unwrap());
  ps.push(param(1043, None).unwrap());
  ps.push(param(17, Some(PgValue.Binary(Bytes.new()))).unwrap());
  let p = parse(&"select $1", &ps, 1048576).unwrap();
  let b = bind(&ps, 1048576).unwrap();
  if p.get(0).unwrap_or(0) != 80 || b.get(0).unwrap_or(0) != 66 { return 1; }
  if describe(7).unwrap().len() != 7 || execute(10).unwrap().len() != 10 || sync(5).unwrap().len() != 5 { return 2; }
  match decode_value(17, &"\\x00ff".as_bytes()).unwrap() { PgValue.Binary(v) => { if v.len() != 2 { return 3; } } _ => { return 4; } }
  match param(21, Some(PgValue.Integer(32768))) { Err(PgError.TypeMismatch) => {} _ => { return 5; } }
  match param(999, None) { Err(PgError.UnsupportedType) => {} _ => { return 6; } }
  match param(25, Some(PgValue.Text("a\0b"))) { Err(PgError.TypeMismatch) => {} _ => { return 7; } }
  match param(16, Some(PgValue.Integer(1))) { Err(PgError.TypeMismatch) => {} _ => { return 8; } }
  return 0;"#);
}
#[test]
fn fragmented_and_coalesced() {
    run(r#"
  let wire = frame(90, "I".as_bytes(), 6).unwrap();
  let mut all = wire.clone(); all.extend(&wire);
  let mut split = 0;
  while split <= wire.len() {
    let mut d = decoder(6).unwrap();
    let a = part(&wire, 0, split); let b = part(&all, split, all.len());
    let first = d.feed(&a).unwrap();
    if split < wire.len() {
      match first { PgProgress.NeedMore(n) => { if n != split { return 1; } } _ => { return 2; } }
      match d.feed(&b).unwrap() { PgProgress.Done(f, n) => { if n != wire.len() - split || f.tag != 90 { return 3; } } _ => { return 4; } }
    }
    match d.finish() { Ok(v) => {} Err(e) => { return 5; } }
    split = split + 1;
  }
  let mut d = decoder(6).unwrap();
  match d.feed(&all).unwrap() { PgProgress.Done(f, n) => { if n != 6 { return 6; } } _ => { return 7; } }
  let mut budget = budget(6, 0).unwrap();
  let f = PgFrame { tag: 90, body: "I".as_bytes() };
  budget.accept(&f).unwrap();
  match budget.accept(&f) { Err(PgError.LimitExceeded) => {} _ => { return 8; } }
  let mut partial = decoder(6).unwrap(); partial.feed(&"Z".as_bytes()).unwrap();
  match partial.finish() { Err(PgError.Protocol) => {} _ => { return 9; } }
  return 0;"#);
}
#[test]
fn backend_shapes_and_null() {
    run(r#"
  let mut b = Bytes.new(); b.push_u16_be(2);
  b.extend(&"a\0".as_bytes()); b.push_u32_be(0); b.push_u16_be(0); b.push_u32_be(25); b.push_u16_be(65535); b.push_u32_be(4294967295); b.push_u16_be(0);
  b.extend(&"b\0".as_bytes()); b.push_u32_be(0); b.push_u16_be(0); b.push_u32_be(17); b.push_u16_be(65535); b.push_u32_be(4294967295); b.push_u16_be(0);
  let desc_frame = PgFrame { tag: 84, body: b };
  let mut reader = PgReader { data: desc_frame.body.clone(), pos: 0 };
  let cols = columns(&mut reader)?;
  match backend(&desc_frame, &List.new())? { PgBackend.RowDescription(c) => {} _ => { return 1; } }
  let mut row = Bytes.new(); row.push_u16_be(2); row.push_u32_be(4294967295); row.push_u32_be(2); row.extend(&"\\x".as_bytes());
  let f = PgFrame { tag: 68, body: row };
  match backend(&f, &cols).unwrap() {
    PgBackend.DataRow(v) => {
      match v.get(0) { Some(x) => { match x { None => {} _ => { return 2; } } } _ => { return 3; } }
      match v.get(1) { Some(x) => { match x { Some(PgValue.Binary(empty_bytes)) => { if empty_bytes.len() != 0 { return 4; } } _ => { return 5; } } } _ => { return 6; } }
    }
    _ => { return 7; }
  }
  let err = PgFrame { tag: 69, body: "SERROR\0C23505\0Msecret SQL password\0\0".as_bytes() };
  match backend(&err, &cols).unwrap() { PgBackend.ErrorResponse(sqlstate) => { if sqlstate != "23505" { return 8; } } _ => { return 9; } }
  let mut bad = Bytes.new(); bad.push_u16_be(2); bad.push_u32_be(4294967294);
  match backend(&PgFrame { tag: 68, body: bad }, &cols) { Err(PgError.Protocol) => {} _ => { return 10; } }
  return 0;"#);
}
#[test]
fn value_boundaries() {
    let valid = [
        (16, "t"),
        (16, "f"),
        (21, "-32768"),
        (21, "32767"),
        (23, "-2147483648"),
        (23, "2147483647"),
        (20, "-9223372036854775808"),
        (20, "9223372036854775807"),
        (25, ""),
        (1043, "日本語"),
        (17, "\\x"),
        (17, "\\x00Ff"),
    ];
    let invalid = [
        (16, "true"),
        (21, "-32769"),
        (21, "32768"),
        (23, "2147483648"),
        (23, "-2147483649"),
        (20, "9223372036854775808"),
        (20, "-9223372036854775809"),
        (20, ""),
        (20, "+1"),
        (20, "1 "),
        (20, "--1"),
        (17, "\\x0"),
        (17, "\\xgg"),
        (17, "abc"),
    ];
    let mut body = String::new();
    for (oid, t) in valid {
        body.push_str(&format!("  match decode_value({oid}, &{t:?}.as_bytes()) {{ Ok(v) => {{}} Err(e) => {{ return 1; }} }}\n"));
    }
    for (oid, t) in invalid {
        body.push_str(&format!("  match decode_value({oid}, &{t:?}.as_bytes()) {{ Err(PgError.TypeMismatch) => {{}} _ => {{ return 2; }} }}\n"));
    }
    body.push_str("  let mut b = Bytes.new(); b.push(255); match decode_value(25, &b) { Err(PgError.TypeMismatch) => {} _ => { return 3; } } return 0;");
    run(&body);
}

#[test]
fn limits_and_broken_frames() {
    run(r#"
  // 許される最大wire長のヘッダーは受理し、1バイト超は本体の確保前に拒否。
  let mut header = Bytes.new(); header.push(67); header.push_u32_be(1048575);
  let mut exact = decoder(1048576)?;
  match exact.feed(&header) { Ok(PgProgress.NeedMore(n)) => { if n != 5 { return 9; } } _ => { return 10; } }
  match exact.finish() { Err(PgError.Protocol) => {} _ => { return 11; } }
  let mut over = Bytes.new(); over.push(67); over.push_u32_be(1048576);
  let mut capped = decoder(1048576)?;
  match capped.feed(&over) { Err(PgError.LimitExceeded) => {} _ => { return 12; } }
  match decoder(1048577) { Err(PgError.LimitExceeded) => {} _ => { return 13; } }
  match budget(4194305, 10000) { Err(PgError.LimitExceeded) => {} _ => { return 14; } }
  match budget(4194304, 10001) { Err(PgError.LimitExceeded) => {} _ => { return 15; } }
  let mut sql_bytes = "a".as_bytes(); let mut doubling = 0;
  while doubling < 16 { let copy = sql_bytes.clone(); sql_bytes.extend(&copy); doubling = doubling + 1; }
  parse(&sql_bytes.to_string_utf8().unwrap_or(""), &List.new(), 1048576)?;
  sql_bytes.push(97);
  match parse(&sql_bytes.to_string_utf8().unwrap_or(""), &List.new(), 1048576) { Err(PgError.LimitExceeded) => {} _ => { return 16; } }
  let mut b = Bytes.new(); let mut i = 0;
  while i < 251 { b.push(65); i = i + 1; }
  frame(67, b.clone(), 256)?;
  b.push(65);
  match frame(67, b, 256) { Err(PgError.LimitExceeded) => {} _ => { return 1; } }
  let mut lengths = List.new(); lengths.push(0); lengths.push(3); lengths.push(2147483648); lengths.push(4294967295);
  i = 0;
  while i < lengths.len() {
    let n = match lengths.get(i) { Some(n) => n, None => 0 };
    let mut bad = Bytes.new(); bad.push(68); bad.push_u32_be(n);
    let mut d = decoder(256)?;
    match d.feed(&bad) { Err(PgError.Protocol) => {} _ => { return 2; } }
    match d.feed(&Bytes.new()) { Err(PgError.Protocol) => {} _ => { return 3; } }
    i = i + 1;
  }
  let mut bad = Bytes.new(); bad.push(68); bad.push_u32_be(256);
  let mut d = decoder(256)?;
  match d.feed(&bad) { Err(PgError.LimitExceeded) => {} _ => { return 4; } }
  let mut cols = List.new(); i = 0;
  while i < 256 { cols.push(PgColumn { name: "", oid: 25 }); i = i + 1; }
  let mut row = Bytes.new(); row.push_u16_be(256); i = 0;
  while i < 256 { row.push_u32_be(4294967295); i = i + 1; }
  let f = PgFrame { tag: 68, body: row };
  backend(&f, &cols)?;
  let mut budget = budget(4194304, 1)?; budget.accept(&f)?;
  match budget.accept(&f) { Err(PgError.LimitExceeded) => {} _ => { return 5; } }
  let mut too_many = Bytes.new(); too_many.push_u16_be(257);
  match backend(&PgFrame { tag: 84, body: too_many }, &cols) { Err(PgError.LimitExceeded) => {} _ => { return 6; } }
  let mut ps = List.new(); i = 0;
  while i < 256 { ps.push(param(25, None)?); i = i + 1; }
  parse(&"", &ps, 1048576)?; bind(&ps, 1048576)?;
  ps.push(param(25, None)?);
  match bind(&ps, 1048576) { Err(PgError.LimitExceeded) => {} _ => { return 7; } }
  match parse(&"", &ps, 1048576) { Err(PgError.LimitExceeded) => {} _ => { return 8; } }
  return 0;"#);
}

fn fuzz_source() -> String {
    let extra = r#"
fn fingerprint(data: &Bytes, split: Int) -> Int
  intent "一括と分割の結果を比較する"
  requires none
  ensures none
  effects none
{
  let mut d = decoder(256).unwrap_or(PgDecoder { data: Bytes.new(), failed: true, max: 256 });
  let mut pos = 0; let mut sum = 0;
  while pos < data.len() {
    let mut end = data.len(); if pos < split { end = split; }
    let b = part(&data, pos, end);
    match d.feed(&b) {
      Err(e) => { return 0 - 1; }
      Ok(pr) => { match pr {
        PgProgress.NeedMore(n) => { pos = pos + n; }
        PgProgress.Done(f, n) => {
          // 任意のbodyを型とbackend解釈へ渡しても契約停止しない。
          decode_value(20, &f.body); decode_value(17, &f.body); backend(&f, &List.new());
          sum = (sum + f.tag + f.body.len()) % 100000;
          pos = pos + n;
        }
      } }
    }
  }
  match d.finish() { Ok(v) => { return sum; } Err(e) => { return 0 - 2; } }
}
"#;
    let mut s = source(
        "probe(input: List<Int>) -> Int",
        r#"
  let mut data = Bytes.new(); let mut i = 0;
  while i < input.len() { let n = match input.get(i) { Some(n) => n, None => 0 }; data.push(n); i = i + 1; }
  let want = fingerprint(&data, data.len());
  i = 0;
  while i <= data.len() { if fingerprint(&data, i) != want { return 1; } i = i + 1; }
  return 0;
"#,
    );
    s.push_str(extra);
    s
}
#[test]
fn short_fuzz_fragmentation() {
    use sigil::exec::Arg;
    let src = fuzz_source();
    let corpus = vec![
        vec![90, 0, 0, 0, 5, 73],
        vec![49, 0, 0, 0, 4],
        vec![68, 255, 255, 255, 255],
        vec![67, 0, 0, 0, 9, 65, 0],
    ];
    let mut rng = fuzz::Rng::new(fuzz::seed());
    let mut inputs = corpus.clone();
    for i in 0..fuzz::iters(64) {
        let b = match i % 3 {
            0 => fuzz::random_bytes(&mut rng),
            1 => {
                let base = rng.pick(&corpus).clone();
                fuzz::mutate(&mut rng, &base, &corpus)
            }
            _ => {
                let mut b = vec![67, 0, 0, 0, 4];
                let n = rng.below(32);
                b[4] = (n + 4) as u8;
                b.extend((0..n).map(|_| rng.next_u64() as u8));
                b
            }
        };
        inputs.push(b);
    }
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/fuzz-failures/pg_protocol");
    std::fs::create_dir_all(&dir).unwrap();
    for (i, b) in inputs.iter().enumerate() {
        std::fs::write(dir.join("current"), b).unwrap();
        let args = [Arg::List(
            b.iter().map(|b| Arg::Int(i64::from(*b))).collect(),
        )];
        let result = if i < 4 {
            common::run(&src, "probe", &args)
        } else {
            sigil::ir_eval::run_with(&src, "probe", &args, &Options::captured())
        };
        match result {
            Ok(v) if v.value == "0" => {}
            other => {
                let p = dir.join(format!("fail-{}-{i}", fuzz::seed()));
                std::fs::write(&p, b).unwrap();
                panic!("{}: {other:?}", p.display());
            }
        }
    }
    std::fs::remove_file(dir.join("current")).unwrap();
}

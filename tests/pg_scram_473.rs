//! ADR 0034 SCRAM: RFC 7677 と入力の上限。評価器/native 共通実装。
mod common;
const SCRAM: &str = include_str!("../src/stdlib/pg_scram.sigil");
fn source(body: &str, effects: &str) -> String {
    format!("{}\npub fn main() -> Result<Int, ScramError>\n intent \"試験\"\n requires none\n ensures none\n effects {effects}\n{{\n{body}\n}}", SCRAM.replacen("module std.pg_scram", "module app.scram", 1))
}
#[test]
fn rfc7677() {
    let src = source(
        r#"
 let p = Secret.from_bytes("pencil".as_bytes());
 let n = "rOprNGfwEbeRWgbNEkqO".as_bytes();
 let s = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096".as_bytes();
 let mut bare = "n=user,r=".as_bytes(); bare.extend(&n);
 let r = respond_bare(&p, &n, &s, &bare)?;
 let want = "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=".as_bytes();
 if !ct_eq(&r.message, &want) { return Ok(1); }
 let good = "v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=".as_bytes();
 let good_ok = verify(&r.signature, &good)?;
 if !good_ok { return Ok(2); }
 let bad = "v=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".as_bytes();
 let bad_ok = verify(&r.signature, &bad)?;
 if bad_ok { return Ok(3); }
 return Ok(0);
"#,
        "task.suspend",
    );
    assert_eq!(common::run(&src, "main", &[]).unwrap().value, "Ok(0)");
}
#[test]
fn format_and_bounds() {
    let src = source(
        r#"
 let mut i = 0;
 while i < 256 {
   let mut b = Bytes.new(); b.push(i);
   let enc = ct_base64(&b); let dec = unbase64(&enc, 1)?;
   if !ct_eq(&b, &dec) { return Ok(1); }
   i = i + 1;
 }
 if credential(&Bytes.new()) || credential(&"日本語".as_bytes()) || credential(&"a\u{0}".as_bytes()) { return Ok(2); }
 if !credential(&"pencil".as_bytes()) { return Ok(3); }
 let n = "abc".as_bytes();
 let mut cases = List.new();
 cases.push("r=abc,s=c2FsdA==,i=4096");
 cases.push("r=abcd,s=c2FsdA==,i=4095");
 cases.push("r=abcd,s=c2FsdA==,i=04096");
 cases.push("r=abcd,s=c2FsdA==,i=100001");
 cases.push("r=abcd,s=c2FsdA==,i=999999999999999999999999");
 cases.push("r=abcd,s=c2FsdA==,i=4096,i=4096");
 cases.push("r=abcd,s=c2FsdA==,i=4096,m=x");
 cases.push("r=abcd,s=c2FsdB==,i=4096");
 cases.push("r=abcd,s=,i=4096");
 cases.push("r=abcd,s=c2FsdA==");
 cases.push("r=abcd,s=c2FsdA==,i=4096,");
 let mut k = 0;
 while k < cases.len() {
   let c = cases.get(k).map(|x| x.clone()).unwrap_or("");
   match challenge(&n, &c.as_bytes()) { Err(e) => {} Ok(v) => { return Ok(4 + k); } }
   k = k + 1;
 }
 let lower = "r=abcd,s=c2FsdA==,i=4096".as_bytes();
 let upper = "r=abcd,s=c2FsdA==,i=100000".as_bytes();
 let lo = challenge(&n, &lower)?; let hi = challenge(&n, &upper)?;
 if lo.iterations != 4096 || hi.iterations != 100000 { return Ok(40); }
 let mut salt = Bytes.new(); let mut j = 0;
 while j < 1024 { salt.push(1); j = j + 1; }
 let mut sm = "r=abcd,s=".as_bytes(); sm.extend(&ct_base64(&salt)); sm.extend(&",i=4096".as_bytes());
 let valid = challenge(&n, &sm)?; if valid.salt.len() != 1024 { return Ok(41); }
 salt.push(1);
 let mut over = "r=abcd,s=".as_bytes(); over.extend(&ct_base64(&salt)); over.extend(&",i=4096".as_bytes());
 match challenge(&n, &over) { Err(ScramError.LimitExceeded) => {} _ => { return Ok(42); } }
 let mut maximum = "r=abcd".as_bytes();
 while maximum.len() < 16384 - 18 { maximum.push(120); }
 maximum.extend(&",s=c2FsdA==,i=4096".as_bytes());
 let accepted = challenge(&n, &maximum)?;
 maximum.push(120);
 match challenge(&n, &maximum) { Err(ScramError.LimitExceeded) => {} _ => { return Ok(43); } }
 let sig = Secret.from_bytes(Bytes.new());
 match verify(&sig, &"v=AAAA".as_bytes()) { Err(ScramError.Protocol) => {} _ => { return Ok(44); } }
 match verify(&sig, &"v=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB=".as_bytes()) { Err(ScramError.Protocol) => {} _ => { return Ok(45); } }
 return Ok(0);
 "#,
        "none",
    );
    assert_eq!(common::run(&src, "main", &[]).unwrap().value, "Ok(0)");
}

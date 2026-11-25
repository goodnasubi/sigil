//! ADR 0064 の「境界と相互運用」の内部試験。nonce の範囲違反・r = 0・s = 0・全候補棄却と、
//! 最初の有効な候補の選択、RFC 6979 の候補列を、公開 API からは見えない内部関数で確かめる。
//! `embedded_extra` で `std.crypto` に試験用の関数を足して、SIR 評価器で実行する。

use sigil::exec::Options;
use std::fmt::Write;

const HEAD: &str = "module crypto_ecdsa_p256_internal.t\n  targets server\n  trust none\n\npub fn main() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 7;\n}\n";

const KEY_D: &str = "c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721";
const SAMPLE_K: &str = "a6e3c57dd01abe90086538398355dd4c3b17aa873382b0f24d6129493d8aad60";
const SAMPLE_R: &str = "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716";
const SAMPLE_S: &str = "f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8";
const ORDER_N: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551";
const ORDER_N_PLUS_1: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632552";
const ORDER_N_MINUS_1: &str = "ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632550";
// SHA-256("sample") を n で割った余り
const SAMPLE_Z: &str = "af2bdbe1aa9b6ec1e2ade1d694f41fc71a831d0268e9891562113d8a62add1bf";
// e + r * d = 0 (mod n) になる e（s = 0 を作る）。r は SAMPLE_R、d は KEY_D。
const E_FOR_ZERO_S: &str = "08ee301548cd9aa52ec3f69fd87f9c57bdf20e9f20419649d0e1b6c700f22e78";
// RFC 6979 A.2.5 の秘密鍵・"sample" の候補列（V）。先頭が SAMPLE_K。
const SAMPLE_V: [&str; 4] = [
    "a6e3c57dd01abe90086538398355dd4c3b17aa873382b0f24d6129493d8aad60",
    "8e83dc490bc5fc4d5992bd63cd87f254adffcb930f8a8011702a88870f638fdb",
    "7b8dc9ad8ce159abca1b9915fc1470e91d5ad2443b3032557e78f47e180ab702",
    "0c2161730a70227da55c7d16df1b6e1302e751bab0caf723ff830f7ba60a30ad",
];

fn bytes_decl(name: &str, hex: &str) -> String {
    let mut s = String::new();
    writeln!(s, "  let mut {name} = Bytes.new();").unwrap();
    for i in (0..hex.len()).step_by(2) {
        writeln!(
            s,
            "  {name}.push({});",
            u8::from_str_radix(&hex[i..i + 2], 16).unwrap()
        )
        .unwrap();
    }
    s
}

fn probe(body: &str) -> String {
    let module = format!(
        "{}\npub fn internal_probe() -> Int\n  intent \"内部の演算を照合する\"\n  requires none\n  ensures none\n  effects none\n{{\n{body}\n}}\n",
        include_str!("../src/stdlib/crypto.sigil"),
    );
    let o = Options {
        embedded_extra: vec![("std.crypto".to_string(), module)],
        embedded_roots: vec!["std.crypto".into()],
        ..Options::captured()
    };
    sigil::ir_eval::run_sources_with(&[HEAD], "std.crypto.internal_probe", &[], &o)
        .unwrap_or_else(|f| panic!("{:?}", f.error))
        .value
}

/// 署名の前準備（sample の e・d をモンゴメリ表現にしたもの）。
fn setup() -> String {
    let mut s = String::new();
    s.push_str(&bytes_decl("key_bytes", KEY_D));
    s.push_str(&bytes_decl("z_bytes", SAMPLE_Z));
    s.push_str(
        r#"  let zero = p256_const_zero();
  let r2n = p256_const_r2_n();
  let z = p256_fe_decode(&z_bytes);
  let e = p256_fn_add(&z, &zero);
  let em = p256_fn_mul(&e, &r2n);
  let d = p256_fe_decode(&key_bytes);
  let dm = p256_fn_mul(&d, &r2n);
"#,
    );
    s
}

/// nonce の範囲違反（0・n・n + 1・全ビット 1）は候補を無効にし、1 と n - 1 と RFC の k は有効にする。
#[test]
fn nonce_range_violations_invalidate_the_candidate() {
    let mut body = setup();
    body.push_str("  let mut bad = 0;\n");
    for (i, (hex, want)) in [
        (&"00".repeat(32), 0),
        (&ORDER_N.to_owned(), 0),
        (&ORDER_N_PLUS_1.to_owned(), 0),
        (&"ff".repeat(32), 0),
        (&("00".repeat(31) + "01"), 1),
        (&ORDER_N_MINUS_1.to_owned(), 1),
        (&SAMPLE_K.to_owned(), 1),
    ]
    .iter()
    .enumerate()
    {
        body.push_str(&bytes_decl(&format!("k{i}"), hex));
        write!(
            body,
            "  let c{i} = p256_candidate(&k{i}, &em, &dm);\n  if c{i}.ok != {want} {{ bad = bad + {}; }}\n",
            1 << i
        )
        .unwrap();
    }
    // RFC の k は有効で、r・s が付録 A.2.5 の値になる。
    body.push_str(&bytes_decl("want_r", SAMPLE_R));
    body.push_str(&bytes_decl("want_s", SAMPLE_S));
    body.push_str(
        r#"  if !ct_eq(&p256_fe_encode(&c6.r), &want_r) { bad = bad + 128; }
  if !ct_eq(&p256_fe_encode(&c6.s), &want_s) { bad = bad + 256; }
  return bad;"#,
    );
    assert_eq!(probe(&body), "0");
}

/// r = 0（アフィン X が 0 または n）と s = 0 の候補は無効になる。
#[test]
fn zero_r_and_zero_s_invalidate_the_candidate() {
    let mut body = setup();
    body.push_str(&bytes_decl("sample_k", SAMPLE_K));
    body.push_str(&bytes_decl("sample_r", SAMPLE_R));
    body.push_str(&bytes_decl("order", ORDER_N));
    body.push_str(&bytes_decl("e_zero_s", E_FOR_ZERO_S));
    body.push_str(
        r#"  let mut bad = 0;
  let ks = p256_fe_decode(&sample_k);
  let zero_x = p256_const_zero();
  let c0 = p256_finish(&zero_x, &ks, 1, &em, &dm);
  if c0.ok != 0 { bad = bad + 1; }
  let order_x = p256_fe_decode(&order);
  let c1 = p256_finish(&order_x, &ks, 1, &em, &dm);
  if c1.ok != 0 { bad = bad + 2; }
  let real_x = p256_fe_decode(&sample_r);
  let c2 = p256_finish(&real_x, &ks, 1, &em, &dm);
  if c2.ok != 1 { bad = bad + 4; }
  let e0 = p256_fe_decode(&e_zero_s);
  let e0m = p256_fn_mul(&e0, &r2n);
  let c3 = p256_finish(&real_x, &ks, 1, &e0m, &dm);
  if c3.ok != 0 { bad = bad + 8; }
  if p256_nonzero(&c3.r) != 1 { bad = bad + 16; }
  if p256_nonzero(&c3.s) != 0 { bad = bad + 32; }
  let c4 = p256_finish(&real_x, &ks, 0, &em, &dm);
  if c4.ok != 0 { bad = bad + 64; }
  return bad;"#,
    );
    assert_eq!(probe(&body), "0");
}

fn small(v: i64) -> String {
    format!("p256_fe_pack({v}, 0, 0, 0, 0, 0, 0, 0, 0, 0)")
}

/// 最初に有効な候補だけが選ばれ、1 つも無ければ署名生成失敗を返す。
#[test]
fn the_first_valid_candidate_is_picked_and_exhaustion_fails() {
    let mut body = String::from("  let mut bad = 0;\n");
    // (ok の並び, 選ばれる r/s の印)。印は 0 なら選ばれない。
    let cases: [([i64; 4], i64); 6] = [
        ([0, 1, 1, 0], 2),
        ([1, 1, 1, 1], 1),
        ([0, 0, 0, 1], 4),
        ([0, 0, 1, 1], 3),
        ([0, 0, 0, 0], 0),
        ([1, 0, 0, 0], 1),
    ];
    for (ci, (oks, want)) in cases.iter().enumerate() {
        writeln!(
            body,
            "  let mut pick{ci} = P256Pick {{ r: {z}, s: {z}, taken: 0 }};",
            z = small(0)
        )
        .unwrap();
        for (j, ok) in oks.iter().enumerate() {
            let mark = (j + 1) as i64;
            writeln!(
                body,
                "  let cand{ci}_{j} = P256Candidate {{ r: {r}, s: {s}, ok: {ok} }};\n  pick{ci} = p256_pick(pick{ci}, &cand{ci}_{j});",
                r = small(mark),
                s = small(mark + 10)
            )
            .unwrap();
        }
        let any = oks.contains(&1);
        writeln!(
            body,
            "  if pick{ci}.taken != {} {{ bad = bad + {}; }}\n  if p256_fe_read(&pick{ci}.r, 0) != {want} {{ bad = bad + {}; }}\n  if p256_fe_read(&pick{ci}.s, 0) != {} {{ bad = bad + {}; }}",
            i64::from(any),
            1 << (ci * 3),
            1 << (ci * 3 + 1),
            if *want == 0 { 0 } else { want + 10 },
            1 << (ci * 3 + 2),
        )
        .unwrap();
        // 署名への変換: 無効なら署名生成失敗
        writeln!(
            body,
            "  match p256_signature_from(&pick{ci}) {{\n    Ok(_) => {{ if {fail} {{ bad = bad + 4096; }} }}\n    Err(_) => {{ if !{fail} {{ bad = bad + 8192; }} }}\n  }}",
            fail = if any { "false" } else { "true" }
        )
        .unwrap();
    }
    body.push_str("  return bad;");
    assert_eq!(probe(&body), "0");
}

/// 全候補が棄却されたときの失敗は、署名生成失敗（`SigningFailed`）である。
#[test]
fn exhaustion_reports_signing_failed() {
    let body = format!(
        r#"  let pick = P256Pick {{ r: {z}, s: {z}, taken: 0 }};
  match p256_signature_from(&pick) {{
    Ok(_) => {{ return 1; }}
    Err(e) => {{
      match e {{
        EcdsaP256Error.SigningFailed => {{ return 0; }}
        _ => {{ return 2; }}
      }}
    }}
  }}"#,
        z = small(0)
    );
    assert_eq!(probe(&body), "0");
}

/// RFC 6979 の候補列（K・V の更新）が、独立実装の値と一致する。先頭は付録 A.2.5 の k。
#[test]
fn rfc6979_candidate_sequence_matches_the_independent_implementation() {
    let mut body = String::new();
    body.push_str(&bytes_decl("x", KEY_D));
    body.push_str(&bytes_decl("h1", SAMPLE_Z));
    for (i, v) in SAMPLE_V.iter().enumerate() {
        body.push_str(&bytes_decl(&format!("v{i}"), v));
    }
    body.push_str("  let mut bad = 0;\n  let mut st = p256_drbg_init(&x, &h1);\n");
    for i in 0..4 {
        writeln!(
            body,
            "  st = p256_drbg_next(st);\n  if !ct_eq(&st.v, &v{i}) {{ bad = bad + {}; }}\n  st = p256_drbg_step(st);",
            1 << i
        )
        .unwrap();
    }
    body.push_str("  return bad;");
    assert_eq!(probe(&body), "0");
}

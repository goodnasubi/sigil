//! `std.http` の増分の解釈器（ADR 0032 決定 2-3、PR 6-3g-b）を評価器で動かす試験の補助。
//!
//! 入口は `tests/data/http_driver.sigil`（評価器とネイティブで同じ）。1 回の lowering で多数の入力を処理するため、
//! 入力ごとに 1 つのルートタスクとして `run_roots_with` で動かす（ADR 0020 決定 2 の「Sigil の対象」）。
//!
//! `probe` の記録（`whole#bytes#cuts#rt`）について、次を確かめる（決定 7 の性質）。
//! - どの入力にも停止せずに値を返す（契約違反・桁あふれ・範囲の外の読み書き・スタックの枯渇は誤り）。
//! - 全体で 1 回・1 バイトずつ・任意の位置で分けて与えた記録が同じ（保持の欄を除く）。
//! - 消費したバイト数の合計が入力の長さを超えない（未完・失敗が無ければ、ちょうど入力の長さ）。
//! - 1 バイトずつ与えたときに観測した保持の最大が、要求の行・見出しの部分・本文の上限の和を超えない。
//! - 完成した要求を書き出して読み直すと同じ要求になる（往復）。
#![allow(dead_code)]

use sigil::exec::{Arg, Options};

/// 解釈の上限の既定値（`std.http.limits` と同じ。`limit_http_defaults` が表と一致を確かめる）。
pub const DEFAULTS: [i64; 6] = [8192, 16384, 100, 1_048_576, 1024, 2_097_152];

pub fn driver() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/http_driver.sigil"
    ))
    .unwrap()
}

fn ints(b: &[u8]) -> Arg {
    Arg::List(b.iter().map(|&x| Arg::Int(x as i64)).collect())
}

fn list(xs: &[i64]) -> Arg {
    Arg::List(xs.iter().map(|&x| Arg::Int(x)).collect())
}

/// 入口 `func` を、引数の組ごとのルートタスクとして 1 回の lowering で動かし、各ルートの値（文字列の引用符を除く）を返す。
/// 停止したルートは `Err(停止の説明)`。
pub fn roots(func: &str, sets: &[Vec<Arg>]) -> Vec<Result<String, String>> {
    let src = driver();
    let out = sigil::ir_eval::run_roots_with(&[&src], func, sets, &Options::captured())
        .unwrap_or_else(|f| panic!("評価器が失敗した: {f:?}"));
    out.roots
        .into_iter()
        .map(|r| match r {
            Ok(s) => Ok(s.trim_matches('"').to_string()),
            Err(stop) => Err(format!("{stop:?}")),
        })
        .collect()
}

/// 1 つの入力の記録を分けたもの。
#[derive(Debug, Clone)]
pub struct Probe {
    /// 全体で 1 回与えた記録（保持の欄を除く）。
    pub whole: String,
    /// 1 バイトずつ与えたときの保持の最大。
    pub held: i64,
    /// 往復の結果（`ok<要求の数>` か `NG:...`）。
    pub rt: String,
}

fn split_held(rec: &str) -> Result<(String, i64), String> {
    let (body, h) = rec
        .rsplit_once("|H")
        .or_else(|| rec.strip_prefix('H').map(|h| ("", h)))
        .ok_or_else(|| format!("保持の欄が無い: {rec}"))?;
    let h = h.parse().map_err(|_| format!("保持が整数でない: {rec}"))?;
    Ok((body.to_string(), h))
}

/// 使う上限（0 は既定値）。
pub fn effective(lim: &[i64]) -> [i64; 6] {
    let mut out = DEFAULTS;
    for (i, v) in lim.iter().enumerate().take(6) {
        if *v > 0 {
            out[i] = *v;
        }
    }
    out
}

/// 記録の各項目の消費の合計。
pub fn consumed_total(whole: &str) -> (i64, bool) {
    let mut sum = 0;
    let mut complete = true;
    for item in whole.split('|').filter(|s| !s.is_empty()) {
        let n: i64 = match item.as_bytes()[0] {
            b'R' => item[1..].split(';').next().unwrap().parse().unwrap(),
            b'E' => {
                complete = false;
                item.split('@').nth(1).unwrap().parse().unwrap()
            }
            b'P' => item[1..].parse().unwrap(),
            _ => panic!("記録の形が違う: {item}"),
        };
        sum += n;
    }
    (sum, complete)
}

/// 1 つの記録（`probe` の値）を確かめて分ける。性質が破れたら `Err(理由)`。
pub fn check_probe(input: &[u8], lim: &[i64], rec: &str) -> Result<Probe, String> {
    let parts: Vec<&str> = rec.split('#').collect();
    if parts.len() != 4 {
        return Err(format!("記録の形が違う: {rec}"));
    }
    let (whole, _) = split_held(parts[0])?;
    let (bytes, held) = split_held(parts[1])?;
    let (cuts, _) = split_held(parts[2])?;
    if whole != bytes {
        return Err(format!(
            "1 バイトずつの結果が違う:\n全体: {whole}\n1 バイトずつ: {bytes}"
        ));
    }
    if whole != cuts {
        return Err(format!(
            "分けて与えた結果が違う:\n全体: {whole}\n分割: {cuts}"
        ));
    }
    let (sum, complete) = consumed_total(&whole);
    if sum > input.len() as i64 || (complete && sum != input.len() as i64) {
        return Err(format!(
            "消費の合計 {sum} が入力の長さ {} と合わない: {whole}",
            input.len()
        ));
    }
    let l = effective(lim);
    if held > l[0] + l[1] + l[3] {
        return Err(format!(
            "保持 {held} が上限の和 {} を超えた",
            l[0] + l[1] + l[3]
        ));
    }
    let rt = parts[3].to_string();
    if !rt.starts_with("ok") {
        return Err(format!("往復で違う要求になった: {rt}: {whole}"));
    }
    Ok(Probe { whole, held, rt })
}

/// 入力の組（入力, 分ける位置, 上限）をまとめて `probe` で動かし、性質を確かめた記録を返す（破れたら panic）。
pub fn probe_all(cases: &[(Vec<u8>, Vec<i64>, Vec<i64>)]) -> Vec<Probe> {
    let sets: Vec<Vec<Arg>> = cases
        .iter()
        .map(|(b, c, l)| vec![ints(b), list(c), list(l)])
        .collect();
    let got = roots("probe", &sets);
    got.into_iter()
        .zip(cases)
        .map(|(r, (b, _, l))| {
            let rec = r.unwrap_or_else(|e| panic!("停止した: {e}\n入力: {:?}", show(b)));
            check_probe(b, l, &rec).unwrap_or_else(|e| panic!("{e}\n入力: {:?}", show(b)))
        })
        .collect()
}

/// 既定の上限と既定の分け方（いくつかの位置）で 1 つの入力を確かめる。
pub fn probe(input: &[u8]) -> Probe {
    probe_lim(input, &[])
}

pub fn probe_lim(input: &[u8], lim: &[i64]) -> Probe {
    let n = input.len() as i64;
    let cuts = vec![1, n / 3, n / 2, n - 1];
    probe_all(&[(input.to_vec(), cuts, lim.to_vec())])
        .pop()
        .unwrap()
}

/// 失敗の状態コード（失敗でなければ `None`）。
pub fn status(p: &Probe) -> Option<u32> {
    p.whole
        .split('|')
        .find(|s| s.starts_with('E'))
        .map(|s| s[1..].split('@').next().unwrap().parse().unwrap())
}

/// 完成した要求の記録（`R` の項目。消費の欄を除く）。
pub fn requests(p: &Probe) -> Vec<String> {
    p.whole
        .split('|')
        .filter(|s| s.starts_with('R'))
        .map(|s| s.split_once(';').unwrap().1.to_string())
        .collect()
}

pub fn show(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// 16 進の小文字。
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn unhex(s: &str) -> Vec<u8> {
    let s: Vec<u8> = s.bytes().filter(|c| c.is_ascii_hexdigit()).collect();
    s.chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
        .collect()
}

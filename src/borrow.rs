//! 借用の区間の計算（ADR 0003 の規則 2・3、ADR 0006 の条件 8）。
//!
//! 検査器（`check.rs`）から切り離した、独立の部品。参照を持つ変数（保持者）が場所を借りていて、
//! その場所が書き換え・移動・`&mut` の借用（`&mut` の貸しなら読み出しも）で使われると、貸しは
//! 「使えなくなる」。使えなくなった貸しの保持者を後で使うとエラーになる。つまり借用の区間は、
//! 借りた地点から、保持者を最後に使う地点までである。
//!
//! 部品の範囲は `Loans` と `overlaps` である（ADR 0013 の決定 4）。貸しが使えなくなる規則は、
//! この部品だけが持ち、検査器は同じ部品を呼ぶ。検査器が持つのは制御の流れと、式と出来事の対応だけである。
//! - `Loans`: 検査器が、式を調べながら更新する貸しの集まり。
//! - `regions`: 出来事の列を渡すと、衝突を返す、単体テスト用の駆動部。`Loans` を直線の列で動かす。
//!   各事例には、検査器を通す Sigil のソースの対のテストが `tests/borrow_pairs.rs` にある。

use crate::error::Span;
use std::ops::{Deref, DerefMut};

/// 一時的な値を借りている貸しの経路。
pub const TEMP: &str = "<temp>";

/// 貸しが使えなくなった理由。
#[derive(Debug, Clone, PartialEq)]
pub enum Dead {
    /// 借りた場所を、`&mut` の借用・書き換え・移動・（`&mut` の貸しなら）読み出しで使った。
    Conflict(String),
    /// 一時的な値が文の終わりで破棄された。
    Temp,
}

/// 変数 `holder` が場所 `path` を借りている。使えなくなったら `dead` に位置と理由を入れ、
/// `holder` を後で使った時点でエラーにする（最後に使う地点までが区間）。
#[derive(Debug, Clone, PartialEq)]
pub struct Loan {
    pub holder: String,
    pub path: String,
    pub mutable: bool,
    pub dead: Option<(Span, Dead)>,
}

/// 2 つの経路が重なるか（同じか、一方が他方に `.フィールド` を足したもの）。
pub fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}.")) || b.starts_with(&format!("{a}."))
}

/// 貸しの集まり。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loans(Vec<Loan>);

impl Deref for Loans {
    type Target = Vec<Loan>;
    fn deref(&self) -> &Vec<Loan> {
        &self.0
    }
}

impl DerefMut for Loans {
    fn deref_mut(&mut self) -> &mut Vec<Loan> {
        &mut self.0
    }
}

impl Loans {
    /// 場所 `path` が使われた。重なる貸しを使えなくする（規則 2）。
    /// 読み出しは `&mut` の貸しだけを、書き換え・`&mut` の借用・移動は全ての貸しを使えなくする。
    pub fn touch(&mut self, path: &str, write: bool, at: Span, op: &str) {
        for l in &mut self.0 {
            if l.dead.is_none() && l.path != TEMP && overlaps(&l.path, path) && (write || l.mutable)
            {
                l.dead = Some((at, Dead::Conflict(op.to_string())));
            }
        }
    }

    /// `holder` が `path` を借りた。
    pub fn lend(&mut self, holder: &str, path: &str, mutable: bool, dead: Option<(Span, Dead)>) {
        self.0.push(Loan {
            holder: holder.to_string(),
            path: path.to_string(),
            mutable,
            dead,
        });
    }

    /// `holder` がいま借りている場所（経路と、`&mut` か）。
    pub fn held(&self, holder: &str) -> Vec<(String, bool)> {
        self.0
            .iter()
            .filter(|l| l.holder == holder)
            .map(|l| (l.path.clone(), l.mutable))
            .collect()
    }

    /// `holder` の使えなくなった貸し（最初の 1 つ）。
    pub fn dead_of(&self, holder: &str) -> Option<&Loan> {
        self.0
            .iter()
            .find(|l| l.holder == holder && l.dead.is_some())
    }

    /// 経路の合流。同じ貸しは、どちらかで使えなくなっていれば使えなくなったとする。
    pub fn merge(mut self, other: Loans) -> Loans {
        for l in other.0 {
            match self
                .0
                .iter_mut()
                .find(|x| x.holder == l.holder && x.path == l.path && x.mutable == l.mutable)
            {
                Some(x) => {
                    if x.dead.is_none() {
                        x.dead = l.dead;
                    }
                }
                None => self.0.push(l),
            }
        }
        self
    }
}

/// 区間の規則を確かめるための、出来事。出来事の番号（1 始まり）が位置の行になる。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `holder` が `path` を借りる（`&path` または `&mut path`）。借りること自体も場所の使用になる。
    Borrow {
        holder: String,
        path: String,
        mutable: bool,
    },
    /// `holder` が、`from` の借りている場所をそのまま引き継ぐ（`let r2 = r;` や、返された参照。規則 3）。
    Derive { holder: String, from: String },
    /// 場所を読む。
    Read(String),
    /// 場所を書き換える。
    Write(String),
    /// 場所を移動する。
    Move(String),
    /// 保持者を使う。
    Use(String),
}

/// 区間の規則に反した使い方。
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// 保持者を使った出来事の番号。
    pub used_at: u32,
    pub holder: String,
    /// 借りていた場所。
    pub path: String,
    /// 貸しを使えなくした出来事の番号。
    pub killed_at: u32,
}

/// 出来事の列を順に処理して、衝突を返す。
pub fn regions(events: &[Event]) -> Vec<Conflict> {
    let mut loans = Loans::default();
    let mut out = vec![];
    for (i, ev) in events.iter().enumerate() {
        let at = Span {
            line: i as u32 + 1,
            col: 1,
        };
        match ev {
            Event::Borrow {
                holder,
                path,
                mutable,
            } => {
                loans.touch(path, *mutable, at, "借用");
                let mut srcs = vec![(path.clone(), *mutable)];
                // 借りた先が参照の保持者なら、その貸しも引き継ぐ（再借用）
                srcs.extend(loans.held(path.split('.').next().unwrap_or(path)));
                for (p, m) in srcs {
                    loans.lend(holder, &p, m, None);
                }
            }
            Event::Derive { holder, from } => {
                for (p, m) in loans.held(from) {
                    loans.lend(holder, &p, m, None);
                }
            }
            Event::Read(p) => loans.touch(p, false, at, "読み出し"),
            Event::Write(p) => loans.touch(p, true, at, "書き換え"),
            Event::Move(p) => loans.touch(p, true, at, "移動"),
            Event::Use(h) => {
                if let Some(l) = loans.dead_of(h) {
                    let killed = l.dead.as_ref().map_or(0, |d| d.0.line);
                    out.push(Conflict {
                        used_at: at.line,
                        holder: h.clone(),
                        path: l.path.clone(),
                        killed_at: killed,
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(h: &str, p: &str, m: bool) -> Event {
        Event::Borrow {
            holder: h.into(),
            path: p.into(),
            mutable: m,
        }
    }
    fn d(h: &str, f: &str) -> Event {
        Event::Derive {
            holder: h.into(),
            from: f.into(),
        }
    }
    fn u(h: &str) -> Event {
        Event::Use(h.into())
    }

    #[test]
    fn overlap_is_prefix_on_field_boundary() {
        assert!(overlaps("a", "a"));
        assert!(overlaps("a", "a.b"));
        assert!(overlaps("a.b", "a"));
        assert!(!overlaps("a", "ab"));
        assert!(!overlaps("a.b", "a.c"));
    }

    // ADR 0003 規則 2: 共有の借用の最後の使用のあとでは、同じ値を排他に借りられる。
    #[test]
    fn exclusive_borrow_is_fine_after_last_use_of_shared() {
        let ev = [b("r", "x", false), u("r"), b("m", "x", true), u("m")];
        assert_eq!(regions(&ev), vec![]);
    }

    // 共有の借用がまだ後で使われるなら、その前の排他の借用は衝突する。
    #[test]
    fn exclusive_borrow_while_shared_is_live_conflicts() {
        let ev = [b("r", "x", false), b("m", "x", true), u("m"), u("r")];
        assert_eq!(
            regions(&ev),
            vec![Conflict {
                used_at: 4,
                holder: "r".into(),
                path: "x".into(),
                killed_at: 2
            }]
        );
    }

    // 共有の借用どうしは重なってよい。読み出しは共有の貸しを使えなくしない。
    #[test]
    fn shared_borrows_and_reads_coexist() {
        let ev = [
            b("r", "x", false),
            b("s", "x", false),
            Event::Read("x".into()),
            u("r"),
            u("s"),
        ];
        assert_eq!(regions(&ev), vec![]);
    }

    // 排他の貸しは、読み出しでも使えなくなる。
    #[test]
    fn read_kills_exclusive_loan() {
        let ev = [b("m", "x", true), Event::Read("x".into()), u("m")];
        assert_eq!(regions(&ev).len(), 1);
    }

    // 書き換えと移動は、共有の貸しも使えなくする。
    #[test]
    fn write_and_move_kill_shared_loan() {
        let w = [b("r", "x", false), Event::Write("x".into()), u("r")];
        let m = [b("r", "x", false), Event::Move("x".into()), u("r")];
        assert_eq!(regions(&w).len(), 1);
        assert_eq!(regions(&m).len(), 1);
    }

    // 別のフィールドは重ならない。親と子は重なる。
    #[test]
    fn disjoint_fields_do_not_conflict() {
        let ok = [b("r", "p.a", true), Event::Write("p.b".into()), u("r")];
        let bad = [b("r", "p.a", true), Event::Write("p".into()), u("r")];
        assert_eq!(regions(&ok), vec![]);
        assert_eq!(regions(&bad).len(), 1);
    }

    // 使えなくなった貸しでも、保持者を使わなければ衝突しない（区間は最後の使用までである）。
    #[test]
    fn dead_loan_without_later_use_is_fine() {
        let ev = [b("r", "x", false), u("r"), Event::Write("x".into())];
        assert_eq!(regions(&ev), vec![]);
    }

    // 規則 3: 返された参照（引き継いだ保持者）の区間は、元の借用と同じ場所を保持する。
    #[test]
    fn derived_holder_keeps_the_original_region() {
        let ev = [
            b("r", "x", false),
            d("ret", "r"),
            Event::Write("x".into()),
            u("ret"),
        ];
        assert_eq!(
            regions(&ev),
            vec![Conflict {
                used_at: 4,
                holder: "ret".into(),
                path: "x".into(),
                killed_at: 3
            }]
        );
    }

    // 返された参照の引き継ぎは推移する。元の保持者を使い終えていても、返された参照が生きている間は続く。
    #[test]
    fn derivation_is_transitive_and_outlives_the_source_holder() {
        let ev = [
            b("a", "x", false),
            d("b", "a"),
            d("c", "b"),
            u("a"),
            Event::Move("x".into()),
            u("c"),
        ];
        let got = regions(&ev);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].holder, "c");
    }

    // 参照の保持者から再借用すると、元の場所も保持する。
    #[test]
    fn reborrow_through_a_holder_keeps_the_root_loan() {
        let ev = [
            b("r", "x", false),
            b("s", "r", false),
            Event::Write("x".into()),
            u("s"),
        ];
        assert_eq!(regions(&ev).len(), 1);
    }

    #[test]
    fn merge_keeps_a_loan_dead_on_either_path() {
        let at = Span { line: 1, col: 1 };
        let mut a = Loans::default();
        a.lend("r", "x", false, None);
        let mut c = a.clone();
        c.touch("x", true, at, "書き換え");
        let m = a.merge(c);
        assert!(m.dead_of("r").is_some());
        assert_eq!(m.held("r"), vec![("x".to_string(), false)]);
    }

    // 移動の判定（`UseAfterMove`）は貸しではなく移動の追跡の仕事。移動は貸しを作らず、
    // 貸しのない保持者を使っても衝突にならない。tests/borrow_pairs.rs の同名の試験の対。
    #[test]
    fn moves_are_not_loans() {
        let ev = [Event::Move("a".into()), u("a"), Event::Read("a".into())];
        assert!(regions(&ev).is_empty());
        let ev = [b("r", "x", false), Event::Move("a".into()), u("r")];
        assert!(regions(&ev).is_empty());
    }
}

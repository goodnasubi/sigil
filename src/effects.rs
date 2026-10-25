//! 効果のカテゴリ表と、検査の設定（ADR 0004・ADR 0006 の条件 4〜6・11）。
//!
//! カテゴリの語彙は表 1 つに集めてあり、コンパイラの他の部分はカテゴリ名を知らない。
//! 表に 1 行足すだけで、宣言・推論・照合・対象の検査が働く（ADR 0006 の条件 4）。

/// カテゴリを使える実行対象。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub bare: bool,
    pub server: bool,
    pub browser: bool,
}

impl Targets {
    /// ブラウザとサーバーで使える（実行環境なしでは使えない）。ADR 0004 の表の「すべて」はこの意味（ADR 0014 決定 3）。
    pub const HOSTED: Targets = Targets {
        bare: false,
        server: true,
        browser: true,
    };
    /// 実行環境なしを含むすべての出力先で使える（`state` だけ。ADR 0014 決定 3）。
    pub const ANY: Targets = Targets {
        bare: true,
        server: true,
        browser: true,
    };
    pub const SERVER: Targets = Targets {
        bare: false,
        server: true,
        browser: false,
    };
    pub const BROWSER: Targets = Targets {
        bare: false,
        server: false,
        browser: true,
    };

    /// 実行対象の語（`bare`・`server`・`browser`）で使えるか。
    pub fn allows(&self, target: &str) -> bool {
        match target {
            "bare" => self.bare,
            "server" => self.server,
            "browser" => self.browser,
            _ => false,
        }
    }
}

/// 効果のカテゴリの表。
#[derive(Debug, Clone)]
pub struct EffectTable {
    cats: Vec<(String, Targets)>,
}

impl EffectTable {
    /// ADR 0004 のカテゴリ（ADR 0007 の `task.*` を含む）。
    pub fn standard() -> EffectTable {
        use Targets as T;
        let rows = [
            ("console", T::HOSTED),
            ("fs.read", T::SERVER),
            ("fs.write", T::SERVER),
            ("net.client", T::HOSTED),
            ("net.server", T::SERVER),
            ("db", T::SERVER),
            ("clock", T::HOSTED),
            ("random", T::HOSTED),
            ("env", T::SERVER),
            ("process", T::SERVER),
            ("dom", T::BROWSER),
            ("browser.storage", T::BROWSER),
            ("state", T::ANY),
            ("foreign", T::HOSTED),
            ("task.spawn", T::HOSTED),
            ("task.suspend", T::HOSTED),
            ("task.share", T::HOSTED),
        ];
        let mut t = EffectTable { cats: vec![] };
        for (n, ts) in rows {
            t.cats.push((n.to_string(), ts));
        }
        t
    }

    /// カテゴリを 1 つ足した表（テストや将来の追加の手続きのため）。
    pub fn with(mut self, name: &str, targets: Targets) -> EffectTable {
        self.cats.retain(|(n, _)| n != name);
        self.cats.push((name.to_string(), targets));
        self
    }

    fn under(&self, name: &str) -> Vec<&(String, Targets)> {
        let prefix = format!("{name}.");
        self.cats
            .iter()
            .filter(|(n, _)| n == name || n.starts_with(&prefix))
            .collect()
    }

    /// `effects` 節に書ける名前か。表にある名前か、下位が 2 つ以上ある上位の名前（`fs`・`net`・`task`）。
    /// `browser` のように下位が 1 つだけの語は書けない（ADR 0008 決定 8）。
    pub fn valid(&self, name: &str) -> bool {
        if self.cats.iter().any(|(n, _)| n == name) {
            return true;
        }
        self.under(name).len() >= 2
    }

    /// 名前が表す葉のカテゴリ（自分と、その下位すべて）。照合はこの集合の比較で行う。
    pub fn atoms(&self, name: &str) -> Vec<String> {
        self.under(name).iter().map(|(n, _)| n.clone()).collect()
    }

    /// 表にある葉のカテゴリが、実行対象 `target` で使えるか。
    pub fn allowed_on(&self, atom: &str, target: &str) -> bool {
        self.cats
            .iter()
            .find(|(n, _)| n == atom)
            .is_none_or(|(_, ts)| ts.allows(target))
    }
}

/// 標準ライブラリが入るまでの、関数の仮の定義（シグネチャと効果）。
///
/// 標準ライブラリ（M1 の次の PR）が入るまで、効果の検査をテストするための差し込み口。
#[derive(Debug, Clone)]
pub struct Primitive {
    /// 例: `"emit(x: Int) -> Int"`。
    pub signature: String,
    /// 例: `["console"]`。
    pub effects: Vec<String>,
}

/// 検査の設定。
#[derive(Debug, Clone)]
pub struct Options {
    pub table: EffectTable,
    pub primitives: Vec<Primitive>,
    /// 型名ごとの Copyable の上書き（ADR0006 の判定表を差し替える差し込み口。名前つきの struct/enum のみ）。
    pub copy_override: Vec<(String, bool)>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            table: EffectTable::standard(),
            primitives: vec![],
            copy_override: vec![],
        }
    }
}

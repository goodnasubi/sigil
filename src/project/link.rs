//! 検査し終えたモジュールを 1 つのプログラムに結ぶ処理（ADR 0021 の 2-2c、ADR 0022 決定 1）。

use std::collections::HashMap;

use crate::ast::*;
use crate::error::{Error, ErrorKind};
use crate::project::decl::item_name;
use crate::project::resolve::Walk;
use crate::project::{Proj, ProjectError, R};

impl Proj<'_> {
    /// 検査し終えたモジュールを 1 つのプログラムに結ぶ（ADR 0021 の 2-2c、ADR 0022 決定 1）。
    /// 入口でないモジュールの関数と型は `<モジュール名>.<名前>` に改め（`.` は利用者の名前に現れないので
    /// 衝突しない）、各モジュールの参照を検査時と同じ解決で、改めた名前に向ける。
    pub(super) fn link(&self, entry: &str) -> R<File> {
        for m in &self.order {
            let (_, f) = &self.mods[m];
            if m != entry && f.module.trust == Trust::Base {
                return Err(ProjectError::Diag {
                    file: self.mods[m].0.clone(),
                    error: Error::new(
                        ErrorKind::Unsupported,
                        f.module.span,
                        format!("入口でないモジュール `{m}` は `trust base` にできない（信頼基盤は入口のモジュールだけ。結合の対象外）"),
                    ),
                });
            }
        }
        let mut items: Vec<Item> = vec![];
        for m in &self.order {
            let (_, f) = &self.mods[m];
            let map: HashMap<String, Result<String, String>> = self.cands[m]
                .picks()
                .into_iter()
                .map(|(n, r)| {
                    let r = r.map(|o| {
                        if o == entry {
                            n.clone()
                        } else {
                            format!("{o}.{n}")
                        }
                    });
                    (n, r)
                })
                .collect();
            let mut decl = HashMap::new();
            if m != entry {
                for it in &f.items {
                    if let Some(n) = item_name(it) {
                        decl.insert(n.to_string(), format!("{m}.{n}"));
                    }
                }
            }
            let mut w = Walk::new(&map, decl);
            for it in &f.items {
                let mut it = it.clone();
                w.item(&mut it);
                items.push(it);
            }
        }
        let (_, ef) = &self.mods[entry];
        Ok(File {
            module: ef.module.clone(),
            uses: vec![],
            items,
        })
    }
}

//! 埋め込み標準モジュールの読み込み（ADR 0025 補足 A-6、PR 3-4h）。
//!
//! 経路は暗黙のみ: `Options::embedded_roots` で名指しされたモジュールと、埋め込みモジュールが `use` する
//! 埋め込みモジュールだけを、レジストリ（`stdlib::embedded`）から読む。利用者のファイルの `use std.…` は
//! 従来どおり `ReservedModuleName`。

use std::path::PathBuf;

use crate::ast::*;
use crate::error::{Error, ErrorKind};
use crate::project::{Proj, ProjectError, R};
use crate::stdlib::embedded;

/// 埋め込みモジュールの診断に使う仮の位置。
pub(super) fn pseudo_path(name: &str) -> PathBuf {
    PathBuf::from(format!("<埋め込み {name}>"))
}

impl Proj<'_> {
    /// 埋め込みモジュール `name` を（まだなら）読んで検査する。
    pub(super) fn visit_embedded(&mut self, name: &str) -> R<()> {
        if self.mods.contains_key(name) {
            return Ok(());
        }
        let path = pseudo_path(name);
        let Some(src) = embedded::lookup(name, &self.opts.embedded_extra) else {
            return Err(ProjectError::Read {
                file: path,
                msg: format!("埋め込みモジュール `{name}` はない"),
            });
        };
        // 標準の名前を定義するので、予約名の検査をしない構文解析を使う。
        let file = crate::parser::parse_std(src).map_err(|error| ProjectError::Diag {
            file: path.clone(),
            error,
        })?;
        if file.module.name.join(".") != name {
            return Err(ProjectError::Diag {
                file: path,
                error: Error::new(
                    ErrorKind::ModulePathMismatch,
                    file.module.span,
                    format!(
                        "埋め込みモジュールの登録名 `{name}` と宣言 `{}` が違う",
                        file.module.name.join(".")
                    ),
                ),
            });
        }
        // 出所の印（`self.embedded`）は、レジストリから読んだモジュールの訪問が終わったときだけ付く。
        self.visit_as(path, file, true)
    }

    /// 暗黙に結ぶ埋め込みモジュール（名指しされたものと `implicit_roots`）を、入口より先に読む。入口の `targets` の全てで使えなければならない
    /// （`use` と同じ規則。取り込む側の出力先のすべてで、取り込まれる側が使える）。
    pub(super) fn visit_embedded_roots(&mut self, entry: &File) -> R<()> {
        // 名指しされたものに、暗黙に結ぶもの（すべてのプログラムに確保器と console。補足 C-2・補足 J）を足す。
        let mut roots = self.opts.embedded_roots.clone();
        for r in embedded::implicit_roots(self.opts) {
            if !roots.iter().any(|x| x == r) {
                roots.push(r.to_string());
            }
        }
        for r in &roots {
            self.visit_embedded(r)?;
            // 確保器と console は処理系が暗黙に結ぶプラットフォーム層で、実体は出力先が与えるため、入口の `targets` との照合の対象ではない
            // （補足 C-2・補足 J）。
            if r == embedded::ALLOC_MODULE
                || r == embedded::CONSOLE_MODULE
                || r == embedded::FS_MODULE
                || r == embedded::ENV_MODULE
                || r == embedded::THREAD_MODULE
                || r == embedded::SCHED_MODULE
            {
                continue;
            }
            let (path, target) = &self.mods[r];
            if let Some(t) = entry
                .module
                .targets
                .iter()
                .find(|t| !target.module.targets.contains(t))
            {
                return Err(ProjectError::Diag {
                    file: path.clone(),
                    error: Error::new(
                        ErrorKind::TargetMismatch,
                        target.module.span,
                        format!(
                            "入口の `targets` に `{t}` があるが、暗黙に結ぶ埋め込みモジュール `{r}` の `targets` にはない"
                        ),
                    ),
                });
            }
        }
        Ok(())
    }
}

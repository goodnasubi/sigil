//! 定数時間検査（ADR 0020・0024・0061）。
//!
//! 検査エンジンは `tests/ct_rules.rs`（標準ライブラリの検証）と `sigil test --ct`（`Sigil.toml` の
//! `[testing.constant_time]` に書かれた外部モジュールの関数の検証）で共通。
//!
//! - `rules`: 静的検査（R1〜R6 とテイント伝播）。
//! - `clif`: CLIF 検査（除算命令・条件分岐の数）。
//! - `manifest`: `Sigil.toml` の解釈。
//! - `stdlib`: 標準ライブラリの対象表。
//! - `run`: `sigil test --ct` の本体。

pub mod clif;
pub mod manifest;
pub mod rules;
pub mod run;
pub mod stdlib;

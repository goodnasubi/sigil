## 概要

<!-- 何を、なぜ変えたか。1 PR 1 つの意図に絞る。 -->

Closes #

## 担当

- レーン: <!-- A1 / A2 / B1〜B5 / C1 / C2 / D1 / D2 / E -->
- エージェント: <!-- claude / gemini / muse / codex -->
- 実行した環境: <!-- local-windows / cloud -->

## 種別

- [ ] コードのみ（仕様に影響しない）
- [ ] 仕様の変更（`sigil spec` の出力・文法・型規則） → 人間のレビューが必要
- [ ] ADR の追加・変更
- [ ] 契約変更（`ast.rs` / `ir.rs`）→ `contract-change` ラベル
- [ ] セキュリティ関連（ADR 0020、CODEOWNERS 対象）

## 仕様の差分

<!-- 仕様に影響する場合、`sigil spec` の出力の変更前後を貼る。影響しない場合は「なし」。 -->

## ADR

<!-- 関連する ADR の番号。新しい決定が要る場合は、先に ADR の PR を出す。 -->

## 確認

- [ ] すべてのコミットの末尾に `Agent:` と `Agent-Env:` のトレーラーがある
- [ ] `origin/main` にリベースした
- [ ] `cargo fmt --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test`
- [ ] 担当レーン外のファイルを変更していない（変更した場合は理由: ）
- [ ] 共有ファイル（`lib.rs`・`error.rs` など）は追加のみ


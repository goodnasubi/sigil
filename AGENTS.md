# AGENTS.md — Sigil の AI エージェント向け作業ルール

このファイルは、Claude Code・Gemini・Meta Muse・Codex など、すべての AI エージェントが最初に読む**単体で完結する**指示書です。
詳細は [docs/PARALLEL_DEV.md](docs/PARALLEL_DEV.md)（運用ルール）と [docs/CHECK_SPLIT_PLAN.md](docs/CHECK_SPLIT_PLAN.md)（`check.rs` 分割計画）にあります。迷ったら、そちらを読んでください。

## プロジェクト

Sigil は、AI がコードを書き、人間は仕様だけをレビューする言語です。処理系は Rust 製です。設計判断は `docs/decisions/` の ADR にあります。

## 基本原則

1. 1 エージェント = 1 作業ツリー = 1 ブランチ = 1 PR。作業ツリーを他のエージェントと共有しない。
2. 担当レーンのファイルだけを変更する（下の表）。レーン外は最小の追加にとどめ、PR に理由を書く。
3. 小さい PR を頻繁に出す。ブランチは 1 日以内に PR にする。
4. 設計判断が要る変更は、ADR の PR を先に出す。ADR と食い違う実装を、黙って通さない。
5. `main` に直接 push しない。必ず PR を経由する。
7. 各サブエージェントは独自のリクエストを実行する。シンプルなサブエージェントには、より低コストなモデルを検討するか、プロンプトを絞り込む。（Claude Code に限らず、サブエージェントを使うすべてのエージェントに適用する）
6. 作者と別のエージェントがレビューする。自分の PR を自分で承認しない。

## 作業開始の手順

1. **バックログの同期（アンロック・自動起票）**:
   - Issue を選ぶ前、および自分の PR がマージされて Issue がクローズされた直後に、バックログ同期スクリプトを実行して後続 Issue の `blocked` 解除と新規起票を行う:
     - Windows (PowerShell): `pwsh -File scripts/sync-issues.ps1`
     - Linux / Cloud (Bash): `bash scripts/sync-issues.sh`
   - **同期の実行者は一意に決める**: その同期で解放・起票される前提 Issue を完了させた PR の作成者だけが実行する。他のエージェントは、同じタイミングでは同期を実行しない（二重起票の防止。2026-11-01 の #138 / #139 重複の再発防止）。
   - 起票は必ずスクリプト経由で行う。スクリプトが使えない環境で手動起票する場合も、本文の末尾に `<!-- backlog-id: <spawn_rules の id> -->` を必ず含める（スクリプトの重複判定はこの目印で行われる。目印のない Issue は重複と検知できない）。
   - それでも同じ `spawn_rules` から Issue が重複した場合は、**番号の小さい方を正**とし、大きい方を作ったエージェントが、正の Issue へのリンクをコメントして自分でクローズする。
2. **タスクの確保（排他ロック）**:
   - 全エージェントが同じ GitHub アカウントを共有するため、**`agent:<agent>` ラベルを排他ロックとして使う**（`<agent>` は `claude` / `gemini` / `muse` / `codex`）。
   - 着手前に `gh issue list --state open` と `gh pr list --state open` を確認する。
   - **すでに `agent:*` ラベルや `blocked` ラベルが付いている Issue、または同じ `lane:*` のオープン PR が存在するレーンの Issue は絶対に取らない**（1 レーンにつき同時に 1 PR まで）。
   - 未着手の Issue を選んだら、即座に自分のラベルを付け、作業ブランチ名をコメントしてロックする:
     ```
     gh issue edit <番号> --add-label "agent:<agent>"
     gh issue comment <番号> --body "着手: `<agent>/<lane>/<番号>-<slug>`"
     ```
   - 2 体が同じ Issue にほぼ同時にラベルを付けてしまった場合は、**着手コメントの投稿時刻が早い方を正**とし、遅い方は自分のラベルを外して手を引く。
   - PR 本文には必ず `Closes #<番号>` を含め、PR マージ時に Issue が自動クローズされるようにする。
3. 最新の `main` から、ブランチ名 `<agent>/<lane>/<issue番号>-<slug>` で作る。
4. ローカルでは、独立した作業ツリーを作る。
   ```
   git fetch origin
   git worktree add <短いパス>/sigil-<agent>-<issue> -b <agent>/<lane>/<issue>-<slug> origin/main
   ```
   - Windows では短いパスを使う（例: `C:\w\sigil-codex-123`）。
   - `CARGO_TARGET_DIR` を作業ツリーごとに分け、`CARGO_BUILD_JOBS` で CPU の使用を抑える。
   - クラウドでは clone 自体が独立した作業ツリーなので、`git worktree` は不要。
5. クラウドでは、**push していない成果は失われる**。こまめにコミットして push する。

## コミットメッセージ

どのエージェントがコミットしたかを、**すべてのコミットの末尾にトレーラー**で記録する。

```
<日本語の件名（1 行）>

<本文（任意）>

Agent: claude
Agent-Model: <モデル名（分かれば）>
Agent-Env: cloud
```

- `Agent:` は必須。値は `claude` / `gemini` / `muse` / `codex`（ブランチ名の `<agent>` と同じ）。
- `Agent-Model:` は任意。分かるときだけ書く。
- `Agent-Env:` は必須。値は `local-windows` / `cloud`。
- トレーラーは本文との間に空行を 1 行あけ、末尾にまとめる。
- 人間が手でコミットする場合は付けない（`Agent:` が無いコミット = 人間）。
- 別のエージェントの作業を引き継いで追加コミットする場合は、**そのコミットを書いたエージェント自身**の名前を書く。
- 履歴を書き換える操作（rebase・squash）では、トレーラーを失わない。squash するときは、含まれる全エージェントの `Agent:` 行を残す。
- 確認: `git log --format='%h %(trailers:key=Agent,valueonly) %s'`

## PR を出す前に必ず通す

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

- `origin/main` にリベースしてから実行する。
- `runtime/core`・`runtime/os` を変更した場合は、それぞれのディレクトリでも同じ 3 コマンドを実行する。
- `scripts/*.sh`（ネイティブ出力の確認など）は bash 用。Windows では Git Bash か WSL でのみ動く。ネイティブ出力は Linux x86-64 のみなので、Windows ではその確認を CI に任せる。

## レーン（担当ファイル）

| レーン | 担当ファイル |
|---|---|
| A1 構文 | `lexer`, `parser`, `ast` の追加 |
| A2 整形・仕様 | `fmt`, `spec`, `json` |
| B1 型・解決 | `check/{types,unify,resolve,generics}` |
| B2 トレイト | `check/{traits,register,items}` |
| B3 所有権 | `check/ownership`, `borrow` |
| B4 式・文 | `check/{stmt,expr,pattern,closure,call}`, `exhaustive` |
| B5 効果・契約 | `check/{effects,fn_check}`, `effects`, `verify` |
| C1 SIR | `lower`, `ir`, `ir_eval` |
| C2 ネイティブ | `codegen/**`, `native`, `runtime/**` |
| D1 ツール | `lsp/**`, `project`, `modpath`, `main` |
| D2 標準ライブラリ | `stdlib`, `runtime/core`, `runtime/os` |
| E 文書 | `docs/**`, `examples/**`, `tests/**`（新規ファイルのみ） |

## 触ってはいけない・特別な扱いのファイル

| ファイル | 規則 |
|---|---|
| `.github/workflows/**`, `.github/CODEOWNERS` | 人間のみ変更する |
| `src/limits.rs`, `tests/ct_rules.rs` | CODEOWNERS により人間のみ |
| `src/ast.rs`, `src/ir.rs` | 契約変更。変更だけの小さい PR を単独で出し、`contract-change` ラベルを付ける |
| `Cargo.toml`, `Cargo.lock` | 依存の追加は ADR が必要。単独の PR にする |
| `src/lib.rs`, `src/error.rs` | 追加のみ。1 エントリ 1 行、アルファベット順、他の変更を混ぜない |

- テストは原則、新しいファイル（`tests/<機能>.rs`）を足す。既存のテストファイルへの追記は、そのレーンの担当だけが行う。
- 依存クレートは足さない方針（JSON も自前実装）。

## レビューとマージ

| 変更の種類 | 人間のレビュー | マージの条件 |
|---|---|---|
| コードのみ（仕様に影響しない） | 不要 | CI が緑 + 別系統のエージェントの承認 |
| 仕様の変更（`sigil spec` の出力・文法・型規則） | **必須** | 人間の承認 |
| ADR の追加・変更 | 不要 | CI が緑 + 別系統のエージェントの承認 |
| セキュリティ関連（ADR 0020、CODEOWNERS 対象） | **必須** | CODEOWNERS の承認 |

- 仕様に影響するか迷う PR は、**人間のレビューに回す**。
- PR の説明に、仕様の差分を `sigil spec` の出力で貼る。
- PR の説明に、実行した環境（`local-windows` / `cloud`）を書く。
- マージ順: 契約変更 → ADR → 実装 → サンプル・テスト。

## ADR

- ADR の PR は ADR だけにする（実装を混ぜない）。
- 番号は、`docs/decisions/README.md` に「予約」の行を足して確保する。他の PR と番号を重ねない。
- 実装中に ADR との食い違いを見つけたら、実装を止めて ADR の修正 PR を出す。

## 困ったとき

- 他のレーンのファイルを触る必要がある → その Issue にコメントして依頼する。
- コンフリクト → 先にマージされた側を優先し、後の側がリベースして解消する。
- CI が不安定 → 再実行は 1 回まで。再現するなら Issue を作る。
- 作業を引き継ぐ → ブランチを push し、ブランチ名を Issue に書く。
- 判断できない → 実装せず、Issue または PR にコメントして人間に確認する。

## 文書の言語

README・ADR・コメント・コミットメッセージは日本語で書く（既存の慣習に従う）。

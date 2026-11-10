# AGENTS.md — Sigil の AI エージェント向け作業ルール

すべての AI エージェント（Claude Code・Gemini・Meta Muse・Codex 等）が最初に読む、単体完結の指示書。
詳細は [docs/PARALLEL_DEV.md](docs/PARALLEL_DEV.md)（運用）と [docs/CHECK_SPLIT_PLAN.md](docs/CHECK_SPLIT_PLAN.md)（`check.rs` 分割計画）。迷ったらそちらを読む。

## プロジェクト

Sigil は、AI がコードを書き、人間は仕様だけをレビューする言語。処理系は Rust 製。設計判断は `docs/decisions/` の ADR にある。

## 基本原則

1. 1 エージェント = 1 作業ツリー = 1 ブランチ = 1 PR。作業ツリーを共有しない。
2. 変更は担当レーン内だけ。レーン外は最小限とし、理由を PR に書く。
3. 小さい PR を頻繁に出す。ブランチは 1 日以内に PR にする。
4. 設計判断が要る変更は ADR の PR を先に出す。ADR と食い違う実装を黙って通さない。
5. `main` へ直接 push しない。必ず PR を経由する。
6. レビューは作者と別のエージェントが行う。自分の PR を自分で承認しない。
7. サブエージェントには各自の依頼を実行させる。単純な作業は低コストなモデルか絞ったプロンプトを検討する（サブエージェントを使う全エージェントに適用。Claude Code に限らない）。

## 作業開始の手順

1. **バックログ同期（アンロック・自動起票）**
   - 時機: Issue を選ぶ前と、自分の PR がマージされ Issue がクローズされた直後。後続 Issue の `blocked` 解除と新規起票を行う。
     - Windows (PowerShell): `pwsh -File scripts/sync-issues.ps1`
     - Linux / Cloud (Bash): `bash scripts/sync-issues.sh`
     - `--dry-run` は書き込まず内容だけを表示（まず `--dry-run` で確かめるとよい）。
   - 実行者は一意: その同期で解放・起票される前提 Issue を完了させた PR の作成者だけ。他者は同じタイミングで実行しない。
   - 起票は必ずスクリプト経由。使えない環境で手動起票する場合も、本文末尾に `<!-- backlog-id: <spawn_rules の id> -->` を必ず含める。
   - 同じ `spawn_rules` から重複したら、番号の小さい方を正とする。大きい方を作ったエージェントが、正へのリンクをコメントして自分でクローズする。
2. **タスク確保（排他ロック）**
   - `agent:<agent>` ラベルを排他ロックに使う（`<agent>` は `claude` / `gemini` / `muse` / `codex`）。
   - 着手前に `gh issue list --state open` と `gh pr list --state open` を確認する。
   - `agent:*` か `blocked` の付いた Issue、および同じ `lane:*` のオープン PR があるレーンの Issue は絶対に取らない（1 レーン同時に 1 PR まで）。
   - 未着手の Issue を選んだら、即座に自分のラベルを付け、ブランチ名をコメントしてロックする:
     ```
     gh issue edit <番号> --add-label "agent:<agent>"
     gh issue comment <番号> --body "着手: `<agent>/<lane>/<番号>-<slug>`"
     ```
   - 2 体がほぼ同時に付けたら、着手コメントの投稿時刻が早い方を正とし、遅い方はラベルを外して手を引く。
   - PR 本文に `Closes #<番号>` を必ず含める。
3. ブランチは最新の `main` から、名前 `<agent>/<lane>/<issue番号>-<slug>` で作る。
4. ローカルでは独立した作業ツリーを作る。
   ```
   git fetch origin
   git worktree add <短いパス>/sigil-<agent>-<issue> -b <agent>/<lane>/<issue>-<slug> origin/main
   ```
   - Windows では短いパスを使う（例: `C:\w\sigil-codex-123`）。
   - `CARGO_TARGET_DIR` は作業ツリーごとに分け、`CARGO_BUILD_JOBS` で CPU 使用を抑える。
   - クラウドでは `git worktree` は不要。
5. クラウドでは、こまめにコミットして push する。

## コミットメッセージ

全コミットの末尾に、作者エージェントをトレーラーで記録する。

```
<日本語の件名（1 行）>

<本文（任意）>

Agent: claude
Agent-Model: <モデル名（分かれば）>
Agent-Env: cloud
```

- `Agent:` は必須。値は `claude` / `gemini` / `muse` / `codex`（ブランチ名の `<agent>` と同じ）。
- `Agent-Model:` は任意（分かるときだけ）。
- `Agent-Env:` は必須。値は `local-windows` / `cloud`。
- トレーラーは本文と空行 1 行をあけ、末尾にまとめる。
- 人間の手作業コミットには付けない（`Agent:` が無いコミット = 人間）。
- 他のエージェントから引き継いで追加コミットする場合は、そのコミットを書いたエージェント自身の名前を書く。
- rebase・squash でトレーラーを失わない。squash では含まれる全エージェントの `Agent:` 行を残す。
- 確認: `git log --format='%h %(trailers:key=Agent,valueonly) %s'`

## PR を出す前の確認（ローカルは最小限、全量は CI）

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --test <変更範囲のテスト>
```

- 全量の `cargo test` はローカルで回さない。CI に任せる。
- `origin/main` にリベースしてから実行する。
- `runtime/core`・`runtime/os` を変更した場合は各ディレクトリでも fmt と clippy を実行する（テストは変更範囲だけ）。
- ツールチェーンが無い環境ではローカル確認を省略してよい。その場合は PR に「ローカル確認なし・CI に委任」と書く。
- `scripts/*.sh` は bash 用で、Windows では Git Bash か WSL でのみ動く。ネイティブ出力は Linux x86-64 のみ。Windows ではその確認を CI に任せる。

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
| `src/limits.rs` | CODEOWNERS により人間のみ |
| `src/ast.rs`, `src/ir.rs` | 契約変更。変更だけの小さい PR を単独で出し、`contract-change` ラベルを付ける |
| `Cargo.toml`, `Cargo.lock` | 依存の追加は ADR が必要。単独の PR にする |
| `src/lib.rs`, `src/error.rs` | 追加のみ。1 エントリ 1 行、アルファベット順、他の変更を混ぜない |

- `tests/ct_rules.rs` は CODEOWNERS の対象外。通常のテストファイルとして扱い、新規作成・変更とも通常の PR 手続きによる。
- テストは原則、新しいファイル（`tests/<機能>.rs`）を足す。既存テストファイルへの追記はそのレーンの担当だけ。
- 依存クレートは足さない方針（JSON も自前実装）。

## レビューとマージ

| 変更の種類 | 人間のレビュー | マージの条件 |
|---|---|---|
| コードのみ（仕様に影響しない） | 不要 | CI が緑 + 別系統のエージェントの承認 |
| 仕様の変更（`sigil spec` の出力・文法・型規則） | **必須** | 人間の承認 |
| ADR の追加・変更 | 不要 | CI が緑 + 別系統のエージェントの承認 |
| セキュリティ関連（ADR 0020、CODEOWNERS 対象） | **必須** | CODEOWNERS の承認 |

- 仕様に影響するか迷う PR は人間のレビューに回す。
- PR の説明に、仕様の差分を `sigil spec` の出力で貼る。
- PR の説明に、実行した環境（`local-windows` / `cloud`）を書く。
- マージ順: 契約変更 → ADR → 実装 → サンプル・テスト。

## ADR

- ADR の PR は ADR だけにする（実装を混ぜない）。
- 番号は `docs/decisions/README.md` に「予約」の行を足して確保する。他の PR と重ねない。
- 実装中に ADR との食い違いを見つけたら、実装を止めて ADR の修正 PR を出す。

## トークンの節約

- GitHub の Issue・PR・コメントの本文を会話や報告に再掲しない。報告は URL と要点だけ、短く。
- 通知（CI 完了、レビュー開始、ready 化など）への応答は、結論と人間の判断が要る点だけ。同じ内容や前回の報告を繰り返さない。
- 文章（報告・コメント・PR 本文・ADR）は端的に。前置き・繰り返し・言い換えを省き、結論と判断が要る点から書く。
- `@codex review` 等の依頼は、同じコメントが投稿済みでないか先に確かめ、重複投稿しない。
- 出力を絞る:
  - `gh api` は `--jq` で必要な項目だけ取る。
  - 長いログ・テスト出力はファイルに書き、末尾か失敗行だけ読む。
  - ファイルは grep で位置を絞り、行範囲を指定して必要な部分だけ読む。同じファイルを繰り返し全文読まない。
- 長いテスト・ビルドはバックグラウンドで実行し、結果をファイルに出す。待つ間に余計なポーリングや確認をしない。
- サブエージェントには範囲を絞った自己完結の指示を渡す:
  - 実務作業（実装・テスト・調査・PR 作成・ADR 起草など）はサブエージェントに任せ、メインセッションは調整役に徹する（依頼・結果確認・判断が要る点の報告のみ）。
  - モデルとエフォートは作業に応じて選ぶ。単純・機械的な作業は軽いモデルと低いエフォート、設計判断や競合を伴う実装・レビュー対応は強いモデルと高いエフォート。
  - 再利用は関係のある続きの作業だけ。関係のない作業では再利用しない。
  - 報告は結果・SHA・チェック結果・判断が要る点に絞らせる。
- 作業ツリーの未コミット変更への Stop hook 等の定型警告には、同じ説明を繰り返さず「進行中の作業」と一言で済ませてよい。原因が変わったときだけ説明する。

## 困ったとき

- 他のレーンのファイルを触る必要がある → その Issue にコメントして依頼する。
- コンフリクト → 先にマージされた側を優先し、後の側がリベースして解消する。
- CI が不安定 → 再実行は 1 回まで。再現するなら Issue を作る。
- 作業を引き継ぐ → ブランチを push し、ブランチ名を Issue に書く。
- 判断できない → 実装せず、Issue または PR にコメントして人間に確認する。

## 文書の言語

- README・ADR・コメント・コミットメッセージは日本語で書く。
- GitHub へ投稿するコメント（Issue・PR のコメント、レビューへの返信）と会話での報告も日本語で書く。
- 日付・時刻（ADR の日付・承認日、Issue・PR の記録、報告・コメントの時刻）は日本時間（JST）で書く。UTC は JST に直して読む（例: UTC 15:53 は翌日 00:53 JST）。

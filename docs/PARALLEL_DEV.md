# 複数 AI エージェントの並行開発 運用ルール（草案 v0.1）

対象エージェント: Claude Code / Gemini / Meta Muse / Codex。
人間のレビューは **仕様（`sigil spec` の出力・言語仕様）だけ**。コードの正しさは CI と別系統のエージェントによるレビューで担保する。
`check.rs` の分割計画は [CHECK_SPLIT_PLAN.md](CHECK_SPLIT_PLAN.md) を参照。

## 1. 基本原則

1. **1 エージェント = 1 作業ツリー = 1 ブランチ = 1 PR**。作業ツリーは共有しない。
2. **担当ファイルで衝突を避ける**（§3 のレーン表）。レーン外のファイルは「最小の追加」だけにする。
3. **小さい PR を `main` へ頻繁に**。ブランチは 1 日以内に PR にし、長寿命ブランチを作らない。
4. **設計判断は ADR が先、実装が後**。ADR が `main` に入るまで、その判断に依存する実装 PR はマージしない。
5. **作者と別系統のエージェントがレビューする**（§6）。
6. **`main` は常に緑**。CI が通らない PR はマージしない。

## 2. 全エージェント共通の指示ファイル

エージェントごとに読むファイルが異なるため、**本体は `AGENTS.md` に 1 つだけ**置き、他は薄い参照にする。

| ファイル | 読むエージェント | 中身 |
|---|---|---|
| `AGENTS.md` | Codex（標準）、他のエージェントも参照 | 本書の要約と必須コマンド（本体） |
| `CLAUDE.md` | Claude Code | `AGENTS.md` を読む旨の 1〜3 行 |
| `GEMINI.md` | Gemini | 同上 |
| Meta Muse | （要確認） | 読み込む指示ファイルの仕様を確認し、同様に `AGENTS.md` へ誘導する |

> [!NOTE]
> Meta Muse の指示ファイルの名前と読み込み方式は、本書の作成時点で確認できていません。使い始める前に確認してください。確認できるまでは、起動時のプロンプトで `AGENTS.md` を読むよう指示します。

`AGENTS.md` に必ず書く内容: 担当レーンの確認、作業開始手順（§5）、PR 前の必須コマンド（`cargo fmt --check`・`cargo clippy --all-targets -- -D warnings`・`cargo test`）、触ってはいけないファイル（§4）。

## 3. レーン（担当ファイル）

**同じファイルを同時に 2 つのレーンが触らない**ことが目的（`src/check/` の分割は [CHECK_SPLIT_PLAN.md](CHECK_SPLIT_PLAN.md) により完了済み）。

| レーン | 主な担当ファイル | 内容 |
|---|---|---|
| A1 構文 | `lexer`, `parser`, `ast` の追加 | 字句・構文解析 |
| A2 整形・仕様 | `fmt`, `spec`, `json` | `sigil fmt`・`sigil spec` |
| B1 型・解決 | `check/{types,unify,resolve,generics}` | 型推論・名前解決 |
| B2 トレイト | `check/{traits,register,items}` | トレイト・impl・アイテムの検査 |
| B3 所有権 | `check/ownership`, `borrow` | 移動・借用 |
| B4 式・文 | `check/{stmt,expr,pattern,closure,call}`, `exhaustive` | 式・文・パターンの検査 |
| B5 効果・契約 | `check/{effects,fn_check}`, `effects`, `verify` | 効果・契約の検査 |
| C1 SIR | `lower`, `ir`, `ir_eval` | 低レベル化・評価器 |
| C2 ネイティブ | `codegen/**`, `native`, `runtime/**` | Cranelift・ランタイム |
| D1 ツール | `lsp/**`, `project`, `modpath`, `main` | LSP・プロジェクト・CLI |
| D2 標準ライブラリ | `stdlib`, `runtime/core`, `runtime/os` | 標準ライブラリ |
| E 文書 | `docs/**`, `examples/**`, `tests/**`（新規ファイルのみ） | ADR・サンプル・テスト |

- **目安**: 同時に 10 体前後まで並行して動かせる。**増やす条件は「今のレーンで衝突が週に 1 回未満」**。
- **テストは原則「新規ファイルを足す」**（`tests/<機能>.rs`）。既存テストファイルへの追記は、そのレーンの担当だけが行う。

## 4. 共有ファイル（衝突しやすい）の規則

次のファイルは複数レーンが触るため、**追加のみ・1 エントリ 1 行・アルファベット順**を守り、他の変更を混ぜない。

| ファイル | 規則 |
|---|---|
| `src/lib.rs` | `pub mod` の行の追加だけ |
| `src/error.rs`（`ErrorKind`） | 末尾ではなく、所定のグループの末尾に 1 行ずつ追加 |
| `src/ast.rs` | **契約変更**（§7）。レーン A1 の許可がある PR だけ |
| `src/ir.rs` | **契約変更**（§7）。レーン C1 の許可がある PR だけ |
| `Cargo.toml` / `Cargo.lock` | 依存の追加は ADR が必要（自前実装の方針）。単独の PR にする |
| `.github/workflows/**`, `.github/CODEOWNERS` | 人間のみ変更（ADR 0020） |
| `src/limits.rs`, `tests/ct_rules.rs` | CODEOWNERS により人間のみ |

## 5. 作業の流れ

1. **バックログの同期（アンロック・自動起票）**:
   - [docs/BACKLOG.json](BACKLOG.json) に Issue 間の依存関係（`unblock_rules`）と、前提完了時に自動起票する後続タスク定義（`spawn_rules`）を管理している。
   - 着手前および PR マージ直後に `pwsh -File scripts/sync-issues.ps1`（Windows）または `bash scripts/sync-issues.sh`（Linux / Cloud）を実行すると、前提が `CLOSED` になった Issue の `blocked` ラベル解除と、次段階の Issue の自動作成が行われる。
2. **タスクを取る（排他ロック）**:
   - 全エージェントが同一の GitHub アカウントで動く場合、`assignee` だけではどのエージェントが担当しているか判別できないため、**`agent:<agent>` ラベル（`agent:claude` / `agent:gemini` / `agent:muse` / `agent:codex`）を排他ロックとして使う**。
   - 着手前に `gh issue list --state open` と `gh pr list --state open` を確認し、**すでに `agent:*` ラベルや `blocked` ラベルが付いている Issue、または同じ `lane:*` でオープン PR が出ているレーンの Issue は取らない**（1 レーン同時 1 PR 原則）。
   - 対象 Issue を決めたら `gh issue edit <番号> --add-label "agent:<agent>"` を実行し、ブランチ名を Issue にコメントしてから作業を始める。
3. **作業ツリーを作る**:
   ```
   git fetch origin
   git worktree add ../sigil-wt/<agent>-<issue> -b <agent>/<lane>/<issue>-<slug> origin/main
   ```
   - `<agent>` は `claude` / `gemini` / `muse` / `codex`。
   - ビルドの競合を避けるため、作業ツリーごとに `CARGO_TARGET_DIR` を分ける（例: `../sigil-wt/target-<agent>-<issue>`）。
4. **実装**: レーン内のファイルだけを変更する。仕様や ADR に影響する変更は §8 に従う。
5. **PR 前**: `origin/main` にリベースし、`cargo fmt --check`・`cargo clippy --all-targets -- -D warnings`・`cargo test` を通す。
6. **PR を出す**: テンプレートの項目（レーン・`Closes #<番号>`・ADR の要否・仕様の差分）を埋める。
7. **レビューを受ける**（§6）。マージ後は `scripts/sync-issues.ps1`（または `.sh`）で後続 Issue を解放・補充し、作業ツリーとブランチを削除する（`git worktree remove`、`git push origin --delete`）。
8. **マージ順**: 契約変更 → ADR → 実装 → サンプル・テスト。同じレーンの PR は番号の若い順。

### 5.1 コミットへのエージェントの記録

すべてのコミットの末尾に、次のトレーラーを付ける（形式の詳細は `AGENTS.md`）。

```
Agent: claude            # claude / gemini / muse / codex（必須）
Agent-Model: <モデル名>   # 任意
Agent-Env: cloud         # local-windows / cloud（必須）
```

- 目的: 誰が書いたかの追跡、§6 の「作者と別系統」のレビューの検証、環境固有の不具合の切り分け。
- `Agent:` が無いコミットは人間のコミットとして扱う。
- 集計・確認: `git log --format='%h %(trailers:key=Agent,valueonly) %s'`
- **Git の author も合わせて分けるのが望ましい**（任意）。エージェントごとに `git config user.name "<agent> (AI)"` と専用のメールアドレスを、作業ツリーのローカル設定に入れる。GitHub の画面でも区別できる。ただし author は設定を忘れやすいので、**トレーラーを正とする**。
- **squash マージ**では、GitHub の既定ではコミットメッセージのトレーラーが残らないことがある。マージ方法は「Create a merge commit」か「Rebase and merge」を使うか、squash の場合はマージ時のメッセージに `Agent:` 行を残す。

> [!NOTE]
> トレーラーの有無を CI で機械的に検査するジョブは未実装です（`.github/workflows/**` は人間のみ変更できるため、あなたの承認が必要）。実装するまでは、レビューするエージェントが確認します。

## 6. レビュー（人間は仕様のみ）

| 変更の種類 | 人間 | レビューするエージェント | マージ |
|---|---|---|---|
| コードのみ（仕様に影響なし） | 不要 | 作者と別系統の 1 体 | CI 緑 + エージェントの承認で自動 |
| 仕様の変更（`sigil spec` の出力・文法・型規則） | **必須** | 別系統の 1 体 + 仕様の差分の検証 | 人間の承認後 |
| ADR の追加・変更 | 不要 | 別系統の 1 体（矛盾の確認） | CI 緑 + エージェントの承認で自動 |
| セキュリティ関連（ADR 0020、`CODEOWNERS` 対象） | **必須** | 別系統の 1 体 | CODEOWNERS の承認後 |

- 「別系統」とは、作者と異なるエージェント（Claude Code が書いたら Gemini・Codex・Muse のいずれかがレビュー）。
- **仕様の変更の判定は機械的に行う**: `tests/spec_golden/` のように、`sigil spec` の出力を固定したテスト（ゴールデンファイル）を用意し、その差分が出た PR に `needs-human` ラベルを自動で付ける（CI のジョブ）。エージェントの自己申告に頼らない。
- 人間のレビュー対象を減らすため、PR の説明には「仕様の差分」を `sigil spec` の出力で貼る。人間はそれだけを見る。

> [!IMPORTANT]
> ゴールデンファイルの CI ジョブと `needs-human` ラベルの自動付与は、まだ存在しません。実装するまでは「仕様に影響するかどうか迷う PR は人間に回す」を既定にします。

## 7. 契約変更（ast / ir）

レーンをまたぐ境界は `ast.rs`（A→B）と `ir.rs`（B→C）。

1. 変更が必要なレーンが、**契約変更だけの小さい PR**（実装を含めない）を出す。
2. 他のレーンへ通知する（Issue のコメント、またはラベル `contract-change`）。
3. マージ後、他のレーンは `origin/main` をリベースしてから作業を続ける。
4. 追加は後方互換（新しい列挙子・新しい欄の追加）を優先し、既存の意味の変更・削除は ADR に書く。

## 8. ADR の運用

- **番号の衝突を避ける**: ADR の番号は、Issue に紐づけて事前に割り当てる（`docs/decisions/README.md` の表に「予約」の行を足す PR を先に出す）。
- ADR の PR は **ADR だけ**（実装を混ぜない）。マージ後に実装 PR はその後に出す。
- 実装中に ADR と食い違う点を見つけたら、実装を止めて ADR の修正 PR を出す。**コードの側で黙って辻褄を合わせない**。

## 9. 失敗時の対処

| 状況 | 対処 |
|---|---|
| コンフリクト | 先にマージされた側を優先し、後の側がリベースして解消する。大きく壊れたら PR を作り直す |
| 他のレーンのファイルを触る必要がある | その Issue にコメントして依頼する。最小変更の場合は PR に理由を書く |
| CI が不安定（fuzz、ネイティブ出力） | 再実行は 1 回まで。再現するなら Issue を作って該当レーンが直す |
| エージェントが止まった・暴走した | Issue の割り当てを外し、ブランチを削除して取り直す |
| 2 体が同じタスクを始めた | 先に PR を出した方を残し、他方は破棄する |

## 10. 導入手順（この順で進める）

1. 本書と [CHECK_SPLIT_PLAN.md](CHECK_SPLIT_PLAN.md) の人間のレビュー（仕様に関する運用なので）。
2. `AGENTS.md`・`CLAUDE.md`・`GEMINI.md`、PR テンプレート、Issue ラベル（`lane:*`、`agent:*`、`contract-change`、`needs-human`）を追加する。
3. 不要になった追跡済みの古いリモートブランチ（マージ済みの 80 本以上）を整理する（人間の確認の後に実施）。
4. `check.rs` の分割（凍結期間あり）。この間、レーン B は新しい作業を始めない。
5. 分割後、レーン B を増やし、同時に動かすエージェントを段階的に増やす。
6. ゴールデンファイルの CI ジョブを追加する（§6）。

## 11. 未決事項

- Meta Muse の指示ファイルの仕様と、CLI からの起動方法。
- 各エージェントの実行環境（ローカルの Windows か、CI やクラウドか）。ローカル並行は CPU とディスク（`target/` が大きい）で上限が決まる。
- GitHub のマージキュー・ブランチ保護を有効にするか（`main` への直接 push の禁止を含む）。
- Issue の運用で足りるか、`docs/TASKS.md` のような一覧が必要か。

# 司令塔方式の導入手順（ローカル Claude Code + GitLab）

> **使い方**: 導入したいリポジトリで Claude Code を起動し、このファイルを渡して
> 「commander-workflow-gitlab.md をみて、導入して」と頼む。以降の作業は Claude が行う。

## 0. Claude への導入指示

このファイルを渡されて導入を頼まれたら、次の手順を順に実行すること。途中で人間に質問するのは、
手順の中で「人間に確認する」と書いた箇所だけにする。

1. **導入先の種類を判定する**（導入先 = Claude Code を起動したディレクトリ。以下「ルート」）
   - **種類 A**: ルートが社内 GitLab のリポジトリそのもの
     （`git rev-parse --show-toplevel` がルートを返し、`git remote get-url origin` が社内 GitLab を指す）。
   - **種類 B**: それ以外。ルートが git 管理されていない、GitLab 以外のリモートを持つ、
     または配下に複数のリポジトリを置いた作業用ディレクトリである場合。
     配下の GitLab リポジトリを `find . -maxdepth 3 -name .git` で探し、一覧にしておく。
   - どちらの場合も `glab auth status` を確かめる。失敗したら、社内 GitLab への `glab auth login` を
     人間に頼んで止まる（種類 B で配下に GitLab リポジトリが一つもなければ、この確認は飛ばす）。
   - 種類 A なら既定ブランチ名を調べる（`git symbolic-ref refs/remotes/origin/HEAD`）。
     以下の `main` はその名前に読み替える。種類 B では、作業のたびに対象リポジトリの既定ブランチを調べる。
2. **作業ブランチを作る**: 種類 A のときだけ、既定ブランチの最新から `chore/commander-workflow` を作る。
   種類 B ではルートに直接ファイルを置く。
3. **ファイルを置く**（すでにあるファイルは上書きせず、内容を統合する）
   - 3.1 の 4 つの定義を `.claude/agents/` に作る。
   - 3.2 の内容を `CLAUDE.md` の末尾に追記する（なければ作る）。同じ見出しがすでにあれば重ねて書かない。
   - 3.3 の `allow` / `deny` を `.claude/settings.json` にマージする（既存の項目は消さない）。
   - 種類 A では、リポジトリにある lint・型検査・テストのコマンド（package.json、Makefile、pyproject.toml など）を調べ、
     `CLAUDE.md` の「MR の流れ」手順 5 に具体的なコマンドとして書き込む。見つからなければ人間に確認する。
     種類 B では書き込まず、作業のたびに対象リポジトリで調べる（手順 5 の記述のまま）。
4. **動作を確かめる**
   - reviewer エージェントを起動し、`/code-review` を呼び出せるか試す。呼び出せなければ、`reviewer.md` から
     `/code-review` の行と `Skill` を外し、`CLAUDE.md` の「MR の流れ」手順 4 を
     「司令塔が `/code-review` を実行し、結果を reviewer に渡す」に改める。
5. **MR を作る**（種類 A のときだけ）: コミットして push し、`glab mr create` で MR を作る。
   この MR は導入そのものなので、自分ではマージせず、人間のマージを待つ。
   種類 B では MR を作らず、置いたファイルの一覧を報告する。
6. **人間に報告する**（5 行以内）: 種類（A / B）、MR の URL またはファイルの一覧、手順 4 の結果、人間が GitLab の画面で行う設定（3.4）の一覧。

## 1. 何をするか

- **司令塔**（メインのセッション）は、作業の依頼、自己レビューの依頼、マージだけを行う。自分では設計も実装もしない。
- 設計や実装は **子エージェント**（サブエージェント）に任せる。子エージェントは作業ごとに新しく起動し、終わったら破棄する。
- 目的は **トークン（費用）を抑えること**。
  - 司令塔の会話は長くなるが、子は必要な文書だけを読んで始めるので軽い。
  - 重い読み書きを司令塔の外で行うことで、長い履歴を何度も読み直す費用を避けられる。

クラウドのセッションや新規セッションは使わない（モデルとエフォートを確実に指定できるのは、サブエージェントの起動時だけであるため）。子エージェントは、同じ PC 上の Claude Code が起動するサブエージェントとして動かす。並行して作業させる場合は、子ごとに git worktree（作業用の別ディレクトリ）を割り当てる。

## 2. 役割とモデル

| 役割 | モデル | エフォート | 担当する作業 |
|---|---|---|---|
| 司令塔 | Sonnet | low | 依頼文の作成、自己レビューの依頼、マージ |
| 設計 | Opus | medium | 設計判断（ADR）、方式の比較 |
| 実装 | Sonnet | medium | コードとテストの作成 |
| 機械的な作業 | Haiku | low | 表記の統一、リンクの修正、単純な置換 |
| 自己レビュー | Sonnet | medium | MR の差分レビュー（実装した子とは別に起動する） |

運用の規則:

- 同時に動かす子は **最大 2 つ**。扱うファイルが重ならないように作業を分ける。
- 司令塔の会話が長くなったら `/compact` で圧縮する。モデルを切り替える前に圧縮すると、切り替え後の読み直しが安く済む。

## 3. セットアップ

### 3.1 子エージェントの定義

リポジトリに `.claude/agents/` を作り、次の 4 つのファイルを置く。チームで共有できるよう、このディレクトリもコミットする。

`.claude/agents/designer.md`

```markdown
---
name: designer
description: 設計判断（ADR）を書く。方式の比較と決定を docs/decisions/ に記録する。
model: opus
effort: medium
---
あなたは設計担当です。依頼された論点について、選択肢・比較・決定・影響を ADR にまとめます。
- 既存の ADR と矛盾させない。矛盾する場合は、改訂する箇所を明記する。
- 作業ブランチを作り、コミットし、push して、MR を作成する（手順は CLAUDE.md の「MR の流れ」に従う）。
- 最後に、MR の URL と要点を 3 行以内で報告する。
```

`.claude/agents/implementer.md`

```markdown
---
name: implementer
description: 仕様や ADR に沿ってコードとテストを書く。
model: sonnet
effort: medium
---
あなたは実装担当です。
- 変更は依頼された範囲に限る。
- push する前に、lint・型検査・テストをローカルで実行し、すべて通ることを確かめる。
- 作業ブランチを作り、コミットし、push して、MR を作成する（手順は CLAUDE.md の「MR の流れ」に従う）。
- 最後に、MR の URL・テスト結果・残った課題を 3 行以内で報告する。
```

`.claude/agents/chore.md`

```markdown
---
name: chore
description: 表記の統一やリンクの修正などの機械的な作業。判断を要しないもの。
model: haiku
effort: low
---
あなたは機械的な作業の担当です。指示どおりに変更し、判断が必要になったら作業を止めて報告します。
MR の作成は CLAUDE.md の「MR の流れ」に従います。報告は 2 行以内にします。
```

`.claude/agents/reviewer.md`

```markdown
---
name: reviewer
description: MR の差分をレビューする。実装した子とは別に起動する。
model: sonnet
effort: medium
tools: Bash, Read, Grep, Glob, Skill
---
あなたはレビュー担当です。
まず MR のブランチを対象に、Claude Code 組み込みの `/code-review` を実行します（例: `/code-review medium <ブランチ名>`）。
続けて `glab mr diff <番号>` で差分を読み、`/code-review` の指摘も含めて次の観点で確かめます。
1. 正しさ（バグ・境界条件・エラー処理）
2. 依頼と仕様（ADR）に合っているか
3. テストが変更内容を確かめているか
4. セキュリティ（秘密情報の混入・入力検証）

結果は「OK」か「要修正」のどちらかで返し、要修正なら該当箇所（ファイル:行）と直し方を箇条書きにします。
コードは書き換えません。
```

### 3.2 CLAUDE.md に追記する内容

リポジトリ直下の `CLAUDE.md` に、次の内容を追記する。

```markdown
## 司令塔方式

- メインのセッションは司令塔として動く。設計・実装は自分で行わず、サブエージェント
  （designer / implementer / chore）に依頼する。
- 同時に動かすサブエージェントは最大 2 つ。並行させる場合は isolation: worktree で起動する。
- 子の作業は **新規セッションではなく、必ずサブエージェント（Agent ツール）で行う**。
  起動のたびに、`subagent_type`・`model`・`effort` の 3 つを **明示して渡す**（定義ファイルの既定値に頼らない）。
  対応: designer = opus / medium、implementer = sonnet / medium、chore = haiku / low、reviewer = sonnet / medium。
  指定を省くと司令塔のモデルを引き継ぐことがあり、意図しない費用になるため、省略しない。
- 依頼文には次を含める: 目的、対象ファイル、完了の条件、参照すべき文書。会話の経緯は渡さない。
- コミット・MR・報告は日本語で書く。時刻は JST で示す。

## MR の流れ

0. ルートが git 管理されていない場合や、配下に複数のリポジトリがある場合は、git・glab の操作を
   すべて対象リポジトリのディレクトリで行う。worktree も対象リポジトリごとに作る。
   依頼文には対象リポジトリのパスを必ず書く。
1. 対象リポジトリの既定ブランチから作業ブランチを作る（例: feat/xxx, docs/adr-0006）。
2. コミットして `git push -u origin <ブランチ>` で push する。
3. `glab mr create --fill --target-branch <既定ブランチ> --remove-source-branch` で MR を作成する。
4. 司令塔が reviewer に自己レビューを依頼する。
   - 「要修正」の場合は、作成した担当に修正を依頼し、もう一度レビューする（最大 3 回まで。
     超えたら人間に判断を求める）。
5. CI（GitLab Runner）は使わないので、マージの前に司令塔が担当の子に、MR のブランチで
   lint・型検査・テストをローカルで実行させ、すべて通ることを確かめる。
   失敗したら修正させ、手順 4 に戻る。テストを無効にして通すことはしない。
6. レビューが「OK」でテストも通ったら、司令塔が
   `glab mr merge <番号> --remove-source-branch --yes` を実行してマージする。
7. マージを確かめたら、worktree を削除する（`git worktree remove <パス>`）。

## 司令塔がマージしないもの（人間の承認を待つ）

- 秘密情報・認証・権限に関わる変更
- データベースのスキーマ変更・データ移行
- CI 設定（.gitlab-ci.yml）・デプロイ設定の変更
- 依存ライブラリの追加・メジャーバージョンの更新
```

### 3.3 権限の設定（確認のプロンプトを減らす）

`.claude/settings.json` に、よく使う安全なコマンドを許可しておく。

```json
{
  "permissions": {
    "allow": [
      "Bash(git status:*)", "Bash(git diff:*)", "Bash(git log:*)",
      "Bash(git add:*)", "Bash(git commit:*)", "Bash(git push -u origin:*)",
      "Bash(git worktree:*)",
      "Bash(glab mr create:*)", "Bash(glab mr view:*)", "Bash(glab mr diff:*)",
      "Bash(glab mr merge:*)"
    ],
    "deny": [
      "Bash(git push --force:*)", "Bash(git push -f:*)"
    ]
  },
  "effortLevel": "medium"
}
```

- `effortLevel` はセッションの既定エフォート。保険として置く。
- モデルのバージョンを書くキー（claude-opus-5-5 など）は、モデルの世代交代で古くなるので使わない。サブエージェントの model / effort は、別名（opus・sonnet・haiku）で、定義ファイルの frontmatter と Agent ツールの呼び出しごとの指定に書く。
- サブエージェントのエフォートの優先順位（高い順）: 環境変数 CLAUDE_CODE_EFFORT_LEVEL、Agent ツールの呼び出しごとの effort、定義ファイル（.claude/agents/*.md）の effort、セッションのエフォート。`effortLevel` の位置が、この順位のどこに入るかは公式ドキュメントに明記されていない。

### 3.4 GitLab 側の設定

- 既定ブランチを保護し、直接 push を禁止する（「設定 > リポジトリ > 保護ブランチ」）。
  マージは MR 経由に限られるので、司令塔のマージも MR の記録として残る。
- CI を使わないため、「パイプラインが成功した場合のみマージを許可」は **無効** のままにする
  （有効にするとパイプラインがない MR をマージできない）。
- 前の節の「司令塔がマージしないもの」を確実に守るには、該当するパスに CODEOWNERS を設定し、
  承認を必須にする。これは有償版（Premium 以上）の機能で、無償版（Community Edition）では使えない。
  無償版の場合は、CLAUDE.md の規則と reviewer の確認だけで守ることになる。

## 4. 使い方

1. 司令塔として起動する: `claude --model sonnet` を実行し、`/effort low` でエフォートを下げる。
2. 依頼する例:
   > designer に ADR0006「P0 の範囲」を書かせて。参照は docs/decisions/0001〜0005。
   > 完了したら reviewer にレビューさせ、OK でテストも通ればマージして。
3. 2 つを並行させる例:
   > implementer を 2 つ worktree で起動して、A はパーサ（src/parser/）、B は検査器（src/check/）を担当させて。
4. 司令塔は子からの短い報告だけを受け取る。差分の全文は司令塔に貼らない（貼ると司令塔のトークンが増える）。

## 5. トークンを抑えるための注意

- 依頼文には目的・対象・完了条件だけを書き、会話の経緯は渡さない。
- 子からの報告は短くさせる（各定義に行数の上限を書いてある）。
- 司令塔では、長いファイルの読み込みやテストの実行をしない。それらは子に任せる。
- 区切りのよいところで `/compact` を実行し、残す内容（役割、対象リポジトリ、決定事項、
  未完了の作業）を指定する。
- 判断の質が成果物そのものになる設計作業だけに Opus を使い、それ以外は Sonnet か Haiku を使う。

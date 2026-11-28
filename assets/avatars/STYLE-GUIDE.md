# リポジトリ用アバター 画風ガイド（STYLE-GUIDE）

Sigil 一族の全23リポジトリ用アバターについて、4つの画風の仕様と再生成手順をまとめた文書。
どの画風でも、この文書のプロンプト要点とモチーフ一覧を使えば同じシリーズを再生成できる。

- **採用画風: 案2（Muse 系ぬいぐるみ風）** — 2026-11-23 所有者決定
- 案1（ポップアート）と agy 版も所有者が「捨てがたい」と評価したため、仕様を残してある
- ファイルの置き場所: このリポジトリの `assets/avatars/` 配下。採用画風（案2）の全23点は `assets/avatars/muse/`、他の画風の試作サンプルは `assets/avatars/samples/pop/`（案1）、`assets/avatars/samples/agy/`（agy 版）、`assets/avatars/samples/codex/`（Codex 版）。ファイル名は `<リポジトリ名>.png`、サイズは 512×512
- 原寸画像と生成メタデータはリポジトリには入れず、ローカル `~/workspace/repo-avatars/` に保管している（画風ごとのフォルダ `pilot/`、`pilot2/`、`agy/`、`codex/` に原寸 PNG と生成メタデータ `media-generation-*.json` がある）

## 共通ルール（全画風）

- 正方形、1リポジトリ1点
- 小さいアイコン表示でも判別できるシンプルな構図（主役は中央に大きく1つ）
- 文字・ロゴ・記号文字は入れない
- モチーフは下記の「モチーフ一覧表」に従う（画風を変えてもモチーフは変えない）

---

## 画風1: 案1（ポップアート）

- **保存先**: `assets/avatars/samples/pop/`（試作3点のみ）
- **生成経路**: media の画像生成ツール（`media.generate_image`）
- **画風仕様**: 高彩度のポップな色使い、太い黒の輪郭線で締める画風。丸っこいマスコット顔。背景は単色またはシンプルな色面
- **プロンプト要点（テンプレート）**:

```
A cute mascot character in bold pop-art style, vivid high-saturation colors,
thick black outlines, round friendly face, simple bold composition readable
at small icon size, square format, plain solid color background, no text.
Subject: <モチーフの説明>
```

## 画風2: 案2（Muse 系ぬいぐるみ風）★採用

- **保存先**: `assets/avatars/muse/`（全23点）
- **生成経路**: media の画像生成ツール（`media.generate_image`）
- **画風仕様**: Muse のアバター（Jolly 系）のような柔らかいぬいぐるみ風 3D マスコット。ふわふわの毛並みの質感、輪郭線のない柔らかい色面、丸みのある造形、パステル基調。表情はドット目・小さな口・頬のピンクの赤み。背景は柔らかい単色（クリーム、ラベンダー、ピーチ等のパステル）
- **プロンプト要点（テンプレート）**:

```
A soft plush 3D mascot character rendered like a cute fluffy stuffed toy,
rounded shapes with no outlines, gentle pastel colors, fluffy fur texture,
simple dot eyes, tiny mouth, soft pink blush on the cheeks, gentle friendly
expression, single character centered, simple composition readable at small
icon size, square format, plain soft solid pastel background, no text.
Subject: <モチーフの説明>
```

## 画風3: agy 版（濃紺×ちびマスコット）

- **保存先**: `assets/avatars/samples/agy/`（試作3点のみ）
- **生成経路**: ローカル agy（Antigravity CLI、クマの VM の `~/.local/bin/agy`、プロンプトは `-p='...'` の形で渡す）
  - 注意: Antigravity Remote Agents でも画像生成自体はできるが、生成物がリモート側ワークスペースに保存されてローカルへ取り戻す手段がない（2026-11-23 実測）。必ずローカル agy で生成し、保存まで完結させる
- **画風仕様**: 濃紺の背景に、光沢のある ちびマスコット風イラスト。柔らかいグラデーションと丸みのある造形、滑らかな質感。太い輪郭線は使わない
- **プロンプト要点（テンプレート）**: agy への依頼文に以下を含める

```
リポジトリ用アバターを1点生成して <保存先パス> に保存してください。
画風: 濃紺の背景、光沢のある ちびマスコット風イラスト、柔らかい
グラデーション、丸みのある造形、滑らかな質感。小さいアイコンでも
判別できるシンプルな構図、正方形、文字なし。
モチーフ: <モチーフの説明>
```

## 画風4: Codex 版（紫×バッジ）

- **保存先**: `assets/avatars/samples/codex/`（試作3点のみ）
- **生成経路**: ローカル Codex CLI（`~/.local/bin/codex`）の `codex exec` 内蔵画像生成。ヘッドレス実行は stdin を閉じる（`< /dev/null`）のが必須。生成画像は一度 base64 で出力されるため、実行ログが大きくなる。2点目以降は出力をログファイルへ逃がすと安定する（2026-11-23 実測）
- **画風仕様**: 深紫＋ラベンダー＋ゴールドの統一パレット。太いプラム色の輪郭線、丸っこいカワイイ系マスコット顔（ピンクのほっぺ、閉じた笑い目）。円形バッジ／紋章の構図で揃える
- **プロンプト要点（テンプレート）**: `codex exec` への依頼文に以下を含める

```
リポジトリ用アバターを1点生成して <保存先パス> に保存してください。
画風: 深紫・ラベンダー・ゴールドの配色、太いプラム色の輪郭線、
丸っこいカワイイ系マスコット顔（ピンクのほっぺ、閉じた笑い目）、
円形バッジの構図。小さいアイコンでも判別できるシンプルさ、正方形、
文字なし。
モチーフ: <モチーフの説明>
```

---

## モチーフ一覧表（全25リポジトリ）

| リポジトリ | モチーフ |
|---|---|
| sigil | 魔法陣（印章）の中心に顔を置いた紋章フェイス |
| sigil-db | 円柱型データベースの缶キャラ |
| sigil-git | 枝分かれする苗木・木のキャラ（バージョン分岐） |
| sigil-container | 中身を運ぶ箱・コンテナのキャラ |
| sigil-os | 光る核（カーネル）のキャラ |
| sigix | トレイでページを運ぶ給仕キャラ（Web サーバー） |
| sigil-shield-tls | 盾のキャラ（TLS 終端） |
| sigil-stash-kv | 鍵付きの宝箱・貯蔵壺のキャラ（KV ストア） |
| sigil-herald-queue | ラッパと手紙の列を持つ伝令キャラ（キュー） |
| sigil-rune-auth | ルーンの刻まれた鍵のキャラ（認証） |
| sigil-argus-observe | たくさんの小さい目を持つ見張りキャラ（監視。フクロウは vigil-ops と重なるので避ける） |
| sigil-atlas-dns | 地球儀を担ぐキャラ、または道しるべの標識キャラ（DNS） |
| sigil-raven-mail | 手紙をくわえたカラスのキャラ（メール） |
| sigil-orchestra | タクトを振る指揮者キャラ |
| sigil-summon-functions | 魔法陣から顔を出す小さい精霊キャラ（サーバーレス召喚） |
| sigil-aegis-waf | ドーム状のバリアを張るキャラ（WAF。shield-tls の盾とは形で区別する） |
| sigil-seer-preempt | 水晶玉をのぞく予見者キャラ |
| sigil-beacon-intel | 烽火台・灯台のキャラ（脅威インテリジェンス共有） |
| sigil-forge-attest | ハンマーと証明の印を持つ鍛冶屋キャラ |
| sigil-hourglass-time | 砂時計のキャラ（時刻同期） |
| sigil-vigil-ops | ランタンを提げたフクロウの夜警キャラ（自律運用） |
| sigil-lantern-stream | 幻灯機（マジックランタン）の映写キャラ（再生ボタン入りの光のビーム） |
| sigil-agora-post | 吹き出しが集まる広場の掲示板キャラ（SNS） |
| sigil-mirror-pane | 鏡のキャラ（白雪姫の魔法の鏡。ブラウザ、製品名 Kagami）。キービジュアル6点（候補A〜E・G）を同フォルダに `sigil-mirror-pane-kv-*.png` として保持 |
| sigil-atelier-desk | 設計図の巻物を抱えた見習いキャラ（IDE、製品名 Atelier）。キービジュアル3点（候補A〜C）を同フォルダに `sigil-atelier-desk-kv-*.png` として保持 |

## 生成状況（2026-11-23 時点）

- 案2（採用）: 全23点そろい（試作3点＋2026-11-23 に残り20点を追加生成）
- 案1／agy 版／Codex 版: 試作3点のみ（sigil、sigil-vigil-ops、sigil-lantern-stream）。全点生成は未実施で、必要になったらこのガイドで再生成する

# 0034: PostgreSQL クライアント（段階 6-5）

- **状態**: 提案（Issue #446）。判断点 A〜G は typesafe-ai 正式判定で決定済み。実装とマージの承認を意味しない。
- **日付**: 2026-11-14（JST）
- **起草・判定実行**: Codex Cloud（OpenAI 系統）
- **対象**: PostgreSQL protocol v3 のクライアント／ドライバと接続プール。サーバー実装は対象外。

## 背景と既決事項

[ADR 0018](0018-p1-scope.md) の段階 6-5 を具体化する。既存言語のドライバや libpq を持ち込まず、通信方式を Sigil で実装する。ORM、スキーマ移行、他の DB、レプリケーションは範囲外。

ADR 0018 の所有者決定（2026-10-26）を継承し、再判定しない。

- **判断 4 = A**: PostgreSQL の公式イメージを固定版で CI に入れる。実装時はタグに加えて digest を固定し、使い捨て資格情報と隔離された網で実行する。イメージの具体的な版・digest は後続の CI 準備で記録する。
- **判断 6b = A**: 認証は SCRAM-SHA-256 のみ。通信は平文、DB はサービスと同じ内部の網に置く。TLS、SCRAM-SHA-256-PLUS、`password`、`md5` は扱わない。SCRAM は SQL・結果の暗号化を提供しない。
- 資格情報は呼び出し側が環境変数（`env` 効果）で取得して渡す。ドライバは環境変数を暗黙に読まない。秘密をリポジトリ、ログ、エラー文へ含めない。

### 既存資産と不足する入口

| 資産 | 再利用するもの | 6-5 で必要な差分 |
|---|---|---|
| 6-1、[ADR 0024](0024-crypto-primitives.md) | `Secret`、SHA-256、HMAC、PBKDF2、定数時間照合。現実装の PBKDF2 は `task.suspend` を持つ | SCRAM の属性・base64・鍵導出・証明・署名照合。導出した 32 バイト鍵を HMAC に渡し、短いパスワードを HMAC の公開 API に直接渡さない |
| 5-2、[ADR 0030](0030-p1b-native-concurrency.md) | スレッド、実行の割り当て、epoll、取消、共有の機構 | DB 用の独立したランタイムを作らず、既存の待機・取消・上限を使う |
| 6-3、[ADR 0031](0031-p1-first-wait-ops-and-socket-api.md) | 非ブロッキング I/O、fd 所有・破棄、`raw.io_wait_fd`、期限、評価器とネイティブの共通 Sigil 実装 | 現在は `accept` 側で、outbound `connect` がない。既存 `TcpStream` の読み書きは `net.server`。接続側の所有型・`net.client` の境界を先に追加する |
| 効果と暗号乱数 | `random` カテゴリは既存 | 乱数源の API・実装は未整備。SCRAM nonce の OS 乱数境界が必要 |

これらの不足を「既に使える」と扱わない。本 ADR は新規文書だけで、ソケットや暗号実装の変更は含まない。

## 判断点と正式判定

評価軸は、宣言効果と推論効果の完全一致、信頼基盤を小さく・明示的・列挙可能にすること、守れない層を無くすこと、既決の所有者方針、秘密と資源の保護、検査可能性の順とした。

| 判断点 | 推奨・採用案 A | 代案 B／C | A の確率 / confidence |
|---|---|---|---|
| A: 型の範囲（`Types`） | 小さい明示型表、NULL を分離、未知型はエラー | 無型文字列／全型とバイナリ形式 | 1.00 / 1.00 |
| B: 実行 API（`API`） | 既存の中断モデル、一接続一操作、単一文と Bind | OS 同期ブロック／多重化と二系統 API | 0.99 / 0.98 |
| C: 効果（`Effects`） | 実際の `net.client` 等をすべて伝播 | 全体を Rust builtin／検査例外で `db` へ隠す | 1.00 / 1.00 |
| D: 信頼基盤（`Trust`） | 最小の connect と OS 乱数の境界、上位は単一 Sigil 実装 | libpq 等／弱い nonce と効果の読み替え | 1.00 / 1.00 |
| E: プール（`Pool`） | 排他的 lease、Idle と清掃成功だけ返却、それ以外は閉じる | 非同期 drop で巻戻し／無検査返却 | 0.99 / 0.98 |
| F: 上限（`Bounds`） | 上限付きの結果収集と期限 | 無制限／初期から COPY 等と背景タスク | 1.00 / 1.00 |
| G: SCRAM の初期範囲（`SCRAMProfile`） | ASCII パスワード、厳密な検証と OS nonce | 未正規化 Unicode を完全対応と称す／初期から TLS 等 | 1.00 / 1.00 |

判定は全問 A。最大の次点確率は 0.01 で近接なし。入力・応答の全文は末尾に記録した。所有者は後から確認・覆せる。

## 決定 1: 認証と通信

- StartupMessage は protocol 3.0、ユーザー・DB 名は NUL を含まない UTF-8。クライアント符号化は UTF8 に固定し、応答の符号化も検査する。DSN 文字列を SQL に展開しない。接続先は明示的な IPv4 数値アドレスとポートとし、DNS と Unix ソケットは後続に回す。
- P1 のパスワードは印字可能 ASCII に限定する。非対応は接続前に `UnsupportedCredential`。Unicode パスワードの SASLprep を実装済みと称さない。空パスワードは禁止し、長さは 1024 バイト以下。ユーザー名と DB 名は各 1024 バイト以下。
- SCRAM-SHA-256 以外へのダウングレードをしない。SCRAM の交換をせず到着する `AuthenticationOk`（`trust` 等）も拒否する。
- nonce は OS の暗号乱数 24 バイトを base64 にする。時計・連番・再使用で代用しない。nonce の接頭辞一致とサーバー側の延長、必須属性の欠落・重複、未知の必須拡張、base64 の形を検査する。SCRAM メッセージは 16 KiB 以下、塩は復号後 1024 バイト以下。
- `4096 <= iterations <= 100000` を PBKDF2 呼び出し前に検査し、未検査の入力を `requires` に渡して停止させない。32 バイトの SaltedPassword から鍵を作り、ClientProof の XOR と ServerSignature の照合を ADR 0020 の定数時間検査に登録する。
- server-final の署名照合、`AuthenticationOk`、`ReadyForQuery` の順まで完了して初めて接続成功。認証失敗・署名不一致・順序違反は閉じる。認証用の秘密と中間値を破棄時にゼロ化し、表示 API を持たせない。

既決の認証方式を維持し、Unicode 全体の新しい信頼対象を初期実装に足さないため、この範囲を採る。

## 決定 2: 型と文の実行

パラメータと結果は text 形式（format code 0）に統一する。接続の初期化で固定の設定から `bytea_output=hex` を選び、プール清掃後も再設定する。利用者がこの設定を変えた結果の非 hex 応答は型エラーとして閉じる。バイナリ形式は入れない。

| PostgreSQL 型 / OID | Sigil の値 | 検査 |
|---|---|---|
| bool / 16 | `Bool` | 結果は `t`・`f` のみ |
| int2 / 21、int4 / 23、int8 / 20 | `Int` | 十進の形と各 PostgreSQL 型の範囲。Sigil の桁あふれを起こす前に検査 |
| text / 25、varchar / 1043 | `String` | UTF-8 と長さ。NUL をパラメータで拒否 |
| bytea / 17 | `Bytes` | `bytea_output=hex` を使い、`\x` と偶数桁の hex を検査 |
| SQL NULL | `Option<PgValue>` の `None` | 空文字列・空バイト列と区別。NULL パラメータも型 OID を必須とする |

`PgValue` は上記の型を区別する列挙、`PgParam` は OID と nullable な値を持つ。OID と値の矛盾は送信前に `TypeMismatch`。未知 OID は `UnsupportedType` とし、文字列への暗黙変換をしない。float、numeric、日時、uuid、json、配列、domain、enum は後続 ADR の対象。これにより損失を伴う変換を避け、対応表をテストで列挙できる。

- 拡張 query protocol の unnamed statement／portal を使う。Parse → Bind → Describe portal → Execute → Sync。複数 SQL 文は Parse の単一文制約で拒否する。文字列連結による値の SQL 展開、複数文の Simple Query 公開 API は作らない。
- パラメータ OID を Parse に渡す。値は Bind に分離し、SQL の識別子をパラメータとして扱わない。SQL 自体の信頼性は呼び出し側が担う。
- RowDescription の列数、OID、format、DataRow のフィールド数・符号付き長さ（NULL は -1 のみ）、メッセージ残量、UTF-8 を検査する。長さの加算・確保前に上限を調べる。
- 戻り値は列情報、上限付きの行一覧、CommandComplete のタグ。BEGIN／COMMIT／ROLLBACK のような行のない単一文も同じ入口を使う。公開 prepared statement ハンドル、pipeline、COPY、LISTEN/NOTIFY、ストリーミングは P1 に入れない。
- ErrorResponse、NoticeResponse、ParameterStatus、BackendKeyData など必要な応答を状態機械で区別する。ErrorResponse の表示は SQLSTATE と固定の分類だけにし、サーバーの詳細文・SQL・値を自動で出力しない。通知は上限付きで読み捨て、未知のメッセージ・不正な順序・途中 EOF は閉じる。CancelRequest 用の秘密はログに出さず、P1 では CancelRequest を実装しない。
- 文エラーでは Sync 後の ReadyForQuery を期限内に読む。受信完了を確認できなければ接続を捨てる。自動で SQL を再実行しない。送信後の切断は実行済みか不明になり得るため、成功とも未実行とも断定しないエラーを返す。

## 決定 3: API と効果の境界

新しい `async` 構文や Future 型を増やさず、既存の `task.suspend` モデルに乗せる。`targets server` に限る。以下は API の責務の表で、コンパイルできる実装の宣言ではない。具体的な表への登録と完全一致を後続の検査で確かめる。表の接続・実行は本体の最小効果であり、公開入口はすべて決定 6 の期限で包むため、`clock, task.spawn` も実際の宣言に含める。無期限の入口は公開しない。

| 入口 | 形 | 効果の方針 |
|---|---|---|
| 接続 | `pg.connect(config, limits) -> Result<PgConn, PgError>` | `net.client, random, task.suspend` |
| 実行 | `PgConn.query(&mut self, sql, params, limits) -> Result<PgResult, PgError>` | `net.client, task.suspend` |
| 借出・返却 | `PgPool.acquire` と消費する `PgLease.release` | 待機の `task.suspend`、共有操作の `task.share`、新規接続や清掃で起こす上記の効果を伝播 |
| 純粋な部分 | フレーム解釈、型変換、上限検査 | `none`。PBKDF2 は例外として既存の `task.suspend` |
| 期限を持つ入口 | 接続・query・借出・清掃を `task.timeout` で包む | `clock, task.spawn, task.suspend` と本体の効果を完全に伝播 |

`PgConn` と `PgLease` は複製せず、`Shareable` にしない。`&mut PgConn` の操作は同時に一つだけ。`PgPool` は既存の `Lock`・共有の型の規則で管理し、共有のロックを持ったままネットワークを待たない。型・借用・共有の検査でこの形が表せない場合は、例外を作らず、その実装を止めて ADR を見直す。

**`db` 効果の扱い**: カテゴリは ADR 0004 に存在するが、現状の Sigil の関数に `db` を名目上追加するだけでは完全一致に反する。`trust none` の上位コードから `net.client` や `random` を隠さない。初期実装の `sigil spec` は上の実効果を示し、`db` だけに集約しない。`db` を与えるためだけに全ドライバを builtin や `unchecked` にしない。将来の集約には完全一致を担保する検査規則と別 ADR が必要であり、本 ADR はその例外を許可しない。

## 決定 4: 信頼基盤

- フレーム解釈、型変換、SCRAM の組立と検証、プールの方針は `std.pg` の Sigil 実装にし、`trust none`、`unchecked`・`raw.*` なし。評価器専用の Rust ドライバを作らない。
- **増分はゼロではない**。outbound ソケットを一つ所有する専用の型と非ブロッキング接続、SO_ERROR による接続完了確認、OS の暗号乱数（Linux x86-64 の `getrandom`）が新しい境界になる。fd は中断前に所有型へ包み、失敗・取消・破棄で一度だけ閉じる。接続完了を writable だけで成功と判断しない。
- `socket`・`connect`・`getsockopt`・`getrandom` と既存の読み書き・close の利用箇所、評価器の syscall 許可表、型の破棄と表の登録を後続 PR で列挙する。新しい原始操作を無条件に増やす決定ではなく、既存の `raw.syscall` と待機を優先する。
- 既存 `TcpStream.read/write_all` の `net.server` を呼んで `db` や `net.client` だけと宣言することを禁止する。接続側の操作を `net.client` として列挙し、低水準の中断しない補助処理は共有できるものを共有する。サーバー側の意味と効果を黙って変更しない。
- 乱数失敗を疑似乱数に落とさない。乱数 API は `random` を明示し、部分読みと EINTR を処理する。テストの固定 nonce は公開の本番 API から選べないようにする。
- 新規 `unchecked`・syscall 許可表等は ADR 0018 判断 2 の信頼基盤差分と、ADR 0020 のレビュー関門の対象。CI・CODEOWNERS の変更は人間の作業として分ける。本 ADR の PR にそれらを混ぜない。

プロトコル全体を信頼基盤へ入れる案より差分を列挙しやすく、二重実装の層も増やさないため採る。

## 決定 5: プールと取消

- プールは最大数を明示して遅延接続する。接続中・貸出中・清掃中も接続枠に数え、借出待機にも上限を持たせる。共有状態の破損は `LockBroken` として失敗し、壊れた接続を貸さない。
- lease が接続を排他所有する。正常な明示的 release だけが返却の機会を持つ。利用者へ fd や内部の接続を複製する入口を出さない。
- **返却条件は ReadyForQuery の I（idle）かつ `DISCARD ALL` の成功と、その後の ReadyForQuery I の確認**。これで前の借り手の SET や prepared statement 等のセッション状態を次へ持ち越さない。清掃中は貸し出さない。
- T（transaction）・E（failed transaction）、途中送信、同期状態不明、文の取消、期限切れ、ルート異常停止、清掃失敗は返却せず閉じる。トランザクションの途中での返却は、ADR 0018 決定 7 の選択肢のうち「捨てる」を採る。暗黙 ROLLBACK は送らない。
- drop は待機・通信・非同期清掃をせず閉じ、枠を回収する。待たない破棄と既存の取り消し順序を守る。SQL の実行結果が不明な接続を自動で再接続・再実行しない。

## 決定 6: 資源の上限と確認

| 項目 | P1 の既定・最大 |
|---|---|
| backend/frontend の一メッセージ | 1 MiB（wire のタグと長さ欄も含む） |
| 一回の結果 | wire 累積 4 MiB、10000 行、256 列 |
| SQL とパラメータ | wire の一メッセージ制限内、SQL 64 KiB 以下、256 パラメータ以下 |
| pool 接続数 / 借出待機数 | 16 / 64 |
| 接続・認証 / 一操作 / 清掃 / 借出 | 10 秒 / 30 秒 / 5 秒 / 30 秒 |
| PBKDF2 の反復 | 4096〜100000 |

初期 API はこれ以下に狭める設定だけを許す。超過は `LimitExceeded`、期限切れは明示的な timeout の結果とし、接続は閉じる。メッセージ数だけを変えて無限に待たせる応答にも、操作全体の期限と wire 累積上限が働く。結果の複製・復号後の値には wire 長から導く明示的なメモリ上限を実装で設け、列ごとのバッファを無制限に残さない。全プールの最悪値は接続上限を掛けて数え、値として取得できるようにする。隠れたログ出力は追加しない。

[ADR 0020](0020-p1-security-addendum.md) の公開前関門を完了条件に含める。

- 純粋部分: 分割受信・結合受信、長さ -1／不正負数／切詰め／桁あふれ、未知 OID、NULL と空、SQLSTATE、SCRAM の署名不一致と downgrade、反復・塩・nonce の上限を単体試験する。
- 暗号: RFC 7677 等の既知ベクタ、ClientProof と ServerSignature の定数時間検査の表への登録、SIR と最適化後 IR の検査、秘密の表示禁止とゼロ化。正負両方の対照を持つ。
- 外部の模擬サーバーから同じ応答列を与え、評価器とネイティブで結果・エラー・閉じた接続を突き合わせる。模擬サーバーは製品ドライバの別実装にしない。
- 固定版 PostgreSQL で SCRAM 接続、全対応型・NULL・Bind の分離、文エラー後の同期、commit／rollback、プールの再利用とセッション清掃を試験する。悪い署名等は模擬サーバーで試す。
- 期限切れと取消を接続・認証・部分送信・受信・借出・清掃の各中断点で起こし、fd・接続枠・待機枠の漏れ、T/E 状態の返却、他のルートへの混入を検査する。
- protocol/SCRAM の解釈へ、既存のクレートを足さない方式でファジングを追加する。独立レビューの指摘を解消してから公開する。

## 後続 Issue の切り方

| 段 | 範囲 | 依存・完了条件 |
|---|---|---|
| 6-5a | 接続側ソケットと暗号乱数の境界 | 本 ADR。所有・破棄・効果・syscall 許可表の列挙、取消、評価器/native の突合せ。契約変更が要れば先に独立 PR。信頼基盤差分のレビュー必須 |
| 6-5b | 純粋な protocol v3 フレームと型変換 | 本 ADR。型表と境界値、分割受信、上限、ファジング。`trust none` を確認 |
| 6-5c | SCRAM と Startup、最小接続 | a・b。既知ベクタ、定数時間検査、server-final と ready までの順序、資格情報の拒否試験 |
| 6-5d | 単一文 Parse/Bind/Execute、結果とエラー同期、期限 | c。全型、NULL、取消と結果不明の区別、外部相手との突合せ |
| 6-5e | プールと lease、清掃、取消時の破棄 | d。容量と待機の上限、DISCARD ALL、T/E の閉鎖、セッション分離、全枠の回収 |
| 6-5f | 固定版 PostgreSQL の CI 準備 | 本 ADR。人間が workflow を変更し、タグ/digest・使い捨て資格情報・隔離網を記録。判断 4 を実現し d/e の実サーバー試験を CI に載せる |
| 6-5g | 統合・サンプル・公開前確認・完了記録 | a〜f。ADR 0020 の関門、信頼基盤の増分と残存制限、`sigil spec` の実効果、native-report、P1 サービスへの接続を記録 |

本 PR では後続 Issue を直接起票しない。ADR マージ後に、マージした PR の作成者が `scripts/sync-issues.sh --dry-run` で既存の backlog 規則を確認し、同スクリプトの起票経路に上記の段と依存を反映する。規則の追加が必要なら、別の範囲の PR で行い、既存の自動起票と重複させない。最初は a・b・f が着手可能、c は a/b 待ち、d→e→g の順とする。f の workflow は人間に依頼する。ADR の確定前に実装・信頼基盤の変更へ着手しない。

## 本 PR の影響と制限

追加するのは本ファイルだけ。既存コード、`sigil spec` 出力、依存、CI の差分はない。番号 0034 は最新 main とオープン PR を確認して選んだ。`docs/decisions/README.md` の予約行は「新規 ADR ファイルのみ」という依頼範囲を優先して変更しない。後続の一覧更新時にリンクを反映する。

認証方式と CI は既決方針に従い、残る選択は以下の正式判定に基づく。これはコードの実装済みの保証ではなく、後続 PR が検査・試験・必要なレビューを満たすための契約である。

## 正式判定の入力・応答・決定の記録

2026-11-14（JST）、Codex Cloud から `POST https://api.typesafe.ai/v1/systemone`、モデル `jev-1.13.0` で実行。認証情報・リクエストヘッダーは記録しない。入力は候補と固定評価軸を含む JSON、応答は API が返した JSON 全体。1 回目で A〜F、2 回目で G を判定した。

### 1 回目: A〜F

入力:

```json
{
  "model": "jev-1.13.0",
  "state": "固定評価軸（優先順）: 1. Sigil の理念: 宣言効果と推論効果の完全一致を検査系が担保する。信頼基盤は小さく明示的で列挙可能にし、守れない層・二重の意味実装を無くす。2. 所有者の明示方針: ADR 0018 判断4は固定版PostgreSQLをCIで試験、判断6bはSCRAM-SHA-256のみ、平文の内部網、TLS/password/md5なし。既存言語のDBライブラリ・ORM・サーバー実装なし。新規ADRだけ起草し実装は後続、マージしない。3. 秘密保護、悪意のある応答と取消に対する資源上限、検査可能性。4. 小さいAPIと段階実装。\n状況: Issue #446、PostgreSQL protocol v3クライアントとプール。暗号sha256/hmac/pbkdf2/Secret/ct_eqとtask.suspend/epoll/スレッドは実装済み。既存TCPはaccept/read/writeだけ、効果net.serverでoutbound connectなし。乱数源もなし。既存db効果は定義のみで純粋なSigil関数が効果dbを宣言するだけでは完全一致を満たせない。最小の接続/乱数のOS境界追加を独立PRでレビューし、プロトコル/型/SCRAM/プールはSigilで単一実装・検査下に置く必要がある。認証/CIは既決で再判定しない。以下の各問いは未決で推奨A。候補を評価し選択してください。",
  "questions": {
    "Types": {
      "type": "choice",
      "criteria": {
        "A": "P1はbool/int2/int4/int8/text/varchar/byteaのみ。nullableはOption、OIDと長さ・範囲を検査。全てtext形式、未知OIDはUnsupportedTypeで黙って変換しない。float/numeric/json/date/uuid/array等は後続ADR。",
        "B": "全型を無型のStringで返す。NULL/型の意味は利用者に委ねる。",
        "C": "最初から全組込み型/バイナリ形式/配列/任意型の自動変換を実装。"
      }
    },
    "API": {
      "type": "choice",
      "criteria": {
        "A": "asyncキーワードは増やさず既存の中断モデル。所有するPgConnを&mutで一度に一操作、query(sql,typedParams,limits)を拡張queryプロトコルの単一文として実行、データ値はBind。同期ブロックAPI/多重化/自動再試行なし。",
        "B": "同期OSブロックAPIのみ。各DB操作がスレッドを止める。",
        "C": "共有接続で多重化とパイプライン、同期/非同期API双方を初期実装。"
      }
    },
    "Effects": {
      "type": "choice",
      "criteria": {
        "A": "初期APIは実際に推論できる既存効果を全て見せる。connectはnet.client,random,task.suspend、queryはnet.client,task.suspend、期限付き入口はclock,task.spawn等task.timeoutの効果も伝播、プールはtask.share等も伝播。trust none側でdbを名目だけに追加しない。db効果への集約は完全一致を保てる検査規則と別ADRなしに行わない。",
        "B": "DBの全操作をRust builtinで包んでdb効果だけにする。プロトコル/暗号/プールも信頼基盤に入れる。",
        "C": "trust noneの関数でnet.client/random等を隠し宣言をdbだけにする。検査の例外で通す。"
      }
    },
    "Trust": {
      "type": "choice",
      "criteria": {
        "A": "outbound専用の所有型とconnect/SO_ERROR境界、getrandom由来nonceの最小OS境界だけ追加。net.server型を再利用して効果を隠さない。Sigilプロトコルと暗号部品を評価器/nativeで共用、syscall許可表/unchecked差分は独立審査。DNS/TLS/FFIなし、IPv4数値アドレスを必須にする。",
        "B": "Rust/libpqドライバと乱数クレートを追加し独立実装。",
        "C": "nonceに時計やカウンタだけを使い、既存net.serverソケットAPIをDB効果に読み替える。"
      }
    },
    "Pool": {
      "type": "choice",
      "criteria": {
        "A": "所有するleaseで接続を排他貸出、容量/待機数/期限を上限化。ReadyForQuery Iのみ返却可能。ただしセッション汚染防止に返却前にDISCARD ALL完了も必要。transaction/失敗/取消/部分送信/同期不明/清掃失敗はclose、dropは待たず閉じる。暗黙ROLLBACKと自動再実行なし。",
        "B": "取消でもROLLBACKを非同期dropから送って全接続をプールへ戻す。",
        "C": "ReadyForQueryの状態やセッション変更を検査せず接続を再利用する。"
      }
    },
    "Bounds": {
      "type": "choice",
      "criteria": {
        "A": "初期は結果を上限付きで収集、message 1MiB/結果4MiB/10000行/256列、pool16接続・待機64、接続10秒/操作30秒/清掃5秒、PBKDF2 4096..100000。超過は型付きエラーと接続破棄。ストリーミング/COPY/LISTENは後続。",
        "B": "無上限の結果収集、サーバー指定反復回数にも制限しない。",
        "C": "初期からストリーム・COPY・LISTENと無制限のバックグラウンドタスクを提供。"
      }
    }
  }
}
```

応答:

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "Types": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    },
    "API": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.98,
      "probabilities": {
        "A": 0.99,
        "B": 0.01,
        "C": 0.0
      }
    },
    "Effects": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    },
    "Trust": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    },
    "Pool": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.98,
      "probabilities": {
        "A": 0.99,
        "B": 0.01,
        "C": 0.0
      }
    },
    "Bounds": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 2059,
    "output_tokens": 213
  }
}
```

### 2 回目: G

入力:

```json
{
  "model": "jev-1.13.0",
  "state": "固定評価軸（優先順）: 1. Sigil の理念: 宣言効果と推論効果の完全一致を検査系が担保する。信頼基盤は小さく明示的で列挙可能にし、守れない層・二重の意味実装を無くす。2. 所有者の明示方針: ADR 0018 判断4は固定版PostgreSQLをCIで試験、判断6bはSCRAM-SHA-256のみ、平文の内部網、TLS/password/md5なし。既存言語のDBライブラリ・ORM・サーバー実装なし。新規ADRだけ起草し実装は後続、マージしない。3. 秘密保護、悪意のある応答と取消に対する資源上限、検査可能性。4. 小さいAPIと段階実装。\n状況: Issue #446、PostgreSQL protocol v3クライアントとプール。暗号sha256/hmac/pbkdf2/Secret/ct_eqとtask.suspend/epoll/スレッドは実装済み。既存TCPはaccept/read/writeだけ、効果net.serverでoutbound connectなし。乱数源もなし。既存db効果は定義のみで純粋なSigil関数が効果dbを宣言するだけでは完全一致を満たせない。最小の接続/乱数のOS境界追加を独立PRでレビューし、プロトコル/型/SCRAM/プールはSigilで単一実装・検査下に置く必要がある。認証/CIは既決で再判定しない。以下の各問いは未決で推奨A。候補を評価し選択してください。\n追加状況: 上記6問は正式判定で全てA。SCRAMのnonce/署名を検査し不正応答は閉じるが、P1の文字列正規化範囲を確定する必要がある。短いASCIIパスワードもPBKDF2では利用可能。hmac_sha256の32バイト以上契約には導出した32バイト鍵だけを渡す。",
  "questions": {
    "SCRAMProfile": {
      "type": "choice",
      "criteria": {
        "A": "P1の資格情報はユーザー/DB名をNULなしUTF-8としてStartupに渡し、パスワードは印字可能ASCIIに限定し非対応は接続前にUnsupportedCredential。Unicode SASLprep完全実装を装わない。SCRAM-SHA-256のみ、PLUS/MD5/password/trustを拒否、nonceはOS暗号乱数24バイト、nonce前方一致と延長・必須属性重複・反復上限・server署名を厳密検査、server-final検証とAuthenticationOkとReadyForQuery後にのみ接続成功。後続ADRでUnicode資格情報拡張。",
        "B": "Unicodeパスワードを正規化せず全て送ってSCRAM完全対応と呼ぶ。",
        "C": "P1からSASLprepのUnicode全表とPLUS/TLSを実装する。"
      }
    }
  }
}
```

応答:

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "SCRAMProfile": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 1160,
    "output_tokens": 41
  }
}
```

### 決定と根拠

全問で選択された A を採用した。A〜F の具体化は決定 2〜6、G は決定 1 に対応する。実効果を隠さず、低水準の差分だけを列挙し、通信・認証・返却の状態を小さな検査可能な範囲に限定できる。代案は無型化・過大な初期範囲・信頼基盤への全体の移動・検査例外・不確かな再利用を招く。最高確率と次点の差は全問 0.98 以上で、近接による所有者への追加判断依頼は不要とした。所有者の既決事項を変更していない。

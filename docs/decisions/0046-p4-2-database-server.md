# 0046: P4-2 データベースサーバー

- **状態**: 提案（typesafe-ai 正式判定は未実施。起草者の案として明記。末尾に判定用の問いと評価軸を記録）
- **日付**: 2026-11-17（JST）
- **対象**: [Issue #578](https://github.com/goodnasubi/sigil/issues/578)、レーン E

## 背景と前提

[ADR 0044](0044-p4-scope.md) の第 2 段として、P4-1（[ADR 0045](0045-p4-1-database-core.md)）の組み込みデータベース核の上に、ネットワーク経由で SQL を解釈・実行し、PostgreSQL の通信方式（protocol v3）の部分集合を話すデータベースサーバーの仕様を定める。

起草基準は `goodnasubi/sigil` の `9c7c3bacd9cae62fd1299a0ce89ce1c6138cdf7b`。本 ADR は `docs/decisions/0046-p4-2-database-server.md` の新規作成と `docs/decisions/README.md` の索引更新のみを行い、処理系・`sigil spec` の出力・実装・バックログは変更しない。

### 入力と前提

1. **P4-1 データベース核（ADR 0045）**:
   - 単一ノードで排他的に管理される組み込みストレージ（固定 8192 byte ページ、B+tree、全ページ像 redo WAL、no-steal、同期後 commit）。
   - 型付き操作 API（型: 64 bit 符号付き整数、bool、UTF-8 文字列、バイト列。NULL は初版未対応。主キー必須、単一列で一意、二次索引対応）。
   - トランザクションは直列化（serializable）され、1 プロセス内で実行中のトランザクションは同時に 1 つ。
2. **標準ライブラリの不足への対処（Issue #576 結論、コメント 6933389330）**:
   - ファイル境界（ディレクトリ操作・排他ロック・追記・位置指定 I/O・fsync・原子的 rename）は共通 fs 境界 ADR と後続 fs 実装（A〜C）で解決する。
   - 標準ライブラリの実装完了は個別 ADR の起草条件にしない。
3. **版取得方法（Issue #573 コメント 6933694595）**:
   - `pinned_source`: 完全なコミット SHA を checkout し、同梱 `Cargo.lock` を使い `cargo build --locked --release` でコンパイラと標準ライブラリを取得する。浮動 main やタグだけに依存しない。
4. **P1 クライアント（ADR 0034）との整合**:
   - 通信プロトコルは PostgreSQL protocol v3、認証は SCRAM-SHA-256 のみ、平文通信。
   - 拡張クエリプロトコル（Parse/Bind/Describe/Execute/Sync）のテキスト形式（format code 0）を標準とする。
5. **実装の置き場所と外部公開**:
   - ADR 0044 決定 4 に従い、仕様は本リポジトリに置き、実装は所有者が作成する別のデータベース用リポジトリで行う。所有者によりリポジトリが作成されるまで実装に着手しない。
   - ADR 0044 決定 3 に従い、「いまは外部公開しない」方針を維持する。CI とローカルの隔離環境でのみ検証し、秘密は使い捨てとする。

---

## 選択肢と決定

| 判断 | 採用案 | 他案を採用しない理由 |
|---|---|---|
| SQL の範囲 | 宣言・挿入・更新・削除・選択・単一 INNER JOIN・集約・基本トランザクションの限定部分集合 | 初版からサブクエリ・外部結合・ウィンドウ関数・VIEW 等を含めるとパーサーと最適化器の範囲が過大になる。キーバリュ操作のみに縮退すると関係モデルと PostgreSQL 互換の検証ができない |
| 通信方式の範囲 | PostgreSQL v3 の拡張クエリプロトコル（Parse/Bind/Describe/Execute/Sync）、単一文の Simple Query、SCRAM-SHA-256 認証 | Simple Query のみに絞ると P1 クライアント（ADR 0034）が動かない。全メッセージ（COPY, CancelRequest, FunctionCall 等）を初期から要求すると境界が広がりすぎる |
| 同時実行アーキテクチャ | 複数接続を受け入れ、P4-1 の直列トランザクション実行キューへ委託・直列化（serializable） | 初版から MVCC や複数 writer を入れると版回収と競合解決が複雑化し P4-1 の保証と食い違う。1 クライアント専用サーバーにすると接続プール等の検証ができない |
| 未対応機能の扱い | 暗黙の縮退や無視を行わず、明示的なエラー（SQLSTATE 42601 / 0A000 等）を返して安全に拒否 | 暗黙の無視や縮退は利用者の意図しないデータ破壊や誤った問い合わせ結果を招く |

---

## 決定 1: 対象とする SQL の文の範囲

SQL パーサーおよび実行エンジンは、純 Sigil（`trust none`）で実装し、外部クレートや C ライブラリに依存しない。キーワードは大文字小文字を区別せず、識別子は既定で小文字化（二重引用符 `"` で囲まれた場合は大文字小文字を保持）する。パラメータプレースホルダは `$1`, `$2`, ... 形式とする。

### 対象とする SQL 文の対応表

| 分類 | 文 / 構文 | 初版の対応状況 | 備考・制限 |
|---|---|---|---|
| DDL | `CREATE TABLE` | 対応 | `PRIMARY KEY`（単一列・必須）のみ。列型は後述の 4 型。初版は NULL 未対応のため全列が実質 NOT NULL |
| DDL | `DROP TABLE` | 対応 | `[IF EXISTS]` を許容。`CASCADE`/`RESTRICT` は未対応 |
| DDL | `CREATE [UNIQUE] INDEX` | 対応 | 単一列の B+tree 索引。`[IF NOT EXISTS]` を許容。複合索引・式索引・部分索引は未対応 |
| DDL | `DROP INDEX` | 対応 | `[IF EXISTS]` を許容 |
| DDL | `ALTER TABLE` / `TRUNCATE` | **未対応** | 明示的エラー（`FeatureNotSupported`） |
| DML | `INSERT INTO ... VALUES` | 対応 | 単一行および複数行の定数/パラメータ VALUES。`INSERT INTO ... SELECT`, `ON CONFLICT`, `RETURNING` は未対応 |
| DML | `UPDATE ... SET ... WHERE` | 対応 | 単一表。WHERE 句による条件指定。`FROM` 句, `RETURNING` は未対応 |
| DML | `DELETE FROM ... WHERE` | 対応 | 単一表。WHERE 句（省略時は全件削除）。`USING` 句, `RETURNING` は未対応 |
| DQL | `SELECT ... FROM` | 対応 | 列名指定、`*`、式および別名（`AS alias`） |
| DQL | `INNER JOIN ... ON` | 対応 | 2 表の結合（単一の INNER JOIN のみ）。結合条件は等結合 |
| DQL | `WHERE` | 対応 | 比較（`=`, `<>`, `!=`, `<`, `<=`, `>`, `>=`）、論理（`AND`, `OR`, `NOT`）、括弧 |
| DQL | 集約関数 / `GROUP BY` | 対応 | `COUNT(*)`, `COUNT(col)`, `SUM(col)`, `MIN(col)`, `MAX(col)`。`GROUP BY` 列指定 |
| DQL | `ORDER BY` | 対応 | 1 列以上の列指定、`ASC` / `DESC` |
| DQL | `LIMIT` / `OFFSET` | 対応 | 非負整数定数またはパラメータ |
| DQL | `OUTER JOIN` (LEFT/RIGHT/FULL) | **未対応** | 明示的エラー（`FeatureNotSupported`） |
| DQL | サブクエリ / `HAVING` / `WINDOW` | **未対応** | 明示的エラー（`FeatureNotSupported`） |
| DQL | `UNION` / `INTERSECT` / `EXCEPT` | **未対応** | 明示的エラー（`FeatureNotSupported`） |
| TX | `BEGIN` / `START TRANSACTION` | 対応 | トランザクションの開始 |
| TX | `COMMIT` | 対応 | トランザクションのコミット |
| TX | `ROLLBACK` | 対応 | トランザクションのロールバック |
| TX | `SAVEPOINT` / 隔離レベル指定 | **未対応** | 明示的エラー（隔離レベルは SERIALIZABLE 固定） |

### データ型

P4-1（ADR 0045）および P1 クライアント（ADR 0034）の型体系と厳密に整合させる。

| SQL 型名 | PostgreSQL OID | Sigil 内部型 | 備考 |
|---|---|---|---|
| `BIGINT`, `INT8`, `INTEGER`, `INT`, `INT4`, `SMALLINT`, `INT2` | 20 (int8), 23 (int4), 21 (int2) | `Int` (64 bit 符号付き) | 範囲外の数値はオーバーフローとして拒否 |
| `BOOLEAN`, `BOOL` | 16 (bool) | `Bool` | `TRUE`, `FALSE` |
| `TEXT`, `VARCHAR` | 25 (text), 1043 (varchar) | `String` | UTF-8 検証済み文字列 |
| `BYTEA` | 17 (bytea) | `Bytes` | 16 進表記（`\x...`） |
| `NULL` | — | **未対応** | 初版の P4-1 核が NULL 未対応のため、初版では NULL 値の格納・NULL 許容列・`IS NULL` 句は未対応 |

浮動小数点（`FLOAT`, `DOUBLE PRECISION`）、日付時刻（`TIMESTAMP`, `DATE`）、`NUMERIC`/`DECIMAL`、`JSON`/`JSONB`、配列型、外部キー制約、CHECK 制約は初版で**未対応**とし、対応表に明記して将来の ADR に持ち越す。

---

## 決定 2: PostgreSQL 通信方式（protocol v3）の対応集合

通信方式は PostgreSQL protocol v3 に準拠する。接続は TCP（既定ポート 5432、IPv4/IPv6 数値アドレス）とし、UNIX ドメインソケットは後続とする。

### 認証（Authentication）

- **SCRAM-SHA-256 のみ**: ADR 0034（クライアント）と完全に対称な認証プロトコルを実装する。
  - 平文パスワード（Cleartext）、MD5、`trust`、Kerberos、TLS クライアント証明書などはすべて**拒否**する。
  - ダウングレード攻撃を防止し、SCRAM-SHA-256 以外の要求は即座にエラーとして接続を閉じる。
  - ソルト（Salt）は OS 暗号乱数から生成（最低 16 バイト）、反復回数（iterations）は 4096 回以上 100000 回以下とする。
  - サーバ側 nonce は暗号乱数から生成し、クライアント nonce の接頭辞一致を検証する。
  - 定数時間比較（ADR 0020 / ADR 0024）により認証検証を行う。
- **平文通信**: TLS（SSL）は初版では非対応。
  - クライアントからの `SSLRequest`（コード 80877103）に対しては、1 バイトの `'N'` を返答して平文接続を継続させる（PostgreSQL 規約準拠）。

### メッセージ対応表

| メッセージ種別 | 方向 | Type コード | 初版対応 | 振る舞い・処理 |
|---|---|---|---|---|
| `StartupMessage` | C -> S | なし | 対応 | protocol 3.0 の検証、`user`, `database`, `client_encoding` の取得 |
| `SSLRequest` | C -> S | なし | 対応 | `'N'` を返して平文で継続 |
| `CancelRequest` | C -> S | なし | **未対応** | ログ記録後、接続を切断（ADR 0034 でもクライアント側未実装） |
| `AuthenticationSASL` | S -> C | `'R'` | 対応 | 機構名 `"SCRAM-SHA-256"` の提示（認証種別 10） |
| `SASLInitialResponse` | C -> S | `'p'` | 対応 | client-first-message の解析と検証 |
| `AuthenticationSASLContinue` | S -> C | `'R'` | 対応 | server-first-message の送信（認証種別 11） |
| `SASLResponse` | C -> S | `'p'` | 対応 | client-final-message の解析と署名検証 |
| `AuthenticationSASLFinal` | S -> C | `'R'` | 対応 | server-final-message の送信（認証種別 12） |
| `AuthenticationOk` | S -> C | `'R'` | 対応 | 認証成功の通知（認証種別 0） |
| `ParameterStatus` | S -> C | `'S'` | 対応 | `server_version`, `client_encoding` (UTF8), `server_encoding` (UTF8), `bytea_output` (hex), `integer_datetimes` (on) 等の通知 |
| `BackendKeyData` | S -> C | `'K'` | 対応 | セッション ID / キャンセルキーの送信（初版は固定またはダミー） |
| `ReadyForQuery` | S -> C | `'Z'` | 対応 | トランザクション状態（`'I'`: Idle, `'T'`: In Tx, `'E'`: Error In Tx）の通知 |
| `Parse` | C -> S | `'P'` | 対応 | SQL 文の構文解析、文の登録（無名/名前付きステートメント） |
| `ParseComplete` | S -> C | `'1'` | 対応 | Parse 成功の通知 |
| `Bind` | C -> S | `'B'` | 対応 | パラメータの型変換・検証、ポータルの生成（無名/名前付きポータル） |
| `BindComplete` | S -> C | `'2'` | 対応 | Bind 成功の通知 |
| `Describe` | C -> S | `'D'` | 対応 | ポータル（`'P'`）またはステートメント（`'S'`）の列情報要求 |
| `RowDescription` | S -> C | `'T'` | 対応 | 列名、テーブル OID、列属性番号、型 OID、サイズ、format code (0) の返答 |
| `NoData` | S -> C | `'n'` | 対応 | 行を返さない文（INSERT, UPDATE, DELETE, DDL）に対する返答 |
| `Execute` | C -> S | `'E'` | 対応 | ポータルの実行、最大行数の制御 |
| `DataRow` | S -> C | `'D'` | 対応 | 行データの返答（テキスト形式、format code 0） |
| `CommandComplete` | S -> C | `'C'` | 対応 | 完了タグ（`"SELECT <n>"`, `"INSERT 0 <n>"`, `"UPDATE <n>"`, `"DELETE <n>"`, `"CREATE TABLE"` 等）の返答 |
| `EmptyQueryResponse` | S -> C | `'I'` | 対応 | 空クエリに対する返答 |
| `Sync` | C -> S | `'S'` | 対応 | 拡張クエリパイプラインの区切り。現在のトランザクション状態に応じて `ReadyForQuery` を送信 |
| `Close` | C -> S | `'C'` | 対応 | ステートメントまたはポータルの解放 |
| `CloseComplete` | S -> C | `'3'` | 対応 | Close 完了の通知 |
| `Query` (Simple Query) | C -> S | `'Q'` | 対応 | 単一 SQL 文の即時実行。複数文（セミコロン区切り）は拒否 |
| `Terminate` | C -> S | `'X'` | 対応 | セッション終了、TCP 接続の正常クローズ |
| `ErrorResponse` | S -> C | `'E'` | 対応 | エラー通知（SQLSTATE、深刻度、安全なエラーメッセージ） |
| `NoticeResponse` | S -> C | `'N'` | 対応 | 警告等の通知 |
| `Flush` | C -> S | `'H'` | **未対応** | Sync を要求し、Flush 単独は明示的エラーまたは未対応扱い |
| `FunctionCall` | C -> S | `'F'` | **未対応** | 明示的エラー（`FeatureNotSupported`） |
| `COPY` 関連メッセージ | 両方向 | `'d'`, `'c'`, `'f'` 等 | **未対応** | 明示的エラー（`FeatureNotSupported`） |

---

## 決定 3: 上限（Bounds）とリソース管理

DoS 攻撃やメモリ枯渇を防ぐため、すべてのリソースに厳格な上限を設ける。上限超過は接続切断またはトランザクション中止とし、部分適用や暗黙の切り詰めを行わない。

| 項目 | 初版の上限値 | 超過時の挙動 |
|---|---|---|
| 同時クライアント接続数 | 64 | 新規接続を即座に拒否（FATAL: too many connections, SQLSTATE 53300） |
| 接続・認証タイムアウト | 30 秒 | 認証未完了接続を自動切断 |
| 1 クエリ実行タイムアウト | 60 秒（設定可能） | クエリを中止し、SQLSTATE 57014（query_canceled）を返答 |
| 受信フレーム長（最大メッセージ長） | 16 MiB | 接続を強制切断（過大フレームによるメモリ枯渇の防止） |
| SCRAM 認証メッセージ長 | 16 KiB | 認証エラーとして切断 |
| SQL 文の最大長 | 64 KiB | 構文エラー（SQLSTATE 42601）として拒否 |
| クエリパラメータ数上限 | 256 個（$1 〜 $256） | 構文/バインドエラー（SQLSTATE 54000）として拒否 |
| クエリパラメータ値の最大長 | 4096 byte | 型エラー/上限超過エラーとして拒否 |
| 識別子（表名・列名・索引名）長 | 63 byte | 構文エラー（SQLSTATE 42601）として拒否 |
| 1 クエリあたりの JOIN テーブル数 | 2 表（単一 INNER JOIN のみ） | 構文エラー（SQLSTATE 54000）として拒否 |
| 1 クエリでバッファする最大結果行サイズ | 16 MiB | 上限超過エラーとしてクエリ中止 |
| 保持可能なステートメント/ポータル数 | 1 接続あたり各 64 個 | 上限超過エラーとして新規登録拒否 |

P4-1 のストレージ上限（ページ数 2^24、表数・索引数各 1024、列数 64、行長 4096 byte、tx 変更 4096 ページ、WAL 64 MiB 等）もそのまま厳格に適用される。

---

## 決定 4: 同時実行とトランザクション制御

1. **セッションとストレージ核の直列化連携**:
   - P4-1（ADR 0045 決定 2）は、データベースを 1 プロセスが排他的に所有し、実行中トランザクションを 1 つに直列化するモデルを採用している。
   - 本サーバーはネットワーク上で複数のクライアント接続（最大 64）を同時に受け入れるが、SQL の実行およびトランザクション処理は、サーバー内部の直列実行キューまたはセマフォを介して P4-1 核へ委託する。
2. **自動コミット（Autocommit）と明示的トランザクション**:
   - `BEGIN` 外で実行される単一文（DDL, DML, DQL）は、自動的に独立したトランザクションとして開始・コミットされる。
   - `BEGIN` 文を受け取ったセッションは、P4-1 核のトランザクション所有権を排他的に確保する。
   - 他のセッションがその間にトランザクションを開始しようとした場合、キューで待機（待機タイムアウトあり）する。
   - `COMMIT` または `ROLLBACK`（あるいはクライアントの予期せぬ切断）により、トランザクション所有権は解放され、待機中のセッションに渡される。
3. **トランザクション中のエラー**:
   - トランザクション内で構文エラー、制約違反、上限超過が発生した場合、そのトランザクションは直ちに `In Error`（`ReadyForQuery` のステータス `'E'`）状態に遷移する。
   - エラー状態のトランザクションでは、以降のすべての DML/DQL は拒否（SQLSTATE 25P02: current transaction is aborted）され、受け付けるのは `ROLLBACK`（または `COMMIT` による暗黙ロールバック）のみとする。

---

## 脅威と対策（公開関門 G1）

| 脅威 | 対策・残る限界 |
|---|---|
| **SQL インジェクション** | パラメータ化クエリ（Parse/Bind）の利用を基本とし、SQL 文字列への値の展開・文字列置換をサーバー内部で絶対に行わない。純 Sigil の厳格な AST パーサーにより構文木を構築し、識別子やリテラルを厳格に検証する |
| **認証バイパス・SCRAM の脆弱性** | SCRAM-SHA-256 のみを許可し、平文や弱暗号へのダウングレードを拒絶する。ソルト付きハッシュのみを永続化し平文パスワードを保持しない。署名・検証には定数時間照合（ADR 0020/0024）を用い、タイミング攻撃を防止する |
| **DoS 攻撃・リソース枯渇** | 同時接続数（64）、最大フレーム長（16 MiB）、SQL 長（64 KiB）、パラメータ数（256）、パラメータ長（4096 byte）、タイムアウト（接続・認証 30 秒、クエリ 60 秒）による厳格な制限。巨大なクエリや過大割当て要求は即座に拒否する |
| **情報の漏洩（エラー・ログ）** | `ErrorResponse` には標準の SQLSTATE と固定の安全な要約メッセージのみを返し、サーバー内部のファイルパス、スタックトレース、他セッションの情報を絶対に含めない。クエリ内のパラメータ値や認証秘密をログに出力しない |
| **中間者攻撃・通信の盗聴** | 初版は平文通信を採用（ADR 0034 と整合）。「いまは外部公開しない」方針を徹底し、ローカルホストまたは隔離された信頼境界内（Docker ネットワーク等）でのみ運用する。外部ネットワークへの公開は行わない |
| **不正・破損メッセージによるクラッシュ** | 不正な長さヘッダ、切り詰められたフレーム、無効な UTF-8、未知のメッセージタイプに対して panic せず、安全にプロトコルエラーを返して該当接続を切断・クリーンアップする |

「いまは外部公開しない」方針（ADR 0044 決定 3）を維持する。本 ADR の承認および実装完了は、外部ネットワークへの公開を意味しない。

---

## 完了条件と検証方法

本 Issue（#578）の完了は、**ADR 0046 の起草 PR 作成**である。以降は別系統エージェントによるレビューとマージを経て、別リポジトリでの実装段階へ進む。

P4-2 データベースサーバーの実装完了判定には、以下のすべてを満たすことを必要とする。

1. **P1 クライアント（ADR 0034）との結合試験**:
   - P1 の `std.pg` クライアントおよび接続プール（`PgPool`）を用い、既存の `pg_*` 試験（単一文、パラメータ付きクエリ、トランザクション、制約違反、接続プール借出/返却/清掃）の接続先を本サーバーに差し替えて、全試験が正常にパスすること。
2. **PostgreSQL 公式イメージとの差分比較試験（Differential Testing）**:
   - CI 上の PostgreSQL（公式イメージ）と本サーバーの双方に対して、同一の SQL 文列（DDL、INSERT、UPDATE、DELETE、SELECT、INNER JOIN、集約、ORDER BY、LIMIT、BEGIN/COMMIT/ROLLBACK）を投入する。
   - 返却される行データ、列情報（RowDescription）、完了タグ（CommandComplete）、および制約違反や構文エラー時のエラー種別（SQLSTATE）が一致することを検証する。
3. **未対応機能の安全性検証**:
   - 本 ADR で「未対応」と定めた機能（NULL 値、外部結合、サブクエリ、未対応メッセージ種別等）が投入された際、暗黙の誤動作やクラッシュを起こさず、明示的エラー（`FeatureNotSupported` 等）を返し、セッション状態を安全に維持または終了すること。
4. **異常系・プロトコルファジング試験**:
   - 壊れたフレーム、切断、過大メッセージ、無効な SCRAM 署名、途中で中断したパケット列などを注入し、サーバープロセス全体が panic せず、該当接続のみを安全に切断・清掃することを検証する。
   - クライアント切断時に、保持されていた未コミットのトランザクションが自動ロールバックされ、ロックが速やかに解放されることを確認する。
5. **二重実行環境での一致**:
   - ネイティブコンパイル出力と SIR 評価器（`sigil run`）の両方で同じ試験セットを実行し、同一の結果となることを確認する。
6. **性能と上限の記録**:
   - スループット（QPS）、同時接続スケーラビリティ、クエリ遅延を測定・記録する（初版の合否基準とはしないが、ベースラインとして残す）。

---

## 正式判定の記録

2026-11-16 の所有者指示に従い、判定には typesafe-ai のみを用いる。

**起草時点では未実施である。** 起草環境（cloud / agy）には `TYPESAFE_API_KEY` が設定されておらず、通信が遮断されている（AGENTS.md の「キーが無い環境」。ADR 0042・0043・0044 起草時と同様）。そのため、本起草 PR では採用案を「判定を経ていない起草者の案」として明記し、以下に判定用の評価軸と問いを記録する。後続のレビューまたは見張りセッションにおいて正式判定を実施可能とする。

### 判定用の評価軸と問い（再現用）

- **評価軸（固定）**: Sigil は AI が実装し人間が仕様だけをレビューする原則（ADR 0001）。ADR 0005 の段階表（P4 データベース）、ADR 0044（P4 の範囲: 単一ノード関係 DB・PostgreSQL 通信方式 v3 部分集合・別リポジトリ実装・外部公開しない）、ADR 0045（P4-1 データベース核の serializable 直列実行・ACID・型と上限）、ADR 0034（PostgreSQL クライアント）との完全な整合性と対称性。

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間が仕様レビューする。ADR0005段階表、ADR0016のADR先行、ADR0044のP4範囲（単一ノード関係DB、PostgreSQL通信方式v3部分集合、SCRAM-SHA-256のみ、平文通信、仕様sigil/実装別repo、所有者がrepo作成、外部公開しない方針）、ADR0045のP4-1核（直列実行serializable、固定8192byteページ、B+tree、全ページredo WAL、no-steal、i64/bool/text/bytes、NULL未対応、主キー必須）、ADR0034（PGクライアントの拡張クエリ・SCRAM整合）を守る。所有者指示: Issue578のP4-2個別ADR0046と索引のみ、実装/新規Issue/マージ禁止。版取得方法はpinned_source（完全SHA+Cargo.lock固定ビルド）。案: SQLはDDL(CREATE/DROP TABLE/INDEX)、DML(INSERT/UPDATE/DELETE)、DQL(SELECT, 単一INNER JOIN, GROUP BY集約, ORDER BY, LIMIT)、TX(BEGIN/COMMIT/ROLLBACK)の部分集合とし、サブクエリや外部結合等は明示的未対応。通信はPG v3の拡張クエリ(Parse/Bind/Describe/Execute/Sync)と単一文Simple Query、認証はSCRAM-SHA-256のみ。接続は最大64、同時実行はP4-1核の直列実行キューへ委託しserializable維持。未対応機能は暗黙縮退せず明示エラー。完了条件はP1クライアント/プール試験差し替えパス、CI公式PostgreSQLとの差分比較、プロトコルファジング、両実行経路一致。",
  "questions": {
    "sql_scope": {
      "type": "choice",
      "criteria": {
        "bounded_subset": "基本DDL/DML/DQL(単一INNER JOIN・集約含む)/TXの限定集合とし未対応は明示エラー",
        "full_sql": "初版から外部結合・サブクエリ・ウィンドウ関数を含む全機能",
        "kv_only": "JOINや集約を省き主キーKV操作のみに縮退"
      }
    },
    "protocol_scope": {
      "type": "choice",
      "criteria": {
        "extended_and_simple": "拡張クエリ(Parse/Bind/Execute/Sync)標準+単一文Simple Query+SCRAM-SHA-256のみ",
        "simple_only": "Simple Queryのみに絞り拡張クエリを除外",
        "full_v3": "COPY・CancelRequest・FunctionCall等を含む全v3メッセージ"
      }
    },
    "concurrency": {
      "type": "choice",
      "criteria": {
        "queue_serialized": "複数接続を受け入れP4-1核の直列実行キューへ委託しserializable維持",
        "mvcc_server": "サーバー層で初版からMVCCを独自実装",
        "single_client": "同時に1クライアントのみ接続可能とする"
      }
    }
  }
}
```

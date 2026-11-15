# PostgreSQL CI（6-5f、Issue #510）

## 固定イメージ

`.github/workflows/postgres.yml` は Docker Hub の公式 `library/postgres` を使う。

- タグ: `postgres:17.6-bookworm`（PostgreSQL 17.6、Debian bookworm）。
- OCI index digest: `sha256:f3bd19c606e442c3d7bdfa8002e03fe260a1023351e0ea4598032022b68dd6e3`。
- Linux amd64 manifest: `sha256:45cd22f8d32e189d245403954882f88e7a8714301fda80dab6da90f1265b25a3`。
- 2026-11-16（JST）、Docker Hub Registry v2 の manifest 応答で確認した。

タグと index digest を同じイメージ参照に書き、タグの移動で内容が変わらないようにする。
更新時は Registry のタグに対する digest と amd64 manifest を再確認し、この記録と workflow を同じ PR で更新する。

## 使い捨て資格情報

ユーザーと DB は `sigil_ci`。パスワードは実行ごとに `openssl rand -hex 32` で生成し、GitHub のログマスクへ登録する。リポジトリや GitHub Secrets に固定値を置かず、シェルの環境変数から Docker へ渡す。コマンドのトレースや環境全体の出力は行わない。

ホスト・ローカル認証と保存するパスワードは SCRAM-SHA-256 に固定する。疎通確認の `psql` は環境変数を明示的に受け取る。DB コンテナは専用の CI ランナーに限って作り、永続データを再利用しない。終了・失敗時は `docker rm -f -v` で匿名データ volume も削除する。取消時も `always()` の回収処理を置き、最終的には使い捨てランナーごと破棄する。

## 隔離網

依存取得、Rust ビルド、イメージの pull を先に終わらせ、その後 `docker network create --internal` で専用 bridge を作る。PostgreSQL はその網だけに接続し、ホストへのポート公開、host network、他の網への接続をしない。網が internal であり、接続網数が1・公開ポート数が0であることも CI で検査する。

試験は `sudo nsenter --target <PostgreSQL PID> --net` で同じ network namespace に入り、数値アドレス `127.0.0.1` を使う。ファイルシステムとコンパイラはホストのものを使うため、隔離後のダウンロードは不要。DB と試験プロセスはこの内部網から外部へ出ない。GitHub のチェックアウト・ログ送信等を行うランナー本体まで通信を遮断する構成ではない。

## 実サーバー試験

所有者決定（2026-11-16 JST）により、6-5f に製品クライアントの実サーバー試験の追加を含める。

`pg_live_510` は製品 `std.db.pg` の Startup/SCRAM・query・pool を差し替えず、上記 PostgreSQL に接続する。評価器と native の 1・4 workers で以下を検査する。

- bool / int2 / int4 / int8 / text / varchar / bytea、空値・NULL・日本語と引用符を含むパラメーター、列情報と command / ReadyForQuery。
- 構文エラーの SQLSTATE `42601` と、その後の同一接続での query 成功、`BEGIN` / `COMMIT` / `ROLLBACK` の状態遷移。
- lease の返却・再取得で backend PID が一致すること、`DISCARD ALL` による設定消去と `bytea_output=hex` の再設定。
- トランザクション中の lease を再利用せず、接続容量を回収して別の backend PID で再接続できること。

`pg_query_492` / `pg_pool_499` の模擬 TCP 試験は、不正フレーム・切断・取消などの障害注入のため保持する。CI は模擬試験と実サーバー試験を両方実行する。`psql SELECT 1` は起動の疎通確認であり、製品試験の代用にはしない。

実サーバー試験は通常の `cargo test` では明示的な ignored とし、postgres ジョブでは必ず `--ignored` を指定する。`POSTGRES_USER` / `POSTGRES_DB` / `POSTGRES_PASSWORD` / `POSTGRES_PORT` が不足すれば失敗する。パスワードはソースやプロセス引数に埋め込まず、`sudo --preserve-env` で試験へ渡す。

手元で使い捨て PostgreSQL を用意した場合は、上記環境変数を設定して `cargo test --locked --test pg_live_510 -- --ignored --test-threads=1` で実行できる。

既知の #484（評価器 PBKDF2 の接続期限超過）を隠すための除外は行わず、評価器の実 SCRAM も必須とする。認証方式・SCRAM iteration・製品期限を緩めない。環境依存の性能問題等が再現した場合は失敗として記録し、6-5f の範囲外の製品本体は修正しない。

workflow の実装権限と AGENTS.md の表の変更は [Issue #510 の所有者ルール変更記録](https://github.com/goodnasubi/sigil/issues/510#issuecomment-6895892627) に基づく。ADR 0034 の版固定・使い捨て資格情報・内部網の判断は変更しない。

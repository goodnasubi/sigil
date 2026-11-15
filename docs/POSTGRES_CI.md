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

## 実行範囲と未充足の前提

CI は `pg_isready` と SCRAM を使う `psql SELECT 1` で実 PostgreSQL の起動・認証・疎通を検査する。既存 `pg_query_492`（6-5d）と `pg_pool_499`（6-5e）も内容を変更せず隔離網で実行する。

ただし、両テストファイルは固定応答の模擬 TCP 相手を自前で起動する試験であり、実 PostgreSQL を接続先に選ぶ入口は存在しない。PR #498・#506 でも実サーバー試験は未提供のまま後続へ送られている。したがって、この workflow だけでは「Sigil の製品クライアントによる実サーバー試験が通る」という Issue #510 の完了条件を満たさない。`psql` の成功を Sigil クライアントの成功とは扱わない。

実サーバー試験自体の追加・変更は今回の明示範囲外なので、別途 6-5d・6-5e の担当範囲で入口と試験を用意する必要がある。既知の #484（評価器 PBKDF2 の接続期限超過）と PR #506 に記録された製品 Startup/query 試験の既存失敗も修正しない。

workflow の実装権限と AGENTS.md の表の変更は [Issue #510 の所有者ルール変更記録](https://github.com/goodnasubi/sigil/issues/510#issuecomment-6895892627) に基づく。ADR 0034 の版固定・使い捨て資格情報・内部網の判断は変更しない。

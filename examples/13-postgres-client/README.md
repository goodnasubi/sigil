# PostgreSQL クライアントの最小サンプル

`postgres_client.sigil`（モジュール `examples.postgres_client`）は、PostgreSQL クライアント（`std.pg`）を用いた直接接続・単一文クエリ・接続プールの最小サンプルです。

## 構成と要点

- **設定の構築**: `pg.config("127.0.0.1", port, "user", "database", Secret.from_bytes("pencil".as_bytes()))` により、パスワードを `Secret` で保護して接続設定を生成します。
- **直接接続と単一文クエリ**: `pg.connect(cfg, &connect_limits)` で Startup と SCRAM-SHA-256 認証を行い、`conn.query` でパラメータ付き単一文クエリを実行します。接続は単一の fd を排他所有し、タイムアウトや同期不明時は所有型の破棄により確実に閉じられます。
- **接続プールと lease**: `pg.pool(cfg, &pool_limits)` で接続プールを初期化し、`pool.acquire()` で排他借出の `PgLease` を取得します。`lease.query` で文を実行した後、`lease.release()` により清掃文（`DISCARD ALL`）を発行して接続をアイドル状態に戻します。
- **信頼基盤**: プロトコル解釈・型変換・SCRAM 認証・プール管理はすべて `trust none` の Sigil 実装（`std.pg`, `std.pg_scram`）で動作します。

## 実行のしかた

サンプルのモジュール名（`examples.postgres_client`）に合わせたパスに配置して実行します。

```sh
root=$(mktemp -d)
mkdir -p "$root/examples"
cp examples/13-postgres-client/postgres_client.sigil "$root/examples/postgres_client.sigil"

# 検査
sigil check --root "$root" "$root/examples/postgres_client.sigil"

# 実行（実 PostgreSQL またはモックが待ち受けるポート番号を渡す）
sigil run --root "$root" "$root/examples/postgres_client.sigil" main 5432

# ネイティブビルド
sigil build --root "$root" "$root/examples/postgres_client.sigil" main -o /tmp/postgres_client_example
/tmp/postgres_client_example 5432

rm -rf "$root"
```

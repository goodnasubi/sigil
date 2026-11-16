# 例 06: P1 完了サンプルのサービス（ADR 0018 7-1）

HTTP/1.1 の要求を受け付け、篩型による入力検証を行い、HMAC-SHA256 Bearer トークン認証を通した要求のみに PostgreSQL への読み書きを提供する、P1（段階 7-1）の統合サービスです。

## 概要

- **HTTP サーバー**: `std.http` による HTTP/1.1 サーバー。持続接続・パイプラインに対応し、`task.roots` で各接続を独立したルートとして隔離します。
- **入力検証**: 篩型（`type ItemId = Int where self > 0 && self <= 10000;`）により、アイテム識別子の範囲を厳格に検証します。範囲外や不正形式は `400 Bad Request` を返します。
- **Bearer 認証**: `std.web` の `verify_bearer` を用いた HMAC-SHA256 署名検証。有効なトークンを持たない要求は `401 Unauthorized` を返します。
- **PostgreSQL 読み書き**: `std.pg` を用いて、認証済み要求に対して PostgreSQL（SCRAM-SHA-256 認証）へのアイテム保存（`POST /items`）および取得（`GET /items`）を行います。
- **契約違反の隔離**: 故意の契約違反エンドポイント（`/fault`）で事前条件違反が発生しても、`http.abort_conn` によりその接続のみが `500` で安全に終了し、サーバープロセスや他の接続の処理は継続します。

## エンドポイント

| メソッド | パス | 認証 | 説明 |
|---|---|---|---|
| `GET` | `/health` | 不要 | 稼働確認。`200 OK`（本文 `"ok"`） |
| `POST` | `/items?id=<id>&name=<name>` | 必須 | アイテムの保存。`id` は `1..=10000` の整数。成功時 `200 OK`（本文 `"stored"`） |
| `GET` | `/items?id=<id>` | 必須 | アイテムの取得。成功時 `200 OK`（本文に対象の `name`）、未登録時は `404 Not Found` |
| `GET` | `/fault` | 不要 | 契約違反のテスト。事前条件違反を発生させ `500 Internal Server Error` で終了 |

## 設定（環境変数）

| 変数名 | 既定値 | 説明 |
|---|---|---|
| `PORT` | `0` | 待ち受けポート（`0` は OS 空きポート自動割り当て） |
| `MAX_CONNS` | `100` | 同時受け付け接続数の上限 |
| `POSTGRES_HOST` | `127.0.0.1` | PostgreSQL ホスト |
| `POSTGRES_PORT` | `5432` | PostgreSQL ポート番号 |
| `POSTGRES_USER` | `sigil_ci` | PostgreSQL ユーザー名 |
| `POSTGRES_DB` | `sigil_ci` | PostgreSQL データベース名 |
| `POSTGRES_PASSWORD` | `""` | PostgreSQL パスワード（SCRAM-SHA-256） |
| `SIGIL_AUTH_HMAC_KEY_HEX` | （テスト用鍵） | HMAC-SHA256 秘密鍵（64文字 16進数） |
| `SIGIL_AUTH_AUDIENCE` | `sigil.api` | 認証の対象サービス識別子（Audience） |

## 実行方法

```sh
# 検査
sigil check --root examples/06-service examples/06-service/main.sigil

# 実行
PORT=8080 POSTGRES_PORT=5432 sigil run --root examples/06-service examples/06-service/main.sigil main
```

## 自動テスト

`tests/service_06_integration.rs` により、コンテナで PostgreSQL を起動して実サーバーに対する統合テストを実行します（CI では postgres / docker 環境で自動実行）。

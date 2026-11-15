# 起動時のベアラー認証設定

ADR 0037 / 6-6c の最小例。一接続を処理して終了する。`/private` は認証必須、`/public` は明示公開で identity を付けない。実際の運用では例10の受付・停止処理と組み合わせる。

運用側で暗号乱数から生成した32バイトを小文字hex64桁として `SIGIL_AUTH_HMAC_KEY_HEX` に、サービス識別子を `SIGIL_AUTH_AUDIENCE` に設定する。鍵の既定値・引数・発行機能はない。実鍵はソースやコマンド履歴に残さない。

```sh
sigil run --root examples/12-bearer-auth examples/12-bearer-auth/main.sigil main
```

`load_bearer_config`（env）と `into_verifier`（clock）の両方が成功した後にだけ待受を開始し、標準出力に一時ポートを表示する。環境は起動時に各一回だけ読む。設定は検証器のmoveクロージャが所有し、`SyncShared<Router>` を読み出して要求を処理する。保護経路の要求ごとの時計取得は一回。

TLS は前段終端。トークンの発行はこの例に含めない。OS環境や既存Stringの全コピー消去は保証しない。詳細は [実装記録](../../docs/authentication/509-config-web.md)。

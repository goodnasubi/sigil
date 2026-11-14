# ソケットなしの Web ルーター

`main.sigil` は `SyncShared<Router>` から経路表を読み、パス変数・クエリ・篩型・認証・前後フックを通して `hello Sigil` を返す。待ち受けや停止処理は含まない。

```sh
cargo run -- run --root examples/11-web-router examples/11-web-router/main.sigil main
cargo run -- build --root examples/11-web-router examples/11-web-router/main.sigil main -o /tmp/sigil-web-example
/tmp/sigil-web-example
```

- `route` は認証必須。検証器が未設定または `Err` の場合は、フックを呼ばず固定の401を返す。`public_route` の経路だけ認証を外し、`public_routes()` で `METHOD PATH` を列挙できる。
- 関数を格納する箱は `RouteHandler { handle: user }`、`BeforeHook { run: before }`、`AfterHook { run: after }`、`AuthVerifier { verify: demo_auth }` として構築する。許可される効果の上限は `db, console, clock, random, env, task.spawn, task.suspend, task.share`。`dispatch` にもこの上限が伝播する。
- 前フックは登録順、後フックは逆順。前フックの `Some(response)` は後続前フックとハンドラを省略し、後フックは実行する。入力検査・認証失敗・404・405にはフックを実行しない。
- パスは一度復号する。ルート `/` 以外の空区間・末尾 `/`、`.`・`..`、逆スラッシュ、制御文字、符号化された区切りを400にする。経路パターンは復号後の文字列で、変数は `:name` の一セグメント。同一メソッドで重なる経路は登録時に拒否する。
- クエリは空欄・重複を含めて既定100欄まで。`+` は空白に復号する。`query` / `query_int` は欠落・重複を400にする。`with_limits(&config.app_limits())` は検証済みのクエリ上限を反映する。
- `web.validate(T.check(value))` は篩型の失敗を400に写す。`param_int` / `query_int`、`json_string` / `json_int` と組み合わせられる。JSON補助はトップレベルのオブジェクトを読み、既存 `std.json` と同じ深さ64で構文・重複キー・型を検査する。
- `AuthIdentity.subject()` は検証器が返した識別子。`header` が返す `X-Forwarded-*` 等は未検証の生値で、認証には使わない。この例の固定トークンは動作確認専用。

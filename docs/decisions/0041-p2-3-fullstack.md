# 0041: P2-3 フルスタック（ブラウザ／サーバー分離・型付き通信・静的配信）

- **状態**: 提案（typesafe-ai の正式判定を末尾に記録。別系統のレビュー前）
- **日付**: 2026-11-17（JST）
- **対象**: [Issue #561](https://github.com/goodnasubi/sigil/issues/561)、レーン E

## 背景

[ADR 0038](0038-p2-scope.md) の三番目の段 P2-3 を、[ADR 0039](0039-p2-1-javascript-output.md) の JavaScript 出力、[ADR 0040](0040-p2-2-ui-model.md) の UI 記述モデル、および P1 のネイティブサーバー基盤（[ADR 0032](0032-p1-http-server.md)・[ADR 0033](0033-p1-framework.md)・[ADR 0037](0037-p1-authentication.md)）の上に具体化する。AI が実装し、人間が仕様だけをレビューする原則（[ADR 0001](0001-vision-and-scope.md)）に従い、ブラウザ／サーバーの境界、型付き要求応答、静的資産配信、および安全保護の規則を明確にする。

起草の基準は `main` の `8c00f60`。0041 は main のファイル・索引およびオープン PR で未使用であることを確認した。本 PR は本 ADR の新規作成と索引追加のみを行う。以下は将来の実装に向けた決定であり、現行の処理系や `sigil spec` に追加された機能を記述するものではない。

## 選択肢と決定

| 判断 | 採用する案 | 他案を採用しない理由 |
|---|---|---|
| プロジェクト構造 | 一つのプロジェクトで `targets browser`／`server`／`bare` を静的検査で厳格分離 | 実行時の動的分岐や環境変数による分離は配信物へのサーバーコード混入を招く。別リポジトリ分割は型と契約の二重管理を生む |
| 通信プロトコル | 同一オリジン HTTP/1.1 の型付き要求応答（POST/GET）。JSON・篩型・上限による境界検証 | 初版から双方向 WebSocket・汎用 RPC・SSR・hydration を必須にすると範囲が過大になり検証が困難になる |
| 静的資産配信 | P1 のネイティブ出力（`std.http`／`std.web`）が HTML／JS をパストラバーサル防止・安全ヘッダ付きで同一オリジン配信 | 外部サーバー（Nginx 等）や Node.js 配信を前提にすると配布単位が単一コンテナで完結せず P1 成果物を生かせない |
| 安全境界 | 受信データは一切信頼せず入口でサイズ・深さ・型・篩型を検査。不正・認証失敗・上限超過は固定エラーで即時拒否。サーバー秘密を配信物から排除 | クライアント側の Sigil UI 検証を信用してサーバー検証を省略すると境界防御が崩れる。診断なしの TCP 切断のみでは原因調査ができない |
| UI との接続 | UI の `update` は純粋に保ち、非同期 HTTP 要求は明示的な効果呼び出しから完了時に結果イベント `E` を UI ループへ送る | `view` 内で直接非同期待機を行うと純粋関数と再描画の規律が崩れる。大域可変変数によるポーリングは効果と所有権を追えなくする |

正式判定の確率は設計の正しさの証明ではない。採用理由は、既存 ADR の型・効果・所有権の規律を維持し、ブラウザ配信物への秘密漏洩を防ぎ、単一コンテナで検証可能な最小のフルスタックを成立させることにある。

## 決定 1: プロジェクト構造と targets による静的境界

一つのプロジェクト内でブラウザコード・サーバーコード・共有コードを記述し、コンパイラが静的に出力先と権限を分離する。

### ターゲットの役割分担

1. `targets bare`（共有コード）:
   - ブラウザとサーバーの双方が依存できる純粋な共通部品。
   - 要求・応答のデータ構造（DTO）、篩型定義、純粋なバリデーション関数、共有ドメインロジック。
   - I/O 効果（`fs.*`, `net.*`, `env`, `dom` 等）を持つ関数、サーバー専用の秘密型、ホスト資源型を含めてはならない。
2. `targets browser`（クライアントコード）:
   - ブラウザ上で実行されるコード。[ADR 0039](0039-p2-1-javascript-output.md) の JS 出力器により `.mjs` にコンパイルされる。
   - [ADR 0040](0040-p2-2-ui-model.md) の UI 定義（`UiNode`, `UiProgram` 等）、クライアント状態、UI 更新ロジック、クライアント通信呼び出し。
   - 許可される効果は `none`、`console`、`dom`、および本 ADR で導入する `net.client` のみ。
3. `targets server`（サーバーコード）:
   - サーバー環境（Linux x86_64）で実行されるコード。P1 のネイティブコンパイラにより単一の ELF バイナリにコンパイルされる。
   - HTTP 要求ハンドラ、ルーティング、DB 接続（`std.db.postgres`）、設定読込（`env`）、認証鍵の管理（`Secret`）、静的資産配信。
   - サーバー専用効果（`fs.*`, `net.server`, `env`, `clock`, `task.*` 等）を使用できる。

### 静的境界の強制と秘密漏洩防止

コンパイラはブラウザ向け成果物の生成前に次の検証を走査し、違反時は生成を即時拒否する。

- **依存閉包の検査**: `targets browser` のモジュールまたはその入口から参照されるすべての依存関係において、`targets server` のモジュールへの直接・間接の依存（`use`）を一切禁止する。
- **効果の隔離**: サーバー専用効果（`fs.*`, `net.server`, `env` 等）を要求する関数がブラウザ向け成果物に含まれていないことを検査する。
- **秘密値の混入排除**: [ADR 0024](0024-crypto-primitives.md) の `Secret<T>` や [ADR 0037](0037-p1-authentication.md) の認証鍵（`SIGIL_AUTH_HMAC_KEY_HEX`）、データベース接続情報等、サーバー専用の環境設定・型がブラウザ成果物に埋め込まれないことを静的に保証する。ビルド出力時にも生成された `.mjs` を走査し、混入を検知した場合はビルドを失敗させる。

## 決定 2: 型付き要求応答と同一オリジン HTTP

ブラウザとサーバー間の通信は、同一オリジン（同一のホスト・ポート）に対する HTTP/1.1 を用いる。通常のローカル関数呼び出しに偽装せず、通信の失敗可能性と効果を型で明示する。

### エンドポイントの宣言

通信は共有コード（`targets bare`）で定義された型付きエンドポイントを介して行う。

```sigil
enum HttpMethod {
  Get,
  Post,
}

struct Endpoint<Req, Resp> {
  path: String,
  method: HttpMethod,
}
```

- `Req` および `Resp` は共有モジュールで定義された所有型とする。借用、関数型、およびホスト資源型（`UiMount`, `TcpConn` 等）を含めてはならない。
- ネットワーク越しに関数ポインタやクロージャを送信する「任意の RPC」は初版に含めない。
- 双方向通信（WebSocket）、サーバーサイドレンダリング（SSR）、およびクライアント hydration は P2-3 の範囲外とし、P2 の初版では扱わない。

### 通信形式と境界検証

- ワイヤフォーマットは厳格な JSON（RFC 8259）に固定する。
- 受信データは構造体へ復元する前に次の上限を検査する:
  - リクエストヘッダ総長: 上限 8 KiB（ADR 0032 準拠）。
  - リクエスト本文サイズ: エンドポイントごとの上限（既定は 64 KiB、最大でも 1 MiB を超えない）。
  - JSON ネストの深さ: 上限 128（ADR 0033 準拠）。
- 復元されたデータに対して篩型の検査（`T.check(...)`）を実行し、静的な表明と実行時契約の整合性を保証する。

## 決定 3: 境界の検証と安全保護（Fail-Closed）

「クライアント側の Sigil UI で入力検証されている」ことを前提とせず、サーバー境界ではすべての要求を受信時に再検証する。

### サーバー境界での検証パイプライン

1. **トランスポート・ヘッダ検査**:
   - 不正な HTTP 構文、過大ヘッダ、不正な Content-Length は [ADR 0032](0032-p1-http-server.md) に従い即座に 400 Bad Request または接続切断（`abort_conn`）とする。
2. **ルーティング**:
   - 登録されていないパス・未許可のメソッドに対する要求は即座に 404 Not Found または 405 Method Not Allowed を返す。
3. **認証検査**:
   - 認証が必要なエンドポイントでは、[ADR 0037](0037-p1-authentication.md) の HMAC ベアラートークン（`Authorization: Bearer sigil1.<payload>.<tag>`）を検証する。
   - 形式不正、署名不一致、期限切れ（`now >= exp`）、対象外の audience はすべて一律で固定の 401 Unauthorized（`WWW-Authenticate: Bearer`）とし、失敗の内部理由や秘密情報を応答に含めない。
4. **入力検証**:
   - JSON の解釈失敗、深さ超過は 400 Bad Request。
   - スキーマ不一致、未定義フィールドの存在、篩型（`T.check`）の失敗は 422 Unprocessable Content を返す。
5. **応答の保護**:
   - 正常応答（200 OK）には `Content-Type: application/json; charset=utf-8`、`X-Content-Type-Options: nosniff`、`Cache-Control: no-store` を付与する。
   - 内部エラー（DB 接続失敗等）は 500 Internal Server Error とし、スタックトレースや内部エラーメッセージをクライアントへ返さない。

## 決定 4: 静的資産配信と P1 ネイティブサーバー

サーバーは外部の配信サーバー（Nginx 等）や Node.js を介さず、P1 ネイティブバイナリ自身が静的資産を同一オリジンで配信する。

### 静的配信の規則

- **配信対象**: ビルド時に生成されたブラウザ成果物（HTML、JS、CSS、SVG 等）。
- **パストラバーサルの完全遮断**:
  - 要求パスに `..`、空セグメント、ヌルバイト（`%00`）、またはパーセント符号化された制御文字が含まれる場合は即座に 400 Bad Request または 404 Not Found として拒否する。
  - 静的資産ディレクトリの外への脱出を静的・動的に完全に防止する。
- **MIME タイプとセキュリティヘッダ**:
  - `.html`: `text/html; charset=utf-8`
  - `.mjs` / `.js`: `text/javascript; charset=utf-8`
  - `.css`: `text/css; charset=utf-8`
  - `.svg`: `image/svg+xml`
  - 全静的応答に `X-Content-Type-Options: nosniff` および `Content-Security-Policy: default-src 'self'; script-src 'self'; style-src 'self'; object-src 'none';` を付与し、XSS や外部リソース読み込みを制限する。

## 決定 5: 構文・型・標準操作・ABI・ホスト呼び出しの許可一覧

[ADR 0039](0039-p2-1-javascript-output.md) および [ADR 0040](0040-p2-2-ui-model.md) と同じ形式で、P2-3 の初版に含まれる構文・型・標準操作を列挙する。

### 構文と型の許可一覧

| 区分 | 初版に含むもの |
|---|---|
| 宣言 | `module`・`targets`（`browser`／`server`／`bare`）・`use`・公開範囲、関数仕様四節、構造体・列挙型・組、総称型／総称関数、篩型、トレイトと静的解決 |
| クライアント通信型 | `Endpoint<Req, Resp>`、`HttpMethod`、`ClientError`（`NetworkError`／`StatusError(Int)`／`DecodeError` を含む閉じた列挙型） |
| サーバー通信型 | `Route`、`Router`、`HttpRequest`、`HttpResponse`、`AppError`、`AuthIdentity`（ADR 0033 / 0037 準拠） |
| UI 統合 | [ADR 0040](0040-p2-2-ui-model.md) の全 UI 型（`UiNode`, `UiAttr`, `UiTag`, `UiRoot`, `UiMount`, `UiProgram` 等） |
| 効果 | ブラウザ: `none`, `console`, `dom`, `net.client`。サーバー: `net.server`, `fs.*`, `env`, `clock`, `task.*`, `crypto.*`, `db` |

### 標準操作の許可一覧

#### クライアント側（`std.web.client`、関数の公開名前空間は `client`）

ブラウザ側での非同期 HTTP 要求は、UI の宣言的更新モデルを崩さないよう、明示的なディスパッチとして提供する。

| 操作 | シグネチャ | 効果・条件 |
|---|---|---|
| 型付き要求 | `client.fetch<Req, Resp, E>(endpoint: &Endpoint<Req, Resp>, req: Req, to_event: fn(Result<Resp, ClientError>) -> E) -> Unit` | `net.client`。非同期要求を発行し、完了時に生成したイベント `E` を所有されたマウントのイベントキューへ非同期に投入する |
| エンドポイント構築 | `client.endpoint<Req, Resp>(path: String, method: HttpMethod) -> Endpoint<Req, Resp>` | `none`。パスとメソッドを束ねる |

`to_event` 関数は `fn(...) -> E effects none` であり、結果を UI のイベント型 `E` に変換する純粋な写像とする。`update` 関数自体は純粋なままとし、副作用（要求発行）は明示的な効果呼び出し文として分離する。

#### サーバー側（`std.web.server`、関数の公開名前空間は `server`）

| 操作 | シグネチャ | 効果・条件 |
|---|---|---|
| ルーター生成 | `server.router() -> Router` | `none`。空のルーターを作成 |
| ハンドラ登録 | `server.handle<Req, Resp>(router: &mut Router, ep: Endpoint<Req, Resp>, handler: fn(Req) -> Result<Resp, AppError>) -> Unit` | `none`。型付きハンドラを登録 |
| 認証付き登録 | `server.handle_auth<Req, Resp>(router: &mut Router, ep: Endpoint<Req, Resp>, handler: fn(Req, AuthIdentity) -> Result<Resp, AppError>) -> Unit` | `none`。Bearer 検証成功時のみハンドラを実行 |
| 静的配信登録 | `server.static_dir(router: &mut Router, url_prefix: String, fs_path: String) -> Unit` | `none`。静的アセットディレクトリを紐付け |
| サーバー起動 | `server.serve(router: Router, port: Int) -> Result<Unit, ServerError>` | `net.server, clock, task.spawn, task.suspend, fs.read, env`。P1 ネイティブサーバーを起動 |

### 値・文字列の ABI と破棄

1. **数値と文字列**:
   - ブラウザ側: [ADR 0039](0039-p2-1-javascript-output.md) 決定 2 に従い、`Int` は `BigInt`、`Float` は `Number`、`String` は UTF-8 不変文字列として扱う。
   - サーバー側: x86_64 ネイティブ ABI（64bit 符号付き整数、IEEE 754 倍精度浮動小数、連続バイト列）。
   - 通信境界: JSON への直列化において、数値は RFC 8259 の数値範囲を遵守し、`Int` の絶対値が 2^53 - 1 を超える場合は文字列形式または範囲外エラーとする。
2. **所有値の破棄**:
   - ブラウザ側: 要求完了時およびイベントディスパッチ完了時に、SIR 評価器と同じ順序で明示的に破棄を実行する。
   - サーバー側: 要求ごとに割り当てられたスコープアリーナ（[ADR 0029](0029-scope-arena-allocation.md)）からメモリを確保し、要求処理完了時にアリーナ全体を一括破棄する。

## 決定 6: CLI と生成物

### ビルドコマンド

フルスタックプロジェクトのビルドは次の CLI で行う。

```bash
sigil build --target fullstack
```

ビルドプロセスは次の順序で決定論的に実行される:

1. **共通検証**: `bare` モジュールの構文・型・契約・篩型を検証する。
2. **クライアントビルド**: `browser` コードを検証し、[ADR 0039](0039-p2-1-javascript-output.md) の JS 出力器により `dist/client/app.mjs` を生成する。あわせて最小の `dist/client/index.html`（`app.mjs` を ES module として読み込む）を生成する。
3. **サーバービルド**: `server` コードを検証し、P1 ネイティブコンパイラにより `dist/server/app`（ELF 実行可能バイナリ）を生成する。
4. **生成後走査（秘密検査）**: `dist/client/` 内の全生成物を走査し、サーバー専用モジュール名・環境変数名・秘密鍵パターンが含まれていないことを自動検証する。違反時は成果物を削除してエラーコードで終了する。

### 生成前診断

次の違反を検知した場合、ファイルを書き出さずにエラーを出力する:

- `targets browser` のコードから `targets server` モジュールへの直接・間接の依存。
- `targets browser` でのサーバー専用効果（`fs.*`, `net.server`, `env` 等）の使用。
- `Endpoint<Req, Resp>` における `Req` または `Resp` への非送信可能型（借用、関数型、ホスト資源型）の指定。
- 登録されていないエンドポイントへのクライアント要求発行。

## 決定 7: P2-3 の完了の確かめ方

[ADR 0038](0038-p2-scope.md) 決定 1 に定められた検証基準に基づき、P2-3 の実装完了を次の項目で確かめる。

1. **正常系の疎通**:
   - Sigil UI（フォーム入力またはボタン）の操作から型付き要求を発行。
   - P1 コンテナ内のネイティブサーバーが要求を受信し、境界検証を経てハンドラを実行。
   - 型付き応答がブラウザへ返り、UI の `update` を経て画面が正しく更新されること。
2. **境界防御と拒否の検証**:
   - 不正な入力（型不一致、篩型の違反）を含む要求が、400 Bad Request または 422 Unprocessable Content で拒否されること。
   - 認証失敗（無効な Bearer トークン、期限切れ、異なる audience）の要求が、401 Unauthorized で拒否され、秘密や詳細が漏洩しないこと。
   - 上限超過（サイズ超過、ネスト超過）が、413 Payload Too Large または 400 Bad Request で拒否されること。
3. **秘密混入防止の検証**:
   - 生成された `dist/client/app.mjs` および静的資産ファイルを自動走査し、サーバー専用コード、DB 接続文字列、HMAC 署名鍵などの秘密情報が一切含まれていないことを自動テストで確認すること。
4. **P1 コンテナでの実行確認**:
   - P1 の最小 Docker コンテナ内でサーバーを起動し、実ブラウザ（Chromium／Firefox／WebKit）または自動化クライアントから同一オリジンで静的配信および API 通信が完結することを確認する。

本 Issue の完了条件は ADR 起草 PR の作成までであり、上記の実装・コンテナ作成・テストは本 PR には含めず、マージ後の実装 Issue で行う。

## 影響

- P1 の `std.http`、`std.web`、および認証基盤をそのまま再利用するため、サーバー側の仕様追加を最小限に抑えられる。
- P2-1（JS 出力）および P2-2（UI モデル）の上に本段が乗ることで、後続の P2-4（Wasm 出力）では通信基盤をそのまま共有できる。
- P2-5（Sigil 製 Web IDE）は、本 ADR で定義されたフルスタック基盤（ネイティブサーバーによる仕様データ配信と UI での表示）を利用して構築される。
- 本 ADR は新たな外部依存や信頼基盤の追加を承認するものではない。P1 の外部公開前の残課題（Issue #547〜#552）は本段でも独立して管理される。

## 正式判定の記録

2026-11-16 の所有者指示に従い、Gemini Decisions は使わず typesafe-ai のみを使用する。

2026-11-17（JST）、agy（Gemini 系統）から `jev-1.13.0` を一度呼び出した。ProjectStructure=strict_targets、Protocol=typed_same_origin、AssetServing=p1_native_server、SecurityBoundary=fail_closed_bounded、UiBridge=event_dispatched_effect が選ばれ、全判断点において確率 1.00、confidence 1.00 であった。判断の分裂はなく、上記案を採用する。

### 入力

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間は仕様だけをレビューする言語（ADR 0001）。ADR 0005のP2目標（WebフルスタックG1・IDE G8）、ADR 0038のP2段組み（P2-1 JS→P2-2 UI→P2-3 フルスタック→P2-4 Wasm→P2-5 IDE）を遵守する。ADR 0038決定4により、P2-3は一つのプロジェクトでのbrowser／server／commonの静的分離、型付き要求応答、同一オリジンHTTP、静的資産配信と成果物生成、サーバーはP1のネイティブ出力とし、境界の検証（要求・応答の型と値の検査、入力不正・認証失敗・上限超過の拒否）、サーバー専用の秘密をブラウザ配信物に入れない、SSR・hydration・WebSocket・任意RPCは初版に含めない。ADR 0004の効果完全一致、ADR 0027の最小信頼基盤・明示的権限・曖昧さ削減、ADR 0033のstd.web・既定拒否のルーター・上限保護、ADR 0037のHMAC署名Bearer認証（発行なし・鍵は環境変数・ブラウザに鍵を渡さない）、ADR 0039のJS出力（Node.js不要のES module）、ADR 0040の宣言UI（純粋view・明示的update・所有UiMount）を再利用し、言語の意味・契約・所有権・効果検査を弱めない。P2-3の完了の確かめ方に沿い、P2-1のADR 0039と同じ形で具体的な構文・型・標準操作・数値表現・文字列ABI・所有値の破棄・ホスト呼び出し・CLIを列挙する。",
  "questions": {
    "ProjectStructure": {
      "type": "choice",
      "criteria": {
        "strict_targets": "一つのプロジェクトでtargets browser/server/bareを静的検査で厳格分離。ブラウザ配信物へのサーバーコード・秘密値の混入をビルド時に排除",
        "runtime_split": "実行時に環境変数や動的分岐で切り替え、単一成果物とする",
        "separate_repos": "別プロジェクト・リポジトリとし、型定義を手動またはコード生成で二重管理する"
      }
    },
    "Protocol": {
      "type": "choice",
      "criteria": {
        "typed_same_origin": "同一オリジンHTTP/1.1の型付き要求応答（POST/GET）。JSONと篩型・サイズ上限で境界検証。SSR/hydration/WebSocket/任意RPCは含めない",
        "full_rpc_ws": "双方向WebSocket、汎用RPC、サーバー関数の直接呼び出しを初版から導入する",
        "ssr_hydration": "サーバーサイドレンダリング（SSR）とクライアントhydrationを初版必須にする"
      }
    },
    "AssetServing": {
      "type": "choice",
      "criteria": {
        "p1_native_server": "P1のネイティブ出力（std.http/std.web）が静的資産（HTML/JS）を厳格なパストラバーサル防止・MIME・セキュリティヘッダ付きで同一オリジン配信",
        "separate_server": "外部Webサーバー（Nginx等）やNode.jsを必須とし、P1サーバーはAPI専用とする",
        "unbounded_fs": "任意のローカルファイルシステム参照と動的走査を許す汎用配信"
      }
    },
    "SecurityBoundary": {
      "type": "choice",
      "criteria": {
        "fail_closed_bounded": "受信データは一切信頼せず入口でサイズ・深さ・型・篩型を検査。不正・認証失敗・上限超過は固定エラーで即時拒否。サーバー秘密はSecret保持し配信物から完全排除",
        "client_trust": "クライアントUIで検証済みとしてサーバー側検証を一部省略する",
        "opaque_abort_only": "診断・HTTPエラー応答を返さずTCP切断のみを行う"
      }
    },
    "UiBridge": {
      "type": "choice",
      "criteria": {
        "event_dispatched_effect": "UIのupdateは純粋に保ち、非同期HTTP要求は明示的な効果呼び出しから完了時に結果イベントEをUIループへ送る",
        "async_view": "view関数内で直接fetchを呼び出し非同期待機する",
        "ambient_state": "大域可変変数にサーバー応答を書き込みUIがポーリングする"
      }
    }
  }
}
```

### 応答

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "ProjectStructure": {
      "type": "choice",
      "choice": "strict_targets",
      "confidence": 1.0,
      "probabilities": {
        "strict_targets": 1.0,
        "runtime_split": 0.0,
        "separate_repos": 0.0
      }
    },
    "Protocol": {
      "type": "choice",
      "choice": "typed_same_origin",
      "confidence": 1.0,
      "probabilities": {
        "typed_same_origin": 1.0,
        "full_rpc_ws": 0.0,
        "ssr_hydration": 0.0
      }
    },
    "AssetServing": {
      "type": "choice",
      "choice": "p1_native_server",
      "confidence": 1.0,
      "probabilities": {
        "p1_native_server": 1.0,
        "separate_server": 0.0,
        "unbounded_fs": 0.0
      }
    },
    "SecurityBoundary": {
      "type": "choice",
      "choice": "fail_closed_bounded",
      "confidence": 1.0,
      "probabilities": {
        "fail_closed_bounded": 1.0,
        "client_trust": 0.0,
        "opaque_abort_only": 0.0
      }
    },
    "UiBridge": {
      "type": "choice",
      "choice": "event_dispatched_effect",
      "confidence": 1.0,
      "probabilities": {
        "event_dispatched_effect": 1.0,
        "async_view": 0.0,
        "ambient_state": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 1699,
    "output_tokens": 246
  }
}
```

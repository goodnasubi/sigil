# 6-6c: 起動時設定と web 統合

ADR 0037 決定2〜4 / Issue #509。Codex Cloud（OpenAI系統）。依存PR #508（`00073d545a9ef146a6de839b7cfcf0a8834780fc`）の純粋核を再利用する。6-6d、発行、ログイン、cookie、セッション、失効DB、OAuth/OIDC、JWT、TLS、日時一般は実装しない。

## 公開入口と効果

| 入口 | 効果 | 責務 |
|---|---|---|
| `web.load_bearer_config() -> Result<BearerConfig, ConfigError>` | `env` | 二変数を各一回読み、固定プロファイルを検査する |
| `BearerConfig.into_verifier(self) -> Result<AuthVerifier, ConfigError>` | `clock` | 起動時計を確認し、設定を所有するmoveクロージャを作る |
| 所有クロージャ → `bearer_request` | `clock` | 単一ヘッダ、Bearer形式、要求時の一回の時刻、純粋核を接続する |
| `web.verify_bearer`（既存） | `none` | 検証済み設定と明示時刻で署名・クレームを検証する |
| `ConfigError.name()` / `kind()` | `none` | 変数名と固定分類だけ返す |

設定は鍵hex64桁の小文字正規形と、ASCII英数字/`._:-` の1〜128バイトaudienceを受け付ける。未設定は`missing`、値不正は`invalid`、起動時計失敗は`unavailable`。時計の名前は固定の`clock.unix_seconds`。既存AppConfigのエラーは従来の変数名と`invalid`。エラーは設定値を保持しない。

二つのResultの成功後にだけ待受を開始する利用例は [例12](../../examples/12-bearer-auth/main.sigil)。読込APIは起動側が一度呼ぶ。検証器には再読込・鍵取出し・表示APIを持たせず、ルーター登録やdispatchに環境取得を加えない。型・効果検査で、読込の`env`と構築の`clock`は欠落・余剰とも拒否する。既存Routerの効果上限は変更しない。

公開APIの分割はtypesafe-ai正式判定（`jev-1.13.0`）でA採用、確率1.00/confidence1.00、対案Bは0.00。[入力](records/509-api-input.json)と[応答](records/509-api-output.json)を保存した。ADRの認証プロファイルや効果境界は変更しない。

## 応答・秘密・共有

既存`with_auth`へ設定所有クロージャを渡す。保護経路は全Authorization出現数を調べ、成功時は純粋核の検証済みsubのみをidentityにする。HTTPで外側OWSを除いた後の、ASCII大小不問Bearer＋SP一個＋tokenだけを受け付ける。直接dispatchではHTTP正規化を信頼せず、混合ケースの重複を拒否する。

Routerは元からverifierのエラー応答を捨て固定401を返す。この認証専用の固定401に`WWW-Authenticate: Bearer`と`Cache-Control: no-store`を追加し、本文`unauthorized`とContent-Typeを維持した。認証失敗時はbefore/handler/afterのいずれも実行しない。公開経路では検証器を呼ばずidentityを付けず、通常のフックと応答を維持する。汎用`web.error_response`と404等の既存境界応答は変更しない。

設定・検証器・RouterのSendable/Shareableを検査し、ネイティブ接続は`SyncShared<Router>`を借用して使う。非Copyable設定の再利用と表示は静的に拒否する。最後の所有者を破棄した際に既存Secretの記述子による破棄へ到達する。追加の暗号演算やCTラッパーはなく、既存`ct_bearer_signature`とHMACを再利用する。設定I/O・公開形式の解釈をCT関数とは称さない。

hexの作業用Bytesと途中まで復号したBytesは、成功・失敗の両方でSecretへ所有移動して既存のネイティブ破棄時ゼロ化を適用する。復号済み32バイトも直ちにSecretへ移す。通常Bytesの別コピーを保持しない。環境Stringは読込関数の局所値だけとし、その寿命を越えて設定・クロージャに保持しない。**既存Stringには安全な上書きAPIがなく、その割当・OS環境・OS/評価器内部のコピーの完全消去は保証しない**。評価器は機能比較用で、メモリ消去の証明ではない。配備側のプロセス・ダンプ・環境アクセス制御が必要というADRの前提を維持する。

統合試験で評価器の破棄traceが本物のSecretを平文Bytesとして表示する不一致を検出したため、`ir_eval/value.rs`の構造体表示だけを既存型標識`TypeDef.secret`に従い`Secret(***)`にする。ネイティブと同じ標識を使い、利用者の同名型は秘密扱いしない。さらに`--trace-drops`で受信Authorization/tokenが漏れないよう、標準の`std.http.HttpHeader`・`HttpRequest`・`HttpParser`・生入力を持つ内部`HttpBuf`・`std.web.AppRequest`の構造体表示を、評価器とネイティブの両方で完全修飾型名＋`(***)`に固定する。入れ子を含めフィールドを表示しない。通常の表示経路にも作用するが表示APIは追加しない。利用者の同名`HttpBuf`には適用しない。

この型単位の秘匿化は、選択的なヘッダ/生バッファ解析との比較で追加正式判定を実施し、採用確率0.95/confidence0.90、対案0.05。[入力](records/509-trace-input.json)と[応答](records/509-trace-output.json)を保存した。新しい原始操作・依存・unchecked・許可表・ABI・上限・CI変更はない。秘密表示境界の差分を含むためPRは**要承認**。

## 検証

`tests/bearer_config_509.rs`と専用Sigilドライバで、評価器/ネイティブの設定エラー、成功identity、Bearer大小、形式不正、同値重複、固定401全見出し、公開経路、404、時計失敗、フック非実行、共有性/所有権、秘密非表示と破棄を確認する。署名成功値は6-6bの独立公開固定ベクタを再利用する。

native buildはまとめて再利用する。試験時だけOS時計モジュールを差替え、実際の壁時計呼出しとclock効果を残して固定時刻または指定取得回数後の失敗を注入する。評価器は既存wall_clock注入を使い、要求時失敗だけwebの取得結果をNoneへ差替える（元のclock呼出しは維持）。製品に試験フックは加えない。

parser非経由試験はHttpParser.feedの試験時差替えで、所有モジュール内で生HttpRequestを直接構築する。混合ケースの同値Authorization二件をRouter.dispatchへ渡し、HTTP正規化に依存しないことを両実行系で確認する。ネイティブの`SIGIL_AUTH_*`環境取得は起動時の二回を越えると失敗する差替えにし、要求時の再読込がないことを観測する。TCP試験では最終wire応答の完全一致、成功主体、公開経路、時計失敗、起動設定失敗でポートを出さないことを確認する。

実token付きの評価器dispatchとネイティブ接続の破棄traceに、文字列/バイト数値列のいずれでも鍵・token・ヘッダ値が出ないことを確認する。全量cargo testはCIに委任し、検証結果はPRに記録する。6-6dのfuzz/G2記録は追加しない。

Codex Cloudで`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --test bearer_config_509`（7件）、`cargo test --test app_config_460`（25件）、`cargo test --test bearer_verify_507`（2件）、`cargo test --test clock_boundary_503`（7件）、`cargo test --test crypto_secret`（9件）、`cargo test --test ct_rules`（11件）、`cargo test --test web_router_478`（6件）、`cargo test --lib stdlib::`（11件）、例12の型/効果検査、`git diff --check`が通過。共有試験はSyncSharedのcloneと二つの借用後、最後の所有者がSecretを一度だけ破棄することを両実行系で観測する。

6-6bの意味解釈観測試験が`bearer_claims`以降を差替える既存の前提を維持するため、統合段のコードは純粋核の前に置く。既存試験の観測方法を変更しない。

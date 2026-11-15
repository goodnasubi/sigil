# bearer_token の試験入力（ADR 0037 / Issue #512）

`tests/fuzz/corpus/bearer_token/` と `tests/fuzz/regress/bearer_token/` のファイルは、次のバイナリ形式。設定・鍵・時刻は保存しない。

| 先頭バイト | 残りの内容 |
|---|---|
| `0`（ほか `1`・`2` 以外も同じ） | 受信 token の生バイト |
| `1` | 署名する payload の生バイト。試験ドライバが正規 base64url と固定公開鍵の HMAC を付ける |
| `2` | 出現数の1バイト（mod 4）、Authorization 値の生バイト |

固定公開鍵は `00 01 ... 1f`、audience は `sigil.api`、時刻は Unix 秒 `1700000000`。署名の生成は製品の HMAC を利用するが、固定 token は `bearer_507.tsv` の Python 標準 hmac による独立計算値。署名生成モードの受理可否と主体は Rust の独立 payload 判定と突き合わせる。評価器の全結果を native の全結果とも比較する。

corpus は138件。原則、同名の `bearer_507.tsv` 行の受信 token（列7）に `0` を付けたもの。対象は固定鍵・audience・時刻が上記と一致する行。`auth_tag_corruption_00`〜`31` は同じ行の token に `02 01` と `Bearer ` を付け、署名32位置の破壊を実際のアダプタでも確かめる。`authorization_*` は欠落・重複・大小文字・タブ・カンマ・NUL・余分なSPの直接入力。`auth_wrong_tag_short_payload` は内容不正かつ署名不一致の固定401を確認する。

`max_payload_404_token_590` は TSV の `valid_max_payload_404_token_590` 行。`signed_*` は名前に対応する行の payload（列3）に `1` を付けたもの。例外の `signed_subject_256` は `iat=1700000000, exp=1700003600, aud=sigil.api, sub=a×256` の正規バイナリ。最大 payload の audience は128バイトなので固定 audience には不一致となる。最大形式そのものの成功は `bearer_limits_512` で設定も合わせて確認する。

回帰3件は、既知の否定入力を縮小した空 token（`00`）、正しく署名する短い payload（`01 00`）、空値の重複（`02 02`）。今回発見した障害の修正例とは称さない。いずれも毎回生成より先に replay する。

通常は `cargo test --test fuzz_bearer_token -- --nocapture`。長い実行も同じ入口で、例えば `SIGIL_FUZZ_ITERS=600 SIGIL_FUZZ_SEED=7 cargo test --test fuzz_bearer_token fuzz_bearer_token -- --nocapture`。生成は random / corpus変異 / 署名付き文法を順に回す。

`target/fuzz-failures/bearer_token/` の `current` / `fail-<seed>-<index>` は既存枠の生入力。バッチ実行中は `batch`（1行1入力のhex）と `batch.meta`（種・絶対入力番号・生成方法・mode・件数）も残す。native は一度ビルドし、64件以下・転送48KB以下でまとめる。端数の失敗・異常終了でも `batch` を回収できる。成功したバッチは削除する。再現後は該当hex行をバイナリへ戻して `regress/bearer_token/` に追加する。生成実行ファイルの一時ディレクトリは成功時に削除する。

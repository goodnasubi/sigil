# 6-6b: 純粋なベアラートークン検証

ADR 0037 決定 1・3・4、Issue #507 の実装記録。2026-11-15（JST）、Codex Cloud（OpenAI 系統）。設定読込・HTTP 統合・時計取得・発行・fuzz・G2 はこの段では実装しない。

## 入口と境界

`web.verify_bearer(key: &Secret, audience: &String, token: &Bytes, now: Int) -> Result<AuthIdentity, String>` は `std.web`、`targets server`、`trust none`、`effects none`。検証済み設定の鍵と audience、および明示的な公開時刻を渡す。新しい設定型を定義せず、6-6c で起動時設定に接続するための純粋な入口だけを用意した。

入口でも鍵の長さ（32 バイト）、audience の制約、時刻の範囲を検査する。不正な設定や入力で暗号関数の `requires` 違反へ進めない。全失敗は `Err("unauthorized")`、成功は subject だけを持つ `AuthIdentity`。入力や鍵をエラーに含めず、表示 API もログも加えない。この段のエラー値は HTTP 応答ではなく、6-6c が一様な 401 へ変換する。

外枠・正規 base64url・復号サイズ・タグ長の確認後、受信バイトの `sigil1.<payload>` をそのまま `ct_bearer_signature` へ渡す。この内部関数は既存 `verify_hmac_sha256` を一回呼ぶだけで、鍵の内容を取り出さない。署名が合った後だけ `bearer_claims` が固定 binary を読み、時刻・audience・subject を検査する。

base64url は許可アルファベット、不可能長、未使用下位ビットを検査する。この三条件で「復号して再符号化した表現と等しい正規形」になる（再符号化した値を署名対象にしない）。8 バイト整数は各乗算・加算の前に上限を検査し、減算は大小の検査後にだけ実行する。

## 公開固定ベクタ

全値は [bearer_507.tsv](../../tests/data/bearer_507.tsv)。鍵は `00..1f` の公開の使い捨て値であり、製品の既定鍵ではない。一部の否定例だけ 31/33 バイトや別の公開鍵を使う。

[generate_507_vectors.py](generate_507_vectors.py) は Python 標準の `struct`、`base64`、`hmac`、`hashlib.sha256` だけで計算する。Sigil の処理系や暗号実装を呼ばない。リポジトリの根で次を実行すると固定記録を再生成できる。

```sh
python3 docs/authentication/generate_507_vectors.py
git diff --exit-code -- tests/data/bearer_507.tsv
```

各行はタブ区切り。順に ID、鍵 hex、復号 payload hex、署名対象 ASCII の全バイト hex、全 32 バイト tag hex、tag の base64url、実際に渡す token の全バイト hex、検証側 audience、公開 now、期待結果。`ERR` は固定エラー、その他は成功時の subject。さらに末尾の 3 列に、独立計算で実際に用いた署名鍵 hex、HMAC 入力 hex、変異前の正しい HMAC 全 32 バイト hex を記録する。これにより別鍵・署名対象取り違え・タグ破損の否定例も全値から再計算できる。変更方法は ID、最終的な token の変異は受信 token 欄に記録する。

107 件のうち正例は 7 件。`iat == now`、寿命 1/3600、期限直前、時刻上下端、識別子の全許可文字、payload 404/token 590/audience 128/subject 256 を含む。否定例は期限一致、未来発行、寿命 0/3601、時刻範囲外と u64/符号境界、対象の大小文字・別サービス、不正識別子、長さ欄・切断・余剰、鍵長、署名対象の取り違え、版・区切り・空欄・base64url・末尾ビット、token 1024/1025、タグの全 32 位置の破損を含む。署名済み不正 payload も独立に署名し、署名後の拒否経路に到達させる。

`tests/bearer_verify_507.rs` は固定値を読み、期待値や署名を Sigil で生成せず、まとめて評価器とネイティブで実行する。意味解釈入口だけを標識に置き換える試験では、署名済み短い payload は標識に到達し、同じ payload の不正署名では到達しないことを両実行系で観測する。製品コードに観測フックは加えない。

## CT の登録と流れ

`tests/ct_rules.rs` に `std.web.ct_bearer_signature` と `std.web.verify_bearer` を登録する。秘密は `key: &Secret` だけ、token/message/tag/audience/now は公開。鍵は長さの確認以外は CT ラッパーと既存 HMAC にだけ渡す。新しい原始操作・依存・`trust base` はない。

検証器全体の所要時間は一定と主張しない。公開の形式と署名成否で分岐してよい。秘密の鍵・期待 MAC・署名の一致位置に依存する分岐は許さない。

比較結果は関数外で公開するという既存の規則を、`verify_hmac_sha256` と `ct_bearer_signature` の Bool 戻り値に限る明示的な要約で表現する。両本体は引き続き R1〜R6 の検査対象。一般の Bool 関数に公開扱いを広げず、登録漏れ検査から検証器を除外しない。表検査は公開する関数の CT 登録と Bool 戻り値を固定し、否定 fixture は一般の Bool 関数の結果による分岐が R1 に残ることを確かめる。

コーパスは実際の `web.verify_bearer` を呼び、DCE 後にも新規関数を残す。静的検査は秘密の領域の流れを含めて実施する。CLIF はラッパー呼出しと既存 HMAC の直接呼出しを比較し、追加除算がなく、追加分岐が SIR の項と関数入口の公開スタック検査で説明できることを検査する。既存 HMAC/SHA/照合の検査も維持する。定数時間保証はネイティブのみで、評価器は機能比較に使う。

## 範囲外の既存制約

`tests/common/native.rs` の共通比較は `embedded_extra` をネイティブビルドへ渡さない。修正せず、観測試験に限り、差替え込みで lowering した同一 SIR を `native::build_program` へ渡して評価結果を比較した。通常の固定ベクタは既存の共通比較を使う。

`src/stdlib/web.sigil` と公開 API 宣言の `src/stdlib.rs` はレーン E 外だが、Issue #507 が明示する `std.web` 内の実装と呼出し口に必要な最小差分である。CT の信頼境界を表す検査変更を含むため、PR は **要承認** とする。

## 実行確認

Codex Cloud、Linux x86-64。`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --test bearer_verify_507`（2 試験、固定値 107 件と意味解釈到達観測）、`cargo test --test ct_rules`（11 試験、登録漏れ・静的・CLIF）、`cargo test --test clock_boundary_503`（7 試験）、`cargo test --lib stdlib::`（11 試験）が通過。全量テストは CI に委任する。

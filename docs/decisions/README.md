# 設計判断の記録（ADR）

Sigil の設計上の決定を、1 件 1 ファイルで記録します。

## 命名規則

`NNNN-短い題名.md`（例: `0001-vision-and-scope.md`）

## 各 ADR の構成

- **状態**: 提案 / 承認 / 却下 / 置き換え
- **背景**: 何を決める必要があったか
- **選択肢**: 検討した案と、それぞれの利点・欠点
- **決定**: 採用した案と理由
- **影響**: 決定によって変わること

## 一覧

新しい ADR の番号は、この表に「提案」の行を足して確保する。

| 番号 | 題名 | 状態 |
|---|---|---|
| [0001](0001-vision-and-scope.md) | 言語の目標と範囲 | 承認 |
| [0002](0002-syntax-family.md) | 構文の基本形 | 承認 |
| [0003](0003-ownership-and-borrowing.md) | 所有権と借用の規則 | 承認 |
| [0004](0004-effects.md) | 効果（副作用）の扱い | 承認 |
| [0005](0005-long-term-goals.md) | 長期目標と段階計画 | 承認 |
| [0006](0006-p0-scope-and-bootstrap.md) | P0 の範囲と実装の始め方 | 承認 |
| [0007](0007-concurrency-model.md) | 並行処理のモデル | 承認 |
| [0008](0008-m0-notation.md) | M0 の前に決める表記 | 承認 |
| [0009](0009-lexical-and-grammar.md) | 字句と文法の全体仕様 | 承認 |
| [0010](0010-fmt-and-m0-addendum.md) | 正規形と M0 の細目 | 承認 |
| [0011](0011-type-system-scope.md) | 型システムの範囲（M1 の前） | 承認 |
| [0012](0012-ownership-check-and-stdlib.md) | 所有権の検査の範囲と標準ライブラリの細目（M2 の前） | 承認 |
| [0013](0013-termination-stdlib-and-function-values.md) | 停止の扱い・標準ライブラリの正式な一覧・関数の値の渡し方（M3 の前） | 承認 |
| [0014](0014-core-and-os-split.md) | 標準ライブラリの核と OS 依存部分の分割（M4） | 承認 |
| [0015](0015-m5-native-output.md) | M5 の範囲と、ネイティブ出力のコンパイラ構成 | 承認 |
| [0016](0016-trust-base-declaration.md) | 信頼基盤（`trust base`）の宣言方法と、検査を外した箇所の契約の書き方 | 承認 |
| [0017](0017-run-on-sir-evaluator.md) | `sigil run` の実行器と、インタプリタの破棄の順序の食い違いの扱い | 承認 |
| [0018](0018-p1-scope.md) | P1 の範囲・順序・PR の分け方と、P0 から持ち越した項目の扱い | 承認 |
| [0019](0019-m6-machine-spec-and-language-server.md) | M6 の仕様の機械向けの形式と、言語サーバー | 承認 |
| [0020](0020-p1-security-addendum.md) | P1 のセキュリティの追補（定数時間・ファジング・公開前のレビュー・DoS の上限） | 承認 |
| [0021](0021-module-unit-and-visibility.md) | モジュールの単位と公開の範囲 | 承認 |
| [0022](0022-type-identity-traits-and-function-types.md) | 型の同一性、利用者のトレイト、関数の型をフィールド・戻り値に置くこと | 提案 |
| [0023](0023-bytes-and-bitwise.md) | Bytes 型とビット演算 | 提案 |
| [0024](0024-crypto-primitives.md) | 暗号の基本部品（SHA-256・HMAC・PBKDF2・定数時間比較・秘密値の保護） | 提案 |
| [0025](0025-runtime-replacement.md) | ランタイムの置き換え（原始操作・大域状態・確保器・標準ライブラリの Sigil 実装） | 提案 |
| [0026](0026-p1a-concurrency.md) | 並行処理 P1-a（SIR 評価器での構造化並行処理・性質の手付け・要求の隔離） | 提案 |

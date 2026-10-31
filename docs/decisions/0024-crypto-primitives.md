# 0024: 暗号の基本部品（SHA-256・HMAC・PBKDF2・定数時間比較・秘密値の保護）

- **状態**: 提案（PR 6-1。所有者レビュー待ち）
- **日付**: 2026-11-01
- **対象**: `std.crypto` モジュール、`Secret` 型、定数時間検証（`tests/ct_rules.rs`）

---

## 背景

[ADR 0018](0018-p1-scope.md) 決定 3（「段階 6 の暗号の基本部品は、段階 2 の後ならいつでも始めてよい」）、および [ADR 0020](0020-p1-security-addendum.md) に基づき、P1 における認証・通信（SCRAM-SHA-256・Bearer トークン署名照合）に必要な暗号基本部品を Sigil 自身で実装する。
PR 2-5（[ADR 0023](0023-bytes-and-bitwise.md)）により、`Bytes` 型および `Int` のビット演算子（`&`, `|`, `^`, `~`, `<<`, `>>`）、回転メソッド（`rotl`, `rotr`, `rotr32`）の実装とネイティブ・ランタイムサポートが完了した。

ADR 0020 では以下の重要論点が各段階の ADR（特に ADR 6-1）へ委ねられていた：
1. **暗号基本部品の API とモジュール構成**: SHA-256, HMAC-SHA-256, PBKDF2-HMAC-SHA-256, 定数時間比較のシグネチャと責務。
2. **秘密値の保護（`Secret` 型）**: メモリゼロ化（Zeroize）、安全な破棄、表示不可（`Displayable` 不実装）。
3. **定数時間規則（`ct_rules`）の検証と登録基準**:
   - `tests/ct_rules.rs` の静的 SIR データフロー解析（`ct_rules_static`）。
   - 表への載せ忘れ防止ルール（命名約束や型基準）。
   - 最適化後 Cranelift IR の検証（`ct_rules_clif`）。
   - 定数時間計測（`ct_measure`）。
4. **協調的マルチタスク（協調的スケジューリング）**:
   - PBKDF2 の反復ループにおける `task.yield` 呼び出し契機。
5. **テストベクタとファジング**:
   - NIST テストベクタ（RFC 6234, RFC 4231, RFC 6070）による検証と property-based / fuzz テスト。

---

## 決定 1: モジュール構成と暗号 API の設計

暗号関連機能は、安全な境界と可視性を確保するため、独立した標準モジュール **`std.crypto`**（`Layer::Core`）に配置する。

### 1. `Secret` 型（秘密情報のカプセル化）

鍵やパスワード、途中状態などの秘密データを通常データ（公開情報）と型レベルで隔離するため、`Secret` 型を提供する。

```sigil
module std.crypto
  targets all
  trust base

struct Secret {
  data: Bytes,
}
```

- **特性と不変条件**:
  - `Displayable` を実装しない（`console.log` やエラー文字列への埋め込みを防止、ADR 0020 R6）。
  - デストラクタ／破棄（`val_drop`）時に、確保されたバイト領域を直ちにゼロクリア（ゼロ化）する。
  - 生成は `Secret.from_bytes(b: Bytes) -> Secret` または専用の API 経由に限定。
  - 内部 `Bytes` への直接参照・取り出しは `expose(self: &Secret) -> &Bytes` などの明示的なメソッドに限定し、定数時間検査の対象とする。

### 2. 定数時間バイト列比較: `ct_eq`

ADR 0020 決定 1 および ADR 0023 決定 2 で定義された `ct_eq` を `std.crypto` の主たる比較手段として位置付ける（`std.core` にもあるエイリアス、または `std.crypto` から再エクスポート）。

```sigil
fn ct_eq(a: &Bytes, b: &Bytes) -> Bool
  intent "定数時間で 2 つのバイト列を比較する"
  requires none
  ensures none
  effects none
```
- **契約と動作**:
  - 秘密の値: `a` および `b` の内容。
  - 公開の値: バイト長（長さが異なる場合は直ちに `false` を返してよい。ADR 0020 決定 1）。
  - 長さが一致する場合、すべてのバイトの排他的論理和を累積し、早期リターンを行わずに比較する。

### 3. SHA-256 ハッシュ関数

ストリーミング型（`Sha256` 構造体）およびワンショット関数を提供する。

```sigil
struct Sha256 {
  state: List<Int>, // 8 つの 32 ビットワード
  buffer: Bytes,    // 未処理ブロックバッファ (最大 64 バイト)
  total_len: Int,   // 入力全バイト長
}

impl Sha256 {
  fn new() -> Sha256
    intent "SHA-256 コンテキストを初期化する"
    requires none
    ensures none
    effects none

  fn update(self: &mut Sha256, data: &Bytes)
    intent "バイト列を追加して状態を更新する"
    requires none
    ensures none
    effects none

  fn finalize(self: &mut Sha256) -> Bytes
    intent "パディングを適用し 32 バイトのダイジェストを返す"
    requires none
    ensures result.len() == 32
    effects none
}

fn sha256(data: &Bytes) -> Bytes
  intent "データ全体の SHA-256 ダイジェスト (32 バイト) を計算する"
  requires none
  ensures result.len() == 32
  effects none
```

- **アルゴリズム実装詳細**:
  - RFC 6234 に完全準拠。
  - 内部圧縮関数は 64 ラウンドのループからなり、ADR 0023 で導入された `&`, `|`, `^`, `~`, `>>`, `rotr32` のみを用いて計算する。
  - 32 ビット加算は 64 ビット `Int` 加算の後に `& 0xFFFFFFFF` を適用し、桁あふれパニックを回避する。

### 4. HMAC-SHA-256

RFC 2104 / RFC 4231 に準拠したメッセージ認証コード。

```sigil
fn hmac_sha256(key: &Secret, message: &Bytes) -> Secret
  intent "HMAC-SHA-256 認証タグ (32 バイト) を計算する"
  requires key.len() >= 32
  ensures result.len() == 32
  effects none

fn verify_hmac_sha256(key: &Secret, message: &Bytes, expected: &Bytes) -> Bool
  intent "HMAC-SHA-256 認証タグを定数時間で検証する"
  requires key.len() >= 32
  ensures none
  effects none
```

- **セキュリティ要件**:
  - `key` は `Secret` 型を強制し、出力タグも `Secret` として扱う（または検証時に `ct_eq` で照合）。
  - `key` 長が 32 バイト未満の場合は事前条件（`requires`）違反として異常停止（ADR 0020）。
  - ブロックサイズ（64 バイト）を超える鍵は `sha256` で圧縮してから使用。
  - `ipad`（`0x36`）および `opad`（`0x5C`）の XOR 展開。

### 5. PBKDF2-HMAC-SHA-256

RFC 2898 / RFC 6070 に準拠したパスワードベース鍵導出関数。PostgreSQL SCRAM-SHA-256（RFC 7677）の鍵生成に用いる。

```sigil
fn pbkdf2_sha256(
  password: &Secret,
  salt: &Bytes,
  iterations: Int,
  dk_len: Int
) -> Secret
  intent "PBKDF2-HMAC-SHA-256 により指定長の鍵を導出する"
  requires iterations >= 4096 && iterations <= 100000 && dk_len > 0
  ensures result.len() == dk_len
  effects none
```

- **反復回数制約**:
  - `4096 <= iterations && iterations <= 100000` を強制（ADR 0020）。
- **協調スケジューリング（決定 2 参照）**:
  - 長時間の計算でイベントループや同一スレッドの他タスクをブロックしないよう、ループ内で定期的に協調 yield を行う。

---

## 決定 2: 協調的マルチタスクと `task.yield` の組み込み

PBKDF2 の計算反復（10,000〜100,000 回）は、単一スレッド上で数十ミリ秒〜数百ミリ秒を消費する可能性がある。

- **方針**:
  - PBKDF2 の反復ブロック処理において、**1024 反復ごと** に `task.yield` を呼び出す。
  - `task.yield` は引数を取らないため、ADR 0020 の秘密情報漏洩規則 R4（「秘密の値を定数時間でない関数に渡すことの禁止」）に抵触しない。
  - 呼び出し周期（1024 回）および反復回数（`iterations`）は **公開の値** であるため、タイミングリーク（サイドチャネル攻撃）の原因にならない。
  - `effects task.yield` の宣言要否:
    - 段階 4（ADR 0007 / ADR 0018）のタスク並行性モデル合流前においては、ダミーまたはプレースホルダ呼び出し（ノーオペレーション）とし、段階 4 のマージ後に本実装へ切り替える。

---

## 決定 3: 定数時間保証の検証方式（`tests/ct_rules.rs`）

ADR 0020 決定 1 に基づき、CI およびテストスイートで定数時間性を機械的に検証する仕組みを確立する。

### 1. 定数時間検査の対象関数（ホワイトリスト）

表への載せ忘れを防止するため、以下の **包括的命名基準および型基準** を定める：
- **基準 1（接頭辞基準）**: 名前が `ct_` で始まるすべての公開関数。
- **基準 2（型基準）**: 引数または戻り値に `Secret` を含むすべての関数。
- **基準 3（モジュール基準）**: `std.crypto` に属するすべてのハッシュ圧縮・MAC・鍵導出関数。

テスト `tests/ct_rules.rs` 内に明示的な検証対象テーブルを定義し、上記基準に合致する関数がテーブルに漏れている場合は **テスト失敗** とする。

```rust
// tests/ct_rules.rs の検査対象テーブル定義
const CT_FUNCTIONS: &[(&str, &[&str])] = &[
    // (関数名, 公開パラメータ引数名一覧)
    ("ct_eq", &["a_len", "b_len"]),
    ("std.crypto.sha256_compress", &["block_count"]),
    ("std.crypto.hmac_sha256", &["message_len"]),
    ("std.crypto.verify_hmac_sha256", &["message_len", "expected_len"]),
    ("std.crypto.pbkdf2_sha256", &["salt", "iterations", "dk_len"]),
];
```

### 2. SIR 上のデータフロー検査（`ct_rules_static`）

- lowering 後の SIR 基本ブロックを走査し、秘密値（テーブルで指定された公開引数以外の引数・`Secret` 内部バイト）に由来する値のフローを taint 解析する。
- 以下の命令・構文に秘密値が到達した場合に違反を報告する：
  1. `Term::Switch` の分岐条件（R1 違反）。
  2. メモリアクセス・リスト・スライスのインデックス添字計算（R2 違反）。
  3. 除算・剰余算命令（R3 違反）。
  4. 非ホワイトリスト関数への引数受け渡し（R4 違反）。
  5. 不正演算子（R5 違反）。
  6. コンソール・ログ・文字列フォーマット関数への引き渡し（R6 違反）。

### 3. Cranelift 最適化後 IR 検査（`ct_rules_clif`）

- 最適化パス（`cranelift-codegen` の optimize）を通過した IR において：
  - 条件分岐命令（`brif`, `br_table`）が、SIR 時点で検証済みの公開条件分岐数を超えて増加していないこと。
  - 除算命令（`sdiv`, `udiv`, `srem`, `urem`）が含まれないこと。
  - ゼロ化コード（`Secret` の破棄）において、書き込み命令（`store`）がデッドコード削除で消去されていないこと。

### 4. 統計的タイミング計測テスト（`ct_measure`）

- `dudect` 手法に基づく Welch's t-test 計測テスト（`tests/ct_measure.rs`、`#[ignore]`）を実装。
- 固定入力（Fix）とランダム入力（Var）の各 100 万回試行での実行時間分布を比較し、$|t| < 4.5$ であることを確認。
- 意図的に早期リターンする非定数時間関数を陽性対照（Positive Control）として同時に測定し、$|t| \ge 4.5$ となる検出感度を保証。

---

## 決定 4: テストベクタとファジング

### 1. 標準テストベクタによる完全一致検証

以下の公的標準テストベクタを `tests/crypto_vectors.rs` に組み込み、1 ビットの狂いもなく一致することを検証する：
- **SHA-256**: RFC 6234 / NIST CAVP テストベクタ（空文字列、"abc"、100 万回の "a"、多ブロックメッセージ）。
- **HMAC-SHA-256**: RFC 4231 テストベクタ（テストケース 1〜7：各種キー長、データ長）。
- **PBKDF2-HMAC-SHA-256**: RFC 6070 テストベクタ（短パスワード、長ソルト、反復回数 4096 回）。

### 2. Property-Based テストとファジング

- `arbitrary` / `cargo-fuzz`（または Sigil 自前のファジングハーネス）を用い、ランダムなバイト列に対する境界値テストを実施：
  - `ct_eq(a, a)` は常に `true`。
  - `a != b` ならば `ct_eq(a, b) == false`。
  - ワンショット `sha256(data)` と、任意のチャンク長で分割 `update` したストリーミング `Sha256` の結果が常に一致すること（結合律・ストリーミング不変性）。
  - 不正な反復回数（< 4096, > 100,000）や不正な鍵長に対する適切な契約違反パニックの確認。

---

## 移行計画と実装 PR の分割

ADR 0020 および ADR 0018 のレーン制に従い、以下の段階的 PR で安全に導入する：

| PR | 内容 | レーン | 主な変更ファイル |
|---|---|---|---|
| **PR 6-1a** | 定数時間検査フレームワークの確立 | C1, E | `tests/ct_rules.rs`（静的データフロー解析・CLIF 検査） |
| **PR 6-1b** | `Secret` 型と `ct_eq` の実装・検証 | D2, C2, E | `src/stdlib.rs`, `runtime/core`, `tests/crypto_secret.rs` |
| **PR 6-1c** | SHA-256 の実装とテストベクタ | D2, E | `src/stdlib.rs`（または `library/crypto/sha256.sigil`）, `tests/crypto_sha256.rs` |
| **PR 6-1d** | HMAC-SHA-256・PBKDF2 の実装とファジング | D2, E | `src/stdlib.rs`, `tests/crypto_hmac_pbkdf2.rs`, `tests/ct_measure.rs` |

---

## 影響

- **セキュリティ**:
  - SCRAM 認証および Bearer トークン署名の検証基盤が整い、タイミング攻撃耐性が機械的に担保される。
  - 秘密情報がログやエラー、メモリ残余から漏洩するリスクをゼロ化する。
- **言語機能**:
  - `std.crypto` が標準ライブラリの中核モジュールとして提供される。
- **CI / ビルド**:
  - `tests/ct_rules.rs` が毎回の PR で自動検証され、定数時間規則の退行が防止される。


# 0055: String・Bytes の長さの名前（単位を名前で分ける）

- **状態**: 提案（予約: Issue #631、正式判定を ADR 内に記録。接戦の 2 点は「正式判定の記録」に明記）
- **日付**: 2026-11-21（JST）
- **対象**: [Issue #631](https://github.com/goodnasubi/sigil/issues/631)、レーン E
- **仕様基準**: sigil の `main`、`3dfc3142eaf75fb7626866e919c005b1746128e0`
- **範囲**: ADR の起草だけ。実装・文書の改訂・呼び出し側の移行は含めない（ADR のマージ後に別 Issue／PR で担当を決める）

## 背景

`String.len()` は Unicode スカラー値の数を返し、`Bytes.len()` はバイト数を返す。同じ名前で単位が違うため取り違えやすく、実際に不具合を生んだ（sigil-git PR #4: UTF-8 妥当性を `to_string_utf8().unwrap_or("")` の結果と元バイト列の `len()` の比較で判定し、日本語のパスを誤って拒否した）。intent には単位が既に書かれていたのに起きたので、文書で明示するだけでは再発を防げない。

設計の方向（名前で単位を分ける rename 案）は、Issue #631 のコメント 7059881173 で、typesafe-ai の正式判定（rename 0.97、confidence 0.96）により決定済みである。本 ADR は、その方向の下で次の 3 点を確定する。

1. 具体的な新しい名前
2. 既存の `.len()` を残すか置き換えるか
3. 移行の範囲と手順

### 現状（仕様基準で確認）

| 型 | 公開メソッド | 返す値 | 計算量 |
|---|---|---|---|
| `String` | `len` | Unicode スカラー値の数 | 長さに比例（続きバイトでないバイトを数える） |
| `String` | （バイト数を返すメソッドは無い） | — | `as_bytes().len()` と書くと、`as_bytes` が毎回 `Bytes` を複製する |
| `Bytes` | `len` | バイト数 | 定数時間（ヘッダの値を読む） |
| `Secret`（crypto） | `len` | 中身のバイト数 | 定数時間 |
| `List` | `len` | 要素数 | 定数時間 |

- 公開メソッドの定義は `src/stdlib.rs` の表と `src/lower/expr.rs` の (型, 名前) の対応にある。IR の組み込み名 `StrLen`・`BytesLen` は `src/ir.rs`（契約）にある。
- 内部の `std.core.string.bytes` は `String` のバイト数を返すが、公開メソッドではない。
- `.len()` の記述は `src/stdlib` 247 件・`examples` 26 件・`tests` 465 件（114 ファイル）・`docs` 28 件（grep による上限の見積り。`List.len` を含み、型ごとの内訳は静的には分からない）。ほかに sigil-git・sigil-db・sigil-container が sigil のコミットを固定して依存する。

## 判断

| 判断点 | 採用 | 他案を採らない理由 |
|---|---|---|
| 1. 文字数の名前 | `String.char_count()` | `char_len` は `len` の語を残すため、定数時間の長さと走査して数える文字数の違いが名前に出ない。`scalar_count` は正確だが、他言語や利用者にとって `char` の方が通じる。 |
| 2. バイト数の名前 | `byte_len`（`String`・`Bytes`・`Secret`） | `bytes_len` は複数形で、単位が「バイト」であることを示す名前として `byte_len` が素直。`byte_count` は定数時間の長さに数え上げを連想させる。 |
| 3. `String` のバイト数 | `String.byte_len()` を公開メソッドとして足す | `as_bytes().byte_len()` は毎回複製し、バイト位置を要求する API に渡す長さを得る最も多い経路で無駄が出る。 |
| 4. `Bytes.len()`・`Secret.len()` | どちらも `byte_len` に置き換える。`.len()` は `List` など要素の集合だけに残す | 一方だけ残すと「`len` はバイト数のことがある」という例外が残り、`String` との取り違えの元が消えない。 |
| 5. 旧 `.len()` の扱い | 旧 `String.len`・`Bytes.len`・`Secret.len` は最終的に削除し、型検査のエラーで新しい名前を案内する。恒久的な別名は残さない | 別名を残すと、書き手が旧名を選び続けられ、同じ取り違えが再発する。 |
| 6. 移行 | 3 段（追加→移行→削除）。各段は別 PR | 1 つの PR で行うと、依存先が固定した版で旧名が突然消えて壊れる。依存先に直しを任せたまま削除する案も同じ理由で採らない。 |

## 決定 1: 名前と意味

| 型 | 新しい名前 | 返す値 | 旧 |
|---|---|---|---|
| `String` | `char_count(self: &String) -> Int` | Unicode スカラー値の数 | `len` |
| `String` | `byte_len(self: &String) -> Int` | UTF-8 のバイト数 | （無し。`as_bytes().len()`） |
| `Bytes` | `byte_len(self: &Bytes) -> Int` | バイト数 | `len` |
| `Secret` | `byte_len(self: &Secret) -> Int` | 中身のバイト数（公開の情報、ADR 0020 決定 1） | `len` |
| `List` | `len`（変更なし） | 要素数 | — |

- `is_empty` は変更しない。バイト数が 0 であることと文字数が 0 であることは同値なので、単位による取り違えは起きない。
- `char_count` は走査して数えるため長さに比例する。intent に「走査して数える」と明記する。`byte_len` は定数時間と明記する。
- 位置を要求する API（`Bytes.slice`、`Bytes.get`、`Bytes.set`、`read_u32_be` など）の引数は、これまでどおりバイト位置である。新しい名前の intent に「バイト位置を要求する API にはこの値を使う」と書く。
- バイト位置と文字位置を別の型にする案は、影響の範囲が今回の問題に対して過大なので採らない（Issue #631 の正式判定どおり）。

## 決定 2: 移行の範囲と手順

3 段にし、各段は別 PR とする。旧名は第 3 段まで残るので、各段で全量テストが緑のまま進められる。

| 段 | 内容 | レーン | 備考 |
|---|---|---|---|
| 1. 追加 | `char_count`（`String`）・`byte_len`（`String`・`Bytes`・`Secret`）を足す。旧名は併存。`sigil spec --std` に反映する | D2（`src/stdlib.rs`・`src/stdlib/*.sigil`）、C1（`src/lower/expr.rs`）、契約（`src/ir.rs`） | `String.byte_len` の組み込み名は新しく、`src/ir.rs` の変更になる。契約変更として単独の PR にし、`contract-change` ラベルを付ける。評価器とネイティブの一致を試験で確かめる。 |
| 2. 移行 | 呼び出し側を新しい名前に直す。範囲は sigil 内（`src/stdlib`・`examples`・`tests`・`docs`）と依存先（sigil-git・sigil-db・sigil-container） | 該当レーン、依存先は各リポジトリ | 依存先は、固定した sigil のコミットを第 1 段以降に更新する PR で直す。レーン外のファイルを直す PR には、理由を書く。 |
| 3. 削除 | 旧 `String.len`・`Bytes.len`・`Secret.len` を削除し、型検査で「`String` に `len` は無い。文字数は `char_count`、バイト数は `byte_len`」の形で案内する | B4／D2 | 削除の条件は下記。 |

### 第 2 段の進め方

- 呼び出し側の機械的な置換はできない（`.len()` の受け手の型が文面から分からず、`List.len` が混じる）。第 1 段の後、旧名を一時的に外した作業用のビルドで型検査を走らせ、エラーになった箇所を一覧にして直す。一覧は実施 Issue に記録する。
- 各箇所は、値を何に使うかで決める。バイト位置・バイト範囲・バッファの大きさ・上限の比較には `byte_len`、利用者が見る文字数の扱い（表示、文字数の制限）には `char_count` を使う。
- UTF-8 妥当性の判定は、長さの比較をやめ、`to_string_utf8()` の成否で直接判定する（sigil-git PR #4 の修正と同じ）。
- 直した箇所に、単位の取り違えによる誤りが残っていないことを確かめ、結果を実施 Issue に記録する。sigil-git の `git/refs.sigil`・`git/pack.sigil` の `string_slice` への文字数の渡しは、別 Issue（backlog-id: sigil-git-string-slice-len）の範囲であり、本 ADR の範囲に含めない。

### 第 3 段の削除条件

次をすべて満たしてから削除する。

1. sigil 内の `src/stdlib`・`examples`・`tests`・`docs` で旧名の使用が 0 件。
2. 依存先の固定した sigil のコミットが第 2 段以降の版に更新され、旧名の使用が 0 件。確認した PR 番号を実施 Issue に記録する。
3. 全量テストが緑。

## 影響

- 既存の ADR（0012・0013・0023・0024・0036 など）の `.len()` の記述は当時の記録であり、改訂しない。本 ADR が名前の変更を記録する。
- `sigil spec --std` の出力が変わる。各段の PR に差分を貼る。
- 第 1 段は `src/ir.rs`（契約）に触れるので、第 1 段の実装に入る前に契約変更の手続き（単独の PR とラベル）を踏む。`src/limits.rs` と `.github/CODEOWNERS` は変更しない。

## 完了条件

本 ADR のマージが完了条件ではない。実施の完了は次で確かめる（実施 Issue に記録する）。

- 第 1〜3 段の PR が別系統のレビューで承認され、CI が緑でマージされていること。
- 第 3 段の後、`String`・`Bytes`・`Secret` に `len` が無く、旧名を使うとエラーで新しい名前が案内されること。
- 第 1 段で足した名前について、評価器とネイティブの結果が一致していること。
- 依存先と sigil 内の呼び出し側に、単位の取り違えによる誤りが残っていないことの確認記録があること。

## 決めないこと

- `String` の文字位置・バイト位置による切り出し API の追加や変更。
- バイト位置と文字位置を別の型で表すこと。
- 非推奨の警告を出す仕組み（言語に無い）。旧名は第 3 段まで黙って併存する。
- `char_count` の高速化（文字数の保持など）。
- 第 2 段の PR の細かい分割と担当（ADR のマージ後に決める）。

## 正式判定の記録

2026-11-21（JST）、起草した Claude が typesafe-ai `jev-1.13.0`（`POST https://api.typesafe.ai/v1/systemone`）に、固定の評価軸で判定を出した。2026-11-16 の所有者指示に従い、Gemini Decisions は使っていない。設計の方向（rename 案）は Issue #631 のコメント 7059881173 の別の正式判定で決定済みであり、ここでは名前・既存 `.len()` の扱い・移行の手順を判定した。

### 第 1 回（7 問）

| 問い | 選ばれた案 | 確率 | confidence | 備考 |
|---|---|---|---|---|
| char_name（文字数の名前） | `char_count` | 0.42（次点 `scalar_count` 0.40） | 0.23 | **接戦**。差は 0.02 |
| byte_name（バイト数の名前） | `byte_len` | 0.58（次点 `bytes_len` 0.41） | 0.37 | |
| string_byte_len（`String` に足すか） | `add_string_byte_len` | 0.96 | 0.92 | |
| bytes_len_fate（`Bytes.len` の扱い） | `keep_bytes_len` | 0.53（`rename_bytes_to_byte_len` 0.47） | 0.06 | **接戦**。第 2 回で再確認 |
| secret_len（`Secret.len` の扱い） | `rename_secret_to_byte_len` | 0.79 | 0.58 | |
| old_len_fate（旧名の扱い） | `remove_with_hint` | 0.89 | 0.83 | |
| migration（移行） | `staged_add_migrate_remove` | 0.99 | 0.98 | |

### 第 2 回（整合の確認、同じ評価軸に第 1 回の結果を追記）

第 1 回で `Bytes` は「残す」、`Secret` は「置き換える」と食い違ったため、二つを一組にして問い直した。

| 問い | 選ばれた案 | 確率 | confidence |
|---|---|---|---|
| bytes_secret_pair | `both_byte_len`（どちらも `byte_len` に置き換える） | 0.72（`bytes_keep_secret_rename` 0.25、`both_keep_len` 0.03） | 0.58 |

### 採用の扱い

- 第 1 回の 7 問のうち `bytes_len_fate`・`secret_len` を除く 5 問は、選ばれた案を採った。`Bytes.len` と `Secret.len` は、食い違いを避けるため第 2 回の一組の判定（両方 `byte_len`）に従った。第 1 回の `keep_bytes_len` とは異なる結果になる。
- 接戦の 2 点は次のとおり。確率は設計の正しさの証明ではなく、採用理由は判断の表と、各段の検証可能な完了条件にある。
  - `char_count` と `scalar_count` は差 0.02。名前の優劣が付かないので、Issue 本文と報告で使われている「文字数」の語に近い `char_count` を採った。レビューで異論があれば、第 1 段の実装の前に名前だけを差し替えられる（他の判断に影響しない）。
  - `Bytes.len` の扱いは第 1 回で 0.53／0.47。第 2 回の一組の判定で `both_byte_len` が 0.72 と大きく優り、`List` 以外の `len` を無くす方針と整合するので、置き換えを採った。
- 別系統のレビューは本記録に含めず、PR のレビューに委ねる。

<details><summary>typesafe-ai 入力 JSON（第 1 回、秘密情報なし）</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間が仕様だけをレビューする言語。AIが主な書き手なので、名前だけで意味と単位が分かり、取り違えが構造的に起きにくいことを最優先する。言語や標準ライブラリの欠陥は sigil で ADR→実装の順に解決する(ADR0044決定4)。ADRは実装を混ぜない。事実: (1) String.len() は Unicode スカラー値の数を返し、続きバイトでないバイトを数えるため長さに比例して走査する。Bytes.len() はバイト数を返し、ヘッダの値を読むだけで定数時間。List.len() は要素数。crypto の Secret.len() は中身のバイト数。(2) String の公開メソッドは len・is_empty・trim・lines・parse_int・as_bytes のみで、文字列のバイト数を直接返す公開メソッドは無い。バイト数が要る呼び出しは as_bytes().len() と書くが、as_bytes は毎回 Bytes を複製する。内部には std.core.string.bytes（バイト数）がある。(3) 同じ .len() で単位が違うため、sigil-git PR#4 で UTF-8 妥当性の判定が取り違えられ、日本語パスが誤って拒否される不具合が実際に起きた。intent には単位が既に明記されていたのに起きた。(4) 影響の規模(grep の上限見積り): src/stdlib 247 件・examples 26 件・tests 465 件(114 ファイル)・docs 28 件。型ごとの内訳は不明で List.len も含む。ほかに sigil-git・sigil-db・sigil-container が sigil のコミットを固定して依存する。(5) コンパイラの組み込みメソッドは src/stdlib.rs の表と src/lower/expr.rs の (型,名前) 対応に定義される。 (6) 設計の方向は『名前で単位を分ける rename 案』で決定済み(別の正式判定 rename 0.97)。ここでは具体的な名前・既存 .len() の扱い・移行の手順を決める。(7) Sigil は 1.0 前で、利用者は所有者の管理下にある。依存先は sigil のコミットを固定するため、移行は各依存先が自分の都合で固定を更新するまで旧名が残っていないと壊れる。",
  "questions": {
    "char_name": {
      "type": "choice",
      "criteria": {
        "char_count": "文字(Unicode スカラー値)の数を返す名前を char_count にする。count は数え上げ(走査)を連想させ、len の定数時間との違いも名前に出る",
        "char_len": "char_len にする。len の語を残して byte_len と対にする",
        "scalar_count": "scalar_count にする。Unicode スカラー値であることを正確に示す",
        "chars_count": "chars_count にする"
      }
    },
    "byte_name": {
      "type": "choice",
      "criteria": {
        "byte_len": "バイト数を返す名前を byte_len にする(定数時間の長さ)",
        "byte_count": "byte_count にする",
        "bytes_len": "bytes_len にする"
      }
    },
    "string_byte_len": {
      "type": "choice",
      "criteria": {
        "add_string_byte_len": "String に byte_len を公開メソッドとして足す(複製なし・定数時間)。as_bytes().len() の複製を避けられる",
        "only_via_as_bytes": "String には足さず、バイト数は as_bytes().byte_len() で得る"
      }
    },
    "bytes_len_fate": {
      "type": "choice",
      "criteria": {
        "rename_bytes_to_byte_len": "Bytes.len() も byte_len に置き換える。.len() は要素の集合(List)だけに残し、String と Bytes は単位つきの名前だけにする",
        "keep_bytes_len": "Bytes.len() は残す(Bytes の要素はバイトなので len=バイト数で自然)。String.len() だけを置き換える"
      }
    },
    "secret_len": {
      "type": "choice",
      "criteria": {
        "rename_secret_to_byte_len": "crypto の Secret.len() も byte_len にして Bytes と揃える",
        "keep_secret_len": "Secret.len() は残す"
      }
    },
    "old_len_fate": {
      "type": "choice",
      "criteria": {
        "remove_with_hint": "旧 String.len()/Bytes.len() は最終的に削除し、型検査のエラーで新しい名前を案内する。恒久的な別名は残さない",
        "permanent_alias": "旧 .len() を恒久的な別名として残す(非推奨の注記つき)",
        "keep_len_unchanged": "旧 .len() をそのまま残す"
      }
    },
    "migration": {
      "type": "choice",
      "criteria": {
        "staged_add_migrate_remove": "3 段: (1)新しい名前を足す(旧名は併存) (2)sigil 内と依存先を新名へ移行 (3)依存先の固定の更新を確かめてから旧名を削除。各段は別 PR",
        "single_flag_day": "1 つの PR で追加・移行・削除をまとめて行う",
        "add_and_remove_now_migrate_later": "新名を足して旧名を同時に削除し、依存先は各自直す"
      }
    }
  }
}
```

</details>

<details><summary>typesafe-ai 応答 JSON（第 1 回）</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "char_name": {
      "type": "choice",
      "choice": "char_count",
      "confidence": 0.23,
      "probabilities": {
        "char_count": 0.42,
        "char_len": 0.16,
        "scalar_count": 0.4,
        "chars_count": 0.02
      }
    },
    "byte_name": {
      "type": "choice",
      "choice": "byte_len",
      "confidence": 0.37,
      "probabilities": {
        "byte_len": 0.58,
        "byte_count": 0.01,
        "bytes_len": 0.41
      }
    },
    "string_byte_len": {
      "type": "choice",
      "choice": "add_string_byte_len",
      "confidence": 0.92,
      "probabilities": {
        "add_string_byte_len": 0.96,
        "only_via_as_bytes": 0.04
      }
    },
    "bytes_len_fate": {
      "type": "choice",
      "choice": "keep_bytes_len",
      "confidence": 0.06,
      "probabilities": {
        "rename_bytes_to_byte_len": 0.47,
        "keep_bytes_len": 0.53
      }
    },
    "secret_len": {
      "type": "choice",
      "choice": "rename_secret_to_byte_len",
      "confidence": 0.58,
      "probabilities": {
        "rename_secret_to_byte_len": 0.79,
        "keep_secret_len": 0.21
      }
    },
    "old_len_fate": {
      "type": "choice",
      "choice": "remove_with_hint",
      "confidence": 0.83,
      "probabilities": {
        "remove_with_hint": 0.89,
        "permanent_alias": 0.06,
        "keep_len_unchanged": 0.05
      }
    },
    "migration": {
      "type": "choice",
      "choice": "staged_add_migrate_remove",
      "confidence": 0.98,
      "probabilities": {
        "staged_add_migrate_remove": 0.99,
        "single_flag_day": 0.0,
        "add_and_remove_now_migrate_later": 0.01
      }
    }
  },
  "usage": {
    "input_tokens": 1919,
    "output_tokens": 315
  }
}
```

</details>

<details><summary>typesafe-ai 入力 JSON（第 2 回、秘密情報なし）</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間が仕様だけをレビューする言語。AIが主な書き手なので、名前だけで意味と単位が分かり、取り違えが構造的に起きにくいことを最優先する。言語や標準ライブラリの欠陥は sigil で ADR→実装の順に解決する(ADR0044決定4)。ADRは実装を混ぜない。事実: (1) String.len() は Unicode スカラー値の数を返し、続きバイトでないバイトを数えるため長さに比例して走査する。Bytes.len() はバイト数を返し、ヘッダの値を読むだけで定数時間。List.len() は要素数。crypto の Secret.len() は中身のバイト数。(2) String の公開メソッドは len・is_empty・trim・lines・parse_int・as_bytes のみで、文字列のバイト数を直接返す公開メソッドは無い。バイト数が要る呼び出しは as_bytes().len() と書くが、as_bytes は毎回 Bytes を複製する。内部には std.core.string.bytes（バイト数）がある。(3) 同じ .len() で単位が違うため、sigil-git PR#4 で UTF-8 妥当性の判定が取り違えられ、日本語パスが誤って拒否される不具合が実際に起きた。intent には単位が既に明記されていたのに起きた。(4) 影響の規模(grep の上限見積り): src/stdlib 247 件・examples 26 件・tests 465 件(114 ファイル)・docs 28 件。型ごとの内訳は不明で List.len も含む。ほかに sigil-git・sigil-db・sigil-container が sigil のコミットを固定して依存する。(5) コンパイラの組み込みメソッドは src/stdlib.rs の表と src/lower/expr.rs の (型,名前) 対応に定義される。 (6) 設計の方向は『名前で単位を分ける rename 案』で決定済み(別の正式判定 rename 0.97)。ここでは具体的な名前・既存 .len() の扱い・移行の手順を決める。(7) Sigil は 1.0 前で、利用者は所有者の管理下にある。依存先は sigil のコミットを固定するため、移行は各依存先が自分の都合で固定を更新するまで旧名が残っていないと壊れる。 追加の整合確認(同じ評価軸): 第1回の判定で bytes_len_fate は接戦(keep_bytes_len 0.53/rename 0.47)、secret_len は rename 0.79 だった。Bytes と Secret で扱いが食い違わないよう、二つを一組として決める。",
  "questions": {
    "bytes_secret_pair": {
      "type": "choice",
      "criteria": {
        "both_byte_len": "Bytes.len() と Secret.len() をどちらも byte_len に置き換える。.len() は List など要素の集合だけに残す",
        "both_keep_len": "Bytes.len() と Secret.len() はどちらも len のまま残し、String.len() だけを置き換える",
        "bytes_keep_secret_rename": "Bytes.len() は残し、Secret.len() だけ byte_len にする"
      }
    }
  }
}
```

</details>

<details><summary>typesafe-ai 応答 JSON（第 2 回）</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "bytes_secret_pair": {
      "type": "choice",
      "choice": "both_byte_len",
      "confidence": 0.58,
      "probabilities": {
        "both_byte_len": 0.72,
        "both_keep_len": 0.03,
        "bytes_keep_secret_rename": 0.25
      }
    }
  },
  "usage": {
    "input_tokens": 1274,
    "output_tokens": 51
  }
}
```

</details>

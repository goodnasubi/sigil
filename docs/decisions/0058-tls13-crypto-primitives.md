# 0058: TLS 1.3 のための暗号部品

- **状態**: 提案
- **日付**: 2026-11-23
- **対象**: `std.crypto`、定数時間検査、公開テストベクタ

## 背景

TLS 終端は sigix の前段に置く `sigil-shield-tls` が担う。[Issue #676](https://github.com/goodnasubi/sigil/issues/676) に沿って、先に Sigil の標準ライブラリを整える。本 ADR は [ADR 0024](0024-crypto-primitives.md) を拡張する。TLS の鍵スケジュール、レコード、ハンドシェイクの状態機械と UDP はそれぞれの設計に委ねる。

## 決定 1 — 部品の範囲と順序

[ADR 0001](0001-vision-and-scope.md) の、仕様を人がレビューでき、処理系が契約を検査できるという目標に従う。スイート・証明書機能の同時導入は相互運用の検証をまとめられる一方、信頼するコードとレビュー範囲が一度に増える。必須スイートの土台を先行し、追加スイートと TLS 終端固有の判断を分ける。

| 優先 | 順序 | 用途 |
|---|---|---|
| 1 | HKDF-SHA-256 → AES-128-GCM → X25519 | `TLS_AES_128_GCM_SHA256` の鍵導出とレコード保護、推奨グループによる鍵共有 |
| 2 | ChaCha20-Poly1305。並行して SHA-384 と HKDF-SHA-384 → AES-256-GCM | `TLS_CHACHA20_POLY1305_SHA256` と `TLS_AES_256_GCM_SHA384` |

AES のブロック演算・鍵展開と GHASH、SHA-384 の SHA-512 系圧縮、ChaCha20 と Poly1305 の単体演算は内部部品にする。

P-256/P-384 の ECDHE、RSA-PSS・ECDSA などの署名、X.509 の読み取りと証明書チェーン検証は `sigil-shield-tls` の ADR へ切り出す。そこで要る標準部品は個別に設計する。[RFC 8446 §9.1](https://www.rfc-editor.org/rfc/rfc8446#section-9.1) は P-256 の鍵交換を必須、X25519 を推奨としている。TLS 終端の適合性には P-256 と必須署名方式の対応も要る。P-384 の採否は同 ADR で相互運用要件に基づいて決める。

## 決定 2 — 公開 API

鍵を `Bytes` で受け取る方式には鍵型の追加が少ない利点がある。用途別の鍵型を選び、アルゴリズム間の取り違えを型で防ぎ、長さの確認を型の生成時にまとめる。鍵と PRK は `Secret` で保管し、演算中は借用する。型名とシグネチャは設計上の形を示す。

| 部品 | API の形 |
|---|---|
| HKDF | `hkdf_extract_sha256(salt: Option<Secret>, ikm: &Secret) -> HkdfSha256Prk` と `hkdf_expand_sha256(prk: &HkdfSha256Prk, info: &Bytes, out_len: Int) -> Secret`。SHA-384 も対応する関数と PRK 型を用いる。 |
| AES-128-GCM、AES-256-GCM、ChaCha20-Poly1305 | アルゴリズム別の鍵型と関数名を使う。各方式に `seal(key: &Key, nonce: Nonce96, aad: &Bytes, plaintext: &Secret) -> Result<Bytes, AeadError>` と `open(key: &Key, nonce: &Nonce96, aad: &Bytes, sealed: &Bytes) -> Result<Secret, AeadError>` を設ける。 |
| X25519 | `x25519_public_key(private: &X25519PrivateKey) -> X25519PublicKey` と `x25519_shared_secret(private: &X25519PrivateKey, peer: &X25519PublicKey) -> Result<Secret, X25519Error>`。秘密鍵を用意する側は `std.os_random.bytes` を使う。 |
| SHA-384 | `Sha384.new`、`update(&mut Sha384, &Bytes)`、`finalize(&mut Sha384) -> Bytes` と `sha384(&Bytes) -> Bytes`。`Sha256` にそろえ、出力は 48 バイト。 |

演算は `effects none` とする。秘密鍵の生成入口は `Secret` の所有権を取り、AES-128 は 16 バイト、AES-256・ChaCha20・X25519 は 32 バイトに限定する。X25519 の公開鍵は 32 バイト、AEAD の nonce は 12 バイトの公開型とする。外部入力の長さ違反は型を作る入口で `Result` にする。信頼済みの呼び出し側が指定する HKDF の `out_len` は `0 <= out_len <= 255 * HashLen` を `requires` に置く。出力の長さは `ensures` に置き、外部入力に由来する長さは呼び出し側で事前に検査する。

HKDF は [RFC 5869](https://www.rfc-editor.org/rfc/rfc5869) に従って任意長の salt と IKM を受け付け、salt の省略時は HashLen バイトのゼロ列を使う。`salt` は `Option<Secret>`（所有権を取る）とする。言語は `Option<&T>` を戻り値の位置にしか許さず（[ADR 0012](0012-ownership-check-and-stdlib.md) 決定 1）、`Option<&Secret>` は引数に書けない。salt は使い終えたら破棄・ゼロ化され、同じ salt を再び使う呼び出し側は `expose()` と `Secret.from_bytes` で新しい `Secret` を作る。省略（`None`）は空の鍵として扱う。HMAC は鍵を HashLen を超えるブロック長へゼロ詰めするので、空の鍵と HashLen バイトのゼロ列は同じ鍵になり、RFC 5869 §2.2 と同値である。既存の公開 `hmac_sha256` は鍵長 32 バイト以上を要求するため、Extract は任意長の鍵を扱う内部 HMAC を使う。既存の公開契約は維持する。`HKDF-Expand-Label` の TLS 固有の符号化は `sigil-shield-tls` が担う。

AEAD の出力は暗号文に 16 バイトのタグを連結する。短い入力・公開長上限の超過・認証失敗は `Result` で返し、認証が成功した場合にだけ平文を渡す。割り当てと暗号演算に先立ち、[NIST SP 800-38D](https://csrc.nist.gov/pubs/sp/800/38/d/final) と [RFC 8439](https://www.rfc-editor.org/rfc/rfc8439) の方式別の入力長上限を検査する。`seal` は nonce の値の所有権を取り、呼び出し側が同一鍵で同じ nonce を生成しないことを契約とする。`sigil-shield-tls` は方向別のシーケンス番号、上限に達する前の鍵更新、更新できない場合の接続終了でこの契約を守る。[RFC 8446 §5.3・§5.5](https://www.rfc-editor.org/rfc/rfc8446#section-5.3) の nonce 構成と鍵ごとの利用上限は TLS 側の契約に置く。

X25519 は RFC 7748 のデコード規則を守り、32 バイトの共有値を `Secret` で返す。共有値が全ゼロの場合はエラーにする。秘密と作業中の鍵・平文は表示やエラー文字列に載せず、破棄で消去する。内部処理は `Secret.expose()` による秘密の複製を避ける。

## 決定 3 — 定数時間規則

[ADR 0020](0020-p1-security-addendum.md) の R1〜R6 と ADR 0024 決定 3 を適用する。`ct_rules` の対象表と漏れ検査に公開 API と内部補助関数を登録する。鍵・鍵展開・秘密スカラー・PRK・IKM・salt・平文・秘密入力のハッシュ状態は秘密とする。データの長さ、公開鍵、nonce、AAD、受信暗号文およびタグ、HKDF の info、固定の処理回数は公開とする。秘密由来の値で分岐、ループの打ち切り、アドレス計算、除算・剰余を行わない。AES の S-box と GHASH は秘密を添字にする表引きを禁止し、固定した演算列で処理する。X25519 のスカラー演算も固定回数とし、入れ替えは秘密分岐を持たない。

タグと全ゼロ値の比較は全バイトを処理し、定数時間の計算が終わった境界で成功・失敗の 1 ビットだけを公開する。`CT_PUBLIC_COMPARISONS` の対象は個別の比較関数に絞り、関数の本体とエラーを返す入口の秘密フローを引き続き検査する。失敗時は平文や共有秘密を渡さず、保持した作業領域を消去する。

SIR のテイント検査とネイティブ生成時の CLIF 検査で各部品とその秘密を扱う補助関数を覆い、破棄時のゼロ化を確かめる。現在の CLIF 差分の切り分けで入れ子の呼び出しを覆えないときは、先に検査を拡張する。`ct_measure` には各部品のネイティブ関数を追加する。同じ実行ファイル・同じ公開入力長で固定した秘密と変動させた秘密を比較する。AEAD のタグ照合では同じ長さと同じ失敗結果で不一致の位置を変える。陽性対照で |t| ≥ 4.5、対象で Welch の |t| < 4.5 を確認し、CPU、最適化条件、試行数、t 値を記録する。計測結果は静的検査とあわせて評価し、対応する出力先を個別に確認する。

## 決定 4 — テストベクタ

ADR 0024 決定 4 に従い、出典、節、ケース ID を固定し、実装の出力を期待値とバイト単位で照合する。暗号化と復号の往復試験も行う。

| 部品 | 公開資料と検査値 |
|---|---|
| HKDF-SHA-256 | [RFC 5869 付録 A](https://www.rfc-editor.org/rfc/rfc5869#appendix-A) の SHA-256 ケースで PRK と OKM をそれぞれ比較する。空の salt と info も含む。 |
| AES-128/256-GCM | [NIST FIPS 197](https://csrc.nist.gov/pubs/fips/197/final) の AES 例と [NIST CAVP GCM ベクタ](https://csrc.nist.gov/projects/cryptographic-algorithm-validation-program/cavp-testing-block-cipher-modes#GCMVS) の 96 ビット IV・128 ビットタグのケースで暗号文、タグ、復号、失敗を照合する。 |
| X25519 | [RFC 7748 §5.2・§6.1](https://www.rfc-editor.org/rfc/rfc7748#section-5.2) のスカラー倍算、反復、公開鍵、共有値を照合する。 |
| SHA-384／HKDF-SHA-384 | [RFC 6234](https://www.rfc-editor.org/rfc/rfc6234) と [RFC 4231](https://www.rfc-editor.org/rfc/rfc4231) の SHA-384、HMAC-SHA-384 を使う。Extract と Expand は独立実装で得た SHA-384 専用ベクタを生成元とともに固定する。 |
| ChaCha20-Poly1305 | [RFC 8439 §2.8.2 と付録 A](https://www.rfc-editor.org/rfc/rfc8439#section-2.8.2) の暗号文・タグと復号を照合する。 |

AEAD は空の平文・AAD、ブロック境界、短すぎる入力と長さ上限、暗号文・タグ・AAD の改変による認証失敗を確かめる。HKDF は出力 0、1、HashLen 前後、255 × HashLen と超過を確かめる。X25519 では長さ誤り、全ゼロ結果、RFC の非正規表現の扱いも確認する。既知値のテストは SIR 評価器とネイティブ出力で実行し、ベクタの照合にテスト依存を加えない。TLS 側では [RFC 8448](https://www.rfc-editor.org/rfc/rfc8448) の鍵スケジュールとレコードを照合する。

## 決定 5 — 実装 PR の分割

| 段 | 内容 | レーン |
|---|---|---|
| 1a | HKDF-SHA-256 と内部 HMAC、RFC 5869 ベクタ | D2 |
| 1b | 内部 AES-128、GHASH と AES-128-GCM、NIST ベクタ | D2 |
| 1c | X25519 と公開鍵・共有値の RFC ベクタ | D2 |
| 2a | ChaCha20 と Poly1305、AEAD、そのベクタ | D2 |
| 2b | SHA-384 と HKDF-SHA-384、そのベクタ | D2 |
| 2c | AES-256 鍵展開と AES-256-GCM、そのベクタ | D2 |

1b は規模に応じて内部 AES/GHASH を先行 PR に分ける。第 2 段のうち 2a と 2b は独立しており、レーン D2 の排他に従って別々に進める。2c は 2b の後にスイートとしての組合せを確認する。各実装 PR に対象の検査登録、ベクタ、ネイティブ計測を含める。検査器やネイティブの原始演算の変更が前提になる場合は担当レーンの先行 PR に分け、対象を検査できる状態で公開する。実装は ADR マージ後に別 Issue で扱い、レビュー条件は ADR 0020 と AGENTS.md に従う。

## 影響

`sigil-shield-tls` が使う鍵導出・レコード保護・鍵共有を標準ライブラリの契約として個別に検証できる。TLS 終端の公開前には、別 ADR で決めるグループ・証明書・署名対応を合わせて相互運用と ADR 0020 の公開条件を確認する。

## 正式判定の記録

2026-11-23 03:15 JST に Codex Cloud から typesafe-ai (`jev-1.13.0`) で正式判定した。[ADR 0001](0001-vision-and-scope.md) と [ADR 0027](0027-ai-era-security-design-principles.md) に沿って、契約をレビューしやすくし、秘密値と誤用を型と境界で管理する観点を固定した。以下は各採用案の確率と confidence。

| 判断点 | 採用案 | 確率 | confidence |
|---|---|---:|---:|
| 範囲と優先順位 | 第 1・第 2 優先の段階導入と TLS 終端の別 ADR | 0.79 | 0.68 |
| 公開 API | 用途別の型と `Secret` の借用 | 0.99 | 0.98 |
| 定数時間 | 静的検査と実測 | 0.83 | 0.66 |
| ベクタ | 公開既知解との照合と失敗条件の検査 | 0.89 | 0.78 |
| PR 分割 | D2 中心の部品別 PR | 0.96 | 0.92 |

<details>
<summary>入力 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: ADR 0001 の AI が実装し人間が仕様・契約をレビューする理念、ADR 0027 の誤用を型と境界で防ぐ設計、ADR 0024 の Secret・定数時間・公開ベクタの方針、依存クレートを足さない方針、所有者の「暗号部品をバックエンドより先行」「TLS 終端は専用 sigil-shield-tls」「本件は文書のみ」という方針。RFC 8446 の実際の MUST（必須スイート AES-128-GCM/SHA-256、鍵交換実装の P-256）と SHOULD（X25519 等）、レビュー可能性、小さな PR、失敗時の秘密値の保護を評価する。Issue #676 は汎用暗号部品の ADR であり、認証や準拠を実装完了と見なす判断ではない。",
  "questions": {
    "scope": {
      "type": "choice",
      "criteria": {
        "staged": "第1段 HKDF-SHA256→AES-128-GCM→X25519。第2段 ChaCha20-Poly1305 と SHA-384→AES-256-GCM。P-256 は RFC 準拠の別 ADR の必須前提、P-384 は別途需要で判断。署名・X.509/検証は shield 側 ADR で仕様・責務を決める",
        "expand": "第1段に P-256 と署名・X.509 まで含め準拠まで一つの ADR で扱う",
        "minimal": "第1段だけ決め後続の優先順位は別 ADR"
      }
    },
    "api": {
      "type": "choice",
      "criteria": {
        "typed": "サイズを検査した用途別の型と借用 Secret、暗号化入力・認証済み復号結果・導出結果・共有秘密を保護する。外部入力は Result、内部で保証できる条件は契約。公開 AEAD は固定 nonce/tag、汎用ブロック API は内部。nonce 一意性を暗号化側の所有・上位プロトコル契約で担保",
        "bytes": "既存 Bytes/Secret だけでサイズ・用途の区別は各呼び出しの契約に任せる"
      }
    },
    "ct": {
      "type": "choice",
      "criteria": {
        "both": "ct_rules で秘密依存分岐・添字等を新規部品と呼び先で検査し、ct_measure で等しい公開条件の秘密値差分を出力先別に測る。秘密添字の AES テーブルを排除し、認証結果による分岐は定数時間確認後に限定",
        "measure": "実測中心で秘密添字や呼び先を静的規則に含めない"
      }
    },
    "vectors": {
      "type": "choice",
      "criteria": {
        "positive_negative": "RFC/NIST の公開既知解を出典付きで固定し完全一致、入力境界・改変と認証失敗・異常点・出力先の差も検査。RFC 8448 は shield の結合検証にも利用",
        "positive": "ラウンドトリップと公開正常系の一部だけ"
      }
    },
    "prs": {
      "type": "choice",
      "criteria": {
        "small": "D2 中心で HKDF、AES 基盤と AES-128-GCM、X25519、ChaCha20-Poly1305、SHA-384/HKDF-SHA384、AES-256-GCM を依存順に小さく分け、各公開部品のベクタと定数時間検査を同じ着地条件にする",
        "one": "複数の公開部品を一つの標準ライブラリ PR にまとめテストは後続"
      }
    }
  }
}
```

</details>

<details>
<summary>応答 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "scope": {
      "type": "choice",
      "choice": "staged",
      "confidence": 0.68,
      "probabilities": {
        "staged": 0.79,
        "expand": 0.01,
        "minimal": 0.2
      }
    },
    "api": {
      "type": "choice",
      "choice": "typed",
      "confidence": 0.98,
      "probabilities": {
        "typed": 0.99,
        "bytes": 0.01
      }
    },
    "ct": {
      "type": "choice",
      "choice": "both",
      "confidence": 0.66,
      "probabilities": {
        "both": 0.83,
        "measure": 0.17
      }
    },
    "vectors": {
      "type": "choice",
      "choice": "positive_negative",
      "confidence": 0.78,
      "probabilities": {
        "positive_negative": 0.89,
        "positive": 0.11
      }
    },
    "prs": {
      "type": "choice",
      "choice": "small",
      "confidence": 0.92,
      "probabilities": {
        "small": 0.96,
        "one": 0.04
      }
    }
  },
  "usage": {
    "input_tokens": 1383,
    "output_tokens": 155
  }
}
```

</details>

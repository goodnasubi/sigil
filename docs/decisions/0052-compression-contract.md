# 0052: 圧縮の契約

- **状態**: 提案（typesafe-ai 正式判定済み、別系統レビュー前）
- **日付**: 2026-11-20（JST）
- **対象**: [Issue #635](https://github.com/goodnasubi/sigil/issues/635)、レーン E
- **仕様基準**: sigil の `main`、`33467e3b43238fdc69331c58a788c95f1de393e5`
- **実装確認基準**: [sigil-git の main](https://github.com/goodnasubi/sigil-git/tree/b64b23fecab923d89293f6a1c0d4d17f73aa49a2)、`b64b23fecab923d89293f6a1c0d4d17f73aa49a2`

## 背景

[ADR 0045](0045-p4-1-database-core.md) 決定 1 が fs から切り出した共通圧縮契約を、純 Sigil／`trust none` で定める。外部ライブラリ・`raw`・新しい信頼基盤を導入しない。[ADR 0047](0047-p4-3-repository-core.md)「前提と持ち越し」2 の zlib／deflate を満たし、[ADR 0048](0048-p4-4-repository-transfer-and-hosting.md) 決定 1 の gzip フレーム検査と CRC-32 を含める。P4-4 第1段の転送 codec と wire 専用 pack 出力は [sigil-git PR #10](https://github.com/goodnasubi/sigil-git/pull/10) で完了した。第2段の upload-pack は本契約の決定を前提とする。

## 判断

| 判断点 | 採用 | 他案を採らない理由 |
| --- | --- | --- |
| 1. 圧縮形式 | zlib の完全な inflate、stored ブロックによる zlib 出力、upload-pack POST の単一 gzip member の復号 | stored 専用の復号や gzip の除外は実物の Git との比較を満たせない。圧縮率を求める deflate エンコーダや gzip 出力を必須にすると初版の検証範囲が広がる。 |
| 2. 資源上限 | 符号化入力・出力・作業メモリを個別に制限し、適用される上限の最小値を守る | 展開量だけでは、空ブロックの列や一時領域の消費を抑えられない。固定の展開率は、高圧縮でも絶対上限内の正常データを拒む。 |
| 3. 検査値 | 各フレームの検査値を必須にし、pack index の CRC 欄の必須化は ADR 0047 の実装判断に委ねる | 外側の整合性検査で代用するとフレーム単位の破損を見逃す。pack index の CRC 欄の必須化は、gzip 受信の成立に不要で、pack 検証の責務を広げる。 |
| 4. 拒否条件 | 全検証に成功した出力だけを返し、破損・打切り・未対応・上限超過を固定理由で拒否する | 検査前の部分出力や余剰の黙認は、受理したバイト列の境界を曖昧にし、無効な要求が処理に届く。 |
| 5. 検証 | 既知ベクトル、本物の Git との比較、ファジング、性能記録を完了時にそろえる | ベクトルだけでは実物のフレームや組合せを覆えない。性能値による合否は [ADR 0044](0044-p4-scope.md) 決定 1 の初版の方針に反する。 |
| 6. 既存実装との適合 | 固定 SHA で静的に照合し、差分は別 Issue で扱う | 既存の欠けた検査に合わせて契約を緩めると先行 ADR の保証を失う。実装を混ぜると契約の独立したレビューができない。 |

## フレームと検査値

- **zlib**（[RFC 1950](https://www.rfc-editor.org/rfc/rfc1950)）はヘッダの方式・窓サイズ・FCHECK と末尾の Adler-32 を検証する。FDICT が立つ入力は辞書を調達せず拒否する。中身は [RFC 1951](https://www.rfc-editor.org/rfc/rfc1951) の stored・固定・動的 Huffman ブロックを受理し、終端と長さ・距離を検証する。後方参照は既に展開した範囲と zlib が宣言した窓の両方に収める。書出しは Adler-32 付きの有効な stored ストリームで足りる。
- **gzip**（[RFC 1952](https://www.rfc-editor.org/rfc/rfc1952)）は upload-pack POST に限り、認証・認可後に単一 member を受理する。magic、方式、予約フラグ、任意項目の長さ・終端、FHCRC があればヘッダ CRC-32 の下位 16 bit、deflate 終端、trailer の展開データ CRC-32 と ISIZE（実際に展開した長さの下位 32 bit）を検証する。gzip の検査値に zlib の Adler-32 は使わない。gzip 出力と HTTP 応答の圧縮は提供しない。
- **境界**。緩いオブジェクトと gzip 本文は、フレーム全体が与えられた入力をちょうど消費することを検査する。pack は各オブジェクトの zlib ストリームを検証し、検査値末尾までの消費長を pack 呼び出し側に返して次の境界を決める。pack 側で宣言長・展開実長とエントリ／trailer の境界を検証する。次の正規エントリの存在は余剰に当たらない。
- **pack index**。現行のローカル読み取りは `.idx` の `crc32` 欄を検証に使わず、オブジェクトの OID を再計算する。CRC 欄を検証に使うかは ADR 0047「前提と持ち越し」4 の実装 PR で扱う。gzip CRC-32 は独立した必須検査であり、既存の pack／OID 検証を置き換えない。

## 上限と拒否

1 MiB は 1,048,576 byte。値は上限を含み、1 byte でも超える操作を拒否する。

| 文脈 | 符号化入力 | 展開量・応答 | 作業メモリ |
| --- | --- | --- | --- |
| ローカルの緩い形式 | 1 zlib ファイル 80 MiB | 内容 64 MiB とヘッダ最大 64 byte | ADR 0047 の核全体で 256 MiB |
| ローカル pack | 1 zlib ストリーム 80 MiB、pack 全体は 4 GiB（位置指定読み取り） | エントリの展開と delta 復元各 64 MiB | 同上 |
| Git 転送 | 受信は要求本文の上限に従う | 1 オブジェクト 8 MiB、1 転送の累計 16 MiB、4096 オブジェクト。HTTP 応答は ACK・side-band・pack trailer を含めて 1 MiB | ADR 0048 のアプリケーション全体で 256 MiB。同時 Git 処理は全体 2、同一 repo 1 |
| upload-pack POST の gzip | ヘッダ・trailer を含め 2 MiB | 展開後の要求本文 1 MiB | 同上 |
| identity／符号化指定なしの HTTP 要求 | 本文の実効上限 1 MiB | 同じ 1 MiB | 同上 |

- 80 MiB はローカルの新しい入力上限で、64 MiB の内容にヘッダと圧縮フレームの余裕を確保する。pack 全体をメモリに載せない。ローカルの上限と転送の上限が同時に適用されるときは厳しい方を使う。各処理で文脈の上限、転送の残予算、信用できない宣言長から得た上限のうち最小値を使う。pack や delta の宣言長は処理前に検査し、実長との一致も検証する。gzip の ISIZE に基づく割当ては行わない。
- 入力・未検証出力・窓・復号表・一時コピー・同時処理分を作業メモリに計上し、保持や割当ての前に予約する。長さ・加算・offset を先に検査し、展開中は各追加の前に残量を確認する。累計には delta の解凍と復元の一時出力も計上し、バッファを解放しても累計は戻さない。上限に収まらない予約、読み取り、展開はその時点で停止し、未検証の出力を破棄する。比率にかかわらず、入力・展開量・メモリのどれかが尽きる圧縮爆弾に同じ規則を適用する。
- gzip 本文を pkt-line、交渉、更新処理に渡すのは全検証後に限る。下表は HTTP 応答開始前の対応。ローカルの zlib 復号には HTTP 状態を返さず、分類と固定理由を使う。Git 応答開始後は ADR 0048 の `ERR`／`ng` を使う。公開する理由には入力内容・内部パス・秘密を含めない。

| 条件 | 分類 | 固定理由 | 該当する HTTP 状態 |
| --- | --- | --- | --- |
| 不正なヘッダ・符号・距離・長さ、検査値不一致 | `Corrupt` | `invalid_compressed_data` | 400 |
| フレーム・ブロック・検査値の途中で入力終了 | `Corrupt` | `truncated_compressed_data` | 400 |
| zlib preset dictionary | `UnsupportedFormat` | `unsupported_preset_dictionary` | 400 |
| 完結したフレーム後の連結 member・余剰データ | `Corrupt` | `trailing_compressed_data` | 400 |
| 入力・展開量・メモリ・転送予算を超過 | `LimitExceeded` | `compression_limit_exceeded` | 413 |
| 未対応・多重 `Content-Encoding`、upload-pack POST 以外の gzip | `UnsupportedFormat` | `unsupported_content_encoding` | 415 |

認証・認可の拒否は ADR 0048 に従う。符号化の種類が未対応なら本文を復号せず 415 とし、対応形式は読取り・展開中に上限超過を検出した時点で 413 とする。宣言長が上下限内でも、実長不一致は 400 とする。ただし出力予算超過で先に停止した場合は 413 とする。

## 実装時の完了条件

以下を評価器とネイティブで確認し、fixture と結果を実装リポジトリに保存する。

1. **既知ベクトル**。stored（空・複数ブロックを含む）、固定・動的 Huffman、gzip の各任意項目と FHCRC について、期待する出力・消費長・検査値を固定する。Adler-32／CRC-32／ISIZE／FHCRC の不一致、不正な構造・窓参照、打切り、辞書、連結・余剰、未対応・多重符号化を固定 fixture で拒否し、表の理由と HTTP 状態を確認する。
2. **境界**。各上限の直下・一致・1 byte 超過、宣言長不一致、整数 overflow、極小入力からの大きな展開、出力を生まない入力、同時予約と累計超過を確認する。拒否時は部分出力を成功として取得できず、gzip 要求処理が開始されないことを確認する。
3. **実物の Git**。SHA-256 対応 Git の版と実行物の検査値を固定する。Git が生成した loose と pack を読み、出力した stored zlib の loose と wire pack を Git が受理することを、内容・OID の一致と `git fsck` で確認する。実際の clone／fetch が送る gzip 要求を捕捉し、検証後の本文が同じ pkt-line を表すことと相互運用を確認する。
4. **ファジングと性能**。zlib、deflate、gzip、pack の境界と予算を対象に seed・corpus・実行時間／試行数・回帰例を記録する。クラッシュ、上限迂回、未検証出力、両経路の結果不一致を欠陥として扱う。固定の実行版・入力・環境で経過時間、スループット、ピーク作業メモリを両経路で記録する。性能値は合否に使わず、契約上の資源上限は境界試験で検証する。

## sigil-git の適合確認

| 参照 | 確認結果 |
| --- | --- |
| [`git/inflate.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/inflate.sigil) | `inflate_zlib` は 3 種のブロックを読み、出力上限超過で `LimitExceeded`、preset dictionary に `UnsupportedFormat` を返す。Adler-32 検証後にだけ出力と消費長を返す。CINFO の範囲は検査するが、後方距離の上限を宣言窓に結び付けていない。汎用の入力上限・作業メモリ予約・固定理由への変換はこの関数にない。 |
| [`git/zlib.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/zlib.sigil) | `adler32` と `zlib_stored` は必要な書出しを備える。`zlib_decode_stored` は stored 専用で、`verify_tail=false` の成功は checksum 未検証のため契約準拠の復号結果として扱えない。 |
| [`git/odb.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/odb.sigil)／[`git/pack.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/pack.sigil) | 緩い形式は全消費を確認する。pack のローカル個別読取りは宣言長と実長を照合するが、消費長を境界照合に使っていない。緩い形式の入力は `64 MiB + 64 + 16384 byte`、pack の読み窓は `宣言長 + 宣言長 / 8 + 256 byte` で、表の入力契約と上限到達時の分類に合わせた確認が必要。 |
| gzip | 固定版に gzip フレーム検査・CRC-32・ISIZE 検証は未実装。 |

上表はソースの確認結果である。適合しない箇所と未実装の gzip／資源予算／固定理由は別の実装 Issue で扱い、実装完了時に検証を記録する。SHA-1 対応、新しい標準 API、upload-pack の交渉処理はそれぞれの後続作業に委ねる。

## 正式判定の記録

2026-11-20（JST）、Codex Cloud が typesafe-ai `jev-1.13.0` を実行し HTTP 200 を受信した。入力と応答の保存場所は本節末尾の JSON。表には採用案の確率と confidence を示す。

| 判断点 | 採用した選択肢 | 確率 | confidence |
| --- | --- | ---: | ---: |
| 1. 圧縮形式 | `bounded_formats` | 0.80 | 0.70 |
| 2. 資源上限 | `minimum_budgets` | 1.00 | 1.00 |
| 3. 検査値 | `frame_checks_defer_index` | 0.96 | 0.94 |
| 4. 拒否条件 | `strict_whole_input` | 1.00 | 1.00 |
| 5. 検証 | `vectors_git_fuzz_measure` | 0.84 | 0.76 |
| 6. 既存実装との適合 | `audit_and_follow_up` | 1.00 | 1.00 |

<details>
<summary>入力 JSON（typesafe-adr0052-input.json）</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: 人間がレビューできる小さい契約、最小の信頼基盤、失敗時の明示的拒否、実物 Git との相互運用を優先する。所有者の Issue #635 は ADR 0052 と索引の予約のみ、実装・処理系・バックログ変更とマージは対象外、判定は typesafe-ai のみ。ADR 0045 に従い fs から分けた純 Sigil・trust none とし、ADR 0047 のローカル SHA-256 Git と ADR 0048 の Smart HTTP v0 の制約を保持する。性能は ADR 0044 のとおり計測するが合否に使わない。仕様 main 33467e3b43238fdc69331c58a788c95f1de393e5、実装 sigil-git main b64b23fecab923d89293f6a1c0d4d17f73aa49a2 を参照した。既存の inflate_zlib は stored/固定/動的、出力上限、Adler-32、辞書拒否、consumed を持つ。緩い形式の呼び出し側は全消費を確認する。zlib_decode_stored の verify_tail=false は未検証の成功を返し得る。汎用の入力予算・メモリ予約と gzip は未確認または未実装。pack 読取は zlib の窓を宣言長+1/8+256 byte まで伸ばし、.idx CRC 欄を検証に使わない。形式仕様は RFC 1950/1951/1952。上限案: ローカル loose body 64 MiB（ヘッダ最大64 byteを別計上）、ローカル pack エントリの展開・delta 復元各64 MiB、loose ファイル／pack 1ストリームの符号化入力各80 MiB、pack 全体4 GiBは位置指定、核作業メモリ256 MiB。転送は単体8 MiB・累計16 MiB、アプリ全作業メモリ256 MiB（同時処理含む）。展開後要求1 MiB、応答全体1 MiB、gzip符号化2 MiB、identity実効1 MiB。予算は文脈・残量・宣言長の最小、宣言長はある文脈のみ使用しgzip ISIZEから予約しない。比率でなく絶対上限、越える割当や書込の直前に停止。正規framingの外部入力だけ完全消費を要求し、packでは検証済み消費長で次の境界へ進む。フォーマットの破損・打切り・未対応辞書・連結・余剰はHTTP入口で該当時400、上限413、未対応・多重Content-Encoding 415、認証認可後の全検証まで要求処理を始めない。CRC-32はgzip FHCRCの下位16bitとtrailer（ISIZEと併用）に用い、zlibのAdler-32とは分離する。",
  "questions": {
    "formats": {
      "type": "choice",
      "criteria": {
        "bounded_formats": "必須の完全inflateとstored書出し、upload-pack POSTだけの単一gzip読取を契約化する",
        "encoder_required": "圧縮率を高めるdeflateエンコーダとgzip出力も初版に要求する",
        "zlib_only": "zlibのみ契約化しgzipは範囲外にする"
      }
    },
    "limits": {
      "type": "choice",
      "criteria": {
        "minimum_budgets": "固定上限案とより厳しい上限の適用、処理前予約と消費途中の検査、比率によらない絶対制限",
        "ratio_gate": "固定の展開率上限を追加して高圧縮の正常データも拒否",
        "expansion_only": "展開量だけ制限し入力と作業メモリは独立に制限しない"
      }
    },
    "checksums": {
      "type": "choice",
      "criteria": {
        "frame_checks_defer_index": "zlib Adler-32 と gzip CRC-32/ISIZE/FHCRC を厳格に検査し pack index CRC 欄の必須化は ADR 0047 の実装 PR 判断に委ねる",
        "require_index": "同時に pack index CRC 欄の検証を必須化する",
        "transport_only": "zlib/gzip の検査値の一方または両方を外側の整合性検査で代替する"
      }
    },
    "rejection": {
      "type": "choice",
      "criteria": {
        "strict_whole_input": "破損・打切り・辞書・連結・余剰・未対応多重符号化を固定理由で拒否し、検証後にだけ要求処理を始める。HTTP対応はstateどおり",
        "permissive_prefix": "検証前の部分展開を要求処理に渡し連結memberや余剰を許容する"
      }
    },
    "validation": {
      "type": "choice",
      "criteria": {
        "vectors_git_fuzz_measure": "既知の正負ベクトル、固定 Git 比較、再現可能なファジング、両実行経路の性能記録を完了基準にする。性能値は合否に使わない",
        "vectors_only": "ベクトルのみで完了判定する",
        "performance_gate": "上に加えて初版の性能値を必須合否閾値にする"
      }
    },
    "conformance": {
      "type": "choice",
      "criteria": {
        "audit_and_follow_up": "実装を固定SHAで静的確認し適合部分と未実装・未確認を記録する。不適合は別Issueで扱い、未検証のstored経路は契約の成功として使わない",
        "adopt_existing": "既存実装の現状に合わせて契約を緩める",
        "patch_now": "本ADR PRに圧縮実装修正も混ぜる"
      }
    }
  }
}
```

</details>

<details>
<summary>応答 JSON（typesafe-adr0052-output.json）</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "formats": {
      "type": "choice",
      "choice": "bounded_formats",
      "confidence": 0.7,
      "probabilities": {
        "bounded_formats": 0.8,
        "encoder_required": 0.0,
        "zlib_only": 0.2
      }
    },
    "limits": {
      "type": "choice",
      "choice": "minimum_budgets",
      "confidence": 1.0,
      "probabilities": {
        "minimum_budgets": 1.0,
        "ratio_gate": 0.0,
        "expansion_only": 0.0
      }
    },
    "checksums": {
      "type": "choice",
      "choice": "frame_checks_defer_index",
      "confidence": 0.94,
      "probabilities": {
        "frame_checks_defer_index": 0.96,
        "require_index": 0.01,
        "transport_only": 0.03
      }
    },
    "rejection": {
      "type": "choice",
      "choice": "strict_whole_input",
      "confidence": 1.0,
      "probabilities": {
        "strict_whole_input": 1.0,
        "permissive_prefix": 0.0
      }
    },
    "validation": {
      "type": "choice",
      "choice": "vectors_git_fuzz_measure",
      "confidence": 0.76,
      "probabilities": {
        "vectors_git_fuzz_measure": 0.84,
        "vectors_only": 0.15,
        "performance_gate": 0.01
      }
    },
    "conformance": {
      "type": "choice",
      "choice": "audit_and_follow_up",
      "confidence": 1.0,
      "probabilities": {
        "audit_and_follow_up": 1.0,
        "adopt_existing": 0.0,
        "patch_now": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 2019,
    "output_tokens": 268
  }
}
```

</details>

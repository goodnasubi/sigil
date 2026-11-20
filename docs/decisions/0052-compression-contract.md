# 0052: 圧縮の契約

- **状態**: 提案（原案の 6 判断点とレビュー対応の 2 判断点を判定済み、再レビュー待ち）
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

- **zlib**（[RFC 1950](https://www.rfc-editor.org/rfc/rfc1950)）はヘッダの方式・窓サイズ・FCHECK と末尾の Adler-32 を検証する。FDICT が立つ入力は辞書を調達せず拒否する。中身は [RFC 1951](https://www.rfc-editor.org/rfc/rfc1951) の stored・固定・動的 Huffman ブロックを受理し、終端と長さ・距離を検証する。後方参照は既に展開した範囲と CINFO が宣言した窓の両方に収める。zlib 1.3.1 の `inflate` は `INFLATE_STRICT` を有効にした場合に宣言窓を検査する。本契約ではこの検査を常に必須とし、既定の zlib ビルドより意図的に厳格にする。書出しは Adler-32 付きの有効な stored ストリームで足りる。[zlib の通常経路](https://github.com/madler/zlib/blob/v1.3.1/inflate.c#L1122-L1128)・[高速経路](https://github.com/madler/zlib/blob/v1.3.1/inffast.c#L156-L162)
- **gzip**（[RFC 1952](https://www.rfc-editor.org/rfc/rfc1952)）は upload-pack POST に限り、認証・認可後に単一 member を受理する。magic、方式、予約フラグ、任意項目の長さ・終端、FHCRC があればヘッダ CRC-32 の下位 16 bit、deflate 終端、trailer の展開データ CRC-32 と ISIZE（実際に展開した長さの下位 32 bit）を検証する。gzip には窓の宣言欄がなく、後方参照は既に展開した範囲と DEFLATE の最大距離 32 KiB の両方に収める。zlib の宣言窓にも DEFLATE のこの最大距離が適用される。gzip の検査値に zlib の Adler-32 は使わない。gzip 出力と HTTP 応答の圧縮は提供しない。
- **境界**。緩いオブジェクトと gzip 本文は、フレーム全体が与えられた入力をちょうど消費することを検査する。pack は各オブジェクトの zlib ストリームを検証し、検査値末尾までの消費長を pack 呼び出し側に返して次の境界を決める。pack 側で宣言長・展開実長とエントリ／trailer の境界を検証する。次の正規エントリの存在は余剰に当たらない。
- **pack index**。現行のローカル読み取りは `.idx` の `crc32` 欄を検証に使わず、オブジェクトの OID を再計算する。CRC 欄を検証に使うかは本契約の範囲外とし、ADR 0047「前提と持ち越し」4 を参照して後続の pack 検証 Issue に明記し、その実装 PR で決める。gzip CRC-32 は独立した必須検査であり、既存の pack／OID 検証を置き換えない。

## 上限と拒否

1 MiB は 1,048,576 byte。値は上限を含み、1 byte でも超える操作を拒否する。

| 文脈 | 符号化入力 | 展開量・応答 | 作業メモリ |
| --- | --- | --- | --- |
| ローカルの緩い形式 | 1 zlib ファイル 80 MiB | 内容 64 MiB とヘッダ最大 64 byte | ADR 0047 の核全体で 256 MiB |
| ローカル pack | 1 zlib ストリーム 80 MiB、pack 全体は 4 GiB（位置指定読み取り） | エントリの展開と delta 復元各 64 MiB | 同上 |
| Git 転送 | 受信は要求本文の上限に従う | 1 オブジェクト 8 MiB、1 転送の累計 16 MiB、4096 オブジェクト。HTTP 応答は ACK・side-band・pack trailer を含めて 1 MiB | ADR 0048 のアプリケーション全体で 256 MiB。同時 Git 処理は全体 2、同一 repo 1 |
| upload-pack POST の gzip | ヘッダ・trailer を含め 2 MiB | 展開後の要求本文 1 MiB | 同上 |
| identity／符号化指定なしの HTTP 要求 | 本文の実効上限 1 MiB | 同じ 1 MiB | 同上 |

- ローカルの 80 MiB は本 ADR が定める符号化入力の上限であり、展開後 64 MiB・核の作業メモリ 256 MiB などを定める ADR 0047 決定 5 を補足する。80 MiB の出典は本 ADR とし、決定 5 の改訂は不要。64 MiB の内容にヘッダと圧縮フレームの余裕を確保する。pack 全体をメモリに載せない。ローカルの上限と転送の上限が同時に適用されるときは厳しい方を使う。出力は文脈の上限、転送の残予算、適用される宣言長をすべて満たす範囲に制限する。gzip の ISIZE に基づく割当ては行わない。
- 入力・未検証出力・窓・復号表・一時コピー・同時処理分を作業メモリに計上し、保持や割当ての前に予約する。長さ・加算・offset を先に検査し、展開中は各追加の前に残量を確認する。累計には delta の解凍と復元の一時出力も計上し、バッファを解放しても累計は戻さない。上限に収まらない予約、読み取り、展開はその時点で停止し、未検証の出力を破棄する。比率にかかわらず、入力・展開量・メモリのどれかが尽きる圧縮爆弾に同じ規則を適用する。
- gzip 本文を pkt-line、交渉、更新処理に渡すのは全検証後に限る。下表は HTTP 応答開始前の対応。ローカルの zlib 復号には HTTP 状態を返さず、分類と固定理由を使う。Git 応答開始後は ADR 0048 の `ERR`／`ng` を使う。公開する理由には入力内容・内部パス・秘密を含めない。

| 条件 | 分類 | 固定理由 | 該当する HTTP 状態 |
| --- | --- | --- | --- |
| 不正なヘッダ・符号・距離・長さ、検査値不一致 | `Corrupt` | `invalid_compressed_data` | 400 |
| フレーム・ブロック・検査値の途中で入力終了 | `Corrupt` | `truncated_compressed_data` | 400 |
| zlib preset dictionary | `UnsupportedFormat` | `unsupported_preset_dictionary` | 400 |
| 完結したフレーム後の連結 member・余剰データ | `Corrupt` | `trailing_compressed_data` | 400 |
| 入力・展開量・転送予算を超過、または作業メモリの予算を予約できない | `LimitExceeded` | `compression_limit_exceeded` | 413 |
| 未対応・多重 `Content-Encoding`、upload-pack POST 以外の gzip | `UnsupportedFormat` | `unsupported_content_encoding` | 415 |

### 検査順序

評価器とネイティブは次の順に論理的な検査を行い、最初の失敗で停止して表の分類・固定理由・HTTP 状態を返す。読取りの分割やデコーダ内の先読みで順序を変えない。

1. HTTP では ADR 0048 の認証・認可に従った後、本文の長さ確認や読取りの前に `Content-Encoding` を検査する。未指定または単一の `identity` を受理し、単一の `gzip` は upload-pack POST で受理する。符号化名は ASCII の大文字・小文字を区別しない。ADR 0048 が列挙した名前に限って受理するため、RFC が同等の扱いを推奨する `x-gzip` も未対応として扱う。別名と、複数のヘッダ行も含めた多重符号化は `unsupported_content_encoding`（415）とする。[RFC 9110 §8.4](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.4)・[§8.4.1.3](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.4.1.3)
2. 確定した符号化入力長を読み取り・展開前に検査する。pack・delta などの宣言長は取得直後に形式と範囲を検査する。宣言長が適用される出力・転送予算を超える場合は `compression_limit_exceeded`（413）。宣言長の形式が不正な場合は `invalid_compressed_data`（400）。対応する HTTP 本文は長さの指定や分割方法にかかわらず、フレームの検査前に本文の終端か、符号化上限を超える最初の byte まで確認する。有効な `Content-Length` が上限を超える場合も本文を読む前に同じ理由で拒否する。緩い形式もファイル長を先に検査し、上限内で入力全体を取得してからフレームを検査する。pack ではそのストリームの消費位置だけを数え、必要な次の byte が pack 側の読み取り境界内に存在し上限を超えるなら 413、入力が尽きていれば打切りとする。次のエントリの先読みは現ストリームの消費に含めない。
3. フレームヘッダ、DEFLATE ブロックの順に進み、フィールドと符号を入力順に検査する。zlib 固定ヘッダの方式・窓・FCHECK を確認した後で FDICT を判定し、辞書指定は DICTID を読む前に拒否する。展開の単位は、LEN／NLEN 確認後の stored の 1 byte、Huffman の 1 literal、または長さと距離を検証済みの 1 後方参照とする。各単位に必要な入力の打切りや符号の破損を先に判定する。有効な単位ごとに独立した出力・転送予算、宣言長の超過、作業メモリ予約の順に確認してから追加する。予算と宣言長の両方を超える単位は 413、宣言長だけを超える単位は `invalid_compressed_data`（400）とする。
4. DEFLATE の終端に達したら展開実長と適用される宣言長の一致を確かめ、次に trailer のフィールドを入力順に検査し、最後に入力全消費または pack の境界を確認する。全検査が終わってから出力を渡す。

入力・出力の保持や割当てに必要な予約はその直前に行う。要求単体と、他の要求が使用中の全体作業メモリが原因の予約失敗は、同じ `compression_limit_exceeded`（413）とする。

| 重なる条件 | 先に返す理由と HTTP 状態 |
| --- | --- |
| 出力が上限ちょうどで次の符号が破損／有効な単位が先に上限を超え、後続が破損 | 順に `invalid_compressed_data`（400）／`compression_limit_exceeded`（413） |
| 途中で打ち切られたデータで、事前に読んだ宣言長自体が予算超過／宣言長は予算内だが次の出力単位が打切り | 順に `compression_limit_exceeded`（413）／`truncated_compressed_data`（400） |
| 有効な出力単位が宣言長のみを超過／宣言長と出力予算を同時に超過し、いずれも後続で打切り | 順に `invalid_compressed_data`（400）／`compression_limit_exceeded`（413） |
| 有効な出力単位で予算超過、後続の検査値も不一致 | `compression_limit_exceeded`（413） |
| gzip ヘッダ不正かつ符号化本文が 2 MiB ちょうど／2 MiB 超過 | 順に `invalid_compressed_data`（400）／`compression_limit_exceeded`（413） |
| 未対応の符号化かつ本文長が上限超過 | `unsupported_content_encoding`（415）。本文を読まない |

## 実装時の完了条件

以下を評価器とネイティブで確認し、fixture と結果を実装リポジトリに保存する。

1. **既知ベクトル**。stored（空・複数ブロックを含む）、固定・動的 Huffman、gzip の各任意項目と FHCRC について、期待する出力・消費長・検査値を固定する。32 KiB 未満の窓を宣言し十分な展開済み履歴がある zlib で、CINFO の宣言窓ちょうどの距離を受理し、宣言窓を 1 byte 超える距離を `invalid_compressed_data` で拒否する。gzip では 32 KiB ちょうどの距離を受理する。Adler-32／CRC-32／ISIZE／FHCRC の不一致、不正な構造・窓参照、打切り、辞書、連結・余剰、未対応・多重符号化を固定 fixture で拒否し、表の理由と HTTP 状態を確認する。認める符号化名の大文字・小文字の違いと別名の扱いも確認する。
2. **境界**。各上限の直下・一致・1 byte 超過、宣言長不一致、整数 overflow、極小入力からの大きな展開、出力を生まない入力、同時予約と累計超過を確認する。検査順序の表の全組合せを固定 fixture にし、`Content-Length` の有無や読み取りの分割位置を変えても、評価器とネイティブで表の分類・理由・該当する HTTP 状態が一致することを確かめる。pack は入力上限ちょうどでの打切りと、境界内に次の byte があり上限を超える場合を区別する。拒否時は部分出力を成功として取得できず、gzip 要求処理が開始されないことを確認する。
3. **実物の Git**。SHA-256 対応 Git の版と実行物の検査値を固定する。Git が生成した loose と pack を読み、出力した stored zlib の loose と wire pack を Git が受理することを、内容・OID の一致と `git fsck` で確認する。実際の clone／fetch が送る gzip 要求を捕捉し、検証後の本文が同じ pkt-line を表すことと相互運用を確認する。zlib の宣言窓を超える距離の拒否は、上の契約固有のベクトルで確かめる。
4. **ファジングと性能**。zlib、deflate、gzip、pack の境界と予算を対象に seed・corpus・実行時間／試行数・回帰例を記録する。クラッシュ、上限迂回、未検証出力、両経路の結果不一致（分類・固定理由・該当する HTTP 状態を含む）を欠陥として扱う。固定の実行版・入力・環境で経過時間、スループット、ピーク作業メモリを両経路で記録する。性能値は合否に使わず、契約上の資源上限は境界試験で検証する。

## sigil-git の適合確認

| 参照 | 確認結果 |
| --- | --- |
| [`git/inflate.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/inflate.sigil) | `inflate_zlib` は 3 種のブロックを読み、出力上限超過で `LimitExceeded`、preset dictionary に `UnsupportedFormat` を返す。Adler-32 検証後にだけ出力と消費長を返す。CINFO の範囲は検査するが、後方距離の上限を宣言窓に結び付けていない。汎用の入力上限・作業メモリ予約・固定理由への変換はこの関数にない。 |
| [`git/zlib.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/zlib.sigil) | `adler32` と `zlib_stored` は必要な書出しを備える。`zlib_decode_stored` は stored 専用で、`verify_tail=false` の成功は checksum 未検証のため契約準拠の復号結果として扱えない。 |
| [`git/odb.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/odb.sigil)／[`git/pack.sigil`](https://github.com/goodnasubi/sigil-git/blob/b64b23fecab923d89293f6a1c0d4d17f73aa49a2/git/pack.sigil) | 緩い形式は全消費を確認する。pack のローカル個別読取りは宣言長と実長を照合するが、消費長を境界照合に使っていない。緩い形式の入力は `64 MiB + 64 + 16384 byte`、pack の読み窓は `宣言長 + 宣言長 / 8 + 256 byte` で、表の入力契約と上限到達時の分類に合わせた確認が必要。 |
| gzip | 固定版に gzip フレーム検査・CRC-32・ISIZE 検証は未実装。 |

上表はソースの確認結果である。適合しない箇所と未実装の gzip／資源予算／固定理由は別の実装 Issue で扱い、実装完了時に検証を記録する。SHA-1 対応、新しい標準 API、upload-pack の交渉処理はそれぞれの後続作業に委ねる。

## 正式判定の記録

2026-11-20（JST）、Codex Cloud が typesafe-ai `jev-1.13.0` を実行し HTTP 200 を受信した。実行時刻は保存した判定記録から確認できない。入力と応答は本節末尾の JSON に収録した。表には判定対象とした 6 判断点の選択肢と、その確率・confidence を示す。

80 MiB、2 MiB、64 byte などの具体値、固定理由名と HTTP 状態の個別の割当ては起草者が契約案に記載した。2 MiB など既に定められた上限や HTTP 状態は該当する先行 ADR に従う。判定にはこれらを背景として示し、個別の値・名称・割当てを選択肢として比較していない。

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

### レビュー対応の追加判定

2026-11-20 **12:46:19 JST**、Codex Cloud が typesafe-ai `jev-1.13.0` を実行し HTTP 200 を受信した。追加の対象は検査順序と HTTP 符号化名の 2 判断点。入力と応答は以下の JSON に収録した。

| 判断点 | 採用した選択肢 | 確率 | confidence |
| --- | --- | ---: | ---: |
| 検査順序 | `canonical_phases` | 0.98 | 0.97 |
| HTTP 符号化名 | `canonical_only` | 0.68 | 0.36 |

<details>
<summary>追加判定の入力 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: Sigil は AI が実装し人間が仕様をレビューする。小さい契約、最小の信頼基盤、明示的拒否、評価器とネイティブで一致する分類、実物 Git と標準 HTTP との相互運用を優先する。所有者指示: PR 636 の ADR 0052 のレビュー対応のみ。既存 ADR 0047/0048 の予算、upload-pack POST に限る単一 gzip member と認証・認可の手順を維持する。元の六判断は独立した既存記録であり、具体値・固定名は起草案。今回の追加判断は拒否順序と HTTP 符号化名についてのみ。local pack index crc32 は後続に引継ぐ。Gemini Decisions は使わない。",
  "questions": {
    "precedence": {
      "type": "choice",
      "criteria": {
        "canonical_phases": "認証認可の後、HTTP の符号化選択を本文読取り前に検査。境界が既知の入力は構文解釈前に bounded intake し、gzip の大小は読取り分割に依存させない。pack 内の個別ストリームは消費位置で入力上限を検査する。宣言長がある場合はその取得直後に範囲検査し、その後フレームヘッダ、復号の論理単位ごとに構造検査から出力予算・適用される宣言長の検査・出力確保と書込みの順、終端後は実長・trailer・余剰を固定順で検証する。最初の失敗で停止し、先行する予算超過後の破損は調べない。評価器とネイティブに同じ順序と重複ベクトルを要求する。",
        "decode_while_intake": "対応する HTTP 本文は到着直後から展開し、入力長とフレーム破損は観測した順に返す。",
        "scan_all_first": "予算を超えても構文・検査値のエラーを検索し、データ破損を優先する。"
      }
    },
    "content_encoding_alias": {
      "type": "choice",
      "criteria": {
        "rfc_alias": "HTTP 符号化名を ASCII 大文字小文字無視で比較し、upload-pack POST の x-gzip を RFC 9110 8.4.1.3 の SHOULD に従い gzip の別名として扱って同じ検証と上限を適用する。他の未対応名、多重符号化、他の入口の gzip/x-gzip は本文を読む前に 415。",
        "canonical_only": "HTTP 符号化名を ASCII 大文字小文字無視で比較し、許可する名前は identity と upload-pack POST の gzip に限定する。x-gzip は未対応として本文を読む前に 415 とする。"
      }
    }
  }
}
```

</details>

<details>
<summary>追加判定の応答 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "precedence": {
      "type": "choice",
      "choice": "canonical_phases",
      "confidence": 0.97,
      "probabilities": {
        "canonical_phases": 0.98,
        "decode_while_intake": 0.02,
        "scan_all_first": 0.0
      }
    },
    "content_encoding_alias": {
      "type": "choice",
      "choice": "canonical_only",
      "confidence": 0.36,
      "probabilities": {
        "rfc_alias": 0.32,
        "canonical_only": 0.68
      }
    }
  },
  "usage": {
    "input_tokens": 1112,
    "output_tokens": 84
  }
}
```

</details>

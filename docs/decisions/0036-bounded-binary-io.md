# 0036: 上限付きバイナリ I/O（段階 6-2a）

- **状態**: 提案（予約: Issue #453）。2026-11-14 JST、Codex Cloud（OpenAI 系統）が typesafe-ai `jev-1.13.0` で正式判定。判断 A〜H は以下の案を採用する設計案。**所有者レビューは未実施であり、マージ・6-2b の着手を承認するものではない。Codex はマージしない。**
- **範囲**: 任意バイトのファイル I/O、確保前の上限、所有権、失敗、効果・信頼基盤差分、起動引数の調査。実装・CI・信頼基盤の設定は変更しない。

## 背景と現状

[ADR 0035](0035-p1-container-image-tool.md) はコンテナイメージ生成器を通常の Sigil プログラムにする。その前提の別 ADR が本書であり、0035 の上書きではない。ELF/tar は任意バイトなので、文字列 API `fs.read` / `fs.write` では代用できない。

調査対象は `src/stdlib/os_fs.sigil`、`core_bytes.sigil`、`src/stdlib.rs`、`src/ir_eval/call.rs`、`src/exec.rs`、`src/shared/args.rs`、`src/project/embedded.rs`、`src/project/trust_base.rs`。既存の OS 経路は openat/read/write/close、EINTR の再試行と部分 write の続行を持つ。Bytes は非 Copyable の所有値で、核の `from_raw` はコピーを作り、`data` / `len` が表現を隠す。

既存文字列 read は評価器では通常ファイル限定・64 MiB の上限があるが、ネイティブでは全量を無制限に拡張し、write は close の失敗を無視する。これは既存経路の差分として報告するだけで、本 Issue で修正しない。バイナリ経路は以下の共通契約を新たに満たす。

## 判断点・選択肢・正式判定

評価軸は、宣言効果と推論効果の完全一致、信頼基盤の小ささと列挙可能性、守れない層の除去、所有者の既存方針、独立に検証できる小さい段階の順。判定中に変更していない。各問の推奨は A。完全な選択肢と入力・応答は付録に保存する。

| 判断点（API のキー） | 推奨・採用案 A | 代案 B / C と比較理由 | 確率 A / B / C | confidence | 上位差 |
|---|---|---|---|---|---|
| A: API (`api`) | Path と明示 max を受ける全量 read/write | ストリームは公開ハンドルと破棄状態が増える / 文字列化は生バイト出力を満たさない | 0.98 / 0.01 / 0.01 | 0.97 | 0.97 |
| B: 上限 (`limits`) | API 128 MiB、生成器入力 64 MiB・出力 128 MiB | 64 MiB は資源半減だが入力 32 MiB / 256 MiB は用途が広いがピーク倍増 | 0.58 / 0.41 / 0.01 | 0.37 | 0.17 |
| C: エラー (`errors`) | 専用の通常列挙型、OS エラーは既存 IoError を包む | 全て Other は識別不能 / String は文字列分岐になる | 0.95 / 0.05 / 0.00 | 0.92 | 0.90 |
| D: 所有権 (`ownership`) | read は所有 Bytes、write は借用、raw→Bytes はコピー | ゼロコピーは配置と所有権の前提が増える / write 消費は再試行に複製が必要 | 0.88 / 0.10 / 0.02 | 0.82 | 0.78 |
| E: 失敗出力 (`failure_output`) | truncate と全量再実行、途中ファイルを明示 | 一時ファイル＋rename は保護に優れるが窓口増 / 新規専用は再実行に削除が必要 | 0.96 / 0.04 / 0.00 | 0.94 | 0.92 |
| F: 信頼基盤 (`trust_effects`) | 既存 raw.syscall、既存効果・許可、新 unchecked を列挙 | 新 raw 操作は別 ADR が必要 / Rust 生成器は 0035 と不整合 | 0.97 / 0.03 / 0.00 | 0.96 | 0.94 |
| G: 評価器 (`evaluator`) | 専用 builtin と独立したバイト用差替え表 | 文字列表変更は既存 API に波及 / ネイティブ専用は突き合わせ不能 | 0.92 / 0.02 / 0.06 | 0.88 | 0.86 |
| H: 起動引数 (`arguments`) | 既存の型付き String 二引数を使う | 専用オプションパーサは 6-2d 先取り / env は別 Issue への依存増 | 0.99 / 0.01 / 0.00 | 0.98 | 0.98 |

上位 2 案の差を応答から計算し、**0.10 以下の近接は 0 件、近接による未決定はない**。B は差 0.17 で閾値外だが confidence 0.37 と低く、128 MiB が用途全体に最適という保証ではない。所有者が確認・覆せるよう、64 MiB 案（確率 0.41）も明示して残す。所有者レビュー待ちは近接判定とは独立したマージ条件である。

## 決定 1: API・上限・エラー

公開 OS API は次の 2 つ。既存の文字列 API は変更しない。

```text
fs.read_bytes(p: &Path, max: Int) -> Result<Bytes, BinaryIoError>     effects fs.read
fs.write_bytes(p: &Path, data: &Bytes, max: Int) -> Result<Unit, BinaryIoError>  effects fs.write

BinaryIoError = InvalidLimit(Int) | InvalidPath | TooLarge(Int)
              | UnsupportedFile | Io(IoError)
```

これは既存の列挙型機能で表す OS の型で、新しい言語機能ではない。`InvalidLimit` は渡された max、`TooLarge` は要求上限 max を保持する。`Io` は既存の NotFound / PermissionDenied / Other の分類とパスを維持する。errno の正負や腕番号を利用者の契約にしない。正常な空ファイルは空 Bytes、長さ 0 の write は空ファイルの作成・切り詰めに成功する。

上限は **1 呼出し当たり 134217728 バイト（128 MiB）**。`0 <= max <= 134217728` を最初に検査し、不正なら `InvalidLimit(max)`。続いてパスの NUL と UTF-8 バイト長（4095 以下）を調べ、不正なら `InvalidPath`。write は `data.len() > max` なら `TooLarge(max)`。この順序で判定し、ここまでの失敗では open・データバッファ確保・出力変更を行わない。max=0 は空だけを許す。

生成器は入力 67108864 バイト（64 MiB）、出力 134217728 バイト（128 MiB）を使う。tar 等の追加長と算術の桁あふれを出力前に検査するのは 6-2c/d の責務で、出力上限超過時に分割出力や無制限へ逃げない。大きい ELF は明示拒否する。値の引上げは設計判断として記録する。

### read の確保前の制約

1. パスは O_RDONLY | O_CLOEXEC | O_NONBLOCK で openat。開いた fd を fstat し、通常ファイル以外を `UnsupportedFile`。FIFO の open 待ちを避け、パスの事前 stat だけには頼らない。fstat 長が max 超ならデータバッファ確保前に `TooLarge(max)`。
2. データ領域を **max バイト以下**で確保し、read の各要求長を残量以下にする。単純な基準実装は max を一度だけ確保する。max=0 ではこの領域を確保しない。全量 read_to_end の後に長さを調べる実装は禁止。
3. EOF まで読む。max に達したときは独立した **1 バイト**の領域へ 1 バイトだけ読む。EOF なら上限丁度で成功、1 バイト読めたら `TooLarge(max)`。ファイルが fstat 後に伸びても max を超えて本体領域を確保・読み込まない。縮んだら実際に読めた長さを返す。スナップショットは保証しない。
4. EOF と close の成功後だけ、既存 `std.core.bytes.from_raw` で実長分をコピーし、元領域・probe・fstat 領域を解放する。失敗時は読めた prefix を返さず、fd と確保領域を回収する。

I/O 内部のデータ領域のピークは `2*max + 1 + 24` バイト以下（読みバッファ＋最終 Bytes＋probe＋Bytes 頭）、他に NUL 終端パス最大 4096、fstat 144、固定サイズの状態がある。確保器の管理情報、既存 Path の文字列化、呼出し元の既存 Bytes/生成途中の出力は別。この数値はプロセス全体のメモリ制限ではない。コピー方式の資源コストを隠さず、複数同時呼出しの総量や生成器の生存値削減は別に考慮する。OOM は既存確保器と同じ異常停止（70）であり、回復可能な `BinaryIoError` を保証しない。

## 決定 2: 所有権と出力の失敗

read の成功 Bytes は呼出し元が一度だけ所有し、通常の破棄規則で解放する。OS の一時領域をそのまま所有 Bytes に見立てない。write は `&Bytes` を呼出し中だけ借用し、変更・解放・保持しない。シムは `std.core.bytes.data` / `len` を通して読む。Bytes の 24 バイトの頭の配置を OS モジュールへ複製せず、利用者向け Addr 変換も公開しない。

write は既存経路同様に openat（O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC、加えて O_NONBLOCK、mode=0666 と umask）を使い、fd の fstat で通常ファイルだけを扱う。既存ファイルの truncate は open 時点で起こり得る。シンボリックリンクは既存パス I/O と同様に辿り、パスを権限分離や隔離の機構とは扱わない。信頼できる出力先を選ぶのは呼出し元の責務。

部分 write は残りから続け、EINTR は同じ位置を再試行する。0 バイト write は Other。read/write/openat/fstat の EINTR は再試行する。close は Linux の規則に従い再試行せず、close 失敗も `Io` とする。先行した I/O 失敗があればそのエラーを優先し、close は一度だけ試みる。全量 write と close の成功でのみ `Ok(Unit)`。fsync は行わず、クラッシュ後の永続性は保証しない。

open 後の `Err` では既存出力が消え、空・prefix・全量のファイルが残る可能性がある。元の内容を復元しない。パス競合下で別のファイルを消す危険を増やさないため、失敗時の自動 unlink も行わない。再実行は同じ保持済み Bytes を先頭から truncate して全量書く。残りの追記や部分ファイルからの再開はしない。同じ入力でも並行 writer があれば一致は保証しない。

生成器は成功した `Ok` の後だけ完了・digest を表示し、失敗ファイルを load しない。read/検証/全出力の上限検査が済むまで write を呼ばない。原子的な置換が必要になったら、一時ファイル・排他的生成・rename・掃除を別 ADR で設計する。

## 決定 3: 効果と信頼基盤の差分（6-2b の実装計画）

| 対象 | 設計上の増分・維持する境界 |
|---|---|
| 公開標準 API・型 | `NAMESPACED_FNS` に 2 行、OS 型 BinaryIoError と腕の登録。名前解決・lowering・SIR builtin・検証・仕様の出力に必要な結線。公開効果は fs.read / fs.write のみ |
| `std.os.fs` | 上限付き read/write と必要なバッファ解放・fd 種別確認の補助。既存 open/read/write の制御を再利用するが、文字列向け read/write の振る舞いを変更しない。内部は既存 `raw.syscall` のため `effects fs`、公開の細分化効果へ結ぶ既存シムの境界を守る |
| 新しい `unchecked` と前提の文 | 入力パスの可読範囲、長さと NUL の検査順、出力 kind/実長の 8 バイト境界、0<=n<=cap<=max、read の書込み先と write の借用元の生存期間、ゼロ長で番地を読まないこと、fd の単独所有・一回の close、全経路の解放、コピー後の一回の所有移譲を関数ごとに明記する |
| fstat の OS ABI | Linux x86-64 の SYS_fstat=5、144 バイト・8 バイト境界の stat 領域、st_mode の +24 と st_size の +48、通常ファイル判定のビットを前提に記す。openat=257/read=0/write=1/close=3 と合わせて列挙。他の ABI へそのまま流用しない |
| `std.core.bytes` | 既存 from_raw/data/len/free を利用。既存の配置・所有権・RAW_OPS の前提の変更は不要。OS の raw 一時領域は OS の解放補助が同じサイズで解放する |
| 生成コード | バイト I/O 用シムが Path、Bytes 借用、実長、エラー番号、Result を結ぶ。頭の配置を読む場所は増やさない。OS の Addr を利用者へ返さない。シムとエラー組立ても信頼基盤の増分としてレビューする |
| 評価器 | Rust の新 builtin 2 個、上限付き read と同じ種別・部分 I/O・close・エラー分類の契約。ネイティブの埋込み Sigil 本体を呼ぶわけではなく、独立した信頼基盤に残る |
| `RAW_OPS` | **行・効果・requires・前提の文の変更 0**。既存 raw.syscall/alloc/free/copy/offset/load/store を利用。fstat は既存 syscall の引数であり新原始操作ではないが、unchecked と OS ABI のレビュー対象 |
| 許可表・検査の例外 | `trust-base.list` の変更 **0**。既存の埋込み std.os.fs / std.core.bytes の許可と targets 例外を利用し、新規許可・利用者の trust base・新効果カテゴリを足さない |
| 形式処理 | ELF/USTAR/JSON/OCI/digest の組立ては後続の通常 Sigil、trust none。I/O を理由に形式処理へ raw/unchecked を広げない |

`raw` / `unchecked` / 許可表は設計として列挙しただけで、本 PR はそれらを変更しない。新しい unchecked を設計するため、ADR 0018 の S2 と ADR 0035 の段表に従い **この ADR のマージ前に所有者レビューが必須**。承認者・日付・対象 SHA の記録欄は未記入（未承認）であり、判定サービスの高確率や別系統エージェントのレビューで代替しない。6-2b でも実際の信頼基盤差分を出し、所有者レビューを受ける。実装中に新原始操作・依存・言語機能が必要と分かったら止めて別 ADR/Issue に分ける。

## 評価器・起動引数・後続の検証

評価器の `Options` に `fs_bytes: Option<BTreeMap<String, Vec<u8>>>` を追加する案とする。バイナリ API だけが読み書きし、Some のときは実 OS へフォールバックしない。入力は確保前に長さを検査してからコピーする。write は検査成功後に差替え表へ全量を反映する。実ファイルの部分失敗はこの単純な表だけでは表現できないため、短い read/write・EINTR・0 write・close 失敗を注入する駆動を別に用意する。既存の `fs_files` は文字列専用のまま、両差替え表の同一パスは別状態と明記し、実 OS では同じファイルを指すことと混同しない。

`src/shared/args.rs` は入口引数として String を扱い、ネイティブには既存 `_start`→argv の経路がある。6-2d は入力・出力を String 二引数として受け、Path に変換すれば足りる。`sigil run` の既存引数解析では数値・Bool・一覧に見える綴りが先に解釈されるため、ファイル名には `./123` のようにパスを明示する。任意の Bytes や ELF 本文を argv に渡す機能は不要。env.get（別 Issue #454）、新 CLI サブコマンド・言語機能は前提にしない。入口名・呼出し・終了表示の結線は 6-2d で確定する。

6-2b の確認は、空・NUL・不正 UTF-8・全 256 値、max=0/1/上限、負/上限超 max、max-1/max/max+1、fstat 後の増減、FIFO/ディレクトリ/デバイス、NUL/長過ぎパス、桁あふれを含む。確保・syscall の駆動で上限拒否前に出力へ触れないこと、全量の過剰確保なし、全失敗経路の解放・二重解放なし、write の借用が保たれることを確かめる。評価器とネイティブで Bytes とエラーを突き合わせ、部分 I/O と close 失敗、同じ Bytes での再実行、成功表示を誤って出さないこと、宣言効果の過不足の拒否を確認する。性能・上限の妥当性はこの正式判定だけでは実測していない。

## 既存決定との整合・範囲外

| 既存決定 | 整合 |
|---|---|
| ADR 0035 決定 1 | 通常 Sigil の生成器・既存 build/入口引数を維持。OCI/ELF/tar の詳細や結線を本書で実装しない |
| ADR 0035 決定 2 | 確保前に効く上限、具体値・エラー・途中出力と再実行を定義。失敗を load/成功表示しない |
| ADR 0035 決定 3 | 既存 OS 経路再利用、効果・raw/unchecked/許可・評価器の増分を可視化し、所有者レビューを残す |
| ADR 0018 判断 4・8（2026-10-26 所有者決定） | Docker/PostgreSQL CI の方針は再判定せず、CI 設定は変更しない。P1 のレジストリ認証・送信を追加しない |

文字列 I/O の既存差分、原子的置換、非 Linux x86-64 のネイティブ ABI、期限付きファイル I/O、ファイルシステム隔離は修正しない。6-2c の形式処理、6-2d の結線、6-2e の Docker 互換性・CI、6-2f の実測記録を先取りしない。

## 正式判定の記録

2026-11-14 JST、実行環境 Codex Cloud。`POST https://api.typesafe.ai/v1/systemone`、model `jev-1.13.0`、JSON 辞書の questions を使用。認証は環境変数 `TYPESAFE_API_KEY` を Bearer としてプロセス内で使用し、値や認証ヘッダーは記録しない。以下は送信した入力と受信した応答（JSON の整形のみ）。再判定は行っていない。

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: 宣言効果と推論効果の完全一致、信頼基盤を小さく列挙可能に保つこと、守れない層を除くこと、所有者の既存方針、独立に検証できる小さい段階の順。Sigil は AI がコードを書き人間が仕様をレビューする言語。外部依存を追加せず検査済み Sigil を増やす。所有者指定: Issue453 の設計だけ、docs の ADR と番号予約のみの PR、マージしない。raw/unchecked/許可表の設計変更は所有者レビューが必要。上位2案差<=0.10は未決定。ADR0035決定1-3: 通常Sigil生成器、既存OS read/write経路再利用、読込み全量確保後の検査は不可、具体的上限と失敗規則を先に決める。ADR0018判断4/8: Docker/PostgreSQL CIは既決、レジストリ送信しない。新言語機能・新原始操作・依存は別ADR。既存Bytesは非Copyableの所有値、from_rawはコピー、頭の配置を知るのはcore_bytesだけ。既存raw.syscallでopenat/read/write/close/fstatが使える。既存文字列fsはそのまま。評価器文字列readは64MiB制限、ネイティブ文字列readは無制限（範囲外問題）。\n共通の詳細案: 新APIはOS層、Pathを借用、NULまたはUTF8のバイト長4095超のパスはInvalidPath、OS操作前に検査。通常ファイルのみ、O_NONBLOCKでFIFO openを防ぎ、開いたfdのfstatで種別判定。Linux x86-64 fstat構造のmode(+24)とsize(+48)、144B/8境界を前提の文に明示。UTF8検査をデータにしない。読込みはmaxまでの一つのバッファと上限超過確認用別1byte、上限丁度時に1byteだけprobeしてEOF成功/余分ありTooLarge。成長・メタデータ競合にも上限を守る。途中データはエラーで返さない。EINTR再試行、部分write残り続行、write0失敗、closeエラーも失敗、Linux closeは再試行しない。リソース確保失敗は既存確保器と同じ異常停止70でResultの回復可能エラーと区別。同期I/Oで期限・スナップショット・永続性・symlink隔離を保証しない。パス文字列化の既存確保以外のI/O内部領域を制限する。新システムコール番号の利用は新raw原始操作ではないがunchecked差分として所有者レビューする。",
  "questions": {
    "api": {
      "type": "choice",
      "criteria": {
        "A": "fs.read_bytes(p: &Path, max: Int) -> Result<Bytes, BinaryIoError> と fs.write_bytes(p: &Path, data: &Bytes, max: Int) -> Result<Unit, BinaryIoError>。明示上限、任意バイト、全量の小さいAPI。ストリームなし。",
        "B": "File所有ハンドルとread_chunk/write_chunk/closeのストリームAPI。ピークメモリ低減だが公開型と生存期間・破棄・状態を増やす。",
        "C": "read_bytesだけ追加し出力は文字列へエンコードして既存fs.write。新APIは小さいがOCIの生バイト出力には復号段が必要。"
      }
    },
    "limits": {
      "type": "choice",
      "criteria": {
        "A": "APIハード上限128MiB(134217728)、maxは0..上限。生成器入力は64MiB(67108864)、出力128MiB。max検査はopen/確保前、writeはlen<=maxをopen前に検査。read領域max+probe1と最終Bytesコピー最大maxで内部ピーク<=2*max+1+24、他にパス最大4096とfstat144と固定状態。巨大ELFは明示拒否、上限増は別判断。",
        "B": "ハード上限64MiB、生成器入力32MiB出力64MiB。同じ事前検査・バッファ規則。資源を半減するが利用可能ELFが小さい。",
        "C": "ハード上限256MiB、生成器入力128MiB出力256MiB。同じ事前検査・バッファ規則。利用範囲が広いがピークが倍増。"
      }
    },
    "errors": {
      "type": "choice",
      "criteria": {
        "A": "新しい通常列挙型BinaryIoError = InvalidLimit(Int) / InvalidPath / TooLarge(Int) / UnsupportedFile / Io(IoError)。TooLargeは要求maxを保持、既存IoErrorのNotFound/PermissionDenied/Otherへerrno分類。既存IoErrorは変更しない。既存列挙型機能で実現。",
        "B": "既存IoErrorだけを使い上限/不正引数/非通常ファイルをOtherへ分類。型追加不要だが回復理由の識別が失われる。",
        "C": "ResultのエラーをStringにし具体的errnoも文字列化。診断容易だが分岐が文字列依存になる。"
      }
    },
    "ownership": {
      "type": "choice",
      "criteria": {
        "A": "readが新しい所有Bytes、writeは共有借用で保持/消費しない。OSのraw領域を既存core_bytes.from_rawでコピーし原領域解放、公開Addr変換なし。shimはcore_bytes.data/lenを介し配置を複製しない。戻りのBytes頭は一回所有移譲。",
        "B": "OSがBytes頭の配置を知りraw領域をゼロコピー所有移譲。ピーク小さいが配置を知る場所と所有権前提が増える。",
        "C": "writeがBytesを消費して所有権移譲する。借用の扱いは単純だが再実行で複製が必要。"
      }
    },
    "failure_output": {
      "type": "choice",
      "criteria": {
        "A": "既存openat O_TRUNC/read/write/close経路を再利用。writeは全量とclose成功でのみOk。open後のErrには空/途中/全量の出力が残り得る、元内容は復元せず削除しない。再実行は最初からtruncateし同じBytesを全量書く。検査失敗は出力に触れない。成功表示/loadはOk後のみ。",
        "B": "同一ディレクトリの排他的一時ファイルに書きrenameで置換。失敗時の既存出力保護に優れるが一時名・排他・rename/unlink・掃除と新しい契約が増える。",
        "C": "O_EXCL新規作成だけとし既存出力は拒否。上書き事故を防ぐが途中ファイル削除APIがないため再実行の手順が増える。"
      }
    },
    "trust_effects": {
      "type": "choice",
      "criteria": {
        "A": "公開効果はread=fs.read/write=fs.write、内部std.os.fsは既存raw.syscallのfs効果。新uncheckedのバッファ/長さ/errno出力/所有権/fstatABIを列挙し所有者レビュー。RAW_OPS行・前提は変更0、trust-base.list変更0、既存std.os.fs/core.bytes許可を使う。コード生成shim、stdlib登録/lowering、評価器の新builtinsは信頼基盤増分として可視化し、形式処理はtrust none。",
        "B": "新しいraw.fs_read_bytes/raw.fs_write_bytesを登録。OS窓口を抽象化できるが新原始操作で別ADRが必要、今回の範囲を超える。",
        "C": "生成器だけRust組込みにしBytes I/Oと形式処理をRustへ寄せる。結線は少ないがADR0035の検査済み生成器方針に反する。"
      }
    },
    "evaluator": {
      "type": "choice",
      "criteria": {
        "A": "新バイトI/O builtinをRustで追加し同じ境界/エラー/部分I/O/close契約を実装。Optionsにfs_bytes: Option<BTreeMap<String,Vec<u8>>>を追加しバイナリ呼出しだけが参照、Some時は実OSにフォールバックしない。既存fs_files文字列マップは変更しない。境界値と失敗注入で評価器/ネイティブを突き合わせる。",
        "B": "fs_filesをバイトマップへ変更し文字列I/Oも同じ保存先に統合。状態共有できるが既存Options APIと文字列経路を変更する。",
        "C": "評価器のバイトI/Oは未対応としネイティブだけ提供。実装少ないが同じSigilプログラムの突き合わせを妨げる。"
      }
    },
    "arguments": {
      "type": "choice",
      "criteria": {
        "A": "既存の入口の型付きString二引数(input/output)とPath変換だけで6-2dを構成可能。バイト内容をargvに渡さずAPIで読む。上限は生成器で固定、環境変数/新argvパーサ/CLIサブコマンドは今回不要。詳細結線は6-2dへ残す。",
        "B": "入力と出力と上限をすべてCLIオプションとして渡す専用パーサを6-2aで設計する。柔軟だが入口結線の後続範囲を先取りする。",
        "C": "引数を使わず環境変数で入力/出力を取得する。自動化しやすいがenv公開が別Issue454で進行中で追加依存と効果を持つ。"
      }
    }
  }
}
```

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "api": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.97,
      "probabilities": {
        "A": 0.98,
        "B": 0.01,
        "C": 0.01
      }
    },
    "limits": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.37,
      "probabilities": {
        "A": 0.58,
        "B": 0.41,
        "C": 0.01
      }
    },
    "errors": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.92,
      "probabilities": {
        "A": 0.95,
        "B": 0.05,
        "C": 0.0
      }
    },
    "ownership": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.82,
      "probabilities": {
        "A": 0.88,
        "B": 0.1,
        "C": 0.02
      }
    },
    "failure_output": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.94,
      "probabilities": {
        "A": 0.96,
        "B": 0.04,
        "C": 0.0
      }
    },
    "trust_effects": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.96,
      "probabilities": {
        "A": 0.97,
        "B": 0.03,
        "C": 0.0
      }
    },
    "evaluator": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.88,
      "probabilities": {
        "A": 0.92,
        "B": 0.02,
        "C": 0.06
      }
    },
    "arguments": {
      "type": "choice",
      "choice": "A",
      "confidence": 0.98,
      "probabilities": {
        "A": 0.99,
        "B": 0.01,
        "C": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 3240,
    "output_tokens": 287
  }
}
```

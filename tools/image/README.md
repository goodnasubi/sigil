# OCI イメージ生成器（ADR 0035 / 6-2c・d）

リポジトリの根で、既にビルドした静的 Linux x86-64 ELF と出力先を指定する。

```sh
sigil check tools/image/main.sigil
sigil build tools/image/main.sigil main -o ./sigil-image
./sigil-image ./app.elf ./image.tar
# 評価器でも同じ入口を利用できる
sigil run tools/image/main.sigil main ./app.elf ./image.tar
```

入口は String 二引数で、数値・Bool・一覧に見えるファイル名は `./123` のようにパスを明示する。`--root` の既定は現在のディレクトリ。入力元とビルドの信頼は利用者の責任で、ELF 検査は命令の安全性や Sigil 由来を証明しない。

入力は64MiB、archive は128MiBまで。ELF の読込み・検証・全 archive の組立てと上限検査が成功するまで出力先に触れない。単一 `app`（0555）、非 root `65532:65532`、`/app` の起動、追加ファイルなしの固定構成で、入力パス・名前・mtime・ホスト時刻を埋め込まない。外側は index、layout、layer blob、config blob、manifest blob の固定順。

成功は `Ok(0)`。完了表示と `layer` / `config` / `manifest` / `archive` の対象名付き SHA-256 は、全量 write と close の成功後だけ表示する。失敗は `Err(1)`（入力 I/O）、`Err(400)`（ELF・形式・上限）、`Err(2)`（出力 I/O）で、完了・digest は表示しない。既存の CLI は Result 値を表示して正常終了するため、シェルの終了値だけで成功判定せず Result を確認する。

出力の失敗は [Issue #470 の仕様判断](https://github.com/goodnasubi/sigil/issues/470#issuecomment-6874956992) と ADR 0036 決定2の契約に従う。空・prefix・全量の失敗ファイルが残る場合があり、自動 unlink は行わない。残存物を load せず、再実行は同じ入力から全量を書き直す。原子的置換は別 ADR の対象。生成器は load、ビルド、外部コマンドを実行しない。

## 純粋な形式部品

以下の形式部品は通常の `trust none` / `effects none` の Sigil モジュール。Rust CLI、原始操作、依存、信頼基盤の許可は追加しない。`sha256` と `json.parse` / `json.stringify` は既存の検査済み標準モジュールを利用する。

| モジュール | 公開部品 |
|---|---|
| `tools.image.binary` | 加算前の範囲検査、符号付き Int に収まる little endian 読取り、ゼロ列、十進・小文字16進表現 |
| `tools.image.elf` | `validate_elf`：64MiB以下の静的 ELF64 / little endian / x86-64 / ET_EXEC、ヘッダと全 program header の範囲、PT_INTERP / PT_DYNAMIC の拒否 |
| `tools.image.ustar` | `archive_size`、`append_file`、`finish_tar`、`read_tar`、`TarFile` の読取り関数 |
| `tools.image.oci` | 実バイト列の `digest` / `descriptor`、descriptor の JSON 組立て・読取り・参照検証、layout / config / manifest / index の固定 JSON、`read_json` |

不正形式・負の長さ・桁あふれ・上限超過は `Err(400)`。検証成功は `Ok(true)`。JSON の読取りは既存 parser の重複キー・構文・深さ・数値の検査を通る。descriptor は未知欄、非整数の size、許可していない mediaType、非正規 SHA-256 を拒否し、`verify_descriptor` が実長と実 digest を照合する。

USTAR は通常ファイルのみで、相対 ASCII 名（100 byte 以下、空成分と `.` / `..` を拒否）、mode `0555` または `0644`、uid/gid/mtime=0、空 uname/gname。数値はゼロ詰め八進＋NUL、checksum は6桁八進＋NUL＋空白。ヘッダ・本体は512 byte境界でゼロ詰めし、`finish_tar` が終端を正確に2ブロック加える。総量は終端込み128MiB以下。reader はこの固定エンコードだけを受理し、checksum・予約欄・padding・終端も検査する。任意の外部 tar の展開器ではない。

`append_file` は終端追加前のバッファへ順に呼び、最後に一度 `finish_tar` を呼ぶ。固定したファイル順・重複のない名前の選択は呼出し側が行う。ELF 確認は命令の安全性や Sigil 由来の証明ではない。

`main.sigil` は `trust none`、純粋な組立ては `effects none`、入口は `fs.read` / `fs.write` / 診断 `console` のみ。`tests/image_generator_470.rs` で製品の入口を評価器とネイティブで駆動し、再現性・digest の変化・出力失敗・効果の過不足の拒否を確認する。Python標準 `tarfile` / `json` / `hashlib` で参照と非 root 設定も独立に確認する。部品の検証は `tests/image_formats_467.rs`。Docker互換性・CI設定は6-2e、実測の記録は6-2fへ残す。

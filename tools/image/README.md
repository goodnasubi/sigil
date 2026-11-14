# イメージの純粋な形式部品（ADR 0035 / 6-2c）

すべて通常の `trust none` / `effects none` の Sigil モジュール。Rust CLI、原始操作、依存、信頼基盤の許可は追加しない。`sha256` と `json.parse` / `json.stringify` は既存の検査済み標準モジュールを利用する。

| モジュール | 公開部品 |
|---|---|
| `tools.image.binary` | 加算前の範囲検査、符号付き Int に収まる little endian 読取り、ゼロ列、十進・小文字16進表現 |
| `tools.image.elf` | `validate_elf`：64MiB以下の静的 ELF64 / little endian / x86-64 / ET_EXEC、ヘッダと全 program header の範囲、PT_INTERP / PT_DYNAMIC の拒否 |
| `tools.image.ustar` | `archive_size`、`append_file`、`finish_tar`、`read_tar`、`TarFile` の読取り関数 |
| `tools.image.oci` | 実バイト列の `digest` / `descriptor`、descriptor の JSON 組立て・読取り・参照検証、layout / config / manifest / index の固定 JSON、`read_json` |

不正形式・負の長さ・桁あふれ・上限超過は `Err(400)`。検証成功は `Ok(true)`。JSON の読取りは既存 parser の重複キー・構文・深さ・数値の検査を通る。descriptor は未知欄、非整数の size、許可していない mediaType、非正規 SHA-256 を拒否し、`verify_descriptor` が実長と実 digest を照合する。

USTAR は通常ファイルのみで、相対 ASCII 名（100 byte 以下、空成分と `.` / `..` を拒否）、mode `0555` または `0644`、uid/gid/mtime=0、空 uname/gname。数値はゼロ詰め八進＋NUL、checksum は6桁八進＋NUL＋空白。ヘッダ・本体は512 byte境界でゼロ詰めし、`finish_tar` が終端を正確に2ブロック加える。総量は終端込み128MiB以下。reader はこの固定エンコードだけを受理し、checksum・予約欄・padding・終端も検査する。任意の外部 tar の展開器ではない。

`append_file` は終端追加前のバッファへ順に呼び、最後に一度 `finish_tar` を呼ぶ。固定したファイル順・重複のない名前の選択は呼出し側が行う。ELF 確認は命令の安全性や Sigil 由来の証明ではない。

生成器の入口、引数、ファイル I/O、全体の結線・再現性は6-2dへ残す。Docker互換性・CI設定は6-2e、実測の記録は6-2fへ残す。`tests/image_formats_467.rs` のみで部品を結び、評価器とネイティブを照合し、Python標準 `tarfile` / `json` / `hashlib` でも各バイト列と参照を独立に確認する。

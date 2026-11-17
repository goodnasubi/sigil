# 0049: 共通 fs 境界

- **状態**: 提案（typesafe-ai 正式判定は未実施。起草者の案として明記。末尾に判定用の問いと評価軸を記録。別系統のレビュー前。実装完了を意味しない）
- **日付**: 2026-11-17（JST）
- **対象**: [Issue #584](https://github.com/goodnasubi/sigil/issues/584)、レーン E

## 背景と前提

[ADR 0044](0044-p4-scope.md) および [ADR 0045](0045-p4-1-database-core.md)（P4-1 データベースの核）の実装順序0として、データベース（P4-1/P4-2）およびリポジトリ管理（P4-3/P4-4）が共通して依存するファイルシステム境界（共通 fs 境界）の仕様と契約を定める。

本 ADR の前提:
- [Issue #576 の結論（2026-11-17 09:10 JST 決定コメント）](https://github.com/goodnasubi/sigil/issues/576#issuecomment-6933389330): 標準ライブラリのファイル関連不足項目（ディレクトリ操作、プロセス間ロック、追記、位置指定 I/O、同期、原子的 rename）は、「公開 `std.fs` の拡張」かつ「既存 `std.os.fs` の信頼基盤拡張」として扱う。圧縮（zlib/deflate）および SHA-1 は純 Sigil／`trust none` の独立モジュールとし、本 fs 境界には含めない。
- [ADR 0045 決定 1](0045-p4-1-database-core.md): 順序0を「共通 fs 境界 ADR」とし、後続を fs A「資源・名前・排他」→ B「ページ I/O・WAL 追記」→ C「同期・原子的置換」の 3 つの実装 Issue に分割する。C まで揃う前に永続 DB を完成扱いとしない。
- 既存の `src/stdlib/os_fs.sigil` は全量読み書き（`read`/`write`）のみを提供し、同期を行わず、単一スレッド前提（補足 A-5）で書かれている。この既存前提を新規ファイル境界に流用せず、並行実行・排他ロック・持続性を満たす境界を設計する。

### 版取得と実装の置き場所

ADR 0044 決定 4 および ADR 0045 に従い、仕様の基準とするコミットは `goodnasubi/sigil` の完全 SHA `4867dc61190c4e2e6953adb25240ea178603c0ed`（PR #583 マージ直後の main）とする。取得方法は本リポジトリを完全 SHA で checkout し、同梱 `Cargo.lock` を用いて `cargo build --locked --release` でビルドする。浮動 main やタグだけに依存しない。

共通 fs 境界の仕様は本リポジトリ（`goodnasubi/sigil`）に置き、公開 `std.fs` および `std.os.fs` の実装も本リポジトリの標準ライブラリ（レーン D2）に追加する。DB 実装（所有者作成の別リポジトリ）は本 fs 境界および fs A〜C が揃うまで実装に着手しない。

本 PR は本 ADR（`docs/decisions/0049-common-fs-boundary.md`）の新規作成と索引（`docs/decisions/README.md`）の更新のみを行う。処理系、`sigil spec` の出力、実装コード、バックログ定義は変更しない。

---

## 選択肢と決定

| 判断点 | 採用案 | 他案を採らない理由 |
|---|---|---|
| ハンドルと資源管理 | 所有権を持つ `File` / `Dir` 型による明示的 close + 自動 drop クリーンアップ。生の fd 整数を公開 API に露出しない | 生の fd をそのまま露出すると double close、fd 再利用競合（ABA）、リークの温床になる |
| プロセス間排他ロック | `flock(2)`（`SYS_flock`）によるオープンファイル記述（OFD）単位の排他・共有ロック | 従来の POSIX レコードロック（`fcntl` `F_SETLK`）は、同一プロセス内の任意 fd の close で全ロックが暗黙解除される致命的欠点があり、複数ハンドル並行時に破綻する。OFD ロック（`F_OFD_SETLK`）は構造体と syscall が複雑で Linux 固有。`flock` は枯れており単純・堅牢 |
| パス制約と symlink 拒否 | 専用ディレクトリ起点（`Dir`）の相対内部名操作、`..` 脱出拒否、`O_NOFOLLOW` による構成要素の symlink 追従拒否（ADR 0045:109 要件） | NUL 検査や長さ制限だけでは、既存 DB の再オープン時に `../` や中間 symlink による専用ディレクトリ脱出・意図しないファイル破壊を防げない |
| 追記と位置指定 I/O | `pread`/`pwrite`（`SYS_pread64`/`SYS_pwrite64`）による無シーク位置指定読み書き、および `O_APPEND` による末尾追記保証。**append ハンドルでの `write_at`/`write_all_at` は明示拒否** | シーク（`lseek`）は並行レースを起こす。Linux では `O_APPEND` の fd に対して `pwrite64` を呼ぶと offset を無視して末尾へ追記するため、同一ハンドルでの混用を許すとページ更新が追記化してデータ破壊を招く |
| 持続性と名前変更 | ファイル／ディレクトリの明示的 `fsync`（`SYS_fsync`）、同一 FS 内の原子的置換 `renameat` | `close` 成功は持続性の証明にならない。親ディレクトリの `fsync` がなければ新規作成・削除・rename の永続性は保証されない。異 FS 間の rename は暗黙コピーせず明示拒否（`EXDEV`）する |
| テスト差し替え（`--fs-double`） | パス写像を尊重し、実ファイルに対する安全な委譲または未対応操作の明示的拒否（fail-closed） | 差し替え下で同期やロックを暗黙に成功偽装すると、クラッシュ復旧や永続性の誤った信頼を生む |
| エラー分類と効果 | 構造化された `IoErrorKind`、既存 `effects fs` の適用。新効果・新 raw 名は追加しない | 信頼基盤を最小化し（ADR 0005）、既存の検査系と互換性を維持する |

---

## 決定 1: ハンドル・資源管理・並行性

### 1.1 ハンドル型と所有権

公開 `std.fs` に以下のハンドル型を提供する。

1. **`File` 型**:
   - OS のオープンファイル記述子（fd）を単一所有する。
   - `Copy` / `Clone` は不可（線形・移動セマンティクス）。
   - 明示的な終了: `pub fn close(self) -> Result<(), IoError>`。
   - 破棄（Drop）時: 明示的に `close` されずにスコープを抜けた場合、ランタイムが自動的に fd を閉じる（エラーは無視され、成功・失敗の判定は得られないため、持続性を気にするコードは明示 `close` を呼ぶ）。
2. **`Dir` 型**:
   - ディレクトリのオープン記述子を単一所有する。
   - 明示的な終了: `pub fn close(self) -> Result<(), IoError>`、および破棄時の自動クローズ。

生 fd の整数値（Int）は公開 `std.fs` の API に露出しない。利用者が不正な整数を渡して別リソースを操作したり、二重 close を引き起こす危険を型安全に排除する。

### 1.2 ファイルのオープン、排他的作成、パス制約と symlink 拒否

ファイルオープンは `OpenOptions` ビルダー経由で行う。

| オプション | 意味 | 基礎フラグ（Linux x86-64） |
|---|---|---|
| `read(true)` | 読み取り専用／読み書き | `O_RDONLY` / `O_RDWR` |
| `write(true)` | 書き込み許可 | `O_WRONLY` / `O_RDWR` |
| `create(true)` | 存在しない場合に作成 | `O_CREAT` |
| `create_new(true)` | 排他的作成（存在すれば失敗） | `O_CREAT | O_EXCL` |
| `truncate(true)` | 既存ファイルを切り詰め | `O_TRUNC` |
| `append(true)` | 常に末尾追記 | `O_APPEND` |
| `follow_symlinks(false)` | symlink 追従の拒否（既定: false） | `O_NOFOLLOW`（`0o400000`） |

- **必須規則**: すべての `open` は内部で必ず `O_CLOEXEC`（`0o2000000`）を付与する。子プロセスへの記述子リークを防止する。
- **排他的作成（`create_new`）**: 一時ファイルや WAL の作成時に必須とし、既に同名ファイルが存在する場合は `AlreadyExists`（`EEXIST`）で失敗する。TOCTOU（Time-of-Check to Time-of-Use）のレースを防止する。
- **作成時モード**: 既定は `0o666`（プロセスの umask でマスクされる）。Git 等で実行ビットが必要な場合のため、明示的に作成モード（`mode: Int`）を指定可能とする。

#### パス制約と symlink 拒否（ADR 0045:109 要件）

データベースディレクトリ配下のファイルを安全に読み書きするため、以下のパス制約と symlink 拒否規則を強制する。

1. **ルート固定の相対操作（`Dir` 起点）**:
   - `Dir` ハンドルを起点とする相対操作 API を提供する:
     - `Dir::open_file(&self, subpath: &Path, options: &OpenOptions) -> Result<File, IoError>`
     - `Dir::create_file(&self, subpath: &Path) -> Result<File, IoError>`
     - `Dir::remove_file(&self, subpath: &Path) -> Result<(), IoError>`
   - `subpath` は単一ファイル名またはディレクトリ内部の相対パスでなければならない。
   - 先頭が `/` の絶対パス、空パス、NUL バイトを含むパス、および `..`（親ディレクトリ参照）を含むパスは、いかなる場合も `InvalidPath` で即座に拒否し、許可された専用ディレクトリ外への脱出を防止する。
2. **シンボリックリンクの拒否**:
   - `OpenOptions::follow_symlinks` の既定値を `false`（安全側）とする。
   - `follow_symlinks(false)` の場合、内部で `O_NOFOLLOW` を指定し、対象ファイルがシンボリックリンクであれば `PermissionDenied` または `InvalidPath`（errno `ELOOP` = -40）で拒否する。
   - `create_new` だけでなく、既存 DB ファイルを再オープンする際の中間構成要素や対象ファイルの symlink によるすり替えも拒否する。内部で `openat` を使用し、専用ディレクトリ外の実体を指す symlink の追従を遮断する。

### 1.3 並行性と fd 再利用（ABA 対策）

- 既存 `std.os.fs` の「単一スレッド前提」を排し、マルチスレッド／並行タスク環境で安全に動作することを保証する。
- スレッド／タスク A が `File` を `close` している最中に、スレッド／タスク B が新規に `open` して同一 fd 番号が即座に再利用された場合でも、型所有権により、スレッド A の閉じたハンドルからスレッド B の新規 fd へ操作が漏洩することはない。
- `read`/`write` 等のストリーム操作は `&mut File` を要求し、単一ハンドルへの並行書き込み競合を言語の借用検査で防止する。位置指定操作（`read_at`/`write_at`）はファイルオフセットを更新しないため `&File` で呼び出し可能とする。

---

## 決定 2: プロセス間排他ロック

### 2.1 方式の選定: `flock(2)` の採用

複数プロセス間でのデータベースディレクトリ・ファイルの排他制御には、`flock(2)`（システムコール番号 73: `SYS_flock`）を採用する。

| 比較項目 | `flock(2)`（採用） | POSIX `fcntl`（`F_SETLK`） | OFD ロック（`F_OFD_SETLK`） |
|---|---|---|---|
| ロックの結合先 | オープンファイル記述（OFD） | プロセスと inode の組（PID + inode） | オープンファイル記述（OFD） |
| 他 fd close 時の挙動 | **影響なし**（ロックした fd が生きている限り保持） | **同一プロセスの任意 fd close で全ロックが暗黙解除** | **影響なし**（fd が生きている限り保持） |
| 並行・複数ハンドル適合性 | 適合 | **不適合（極めて危険）** | 適合 |
| 移植性・単純性 | 高い（Linux/BSD/macOS 全対応、引数 2 つ） | POSIX 標準だが挙動が危険 | Linux 3.15 以降のみ、`flock64` 構造体必須 |

Claude のレビュー指摘（#576 コメント 6933530603）に照らし、従来の POSIX `fcntl` レコードロックは「同一プロセスが別用途で同一ファイルを開いて閉じた瞬間に、別スレッドが保持していたロックが警告なく消滅する」という重大な欠点があるため、絶対に採用しない。

### 2.2 ロック API と寿命

`File` ハンドルに以下のメソッドを提供する。

```sigil
pub fn try_lock_exclusive(&self) -> Result<bool, IoError>
pub fn try_lock_shared(&self) -> Result<bool, IoError>
pub fn unlock(&self) -> Result<(), IoError>
```

- **非ブロッキングを既定**: `LOCK_EX | LOCK_NB` および `LOCK_SH | LOCK_NB` を基本とする。ロックが他プロセスに保持されている場合は即座に `Ok(false)` または `Err(IoError::new(Busy))` を返し、呼び出し側を無制限に停止（ハング）させない。
- **ロックの寿命**: ロックは当該 `File` ハンドルの生存期間、または明示的な `unlock()` 呼び出しまで保持される。`File` が `close` されるか Drop された時点で OS により自動的に解放される。

---

## 決定 3: ファイル I/O・追記・位置指定操作

### 3.1 位置指定読み書き（`pread`/`pwrite`）

B+tree ページのランダム読み書きのため、カーネルのファイル位置ポインタに依存しない位置指定 I/O を提供する。

- システムコール: `SYS_pread64`（17）、`SYS_pwrite64`（18）。
- API:
  - `pub fn read_at(&self, buf: &mut Bytes, offset: Int) -> Result<Int, IoError>`
  - `pub fn write_at(&self, buf: &Bytes, offset: Int) -> Result<Int, IoError>`
  - 完全充足ヘルパー: `read_exact_at`, `write_all_at`（途中で戻った場合や `EINTR` 時に全量完了まで再試行）。

**保証と限界**:
- **保証**: ファイルポインタ（seek offset）を変更しない。同一 `File` または同一ファイル記述に対して、シーク競合なしに安全に呼び出せる。
- **限界**: `offset < 0` や `offset + len > 2^63 - 1` は `LimitExceeded` または `InvalidPath` で拒否する。ディスク満杯（`ENOSPC`）やクォータ超過（`EDQUOT`）は部分書き込みの後にエラーとなる場合があり、ロールバックまたはトランザクション中止を要する。

#### append ハンドルでの write_at / write_all_at の明示的拒否

- **契約**: `append(true)`（`O_APPEND`）を指定して開かれた `File` ハンドルに対して `write_at` または `write_all_at` を呼び出した場合、**必ず即座にエラー（`IoErrorKind::InvalidInput`）を返して拒否する**。
- **背景（Linux の pwrite 挙動）**: Linux において `O_APPEND` フラグを持つオープンファイル記述に対して `pwrite64(2)` を発行すると、システムコールに渡された `offset` 引数は完全に無視され、常にカーネルによりファイル末尾への追記として書き込まれる。これを許容すると、データベースの特定ページ更新（例: オフセット 8192 への書込み）のつもりが末尾への意図しない追記として成功し、サイレントなデータ破損を引き起こす。
- **オープンファイル記述の分離**:
  - 位置指定書込み（ページ I/O）と末尾追記（WAL 追記）を同一プロセスで行う場合、同一のオープンファイル記述（同一の `File` ハンドル）を共有してはならない。
  - アプリケーションは、追記用ハンドル（`append(true)`、`write_at` 不可）と、位置指定書込み用ハンドル（`append(false)`, `write(true)`、`write_at` 専用）を**別個のオープンファイル記述として独立して開かなければならない**（または ADR 0045 のとおり WAL ファイルと DB ページファイルを物理的に別ファイルとして管理する）。
  - なお、`append(true)` ハンドルに対する位置指定「読み取り」（`read_at`）は、オフセットが正常に適用され追記も発生しないため許可される。

### 3.2 追記（`O_APPEND`）

WAL（Write-Ahead Log）の連続追記のため、オープン時に `append(true)` を指定したハンドルによる書き込みを提供する。

- **保証**: 各 `write(2)` システムコールの発行時に、カーネルがファイル末尾オフセットの取得とデータ追記を不可分（アトミック）に実行する。複数スレッド／複数プロセスが追記した場合でも、個々の write で書き込まれたバイト列が過去のオフセットを上書きすることはない。
- **限界**: `O_APPEND` が保証するのは**単一の write システムコール内での追記位置決定**のみである。複数回の write に跨る論理レコード全体の不可分性は保証されない（他プロセスの write が間に挟まる可能性がある）。したがって、WAL レコードは 1 回のバッファ化 write で書き切るか、決定 2 の排他ロック下で追記しなければならない。

### 3.3 中断（EINTR）と Short I/O

- シグナル割り込み（`EINTR` = -4）は、標準ライブラリ内部で自動再試行する。
- 部分読み込み・部分書き込み（Short I/O）は、残余バッファに対して再試行を行うループを提供する。エラー発生時はそれまでに処理された長さを破棄せず、適切に伝播する。

---

## 決定 4: ディレクトリ操作・同期・原子的名前変更

### 4.1 ディレクトリ操作

`Dir` 型および `std.fs` 名前空間にディレクトリ操作を提供する。

- 作成: `fs.create_dir(path: &Path) -> Result<(), IoError>`（`SYS_mkdirat`、既定モード `0o777`）。
- 再帰作成: `fs.create_dir_all(path: &Path) -> Result<(), IoError>`。
- 削除: `fs.remove_dir(path: &Path) -> Result<(), IoError>`（`SYS_unlinkat` + `AT_REMOVEDIR`）。空でない場合は `DirectoryNotEmpty`（`ENOTEMPTY`）。
- ファイル削除: `fs.remove_file(path: &Path) -> Result<(), IoError>`（`SYS_unlinkat`、フラグ 0）。
- ルート固定相対操作: `Dir::open_file`、`Dir::create_file`、`Dir::remove_file`（決定 1.2）。
- 上限付き列挙: `Dir::read_entries(&self, max_entries: Int) -> Result<Vec<DirEntry>, IoError>`（`SYS_getdents64` = 217）。無制限のメモリ確保を防止するため、1 呼び出しあたりの取得件数に上限（既定 1024 件）を課す。

### 4.2 同期（`fsync`）と持続性の契約

**「`close` 成功は持続性（Durability）ではない」** ことを本境界の絶対原則とする。OS のキャッシュ（ページキャッシュ）に書き込まれたデータは、電源断や OS クラッシュ時に失われる。

1. **ファイルデータとメタデータの同期**:
   - `File::sync_all(&self) -> Result<(), IoError>`: `SYS_fsync`（74）を発行。データおよび inode メタデータ（サイズ・更新日時等）を永続媒体へフラッシュする。
   - `File::sync_data(&self) -> Result<(), IoError>`: `SYS_fdatasync`（75）を発行。ファイル長変更を伴わない純粋なデータ更新時にメタデータ同期コストを削減する。
2. **ディレクトリの同期**:
   - 新規ファイルの作成、ファイルの削除、ファイルの rename は、**対象エントリが存在する親ディレクトリを `Dir::open` して `Dir::sync`（`SYS_fsync`）を成功させるまで持続性が保証されない**。
   - ディレクトリ自体のメタデータ（ディレクトリエントリ）が永続化される前にクラッシュした場合、作成したはずのファイルが消失したり、サイズ 0 の孤立 inode となる可能性がある。

### 4.3 原子的名前変更（Atomic Rename）

チェックポイントメタデータの公開や Git 参照の更新のため、同一ファイルシステム内での原子的置換を提供する。

- システムコール: `SYS_renameat`（264）または `SYS_renameat2`（316）。
- API: `fs.rename(from: &Path, to: &Path) -> Result<(), IoError>`。
- **保証**:
  - `to` が既に存在する場合、`from` の内容で**アトミックに置換**される。置換の瞬間に `to` が存在しない空白期間は発生しない。
  - 同一マウントポイント（同一ファイルシステム）内でのみ有効。
- **限界と規則**:
  - 異なるファイルシステムを跨ぐ rename は `NotSameFileSystem`（`EXDEV` = -18）として**即座に失敗させる**。暗黙に「コピーして元を削除」という非原子的代替を行わない。
  - 原子的置換を行った後、持続性を確定するためには、**置換先（および必要に応じて置換元）の親ディレクトリに対する `sync` が完了しなければならない**。

---

## 決定 5: テスト用差し替え（`--fs-double`）の振る舞い

ADR 0004 規則 7 の `--fs-double 仮のパス=実ファイル` に対する拡張仕様を定める。

1. **基本方針**:
   - `--fs-double` は、テスト実行時に外部環境を汚染せず隔離検証するための仕組みである。
   - 登録された「仮のパス」に対する `open`（読み取り・書き込み）、`read_at`/`write_at`、`sync`、`lock` は、対応する「実ファイル」に対して透過的に実行される。
2. **未対応操作・不整合の明示的拒否（Fail-Closed）**:
   - 差し替え対象のパスに対する操作のうち、シミュレーションが整合性を保証できない操作（例: マッピング外のパスとマッピング内のパスを跨ぐ `rename`、メモリ内疑似 fs での偽装 `fsync` 等）は、**暗黙に成功と偽らず、`IoOther`（`UnsupportedUnderFsDouble`）で明示的に失敗させる**。
   - ロック操作（`try_lock_exclusive` 等）は、マップされた実ファイルに対して実際に `flock` を発行するか、差し替えが純メモリ mock の場合は「未対応」として拒否する。ロックが効いていないのに `Ok(true)` を返す偽装は禁止する。
3. **ネイティブと SIR 評価器の一致**:
   - `--fs-double` 指定時のエラー・拒否・成功の挙動は、SIR 評価器（`sigil run`）とネイティブ実行バイナリ（`sigil build`）で完全に一致しなければならない。

---

## 決定 6: エラー分類・上限・効果の割当て

### 6.1 エラーの分類（`IoErrorKind`）

OS の errno を以下の構造化エラーに分類し、未分類の errno は `Other(Int)` に収める。

| 腕（Kind） | 主な errno | 発生条件 |
|---|---|---|
| `NotFound` | `ENOENT` | パスが存在しない |
| `PermissionDenied` | `EACCES`, `EPERM`, `ELOOP` | 権限不足、symlink 追従拒否（`follow_symlinks(false)` 下での symlink 遭遇） |
| `AlreadyExists` | `EEXIST` | `create_new` や排他作成で既に存在 |
| `Busy` | `EWOULDBLOCK`, `EAGAIN` | 非ブロッキングロックで競合中 |
| `InvalidInput` | `EINVAL` | `append(true)` ハンドルに対する `write_at`/`write_all_at` 呼び出し、不正引数 |
| `LimitExceeded` | `EFBIG`, `EOVERFLOW`, `ENOSPC`, `EDQUOT` | サイズ上限超過、容量不足 |
| `NotSameFileSystem` | `EXDEV` | クロスデバイス rename |
| `IsADirectory` | `EISDIR` | ファイル操作をディレクトリに対して実行 |
| `NotADirectory` | `ENOTDIR` | ディレクトリ操作をファイルに対して実行 |
| `DirectoryNotEmpty` | `ENOTEMPTY` | 空でないディレクトリを削除しようとした |
| `InvalidPath` | `EINVAL` | パスに NUL を含む、`..` による脱出、相対操作での絶対パス混入、不正エンコーディング |
| `Other(Int)` | その他 | 上記以外の OS errno |

### 6.2 境界の上限値

資源枯渇攻撃やバッファオーバーフローを防止するため、境界で以下の静的上限を課す。

| 項目 | 上限値 | 超過時の扱い |
|---|---|---|
| パス長（`MAX_PATH_LEN`） | 4096 バイト | `InvalidPath` で失敗 |
| ファイル名長（`MAX_NAME_LEN`） | 255 バイト | `InvalidPath` で失敗 |
| 単一 I/O 長（`MAX_IO_CHUNK`） | 64 MiB（67,108,864 バイト） | `LimitExceeded` で失敗 |
| ディレクトリ列挙件数（`MAX_DIR_ENTRIES`） | 1024 件／回 | バッファサイズで制限 |
| ファイルオフセット（`MAX_FILE_OFFSET`） | 2^63 - 1 バイト | `LimitExceeded` で失敗 |

### 6.3 効果（Effects）の割当て

- すべてのファイルおよびディレクトリ操作（`open`, `close`, `read`, `write`, `read_at`, `write_at`, `sync`, `lock`, `create_dir`, `remove_dir`, `rename`）は、既存の **`effects fs`** に割り当てる。
- 新しい効果カテゴリ（例: `fs.sync`, `fs.lock` 等）は新設しない。既存の言語仕様・効果検査系との完全な整合を維持する。

---

## 信頼基盤（trust base）の手続きと検証範囲

ADR 0005 決定 3、ADR 0016、および [ADR 0025 補足 A-3](0025-runtime-replacement.md#L220) に従い、本境界の実装に必要な信頼基盤拡張の手続きを定める。

1. **原始操作（`raw.*`）の新設禁止**:
   - 新しい `raw.*` 組み込み関数は追加しない。既存の `raw.syscall`, `raw.alloc`, `raw.free`, `raw.load_*`, `raw.store_*` のみを利用する。
2. **評価器（SIR evaluator）の syscall 許可表の拡張**:
   - 現行評価器（`src/ir_eval/call.rs`）は未知の syscall 番号で停止する。
   - 以下の syscall 番号を検査付きで許可表に追加する:
     - `SYS_PREAD64` (17): `count >= 0`、バッファポインタ妥当性、`offset >= 0`。
     - `SYS_PWRITE64` (18): `count >= 0`、バッファポインタ妥当性、`offset >= 0`。
     - `SYS_FLOCK` (73): `operation` が `LOCK_SH`, `LOCK_EX`, `LOCK_UN`, `LOCK_NB` の適正な組み合わせであること。
     - `SYS_FSYNC` (74): 有効な fd。
     - `SYS_FDATASYNC` (75): 有効な fd。
     - `SYS_GETDENTS64` (217): バッファポインタ妥当性、`count >= 0`。
     - `SYS_MKDIRAT` (258): パス文字列の NUL 終端検証。
     - `SYS_RENAMEAT` (264) または `SYS_RENAMEAT2` (316): パス文字列の NUL 終端検証。
3. **ネイティブ実装（`std.os.fs`）**:
   - `targets server`、`trust base` を宣言した内部モジュールに実装し、利用者のコード（`trust none`）からは直接参照させない。
4. **差分監査と一致検証**:
   - ネイティブ実行（`sigil build`）と SIR 評価器（`sigil run`）の間で、正常系結果、エラー種別、上限違反、および `--fs-double` 下の挙動が完全に一致することを検証する。

---

## 後続 fs 実装 Issue の切り分け（完了条件の確かめ方）

本 ADR の合否および完了条件の確かめ方として、後続の 3 つの実装 Issue を明確に切り分ける。

### Issue A: 資源・名前・排他（fs A）
- **範囲**:
  - `File` / `Dir` ハンドルの定義、単一所有権、明示的 `close`、Drop 時自動クローズ。
  - `OpenOptions` によるファイルオープン、`O_CLOEXEC`、`create_new`（排他的作成）、`follow_symlinks(false)`（`O_NOFOLLOW`）。
  - `Dir` ハンドルによるルート固定相対オープン（`Dir::open_file` 等）。
  - パス検証（NUL 拒否、`..` 拒否、絶対パス拒否、symlink 拒否）。
  - `flock(2)` による非ブロッキング排他／共有ロック（`try_lock_exclusive`, `try_lock_shared`, `unlock`）。
  - ディレクトリ作成（`create_dir`）、削除（`remove_dir`, `remove_file`）、上限付き列挙（`read_entries`）。
- **依存**: 本 ADR（ADR 0049）のマージ。
- **完了条件**:
  - ハンドルのライフサイクルが正しく機能し、fd リークがないこと。
  - 既存ファイルに対する `create_new` が `AlreadyExists` で失敗すること。
  - 専用ディレクトリ配下の操作において `..` による親ディレクトリへの脱出試行が `InvalidPath` で確実に拒否されること。
  - 末尾および構成要素がシンボリックリンクであるファイル／ディレクトリを開こうとした際に、リンクを追従せず明示的に拒否（`PermissionDenied` / `InvalidPath`）されること（ADR 0045:109 要件の検証）。
  - プロセス間ロックが正しく排他され、競合時に `Busy` が返ること。
  - ディレクトリ操作が上限内で動作すること。
  - ネイティブと SIR 評価器で全テストが一致して成功すること。

### Issue B: ページ I/O と WAL 追記（fs B）
- **範囲**:
  - 位置指定読み書き `read_at` / `write_at`（`SYS_pread64` / `SYS_pwrite64`）、完全充足ループ（`read_exact_at`, `write_all_at`）。
  - `append(true)` による `O_APPEND` 追記モード。
  - **`append(true)` ハンドルに対する `write_at`/`write_all_at` の拒否契約（`InvalidInput`）**。
  - `EINTR` の自動再試行、Short I/O の処理、EOF 判定。
  - オフセットおよび長さの境界値・オーバーフロー検査。
- **依存**: Issue A（ハンドルとオープン基盤）。
- **完了条件**:
  - 8192 バイト境界でのランダムな位置指定読み書きがシークなしに正しく行えること。
  - `append` モードで複数書き込みが常にファイル末尾へ追記されること。
  - `append(true)` で開かれた `File` ハンドルに対して `write_at` または `write_all_at` を呼び出した際、書込みを実行せず即座にエラー（`InvalidInput`）を返すこと（Linux の `pwrite64` による誤追記防止の検証）。
  - `EINTR` および Short I/O 注入試験でデータ欠損なく完了すること。
  - ネイティブと SIR 評価器で挙動が一致すること。

### Issue C: 同期と原子的置換（fs C）
- **範囲**:
  - `File::sync_all`（`fsync`）および `File::sync_data`（`fdatasync`）。
  - `Dir::sync`（ディレクトリに対する `fsync`）。
  - `fs.rename`（同一 FS 内の原子的置換、`SYS_renameat`）、異 FS 時の `NotSameFileSystem` 失敗。
  - `--fs-double` における同期・rename・ロックの振る舞い検証。
- **依存**: Issue A および Issue B。
- **完了条件**:
  - ファイル更新後の `sync_all` および親ディレクトリ `sync` が正常終了すること。
  - 原子的 `rename` により、既存ファイルが欠落区間なしに置換されること。
  - 異なるマウントポイント間での rename が `NotSameFileSystem` で明示的に失敗すること。
  - 強制終了／クラッシュ注入シミュレーションにおいて、同期境界前のデータ保全が確認できること。
  - **Issue C の完了をもって初めて、P4-1 データベース核が永続 DB を完成扱いできる前提が満たされる**。

---

## 脅威と対策

| 脅威 | 対策・残る限界 |
|---|---|
| ディスク満杯／クォータ枯渇による不完全書込み | Short I/O を検知してエラーを即座に返し、部分書込み状態での commit 成立を阻止する |
| TOCTOU レース（ファイル作成時の割込み） | `O_CREAT | O_EXCL` による排他的作成を強制し、検査と作成の隙間を排除する |
| 他プロセス close によるロック消失 | POSIX `fcntl` を排除し、OFD 単位で保持される `flock(2)` を採用する |
| 電源断・クラッシュによるメタデータ消失 | ファイル自身の `fsync` に加え、親ディレクトリの明示的 `fsync` を契約として要求する |
| パストラバーサル・symlink・ディレクトリ外アクセス | `Dir::open_file` によるルート固定相対オープン、`..` および絶対パスの構文的拒否、`O_NOFOLLOW` によるシンボリックリンク追従拒否により、専用ディレクトリ外へのアクセスおよび中間 symlink によるすり替え破壊を遮断する（ADR 0045:109 要件） |
| append と位置指定の混同によるページ破壊 | `append(true)` ハンドルでの `write_at` 呼出しを境界で即座に拒否し、`O_APPEND` を共有しない別個のオープンファイル記述を強制する |
| 同一 OS 権限の悪意ある他プロセス | 同一 UID による直接的ストレージ破壊は OS 境界外。専用実行ユーザーとパーミッション設定に依存する |

---

## 正式判定の記録

2026-11-16 の所有者指示に従い、Gemini Decisions は使わず typesafe-ai のみを用いる。

**起草時点では未実施である。** 起草環境（cloud / agy）には `TYPESAFE_API_KEY` が設定されておらず、通信が遮断されている（AGENTS.md の「キーが無い環境」。ADR 0042・0043・0044・0046・0047 起草時と同様）。そのため、本起草 PR では採用案を「判定を経ていない起草者の案」として明記し、以下に判定用の評価軸と問いを記録する。後続のレビューまたは見張りセッション（Codex Cloud 等のキー設定済み環境）において正式判定を実施可能とする。

### 判定用の評価軸と問い（再現用）

- **評価軸（固定）**: Sigil は AI が実装し人間が仕様だけをレビューする原則（ADR 0001）。ADR 0005 の段階表と信頼基盤最小化、ADR 0016 の明示許可・unchecked 前提・ADR 先行、ADR 0044（P4 の範囲、単一ノード DB 核、外部公開しない方針）、ADR 0045（P4-1 データベース核、順序0 fs 境界 ADR、ACID と持続性の厳格保証、fs A〜C 分割）、Issue #576 結論（ファイル 6 不足の公開 std.fs 拡張＋std.os.fs 信頼基盤拡張）。

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間が仕様レビューする。ADR0005の信頼基盤最小化、ADR0016のADR先行、ADR0044のP4範囲（単一ノードDB核、所有者作成別repo実装、外部公開しない方針）、ADR0045のP4-1核（直列実行serializable、固定8192byteページ、全ページredo WAL、no-steal、fsync後commit ack、順序0 fs境界ADR先行、fs A〜C揃うまで永続DB完成扱いにしない、ADR0045:109の相対内部名/symlink拒否要件）、Issue576結論（ファイル6不足を公開std.fs拡張かつ既存std.os.fs信頼基盤拡張とし、圧縮/SHA1は純Sigil trust noneで別扱い）を守る。所有者指示: Issue584の共通fs境界ADR0049と索引のみ、実装/新規Issue/マージ禁止。版取得方法はpinned_source（完全SHA 4867dc61190c4e2e6953adb25240ea178603c0ed + Cargo.lock固定ビルド）。案: 公開APIは所有権を持つFile/Dirハンドル型（明示close+drop自動クリーンアップ、生fd非露出）。Dir起点のルート固定相対操作と..拒否、O_NOFOLLOWによるsymlink追従拒否（ADR0045:109要件）。ロックはflock(2)を採用し、同一プロセスの任意fd closeで暗黙解除されるPOSIX fcntlを禁止しOFD寿命に結合。追記はO_APPENDによるwriteごとの末尾アトミック位置決定（レコード全体の原子性は保証せず）、位置指定IOはpread/pwriteでシーク競合排除し、Linuxのpwrite追記化による誤書込み防止のためappendハンドルでのwrite_atを明示拒否。持続性はclose成功を信用せず明示fsyncと親ディレクトリfsyncを要求。renameは同一FSアトミック置換としEXDEVは明示拒否。--fs-doubleは実ファイル委譲と未対応操作明示拒否(fail-closed)。エラーはIoErrorKind、効果は既存effects fsに割当て新raw名追加なし。後続はfs A(資源/名前/排他/symlink拒否)→B(ページIO/追記/write_at排他)→C(同期/原子的置換)に分割。",
  "questions": {
    "lock_mechanism": {
      "type": "choice",
      "criteria": {
        "flock_ofd": "flock(2)を採用しオープンファイル記述単位で排他管理、任意fd closeによる解除を防止",
        "posix_fcntl": "従来のPOSIX fcntl(F_SETLK)を採用",
        "process_mutex_only": "OSプロセス間ロックを提供せずプロセス内mutexのみとする"
      }
    },
    "io_and_durability": {
      "type": "choice",
      "criteria": {
        "pread_pwrite_strict_sync": "pread/pwrite位置指定IO+O_APPEND+appendハンドルでのwrite_at明示拒否+明示fsync/親dir同期+同一FSアトミックrename",
        "seek_based": "シーク(lseek)と通常のread/writeに依存し同期はcloseに委ねる",
        "fake_sync": "同期要求をノーオペレーション(no-op)として扱う"
      }
    },
    "path_and_symlinks": {
      "type": "choice",
      "criteria": {
        "strict_root_relative_no_symlinks": "Dir起点ルート固定相対操作、..拒否、O_NOFOLLOWによるsymlink拒否を境界契約とする",
        "allow_arbitrary_paths": "任意絶対パスや..脱出、symlink追従を許容する",
        "path_only_no_symlink_check": "パス構文検査のみ行いsymlink検査は行わない"
      }
    },
    "split_plan": {
      "type": "choice",
      "criteria": {
        "three_issues_a_b_c": "ADR確定後、fs A(資源/名前/排他)→B(ページIO/追記)→C(同期/原子的置換)の3Issueに分割しC完了までDB完成にしない",
        "one_big_fs_issue": "すべてのfs実装を単一の巨大Issueで行う",
        "no_split_with_db": "fs分割を行わず別repoのDB実装と同時に実装する"
      }
    }
  }
}
```

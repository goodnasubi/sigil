# 0025: ランタイムの置き換え（原始操作・大域状態・確保器・標準ライブラリの Sigil 実装）

- **状態**: 提案（PR 3-1。所有者レビュー待ち）。補足 A・B は所有者承認済み（「推奨で」、2026-11-02）。補足 C は所有者承認済み（「推奨で」「自動的に進めて」、2026-11-02）。補足 D は提案で、本文の「仮置き: D-8 の n」の印の箇所が所有者の確認待ち
- **日付**: 2026-11-01
- **対象**: 原始操作（`raw.*`）、大域状態構文、アロケータ設計、標準ライブラリ核・OS の Sigil 実装、暫定 Rust ランタイム（`runtime/`）の削除

---

## 背景

[ADR 0005](0005-long-term-goals.md) は「標準ライブラリとランタイムに既存言語（Rust / C）のライブラリを持ち込まない」ことを言語の基本方針とし、[ADR 0006](0006-p0-scope-and-bootstrap.md) は暫定 Rust 製ランタイム（`runtime/core`, `runtime/os`）を「P1 で Sigil 自身で書いた信頼基盤モジュールに置き換える期限付きの例外」と定めた。

[ADR 0018](0018-p1-scope.md) 決定 6 では暫定ランタイム置き換えの基本方針と最初の原始操作一覧を定め、その細目（型の細部・大域状態構文・確保器設計・移行手順）を本 ADR（3-1）で決定することとした。
また、PR 2-4d（[ADR 0022](0022-type-identity-traits-and-function-types.md)）、PR 2-5（[ADR 0023](0023-bytes-and-bitwise.md)）、および PR 3-0（`trust-base.list` 許可ファイルによる `--trust-base` の運用管理）がマージされたことで、段階 3（P1-R: ランタイムの置き換え）に着手する前提条件がすべて整った。

本 ADR は、段階 3 の完遂に必要な以下の重要事項を決定する：
1. **原始操作（`raw.*`）の完全なシグネチャ・前提契約・型の定義**
2. **大域状態（`global`）の構文と安全性規則（ADR 0016 決定 5 の具体化）**
3. **Sigil 製確保器（アロケータ）の設計・大域状態との結合・前提条件**
4. **標準ライブラリの性質（トレイト）と Sigil への移植設計**
5. **PR 3-2 〜 PR 3-7 への分割計画と移行手順（突き合わせによる安全性の担保）**

---

## 決定 1: 原始操作（`raw.*`）の仕様と契約

原始操作は、信頼基盤（`trust base` かつ許可一覧 `trust-base.list` に登録されたモジュール）の `unchecked` 関数内でのみ直接呼び出し可能とする（ADR 0016）。（補足 A で置換: 処理系に埋め込んだ `std.*` は一覧を経ずに許可される。A-6）

### 1. `Addr` 型の導入

番地を直接扱う型として、核層（`Layer::Core`）に不透明な組み込みポインタ型 **`Addr`** を導入する。
- 内部表現は 64 ビット符号なし整数（ネイティブでは Cranelift の `I64` ポインタ値、SIR 評価器では `usize` 番地）。
- 算術演算子（`+`, `-`）は直接適用できず、`raw.offset` または明示的な変換関数（`raw.addr_to_int` / `raw.int_to_addr`）を介して操作する（誤ったポインタ計算の抑制）。
- `Addr.null() -> Addr`（番地 0）を定数/関数として提供する。

### 2. 原始操作の一覧

| 原始操作 | 層 | 引数 / 戻り値 | 効果 | 内容と事前条件（`requires`） |
|---|---|---|---|---|
| `raw.addr_null` | Core | `() -> Addr` | なし | ヌル番地（0）を返す |
| `raw.addr_to_int` | Core | `(a: Addr) -> Int` | なし | 番地を整数値として取得する |
| `raw.int_to_addr` | Core | `(v: Int) -> Addr` | なし | 整数値を番地に変換する（`v >= 0`） |
| `raw.offset` | Core | `(a: Addr, n: Int) -> Addr` | なし | 番地を `n` バイト進める（`n` は負でもよい） |
| `raw.load_u8` | Core | `(a: Addr) -> Int` | なし | 番地から 1 バイトを 0〜255 として読み取る |
| `raw.store_u8` | Core | `(a: Addr, v: Int) -> ()` | なし | 番地へ 1 バイト書き込む（`0 <= v && v <= 255`） |
| `raw.load_i64` | Core | `(a: Addr) -> Int` | なし | 番地から 8 バイト（リトルエンディアン）読み取る |
| `raw.store_i64` | Core | `(a: Addr, v: Int) -> ()` | なし | 番地へ 8 バイト書き込む |
| `raw.load_addr` | Core | `(a: Addr) -> Addr` | なし | 番地からポインタサイズ（8 バイト）の `Addr` を読む |
| `raw.store_addr` | Core | `(a: Addr, v: Addr) -> ()` | なし | 番地へ `Addr` を書き込む |
| `raw.copy` | Core | `(dst: Addr, src: Addr, n: Int) -> ()` | なし | `src` から `dst` へ `n` バイト複写する（`memmove` 相当。重複可、`n >= 0`） |
| `raw.set` | Core | `(dst: Addr, byte: Int, n: Int) -> ()` | なし | `dst` から `n` バイトを `byte`（0〜255）で埋める（`memset` 相当、`n >= 0`） |
| `raw.alloc` | Core | `(size: Int, align: Int) -> Addr` | なし | 指定バイト・アライメントの領域を確保する（`size > 0`, `align` は 2 の累乗） |
| `raw.free` | Core | `(a: Addr, size: Int, align: Int) -> ()` | なし | 確保した領域を返却する（`a != Addr.null()`） |
| `raw.syscall` | OS | `(n: Int, a0: Int, a1: Int, a2: Int, a3: Int, a4: Int, a5: Int) -> Int` | 宣言に委ねる | Linux x86-64 システムコール（`syscall` 命令）を実行する |

- **効果の扱い**: ADR 0018 決定 6 に従い、メモリ読み書き・確保・解放はプロセス外から直接観測できないため「効果なし」とする。`raw.syscall` は呼び出し側関数の宣言に委ねる。
- **実行器と評価器の保護**:
  - SIR 評価器は、確保されたチャンクの境界情報（`Addr`, `size`, `align`, 有効フラグ）を管理するテーブルを保持する。
  - 範囲外アクセス、未確保番地の読み書き、二重解放（`double-free`）、不一致サイズでの解放を検知した場合は直ちに評価器がパニック（契約違反停止）を発生させる。
  - `raw.syscall` では、許可されたシステムコール番号（`read`, `write`, `openat`, `close`, `mmap`, `munmap`, `exit_group` 等）のみを実行し、引数で渡されたポインタ境界が生きたチャンク内にあるかを事前検証する。

---

## 決定 2: 大域状態（`global`）の構文と安全性規則

[ADR 0016](0016-trust-base-declaration.md) 決定 5 で定められた大域状態の基本原則を具体化し、アロケータや OS 入出力テーブルに必要な大域状態の構文を導入する。

### 1. 宣言の構文

大域状態は、`trust base` モジュールのトップレベルにおいてのみ宣言可能とする。

```sigil
module std.alloc
  targets all
  trust base

global mut ALLOCATOR_STATE: Allocator = Allocator.new()   // 補足 A で置換: 初期化式は定数のみ（A-1）
```

- **構文規則**:
  - キーワード: `global`（不変）または `global mut`（可変）。
  - 大域変数の名前は大文字スネークケース（`UPPER_SNAKE_CASE`）。
  - 初期化式は定数式（コンパイル時評価可能）、または副作用のない純粋関数呼び出し（`effects none`）に限る。（補足 A で置換: 純粋関数呼び出しは採らず、定数式のみ。A-1）
- **アクセス規則**:
  - `global mut` へのアクセス（読み書き）は、**`unchecked` 関数内でのみ許可**される（通常の関数からは参照できない）。
  - アクセスする `unchecked` 関数の前提節（`unchecked "..."`）には、並行アクセス安全性（タスク間排他またはアロケータの無競合性）の根拠を明記しなければならない。
  - P1-R（段階 3）の時点では単一スレッド／協調マルチタスク前提であるため、アロケータの関数がリエントラントにならない（メモリ確保中にシグナルハンドラや中断・コンテキストスイッチを挟まない）ことを前提とする（ADR 0018 決定 7）。

---

## 決定 3: アロケータ（確保器）の設計と前提条件

暫定ランタイムの Rust アロケータに依存せず、Sigil 自身でメモリ管理を完結させる。

### 1. アロケータの構造

（ブロック頭・整列・異常停止の細目は補足 A-4 を参照。）

OS 依存層（`std.os.alloc`）から Linux システムコール（`mmap` / `munmap`）を直接呼び出し、ページ単位でメモリを取得して核アロケータ（`std.alloc`）へ供給する。

- **小サイズ領域（<= 2048 バイト）: スラブ／サイズクラス方式**:
  - 16, 32, 64, 128, 256, 512, 1024, 2048 バイトのサイズクラス。
  - 各サイズクラスごとに 64KB または 4KB 単位のページブロックを `mmap` し、フリーリストを管理。
  - 高速な確保・解放とフラグメンテーションの抑制。
- **大サイズ領域 (> 2048 バイト）: ダイレクト `mmap`**:
  - ページサイズ（4096 バイト）の倍数に繰り上げて直接 `mmap`（`PROT_READ | PROT_WRITE`, `MAP_PRIVATE | MAP_ANONYMOUS`）。
  - 先頭に確保サイズなどのメタデータを記録し、`raw.free` 時に `munmap` でカーネルへ返却。
- **アライメント**:
  - すべての確保ブロックは最低 16 バイトアライメントを保証する（Cranelift SIMD / 64 ビット整数の要求を満たす）。
- **核層と OS 層の結合**:
  - 核の `raw.alloc` / `raw.free` は、OS 依存層のアロケータエントリポイント（`std.os.alloc`）に処理をディスパッチする。
  - ネイティブ出力では、Cranelift が生成する組み込み関数シンボル `sigil_raw_alloc` / `sigil_raw_free` としてリンクされる。（補足 A で置換: 生成コードは現行の `sigil_rt_raw_alloc` / `sigil_rt_raw_free` を呼び、その定義を Sigil 製確保器に差し替える。A-2）

### 2. メモリ枯渇・契約違反時の振る舞い

- メモリ確保の失敗（`mmap` が `MAP_FAILED` を返した等）は、[ADR 0018](0018-p1-scope.md) 決定 7 に従い、**プロセス全体の異常停止（`abort` / `exit_group(1)`）** とする（タスク単位の隔離は行わない）。（補足 A で置換: 終了コードは既存の異常停止に合わせ `exit_group(70)`。A-4）
- 解放時、渡されたサイズ・アライメントがブロックのメタデータと異なる場合、デバッグビルド（または評価器）では異常停止し、ネイティブ最適化ビルドでは未定義動作とならないよう防護策（サイズヘッダの検証）を講じる。

---

## 決定 4: 標準ライブラリの Sigil 実装方針とトレイト

既存の `runtime/core` および `runtime/os` に Rust で書かれていた標準ライブラリの各機能を、すべて Sigil 言語で再実装する。

### 1. 核ライブラリ（`std.core`）の Sigil 実装

1. **`String`**:
   - 表現: `{ ptr: Addr, len: Int, cap: Int }`（UTF-8 バイト列）。
   - ヒープ確保バッファの管理、連結、スライス、UTF-8 検証を Sigil で実装。
   - `Equatable` トレイトの実装（定数時間または通常比較）。
   - デストラクタ: `raw.free(ptr, cap, 1)`。
2. **`List<T>`**:
   - 表現: `{ ptr: Addr, len: Int, cap: Int }`。
   - 幾何学的リサイズ（2倍拡大）、要素のインデックスアクセス、イテレーション。
   - 要素破棄の走査とバッファの解放。
3. **`Map<K, V>`**:
   - オープンアドレス法（ロビンフッドハッシュまたは二次走査法）。
   - ハッシュ値の計算とバケットバッファ管理。
4. **`Bytes`**:
   - ADR 0023 で規定された表現。スライシング、ビット演算、ゼロ埋め。
   - 破棄時の適切なメモリ返却。

### 2. OS 依存ライブラリ（`std.os`）の Sigil 実装

Linux x86-64 システムコール（`raw.syscall`）を直接発行して実装する。
- **`console`**:
  - `console.log` / `console.err`: システムコール `SYS_write`（1 番）を fd 1 / fd 2 に対して発行。
- **`fs` / `path`**:
  - `SYS_openat` (257), `SYS_read` (0), `SYS_close` (3), `SYS_fstat` / `SYS_newfstatat` (262), `SYS_unlinkat` (263) 等。
- **`env`**:
  - プロセス起動時のスタックレイアウトから `envp`（環境変数ポインタ配列）を読み取るか、起動コード（`_start`）で取得した大域番地から環境変数を辞書化。

---

## 決定 5: ネイティブ起動コード（`_start`）と glibc からの脱却

[ADR 0018](0018-p1-scope.md) 判断 7（所有者承認済み）に基づき、P1-R の完了をもってネイティブ出力から glibc のリンクを完全に廃止し、`-nostdlib -static` でバイナリを生成する。

### 1. 起動エントリポイント（`_start`）の生成

コンパイラは、実行可能バイナリの真のエントリポイント `_start` をアセンブリ／機械語として先頭に配置する：
1. スタックポインタ `rsp` から `argc` をポップ（8 バイト）。
2. `argv` の先頭番地（`rsp`）を取得。
3. `envp`（`argv + (argc + 1) * 8`）を取得し、環境変数大域ポインタに設定。
4. アロケータおよび標準ライブラリの初期化関数を呼び出し。（補足 A で置換: 大域は静的データで初期化されるため、P1-R では呼び出す初期化関数はない。A-1）
5. ユーザーの `main` 関数を実行。
6. `main` の戻り値コードを引数として、システムコール `SYS_exit_group`（231 番）を発行。

これにより、外部の C ランタイム（`crt1.o`, `libc.a`）を一切含まず、Sigil で生成されたコードとカーネルシステムコールだけで自己完結した Linux ELF バイナリを出力する。

---

## PR 分割計画と移行手順（段階 3: PR 3-2 〜 PR 3-7）

ADR 0018 で策定された分割を詳細化し、各 PR において常に「SIR 評価器とネイティブバイナリの動作突き合わせ」をパスさせながら安全に置き換える。

| PR | タイトル・責務 | 担当レーン | 変更内容・完了基準 |
|---|---|---|---|
| **PR 3-1**（本 PR） | **ADR 0025: ランタイムの置き換え** | Lane E | 本 ADR の起草・合流（実装は含めない） |
| **PR 3-2** | **原始操作（`raw.*`）の検査と SIR 評価器** | Lane B5, Lane C1, Lane D2 | `RAW_OPS` テーブルの登録、型検査、SIR 評価器での実行・バッファ境界検証、単体テスト |
| **PR 3-3** | **原始操作のネイティブコード生成とシステムコール** | Lane C2 | Cranelift による `raw.*` 命令生成、`_start` エントリポイントの生成、システムコール発行確認 |
| **PR 3-4** | **大域状態（`global mut`）と Sigil 製アロケータ** | Lane A1, Lane B3, Lane C1, Lane C2 | 大域構文のパース・検査・lowering、Sigil 製スラブアロケータの実装、`SIGIL_RT_LIVE` テスト（補足 A-7 で PR 3-4a〜3-4i に分割） |
| **PR 3-5** | **標準ライブラリ核（`String`, `List`, `Map`）の Sigil 実装** | Lane D2, Lane C1, Lane C2 | `std.core` 各型の Sigil 実装、既存 Rust 組み込みとの挙動突き合わせ（400 件超全テスト通過） |
| **PR 3-6** | **標準ライブラリ OS 部門（`console`, `fs`, `path`, `env`）の Sigil 実装** | Lane D2, Lane C2 | `std.os` の Sigil 実装、システムコール直接呼び出し、全 OS テストの通過 |
| **PR 3-7** | **暫定 Rust ランタイム（`runtime/`）の削除と glibc リンク解除** | Lane C2, Lane D2, Lane E | `runtime/core`, `runtime/os` ディレクトリの完全削除、`-nostdlib -static` 移行、ADR 0016 決定事項の見直し記録 |

（補足 C で置換: PR 3-5・3-6 は未完了と認定し、PR 3-7 と合わせて 3-5a〜3-5g・3-6a〜3-6c・3-7a〜3-7e に分割し直す。C-7）

### 移行中の突き合わせ戦略

- PR 3-5 および PR 3-6 の進行中、まだ置き換わっていない標準機能は既存の暫定ランタイム・組み込みを利用し、置き換えが完了した機能から順次 Sigil 実装へ切り替える。
- 切り替えの各ステップで、`scripts/native-report.sh` および全統合テスト（768 件以上）を実行し、Rust 実装時代と 1 件の差分もなく動作することを保証する。

---

## 補足 A（PR 3-4a）: 決定 2・3 の細目

- **日付**: 2026-11-02
- **状態**: 所有者承認済み（「推奨で」、2026-11-02）。以下は決定 2・3 を狭める・補う記録で、設計の選択肢はすべて推奨案を採用した。本文の古い記述と食い違う箇所は、本補足が置き換える。

### A-1. 大域（決定 2 の細目）

- 大域の型は `Copyable` な型に限る（`Int`・`Bool`・`Addr`、およびそれだけからなる構造体・組）。破棄の要る型（`List`/`String` 等）は不可。終了時の破棄順序を決めずに済ませるため。
- 初期化式は定数式に限る（整数・真偽値の字句、それだけからなる構造体・組）。決定 2.1 の「純粋関数の呼び出し」は P1-R では採らない（要るときに追加）。帰結として初期化はコンパイル時に定数化でき、起動時の初期化関数・初期化順序は不要で、決定 5.1 の手順 4 は P1-R では何もしない。
- 特例: `raw.addr_null()` は大域の初期化式に限り `unchecked` 外でも書ける。
- `pub global` は構文エラーとする。大域はモジュール内に閉じ、他モジュールからは `unchecked` 関数経由でのみ触る。
- `global mut` に触る関数は、中断しうる効果（`task.suspend`、および中断点になりうる `task.spawn`。効果名は `src/effects.rs` の `EffectTable` による）を宣言できない。ADR 0018 決定 7 の「確保器の中に中断点を置かない」を効果検査で機械的に担保する。
- 不変の `global` は `unchecked` でない通常の関数からも読める（読むだけで状態が変わらないため）。`global mut` の読み書きは決定 2 のとおり `unchecked` のみ。
- 大域の読み書きの効果は `none` のままとする。ただし `effects none` の関数が非決定的になりうることを、`unchecked` の前提の文に書く義務とする（ADR 0016 決定 5 規則 2 への追記）。
- 大域から `Copyable` でない値を取り出す経路は型の制限で消えるが、所有権検査でも「大域を移動の元にしない」「借用を関数の外へ出さない」を明示的に拒否する。
- `sigil spec` は、信頼基盤のモジュールの行の下に大域の宣言（例: `大域: mut HEAP: Addr = raw.addr_null()`）を並べ、`unchecked` 関数ごとに「触る大域: 名前（読み書き / 読む）」を名前順・重複なしで出す。

### A-2. 評価器の `raw.alloc` と Sigil 確保器（決定 3 の例外）

- 評価器の `raw.alloc` / `raw.free` は組み込み（Rust の確保＋塊の表）のまま**恒久的に**残し、Sigil 製確保器（`std.os.alloc`）はネイティブでのみ使う。評価器にとってのプラットフォーム層は評価器自身、という整理（ADR 0018 決定 6「実体は出力先のプラットフォーム層が与える」）。
- これは ADR 0018 決定 6「替えた後は評価器も Sigil 版を実行し、組み込みを消す」の**例外**である。評価器が Sigil 確保器を通すと塊の表が `mmap` のページ単位になり、ブロック単位の検知（範囲外・二重解放・不一致解放）を失うため。
- Sigil 確保器の正しさは、評価器上で確保器の関数を直接呼ぶ「影の表」付きテスト（返った番地が `mmap` 済み塊の中・互いに重ならない・`free` の size/align が一致）で確かめる。
- 2 つの確保器の共存: Rust ランタイム自身の `String`/`List` は PR 3-5 まで glibc の `malloc` のままで、`raw.alloc` は Sigil 確保器を使う。規則は「`raw.free` に渡してよいのは `raw.alloc` で得た塊だけ」。（補足 B で細目: ネイティブで Sigil 確保器を使うのは確保器を結んだプログラムだけで、入口の targets が server だけでないときは PR 3-7 まで Rust の確保器。B-2〜B-4）評価器の塊の表（由来タグ付き、A-3）がこれを突き合わせる。

### A-3. `raw.syscall` の許可表（決定 1 の細目。確保器より先に入れる）

評価器の `raw.syscall` は現状、番号の許可表も番地引数の範囲検証もなく、ADR 0018 決定 6・本 ADR 決定 1 に未適合である。**確保器の導入（3-4i）より先に**、3-4b で次を入れる。

- 最初の許可表（9 個）: `read`(0)・`write`(1)・`close`(3)・`mmap`(9)・`munmap`(11)・`exit_group`(231)・`openat`(257)・`newfstatat`(262)・`unlinkat`(263)。表にない番号は停止する。行の追加は ADR 0018 判断 2 のレビューを要する。`getcwd` 等は 3-5/3-6 で必要になれば同じ手続きで足す。
- 番地を渡す引数は、実行前に生きた塊の範囲内にあることを検証し、外なら停止する。
  - `openat`/`newfstatat`/`unlinkat` のパスは NUL 終端を必須とし、終端までが範囲内であること。`statbuf` は固定サイズで範囲内であること。
  - `read`/`write` は `buf` から `count` バイトが範囲内であること。
- `mmap` は `addr` = 0、`prot` に `PROT_EXEC` を含まない、`flags` は `MAP_PRIVATE|MAP_ANONYMOUS` のみ（`MAP_FIXED`/`MAP_FIXED_NOREPLACE` は拒否）、`fd` = −1、`offset` = 0 のみ許可する。成功値を塊の表に「`mmap` 由来」タグ付きで足す。（補足 B で細目: `prot` は `PROT_READ|PROT_WRITE` との完全一致のみ。B-6）
- `munmap` は、`mmap` で得た要求長を 4096 に切り上げた長さと完全一致する範囲のみ許可し、表から除く。
- 塊は由来タグ（`raw.alloc` 由来 / `mmap` 由来）を持つ。由来違いの `munmap` / `raw.free` は停止する。
- `close` の fd と `openat`/`unlinkat` が触るホストのパスは、評価器では縛れない。効果の委譲（ADR 0016 決定 4）で守る前提とする。
- `exit_group` は評価器では捕捉可能な停止として模擬する（処理系のプロセスを終了させない）。
- 塊の探索は線形探索から `BTreeMap::range` に改める。

### A-4. 確保器のブロック頭と異常停止（決定 3.1・3.2 の細目）

- ブロックの頭は 16B で、`magic|class`（8B）と `size`（8B）。**返却アドレスは block+16** とし、16B 境界を維持する。ヘッダに `align` 欄はない。
- `align` の上限は 4096。超過は評価器・ネイティブとも同じ停止（検査エラー相当）とする。
- `align` ≤ 16 の要求のみスラブ（小ブロック）経路で、返却アドレスは block+16。
- `align` > 16 は大ブロック経路（ページ境界）とする。`round_up(align + 16 + size, 4096)` を `mmap`（先頭を `base`）し、返却アドレスを `round_up(base + 16, align)` とする。頭はその直前の 16B に置き、`size` に加えて `base` を持たせて、`free` が `munmap` の範囲を復元できるようにする。`align` ≤ 16 の大ブロックは返却アドレスが `base`+16 になる。（補足 B で置換: 頭は 16B のまま、`base` と `mmap` の長さは align > 16 のときだけ頭のさらに前の 16B に置く。B-1）
- `free(a, size, align)` は頭を読み、magic と class/size が一致しなければ停止する。ヘッダに `align` 欄がないため、**ネイティブは解放時の `align` を確保時の `align` と突き合わせない**（`align` が 2 の冪かつ 1 以上 4096 以下であることは確保時と同じく検査し、違えば停止する。補足 B-1）。評価器は塊の表で確保時の `align` と厳密に照合するので、評価器とネイティブの差分テストは評価器側の厳密検査を基準にする。
- スラブの二重 `free` は解放済み印（FREED）で最善の努力として検知する。大ブロックの二重 `free` は、`mmap` 由来タグと表で検出して停止する（SIGSEGV に頼らない）。
- 異常停止は `exit_group(70)` とする。既存の異常終了コード（`runtime/core` の `EXIT_ABNORMAL_STOP` = 70、ADR 0014）に合わせるためで、設計案の 134（SIGABRT 相当）は採らない。`size` > 2^46 も同じ停止とする。`mmap` 失敗（戻り値 −4095〜−1）も、決定 3.2 の「プロセス全体の異常停止」として同じ 70 で停止する。
- すべての検査は状態の書き換えより前に終える。`size` の上限を最初に確かめ、切り上げの算術のあふれが確保器の途中で起きないようにする。
- `SIGIL_RT_LIVE` には Sigil 確保器の生存数を `sigil_blocks=` として追加する（既存の `strs=` / `blocks=` の書式は変えない）。（補足 B で細目: 確保器を結んだときだけ出す。B-5）

### A-5. 単一スレッド前提

確保器は排他を持たない。ネイティブで並行処理を入れる PR は、確保器の前提の文の見直しを必須とする。

### A-6. 埋め込み標準 Sigil モジュール

確保器（`std.os.alloc`）は処理系に埋め込んだ Sigil ソースとし、利用者のプログラムと同じ検査・lowering にかけて合流させる。`std.*` の `trust base` は処理系内蔵として許可する（`trust-base.list` を経ない。根拠は処理系のソースに入っていること）。`sigil spec` と CI の信頼基盤差分検出の対象に加える。組み込みは独立の PR（3-4h）とする。`core`/`bare` 対象での `raw.alloc` は、ADR 0014 決定 2 に従い引き続き使える（確保器の供給元は出力先による）。（補足 B で細目: 結ぶのは `raw.alloc`/`raw.free` を使うプログラムだけで、`sigil spec` にもそのときだけ出す。入口が `server` だけでないときは PR 3-7 まで Rust の確保器。B-3・B-4）

### A-7. PR 3-4 の分割

| PR | 範囲 | 依存 |
|---|---|---|
| 3-4a | 本補足（docs のみ） | — |
| 3-4b | 評価器の `raw.syscall` 許可表・番地検証・`MAP_FIXED` 拒否（要承認） | 3-4a |
| 3-4c | 大域の検査の仕上げ（`Copyable` 限定・定数初期化・`pub global` 拒否・中断効果の禁止）（要承認） | 3-4a |
| 3-4d | 所有権: 大域を移動の元にしない | 3-4c |
| 3-4e | `sigil spec` に大域と「触る大域:」を表示（要承認） | 3-4c |
| 3-4f | IR 契約変更: `Program.globals`・`Rvalue::GlobalRef`（`src/ir.rs` のみ、`contract-change` ラベル） | 3-4a |
| 3-4g | lowering・評価器・ネイティブの大域 | 3-4f・3-4c |
| 3-4h | 埋め込み標準 Sigil モジュールの組み込み | 3-4g |
| 3-4i | Sigil 製確保器とネイティブ `raw.alloc`/`free` の差し替え（要承認） | 3-4b・3-4d・3-4h |

3-4b と 3-4c〜3-4e は並行可。スラブのページは 4096B 単位で `mmap` する（決定 3.1 の「64KB または 4KB」のうち 4KB を採る）。

### 補足 B（PR 3-4i の前）: 確保器の差し替えの細目

- **日付**: 2026-11-02
- **状態**: 所有者承認済み（「推奨で」、2026-11-02）。3-4i の着手前に見つかった補足 A の食い違いと未決の点を決める。補足 A と食い違う箇所は、本補足が置き換える。

#### B-1. 大ブロックの頭と `munmap` の長さ（A-4 の食い違いの解消）

A-4 は「頭は 16B（`magic|class` と `size`）」と「align > 16 の頭に `base` も持たせる」を同時に求めており、8B が 3 つで 16B に収まらない。また頭に `align` 欄がないので、`free` 時に `mmap` の長さを計算し直せない。

- `align` は 2 の冪かつ 1 以上 4096 以下に限り、違えば `exit_group(70)` で停止する（A-4 の上限 4096 の具体。以下の `ret` = `base` + `align` と `ret`−32 ≥ `base` はこの前提による）。
- 頭は全ブロックで 16B のまま（`ret`−16 に `magic|class`、`ret`−8 に `size`）。返却アドレス `ret` の規則（小ブロックと align ≤ 16 の大ブロックは block+16、align > 16 は `round_up(base + 16, align)`）は A-4 のまま。
- align > 16 の大ブロックに限り、`ret`−32〜`ret`−16 の 16B に `{base, map_len}` を置く。`base` はページ境界で `align` は 4096 を割り切るので `ret` = `base` + `align` となり、`align` ≥ 32 なら `ret`−32 ≥ `base` で領域内に収まる。
- `mmap` の長さは次のとおりとし、確保時に一度だけ計算する。
  - align ≤ 16 の大ブロック: `map_len` = `round_up(16 + size, 4096)`、`base` = `ret`−16。
  - align > 16: `map_len` = `round_up(align + 16 + size, 4096)`。
- `magic|class` の `class` には、小ブロックのサイズクラスとは別に「align ≤ 16 の大ブロック」と「align > 16 の大ブロック」をそれぞれ別のクラスとして割り当てる。`free` は頭の `magic` と `class`・`size` を検査したうえで、`class` によって `{base, map_len}` の有無を分岐する（align の引数では分岐しない。ネイティブは解放時の align を範囲と 2 の冪であることだけ検査し、確保時の align とは突き合わせないため）。
- align > 16 の大ブロックの `free` は、`munmap` の前に次をすべて検査し、1 つでも違えば `exit_group(70)` で停止する: `base` が 4096 の倍数、`ret`−`base` が 32 以上 4096 以下、`map_len` が `round_up(d + 16 + size, 4096)` と等しい（`d` = `ret`−`base`。正しい塊では `d` = `align` なので上の式と一致し、4096 の倍数かつ 0 より大きい）。
- `free` は記録した `map_len` でそのまま `munmap` する（計算し直さない）。align ≤ 16 の大ブロックは `size` から上の式で求める（頭の `size` と `magic|class` の検査を先に通す）。これにより評価器の「要求長を 4096 に切り上げた長さと完全一致」（A-3）を満たす。

#### B-2. ネイティブでの差し替えの仕方（A-2 の細目）

- runtime/core の Rust 製 `raw.alloc`/`raw.free` は `sigil_rt_raw_alloc`/`sigil_rt_raw_free` の名前から外す（PR 3-7 まで、B-4 の場合の供給元として別名で残す）。
- 確保器を結ぶときだけ、codegen が C 呼び出し規約のシム `sigil_rt_raw_alloc`/`sigil_rt_raw_free` を公開し、Sigil 製の関数を呼ぶ。生成した Sigil 関数の戻り値（値と状態）のうち状態は捨てる。確保器の中の停止は A-4 のとおり `exit_group(70)` で直接止めるため。（補足 F-10 の 1 で改める: 実装のシム（`Fx::alloc_shim`）は状態を確かめ、0 でなければ停止の報告を出して `exit_group(70)` で止める。確保器・文字列・一覧などすべてのシムで同じ。）
- 生成コードの呼び出し先の名前は変えない（A-2 のとおり）。

#### B-3. 結ぶ範囲と `sigil spec`（A-6 の細目）

- `std.os.alloc` を暗黙に結ぶ（`embedded_roots`）のは、`raw.alloc`/`raw.free` を使うプログラムだけとする。使わないプログラムの検査・出力・`sigil spec` は変わらない。
- 評価器で走らせるときも同じ条件で結ぶ（検査の結果を出力先で変えないため）。ただし評価器の `raw.alloc`/`raw.free` は組み込みのまま（A-2）で、確保器の関数は呼ばない。
- `sigil spec` は、結んだときだけ `std.os.alloc` を信頼基盤のモジュールとして出す（A-1 の大域と「触る大域:」の表示を含む）。利用者に見える出力の変更なので、3-4i の PR で所有者の承認を得る。

#### B-4. 出力先（targets）との整合（A-6 の細目）

- 確保器は `raw.syscall`（OS 層）を使うので `targets server` とする。入口の `targets` が `server` だけのときに結ぶ。
- 入口の `targets` に `bare` や `browser` を含むときは結ばず、ネイティブの `raw.alloc`/`raw.free` は PR 3-7 まで Rust の確保器（B-2 で別名にしたもの）を使う。これは A-6 の「確保器の供給元は出力先による」の P1-R での具体で、本文決定 3 の「Rust アロケータに依存しない」との差として PR 3-7 で見直す。

#### B-5. `SIGIL_RT_LIVE` の `sigil_blocks=`（A-4 の細目）

- 確保器を結ぶときだけ、生成コードは生存数を返す関数 `sigil_rt_sigil_live_blocks` を公開する。
- Rust ランタイムは起動時にその関数ポインタの登録を受け、終了時に登録があれば既存の行の末尾に ` sigil_blocks=<数>` を足す。登録がなければ `sigil_blocks=` を出さない（既存の `strs=` / `blocks=` の書式は変えない）。

#### B-6. `mmap` の `prot` とガードページ（A-3 との差の記録）

- 評価器の許可表（3-4b）は `prot` を `PROT_READ|PROT_WRITE` との完全一致で許す。A-3 の「`PROT_EXEC` を含まない」より狭いが、こちらを正とする（`PROT_NONE` 等も拒否）。
- `PROT_NONE` のガードページは採らない（決定 3.1・A-4 にもない）。必要になれば許可表の変更として ADR 0018 判断 2 のレビューを経る。

### 補足 C（PR 3-7 の前）: 3-5〜3-7 の細目と分割し直し

- **日付**: 2026-11-02
- **状態**: 所有者承認済み（「推奨で」「自動的に進めて」、2026-11-02）。C-2（`sigil spec --std`）・C-3（`Float.to_bits` を通常の関数にする）・C-5（`live: sigil_blocks=` への一本化）・C-4 の aarch64（未対応で止める）は、起草者の推奨案について所有者が「自動的に進めて」と答え、推奨案のまま承認された。
- PR 3-7 の設計のための調査（main `cdee378`）で、PR 3-5・3-6 が未完了だと分かった。そのため、残りの置き換えの方式と分割を決める。本文・補足 A・B と食い違う箇所は、本補足が置き換える。

#### C-0. 現状の認定

- PR 3-5（#151）と PR 3-6（#153）が足したのは、構文として読めるだけの Sigil ソース（`src/stdlib/core_sigil.rs`・`src/stdlib/os_sigil.rs`）である。どのプログラムにも結ばれておらず、生成コードは 1 つも Sigil 版に切り替わっていない。両 PR は**未完了**と扱い、C-7 の 3-5a〜3-5g・3-6a〜3-6c で置き換える。`core_sigil.rs` の `String.len` はバイト数を返すが、言語の `String.len` は Unicode のスカラー値の数（`src/shared/text.rs` の `char_len`）で、意味が違う。置き換えでは言語の意味に合わせる。
- PR 3-3 の完了基準のうち、`_start` の生成はまだない。ネイティブの `raw.syscall` は、Rust ランタイムの `sigil_rt_raw_syscall`（`asm!`。x86_64 以外では −1 を返す）を呼んでいる。C-4 で処理系が生成するコードに替える。
- 生成コードは、暫定ランタイムの約 60 種の `sigil_rt_*`（`src/codegen.rs` の `rt_spec`）と、glibc の `memmove`・`memset`・`fmod`・`main` の呼び出し規約・スレッド・標準入出力に依存している。
- ADR 0016 の「段階 3 完了時の見直し記録」（#155）は事実と合わない先取りだったので、撤回した。見直しの記録は 3-7e で書き直す。

#### C-1. 型の記述子を読む汎用の処理（`val_show`・`val_eq`・`val_cmp`・`val_drop`・`val_clone`・`val_shadow`・`blk_new`・`shared_*`）

- Sigil の標準ライブラリには書かない。codegen が**型ごとの関数として生成する**（ADR 0005 の「処理系が生成するコード」として信頼基盤に入る）。
  - 理由: 記述子を実行時に読む処理を Sigil で書くと、`raw.load_*` を多く使う `unchecked` の関数が必要になり、前提の文が「記述子が正しい」という処理系側の不変条件に頼ることになる。型ごとに生成すれば、記述子を読む必要がなく、生成器の試験で確かめられる。
- 塊の配置（頭 16B、参照数、解放済みの印 `FREED`）と `--trace-drops` の出力は、今の Rust 版と同じにする。確保と解放は、`sigil_rt_raw_alloc`/`sigil_rt_raw_free`（Sigil 製確保器のシム）を呼ぶ。
- `String`・`List`・`Map`・`Bytes` の操作（連結・切り出し・追加など）は、本文決定 4 のとおり Sigil の標準ライブラリ（埋め込み `std.core.*`）で書く。
- **Sigil ソースの置き場所**（3-5b 以降）: 確保器（`src/stdlib/os_alloc.sigil`）と同じく、`src/stdlib/` の下に `.sigil` のファイルとして置く（例: `core_string.sigil`・`os_fs.sigil`）。`include_str!` で `src/stdlib/embedded.rs` の `EMBEDDED` に登録する。Rust の文字列に Sigil を埋め込む形（`core_sigil.rs`・`os_sigil.rs`）は使わない。下書きの `core_sigil.rs`・`os_sigil.rs` とその構文試験は、同じ機能を置き換える PR（3-5b・3-5c・3-6a・3-6b）で消す。
- **信頼基盤の差分の検出**: `scripts/trust-base-diff.sh` は今も `tests/`・`target/` を除くすべての `.rs`・`.sigil` を走査する。そのため、埋め込みの `.sigil` の `unchecked` の関数の追加・削除・本体の変更・前提の文の変更は検出の対象になる（ADR 0018 判断 2 の S2）。C-1 の生成コードは Sigil の `unchecked` の関数ではないので、この検出には掛からない。生成器（`src/codegen/**`）の変更は、通常のレビューで見る。

#### C-2. B-4 の解消と `sigil spec` の表示

- ネイティブ出力では、入口の `targets` によらず確保器 `std.os.alloc` を結ぶ（B-4 を置き換える）。出力先は常に Linux（ADR 0015）である。
- **層と出力先の検査（ADR 0014）との境界**:
  - `raw.alloc`/`raw.free` は核の層の原始操作である。`bare`・`browser` を含むモジュールからも呼べる（ADR 0014 決定 2、A-6）。
  - その実体（確保器）を何が与えるかは出力先のプラットフォーム層が決め、利用者のモジュールの検査の対象ではない（ADR 0018 決定 6）。ネイティブ出力では、それが `std.os.alloc` になる。
  - 確保器は、利用者のモジュールが `use` で取り込むものではなく、処理系が暗黙に結ぶ（A-6）。そのため、「`bare` のモジュールから OS の層の項目を呼ばない」（`OsItemInCore`）や `use` の出力先の包含の規則は、確保器を結ぶことに掛からない。
  - 確保器の `targets server` と、その中の `raw.syscall` の検査（OS の層）は、確保器自身のモジュールとして通常どおり行う。
  - 利用者の `bare` のモジュールが OS の層の項目を直接呼べないことは変わらない。将来 `browser` などへの出力を作るときは、その出力先の確保器を同じ仕組みで結ぶ。
- 3-5 以降は、`String`・`List` なども `raw.alloc` を使う。そのため、結ぶ条件（B-3）は「`raw.alloc`/`raw.free` を使うプログラム」から「すべてのプログラム」に変わる。評価器でも同じ条件で結ぶ。評価器の `raw.alloc`/`raw.free` は A-2 のとおり組み込みのままである。
- **混在させない**: Sigil の標準ライブラリで確保する PR（3-5b・3-5c・3-5d・3-6a・3-6c 以降）は、3-7a（確保器の一本化）の後に置く。3-7a より前に置くと、入口の `targets` が `server` だけでないプログラムでは、Sigil の `String` などが Rust の確保器（`raw_alloc_rust`）から確保することになり、出力先によって確保器が変わる組み合わせの試験が増える。そのため、この段階は作らない。
- `sigil spec` は、利用者のモジュールだけを表示する。埋め込みの `std.*`（確保器を含む）は出さない（B-3 の「結んだときだけ出す」を置き換える）。埋め込みの `std.*` の仕様は、`sigil spec --std` で別に表示する。
  - 理由: 埋め込みの `std.*` は、処理系の版が同じならどのプログラムでも同じである。人間は、変わったときに 1 度だけ読めばよい。変化は CI の信頼基盤の差分の検出（`scripts/trust-base-diff.sh`、ADR 0018 判断 1・2）が `.sigil` の `unchecked` の単位で示す。プログラムごとに同じ行を並べると、利用者の仕様が読みにくくなる。
  - 利用者に見える出力の変更なので、実装の PR（3-7a）で `sigil spec` の差分を貼り、所有者の承認を得る。

#### C-3. `Float` のビット列と、libc の関数の置き換え

- `Float` の表示（最短で往復できる表現）・入口の引数の解析・`fmod` を Sigil で書くために、`Float` のビット列を出し入れする関数を、**原始操作ではない通常の標準ライブラリの関数**として核の層に置く（原始操作として `RAW_OPS` に置く案は採らない）: `Float.to_bits(x: Float) -> Int`・`Float.from_bits(n: Int) -> Float`（効果 `none`、IEEE 754 binary64 のビット列を `Int` の 64 ビットとしてそのまま読み替える）。
  - 理由: ビット列の読み替えは、どの値に対しても安全で、前提の文が要らない。`RAW_OPS` に置くと、使う関数がすべて `unchecked` になり、検査を外す範囲が無用に広がる（ADR 0016 の評価の観点 2）。
  - 評価器は `f64::to_bits`/`f64::from_bits`、ネイティブは Cranelift の `bitcast` で実装する。標準ライブラリの表に関数を足すのは言語の仕様の変更なので、実装の PR（3-5a）は所有者の承認を要する。
- `memmove`・`memset`（`raw.copy`/`raw.set` が呼ぶ）は、codegen が自分のオブジェクトの中に関数として生成する（バイト単位のループ。`memmove` は重なりの向きで分岐する）。
- `Float.to_bits`/`from_bits` を使う Sigil の関数 `fmod` を、`std.core.float`（`src/stdlib/core_float.sigil`）に書いて埋め込みの `std` に登録する（musl の `fmod` と同じ方式で厳密に計算する。`unchecked` は要らない）。`Float` の `%` 演算子は変えない（ADR 0011 の決定のとおり、`%` は `Float` に使えず検査器が拒否する）ので、`fmod` はどのプログラムにも暗黙には結ばず、登録のみとする（現実装 #228）。評価器の Rust の `%`（`f64`）との突き合わせの試験で値を確かめる。`src/stdlib/core_float.sigil` の 5 行目のコメント（「`Float` の `%` の実体」）は、この文言に合わせて 3-5b-ii 以降の PR で直す（この PR ではコードを変えない）。

#### C-4. `_start`・システムコールの入口の生成方式と aarch64

- Cranelift にはシステムコールの命令がない。そのため、`_start`・`sigil_syscall6`（6 引数のシステムコール）・スタックの切り替えの 3 つは、x86_64 の機械語のバイト列として `ObjectModule::define_function_bytes` で定義する（ADR 0018 決定 6 の「処理系が機械語を生成する」）。バイト列は短く保ち、残りの起動処理（`argc`・`argv`・`envp` の取り出し、スタックの確保、入口の呼び出し、`exit_group`）は Cranelift で生成する。
- `_start` の手順は本文決定 5.1 のとおりで、手順 4 は A-1 のとおり何もしない。
- **`envp` の受け渡し**: 新しい原始操作は置かず、大域で渡す（`RAW_OPS` は変えない）。
  - 埋め込みの `std.os.env`（3-6c）に `global mut ENVP: Addr = raw.addr_null()` を宣言する。起動の処理（生成コード）は、入口を呼ぶ前に、その大域の静的データ（`Program.globals` の該当の添字。3-4g のネイティブの大域）へ `envp` を書く。
  - Sigil の側は、`unchecked` の関数で `ENVP` を読み、`raw.load_addr`・`raw.load_u8` で `NAME=VALUE` の並びを辿る。前提の文には「`ENVP` は起動の処理が書いた、NULL で終わる `envp` の配列を指す。起動の後は書き換えない」と書く。
  - 生成コードは、その大域を名前（`std.os.env.ENVP`）で探す。`std.os.env` を結ばないプログラムでは書かない。
  - 評価器では、`env` の関数は Rust の組み込みのまま（C-7 の最後の項）で、`ENVP` は使わない。
  - `SIGIL_RT_LIVE`（C-5）の判定は、生成コードが `envp` を直接見て行う。
- ネイティブの `raw.syscall` と、生成コードの停止の経路（`exit_group(70)`）は `sigil_syscall6` を呼ぶ。
- バイト列は信頼基盤の一部である。PR では逆アセンブル（`objdump -d`）の結果を貼り、試験では、出力した実行ファイルの `_start` の先頭のバイト列と、システムコールの結果（`write`・`exit_group`）を確かめる。
- **aarch64**: ネイティブ出力は x86_64 の Linux に限る。aarch64 のホストでの `sigil build` は、未対応として `BuildError::Toolchain` で止める（今も `raw.syscall` は aarch64 では −1 を返すだけで、確保器が動かない）。aarch64 への対応は、必要になったときに別の ADR で決める。

#### C-5. `SIGIL_RT_LIVE` の書式

- Rust ランタイムの数（`strs=`・`blocks=`）は、ランタイムの削除で無くなる。最終の出力は `live: sigil_blocks=<数>`（Sigil 製確保器の生存数。B-5）の 1 項目にする。
- 出す主体と時期:
3-7a と 3-7b はどちらが先に合流してもよい。下の 2 つの規則は互いに独立で、どちらの順でも成り立つ。

- **出す主体**: 3-7b より前は、今のとおり Rust の `sigil_rt_enter_with` が終了時に出す。3-7b からは `sigil_rt_enter` を使わないので、環境変数の判定と終了時の出力を生成コード（起動の処理。`envp` は C-4）が行う。`strs=`・`blocks=` の数は、残っている Rust の関数（`sigil_rt_live_strs`・`sigil_rt_live_blocks`）を呼んで得る。主体が移っても書式は変えない。
- **`sigil_blocks=` を出す条件**: どちらの主体でも B-5 のとおり、確保器を結んだときだけ出す（書式は `live: strs=<数> blocks=<数> sigil_blocks=<数>`）。3-7a より前は、確保器を結ぶプログラムだけで出る。3-7a からは、すべてのプログラムが確保器を結ぶ（C-2）ので、常に出る。
- 3-5e（3-7a と 3-7b の両方の後）で、Rust の確保の経路を消す。それに合わせて、`strs=`・`blocks=` を外し、`live: sigil_blocks=<数>` にする。（補足 G-7 で改める: 書式を変えるのは 3-5e-iii で、3-7b とはどちらが先に合流してもよい。後から合流する方が新しい書式と出す主体を持っていく。）
- `tests/m5_free_strings.rs` の期待は、3-5e で「`sigil_blocks=` が繰り返しの回数に比例して増えない」に改める。（補足 D で置換: 期待の改めは 3-5b-i に前倒しし、3-5b-i から `strs=` は常に 0。D-5）

#### C-6. 256MiB のスタック

- スレッドは作らない。起動の処理（C-4）で、`mmap(0, 256MiB + 4096, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS|MAP_NORESERVE, -1, 0)` で領域を取る。続いて、その下端の 4096B を `mprotect(base, 4096, PROT_NONE)` でガードページにする。スタックポインタは上端（16B 境界）に切り替えて、入口を呼ぶ。
  - ガードページの理由: 今の Rust の `std::thread` のスタックには、ガードページがある。これをなくすと、下限の検査をすり抜ける溢れ（検査のない生成コードの枠や、下限と端の間を一度に越える大きな枠）が、隣の領域を黙って壊しうる。この後退は避ける。
- 生成した関数の入口が比べる下限（`stack_limit`）は、今と同じく上端から 3/4（192MiB）下に置き、溢れを止める主な手段とする。ガードページは、その検査をすり抜けた場合の最後の止め（SIGSEGV）である。下限の値は大域の静的データに置く（単一スレッド前提。A-5）。
- `mmap` の戻り値が −4095〜−1 なら失敗とし、`exit_group(70)` で止める（A-4 の確保器の `mmap` の失敗と同じ扱い）。`mprotect` の戻り値が負のときも、同じく `exit_group(70)` で止める。この領域は解放しない（プロセスの終了で返る）。
- **B-6・A-3 との整合**:
  - この `mmap`・`mprotect` は起動の処理（生成コード）が `sigil_syscall6` で直接発行するもので、`raw.syscall` を通らない。そのため、評価器の `raw.syscall` の許可表（A-3・B-6）は変えない。許可表に `mprotect` は足さない。`mmap` の `prot` を `PROT_READ|PROT_WRITE` だけに限る B-6 も、Sigil のコードについての規則のままである。
  - B-6 の「`PROT_NONE` のガードページは採らない」は、確保器（`std.os.alloc`）の塊についての決定で、このスタックには掛からない。
- 3-7b の完了基準に、深い再帰の試験を入れる。
  - 下限の検査が働く深さで、今と同じ停止の報告と終了コードになる。
  - 1 つの枠の大きい関数の深い再帰でも、黙って壊れずに停止する（下限の検査か SIGSEGV）。
  - 試験は、今の Rust のスレッドのスタックでの結果と同じ期待を使う。

#### C-7. 分割し直し（PR 3-5 〜 3-7）

（補足 D で細目: 3-5b を 3-5b-i・3-5b-ii に分け、3-5c〜3-5e・3-6a・3-6c の依存を改めた表を D-6 に置く。）

各 PR の完了基準は、評価器（Rust の組み込み）とネイティブ（Sigil 版）の突き合わせが全件一致すること、`scripts/native-report.sh` を通ること（突き合わせ 400 件以上、飛ばし 10 件以下）、そして切り替えた `sigil_rt_*` が `rt_spec` から消えることとする。埋め込みの `std.*` に `unchecked` の関数を足す PR は、CI の信頼基盤の差分の検出（ADR 0018 判断 2 の S2）の対象になり、所有者のレビューを要する。

| PR | 範囲 | 依存 |
|---|---|---|
| 3-5a | `Float.to_bits`/`from_bits`（要承認）。`memmove`・`memset` の生成、`fmod` の Sigil 版（C-3） | 補足 C |
| 3-5b | `String`（`str_*`。`len` は Unicode のスカラー値の数）（補足 D で置換: 3-5b-i・3-5b-ii に分割。D-6） | 3-5a・3-7a |
| 3-5c | `List`（`list_*`・`list_sum_*`・`list_best`）（補足 E で置換: 3-5c-i・3-5c-ii・3-5c-iii に分割。`list_best` は Sigil へ移さず codegen のループにする。E-3・E-6） | 3-5b・3-7a（D-6 で置換） |
| 3-5d | `Bytes`・`int_rot*` | 3-5b・3-7a（D-6 で置換。補足 F で置換: 3-5d-0・3-5d-i・3-5d-ii・3-5d-iii に分割。F-9） |
| 3-5e | 型の記述子の処理の生成（C-1）・`Shared`/`Cell`・`--trace-drops`。`SIGIL_RT_LIVE` の `strs=`/`blocks=` を外す（C-5）（補足 G で置換: 3-5e-i・3-5e-ii・3-5e-iii に分割。G-9） | 3-5c・3-5d・3-7a・3-7b（D-6 で置換） |
| 3-5f | 表示（`val_show`・`print_result`・`Float` の最短表現）（D-6 で置換: `show_*` を加える） | 3-5e（補足 G-9 で置換: 3-5e-ii） |
| 3-5g | 入口の引数（`arg_*`・`entry_args`・`Float` の解析）と停止の報告（`stop`/`report`）（D-6 で置換: `new_lossy` を加える） | 3-5f |
| 3-6a | `console.log`。`write` は全部を書き終えるまで繰り返す（部分書き込みは残りから続け、−4（EINTR）は同じ位置から再試行し、その他の負の値は書くのをやめる。今の Rust 版と同じく失敗を呼び出し側へ返さない）。末尾の改行を書く。`os_buf_free` を廃止 | 3-5b・3-7a（D-6 で置換） |
| 3-6b | `fs.read`/`fs.write` と `Path`。詳細は下の表の後の項（D-6 で置換: `valid_utf8` を加える） | 3-6a |
| 3-6c | `env`（C-4 の大域 `std.os.env.ENVP` を読む。`RAW_OPS` は変えない）（D-6 で置換: UTF-8 の扱いを `new_lossy` に確定） | 3-5b・3-7a・3-7b（D-6 で置換） |
| 3-7a | 確保器を一本化する（C-2。`raw_alloc_rust` の経路を消す）。`sigil spec` から埋め込みの `std.*` を外し、`sigil spec --std` を実装する（C-2。`sigil spec` の変更は要承認） | 補足 C |
| 3-7b | `_start`・`sigil_syscall6`・`mmap` のスタックとガードページの生成（C-4・C-6）、`SIGIL_RT_LIVE` の出力を生成コードへ移す（C-5）。この時点では glibc をリンクしたまま `-nostartfiles` で、`main`・`sigil_rt_enter`・`sigil_rt_raw_syscall` を使わない。完了基準に C-6 の深い再帰の試験を含める（**要承認**: 信頼基盤に入る機械語のバイト列と、起動の経路の変更） | 補足 C |
| 3-7c | `cc -nostdlib -static -no-pie` に移行する（ADR 0018 判断 7 の実施。要承認）。出力に glibc のシンボルがなく、INTERP もないことを試験する | 3-5g・3-6b・3-6c・3-7b |
| 3-7d | `runtime/`・`build.rs`・`src/runtime_lib.rs`・`tests/runtime_parity.rs` を削除する（`src/lib.rs` の行の削除は「追加のみ」の例外で、要承認）。CI の扱いは下の項 | 3-7c・CI の PR (1) |
| 3-7e | ADR の記録。完了基準は下の項 | 3-7d |

- **3-6b の細目**:
  - `fs.read`: `openat(AT_FDCWD, path, O_RDONLY|O_CLOEXEC)` で開く。`read` を 0（終わり）が返るまで繰り返し、領域は `raw.alloc` で広げる。−4（EINTR）は再試行する。読み終えたら UTF-8 を検証し、不正なら Other にする。
  - `fs.write`: `openat(AT_FDCWD, path, O_WRONLY|O_CREAT|O_TRUNC|O_CLOEXEC, 0o666)` で開く。`write` は全部を書き終えるまで繰り返す。部分書き込みは残りから続け、−4（EINTR）は同じ位置から再試行する。0 が返ったときは Other にする。その他の負の値は、そこでやめて errno から分類する。
  - 両方とも、`openat` の −4 も再試行する。`close` は必ず呼び、その失敗は無視する（Rust の `std::fs` と同じ）。
  - errno から `IoError` への分類: ENOENT→NotFound、EACCES・EPERM→PermissionDenied、それ以外→Other（`src/shared/path.rs` の `io_arm` と同じ結果になるように）。
  - `Path` の操作は `src/shared/path.rs` と同じ意味の純粋な関数として書く。
- **CI の扱い**（`.github/workflows/**` は人間だけが変える）: 該当する PR の作者は、変更の差分案を Issue に書き、人間が別の PR として合流させる。
  - CI の PR (1)（3-7d の前）: `runtime/core`・`runtime/os` の手順とキャッシュの `runtime/*` を外す。`if: hashFiles('runtime/core/Cargo.toml') != ''` で条件付きにしてもよい。外さないと、3-7d で CI が必ず失敗する。
  - CI の PR (2)（3-7d の後）: 「ネイティブ出力の前提」の手順から `libc6-dev` を外す。`gcc` はリンカの起動役（`cc -nostdlib`）として残す。`tests/runtime_parity.rs` が消えるまでは C のドライバを glibc と結ぶので、3-7c の時点ではまだ外せない。外した後の CI が通ることで、`libc.a` に依存しないことを確かめる。
- **3-7e の完了基準**:
  - ADR 0016 の「段階 3 完了時の見直し記録」を、実装の結果で書き直す。見直しの 3 条件のそれぞれについて、当たったかどうかと根拠（該当の PR・関数）を書く。ADR 0016 の「次を計測する」の数（`unchecked` の関数の数と原始操作の呼び出しの数）を、合流した main の版を添えて書く。
  - ADR 0006 の暫定ランタイムの期限付きの例外と、ADR 0015 決定 6（暫定ランタイムと `cc` + glibc の静的リンク）に、解消したことと本補足への参照を書く。
  - 本 ADR の状態を書く。
  - `README.md` のステータスの行を直す。
  - `docs/` の中で、`runtime/` を今もあるものとして書いた記述が、履歴の記録を除いて残っていない。
- 3-5a・3-7a・3-7b は並行してよく、3-7a と 3-7b はどちらが先に合流してもよい（C-5）。Sigil の標準ライブラリで確保する 3-5b・3-5c・3-5d・3-6a・3-6c は、3-7a の後にする（C-2 の「混在させない」）。3-6c は `String` を返すので 3-5b の後にもする。3-6c と 3-5e は 3-7b の後にする（`envp` と `SIGIL_RT_LIVE` の出力の主体。C-4・C-5）。3-7b は glibc を残したまま入口だけを替えるので、単独で戻せる。
- 評価器の標準ライブラリの組み込み（Rust）は、3-7 の後も残す。ADR 0018 決定 6 の「替えた後は評価器も Sigil 版を実行し、組み込みを消す」は、段階 3 の範囲の外とし、時期は 3-7e で記録する。

### 補足 D（PR 3-5b の前）: `String` の Sigil 側への橋渡し

- **日付**: 2026-11-02
- **状態**: 提案（所有者の確認待ち）。本文の中で D-8 の選択に当たる箇所には「（**仮置き: D-8 の n**）」の印を付け、起草者の推奨案で書いた。確認が取れたら印を外す。印のない箇所は、推奨案の帰結として書いた細目である。本文・補足 A〜C と食い違う箇所は、本補足が置き換える。
- 3-5b（`String`・`str_*`、`len` は Unicode のスカラー値の数）の実装の着手時（main `ca2d970`）に、次の 2 点で止まった。その方式と分割を決める。
  1. Sigil のソースから `String` のバイト列に触れる手段がない。IR の `Ty::Str` は不透明で、`RAW_OPS` は `Addr`・`Int` しか扱わない。本文の決定 4 は `{ptr, len, cap}` の表現を定めるが、今の IR の `String` はその構造体ではなく、`Builtin::Str*` が Rust ランタイムの `sigil_rt_str_*` を呼ぶ。
  2. 表現を替えると、`RtStr(String)` を前提にした Rust ランタイムの消費者（`val_show`・`val_eq`・`val_cmp`・`val_clone`・`val_drop`・`list_free_strs`・`show_*`・`print_result`・`trace_drop`・`arg_str`/`arg_name`・`bytes_to_string_utf8`/`str_as_bytes`・`bytes_*` の誤りの文字列・`str_lines`）と、`sigil_rt_str_new`/`_data`/`_bytes` を呼ぶ生成コード（`src/codegen/func/call.rs` の `PathNew`・`PathJoin`・`PathFileName`・`PathParent`・`ConsoleLog`・`FsRead`・`FsWrite` など。`runtime/os` は `(ptr, len)` の領域を返すだけで `RtStr` を知らない）を同時に直す必要がある。あわせて、`runtime/core` の単体の `cargo test` と `tests/runtime_parity.rs` のリンク、`LIVE_STRS`（`strs=`）、C-5、`tests/m5_free_strings.rs` の期待に影響する。

#### D-1. 方式の比較と採用

| 案 | 内容 | 契約変更 | 長所 | 短所 |
|---|---|---|---|---|
| (a) | `raw.str_ptr`・`raw.str_len`・`raw.str_from_raw` などを `RAW_OPS` と `Builtin` に足し、Sigil の関数が `String` を直接受け渡す | `src/ir.rs`（`Builtin`）と `RAW_OPS`（ADR 0016 決定 1・ADR 0018 判断 2 のレビュー） | Sigil の関数の引数が `String` のままで、所有権の検査が効く | 評価器にも `raw.str_*` が要り、評価器の `String`（Rust の値）を塊の表に載せる仕組みが要る。`str_from_raw` は所有を移す原始操作で、前提の文が重い。手続きが最も長い |
| (b1) | codegen が `String` の頭 `{ptr, len, cap}` を読み書きするグルーを生成し、Sigil の関数には `(ptr, len)` などの `Addr`・`Int` だけを渡す。新しい文字列の頭も codegen が作る | なし | IR・`RAW_OPS` を変えない | 頭の配置を知る場所が codegen・Sigil・Rust ランタイムの 3 つになる。戻り値が文字列の関数（`trim`・`concat`）は、Sigil から複数の値を返す形の取り決めが要る |
| **(b2)** | 頭の配置と確保・解放を、埋め込みの Sigil モジュール `std.core.string` だけが持つ。`String` の語は頭の番地（`Addr`）そのもので、codegen は語の読み替えと C の呼び出し規約のシムだけを生成する。Rust ランタイムは登録された関数表を通して文字列を作る・読む・捨てる | なし | 配置を知るのは Sigil の 1 か所。IR・`RAW_OPS`・評価器を変えない。codegen の呼び出し箇所（`rt("str_new")` など）は変わらない（補足 B-2 の確保器のシムと同じ形） | Rust の消費者は関数表の間接呼び出しになる（3-7d で消えるまでの一時的な形） |

- **採用: (b2)**（**仮置き: D-8 の 1**）。理由:
  - 契約の変更がなく、補足 B-2（確保器のシム）・B-5（関数ポインタの登録）と同じ仕組みの組み合わせで済む。
  - Sigil の関数は `Addr`・`Int` だけを受け渡すので、評価器の上で直接呼んで、塊の表で確保と解放を確かめられる（補足 A-2 の「影の表」付き試験と同じ）。
  - 頭の配置を知るのが Sigil の 1 か所になり、`unchecked` の前提の文と CI の信頼基盤の差分の検出（ADR 0018 判断 2 の S2）の対象に集まる。
- (a) は採らない。評価器の `String` は Rust の組み込みのまま残す（補足 C-7 の最後の項）ので、`raw.str_*` を評価器で意味のある形にするのが難しい。
- 本文の決定 4 の「`String` を Sigil で実装」は、P1-R では「頭と操作を Sigil の `Addr`・`Int` の関数で実装し、言語の `String` 型は不透明のまま、処理系が語を読み替えて呼ぶ」と具体化する。`String` を Sigil の構造体（`Ty::Adt`）にする置き換えは、P1-R の範囲の外とする。

#### D-2. 表現と UTF-8 の不変条件

- `String`（と `Path`。今と同じく同じ表現を共有する）の語は、24B の頭の塊の番地とする。頭は `ptr: Addr`（+0）・`len: Int`（+8、バイト数）・`cap: Int`（+16、`ptr` の塊の大きさ）。頭は `raw.alloc(24, 8)`、中身は `raw.alloc(cap, 1)` で確保する（本文の決定 4 の `{ptr, len, cap}` を 2 つの塊で持つ。**仮置き: D-8 の 4**）。
- 空の文字列は `ptr` = 空番地・`len` = `cap` = 0 とし、中身の塊を確保しない（`raw.alloc` は `size > 0` を要する。決定 1）。`data` は空番地を返しうるので、読む側はすべて長さ 0 のときに番地を使わない（Rust の `from_raw_parts` は空番地で未定義動作になるので、長さ 0 なら空の `&str` を返す。C の試験のドライバも同じ）。
- 不変条件: `0 <= len <= cap`、`len > 0` なら `ptr..ptr+len` は読める、`ptr..ptr+len` は正しい UTF-8。`String` は今の言語では書き換えないので、作るときは `cap` = `len` とする（`cap` は将来の追記のために持つ）。
- 破棄は、`cap > 0` なら `raw.free(ptr, cap, 1)`、続いて `raw.free(頭, 24, 8)`（本文の決定 4 のデストラクタの具体）。
- **UTF-8 の検証**（**仮置き: D-8 の 5**）:
  - `std.core.string` は、作る関数を 2 つ持つ。`new_lossy(p, n)` は今の `sigil_rt_str_new`（`from_utf8_lossy`）と同じく、不正な列を置換文字 U+FFFD にして作る（正しい列はそのまま複写する）。`new(p, n)` は検証せずに複写し、前提の文に「`p..p+n` は正しい UTF-8」を書く。あわせて `valid_utf8(p, n) -> Int`（補足 F-10 の 3: 実装は `-> Bool`）（正しい UTF-8 なら 1、そうでなければ 0。D-3 の「`Addr`・`Int` だけを受け渡す」に合わせ、シムの戻りを 32 ビットへ切り詰める規則は D-3 の codegen の項にある（`i` の戻りは 32 ビットへ切り詰める））を置く（3-6b の `fs.read` が使う。3-5d の `bytes_to_string_utf8` は補足 F-5 で `std.core.string.from_utf8` を使う形に改めた）。この 3 つは 3-5b-i に入れる。
  - 生成コードと Rust ランタイムが呼ぶ `sigil_rt_str_new`（シムと関数表の `new`）は `new_lossy` を呼ぶ。外から来たバイト列（入口の引数・ファイル・環境変数。環境変数は `new_lossy`）を含め、どの経路から作っても不変条件が成り立つ。検証の手間は今の `from_utf8_lossy` と同じく長さに比例する。
  - 検証なしの `new` を呼ぶのは、出力が構成上正しい UTF-8 になる `std.core.string` 自身の関数（`clone`・`trim`・`concat`、3-5c の `lines`）と、正しさを自分で確かめた埋め込みの `std.*` に限る。呼ぶ関数は `unchecked` で、前提の文に根拠を書く（S2 のレビューの対象）。
  - Rust の側の境界はそのまま残す: 入口の引数は今のとおり `to_string_lossy`、`fs.read` は Rust の検証、`bytes_to_string_utf8` は `String::from_utf8` で、それぞれ 3-5g・3-6b・3-5d で Sigil へ移すときに `new_lossy`・`valid_utf8`・`from_utf8`（3-5d。補足 F-5）へ置き換える。
  - Rust の消費者が中身を `&str` として読むとき（今の `s(p)`）は、`std::str::from_utf8` で検査し、不正なら異常停止（終了コード 70）する。`from_utf8_unchecked` にすると、信頼の義務が `new` を呼ぶすべての箇所に掛かるため。検査の手間は 3-7d で Rust の消費者が無くなるまでの一時的なものとする。
- **利用者からは呼べない**: 利用者の `use std.*` は拒否される（ADR 0016 決定 3、ADR 0021。`src/stdlib/embedded.rs`）ので、`std.core.string` の関数は `pub` であっても利用者のモジュールからは呼べず、利用者が不変条件を破る経路はない。`pub` にするのは、ほかの埋め込みの `std.*`（3-5c 以降・3-6a 以降）が使う関数だけとする。

#### D-3. 接続（codegen・Rust ランタイム・停止）

- **Sigil 側**: 埋め込みのモジュール `std.core.string`（`src/stdlib/core_string.sigil`、`targets bare, browser, server`、`trust base`。C-1 の置き場所）に、`Addr`・`Int` だけを受け渡す関数を置く。バイトや頭を読み書きする関数は `unchecked` で、前提の文に「`s` は `std.core.string` が返し、まだ解放していない頭（したがって `len <= cap`、`len > 0` なら `ptr..ptr+len` は読める正しい UTF-8）」「`p..p+n` は読める」などを書く。
  - 3-5b-i: `new`・`new_lossy`・`valid_utf8`・`free`・`clone`・`data`・`bytes`。
  - 3-5b-ii: `len`（スカラー値の数。続きのバイト `0b10xxxxxx` でないバイトを数える）・`is_empty`・`trim`（Rust の `str::trim` と同じ `White_Space` の符号点の表）・`concat`・`parse_int(s: Addr, out: Addr) -> Int`・`eq`・`cmp`（バイト列の辞書順。Rust の `str::cmp` と同じ）。`eq`・`cmp` は 3-5b-ii で足して試験し、3-5e の生成する `val_eq`・`val_cmp` の文字列の腕が呼ぶ。それまで `val_eq`・`val_cmp` は Rust のまま関数表で読んで比べる。Rust の `sigil_rt_str_cmp`（生成コードからは呼ばれていない）は 3-5e で消す（補足 G-0: main `9cf93f7` の時点で既に無い）。
  - 長さと確保の検査: `new`・`new_lossy` は `n < 0` なら停止する（終了コード 70）。`n` = 0 なら `p` を読まない。`concat` の長さの和は、各長さが確保器の上限（2^46。A-4）未満なので `Int` であふれず、上限を超える確保は確保器が停止する。
  - 確保器（`std.os.alloc`）と同じく、すべてのプログラムに暗黙に結び（`implicit_roots`。C-2）、lowering は入口から辿れなくても SIR に入れる（`src/lower/mod.rs` の確保器の関数の扱いと同じ。`src/ir.rs` は変えない）。評価器でも結ぶが、評価器の `String` の組み込みは Rust のまま（C-7 の最後の項）で、これらの関数は呼ばない。
  - 試験用の差し込み（`Options::raw_ops`）で `raw.alloc`/`raw.free` を置き換えたときは、確保器と同じく `std.core.string` も結ばない（偽の原始操作で文字列を作らないため）。そのときにネイティブ出力を求めたら、codegen は「`std.core.string` が結ばれていない」として `CgError::Internal` で止める（差し込みは検査と評価器の試験だけで使う）。
- **codegen**: `sigil_rt_str_new`（`new_lossy` を呼ぶ）・`_free`・`_clone`・`_data`・`_bytes`（3-5b-ii で `_len`・`_is_empty`・`_trim`・`_concat`・`_parse_int` を加える）を、Sigil の関数を呼ぶ C の呼び出し規約のシムとして生成コードの中で定義し、`rt` に登録する。生成コードの呼び出し先の名前と形（`rt_spec` の形。`i` の戻りは 32 ビットへ切り詰める）は変えない。シムを定義した名前は `rt_spec` から消す（C-7 の完了基準）。
  - シムは `Linkage::Local`（公開しない）で定義する。Rust ランタイムは番地を関数表で受け取るので、シンボルとして公開する必要がない。確保器のシム（B-2）が `Linkage::Export` で公開されるのとはここが違う。
  - シムは、Sigil の関数の戻りの状態が 0 でなければ、停止の報告を出して `exit_group(70)`（終了コード 70）で止める。確保器のシム（B-2）も同じ本体（`Fx::alloc_shim`）で、状態を確かめて止める（補足 F-10 の 1 で改めた。以前の「確保器のシムは状態を捨てる」は実装と食い違っていた）。文字列の関数は算術などの停止を持ちうるので、黙って続けない。
  - `data`・`bytes` は、頭の +0・+8 を codegen が直接読む形にもできる（呼び出しが 1 回減る）。P1-R では採らない。配置を知る場所を Sigil の 1 か所に保つためで、計測で必要になれば、配置を定数として試験で縛ったうえで別に決める。
  - 性能: 複写は `raw.copy`（C-3 の生成した `memmove`。バイト単位のループ）になり、今の libc の `memcpy` より遅い。P1-R では受け入れ、`scripts/native-report.sh` の所要時間が目立って延びた場合は、`memmove` を語単位にする改善を別の PR で行う。
- **Rust ランタイム**: `RtStr` を中身の分からない型（`#[repr(C)] struct RtStr { _p: [u8; 0] }`）にし、Rust の側の `sigil_rt_str_new`・`_free`・`_clone`・`_data`・`_bytes` の定義を消す。文字列を作る・読む・捨てるのは、起動時に登録された関数表を通す（**仮置き: D-8 の 2**）。
  - 生成コードの `main` は、入口を呼ぶ前に `sigil_rt_set_str_ops(new, free, clone, data, bytes)` でシムの番地を登録する（B-5 の `sigil_rt_set_sigil_live` と同じ形）。登録の前に文字列を作ろうとしたら、Rust ランタイムは異常停止する（Rust の `String` に黙って戻らない）。
  - 外部のシンボルとして参照しない理由: `core.a` が生成コードのシンボルに依存しなくなり、`runtime/core` の単体の `cargo test` と `tests/runtime_parity.rs` が、未定義のシンボルなしにリンクできる（試験の側で関数表を登録する。D-7）。
  - Rust の消費者（`new_str(String)`・`s(p) -> &str`）は、関数表の `new`・`data`/`bytes` を呼ぶ形に機械的に替える（`s(p)` の検査は D-2）。Rust の中で一時的に作る `String`（表示の組み立てなど）は glibc の確保器のままでよく、言語の `String` の値になるのは関数表の `new` で複写したものだけとする。
  - 3-7b（`_start`）は、起動の処理で同じ登録を行う。3-5b-i と 3-7b はどちらが先に合流してもよく、後から合流する方が登録を新しい起動の経路へ持っていく。登録と関数表は、Rust の消費者が無くなる 3-7d で消す。

#### D-4. 契約変更の有無と順序

- 採用の (b2) では、`src/ir.rs`（`Builtin`・`Ty`）と `RAW_OPS` を変えない。契約変更の PR は要らず、順序は「本補足 D（ADR）→ 3-5b-i → 3-5b-ii」となる（AGENTS.md の「契約変更 → ADR → 実装」のうち、契約変更の段がない）。
- 仮に (a) を選ぶ場合の順序: `src/ir.rs` に `Builtin::RawStr*` を足す契約変更だけの PR（`contract-change`）→ ADR 0016 決定 1 の手続きと ADR 0018 判断 2 のレビュー（`RAW_OPS` の追加）を本 ADR の補足に記録する PR → 評価器・ネイティブの `raw.str_*` → 3-5b。いずれも所有者の承認を要する。

#### D-5. `strs=` と `LIVE_STRS`（C-5 との整合）と二重解放の検知

- 3-5b-i から、言語の文字列は Sigil 製確保器から確保するので、`sigil_blocks=`（B-5）が文字列の頭と中身を数える。Rust の `LIVE_STRS` は、Sigil が作る文字列（`trim`・`concat` の結果など）を数えられず、Rust の側で減らすと負になって異常停止する。
- そのため 3-5b-i で、`LIVE_STRS` の増減をやめる（`strs=` は常に 0。**仮置き: D-8 の 3**）。`SIGIL_RT_LIVE` の書式（`live: strs=<数> blocks=<数> sigil_blocks=<数>`）は C-5 のとおり 3-5e まで変えず、3-5e で `strs=`・`blocks=` を外して `live: sigil_blocks=<数>` にする（C-5 の時期は変えない）。
- C-5 の最後の項（`tests/m5_free_strings.rs` の期待を「`sigil_blocks=` が繰り返しの回数に比例して増えない」に改める）は、3-5e から 3-5b-i に前倒しする。`strs=` が常に 0 になると、文字列の漏れを見つける試験が働かなくなるため。`tests/fn_types_2_4d_exec.rs` の同じ形の検査も、同じ PR で `sigil_blocks=` を比べるように改める。
- `tests/sigil_alloc_34i.rs` などの `strs=0 blocks=0 sigil_blocks=<数>` の完全一致の期待は、終了時に文字列が残らない限り変わらない見込みである。変わる場合は、実装の PR に理由を書く。
- **二重解放の検知は弱くなる**。今は Rust の `LIVE_STRS` が負になること（`double_free_of_a_string_aborts`）と、Rust の確保器（試験の道具で調べられる）で見つかる。3-5b-i からは確保器の解放済みの印と頭の検査（A-4）だけになり、次を見逃す:
  - スラブの塊が解放の後に別の確保で再利用されてからの二重解放（解放済みの印が上書きされている）。
  - `mmap` の塊への誤った書き込み（ASan・Miri の対象外）。
- この弱さへの手当てとして、3-5b-i の完了基準に次の 2 つを入れる:
  1. **数の完全一致**: `m5_free_strings`・`fn_types_2_4d_exec` の `sigil_blocks=` を、1000 回と 100000 回の繰り返しで完全一致で比べる。二重解放は生存数を減らしすぎるので、漏れと同じくこの比較で見つかる。この比較が実際に働くことを、わざと解放を 1 つ抜いた（または 2 回にした）版で失敗するのを手元で確かめ、PR に結果を書く。
  2. **評価器の塊の表による試験**: `std.core.string` の各関数を評価器で直接呼ぶ試験（D-7）で、確保と解放の対を塊の表で厳密に確かめる（範囲外・二重解放・大きさ違いの解放は評価器が停止する。決定 1）。
- Rust の `double_free_of_a_string_aborts`（`runtime/core`）は、Rust が文字列を解放しなくなるので消す。

#### D-6. 3-5b の分割し直し（C-7 の表の置き換え）

| PR | 範囲 | 依存 |
|---|---|---|
| 補足 D | 本補足（docs のみ） | 補足 C |
| 3-5b-i | 表現の切り替え: `std.core.string` の `new`・`new_lossy`・`valid_utf8`・`free`・`clone`・`data`・`bytes`、codegen のシムと `sigil_rt_set_str_ops` の登録、Rust ランタイムの `RtStr` の不透明化と消費者の関数表への付け替え（`s(p)` の UTF-8 検査）、`LIVE_STRS` の停止と `m5_free_strings` などの期待の前倒し（D-5）、試験用の関数表（D-7）。言語の振る舞いは変えない。完了基準に D-5 の 2 つの手当て（`sigil_blocks=` の完全一致の比較が働くことの確認、評価器の塊の表による試験）を含める（**要承認**: 埋め込みの `std.*` に `unchecked` の関数を足す（S2）、`sigil spec --std` の出力が増える、`SIGIL_RT_LIVE` の `strs=` の意味が変わる） | 補足 D・3-5a・3-7a |
| 3-5b-ii | 操作の移し替え: `len`（スカラー値の数）・`is_empty`・`trim`・`concat`（`+`）・`parse_int` を Sigil へ。`eq`・`cmp` を足す（使うのは 3-5e）。`rt_spec` から `str_len`・`str_is_empty`・`str_trim`・`str_concat`・`str_parse_int` を消す。下書きの `core_sigil.rs` の `String` の部分とその構文試験を消す（C-1）（**要承認**: S2） | 3-5b-i |
| 3-5c | `List`（`list_*`・`list_sum_*`・`list_best`）に加えて `str_lines`（`List<String>` を返すので `List` と一緒に移す。D-8 の 6 は補足 E で案 A に確定）（補足 E で置換: 3-5c-i・3-5c-ii・3-5c-iii に分割。E-6） | 3-5b-i・3-7a |
| 3-5d | `Bytes`・`int_rot*`。`str_as_bytes`・`bytes_to_string_utf8`（補足 F-5 で改める: `valid_utf8` ではなく `std.core.string.from_utf8` を使う）も移す（補足 F で置換: 3-5d-0・3-5d-i・3-5d-ii・3-5d-iii に分割。F-9） | 3-5b-i・3-7a・3-5c-ii（補足 E-6 で追加: `bytes_from_list` が `std.core.list` を読む） |
| 3-5e | 型の記述子の処理の生成（C-1。`val_eq`・`val_cmp`・`val_clone`・`val_drop` の文字列の腕は `std.core.string` の `eq`・`cmp`・`clone`・`free` を呼ぶ）・`Shared`/`Cell`・`--trace-drops`（`trace_drop` を含む）（`trace_drop` は 3-5f。G-3）。Rust の `sigil_rt_str_cmp` を消す。`SIGIL_RT_LIVE` の `strs=`/`blocks=` を外す（C-5。試験の期待の改めは 3-5b-i に前倒し済み）（補足 G で置換: 3-5e-i・3-5e-ii・3-5e-iii に分割。`sigil_rt_str_cmp` は既に無い。`trace_drop` の書き出しは 3-5f へ移す。G-3・G-9） | 3-5b-ii・3-5c（補足 E-6 で置換: 3-5c-iii）・3-5d・3-7a・3-7b（補足 G-9 で置換: 3-7b に依存するのは 3-5e-iii だけで、どちらが先でもよい） |
| 3-5f | 表示（`val_show`・`show_*`・`print_result`・`Float` の最短表現） | 3-5e（補足 G-9 で置換: 3-5e-ii） |
| 3-5g | 入口の引数（`arg_*`・`entry_args`・`Float` の解析。不正な UTF-8 は `new_lossy`）と停止の報告（`stop`/`report`） | 3-5f |
| 3-6a | `console.log`（C-7 のとおり） | 3-5b-i・3-7a |
| 3-6b | `fs.read`/`fs.write` と `Path`（C-7 のとおり。UTF-8 の検証は `valid_utf8`） | 3-6a |
| 3-6c | `env`（C-7 のとおり。UTF-8 の扱いは `new_lossy` に確定する: 今の Rust の `runtime/` に環境変数を読む関数はなく、外から来るバイト列の既存の扱いは入口の引数の `to_string_lossy`（不正な列を U+FFFD にする）なので、それと同じ結果にする。`valid_utf8` は使わない） | 3-5b-i・3-7a・3-7b |

- 3-5b-i は、作る側と読む側を一度に替える必要があるので、それ以上には分けない。3-5b-ii は、関数ごとに分けてもよい（`len`・`is_empty` と `trim`・`concat`・`parse_int`・`eq`・`cmp` の 2 つなど）。
- 3-5c・3-5d・3-6a・3-6c は、文字列を作る・読むことだけが要るので、3-5b-i の後なら 3-5b-ii と並行してよい（3-5c の `lines` が要るのは `new` だけで、`len`・`trim` には依存しない）。3-5e は `eq`・`cmp` を使うので 3-5b-ii の後にする。（補足 E-6 で改める: 3-5d は `bytes_from_list` が `std.core.list` を読むので、3-5c-ii の後にもする。3-5c 自体は 3-5c-i〜iii の 3 つに分けた。）
- 3-5b-i から 3-7d まで、Rust の消費者は関数表を通す。各 PR が消費者を Sigil 版や生成コードへ移すたびに、関数表の利用者は減る。
- 3-7c・3-7d・3-7e の依存は C-7 のとおり変えない。3-7c の依存の 3-5g は、3-5b-ii を含む全体の後になる。

#### D-7. 試験の戦略

- **評価器の上での直接の試験**（新しいファイル `tests/str_35b.rs` など）: `std.core.string` の関数を、`raw.alloc` で用意した入力で評価器から直接呼ぶ（A-2 の「影の表」と同じ）。頭の 3 つの欄、`cap` = `len`、空の文字列の空番地、`free` の後に塊の表が空に戻ること、`n < 0` の停止、多バイト（2〜4 バイト）・空・`White_Space` の各符号点（U+0085・U+00A0・U+2028・U+3000 など）・`parse_int` の境界（`i64` の最小・最大と、その 1 つ外、`-`・`+`・空）・不正な UTF-8 の列（`new_lossy`・`valid_utf8`。途中で切れた列・余分な続きのバイト・過長の表現・サロゲート）を、`src/shared/text.rs` と Rust の `from_utf8_lossy`・`from_utf8` の結果と突き合わせる。
- **評価器とネイティブの突き合わせ**: 3-5b-i は振る舞いを変えないので、既存の全件の突き合わせと `scripts/native-report.sh` がそのまま通ることを基準にする。3-5b-ii は、上の入力の組をネイティブでも走らせる。
- **`runtime/core` の単体の `cargo test`**: `#[cfg(test)]` の試験用の関数表（同じ 24B の頭を Rust の確保で持つ小さな実装）を、試験の始めに一度だけ登録する。生成コードのシンボルに依存しないので、スタブのシンボルは要らない。`LIVE_STRS` と文字列の二重解放の試験は D-5 のとおり消す。
- **`tests/runtime_parity.rs`**: C のドライバ（`DRIVER`）は今 `sigil_rt_str_new`・`_free`・`_data`・`_bytes` を Rust の関数として呼んでいるが、3-5b-i でこれらは Rust から消える。そのため、ドライバの側に同じ名前の C の関数（`malloc` で同じ 24B の頭を持つ。長さ 0 の扱いは D-2）を定義し、`main` の冒頭で `sigil_rt_set_str_ops` に登録する。`core_alone_links_without_os` の `d.c` も、`sigil_rt_show_int` → `sigil_rt_print_result` が関数表の `new`・`data`/`bytes` を通るので、同じ C の関数表の定義と登録を足す（リンクの対象と期待の出力 `42` は変えない）。3-5b-i の時点では `len`・`trim`・`parse_int`・`lines` は Rust の実装のままなので、突き合わせはそのまま意味を持つ。3-5b-ii で移した関数の組は、ドライバでは C の関数表を試すだけになるので、このファイルから外し、`tests/str_35b.rs` の突き合わせへ移す。
- `tests/m5_free_strings.rs` の冒頭のコメントは、`double_free_of_a_string_aborts` を「直接の検査」と呼んでいる。D-5 でその試験を消すので、該当の PR（3-5b-i）でコメントも直す（直接の検査が無くなり、二重解放は D-5 の数の完全一致と確保器の印で見つける、と書く）。
- 既存の試験ファイルの変更（`m5_free_strings.rs`・`fn_types_2_4d_exec.rs`・`runtime_parity.rs`）は、本補足を根拠として該当の PR で行う（AGENTS.md の「既存のテストファイルへの追記」の例外）。

#### D-8. 未決の選択肢（推奨つき。所有者の確認待ち）

本文の「**仮置き: D-8 の n**」の印は、次の n 番の推奨案で書いた箇所を指す。

1. **橋渡しの方式**（D-1）: (b2) Sigil が頭を持ち、codegen は読み替えとシムだけ（**推奨**）／(b1) codegen が頭を読み書きする／(a) `raw.str_*` を `RAW_OPS` に足す（契約変更と ADR 0016 の手続きが要る）。
2. **Rust ランタイムとのつなぎ方**（D-3）: 起動時に関数表を登録する（**推奨**。`core.a` が生成コードのシンボルに依存しない）／生成コードの `sigil_rt_str_*` を外部のシンボルとして参照する（単体の `cargo test` と `runtime_parity` にスタブのシンボルが要る）／Rust が `repr(C)` の頭を読むだけにし、作る・捨てるは関数表を通す（読む側の間接呼び出しは減るが、配置を知る場所が Rust にも増え、3-7d まで 2 か所を揃え続ける）／Rust が `repr(C)` の頭を持ち、`sigil_rt_raw_alloc`/`raw_free` を直接呼んで作る・捨てるも行う（配置を知る場所が 2 つになり、確保器のシンボルのスタブが要る）。
3. **`strs=` の扱い**（D-5）: 3-5b-i で数えるのをやめて常に 0 とし、書式は 3-5e まで変えない（**推奨**。C-5 の時期を守る）／3-5b-i で `strs=` を外す（C-5 の一部を前倒し。完全一致の期待を持つ試験を 2 度直すことになる）。
4. **頭と中身の持ち方**（D-2）: 頭（24B）と中身の 2 つの塊（**推奨**。本文の決定 4 の `{ptr, len, cap}` のまま）／`{len, cap, バイト列…}` の 1 つの塊（確保は 1 回で済むが、決定 4 の改訂が要る）。
5. **不正な UTF-8 の扱い**（D-2）: シムと関数表の `new` は `new_lossy`（今の `from_utf8_lossy` と同じ）とし、検証なしの `new` は `std.core.string` の内部と正しさを確かめた `std.*` だけが `unchecked` で呼ぶ（**推奨**。どの経路から作っても不変条件が成り立つ）／シムも検証なしの `new` とし、呼び出し元が正しい UTF-8 だけを渡すことを一覧で確かめる（検証の手間は省けるが、外から来たバイト列の経路を 1 つ見落とすと不変条件が崩れ、Rust の `s(p)` の検査で止まる）。
6. **`str_lines` の時期**（D-6）（**確定: 案 A**。所有者の判断で、`List` ごと Sigil へ移す案に決めた。分割と細目は補足 E。`str_lines` は 3-5c-iii で移す）: `List` と一緒に 3-5c で移す（**推奨**。案 A）／3-5b-ii で移す。後者では `List` がまだ Rust の `RtList` なので、Sigil の `lines` が結果の一覧を作るために、`list_new`・`list_push` を呼ぶ 2 つ目の関数表（Rust の一覧を Sigil から作るためだけの一時的な橋）が要り、3-5c でその橋を捨てることになる。

### 補足 E（PR 3-5c の前）: `List` の Sigil 側への橋渡しと 3-5c の分割

- **日付**: 2026-11-02
- **状態**: 確定: 案 A（Jev と司令塔の判断。所有者の包括委任に基づく）。D-8 の 6（`str_lines` の時期）は案 A（`List` ごと Sigil へ移し、`str_lines` はその後に移す。2 つ目の一時的な橋を作らない）に確定した。本補足の中の細目の選択は E-9 に推奨案つきで並べ、本文はその推奨案で書いた（異論があれば本補足の修正 PR で直す）。本文・補足 A〜D と食い違う箇所は、本補足が置き換える。
- 3-5c の実装の着手時（main `96687f5`。3-5a・3-7a・3-5b-i（#240）・3-5b-ii（#242）は合流済み、3-7b は未合流）に、`List` が Rust の `RtList` のままで、作る側（codegen）と読む側（Rust の消費者）を同時に替える量が 1 つの PR としては大きく、設計の判断も要るので止まった。D の (b2)（Sigil が頭を持ち、codegen は語の読み替えとシム、Rust は関数表）を `List` に当てはめ、分割を決める。

#### E-0. 現状の確認（main `96687f5`）

- **Rust の表現**（`runtime/core/src/lib.rs`）: `RtList { elem, len, cap, buf }` を `Box` で持ち、`buf` は `sigil_rt_alloc(cap * elem, 16)`（Rust の確保器）で取る。増やし方は 0→4→2 倍。`sigil_rt_list_new(elem_size)`・`_push(l, elem)`（`elem` の指す `elem_size` バイトを末尾へ複写）・`_get(l, i)`（範囲外は null、`elem == 0` なら 16 を返す）・`_len`・`_free`（入れ物だけ。null は何もしない）・`_free_strs`（要素を文字列の関数表の `free` で捨ててから `_free`）。
- **要素の大きさは常に 8B**: codegen が `list_new` に渡すのは `ListNew`・`ListMap` の 2 か所とも定数 8（`src/codegen/func/call.rs`）。`ListPush` も 8B の一時領域に語を置いて番地を渡す。Rust の消費者（`runtime/core/src/val.rs` の `list_items`・`dup`・`coerce`）も、要素を語（`i64`）として読む・`list_new(8)` で作る。`elem == 0` の経路は使われていない。
- **Rust の消費者**（`RtList` を作る・読む・捨てる）: `val_show`・`eq`・`cmp`・`dup`（`val_shadow`・`val_clone`）・`val_drop`（要素を記述子で積んでから `sigil_rt_list_free`）・入口の引数の `coerce`（`List` の引数）・`list_sum_int`・`list_sum_float`・`list_best`（`val.rs`）・`str_lines`・`bytes_from_list`（`(*l).len`・`(*l).buf` を直接読む）（`lib.rs`）。`runtime/os` は `RtList` を使わない。
- **生成コードの呼び出し**（`rt_spec` の `list_new`・`list_push`・`list_get`・`list_len`・`list_sum_int`・`list_sum_float`（戻り `f`）・`list_best`（引数 `ppi`）・`str_lines`）。`list_free`・`list_free_strs` は生成コードから呼ばれない（`List` の破棄は `val_drop` が記述子で行う）。`list_free_strs` を呼ぶのは Rust の単体試験（`lines_list_and_shared`）だけである。
- **生存数**: `RtList` の頭と `buf` は、`blocks=`（`LIVE_BLOCKS`。`blk_new`・`Bytes` だけを数える）にも `sigil_blocks=` にも数えられていない。**今の `List` の漏れは、どの試験でも見つからない**。
- **評価器**: `List` は `Val::Agg`（`Tag::List`）で、`src/ir_eval/call.rs` の組み込みが扱う。`std.*` の関数は呼ばない（C-7 の最後の項）。
- **埋め込みの `std.*` どうしの `use`**: `use` に別名はない（`src/ast.rs` の `UseDecl` は `path` だけ）。同じ名前の関数を 2 つのモジュールから取り込むと衝突する（`std.core.string.new` と、`List` の側に `new` を置いた場合）。
- 下書き `src/stdlib/core_sigil.rs` には `LIST_SIGIL`（構造体 `List<T>`）と `MAP_SIGIL` が残り、`tests/core_sigil_test.rs` が構文だけを試験している。

#### E-1. 表現（D-2 に対応）

- `List<T>` の語は、24B の頭の塊の番地とする。頭は `buf: Addr`（+0）・`len: Int`（+8、要素の数）・`cap: Int`（+16、`buf` の塊に入る要素の数）。頭は `raw.alloc(24, 8)`、要素の領域は `raw.alloc(cap * 8, 8)` で確保する（`String` の頭と同じ 3 欄の配置。D-2）。
- **要素は 8B の語に固定する**（**E-9 の 1**）。E-0 のとおり、要素の大きさは今も常に 8 である。頭に `elem` を持たない。作る関数は引数 `elem` を受け取り（呼び出しの形を変えないため）、8 でなければ停止する（終了コード 70）。
- 要素の領域の境界は 8 とする（**E-9 の 2**）。今の 16 は要素の大きさに由来しない（要素は語）。確保器の小さい塊は 16 の倍数の大きさのクラスから取るので、実際の番地は今と同じく 16 の境界になることが多いが、それには頼らない。
- 空の一覧は `buf` = 空番地・`len` = `cap` = 0 とし、要素の領域を確保しない（`raw.alloc` は `size > 0` を要する）。最初の追加で `cap` = 4、満ちたら 2 倍にする（今と同じ）。新しい領域へ `len * 8` バイトを `raw.copy` で写し、古い領域を `raw.free(buf, cap * 8, 8)` で返す。`cap` は確保器の上限（2^46。A-4）の 1/8 未満なので、`cap * 2 * 8` は `Int` であふれず、上限を超える確保は確保器が停止する。
- 不変条件: `0 <= len <= cap`、`cap > 0` なら `buf` は `raw.alloc(cap * 8, 8)` の塊の先頭、`cap == 0` なら `buf` は空番地。
- `get(l, i)` は、`0 <= i < len` なら `buf + 8 * i`、それ以外は空番地を返す（今と同じ。生成コードは空番地で `None` を作る）。返した番地が次の `push`（領域の取り直し）で無効になりうるのは今の Rust 版と同じで、表現の変更で新しい危険は増えない。
- **破棄**: `free(l)` は入れ物だけを捨てる（空番地は何もしない。`cap > 0` なら `raw.free(buf, cap * 8, 8)`、続いて `raw.free(l, 24, 8)`）。要素は呼び出し側が先に捨てる（今の `sigil_rt_list_free` と同じ取り決め）。
- **要素が `String` のときの解放（`list_free_strs` 相当）**: `std.core.list` は要素の型を知らないので、`free_strs` のような型別の関数は置かない。要素の破棄は型ごとに次の主体が行う。
  - 3-5e より前: Rust の `val_drop` が、記述子の `K_LIST` の腕で、関数表の `len`・`get` で要素を読み、要素の記述子で積んでから（`String` なら文字列の関数表の `free`）、関数表の `free` で入れ物を捨てる（今の手順のまま、呼ぶ先だけを替える）。
  - 3-5e から: codegen が型ごとに生成する破棄の関数（C-1）が、要素の型の破棄（`List<String>` なら `std.core.string.free`）を各要素に呼んでから `std.core.list.free` を呼ぶ。
  - `sigil_rt_list_free_strs` は Rust の単体試験からしか呼ばれないので、3-5c-ii で消す（試験は要素を 1 つずつ捨ててから `free` を呼ぶ形に直す）。

#### E-2. Sigil 側のモジュール

- 埋め込みのモジュール `std.core.list`（`src/stdlib/core_list.sigil`、`targets bare, browser, server`、`trust base`）に、`Addr`・`Int` だけを受け渡す関数を置く。頭や要素の領域を読み書きする関数は `unchecked` で、前提の文に「`l` は `std.core.list` が返し、まだ解放していない頭（したがって E-1 の不変条件）」「`e` は 8 バイト読める」などを書く。
  - 3-5c-ii: `create(elem: Int) -> Addr`・`free(l: Addr)`・`push(l: Addr, e: Addr)`（`e` の指す語を末尾へ写す）・`get(l: Addr, i: Int) -> Addr`・`len(l: Addr) -> Int`。
  - 3-5c-iii: `sum_int(l: Addr, out: Addr) -> Int`・`sum_float(l: Addr) -> Float`。
- **関数の名前**: 作る関数は `new` ではなく `create` とする（**E-9 の 5**）。`lines` が `std.core.string.new` と一覧の作る関数を同じモジュールへ取り込むので、`use` に別名がない今の言語では名前が衝突する（E-0）。ほかの名前（`free`・`len`）は `lines` が取り込まないので衝突しない。
- `lines` は `std.core.string` に置き、`use std.core.list.create;`・`use std.core.list.push;` で一覧を作る（文字列の配置を知るのは `std.core.string` だけ、一覧の配置を知るのは `std.core.list` だけ、を保つ）。`std.core.list` は `std.core.string` を使わないので、取り込みは一方向で循環しない。
- 結び方は D-3 と同じ: すべてのプログラムに暗黙に結ぶ（`implicit_roots` に `std.core.list` を足す）。lowering は入口から辿れなくても SIR に入れる。評価器でも結ぶが、評価器の `List` の組み込みは Rust のまま（C-7 の最後の項）で、これらの関数は呼ばない。試験用の差し込み（`Options::raw_ops`）で `raw.alloc`/`raw.free` を置き換えたときは結ばず、ネイティブ出力を求めたら codegen は `CgError::Internal` で止める（D-3 と同じ）。
- 利用者からは呼べない（D-2 の最後の項と同じ。`use std.*` は拒否される）。

#### E-3. codegen と評価器の付け替え（D-3 に対応）

- **シム**: `sigil_rt_list_new`（`create` を呼ぶ）・`_free`・`_push`・`_get`・`_len` を、Sigil の関数を呼ぶ C の呼び出し規約のシムとして生成コードの中で定義し（`Linkage::Local`。D-3 と同じ）、`rt` に登録する。名前を `rt_spec` から `shim_spec` へ移す。形は変えない（`list_new`・`list_len` は `("p", 'p')`、`list_get` は `("pp", 'p')`、`list_push` は `("pp", 'v')`、`list_free` は `("p", 'v')`）。生成コードの呼び出し箇所（`self.rt("list_new", ..)` など）は変えない。
  - シムは、Sigil の関数の戻りの状態が 0 でなければ異常停止する（D-3 と同じ。`create` の `elem != 8` もここで止まる）。
- **関数表**: Rust ランタイムは `sigil_rt_set_list_ops(new, free, push, get, len)`（`rt_spec` は `("ppppp", 'v')`）で番地を受け取る。生成コードの `main` は、`sigil_rt_set_str_ops` の直後、入口を呼ぶ前に登録する（入口の引数の `List` を `coerce` が作るので、`enter` より前でなければならない）。未登録で一覧に触れたら Rust ランタイムは異常停止する（D-3 と同じ。Rust の `Vec` に黙って戻らない）。3-7b（未合流）は、文字列の表と同じく起動の処理で登録する（D-3 の最後の項。後から合流する方が持っていく）。
- **Rust ランタイム**: `RtList` を中身の分からない型（`#[repr(C)] struct RtList { _p: [u8; 0] }`）にし、`sigil_rt_list_new`・`_free`・`_free_strs`・`_push`・`_get`・`_len` の Rust の定義を消す。E-0 の消費者は、関数表の `len`・`get`（読む）・`create`・`push`（作る）・`free`（捨てる）を呼ぶ形に機械的に替える。`bytes_from_list` の `(*l).buf` の直接の読みは、3-5c-i で `len`・`get` を使う形に先に替える（E-6）。
  - Rust の側の `str_lines`（3-5c-iii で消すまで）は、関数表で一覧を作り、文字列の関数表で要素を作る（2 つの表を通すだけで、一時的な橋は要らない。D-8 の 6 の案 A）。
- **`list_sum_int`**（3-5c-iii）: シム `("pp", 'i')` で `std.core.list.sum_int` を呼ぶ。あふれは Sigil の `+` の停止（シムの異常停止になり、停止の報告の位置と種類が変わる）にせず、足す前に `x > 0 && acc > MAX - x`・`x < 0 && acc < MIN - x` を比べて 1 を返す。生成コードは今のとおり戻りが 0 でなければ呼び出し元の位置の `Overflow` の停止にする（`call.rs` の `ListSumInt` は変えない）。
- **`list_sum_float`**（3-5c-iii）: 先頭の要素から順に足す（空は 0.0。最初の要素を初期値にするので、`[-0.0]` の和は `-0.0` のまま。評価器の `ListSumFloat` と同じ）。要素の語は `raw.load_i64` で読み `Float.from_bits`（3-5a）で読み替える。シムの戻りは `Float` のビット列の語とし、`shim_spec` の形を `("p", 'p')` にする（**E-9 の 6**。`rt` は `f` の戻りを語に読み替えているだけなので、呼び出し側の値は同じ。`shim_sig` に浮動小数の戻りを足さない）。
- **`list_best`**（3-5c-i）: Sigil へは移さない。要素の比較は記述子が要り（C-1 の `val_cmp` は 3-5e まで Rust）、`std.core.list` は記述子を読めないため。codegen が `ListMin`/`ListMax` の腕で、`list_len`・`list_get` と `rt("val_cmp", ..)` を使うループを生成する（今の `ListMaxBy`/`ListMinBy` と同じ形。**E-9 の 3**）。規則は今と同じ: 最初の要素を仮の最良とし、`val_cmp` が `Less`（`min`）/`Greater`（`max`）を返したときだけ取り替える（同順位は先のもの、比べられない相手（戻り 2）は取らない）。`rt_spec` から `list_best` を消し、Rust の `sigil_rt_list_best` を消す。3-5e で `val_cmp` が型ごとの生成になれば、このループもそれを呼ぶ。
- **`str_lines`**（3-5c-iii）: `std.core.string.lines(s: Addr) -> Addr` を足し、シム `("p", 'p')` を `shim_spec` へ。規則は `src/shared/text.rs` の `lines` と同じ（`\n` で分け、最後の空の行を落とし、行末の `\r` を 1 つ外す）。各行は `new`（検証なし。正しい UTF-8 を ASCII の `\n`・`\r` の位置で切った部分は正しい UTF-8。D-2 の検証なしの `new` を呼ぶ条件に当たる）で作り、`create(8)`・`push` で積む。Rust の `sigil_rt_str_lines` を消す。
- **評価器**: 組み込み（`src/ir_eval/call.rs` の `List*`・`StrLines`）は変えない（C-7 の最後の項）。評価器で変えるのは `implicit_roots` で `std.core.list` を結ぶことだけで、`std.core.list` の関数は試験（E-5）が評価器の上で直接呼ぶ。

#### E-4. 契約変更の有無

- `src/ir.rs`（`Builtin`・`Ty`）・`src/ast.rs`・`RAW_OPS` は変えない。`List` を Sigil の構造体（`Ty::Adt`）にする置き換えは、`String` と同じく P1-R の範囲の外とする（D-1 の最後の項）。順序は「本補足 E（ADR）→ 3-5c-i → 3-5c-ii → 3-5c-iii」。
- 新しい `use` の別名の構文（E-2 の衝突を避ける別案）は、`src/ast.rs` の契約変更になるので採らない。

#### E-5. 漏れと二重解放の補完策（D-5 に対応）

- 3-5c-ii から、`List` の頭と要素の領域は Sigil 製確保器から確保するので、`sigil_blocks=` が数える（空の一覧は頭の 1 塊、要素を持つ一覧は 2 塊）。E-0 のとおり今は `List` がどの数にも入っていないので、切り替えで初めて `List` の漏れが見えるようになる。切り替えと既存の漏れの発見を混ぜないため、3-5c-i で、Rust の `RtList` の頭を `blocks=`（`LIVE_BLOCKS`）に数える（`sigil_rt_list_new` で 1 増やし、`sigil_rt_list_free` で 1 減らす。**E-9 の 4**）。3-5c-ii では、その数は `sigil_blocks=` へ移る（`blocks=` から `List` の分が消える）。
  - 3-5c-i で既存の漏れが見つかったら、その PR で原因を直す（codegen・`val_*` の漏れなら同じ PR、レーンが違うなら Issue を立てて 3-5c-i を止める）。見つからなければ PR にその旨を書く。
- **数の完全一致**（3-5c-i・3-5c-ii・3-5c-iii の完了基準）: 新しい試験ファイル（`tests/list_35c.rs`）に、次の繰り返しを 1000 回と 100000 回で走らせ、`blocks=`（3-5c-i。`List` の頭）と `sigil_blocks=`（3-5c-i から。3-5c-i では `List<String>` の要素の文字列と `lines` が作る各行の文字列の漏れ、3-5c-ii 以降は `List` の頭と領域も）が完全一致することを置く: `List<Int>` の追加（`cap` が 4→8→16 と伸びる数を含む）・`map`・`find`・`min`/`max`・`max_by`・`get`/`get_mut`、`List<String>`・`List<List<String>>`・組の一覧の作成と破棄、`clone`（`val_clone`）、`sum`、`lines`、`Bytes.from_list`。わざと解放を 1 つ抜いた版と 2 回にした版で失敗することを手元で確かめ、PR に結果を書く（D-5 の 1 と同じ）。
- **評価器の塊の表の試験**（3-5c-ii・3-5c-iii）: `std.core.list` の各関数（と 3-5c-iii の `lines`）を評価器の上で直接呼び（`tests/str_35b.rs` の駆動モジュールと同じ形）、塊の表で次を確かめる: `create` の後は頭だけ、追加で領域が 4・8・16 要素の大きさで取り直され古い領域が返ること、`free` の後に表が空に戻ること、二重の `free`・`free` の後の `get`/`push`・大きさ違いの解放（`cap` を書き換えた頭）で評価器が止まること、`create(0)`・`create(16)` で止まること、`get` の範囲外（−1・`len`）が空番地を返すこと。`lines` は、各行の文字列（頭と中身）と一覧の頭・領域が、要素ごとの `std.core.string.free` と `std.core.list.free` で全部返ることを確かめる。
- 終了時の数の完全一致を持つ既存の試験（`tests/sigil_alloc_34i.rs` の `strs=0 blocks=0 sigil_blocks=<数>` など）は、終了時に一覧が残らない限り変わらない見込みである。変わる場合（例: 入口が `List` を返し、表示の後に捨てていない）は、実装の PR に理由を書き、`String` を返すときと同じ扱いにそろえる。
- **見逃すもの**は D-5 と同じ（スラブの塊が再利用されてからの二重解放、`mmap` の塊への誤った書き込み）。頭（24B）は文字列の頭と同じ大きさのクラスから取るので、一覧の頭と文字列の頭を取り違えた解放は、大きさの検査では見つからない。取り違えは型の検査と codegen の生成で防ぎ、上の完全一致の試験で `List<String>` を多く作って確かめる。

#### E-6. 分割（3-5c を 3 つに分ける）

| PR | 範囲 | 依存 | 要承認 |
|---|---|---|---|
| 補足 E | 本補足（docs のみ）。C-7・D-6 の 3-5c の行と D-8 の 6 を直す | 補足 D | 不要（ADR） |
| 3-5c-i | 下準備（表現は変えない）: (1) Rust の `RtList` の頭を `blocks=` に数える（E-5）。(2) `ListMin`/`ListMax` を codegen のループにし、`rt_spec` の `list_best` と Rust の `sigil_rt_list_best` を消す（E-3）。(3) `bytes_from_list` の `(*l).len`・`(*l).buf` の直接の読みを `sigil_rt_list_len`・`_get` に替える。`tests/list_35c.rs` を足す | 補足 E | 不要（`unchecked` を足さず、言語の振る舞いを変えない） |
| 3-5c-ii | 表現の切り替え（注: `list_get`・`list_len` はシム経由の呼び出しになる。その性能は測定せずに許容し、`scripts/native-report.sh` の所要時間などで退行が大きければ後続の PR で見直す）: `std.core.list` の `create`・`free`・`push`・`get`・`len`（E-1・E-2）、`implicit_roots` への追加、codegen のシムと `sigil_rt_set_list_ops` の登録、Rust の `RtList` の不透明化と消費者の関数表への付け替え（`str_lines` は Rust のまま関数表で作る）、`sigil_rt_list_*`（`_free_strs` を含む）の Rust の定義の削除、`runtime/core` の試験用の関数表（`#[cfg(test)]`。同じ 24B の頭を Rust の確保で持つ小さな実装）と `tests/runtime_parity.rs` の C のドライバの関数表（E-7）、`blocks=` から `List` の分が `sigil_blocks=` へ移ることに合わせた試験の期待の更新。言語の振る舞いは変えない | 3-5c-i（と合流済みの 3-5b-i・3-7a） | **要**: 埋め込みの `std.*` に `unchecked` の関数を足す（S2）、`sigil spec --std` の出力が増える |
| 3-5c-iii | 操作の移し替え: `std.core.list` の `sum_int`・`sum_float`、`std.core.string` の `lines`（E-3）。`rt_spec` から `list_sum_int`・`list_sum_float`・`str_lines` を消し、Rust の `sigil_rt_list_sum_int`・`_sum_float`・`sigil_rt_str_lines` を消す。`tests/runtime_parity.rs` の `lines` の項を外し、`tests/list_35c.rs` の評価器・ネイティブの突き合わせへ移す。下書きの `core_sigil.rs` の `LIST_SIGIL` と `tests/core_sigil_test.rs` のその行を消す（C-1。`MAP_SIGIL` は残す） | 3-5c-ii（と合流済みの 3-5b-i・3-7a、`sum_float` が使う `Float.from_bits` の 3-5a） | **要**: S2 |

- **各 PR の完了条件**（C-7 の共通の基準に加えて）:
  - 3-5c-i: 既存の全件の突き合わせと `scripts/native-report.sh` が通る。`tests/list_35c.rs` の `blocks=` と `sigil_blocks=`（`List<String>` の要素と `lines` の各行の文字列）の完全一致が通り、わざと解放を抜いた版で失敗することを確かめた結果が PR にある。`min`/`max` について、同順位・比べられない値（`Float` の NaN）・1 要素の一覧で評価器とネイティブが一致する試験がある。`rt_spec` から `list_best` が消えている。
  - 3-5c-ii: 上に加えて、`tests/list_35c.rs` の `sigil_blocks=` の完全一致と、評価器の塊の表の試験（E-5）が通る。`rt_spec` から `list_new`・`list_push`・`list_get`・`list_len` が消え、`runtime/core` に `RtList` の中身を知る Rust のコードが残っていない（`grep` で `(*l).buf` などがない）。`runtime/core` の単体の `cargo test` と `tests/runtime_parity.rs` が、未定義のシンボルなしにリンクして通る。
  - 3-5c-iii: 上に加えて、`sum_int`（空・1 要素・`i64` の最大と最小のちょうど・1 つ外のあふれ・途中であふれて後で戻る列）・`sum_float`（空・`-0.0`・`inf`・NaN・足す順で結果が変わる列。ビット列で比べる）・`lines`（空・`\n` だけ・末尾の改行あり/なし・`\r\n`・`\r` だけ・連続した空の行・多バイトの文字）を、評価器の上の直接の呼び出しとネイティブで `src/shared/text.rs` の `lines`・評価器の組み込みと突き合わせる。`rt_spec` から 3 つの名前が消えている。
- **並行と順序**: 3-5c-i・ii・iii は直列にする（ii は i の数え方と `bytes_from_list` の読み替えに、iii は ii の表現に依存する）。3-5c-i は 3-6a と並行してよい（3-5b-ii は合流済み。3-5d は次のとおり 3-5c-ii の後なので、3-5c-i とは並行しない）。3-5d は `bytes_from_list` を Sigil へ移すときに `std.core.list` の `len`・`get` を使うので、3-5c-ii の後にする（D-6 の 3-5d の依存を改める）。3-5e は 3-5c-iii の後にする（`val_*` の生成が `std.core.list` を呼ぶ。D-6 の 3-5e の依存の「3-5c」は「3-5c-iii」と読む）。3-7b とはどちらが先に合流してもよい（関数表の登録を後から合流する方が起動の経路へ持っていく。E-3）。
- 3-5c-ii は、作る側と読む側を一度に替える必要があるので、それ以上には分けない。3-5c-iii は、`sum_*` と `lines` の 2 つに分けてもよい。

#### E-7. 試験の戦略（D-7 に対応）

- **`tests/list_35c.rs`**（新しいファイル）: E-5 の完全一致と塊の表の試験、E-6 の完了条件の突き合わせを置く。駆動は `tests/str_35b.rs` と同じく、埋め込みの追加（`Options::embedded_extra`）で `std.core.t` を足し、評価器（`ir_eval::run_sources_with`）とネイティブ（`native::build_program`）の両方で走らせる。
- **`runtime/core` の単体の `cargo test`**: 3-5c-ii で、文字列と同じく試験用の一覧の関数表を、試験の始めに一度だけ登録する。`lines_list_and_shared` は、3-5c-ii では関数表の `len`・`get` と要素ごとの `str_free`・表の `free` で書き直し、3-5c-iii で `sigil_rt_str_lines` とともに消す。`val.rs` の `list_sum_int`・`list_best` の試験は、それぞれ消す PR（iii・i）で消す。
- **`tests/runtime_parity.rs`**: 3-5c-ii で、C のドライバに同じ 24B の頭を `malloc` で持つ一覧の関数表（`create` は `elem != 8` で `abort`）を定義し、`main` の冒頭で `sigil_rt_set_list_ops` に登録する（`lines` の項が Rust の `str_lines` を通るため）。`core_alone_links_without_os` の `d.c` は一覧を使わないので登録しない見込みで、リンクに失敗するなら同じ表を足す。3-5c-iii で `lines` の項を外す（E-6）。
- 既存の試験ファイルの変更（`runtime_parity.rs`・`core_sigil_test.rs`、期待が変わる場合の `sigil_alloc_34i.rs` など）は、本補足を根拠として該当の PR で行う（AGENTS.md の「既存のテストファイルへの追記」の例外。D-7 の最後の項と同じ）。

#### E-8. 既存の記述との整合

- **C-7 の表の 3-5c の行**: 「3-5c-i・3-5c-ii・3-5c-iii に分割（E-6）」の注を付ける。範囲（`list_*`・`list_sum_*`・`list_best`）は変えないが、`list_best` は Sigil へ移さず codegen のループにする（E-3）。
- **D-6 の表**: 3-5c の行を E-6 の 3 行で置き換える（注を付ける）。3-5d の依存に 3-5c-ii を足す。3-5e の依存の 3-5c は 3-5c-iii と読む。D-6 の表の後の「3-5c・3-5d・3-6a・3-6c は … 3-5b-ii と並行してよい」の項に、3-5d の依存が増えた旨の注を付ける。
- **D-8 の 6**: 案 A（`List` と一緒に 3-5c で移す）に確定した。D-6 の 3-5c の行の「仮置き: D-8 の 6」の印を外す。`str_lines` は 3-5c-iii で移し、3-5b-ii の案（2 つ目の関数表）は採らない。
- **D-2 の「検証なしの `new` を呼ぶのは … 3-5c の `lines`」**: E-3 の `lines` がそれに当たる（変更なし）。
- **D-3 の「試験用の差し込みでは `std.core.string` も結ばない」**: `std.core.list` も同じ扱いにする（E-2）。

#### E-9. 選択肢と推奨（本文はすべて推奨案で書いた）

所有者に確認が要るのは、3-5c-ii・3-5c-iii の S2 のレビュー（`unchecked` の追加）と、`sigil spec --std` の出力の増加の承認だけである。次の 6 つは起草者の推奨案で進め、異論があれば本補足の修正 PR で直す。

1. **要素の大きさ**（E-1）: 8B の語に固定し、`create(elem)` は 8 以外で停止する（**推奨**。今の生成コードと Rust の消費者は 8 しか使わない。呼び出しの形を変えない）／頭に `elem` を持って任意の大きさを許す（使われない一般性のために `unchecked` の前提が増える）／引数 `elem` を消す（codegen の 2 か所と関数表の形を変える。3-5e で整理するときに改めて決める）。
2. **要素の領域の境界**（E-1）: 8（**推奨**。要素は語）／今の 16 のまま。
3. **`list_best`**（E-3）: codegen が `val_cmp` を呼ぶループを生成する（**推奨**。`ListMaxBy` と同じ形で、記述子を読むのは Rust の `val_cmp` だけのまま）／Rust のまま関数表で読む（3-5e まで Rust の関数が 1 つ残る）／Sigil へ移す（記述子を読む `unchecked` が要り、C-1 の方針に反する。不採用）。
4. **3-5c-i の下準備**（E-5）: Rust の `RtList` を先に `blocks=` へ数え、既存の漏れを表現の切り替えと別の PR で見つける（**推奨**）／置かずに 3-5c-ii で初めて数える（切り替えの不具合と既存の漏れの区別が付かない）。
5. **作る関数の名前と `lines` の置き場所**（E-2）: `std.core.list.create` とし、`lines` は `std.core.string` に置く（**推奨**）／`use` に別名を足す（`src/ast.rs` の契約変更。不採用）／`lines` を `std.core.list` に置く（`std.core.list` が文字列の配置を知る関数を呼び、取り込みも同じ衝突が起きる）。
6. **`sum_float` のシムの戻り**（E-3）: ビット列の語（`("p", 'p')`。**推奨**）／`shim_sig` と `alloc_shim` に浮動小数の戻りを足す（シムの生成器が増える）。

### 補足 F（PR 3-5d の前）: `Bytes` の表現と関数表

- **日付**: 2026-11-03
- **状態**: 提案（推奨案で本文を書いた。細目の選択は F-11 に推奨つきで並べ、異論があれば本補足の修正 PR で直す）。本文・補足 A〜E と食い違う箇所は、本補足が置き換える。
- 3-5d（`Bytes`・`int_rot*`・`str_as_bytes`・`bytes_to_string_utf8`）の着手の前（main `825781e`。3-5a・3-7a・3-5b-i・3-5b-ii・3-5c-i〜iii は合流済み、3-7b は未合流）に、D の (b2)（Sigil が頭を持ち、codegen は語の読み替えとシム、Rust は関数表）を `Bytes` に当てはめ、現行の関数をすべて棚卸しして振り分けと分割を決める。

#### F-0. 現状の確認（main `825781e`）

- **Rust の表現**（`runtime/core/src/lib.rs`）: `RtBytes(Vec<u8>)` を `Box` で持ち、`new_bytes` が `live_blocks(1)` で `blocks=` に数える（`Vec` の中身は数えない）。`sigil_rt_bytes_free` が 1 減らす。
- **生成コードの呼び出し**（`src/codegen/func/call.rs`）: `bytes_*` の 20 個のうち、`bytes_free`・`bytes_clone` 以外の 18 個と `str_as_bytes`・`int_rotl`・`int_rotr`・`int_rotr32`。`bytes_free`・`bytes_clone` は Rust の `val_drop`・`dup`（`val_clone`）だけが呼ぶ。
- **Rust の消費者**: `val.rs` の `K_BYTES` の腕（`val_show` の `Bytes([..])`・`eq`・`cmp`・`dup`・`val_drop`）と、`lib.rs` の `bytes_to_string_utf8`・`str_as_bytes`・`bytes_from_list`（一覧は 3-5c-ii から関数表の `len`・`get` で読む）。`runtime/os` と `tests/runtime_parity.rs` は `Bytes` を使わない。
- **評価器**: `Val::Bytes(Rc<RefCell<Vec<u8>>>)`。`src/ir_eval/call.rs` の組み込みが扱い、`std.*` の関数は呼ばない（C-7 の最後の項）。本補足でも変えない。
- **潜在の不一致（ネイティブの不具合）**: `rt_spec` は `int_rotl`・`int_rotr` を `("ppp", 'i')`、`int_rotr32` を `("pp", 'i')` としている。`'i'` の戻りは 32 ビットを符号拡張して読む（`Fx::rt`）ので、Rust の関数が返す `i64` が切り詰められる。実測（main `825781e`、x86_64 Linux）: `(1.rotl(40, 64), 1.rotr(1, 64), 1.rotr32(1), 1.rotl(31, 32), 1.rotl(-1, 32))` は、評価器で `(1099511627776, -9223372036854775808, 2147483648, 2147483648, 2147483648)`、ネイティブで `(0, 0, -2147483648, -2147483648, -2147483648)`。64 ビット幅の回転と、結果の最上位（第 31 ビット）が立つ 32 ビット幅の回転が評価器と食い違う。3-5d-i を待たず、先に 3-5d-0 で `rt_spec` の戻りの形を `'p'` に直す（F-9）。
- **評価器とネイティブで `Err` の文言が違う**（既存の差）: `set` は評価器が `"インデックス範囲外"`・`"不正な引数"`、ネイティブが `"インデックスが範囲外"`・`"バイト値が 0..=255 の範囲外"`。`slice` は評価器が `"範囲外"`・`"範囲不正"`、ネイティブが `"スライス範囲が不正"`。`from_list`（`"0..=255 の範囲外"`）と `to_string_utf8`（Rust の `FromUtf8Error` の文言）は同じ。本補足は評価器を変えないので、この差はそのまま残す（F-8）。

#### F-1. 表現（D-2・E-1 に対応）

- `Bytes` の語は、24B の頭の塊の番地とする。頭は `buf: Addr`（+0）・`len: Int`（+8、バイト数）・`cap: Int`（+16、`buf` の塊のバイト数）。頭は `raw.alloc(24, 8)`、要素の領域は `raw.alloc(cap, 1)` で確保する（`String`・`List` と同じ 3 欄の配置。要素は 1B、境界は 1）。
- 空の `Bytes` は `buf` = 空番地・`len` = `cap` = 0 とし、領域を確保しない（`raw.alloc` は `size > 0` を要する）。
- 不変条件: `0 <= len <= cap`、`cap > 0` なら `buf` は `raw.alloc(cap, 1)` の塊の先頭、`cap == 0` なら `buf` は空番地。`buf..buf+len` の各バイトは 0..=255（`raw.load_u8` の値）。
- **伸ばし方**: 追加で `len + k > cap` になったら、新しい `cap` を `max(2 * cap, len + k, 8)` とし、新しい領域へ `len` バイトを `raw.copy` で写して古い領域を `raw.free(buf, cap, 1)` で返す（`Vec<u8>` の最小の容量 8 に合わせる。容量は言語から観測できないので、Rust と同じである必要はない）。各長さは確保器の上限（2^46。A-4）未満なので、`2 * cap`・`len + k` は `Int` であふれず、上限を超える確保は確保器が停止する（終了コード 70）。
- **`with_capacity(c)`**: `c <= 0` なら領域を確保しない（今の `capacity.max(0)` と同じ）。`c > 0` なら `raw.alloc(c, 1)`。上限を超える `c` は確保器の停止（終了コード 70）になり、今の Rust の `Vec::with_capacity`（確保の失敗で `abort`）から終了の仕方が変わる。言語の値としての結果は変わらない（F-8）。
- **破棄**: `free(b)` は、空番地なら何もしない。`cap > 0` なら `raw.free(buf, cap, 1)`、続いて `raw.free(b, 24, 8)`。
- `clone`・`slice`・`concat` の結果は `cap` = `len`（空なら領域なし）とする。

#### F-2. 関数の棚卸しと振り分け

現行の `runtime/core/src/lib.rs` の `bytes_*`（20 個）・`str_as_bytes`・`int_rot*`（3 個）の 24 個と、本補足で足す関数表の項の振り分け。「Sigil」は埋め込みモジュールの関数をシムで呼ぶ（F-4）、「codegen」は生成コードがシムを組み合わせて処理を生成する（`list_best` と同じ扱い。E-3）。

| # | 現行の Rust 関数（`rt_spec` の形） | 移す先 | PR | 備考 |
|---|---|---|---|---|
| 1 | `bytes_new`（`("", 'p')`） | Sigil `std.core.bytes.empty()` | 3-5d-ii | |
| 2 | `bytes_with_capacity`（`("p", 'p')`） | Sigil `create(cap)` | 3-5d-ii | `cap <= 0` は領域なし（F-1） |
| 3 | `bytes_free`（`("p", 'v')`） | Sigil `free(b)` | 3-5d-ii | 関数表だけが使う（Rust の `val_drop`）。シムは定義するが生成コードからは呼ばれない |
| 4 | `bytes_clone`（`("p", 'p')`） | Sigil `clone(b)` | 3-5d-ii | 関数表だけが使う（Rust の `dup`） |
| 5 | `bytes_len`（`("p", 'p')`） | Sigil `len(b)` | 3-5d-ii | シムと関数表 |
| 6 | `bytes_is_empty`（`("p", 'i')`） | Sigil `is_empty(b) -> Bool` | 3-5d-ii | |
| 7 | `bytes_get`（`("ppp", 'i')`） | Sigil `get(b, i, out) -> Int` | 3-5d-ii | 範囲内なら `out` に書いて 1、それ以外は 0。形は変えない |
| 8 | `bytes_set`（`("pppp", 'i')`） | Sigil `set(b, i, v) -> Int` ＋ codegen | 3-5d-ii | 戻りは 0（成功）・1（添字が範囲外）・2（値が 0..=255 の範囲外）。添字を先に調べる（今の順）。`Err` の文字列は codegen が作る（F-11 の 5）。形は `("ppp", 'i')` に変わる |
| 9 | `bytes_push`（`("pp", 'v')`） | Sigil `push(b, v)` | 3-5d-ii | `v & 255` を追加する（今の `byte as u8` と評価器と同じ。ADR 0023 の `requires` は実行時に調べていない） |
| 10 | `bytes_slice`（`("ppppp", 'i')`） | Sigil `slice(b, s, e) -> Addr` ＋ codegen | 3-5d-ii | `0 <= s <= e <= len` でなければ空番地。`Err("スライス範囲が不正")` は codegen が作る。形は `("ppp", 'p')` に変わる |
| 11 | `bytes_concat`（`("pp", 'p')`） | Sigil `concat(a, b)` | 3-5d-ii | |
| 12 | `bytes_extend`（`("pp", 'v')`） | Sigil `extend(a, b)` | 3-5d-ii | 前提の文に「`a != b`」を書く（`&mut a` と `&a` を同時に取れないので、借用の検査が保証する）。`b` の長さだけを先に読み、伸ばした後に `b` の `buf` を読んで写す（`a` の取り直しが `b` の領域に触れないことは `a != b` による） |
| 13 | `bytes_to_string_utf8`（`("ppp", 'i')`） | codegen ＋ Sigil `std.core.string.from_utf8` | 3-5d-iii | F-5 |
| 14 | `bytes_from_list`（`("ppp", 'i')`） | codegen のループ | 3-5d-iii | `list_len`・`list_get` で読み、`bytes_with_capacity(n)`・`bytes_push` で積む。範囲外の要素で作りかけの `Bytes` を `bytes_free` で捨て、`Err("0..=255 の範囲外")` を作る。`std.core.bytes` に置かない理由は F-3 |
| 15 | `bytes_push_u16_be`（`("pp", 'v')`） | Sigil `push_u16_be(b, v)` | 3-5d-ii | 下位 16 ビット |
| 16 | `bytes_push_u32_be` | Sigil `push_u32_be(b, v)` | 3-5d-ii | 下位 32 ビット |
| 17 | `bytes_push_u64_be` | Sigil `push_u64_be(b, v)` | 3-5d-ii | 64 ビット（`>>` は論理右シフト） |
| 18 | `bytes_push_u32_le` | Sigil `push_u32_le(b, v)` | 3-5d-ii | |
| 19 | `bytes_read_u32_be`（`("ppp", 'i')`） | Sigil `read_u32_be(b, off, out) -> Int` | 3-5d-ii | 範囲の判定は `off >= 0 && len >= 4 && off <= len - 4`（`off + 4` を計算しない。`off` が `Int` の最大に近いと Sigil の `+` が停止するため）。形は変えない |
| 20 | `bytes_ct_eq`（`("pp", 'i')`） | Sigil `ct_equal(a, b) -> Bool` | 3-5d-ii | ADR 0023 決定 4 の 2 のとおり。名前は `ct_eq` ではなく `ct_equal`（言語の自由関数 `ct_eq` が、埋め込みモジュールの検査で同名の宣言と衝突する `DuplicateDefinition` になるため。3-5d-ii で確かめた。シムの名前 `bytes_ct_eq` は変えない）。長さが違えば偽、同じなら全バイトの `x ^ y` を `|` で積み、途中で抜けない。分岐は長さ（公開の情報）と添字にだけ依存する |
| 21 | `str_as_bytes`（`("p", 'p')`） | codegen | 3-5d-iii | `str_data`・`str_bytes` のシムで読み、`bytes_from_raw` のシム（F-4）で写す。ループは要らない |
| 22 | `int_rotl`（`("ppp", 'i')`） | Sigil `std.core.int.rotl(x, n, w)` | 3-5d-i | 形は 3-5d-0 で先に `("ppp", 'p')` に直す（F-0 の不具合）。3-5d-i でシムへ移すときもその形を保つ |
| 23 | `int_rotr`（`("ppp", 'i')`） | Sigil `std.core.int.rotr(x, n, w)` | 3-5d-i | 同上 |
| 24 | `int_rotr32`（`("pp", 'i')`） | Sigil `std.core.int.rotr32(x, n)` | 3-5d-i | 形は 3-5d-0 で先に `("pp", 'p')` に直す |
| 追加 | （なし） | Sigil `from_raw(p, n) -> Addr` | 3-5d-ii | `p..p+n` を写して作る。`n < 0` は停止。`n == 0` なら `p` を読まない（空番地でよい。D-2 の長さ 0 の扱いと同じ）。関数表（Rust の `str_as_bytes`・`bytes_from_list` が 3-5d-iii まで使う）と、3-5d-iii の `str_as_bytes` のシム |
| 追加 | （なし） | Sigil `data(b) -> Addr` | 3-5d-ii | 関数表（Rust の `val_show`・`eq`・`cmp`・`bytes_to_string_utf8`）と、3-5d-iii の `to_string_utf8` のシム。長さ 0 なら空番地を返しうる（D-2 と同じく、読む側は長さ 0 で番地を使わない） |
| 追加 | （なし） | Sigil `eq(a, b) -> Bool`・`cmp(a, b) -> Int` | 3-5d-ii | 足して試験だけをし、使うのは 3-5e（D-3 の文字列の `eq`・`cmp` と同じ扱い。`cmp` はバイト列の辞書順で −1・0・1）。`eq` は早く抜けてよい（定数時間は `ct_equal` だけ。`eq` を秘密の値の比較に使わない） |

- 3-5d-ii の後、`rt_spec` に残る `Bytes` の名前は `bytes_to_string_utf8`・`bytes_from_list`・`str_as_bytes` の 3 つで、3-5d-iii で 0 になる。Rust に残る `Bytes` の関数は、関数表の登録 `sigil_rt_set_bytes_ops` だけになる（3-7d で消す）。

#### F-3. Sigil 側のモジュール

- **`std.core.bytes`**（`src/stdlib/core_bytes.sigil`、`targets bare, browser, server`、`trust base`）: F-2 の「Sigil」の関数を、`Addr`・`Int`（・`Bool`）だけを受け渡す形で置く。頭や領域を読み書きする関数は `unchecked` で、前提の文に「`b` はこのモジュールが返し、まだ解放していない頭（したがって F-1 の不変条件）」「`out` は 8 バイト書ける 8 バイト境界の番地」「`p..p+n` は読める」などを書く。
  - 作る関数の名前は `empty`・`create`・`from_raw` とし、`new` を使わない（E-2 と同じく、将来 `std.core.string.new` と同じモジュールへ取り込んでも衝突しないため）。
  - **ほかの埋め込みモジュールを `use` しない**。`len`・`get`・`data`・`free`・`clone` などが `std.core.list`・`std.core.string` の関数と同じ名前で、`use` に別名がない今の言語では取り込めない（E-0）。`bytes_from_list`（一覧を読む）と `str_as_bytes`（文字列を読む）を codegen の側に置くのはこのためで（F-2 の 14・21）、`Bytes` の配置を知るのは `std.core.bytes` だけ、一覧と文字列の配置を知るのはそれぞれのモジュールだけ、を保つ。
  - 結び方は D-3・E-2 と同じ: すべてのプログラムに暗黙に結ぶ（`implicit_roots` に足す）。lowering は入口から辿れなくても SIR に入れる。評価器でも結ぶが、評価器の組み込みは変えない。試験用の差し込み（`Options::raw_ops`）で `raw.alloc`/`raw.free` を置き換えたときは結ばず、ネイティブ出力を求めたら codegen は「`std.core.bytes` が結ばれていない」として `CgError::Internal` で止める。
  - 利用者からは呼べない（D-2 の最後の項と同じ）。
- **`std.core.int`**（`src/stdlib/core_int.sigil`、`targets bare, browser, server`、`trust none`）: `rotl(x, n, w)`・`rotr(x, n, w)`・`rotr32(x, n)` を置く。`raw.*` を使わず、`unchecked` も要らない（S2 の対象外）。結び方は `std.core.bytes` と同じ（暗黙に結ぶ。差し込みのときは結ばない）。
  - 規則は今の Rust（評価器とネイティブの Rust の関数）と同じ: 以下の `w` は有効幅（引数の `width` が 32 のとき 32、それ以外はすべて 64）とする。回転の量は `n % w` を符号なしとして読むもので、2 の補数では `n & (w - 1)` に等しい（負の `n` も含む）。32 ビット幅の結果は 0..2^32 の非負の `Int`（上位 32 ビットは 0）。
  - 実装は分岐なし（ADR 0023 決定 4 の 1）: 32 ビット幅は `u = x & 0xFFFFFFFF`、`k = n & 31`、`((u << k) | (u >> (32 - k))) & 0xFFFFFFFF`（`32 - k` は 1..=32 で、`>>` の範囲 0..64 に入る。`k == 0` でも `u >> 32 == 0` なので正しい）。64 ビット幅は `k = n & 63`、`(x << k) | (x >> ((64 - k) & 63))`（`k == 0` で `x | x`）。右回転は左回転の量を `(w - k) & (w - 1)` にしたもの。`<<` はあふれで止まらず（評価器の `wrapping_shl`・ネイティブの `ishl`）、`>>` は論理右シフト（ADR 0023 決定 3）なので、秘密に依存する分岐も停止もない（検査つき算術は常に範囲内で、発生しない）。実装（`core_int.sigil` の `rotl_ew`）は、32・64 ビットを一つの式にまとめ、マスクと有効幅 `ew` から作り、右側のシフト量を `s = (ew - k) & (ew - 1)` と書く。上の式と形は違うが、結果は同じ（`tests/bytes_35d.rs` で突き合わせる）。
  - ADR 0023 の `requires 0 <= n && n < width && (width == 32 || width == 64)` は実行時に調べていない。本補足でも今の寛容な規則（上）をそのまま保つ。

#### F-4. codegen のシムと関数表（D-3・E-3 に対応）

- **シム**: F-2 の「Sigil」の関数を、C の呼び出し規約のシムとして生成コードの中で定義し（`Linkage::Local`）、`rt` に登録する。名前は `rt_spec` から `shim_spec` へ移す。`bytes_set`・`bytes_slice` は形が変わる（F-2）ので、`call.rs` の該当の腕を直す。ほかの腕の呼び出し（`self.rt("bytes_len", ..)` など）は変えない。
  - 3-5d-i: `int_rotl`・`int_rotr`（`("ppp", 'p')`）・`int_rotr32`（`("pp", 'p')`）。形は 3-5d-0 で `rt_spec` の側を先に直したものと同じ。
  - 3-5d-ii: `bytes_new`（`("", 'p')`）・`bytes_with_capacity`・`bytes_from_raw`（`("pp", 'p')`）・`bytes_free`・`bytes_clone`・`bytes_data`・`bytes_len`・`bytes_is_empty`・`bytes_get`・`bytes_set`・`bytes_push`・`bytes_slice`・`bytes_concat`・`bytes_extend`・`bytes_push_u*`・`bytes_read_u32_be`・`bytes_ct_eq`。`bytes_eq`・`bytes_cmp` は 3-5e でシム（または直接の呼び出し）にする。
  - 3-5d-iii: `str_from_utf8`（`("ppp", 'p')`。F-5）。
  - シムは、Sigil の関数の戻りの状態が 0 でなければ、停止の報告を出して `exit_group(70)` で止める（`Fx::alloc_shim`。確保器のシムを含むすべてのシムで同じ。F-10 の 1）。
  - `'i'` の戻りは 32 ビットへ切り詰めて符号拡張で読むので、`Int` の値を返す関数のシムは必ず `'p'` にする（F-0 の不具合の再発防止）。`'i'` にしてよいのは、戻りが `Bool` か小さい状態の番号（0..=2）のものだけとする。
- **関数表**: Rust ランタイムは `sigil_rt_set_bytes_ops(from_raw, free, clone, data, len)`（`rt_spec` は `("ppppp", 'v')`）で番地を受け取る。生成コードの `main` は、`sigil_rt_set_list_ops` の直後、入口を呼ぶ前に登録する（登録の順は `set_sigil_live` → `set_str_ops` → `set_list_ops` → `set_bytes_ops` → `enter`）。未登録で `Bytes` に触れたら、Rust ランタイムは「`sigil: バイト列の関数表が未登録のままバイト列を使った`」を出して終了コード 70 で止める（D-3・E-3 と同じ。Rust の `Vec` に黙って戻らない）。`#[cfg(test)]` の自動の登録と `SIGIL_TEST_STR_MODE=unregistered` での未登録の試験も、文字列・一覧の表と同じ形にする。3-7b（未合流）は、文字列・一覧の表と同じく起動の処理で登録する（後から合流する方が持っていく）。
- **Rust ランタイム**（3-5d-ii）: `RtBytes` を中身の分からない型（`#[repr(C)] struct RtBytes { _p: [u8; 0] }`）にし、F-2 の 3-5d-ii の行の Rust の定義（`sigil_rt_bytes_*` の 18 個）と `new_bytes`・`bytes_slice`・`bytes_vec` を消す。`val.rs` の `K_BYTES` の腕と、残る `bytes_to_string_utf8`・`str_as_bytes`・`bytes_from_list` は、関数表の `data`・`len`（読む）・`from_raw`（作る）・`clone`・`free` を呼ぶ形に機械的に替える。
- **評価器**: 組み込みは変えない（C-7 の最後の項）。変えるのは `implicit_roots` で `std.core.bytes`・`std.core.int` を結ぶことだけ。

#### F-5. `bytes_to_string_utf8` の失敗の文言と `valid_utf8` の使い方

- **方針（第一候補）**: 今の Rust の `FromUtf8Error` の `Display` の文言を、ネイティブで 1 字も変えずに再現する。評価器（`src/ir_eval`。`String::from_utf8` の `e.to_string()`）と契約（`src/ir.rs`）には触れない。文言は次の 2 つ（Rust 1.x の標準ライブラリ。単数でも `bytes`）:
  - `error_len` があるとき: `invalid utf-8 sequence of {error_len} bytes from index {valid_up_to}`
  - 入力の終わりで切れたとき（`error_len` が `None`）: `incomplete utf-8 byte sequence from index {valid_up_to}`
- **Sigil の側**（3-5d-iii）: `std.core.string` に `from_utf8(p: Addr, n: Int, info: Addr) -> Addr` を足す（`unchecked`。前提は「`p..p+n` は読める、`info` は 16 バイト書ける 8 バイト境界の番地」）。正しい UTF-8 なら検証なしの `new(p, n)` で作って返す（D-2 の「検証なしの `new` を呼ぶのは `std.core.string` 自身」に当たる。検証は 1 回で済む）。不正なら空番地を返し、`info` の +0 に `valid_up_to`、+8 に `error_len`（終わりで切れたら 0、それ以外は 1..=3）を書く。
  - 判定は `valid_utf8` と同じ内部の `seq` を使う。`seq` は不正な列で `-(最大の不正な部分列の長さ)` を返すが、終わりで切れたかを区別しない。そこで「`seq` が `-k` を返し、`i + k == n` で、かつ先頭のバイト `b0` が 194..=244（多バイトの列の先頭として正しい）とき、かつそのときに限り終わりで切れた（`error_len` が `None`）」とする。この規則は、`std::str::from_utf8` の `valid_up_to`・`error_len` と、長さ 0〜5 の 25 種のバイト（境界の値）の全組み合わせと 3 種の接頭辞の 30,517,578 通りで一致することを、起草時に Rust で確かめた（`seq` を写した Rust の関数で比べた）。
  - `valid_utf8(p, n) -> Bool` は変えない（実装は `Bool` を返す。D-2 の「`-> Int`」は誤り。F-10 の 3）。`from_utf8` が誤りの位置を要るので、`valid_utf8` を呼んでからもう一度走査する形にはしない。`valid_utf8` は 3-6b の `fs.read` が使う（D-2 のとおり）。
- **codegen の側**（3-5d-iii）: `BytesToStringUtf8` の腕を、`bytes_data`・`bytes_len` のシムで読み、`str_from_utf8` のシムを呼ぶ形にする。空番地でなければ `Ok(s)`。空番地なら、`info` を読み、`error_len` が 0 でなければ「文字列のリテラル `invalid utf-8 sequence of ` ＋ `show_int(error_len)` ＋ ` bytes from index ` ＋ `show_int(valid_up_to)`」、0 なら「`incomplete utf-8 byte sequence from index ` ＋ `show_int(valid_up_to)`」を `str_concat` のシムでつなぎ、`Err` を作る（**F-11 の 3**）。リテラルは今の `cg.bytes_data` と `str_new` で作る。`show_int` は 3-5f まで Rust のままで、3-5f で Sigil 版になっても呼び出し側は変わらない。この分岐は失敗のときだけ走るので、`show_int` と連結の手間は問題にしない。途中の文字列は連結の後に `str_free` で捨てる（F-9 の数の完全一致で確かめる）。
- **やむを得ず契約の変更が要る場合**（例: 評価器とネイティブの文言をそろえるために `Builtin` の形を変える、`Err` の型を構造化する）は、本補足の範囲で行わない。`src/ir.rs` だけの `contract-change` の PR を分けて先に出し、その後に ADR の修正 PR、実装の PR の順にする（AGENTS.md の「契約変更 → ADR → 実装」）。評価器（`src/ir_eval`）の文言だけを変える場合も、3-5d-iii に混ぜず別の PR にする。

#### F-6. 生存数（`blocks=`）と `val.rs` の腕（3-5e に委ねる範囲）

- **`blocks=` の数え方**: 今の `RtBytes` は、頭を `blocks=`（`LIVE_BLOCKS`）に数えている（F-0）。`List` と違って下準備（E-5 の 3-5c-i）は要らない。3-5d-ii から `Bytes` は Sigil 製確保器から確保するので、`sigil_blocks=` が数える（空の `Bytes` は頭の 1 塊、領域を持つ `Bytes` は 2 塊）。`blocks=` から `Bytes` の分が消え、`blocks=` は `blk_new` の塊だけを数える（**F-11 の 4**）。Rust の側で `Bytes` を数え続ける形（シムから Rust の数を増減する）は採らない。`SIGIL_RT_LIVE` の書式と、`strs=`/`blocks=` を外す時期（3-5e。C-5）は変えない。
  - 3-5d-ii で、`Bytes` を作る試験の `blocks=`・`sigil_blocks=` の完全一致の期待（`tests/list_35c.rs` の `Bytes.from_list` の項など）を、`Bytes` の分を `blocks=` から `sigil_blocks=` へ移す形に直す。変わる試験ファイルと理由を PR に書く。
- **`val.rs` の `K_BYTES` の腕**（`val_show`・`eq`・`cmp`・`dup`（`val_clone`・`val_shadow`）・`val_drop`）:
  - 3-5d-ii で行うのは、腕の中の呼び出し先の付け替えだけとする（`bytes_slice` → 関数表の `data`・`len`、`sigil_rt_bytes_clone` → 関数表の `clone`、`sigil_rt_bytes_free` → 関数表の `free`）。`K_BYTES` の番号（15）、表示の書式（`Bytes([1, 2])`）、比較の規則（辞書順）、空番地の語の扱い（`w == 0` の判定）は変えない。
  - **3-5e に委ねる**: 型ごとの `eq`・`cmp`・`clone`・`drop` の処理の生成（C-1）で、`Bytes` の腕が `std.core.bytes` の `eq`・`cmp`・`clone`・`free` を呼ぶようにすること、`val.rs` の `K_BYTES` の腕（と記述子の `K_BYTES`）を消すこと、`--trace-drops` の `Bytes` の記録。`eq`・`cmp` の Sigil の関数は 3-5d-ii で足して試験しておく（F-2）。
  - **3-5f に委ねる**: `val_show` の `Bytes([..])` の表示（3-5f の表示の移し替え）。

#### F-7. 契約変更の有無

- `src/ir.rs`（`Builtin`・`Ty`）・`src/ast.rs`・`RAW_OPS` は変えない。`Builtin::Bytes*`・`IntRot*`・`StrAsBytes` はそのまま残り、lowering も変えない。順序は「本補足 F（ADR）→ 3-5d-i → 3-5d-ii → 3-5d-iii」。
- `Bytes` を Sigil の構造体（`Ty::Adt`）にする置き換えは、`String`・`List` と同じく P1-R の範囲の外とする。

#### F-8. 既知の差

- **`set`・`slice` の `Err` の文言**（F-0）: 評価器とネイティブの文言の違いは今のまま残す（ネイティブは今の Rust の文言を Sigil・codegen で再現する）。そろえるのは評価器を直す別の PR とし（F-5 の最後の項）、3-5d の完了基準には入れない。3-5d の試験は、文言を両方の値で個別に固定する（ネイティブは今の文言のまま変わらないことを確かめる）。`set` は、添字だけが不正・値だけが不正に加えて、**添字と値が両方不正**の入力を明示的に含める（ネイティブは添字を先に調べるので `"インデックスが範囲外"`、評価器は `"不正な引数"`。3-5d-ii で調べる順を変えていないことを確かめる）。
- **`with_capacity` の上限超え**（F-1）: 今の Rust は `abort`、3-5d-ii からは確保器の停止（終了コード 70）。どちらも異常終了で、言語の値は変わらない。
- **`Float` の NaN のペイロード**: NaN を生む演算の結果の符号ビットと仮数は、評価器（Rust の `f64`）とネイティブ（Cranelift）で**一致を保証しない**（`Float.to_bits` などで観測できる。起草時の x86_64 の実測では一致したが、CPU や最適化で変わりうる）。試験は NaN を `is_nan` でだけ比べる。`tests/list_35c.rs` の `sum_float` の試験のコメント（「符号・ペイロードも含めて同じ」）は判定より強いので、3-5d-i で直す。
- **`int_rot*` の評価器とネイティブの不一致**（F-0）は既知の差ではなく不具合として 3-5d-0 で先に直す（ネイティブを評価器に合わせる）。

#### F-9. 分割と完了基準

| PR | 範囲 | 依存 | 要承認 |
|---|---|---|---|
| 補足 F | 本補足（docs のみ）。D-3・B-2 の古い記述、D-2 の `valid_utf8` の戻りの型、C-7・D-6 の 3-5d の行を直す（F-10） | 補足 E | 不要（ADR） |
| 3-5d-0 | 不具合の修正だけ: `rt_spec` の `int_rotl`・`int_rotr` の戻りを `("ppp", 'p')`、`int_rotr32` を `("pp", 'p')` に直す（`src/codegen.rs` の 1 行）。回帰試験（F-0 の 5 つの例と、64 ビット幅・第 31 ビットが立つ 32 ビット幅の結果を評価器とネイティブで突き合わせる）を足す。Rust の関数・表現は変えない | 補足 F | S2 は不要。回転は ADR 0020・0023 の定数時間の対象だが、生成する命令の列は呼び出しの戻りの読み方だけが変わるので、通常のレビューでよい |
| 3-5d-i | (1) `std.core.int`（`rotl`・`rotr`・`rotr32`）と `implicit_roots` への追加、`int_rot*` のシム（戻り `'p'`。3-5d-0 の形のまま）、`rt_spec` から `int_rotl`・`int_rotr`・`int_rotr32` を消し、Rust の `sigil_rt_int_rot*` を消す。(2) `tests/bytes_35d.rs` を足す（回転の突き合わせ。下の完了基準）。`Bytes` の振る舞いの固定と `blocks=`/`sigil_blocks=` の完全一致の基準線は 3-5d-i には含めず、3-5d-ii の前提とする（下の完了基準・並行と順序）。(3) 繰り越しの low の 3 件: (a) `std.core.list.sum_int` の前提の文（`src/stdlib/core_list.sigil:117`）の「`out` は 8 バイト書ける」を「`out` は 8 バイト書ける 8 バイト境界の番地」に直す（`raw.store_i64` の境界の要件を明記する。本体は変えない）。(b) `src/spec.rs` の `std_specs`（196〜212 行で集めた外部の宣言）と `extract_file`（257 行の `Some(r.ok())`）が、埋め込みモジュールの検査の失敗を黙って捨てるのを、`Err` として返す（埋め込みの `std.*` の検査の失敗は処理系の不具合なので、`sigil spec --std` は失敗で終わる。`use` した名前に対応する `pub` の関数が見つからない場合も失敗にする）。試験を足す。(b) は `Bytes` と関わらないので、3-5d-i の中の**独立したコミット**にする（別の PR に分けてもよい）。(c) NaN のペイロードの差を既知の差として記録する（本補足 F-8 で記録した。3-5d-i では `tests/list_35c.rs` のコメントを直す） | 3-5d-0（と合流済みの 3-5c-iii） | S2 は不要（`unchecked` を足さない。(a) は既存の前提の文の字句の修正だけで、CI の信頼基盤の差分の検出が反応した場合は PR に理由を書く）。`sigil spec --std` の出力に `std.core.int` が増える。回転は ADR 0020・0023 の定数時間の対象なので、人間のレビューに回す（AGENTS.md） |
| 3-5d-ii | 表現の切り替え: `std.core.bytes`（F-2 の 3-5d-ii の行。`eq`・`cmp` を含む）と `implicit_roots` への追加、codegen のシム（`bytes_set`・`bytes_slice` の腕の形の変更と `Err` の文字列の生成を含む）と `sigil_rt_set_bytes_ops` の登録、Rust の `RtBytes` の不透明化と消費者の関数表への付け替え（`bytes_to_string_utf8`・`str_as_bytes`・`bytes_from_list` は Rust のまま関数表で作る・読む）、`sigil_rt_bytes_*` の 18 個の Rust の定義の削除、`runtime/core` の試験用の関数表（`#[cfg(test)]`。同じ 24B の頭を Rust の確保で持つ小さな実装）と Rust の単体試験の書き直し、`blocks=` から `Bytes` の分が `sigil_blocks=` へ移ることに合わせた試験の期待の更新（F-6）。言語の振る舞いは変えない | 3-5d-i | **要**: 埋め込みの `std.*` に `unchecked` の関数を足す（S2）、`sigil spec --std` の出力が増える。`ct_eq` は ADR 0020 の定数時間の対象なので、人間のレビュー（CODEOWNERS）に回す |
| 3-5d-iii | 操作の移し替え: `std.core.string.from_utf8`（F-5）、`bytes_to_string_utf8`（F-5 の codegen）・`bytes_from_list`（codegen のループ）・`str_as_bytes`（codegen の組み合わせ）。`rt_spec` から 3 つの名前を消し、Rust の `sigil_rt_bytes_to_string_utf8`・`sigil_rt_bytes_from_list`・`sigil_rt_str_as_bytes` を消す | 3-5d-ii（と合流済みの 3-5c-ii。一覧のシム `list_len`・`list_get` を使う） | **要**: S2（`from_utf8` は `unchecked`） |

- **各 PR の完了基準**（C-7 の共通の基準に加えて）:
  - 3-5d-0: 既存の全件の突き合わせが通る。回帰試験が今の main（修正前）でネイティブだけ違って失敗し、修正後に通ることを PR に書く。
  - 3-5d-i: 既存の全件の突き合わせと `scripts/native-report.sh` が通る。3-5d-0 の回帰試験が通り続ける。`tests/bytes_35d.rs` で、`rotl`・`rotr`（幅 32・64・それ以外の値（64 として扱う）、`n` が 0・1・`w - 1`・`w`・`w + 1`・負（−1・−33・`Int` の最小）・大きい値、`x` が 0・1・−1・`Int` の最大と最小・第 31 ビットだけ・上位 32 ビットに値のある値）と `rotr32` を、評価器・ネイティブ・Rust の `rotate_left`/`rotate_right` の 3 つで突き合わせる（F-0 の 5 つの例を含む）。`std.core.int` の関数を評価器の上で直接呼ぶ試験がある。`rt_spec` から `int_rot*` が消えている。`sigil spec --std` が埋め込みの検査の失敗を返す試験がある（(b) のコミット）。
  - 3-5d-ii の前提（基準線）: 3-5d-ii の最初のコミットを試験だけとし、旧実装（Rust の `RtBytes`）の値で次を固定する（または別 PR で先に入れる）。`Bytes` の今の振る舞いの固定: 全メソッドの成功と失敗の値（`set`・`slice` の `Err` の文言を評価器とネイティブで個別に固定。F-8）、`push` の 0..=255 の外の値（−1・256・`Int` の最大）、`read_u32_be` の `off` の境界（−1・`len - 4`・`len - 3`・`Int` の最大）、`to_string_utf8` の不正な列（F-5 の 2 つの文言の両方。途中で切れた列・過長・サロゲート・U+10FFFF 超・先頭の続きのバイト）、`ct_eq`（長さ違い・同じ長さで先頭/末尾だけ違う・空どうし）。`Bytes` の作成と破棄（`new`・`with_capacity`・`from_list`・`as_bytes`・`clone`・`slice`・`concat`・`extend`・`List<Bytes>`）を 1000 回と 100000 回の繰り返しで走らせ、`blocks=` が完全一致することを置き、わざと解放を 1 つ抜いた版と 2 回にした版で失敗することを手元で確かめて PR に書く（D-5 の 1・E-5 と同じ）。
  - 3-5d-ii: 上に加えて、`tests/bytes_35d.rs` の繰り返しの比較を `sigil_blocks=` の完全一致に移して通り、`blocks=` から `Bytes` の分が消えている。評価器の塊の表の試験: `std.core.bytes` の各関数を評価器の上で直接呼び、`empty`/`create(0)` の後は頭だけ、`create(c)` で領域が `c` バイト、追加で領域が `max(2 * cap, len + k, 8)` で取り直され古い領域が返ること、`free` の後に表が空に戻ること、二重の `free`・`free` の後の `get`/`push`・大きさ違いの解放（`cap` を書き換えた頭）で評価器が止まること、`from_raw` の `n < 0` で止まること、`get`/`read_u32_be` の範囲外が 0 を返すこと、`slice` の不正な範囲が空番地を返すこと、`set` の戻りの番号の順（添字を先に調べる）を確かめる。`ct_eq` が長さが同じなら途中で抜けずに全バイトを読むこと（評価器の上の直接の呼び出しで、先頭だけ違う入力と末尾だけ違う入力の `raw.load_u8` の回数が同じこと。数える手段が試験の駆動にない場合は、本体に早く抜ける経路がないことをレビューで確かめ、PR に書く）。`rt_spec` から F-2 の 3-5d-ii の行の名前が消え、`runtime/core` に `RtBytes` の中身を知る Rust のコードが残っていない（`grep` で `RtBytes(`・`bytes_vec` などがない）。`runtime/core` の単体の `cargo test` と `tests/runtime_parity.rs` が、未定義のシンボルなしにリンクして通る（`runtime_parity` は `Bytes` を使わないので表の登録は要らない見込みで、リンクに失敗するなら E-7 と同じく C の表を足す）。関数表の未登録の試験（終了コード 70 と文言）がある。
  - 3-5d-iii: 上に加えて、`to_string_utf8` を、評価器の上の `std.core.string.from_utf8` の直接の呼び出しとネイティブで、Rust の `String::from_utf8` の `valid_up_to`・`error_len`・`to_string()` と突き合わせる（F-5 の規則を確かめた入力の組の部分集合: 長さ 0〜4 の境界の値の全組み合わせと、正しい列のあとに不正な列が続くもの）。`from_list`（空・範囲内・−1・256・途中の範囲外で作りかけを捨てること・`cap` が伸びる長い一覧）と `as_bytes`（空・ASCII・2〜4 バイトの文字）を評価器とネイティブで突き合わせる。失敗の経路を含めて 1000 回と 100000 回の繰り返しの `sigil_blocks=` が完全一致する（`show_int` と連結の途中の文字列が漏れない）。`rt_spec` から 3 つの名前が消えている。
- **定数時間の計測**（3-5d-i・3-5d-ii の完了基準に添える案）: `ct_eq` と `rotl`・`rotr`・`rotr32` を、定数時間の関数の表 `tests/ct_rules.rs`（ADR 0020 決定 1。CODEOWNERS の対象で、まだない。6-1 で加わる予定）の計測の対象に加える案を PR に書く。`tests/ct_rules.rs` を作る・変える PR は人間のレビュー（CODEOWNERS）を要するので、3-5d の PR には混ぜず、案として残す（ファイルがあれば別の PR で足す）。
- **既存の試験との使い分け**: `tests/bytes_methods.rs`（ADR 0023 のメソッドの構文・型検査と評価器での小さな実行）と `tests/examples_bytes_2_5.rs`（サンプル `examples/07-bytes` の構文・正規形・検査・仕様の出力・評価器の実行結果の固定）は、言語の表面の試験としてそのまま残し、3-5d では変えない。`tests/bytes_35d.rs`（新規）は、表現の移し替えの試験（評価器とネイティブの突き合わせ、境界の値、`blocks=`/`sigil_blocks=` の完全一致、評価器の上での `std.core.bytes`・`std.core.int` の直接の呼び出しと塊の表）を受け持つ。既存の 2 つと重なる入力は `tests/bytes_35d.rs` に写さず、ネイティブとの突き合わせが要るものだけを置く。
- **並行と順序**: 3-5d-0・i・ii・iii は直列にする（i は 0 の回帰試験を基準に使い、ii は自分の最初のコミット（または先に入れた別 PR）の基準線の試験を使い、iii は ii の表現に依存する）。3-5d-i は `std.core.int` だけを結ぶので、3-6a・3-6c とは並行してよい。3-5e は 3-5d-iii の後にする（D-6 の 3-5e の依存の「3-5d」は「3-5d-iii」と読む）。3-7b とはどちらが先に合流してもよい（関数表の登録を後から合流する方が起動の経路へ持っていく）。
- 3-5d-ii は、作る側と読む側を一度に替える必要があるので、それ以上には分けない。3-5d-iii は、`to_string_utf8` と `from_list`・`as_bytes` の 2 つに分けてもよい。

#### F-10. 既存の記述との整合（本補足の PR で直したもの）

1. **D-3 と B-2 の「確保器のシムは状態を捨てる」**: 古い。実装（`src/codegen.rs` で確保器・文字列・一覧のシムをすべて `Fx::alloc_shim` で定義し、`src/codegen/func/mod.rs` の `alloc_shim` が戻りの状態を調べて、0 でなければ停止の報告を出して `exit_group(70)` で止める）では、確保器のシムも状態を確かめて止める。B-2 の項と D-3 の codegen の項を、そのように直した（注を付けた）。
2. **C-7・D-6 の表の 3-5d の行**: 「補足 F で置換: 3-5d-0・3-5d-i・3-5d-ii・3-5d-iii に分割。F-9」の注を付けた。D-6 の 3-5d の行の「`bytes_to_string_utf8`（`valid_utf8` を使う）」は、F-5 のとおり `std.core.string.from_utf8`（`valid_utf8` と同じ `seq` を使い、誤りの位置も返す）を使う形に改めた。
3. **D-2 の `valid_utf8(p, n) -> Int`**: 実装（3-5b-i）は `-> Bool` を返す（シムを持たず、Sigil の関数からだけ呼ばれるので、32 ビットの戻りの規則は関わらない）。D-2 に注を付けた。

#### F-11. 選択肢と推奨（本文はすべて推奨案で書いた）

所有者に確認が要るのは、3-5d-ii・3-5d-iii の S2 のレビュー（`unchecked` の追加）、3-5d-i・3-5d-ii の定数時間の処理（回転、`ct_eq`）の人間のレビュー（3-5d-0 は戻りの型の修正だけなので通常のレビュー）、`sigil spec --std` の出力の増加の承認だけである。次の 5 つは起草者の推奨案で進め、異論があれば本補足の修正 PR で直す。

1. **`int_rot*` の置き場所**（F-3）: Sigil の `std.core.int`（**推奨**。`unchecked` が要らず、S2 なしで 3-5d の最初に入れられる。ネイティブの戻りの型の不具合は、3-5d-0（PR #258）で先に直し済みで、3-5d-i はその回帰試験を保ったまま移す）／codegen が Cranelift の `rotl`・`rotr` 命令を直接生成する（ADR 0023 決定 4 の「命令への直接の対応」に近く速いが、幅と量の寛容な規則（`w != 32` は 64、負の `n`）を codegen で再現する分岐が要り、本 ADR の「Sigil へ移す」の方針から外れる。性能が要るなら後の PR で改める）／Rust のまま `rt_spec` の形だけ `'p'` に直して終える（Rust の関数が 3-7d まで残る。形の修正そのものは不具合の修正として 3-5d-0 で先に行い、3-5d-i で Sigil へ移す）。
2. **`bytes_from_list`・`str_as_bytes` の置き場所**（F-2・F-3）: codegen がシムを組み合わせる（**推奨**。配置を知る場所を 1 モジュールに保ち、`use` の名前の衝突を避ける）／`std.core.bytes` に置いて `std.core.list`・`std.core.string` を `use` する（`len`・`get`・`data` が衝突するので、関数の名前を変える必要がある）。
3. **`to_string_utf8` の失敗の文言の組み立て**（F-5）: codegen が失敗の分岐でリテラル・`show_int`・`str_concat` を生成する（**推奨**。Rust を足さず、3-5f で `show_int` が Sigil になれば自動で追随する。起草時の相談（jev）でも同じ選択）／Rust ランタイムに一時的な `sigil_rt_utf8_error_msg(i, n)` を足す（3-7d で消す Rust が増える）／Sigil の側で 10 進の変換と文言を組み立てる（リテラルのバイト列を `raw.store_u8` で並べる `unchecked` が増える）。
4. **`blocks=` の扱い**（F-6）: 3-5d-ii で `sigil_blocks=` へ移す（**推奨**。E-5 の `List` と同じ）／Rust の数を残し、シムから増減する（Sigil の確保と二重に数え、3-5e で消す手間が増える）。
5. **`set`・`slice` の失敗の表し方**（F-2）: Sigil は番号（`set`）・空番地（`slice`）を返し、codegen が文言を作る（**推奨**。文言を生成コードの静的データに置き、Sigil に文字列のリテラルを持ち込まない）／今の形（`err` の出力の引数）のまま、Sigil が `std.core.string` で文言を作る（`std.core.bytes` が `std.core.string` を `use` することになり、2 の衝突が起きる）。

### 補足 G（PR 3-5e の前）: 型ごとの値の処理の生成と 3-5e の分割

- **日付**: 2026-11-03
- **状態**: 提案（推奨案で本文を書いた。細目の選択は G-12 に推奨つきで並べ、異論があれば本補足の修正 PR で直す）。本文・補足 A〜F と食い違う箇所は、本補足が置き換える。
- 3-5e（`val_eq`・`val_cmp`・`val_clone`・`val_drop` の型ごとの生成、`Shared`/`Cell`、`--trace-drops`、`SIGIL_RT_LIVE` の `live: sigil_blocks=<数>` への一本化）の着手の前（main `9cf93f7`。3-5a・3-7a・3-5b・3-5c・3-5d は合流済み、3-7b・3-6a は未合流）に、C-1・C-5・D-6・E-6・F-6 では生成する関数の形・破棄の反復・塊の頭の操作・Rust に残す範囲・試験の期待が決まっていないと実装担当から報告があった。`runtime/core/src/val.rs`・`lib.rs` と codegen の呼び出し箇所を棚卸しして、それらを決め、分割を決める。

#### G-0. 現状の確認（main `9cf93f7`）

- **生成コードが呼ぶ Rust の値の処理**（`rt_spec`）: `val_eq`・`val_cmp`（`("ppp", 'i')`。`src/codegen/func/expr.rs` の `compare`、`call.rs` の `ListMin`/`ListMax`・`ListMaxBy`/`ListMinBy` のループ）、`val_clone`（`expr.rs` の `Rvalue::Clone`）、`val_shadow`（`drop.rs` の `shadow`。`finish` と `SharedGet`）、`val_drop`（`drop.rs` の `drop_val`。`Stmt::Drop`・`store_drop`・`SharedSet`・入口の戻り値・大域）、`blk_new`（`drop.rs` の `blk`。組・構造体・列挙型・`Cell`/`Shared`・クロージャの環境と本体の塊）、`trace_on`（`thunk`。`--trace-drops` のとき）、`val_show`（3-5f）。
- **塊の頭**（`val.rs`）: `blk_new(bytes)` は `n = max(bytes, 8)` を 8 の倍数に切り上げ、`sigil_rt_alloc(16 + n, 8)`（Rust・glibc の確保器）で取り、頭の 2 語 `[n（第 62 ビットは黙った印 SILENT）][参照の数 = 1]` を書いて中身の先頭（頭 + 16）を返し、`LIVE_BLOCKS`（`blocks=`）を 1 増やす。`blk_free` は参照の数の場所が `FREED`（`0x5afe_dead_0000_0001`）なら `abort`、違えば `FREED` を書いて `LIVE_BLOCKS` を 1 減らし（負なら `abort`）、`sigil_rt_free(頭, 16 + n, 8)` で返す。`Cell`/`Shared` は中身の語 1 つの塊で、参照の数は頭の第 2 語（中身 − 8）。
- **`sigil_rt_shared_*`（`lib.rs`）は使われていない**: `rt_spec` にない。生成コードは `Cell`/`Shared` を `blk_new` で作り、参照の数を `val_shadow`・`val_clone`・`val_drop` が増減する。`sigil_rt_shared_*` を呼ぶのは Rust の単体試験（`shared_and_alloc`）だけである。`rt_spec` の `alloc` も呼び出し箇所がない。
- **クロージャ**: 本体は 24B の塊 `[関数の番地][環境の塊（なければ 0）][環境の型の記述子]`（`expr.rs` の `MakeClosure`）。環境を持たない関数の値は頭のない 16B の静的データ `[関数の番地][0]`（`fnblock`）で、参照の数を持たない。`val_drop` の `K_FN` の腕は、環境が 0 でなく参照の数が 0 になったときだけ、+16 の記述子で環境を積んで本体を解放する。記述子の鍵は関数の型の中身を消す（`norm_with(ty, true)`）ので、関数の値の型からは環境の型が分からない。
- **`--trace-drops`**: 生成コードは `cg.trace`（`Opts::trace_drops`。コンパイル時に決まる）のときだけ `thunk` で `sigil_rt_trace_on` を呼び、Rust の `val_drop` が実行時の `TRACE` を見て、構造体を破棄する前に `drop: <表示>`（`show_into` の引用符つきの表示）を `stderr().lock()` へ書く。`sigil_rt_trace_drop`（`lib.rs`）は `rt_spec` になく、使われていない。
- **既に済んでいるもの**: Rust の `sigil_rt_str_cmp`（D-3・D-6 が 3-5e で消すとしたもの）と `sigil_rt_list_free_strs`（E-1）は、main `9cf93f7` に既に無い（`grep` で 0 件）。3-5e で消す Rust の関数には数えない。
- **評価器**（`src/ir_eval/value.rs`）: 破棄は明示の積み（`drain`）で再帰しない。300 万段の `enum L { Nil, Cons(Int, L) }` の破棄が通る（`tests/interp.rs` の `very_deep_value_is_dropped_without_overflowing_the_host_stack`）。`val_eq`・`cmp_val` は再帰し、深すぎれば `DEEP` を立てて `StackExhausted` の停止にする。`Addr` は `==`・`<` で比べられる（`val.rs` の記述子の種類 16 は `eq`・`cmp` の腕がなく、偽・比べられないになる。G-10）。

#### G-1. 生成する関数の形と呼び出し規約

- **作る関数**: 型 `T` ごとに次の 6 つを、必要になったものだけ生成する。すべて `Linkage::Local`、C の呼び出し規約（`cg.cc`）で、Sigil の関数の `(値, 停止の符号)` の形はとらない（停止しない。確保の失敗は確保器とシムが `exit_group(70)` で止める）。
  - `sigil_eq_<k>(a, b) -> i64`（1 = 等しい、0 = 違う）
  - `sigil_cmp_<k>(a, b) -> i64`（−1・0・1、比べられないときは 2。今の `val_cmp` の戻りと同じ値）
  - `sigil_clone_<k>(w) -> i64`（`clone`。`Cell` も独立した複製にする）
  - `sigil_shadow_<k>(w) -> i64`（影。`Copy` の複写。共有するものは共有のまま）
  - `sigil_drop_<k>(w)`（破棄の入口。G-2）
  - `sigil_dstep_<k>(w)`（破棄の 1 段。G-2。入口からと、積みの取り出しからだけ呼ぶ）
- **鍵と重複の排除**: 鍵は記述子と同じ正規形 `norm_with(strip(ty), true)`（参照の入れ子を 1 段にし、関数の型の中身を消す）。`Cg` に `HashMap<(Op, Ty), FuncId>` を持ち、`<k>` はその表の通し番号とする。同じ鍵の関数は 1 つだけ作る。
- **再帰する型と生成の順**: 関数は最初に要った時点で宣言だけして表に入れ、本体は待ち行列に積む。利用者の関数をすべて定義した後、待ち行列が空になるまで本体を定義する（本体の生成が新しい鍵を要求すれば、宣言して積む）。宣言が本体より先なので、`enum L { Nil, Cons(Int, L) }` の `sigil_eq_L` は自分自身を `FuncId` で呼べる。`UserFuncName` の番号は既存の関数・シム・`memmove` の後に続けて振る。
- **呼び出し箇所の置き換え**: `rt("val_eq", [d, x, y])` → `sigil_eq_<k>(x, y)`、`val_cmp`・`val_clone`・`val_shadow`・`val_drop` も同じ。記述子の番地は渡さない。`sigil_eq`・`sigil_cmp` の戻りは `i64` になる（今の `rt` の `'i'` は 32 ビット）。`compare` と `ListMin`/`ListMax`・`ListMaxBy`/`ListMinBy` のループは `icmp_imm` で比べているだけなので、幅を合わせれば式は変わらない。
- **その場で済ませるもの**（関数を作らない）: スカラー（`Int`・`Bool`・`Float`・`Unit`・`Addr`・`ParseError`）の比較・複写と、`String`・`Path`・`Bytes` の破棄・複写は、呼び出し箇所でシムを直接呼ぶか命令にしてよい（今の `Rvalue::Clone` が `String`・`Path` で `str_clone` を直接呼ぶのと同じ）。生成する関数の中でも、スカラーのフィールドは命令で比べる。
- **種類ごとの規則**（今の `val.rs` の `eq`・`cmp`・`dup`・`val_drop` を写す。違うのは G-10 の `Addr` だけ）:

| 型 | `eq` | `cmp` | `clone` / `shadow` | 破棄（`dstep`） |
|---|---|---|---|---|
| `Unit`・`ParseError` | 1 | 2 | 語のまま | なし |
| `Int`・`Bool`・`Addr` | 語の `==` | 符号つきの比較 | 語のまま | なし |
| `Float` | `fcmp` の `==`（`-0.0 == 0.0`、NaN は 0） | `partial_cmp`（NaN を含めば 2） | 語のまま | なし |
| `String` | `std.core.string.eq` | `std.core.string.cmp` | 0 なら 0、それ以外は `str_clone` | `str_free` |
| `Path` | `std.core.string.eq` | 2 | 同上 | `str_free` |
| `Bytes` | `std.core.bytes.eq` | `std.core.bytes.cmp` | 0 なら 0、それ以外は `bytes_clone` | `bytes_free` |
| `List<T>` | 長さが同じで要素ごとに `eq_T` | 短い方の長さまで `cmp_T`（2 ならすぐ 2、0 以外ならそれ）、続いて長さの比較 | 0 なら 0。`list_new(8)` に要素ごとの `clone_T`/`shadow_T` を `list_push` | 要素を積み（逆順）、`list_free` |
| 組 | フィールドごとに `eq` | 組の順に `cmp`（2 はすぐ 2）、全部 0 なら 0 | 0 なら 0。`blk_new(8n)` に各フィールドの複写 | フィールドを積み（逆順）、`blk_free` |
| 構造体 | フィールドごとに `eq` | 2 | 組と同じ。`shadow` は塊に SILENT を立てる | trace（G-3）、フィールドを積み（逆順）、`blk_free` |
| 列挙型 | 腕の番号が同じで、その腕のフィールドごとに `eq` | 2 | 0 なら 0。`blk_new(8 * max(1 + nf, 2))` に番号とフィールドの複写。`shadow` は SILENT を立てる | その腕のフィールドを積み（逆順）、`blk_free` |
| `Ref<T>` | 両方の指す語で `eq_T` | 両方の指す語で `cmp_T` | 語のまま（共有） | なし |
| `Shared<T>` | 番地が同じなら 1、違えば中身の語で `eq_T` | 2 | 0 なら 0。参照の数 +1 | 参照の数 −1、0 になれば中身を積み `blk_free` |
| `Cell<T>` | `Shared` と同じ | 2 | `clone`: 0 なら 0、`blk_new(8)` に中身の `clone_T`。`shadow`: 参照の数 +1 | `Shared` と同じ |
| 関数の値 | 0 | 2 | +8（環境）が 0 でなければ参照の数 +1 | G-2 |

  - 文字列・バイト列の `eq`・`cmp` は、シム `str_eq`（`("pp", 'i')`）・`str_cmp`（`("pp", 'p')`）・`bytes_eq`（`("pp", 'i')`）・`bytes_cmp`（`("pp", 'p')`）を `shim_spec` に足して呼ぶ（F-4 の「`bytes_eq`・`bytes_cmp` は 3-5e でシムにする」。`cmp` は −1 を返すので `'p'`。F-4 の規則）。シムの本体は `Fx::alloc_shim`（状態が 0 でなければ停止）。
  - 一覧の要素は、今と同じく一覧のシム `list_len`・`list_get` で読む（E-6 の 3-5c-ii の注と同じく、呼び出しの手間は測らずに許容する）。
- **入れ子の深さ**: `eq`・`cmp`・`clone`・`shadow` は、今の Rust と同じく値の入れ子に沿って再帰し、深さの検査はしない（G-2 の破棄だけは反復にする）。評価器は深い比較を `StackExhausted` で止めるが、比較の式（`Rvalue::Binary`）には停止の場所（`StopId`）がなく、生成した関数から呼び出し元の位置の停止を報告するには `src/ir.rs` の契約変更が要る。そのため本補足では変えず、既知の差として残す（G-10）。

#### G-2. 破棄の反復と順序

- **反復にする**（再帰は許さない）。理由: 評価器も今の Rust も積みで反復しており、300 万段の連鎖の破棄が通る（G-0）。破棄は停止の後始末（下限 192MiB に掛かった後の残り 64MiB。C-6）でも走るので、値の深さに比例してスタックを使う形にすると、後始末の途中で溢れる。深さの上限は置かない（積みは確保器から伸ばし、上限を超える確保は確保器が `exit_group(70)` で止める）。
- **積み（作業の列）**: 項は `(段の関数の番地, 語)` の 16B。生成コードの静的データ（書ける、0 で始まる。単一スレッド前提。A-5）に、項 4096 個分の領域（64KiB）と、`top`（項の数）・`buf`（今の領域の番地）・`cap`（今の領域の項の数）の 3 語を置く。`top` が `cap` に達したら、`raw.alloc`（シム `sigil_rt_raw_alloc`）で 2 倍の領域を取り、中身を `memmove` で写し、前の領域が確保した領域なら `raw.free` で返す。`top` が 0 に戻ったとき、確保した領域を使っていれば返して静的な領域に戻す。こうすると、入口が終わった時点（`SIGIL_RT_LIVE` を出す時点）に積みの領域は残らず、`sigil_blocks=` の終了時の完全一致は変わらない。
  - 積みを伸ばす `raw.alloc` が失敗した場合（`mmap` の失敗・上限 2^46 超え）は、確保器が `exit_group(70)` で止める（A-4。シムの状態の検査も同じ 70。F-10 の 1）。これは意図どおりで、破棄を途中でやめて黙って続けることはしない。停止の後始末の途中の破棄も同じで、後始末は C-6 の下限の下に残る 64MiB の範囲で行う（積みはスタックを一定しか使わないので、この範囲に収まる）。
- **入口と取り出し**: `sigil_drop_<k>(w)` は、`w` が 0 なら何もしない。それ以外は、`base = top` を覚え、`(sigil_dstep_<k>, w)` を積み、`top > base` の間、項を 1 つ取り出してその段の関数を間接呼び出しで呼ぶ。`base` を覚えるのは入れ子の呼び出しへの備えで、今の段の関数から破棄の入口を呼ぶ経路はない（表示・解放のシムは生成した破棄を呼ばない）。
- **順序**: 段の関数は、値そのものを処理し（構造体なら G-3 の trace）、子（フィールド・要素）を**宣言の逆順に**積んでから、自分の塊を解放する。取り出しは後入れ先出しなので、子は宣言の順に、それぞれの子孫を先に処理しながら破棄される（深さ優先の前順）。今の Rust の `val_drop` と評価器の `drain` と同じ順である。積む前に子の語を読んでおく（塊の解放の後に読まない）。
  - 積むのは、0 でなく、型が破棄の対象（`aggregate` が真: 組・構造体・列挙型・一覧・`Cell`・`Shared`・関数の値・`String`・`Path`・`Bytes`）の子だけ（今の `push` の条件と同じ）。参照・スカラーは積まない。
  - 子が `String`・`Path`・`Bytes`（子孫を持たず、trace も出さない）なら、積まずにその場で解放してよい。観測できる順序（`drop:` の行）は変わらない。長い `List<String>` で積みが要素の数だけ伸びるのを避けるためである。
- **関数の値の環境**: クロージャの本体の +16 には、記述子ではなく、環境の組の型の段の関数 `sigil_dstep_<環境の型>` の番地を置く（`MakeClosure` を直す）。関数の値の段は、+8 の環境が 0 でなく、参照の数を 1 減らして 0 になったときだけ、`(+16 の番地, 環境)` を積んで本体を `blk_free` する（今の `K_FN` の腕と同じ）。+16 を読む Rust のコードは `val_drop` だけで、`val_show` の `K_FN` は `<関数>` を出すだけなので、表示には影響しない。
- **C-6 との関係**: 破棄はスタックを一定しか使わない。`eq`・`cmp`・`clone`・`shadow` の再帰は今の Rust と同じく下限の検査をせず、溢れは 3-7b より前は Rust のスレッドのガードページ、3-7b からは C-6 のガードページで止まる（黙って壊れない）。

#### G-3. `--trace-drops` と表示・記述子

- **出すかどうかはコンパイル時に決める**: `cg.trace` が偽なら、構造体の段に trace の命令を入れない。真なら入れる。Rust の `TRACE`・`sigil_rt_trace_on` と、`thunk` の `trace_on` の呼び出しを消す。
- **条件の移し方**（構造体の段の先頭、子を積む前）: 頭の第 1 語（中身 − 16）の SILENT（第 62 ビット）が立っていない、かつ、破棄の対象の型のフィールド（G-2 の積む条件と同じ型）のどれも語が 0（移動済み）でない、とき出す。フィールドの型は静的に分かるので、調べるフィールドの列は生成時に決まり、展開した比較の列になる。今の Rust の `moved` と同じ判定である。
- **出し方**: 表示は 3-5f まで Rust の `val_show` に任せる。段の関数は `rt("val_show", [構造体の記述子, w, 1])` で引用符つきの表示を作り、`rt("trace_drop", [s])`（`sigil_rt_trace_drop`。`rt_spec` に `("p", 'v')` で足し直す）で `drop: <表示>` の 1 行を標準エラーへ書き、`str_free(s)` で捨てる。`sigil_rt_trace_drop` は今の `val_drop` と同じく `stderr().lock()` へ `writeln!` するので、出力は 1 字も変わらない。
  - D-6 の「3-5e は `--trace-drops`（`trace_drop` を含む）」を改める: `trace_drop`（書き出し）は 3-5f へ移す。入力が `val_show` の文字列で、標準エラーへの全部を書き切るまでの書き込みの繰り返しは 3-6a の `console.log` の書き込みと同じ処理になるためで、3-5e で別に書くと 3-6a と重なる。3-5f は `val_show` を型ごとの生成にするときに、`trace_drop` を生成コードの書き込み（3-6a が合流していればその関数）にする。（補足 H-6 で改める: 3-6a が未合流のため、書き込みの補助 `sigil_write_all` は 3-5f で持ち、3-6a はそれを使う。）
- **記述子の表を残す範囲**: 3-5e の後も、記述子（`Cg::desc`）は `val_show`（表示・trace。3-5f まで）と `entry_args`/`coerce`（入口の引数。3-5g まで）のために、今の形（`K_*` の番号、`K_BYTES` = 15 を含む）のまま残す。3-5e で記述子を読まなくなるのは比較・複写・破棄だけで、クロージャの +16 は記述子でなくなる（G-2）。記述子の生成は 3-5g で使い手がなくなったときに消す。

#### G-4. 塊の頭と `Shared`/`Cell`

- **頭の配置は今と同じ**（C-1）: 塊は `16 + n` バイト（`n = max(bytes, 8)` を 8 の倍数に切り上げ）、頭は `[n | SILENT][参照の数]`、中身の先頭（頭 + 16）が値の語。`Cell`/`Shared` の中身は語 1 つ。`FREED` = `0x5afe_dead_0000_0001`、SILENT = `1 << 62`。Rust の `val_show` は中身だけを読む（`Shared`/`Cell` は `*(w)`）ので、頭の配置を Rust と揃え続ける必要はない。
- **codegen が生成する**（Sigil の `std.*` に置かない。**G-12 の 2**）:
  - `sigil_blk_new(bytes) -> 番地`（`Linkage::Local`）: 頭を `raw.alloc(16 + n, 8)`（シム）で取り、`n` と 1 を書いて頭 + 16 を返す。`rt("blk_new")` の呼び出しはこれを呼ぶ（呼び出し箇所の `self.blk` は変えない）。
  - `sigil_blk_free(p)`: `h = p − 16`。`h[1] == FREED` なら停止する（下の項）。違えば `h[1] = FREED` を書き、`raw.free(h, 16 + (h[0] & !SILENT), 8)`（シム）で返す。
  - 参照の数の増減・SILENT の設定は、関数にせず命令でその場に書く（`p − 8` の語の `+1`・`−1` と 0 との比較、`p − 16` の語の `| SILENT`）。
  - 理由: 頭の配置を知るのは codegen の 1 か所になる（作る `blk_new`、読む `dstep`・`clone`・`shadow` のすべてが生成コード）。Sigil に置くと、頭を読み書きする `unchecked` の関数（S2）と、参照の数の増減ごとのシムの呼び出しが増える。C-1 の「`shared_*` も型ごとに生成する」に合う。
- **確保器が替わる**: 塊は Rust・glibc の確保器ではなく、Sigil 製確保器（`std.os.alloc`）から取る。`sigil_blocks=` が塊を数え、`blocks=`（`LIVE_BLOCKS`）は増えなくなる（G-5・G-7）。
- **二重解放・解放後の使用の検出**（今の Rust と同じ点で調べる）:
  - `sigil_blk_free` の `FREED` の検査は、今と同じ位置（頭の第 2 語）・同じ値で行う。Sigil 製確保器の小さい塊の解放（`src/stdlib/os_alloc.sigil` の `small_free`）は、確保器の頭（返した番地 − 16 と − 8）に解放済みの印を書き、空きの連結を返した番地の語（= 塊の頭の第 1 語）に書き、第 2 語には触れない（`refill`・`small_alloc` も同じ。第 2 語を書くのは再利用した後の書き手だけ）。そのため、今（glibc。空きの管理が中身の第 2 語を上書きしうる）より `FREED` が残りやすい。確保器自身も解放済みの印（A-4）で二重の `free` を止めるので、検出は今より弱くならない。
  - 停止の仕方は `abort`（SIGABRT）から、`rt("stop", ..)`（`sigil_rt_stop`。報告を標準エラーへ出して `std::process::exit(EXIT_ABNORMAL_STOP)` = 70 で終わる。シムの停止の `report_stop` も同じ関数を呼ぶ。3-5g で生成コードへ移る）で `sigil: 解放済みの塊をもう一度解放した` を出して終了コード 70 に変える（A-4 の確保器の異常停止と同じ 70。生成コードには `abort` がない）。この経路の試験は今もない（わざと 2 回解放する版を手元で確かめる。G-9）。
  - `LIVE_BLOCKS` が負になったら `abort` する検査は、数え自体を消す（G-5）ので無くなる。その代わりは D-5・E-5 と同じく `sigil_blocks=` の 1000 回と 100000 回の完全一致である。
  - 解放後の使用（解放した `Shared` の参照の数の増減など）は、今の Rust も調べていない（`FREED` の語を増減するだけで止まらない）。本補足でも足さない。
- **Rust の `sigil_rt_shared_*` を消す**（3-5e-ii）: 使われていない（G-0）。単体試験 `shared_and_alloc` も消す（`sigil_rt_alloc`/`sigil_rt_free` の部分を含む。下の項）。
- **`sigil_rt_alloc`/`sigil_rt_free` を消す**（3-5e-ii）: 使い手は `blk_alloc`・`blk_free`・`sigil_rt_shared_*` だけで、どれも消える。`rt_spec` の `alloc`（呼び出し箇所がない）も消す。

#### G-5. `val_shadow`・`blk_new` と `LIVE_BLOCKS` の時期

- **`val_shadow` は 3-5e に含める**（3-5e-ii）: `clone` と同じ複写の生成（`deep` の真偽の違いだけ。G-1 の表）で、`Cell` の扱いと SILENT の設定のほかは同じ本体になる。分けると、同じ鍵の複写の生成器を 2 つの PR で作ることになる。
- **`blk_new` も 3-5e-ii に含める**: 塊を作る側（`blk_new`）と捨てる側（`blk_free`、生成した破棄）は同じ確保器でなければならず、`clone`・`shadow`・破棄・`Cell`/`Shared` の生成と一度に替える必要がある（D-6 の 3-5b-i と同じ理由）。
- **`LIVE_BLOCKS`・`blocks=`**: 3-5e-ii で `LIVE_BLOCKS` と `live_blocks` を消し、`sigil_rt_live_blocks` は常に 0 を返す（`sigil_rt_live_strs` と同じ扱い。D-5）。書式は 3-5e-iii まで変えない。3-5e-iii で `sigil_rt_live_strs`・`sigil_rt_live_blocks` ごと消し、書式を `live: sigil_blocks=<数>` にする（G-7）。

#### G-6. Rust の消費者の線引き

| Rust の関数・処理 | 3-5e での扱い | 移す PR |
|---|---|---|
| `val.rs` の `eq`・`cmp`・`sigil_rt_val_eq`・`sigil_rt_val_cmp`（`K_BYTES` の腕を含む） | 消す | 3-5e-i |
| `val.rs` の `dup`・`sigil_rt_val_clone`・`sigil_rt_val_shadow`・`sigil_rt_val_drop`・`TRACE`・`sigil_rt_trace_on`・`blk_alloc`・`blk_free`・`blk_silent`・`rc_inc`・`rc_dec`・`block`・`sigil_rt_blk_new`・`owning_kind`（`show_into` の移動済みの判定で使う分は残す） | 消す | 3-5e-ii |
| `lib.rs` の `sigil_rt_shared_*`・`sigil_rt_alloc`・`sigil_rt_free`・`LIVE_BLOCKS`・`live_blocks` | 消す | 3-5e-ii |
| `lib.rs` の `sigil_rt_live_strs`・`sigil_rt_live_blocks`・`sigil_rt_enter_with` の `live:` の行の組み立て | 書式を改める（G-7） | 3-5e-iii |
| `val.rs` の `show_into`・`sigil_rt_val_show`、`lib.rs` の `show_*`・`print_result`・`sigil_rt_trace_drop` | 残す（3-5e-ii から `trace_drop` を生成コードが呼ぶ） | 3-5f |
| `val.rs` の `coerce`・`entry_args`・`kind_name`、`lib.rs` の `arg_*`・`report`・`stop`・`exit` | 残す | 3-5g |
| `lib.rs` の `stack_*`・`enter`・`enter_with` | 残す | 3-7b |
| 関数表（`set_str_ops`・`set_list_ops`・`set_bytes_ops`）とその Rust の包み | 表の形（5 項目）は変えない。Rust の側で使われなくなった包み（`list_free`・`bytes_clone`・`bytes_free`・`str_clone` など）は、`clippy -D warnings` の未使用の警告に合わせて消す | 3-7d（表そのもの） |

- 3-5e の後に Rust が記述子を読むのは `show_into`（3-5f）と `coerce`（3-5g）だけである。どちらも `List` の要素を関数表の `len`・`get` で読み、`coerce` は `create`・`push` で作る。

#### G-7. `SIGIL_RT_LIVE` の出す主体と既存の試験

- **出す主体**: C-5 のとおり、3-7b より前は Rust の `sigil_rt_enter_with`、3-7b からは生成コード（起動の処理）。3-5e-iii と 3-7b は**どちらが先に合流してもよい**（C-5 の「3-5e は 3-7b の後」を改める）。
  - 3-5e-iii が先: `sigil_rt_enter_with` の行を `live: sigil_blocks=<数>` に改め、`sigil_rt_live_strs`・`sigil_rt_live_blocks` を消す。後の 3-7b は、生成コードで生存数のシム（`sigil_rt_sigil_live_blocks`）だけを呼び、同じ書式で出す。
  - 3-7b が先: 3-7b は C-5 のとおり今の書式で、Rust の `sigil_rt_live_strs`・`sigil_rt_live_blocks` を呼んで出す。後の 3-5e-iii は、生成コードの出力を `live: sigil_blocks=<数>` に改め、その 2 つの Rust の関数を消す。
  - 理由: 書式の変更と出す主体の移動は互いに独立で、どちらの順でも 1 か所を直すだけで済む（D-3・E-3 の関数表の登録と同じ「後から合流する方が持っていく」）。
- **生存数の関数が未登録のとき**: 3-7a からすべてのネイティブのプログラムが確保器を結ぶ（C-2）ので起きない。起きた場合は `live:` の行を出さない（試験が「`live` の行がない」で失敗して見つかる。偽の 0 を出さない）。
- **既存の試験の期待**（3-5e-iii で直す。本補足を根拠に、既存の試験ファイルを該当の PR で変える。D-7 の最後の項と同じ例外）:
  - `tests/m5_free_strings.rs`・`tests/fn_types_2_4d_exec.rs`: `live()` の `num("strs=")` の検査と `blocks=` の戻りを外し、`sigil_blocks=` だけを返す。期待は「1000 回と 100000 回で `sigil_blocks=` が完全に一致し、100 未満」にする。3-5e-ii で組・構造体・列挙型・クロージャの塊が `blocks=` から `sigil_blocks=` へ移るので、`fn_types_2_4d_exec` の `stored_closure_drop_does_not_grow_with_the_iteration_count`（クロージャの塊）は 3-5e-ii から `sigil_blocks=` の比較で漏れを見る形になる（`(s1, b1) == (s2, b2)` の比較のままでも成り立つ。`b` は 0 になる）。
  - `"strs=0 blocks=0 sigil_blocks=<数>"` の完全一致を持つ `tests/sigil_alloc_34i.rs`・`tests/str_35b.rs`・`tests/list_35c.rs`・`tests/bytes_35d.rs`・`tests/bytes_35dii.rs`・`tests/bytes_35diii.rs` は、`"sigil_blocks=<数>"` に機械的に直す（数は変えない。`num("strs=")`・`num("blocks=")` の検査も外す）。
  - `tests/globals_backend_34g.rs` の `globals_leave_no_live_blocks_at_exit` は、大域の組・構造体の塊が 3-5e-ii で `sigil_blocks=` へ移るので、3-5e-ii で期待を `sigil_blocks=0` を含むことに改める（`blocks=0` だけを見ると、`sigil_blocks=0` の部分文字列にも当たって検査が働かなくなる）。3-5e-iii で行全体の一致 `sigil_blocks=0` にする。
  - 3-5e-ii の時点で、終了時の `sigil_blocks=<数>` の完全一致の期待は、終了時に組・構造体などの塊が残らない限り変わらない見込みである（今の期待はすべて `blocks=0`）。変わる場合は、実装の PR に理由を書く。
  - `tests/request_isolation_4_2f.rs` の `live: frames=0 chunks=0` は別の数（評価器の要求の分離）で、変えない。

#### G-8. 契約変更の有無

- `src/ir.rs`（`Builtin`・`Ty`・`Rvalue`・`Term`）・`src/ast.rs`・`RAW_OPS` は変えない。生成するのは codegen の中の関数とシムだけで、lowering も評価器も変えない。順序は「本補足 G（ADR）→ 3-5e-i → 3-5e-ii → 3-5e-iii」。
- 深い比較を評価器と同じ `StackExhausted` の停止にする（比較の式に停止の場所を持たせる）のは契約変更になるので、本補足の範囲で行わない（G-1・G-10）。行うなら `src/ir.rs` だけの `contract-change` の PR を先に出す。

#### G-9. 分割と完了基準

実装担当の提案の 3 段（i: 比較、ii: 複写・破棄・塊・trace、iii: `SIGIL_RT_LIVE`）は妥当なので、そのまま採る。理由: i は読むだけの処理で塊の確保器に触れず、単独で戻せる。ii の複写・破棄・`blk_new`・`Cell`/`Shared` は同じ確保器で作って捨てる組なので分けられない（G-5）。iii は書式の機械的な直しで試験ファイルを多く触るので、意味の変わる ii と分けた方がレビューしやすい。

| PR | 範囲 | 依存 | 要承認 |
|---|---|---|---|
| 補足 G | 本補足（docs のみ）。C-5・C-7・D-3・D-6 の 3-5e の記述に注を付ける（G-11） | 補足 F | 不要（ADR） |
| 3-5e-i | `sigil_eq_<k>`・`sigil_cmp_<k>` の生成（G-1）と `compare`・`ListMin`/`ListMax`・`ListMaxBy`/`ListMinBy` の付け替え。シム `str_eq`・`str_cmp`・`bytes_eq`・`bytes_cmp` を足す。`rt_spec` から `val_eq`・`val_cmp` を消し、Rust の `eq`・`cmp`・`sigil_rt_val_eq`・`sigil_rt_val_cmp`（`K_BYTES` の腕を含む）を消す。`tests/val_35e.rs` を足す | 補足 G（と合流済みの 3-5d-iii） | 不要（`unchecked` を足さない。`std.core.string`・`std.core.bytes` の `eq`・`cmp` は既にある。言語の振る舞いは G-10 の `Addr` のほか変えない。生成器の変更は通常のレビュー。C-1） |
| 3-5e-ii | `sigil_clone_<k>`・`sigil_shadow_<k>`・`sigil_drop_<k>`・`sigil_dstep_<k>`・`sigil_blk_new`・`sigil_blk_free` と積み（G-1・G-2・G-4）、クロージャの +16 の段の関数の番地への変更、`--trace-drops` の生成（G-3。`sigil_rt_trace_drop` を `rt_spec` に足し直す）。G-6 の 3-5e-ii の行の Rust を消し、`sigil_rt_live_blocks` を常に 0 にする。`globals_backend_34g` の期待を直す（G-7） | 3-5e-i | 不要（`unchecked` を足さない。`--trace-drops` の出力・`sigil spec` は変わらない。塊の確保器が Sigil 製に替わるのは 3-7a で承認済みの方針（C-2）の帰結） |
| 3-5e-iii | `live: sigil_blocks=<数>` への書式の変更と `sigil_rt_live_strs`・`sigil_rt_live_blocks` の削除（G-7。出す主体は合流の時点のもの）、既存の試験の期待の機械的な更新 | 3-5e-ii（3-7b とはどちらが先でもよい） | 不要（書式は C-5 で承認済み） |

- **各 PR の完了基準**（C-7 の共通の基準に加えて）:
  - 3-5e-i: 既存の全件の突き合わせと `scripts/native-report.sh` が通る。`tests/val_35e.rs` で、`==`・`!=`・`<`・`<=`・`>`・`>=` と `min`/`max`/`max_by`/`min_by` を、評価器とネイティブで突き合わせる（検査器が許す組み合わせだけ）: `Int`（最小・最大）、`Float`（`-0.0` と `0.0`、NaN、`inf`）、`Bool`、`String`（空・多バイト・接頭辞の関係）、`Path`（`==`）、`Bytes`（空・接頭辞・255 と 0）、`List`（空・接頭辞・長さ違い・NaN を含む要素）、組（NaN の要素で 2 が伝わること。先の要素で決まれば後の NaN を見ないこと）、構造体と列挙型の `==`（腕の違い・フィールドの違い・`Option`・`Result`）、`Shared`/`Cell` の `==`（同じ塊・別の塊で中身が同じ）、参照を含む組、`Addr` を含む組（G-10）、再帰する列挙型（`L.Cons` の連鎖。1000 段程度）。`rt_spec` から `val_eq`・`val_cmp` が消え、`runtime/core` に `fn eq(`・`fn cmp(` が残っていない。
  - 3-5e-ii: 上に加えて、(1) 評価器とネイティブの `drop:` の行を突き合わせる既存の試験（`tests/m3_drop_order.rs`・`tests/m3_drop_limits.rs`・`tests/fn_types_2_4d_exec.rs` など、`common::native::native_drops` を使うもの）が通る。(2) `tests/val_35e.rs` に、ネイティブで 300 万段の `L.Cons` の連鎖を作って捨てても溢れないこと（評価器の `very_deep_value_is_dropped_without_overflowing_the_host_stack` と同じ入力）と、積みが 4096 項を超えて確保した領域へ移り、戻ったときに返されること（幅の広い値: 要素が組の 1 万要素の一覧の入れ子など。終了時の `sigil_blocks=` で確かめる）を置く。(3) `clone` と影の違い（構造体の `clone` は破棄で `drop:` を出し、影は出さない。`Cell` の `clone` は独立し、影は共有する）と、`Shared`/`Cell` の参照の数（複写の後に片方を捨てても中身が残り、最後の 1 つで捨てられる）を、評価器とネイティブの `drop:` の行と値で突き合わせる。(4) 組・構造体・列挙型・`Cell`・`Shared`・クロージャ（環境あり・なし、`move` で捕まえた文字列）・`List<組>` を作って捨てる繰り返しを 1000 回と 100000 回で走らせ、`sigil_blocks=` が完全に一致し、`blocks=` が 0 であること。わざと `blk_free` を 1 つ抜いた版と、2 回にした版（`sigil_blk_free` の `FREED` の検査で終了コード 70 と文言が出ること）で失敗することを手元で確かめ、PR に結果を書く（D-5 の 1 と同じ）。(5) `rt_spec` から `val_clone`・`val_shadow`・`val_drop`・`blk_new`（`rt` の名前はシムとして残る）・`trace_on`・`alloc` が消え、`runtime/core` に `sigil_rt_val_drop`・`sigil_rt_shared_`・`sigil_rt_alloc`・`LIVE_BLOCKS` がない（`grep`）。`runtime/core` の単体の `cargo test` と `tests/runtime_parity.rs` が、未定義のシンボルなしにリンクして通る。
  - 3-5e-iii: 上に加えて、`SIGIL_RT_LIVE` の行が `live: sigil_blocks=<数>` と完全に一致し、`strs=`・`blocks=` を含まないことの試験がある（`tests/val_35e.rs`）。G-7 の試験ファイルがすべて新しい書式で通る。`runtime/core` に `sigil_rt_live_strs`・`sigil_rt_live_blocks` がない。3-7b が先に合流していた場合は、生成コードの出力で同じことを確かめる。
- **`unchecked` の要否**: 3 つとも要らない。埋め込みの `std.*` に関数を足さない（使う `eq`・`cmp`・`clone`・`free` は 3-5b-ii・3-5c・3-5d で足して S2 のレビューを経た）。生成コードは Sigil の `unchecked` の関数ではないので、CI の信頼基盤の差分の検出には掛からない（C-1）。生成器（`src/codegen/**`）の変更は通常のレビューで見る。
- **試験の置き場所**: `tests/val_35e.rs`（新しいファイル）に、3-5e の 3 つの PR の試験を置く。駆動は `tests/list_35c.rs`・`tests/bytes_35d.rs` と同じく、評価器（`ir_eval::run_with`）とネイティブ（`common::native`）の両方で走らせる。既存の試験ファイルの変更は G-7 に挙げたものだけとする。
- **並行と順序**: 3-5e-i・ii・iii は直列にする（ii は i で作る生成の仕組み（鍵の表と待ち行列）を使い、iii は ii で `blocks=` が 0 になった後の書式の変更）。3-5e-i・ii は 3-7b・3-6a と並行してよい（起動の経路と `console.log` に触れない）。3-5f は 3-5e-ii の後にする（`trace_drop` を移すため。3-5e-iii とは並行してよい）。

#### G-10. 既知の差

- **`Addr` の比較**: 今のネイティブは、組・構造体などの中の `Addr`（記述子の種類 16）を `val_eq` で偽、`val_cmp` で比べられない（2）としており、評価器（番地の値で比べる）と食い違う。3-5e-i で生成する `eq`・`cmp` は評価器に合わせて番地の値で比べる（ネイティブの不具合の修正。`tests/val_35e.rs` に回帰試験を置く）。
- **深い比較・複写**: 評価器は深い `==`・`<` を `StackExhausted` の停止にするが、ネイティブは今と同じく再帰し、スタックの端（ガードページ）で SIGSEGV になる（G-1）。複写（`clone`・影）は評価器も再帰する。差は今のまま残し、試験は評価器の停止を基準にしない（ネイティブの深い比較の試験は置かない）。
- **二重解放の停止の仕方**: `abort`（SIGABRT）から終了コード 70 に変わる（G-4）。どちらも異常終了で、言語の値は変わらない。
- **`--trace-drops` の出し方（3-5e-ii で G-3 から変更）**: G-3 は `val_show` の文字列を `trace_drop` に渡す形を想定したが、表示の文字列を Sigil の関数で作ると、スタックの下限の付近（枯渇の停止の途中）で `new_lossy` の検査に掛かって 70 で止まり、枠の破棄の記録が途切れる（`tests/m3_drop_limits.rs`）。そのため `sigil_rt_trace_drop(記述子, 値)` が Rust の中で直接 `drop: <表示>` を書く形にした（表示の文言と順序は同じ）。
- **確保器の中の呼び出しのスタック検査（3-5e-ii で追加）**: 塊が Sigil の確保器に移ったので、確保器の中の呼び出し（`std.os.alloc.*` どうし）でスタックの検査をすると、確保の途中で `exit_group(70)` になり、利用者の枠の破棄（巻き戻し）が行われない。確保器は下限の外の残りの範囲で一定量しか使わないので、確保器の中の呼び出しは検査しない。
- **確保器の表の索引（3-5e-ii で追加）**: 解放のたびに表（ページと大ブロックの一覧）を先頭から線形に探すと、塊が確保器に移ったことで、生きているページの数に比例して遅くなる（300 万段の連鎖の破棄で数百秒。`tests/m3_drop_limits.rs` の native 側）。表の項目の並び（ページ・大ブロックの番地と印）は変えず、番地から項目を引く索引（開番地法。項目の 2 倍の大きさ）を足した。確保器の仕様（A-4・A-5）と検査の内容は変えない。

#### G-11. 既存の記述との整合（本補足の PR で直したもの）

1. **C-5 の「3-5e（3-7a と 3-7b の両方の後）」**: 3-5e-iii と 3-7b はどちらが先でもよい、と注を付けた（G-7）。
2. **C-7・D-6 の表の 3-5e の行**: 「補足 G で置換: 3-5e-i・3-5e-ii・3-5e-iii に分割。G-9」の注を付けた。依存の 3-7b は 3-5e-iii についてだけ「どちらが先でもよい」とし、i・ii は 3-7b に依存しない。D-6 の「`--trace-drops`（`trace_drop` を含む）」の `trace_drop` は 3-5f へ移す（G-3）。「Rust の `sigil_rt_str_cmp` を消す」は既に済んでいる（G-0）。
3. **D-3 の「Rust の `sigil_rt_str_cmp`（生成コードからは呼ばれていない）は 3-5e で消す」**: 既に無い（G-0）と注を付けた。

#### G-12. 選択肢と推奨（本文はすべて推奨案で書いた）

1. **破棄の形**（G-2）: 生成コードの積みによる反復（**推奨**。評価器と今の Rust と同じ順で、深さによらず後始末でも溢れない）／型ごとの再帰（単純だが、300 万段の連鎖や停止の後始末で溢れる）／再帰に深さの上限を付け、超えたら停止（評価器と結果が変わる）。
2. **塊の頭と参照の数の操作の置き場所**（G-4）: codegen が `blk_new`・`blk_free` を関数として、参照の数を命令として生成する（**推奨**。配置を知るのが codegen の 1 か所。`unchecked` が要らない）／Sigil の `std.core.shared` に置く（頭を読み書きする `unchecked` と、増減ごとのシムの呼び出しが増える）。
3. **積みの領域**（G-2）: 静的な 4096 項＋あふれたら確保器から 2 倍で伸ばし、空になったら返す（**推奨**。普段は確保しない。終了時の `sigil_blocks=` が変わらない）／毎回の破棄で確保する（確保と解放が破棄のたびに増える）／確保した領域を返さずに持ち続ける（終了時の `sigil_blocks=` が入力によって変わる）。
4. **trace の書き出し**（G-3）: 3-5f まで Rust の `sigil_rt_trace_drop` を呼ぶ（**推奨**。出力が 1 字も変わらず、書き込みの繰り返しを 3-6a と重ねて書かない）／3-5e で生成コードが `write` を直接発行する（部分書き込みと EINTR の扱いを 3-6a と別に書くことになる）。
5. **3-5e の分割**（G-9）: 実装担当の提案の 3 段（**推奨**）／ii を複写（`clone`・影）と破棄・塊に分ける（複写だけを先にすると、Rust の `blk_new` で作った塊を生成した複写が作り、Rust の `val_drop` が捨てる中間の形になり、確保器を替えるのは結局 ii の後半で一度に行う必要がある。分けるなら ii の中のコミットの順にとどめる）／ii と iii をまとめる（意味の変わる変更と多数の試験ファイルの機械的な直しが混ざる）。
6. **`eq`・`cmp` の戻りの幅**（G-1）: `i64`（**推奨**。生成コードの語と同じ。−1 と 2 をそのまま返せる）／今の `rt` と同じ 32 ビット（呼び出し側は変えずに済むが、生成する関数の中で切り詰める命令が増える）。

### 補足 H（PR 3-5f の前）: 表示の生成と 3-5f の分割

- **日付**: 2026-11-06
- **状態**: 提案（推奨案で本文を書いた。細目の選択は H-11 に推奨つきで並べ、異論があれば本補足の修正 PR で直す）。本文・補足 A〜G と食い違う箇所は、本補足が置き換える。
- 3-5f（`val_show`・`show_*`・`print_result`・`sigil_rt_trace_drop` の生成コードへの移行。C-7・D-6・G-6）の着手の前（main `d6fdf0f`。3-5e-ii は合流済み、3-5e-iii・3-6a は未合流）に、生成する関数の形・文字列の組み立て方・`Float` の最短表現のアルゴリズム・書き出しの経路が決まっていないので、`runtime/core/src/val.rs`・`lib.rs` と codegen の呼び出し箇所を棚卸しして、それらを決め、分割を決める。

#### H-0. 現状の確認（main `d6fdf0f`）

- **生成コードが呼ぶ Rust の表示**（`rt_spec`）: `val_show`（`("ppi", 'p')`。呼び出しは `src/codegen/func/call.rs` の `ConsoleLog`（引用なし `q = 0`）と UTF-8 のエラーメッセージの合成（`Int` のみ、`q = 0`）、`src/codegen/func/mod.rs` の入口の戻り値（`q = 1`））、`print_result`（`("p", 'v')`。入口だけ）、`trace_drop`（`("pp", 'v')`。3-5e-ii で生成した破棄の段が呼ぶ）。
- **`show_into`**（`runtime/core/src/val.rs`）: 記述子を再帰で読む表示。文言の正は評価器の `show_val`（`src/ir_eval/value.rs`）で、両者は H-10 の `Addr` を除いて揃っている。規則は H-1 の表のとおり（本補足で写す）。戻り値は Rust の `String` から `new_str` で作る文字列で、呼び出し側が `str_free` で捨てる。
- **`show_*`**（`sigil_rt_show_int`・`show_float`・`show_bool`・`show_unit`・`show_str`。`runtime/core/src/lib.rs`）: 生成コードからも `rt_spec` からも呼ばれていない（残っているのは `runtime/core` 自身の単体試験だけ）。G-6 の表は 3-5f の行に数えているので、本補足では 3-5f で消すものとして扱う（H-8）。
- **`Float` の表示**: `text::fmt_float`（`src/shared/text.rs`。`runtime/core` は写しを持つ）: `NaN`・`inf`・`-inf` のほかは Rust の `format!("{f}")`（最短往復の 10 進。指数表記を使わない）で、小数点がなければ `.0` を足す。評価器も同じ関数を使う。
- **文字列の引用**: `text::quote` = Rust の `format!("{s:?}")`（`char::escape_debug` の規則。H-4）。
- **`print_result`**（`lib.rs`）: 表示の文字列に `\n` を足し、`stdout().lock()` へ `write_all` と `flush` をする（エラーは捨てる）。
- **`sigil_rt_trace_drop`**（`val.rs`）: Rust の中で `show_into`（引用あり）で表示を作り、`stderr().lock()` へ `drop: <表示>` を 1 行書く（G-10 の変更でこの形になった）。
- **記述子**（`Cg::desc`）: 3-5e の後も、表示（3-5f まで）と `entry_args`/`coerce`（入口の引数。3-5g まで）が読む（G-3）。

#### H-1. 生成する関数の形と表示の規則

- **作る関数**: 型 `T` ごとに `sigil_show_<k>(w, quote)` を 1 つ、必要になったものだけ生成する。G-1 の生成の仕組み（鍵 `norm_with(strip(ty), true)`・宣言と待ち行列・通し番号）をそのまま使い、`Gk` に `Show` を足す。すべて `Linkage::Local`、C の呼び出し規約で、戻り値は持たず、H-2 のバッファへ追記する（今の `show_into` の `out: &mut String` と同じ形）。停止しない（確保の失敗は確保器とシムが `exit_group(70)` で止める。G-1 と同じ）。
- **呼び出し箇所の形**: 呼び出し側は、生成した補助 `sigil_show_begin()` でバッファを空にしてから `sigil_show_<k>(w, q)` を呼び、使い道で分ける。
  - そのまま書く（`console.log`・入口の戻り値・`trace_drop`）: バッファの番地と長さを H-6 の書き出しへ渡し、`sigil_show_end()` で片付ける。文字列は作らない。
  - 文字列が要る（UTF-8 のエラーメッセージの合成）: `sigil_show_end_str()` が、バッファの中身から `std.core.string` の `new`（検証なし。表示のバイト列は構成上つねに正しい UTF-8）のシムで文字列を作って返す。呼び出し側は今と同じく `str_free` で捨てる。
- **種類ごとの規則**（今の `show_into` と評価器の `show_val` を写す。H-10 の `Addr` を除いて変えない）:

| 型 | 表示 |
|---|---|
| `Unit` | `()` |
| `Int` | 10 進（H-5） |
| `Float` | H-3 |
| `Bool` | `true` / `false` |
| `String`・`Path` | 引用ありなら H-4 の引用、でなければ中身そのまま |
| `Bytes` | `Bytes([` + 各バイトの 10 進を `, ` でつなぐ + `])` |
| 秘密の構造体（記述子の種類 17） | `Secret(***)`（中身も長さも出さない） |
| `ParseError` | `ParseError` |
| `Addr` | `Addr(<10 進>)`（H-10） |
| `List<T>` | `[` + 要素（引用あり）を `, ` でつなぐ + `]` |
| 組 | `(` + フィールド（引用あり）を `, ` でつなぐ + `)` |
| 構造体 | `名前 { フィールド名: 値, ... }`（値は引用あり） |
| 列挙型 | `型名.腕名(値, ...)`。型名が `Option`・`Result` のときだけ型名と `.` を省く（今の記述子の印と同じ規則）。フィールドがなければ `(...)` を付けない |
| `Ref<T>` | 指す値を、受けた `quote` のまま |
| `Shared<T>` | `Shared(` + 中身（引用あり） + `)` |
| `Cell<T>` | `Cell(` + 中身（引用あり） + `)` |
| 関数の値 | `<関数>` |
| 移動済み（所有する種類・`String`・`Path`・`Bytes` で語が 0） | `()` |

- **名前の置き場所**: 構造体名・フィールド名・列挙型名・腕名は、記述子から読まず、型定義（`Program.types`）から生成時に文字列リテラルとして埋める。3-5f の後、表示は記述子を読まない（H-7）。
- **入れ子の深さ**: `eq`・`cmp` と同じく値の入れ子に沿って再帰し、深さの検査はしない（G-1 の方針。評価器の `show_val` も再帰する）。

#### H-2. バッファ

- 生成コードの静的データに、番地・長さ・容量の 3 語と、最初の領域 4096 バイトを置く（G-2 の破棄の積みと同じ作り。単一スレッド前提。A-5）。
- 追記で容量が足りなければ、`raw.alloc`（シム `sigil_rt_raw_alloc`）で 2 倍の領域を取り、`memmove` で写し、前の領域が確保した領域なら `raw.free` で返す。
- `sigil_show_end` / `sigil_show_end_str` の時点で長さを 0 に戻し、確保した領域を使っていれば返して静的な領域に戻す。入口が終わった時点（`SIGIL_RT_LIVE` を出す時点）にバッファの領域は残らず、`sigil_blocks=` の終了時の完全一致は変わらない（G-2 と同じ理由）。
- 表示は利用者のコードを呼ばないので、表示の最中に表示が入れ子になることはない（`console.log` の引数の表示中に利用者の関数が走らず、`trace_drop` からの確保も Sigil の関数を呼ばず破棄を起こさない）。そのため、破棄の積みの `base` のような入れ子対策は要らない。
- バッファを伸ばす `raw.alloc` の失敗は、確保器が `exit_group(70)` で止める（G-2 と同じ。黙って切れた表示を書くことはしない）。

#### H-3. `Float` の最短表現

- **意味**（`fmt_float` を写す。変えない）: `NaN` → `NaN`、正の無限大 → `inf`、負の無限大 → `-inf`。それ以外は Rust の `format!("{f}")` と同じ文字列: その文字列を最近接・偶数丸めで読むと元の `f64` に戻る最短の 10 進列（最短が複数あれば元の値に最も近いもの、なお同点なら最後の桁が偶数のもの）を、指数表記を使わずに書き、小数点が含まれなければ末尾に `.0` を足す。`-0.0` は `-0.0`、`0.0` は `0.0` になる。
- **アルゴリズム**: 正確な多倍長演算による区間方式（Steele & White の方法）。`f = m × 2^e` の `m`・`e` から、`f` に戻る実数の区間（下限と上限。境界を含むかどうかは仮数の偶数と丸めの規則で決まる）を多倍長の整数で正確に表し、区間に入る最短の 10 進列を桁ごとに決める。10 進文字列を `f64` に戻す解析は使わない（`Float` の解析は 3-5g の範囲）。
  - 多倍長は固定長で足りる: 必要な大きさは高々約 1100 ビット（仮数の規模と、区間の端を 10 進の桁に合わせる拡大分）。生成コードの中の固定長の配列（64 ビット × 24 語 = 1536 ビット）で表し、使う演算は加減・10 倍・比較と、1 語ずつの 10 での割り算（剰余つき）だけにする。
  - 生成するのは型ごとの関数ではなく、1 つの補助 `sigil_show_float(f)`（バッファへ追記）とする。H-1 の `Float` の腕はこれを呼ぶ。
- **置き場所**: codegen の生成コード（Sigil の `std.*` に置かない）。理由は G-4 と同じで、表示は `trace_drop`（H-6）や入口など Sigil の関数を呼べない場所から使われ、埋め込みの標準ライブラリに足すと信頼基盤のレビュー（S2）が要るため。`text::fmt_float`（`src/shared/text.rs`）は、評価器と処理系自身の表示のために残す。
- **検証**: 評価器（Rust の `{}`）との突き合わせが正しさの基準。H-9 の行列（境界値・最小の非正規化数・最大値・往復で知られた値）に加え、10 進と 2 進の往復が壊れやすい値（最小の非正規化数のまわり、2 の冪の境界、その前後）を必ず含める。

#### H-4. 文字列の引用

- Rust の `{:?}`（`char::escape_debug`）と同じ規則: `"` → `\"`、`\` → `\\`、改行 → `\n`、復帰 → `\r`、タブ → `\t`。それ以外の文字は、Rust の `char::escape_debug` が使う印刷可否の判定で印刷できないものだけを `\u{...}`（16 進・小文字・先行ゼロなし）にする。印刷できる文字はそのままバッファへ写す。
- 判定は符号点ごとに行う。生成コードは文字列を UTF-8 として走査し、符号点を復号して判定し、エスケープしない文字は元のバイト列をそのまま写す（再符号化しない）。
- 検証は H-9 の行列で、制御文字（U+0000〜U+001F・U+007F）、U+0085、U+00AD、結合文字、U+200B、U+FEFF、私用領域、面 1 以降の文字を含め、評価器の `{:?}` と突き合わせる。

#### H-5. `Int`・`Bytes`・そのほかのスカラー

- `Int`: 10 進。負は `-` を先に書き、絶対値を桁ごとに書く（`i64` の最小値でも溢れないよう、符号なしに直してから割る）。補助 `sigil_show_int(n)`（バッファへ追記）を 1 つ生成し、H-1 の `Int` の腕と `Bytes` の各バイトと `Addr` が使う。
- `Bytes`: H-1 の表のとおり（今の `format!("Bytes({:?})", bytes)` と同じ）。
- `Bool`・`Unit`・`ParseError`・秘密・関数の値の表示は、文字列リテラルをそのままバッファへ写す。

#### H-6. 書き出し（`print_result` と `trace_drop`）

- 生成した補助 `sigil_write_all(fd, ptr, len)`: `raw_syscall`（`rt_spec` の `raw_syscall`）で `write`（番号 1）を、全部を書き終えるまで繰り返す。部分書き込みは残りから続け、`-4`（EINTR）は同じ位置から再試行し、その他の負の値では書くのをやめる（3-6a の `console.log` の規則と同じで、今の Rust の `write_all` + エラーを捨てる扱いに等しい）。戻り値は持たず、エラーは報告しない（今の `print_result`・`sigil_rt_trace_drop` と同じ）。
- **入口の戻り値**（`src/codegen/func/mod.rs`）: `sigil_show_begin` → `sigil_show_<戻り値の型>(w, 1)`（引用あり。今と同じ）→ バッファを fd 1 へ `sigil_write_all` → `\n` を fd 1 へ → `sigil_show_end`、とする。`print_result` の `rt` 呼び出しは消える。今の Rust は標準出力のバッファを `flush` するが、生成コードはバッファを持たないので、出力の内容と順序は変わらない。
- **`trace_drop`**（生成した破棄の段）: 構造体の段は、`sigil_show_begin` → `sigil_show_<構造体の型>(w, 1)` → `drop: ` とバッファと `\n` を fd 2 へ → `sigil_show_end`、とする。`rt_spec` の `trace_drop`（記述子と値を取る形）は消える。
  - G-10 で `sigil_rt_trace_drop` を Rust の中にした理由（スタックの下限の付近で、Sigil の関数による表示が検査に掛かって枠の破棄の記録が途切れる）は、生成した表示には当てはまらない: 生成した `sigil_show_<k>` は Sigil の関数ではなく、スタックの下限の検査も `new_lossy` の検証も行わない。使うのはバッファの確保（スタックは一定量しか使わない）だけで、C-6 の後始末の範囲に収まる。表示の文言と順序は今の Rust と同じにする。
- **`console.log`**（`call.rs` の `ConsoleLog`）: `sigil_show_begin` → `sigil_show_<引数の型>(v, 0)`（引用なし。今と同じ）→ バッファの番地と長さを今の `console_log` の `rt` へ渡す → `sigil_show_end`、とする。`console_log` 自体の生成コード化は 3-6a の範囲で、本補足では触らない。G-3 の「`trace_drop` の書き込みは、3-6a が合流していればその関数にする」は、3-6a が未合流なので、`sigil_write_all` を 3-5f で持ち、3-6a はそれを使う形に改める（H-10）。

#### H-7. 記述子の残す範囲

- 3-5f の後、記述子（`Cg::desc`）を読むのは `entry_args`/`coerce`（入口の引数。3-5g まで）だけになる（G-3 の予定どおり）。表示のために `desc_addr` を呼ぶ箇所（`ConsoleLog`・UTF-8 のエラーメッセージ・入口・破棄の段）はすべて無くなる。記述子の生成そのものは、3-5g で使い手がなくなったときに消す（G-3）。
- **補足 I-7 で改める**: 3-5f-ii の後、`trace_struct` も記述子をオブジェクトへ残すため `desc` を呼ぶようになった（PR #303）。この呼び出しも、記述子の生成と一緒に 3-5g で消す。`tests/ct_rules.rs` の `clif_secret_descriptor_is_zeroizing_kind` の扱いも補足 I-7 にある。

#### H-8. Rust の消費者の線引き

| Rust の関数・処理 | 3-5f での扱い |
|---|---|
| `val.rs` の `show_into`・`sigil_rt_val_show` | 消す |
| `val.rs` の `sigil_rt_trace_drop` | 消す（H-6） |
| `lib.rs` の `show_*`（`show_int`・`show_float`・`show_bool`・`show_unit`・`show_str`） | 消す（生成コードから呼ばれていない。H-0）。その単体試験も消す |
| `lib.rs` の `sigil_rt_print_result`・`write_out` | 消す（H-6） |
| `src/shared/text.rs` の `fmt_float`・`quote` | 残す（評価器と処理系自身が使う） |
| `runtime/core` の `text` の写しの `fmt_float`・`quote` | 使う者（`val.rs`・`lib.rs`）が消えた分は、未使用の警告に合わせて消す |
| 関数表（`set_str_ops` 等） | 変えない（表示は関数表を経由しない） |

- `rt_spec` から `val_show`・`print_result`・`trace_drop` を消す。

#### H-9. 分割と完了基準

| PR | 範囲 | 依存 | 要承認 |
|---|---|---|---|
| 補足 H | 本補足（docs のみ）。G-3 へ H-10 の注を付ける | 補足 G | 不要（ADR） |
| 3-5f-i | 土台とスカラー: バッファ（H-2）と `sigil_write_all`（H-6）、`sigil_show_float`（H-3）・`sigil_show_int`（H-5）・引用（H-4）の補助の生成。入口の戻り値の表示を生成へ替える（H-6）。`print_result` と `show_*` の Rust を消す。`tests/show_35f.rs` を足す（`Float` と入口の表示の分） | 補足 H | 不要（言語の振る舞いは変えない。表示の文言は評価器と一致させる） |
| 3-5f-ii | `sigil_show_<k>` の生成（H-1）と呼び出し箇所の付け替え（`ConsoleLog`・UTF-8 のエラーメッセージ・破棄の段の `trace_drop`）。`val_show`・`trace_drop` を `rt_spec` から消し、`show_into`・`sigil_rt_val_show`・`sigil_rt_trace_drop` の Rust を消す。`tests/show_35f.rs` に全型の突き合わせを足す | 3-5f-i | 不要（同上） |

- **各 PR の完了基準**（C-7 の共通の基準に加えて）:
  - 共通: 既存の全件の突き合わせと `scripts/native-report.sh` が通る。`--trace-drops` の `drop:` の行を突き合わせる既存の試験（`tests/m3_drop_order.rs`・`tests/m3_drop_limits.rs` など）が変わらず通る（3-5f-ii）。
  - 3-5f-i: `tests/show_35f.rs` で、入口の戻り値の表示を評価器とネイティブで突き合わせる: `Float` の行列（`0.0`・`-0.0`・`0.1`・`1.5`・最大値・最小の正規化数・最小の非正規化数・`1e22`・`1e23`・2 の冪とその前後・整数に近い値・小数点以下が長くなる値）、`Int`（最小・最大）、`Bool`、`Unit`。
  - 3-5f-ii: `tests/show_35f.rs` で、全型の表示を `console.log` と入口の戻り値の両方で評価器とネイティブで突き合わせる: 文字列（引用あり・なし。H-4 の行列の文字を含む）、`Bytes`（空・1 要素・複数）、秘密、`Addr`（H-10 の回帰試験）、一覧（空・入れ子・文字列の要素）、組、構造体、列挙型（腕のフィールドの有無・`Option`・`Result`）、`Shared`・`Cell`、参照、関数の値、移動済みの値（`()`）。`rt_spec` に `val_show`・`print_result`・`trace_drop` がなく、`runtime/core` に `show_into`・`sigil_rt_val_show`・`sigil_rt_trace_drop`・`sigil_rt_show_` がない（`grep`）。
- **`unchecked` の要否**: 要らない（3-5e と同じ。埋め込みの `std.*` に関数を足さず、生成は codegen の中で閉じる。使うシムは `std.core.string.new` など既にあるものだけ）。
- **試験の置き場所**: `tests/show_35f.rs`（新しいファイル）。駆動は `tests/list_35c.rs` と同じく、評価器（`ir_eval::run_with`）とネイティブ（`common::native`）の両方で走らせる。

#### H-10. 既知の差と既存の記述との整合

- **`Addr` の表示**: 評価器は `Addr(<10 進>)` と出すが、今のネイティブの `show_into` には `Addr`（記述子の種類 16）の腕がなく、既定の腕で `<関数>` になる（評価器と食い違う）。3-5f で生成する表示は、G-10 の `Addr` の比較と同じく評価器に合わせて `Addr(<10 進>)` にする（ネイティブの不具合の修正。`tests/show_35f.rs` に回帰試験を置く）。
- **深い表示**: 評価器は深さの上限で表示を空にしうる（`show_val` の `deep()`）が、ネイティブは今の `show_into` と同じく再帰する（H-1）。差は今のまま残し、ネイティブの深い表示の試験は置かない（G-10 の深い比較と同じ方針）。
- **G-3 の `trace_drop` の書き込み**: 「3-6a が合流していればその関数にする」とあるが、3-6a は未合流である。本補足の PR で、G-3 のその文へ「補足 H-6 で改める: 書き込みの補助 `sigil_write_all` は 3-5f で持ち、3-6a はそれを使う」の注を付ける。

#### H-11. 選択肢と推奨（本文はすべて推奨案で書いた）

1. **文字列の組み立て**（H-1・H-2）: 共有のバッファへ追記し、根元でだけ文字列化する（**推奨**。今の `show_into` と同じ形で、長さに比例する手間で済み、書くだけの経路では文字列を作らない）／型ごとの関数が文字列を返し `str_concat` で合成する（呼び出し箇所の形は今に近いが、合成が長さの二乗の手間になり、`trace_drop` でも文字列の確保が要る）。
2. **`Float` の変換**（H-3）: 正確な多倍長演算の区間方式（**推奨**。決定的で、Rust の `{}` との一致を試験で固定できる）／`f64` の演算による近似の桁生成に正確な経路へのフォールバックを組み合わせる（Grisu 方式。速いが、境界の正しさの検証が重い）／Rust のシムとして残す（3-7 で `runtime/` を消せなくなるので採らない）。
3. **バッファの伸ばし方**（H-2）: G-2 の破棄の積みと同じ（静的 4096 バイト + 2 倍に伸ばし、終わったら返す。**推奨**）／表示のたびに確保して返す（単純だが、`trace_drop` の頻度では確保と解放が増える）。
4. **3-5f の分割**（H-9）: 土台・スカラー（i）と型ごとの生成（ii）の 2 段（**推奨**。`Float` の変換を単独でレビューできる大きさになる）／1 つの PR にまとめる（表示の全経路が一度に替わり、問題があったときに切り分けにくい）。

---

### 補足 I（PR 3-5g の前）: 入口の引数と停止の報告の生成と 3-5g の分割

- **日付**: 2026-11-07
- **状態**: 提案（推奨案で本文を書いた。細目の選択は I-10 に推奨つきで並べ、異論があれば本補足の修正 PR で直す）。本文・補足 A〜H と食い違う箇所は、本補足が置き換える。
- 3-5g（入口の引数 `arg_*`・`entry_args`・`Float` の解析と、停止の報告 `stop`/`report`。C-7・D-6）の着手の前（main `248eb36`。3-5f は合流済み）に、生成する関数の形・リテラルの読み取りと `Float` の解析の置き場所・合わせ込みの生成・記述子の削除の扱いが決まっていないので、`runtime/core/src/val.rs`・`lib.rs` と codegen の呼び出し箇所を棚卸しして、それらを決め、分割を決める。

#### I-0. 現状の確認（main `248eb36`）

- **入口の引数**: 生成した入口（thunk）は、引数ごとに参照を読み飛ばした型の記述子（`Cg::desc`）と名前の表を作り、`rt_spec` の `entry_args`（`"ippppppp", 'i'`）を呼ぶ。実体は `val.rs` の `sigil_rt_entry_args` で、コマンド行の引数を `CStr::to_string_lossy` で文字列にしてから、共有の `src/shared/args.rs` の `parse_arg` でリテラル値に読み、`coerce` が記述子を歩いて型に合わせた語を作る（文字列は `str_new`、一覧は `list_new`/`list_push`、どちらも関数表経由）。個数や型が合わないと「入口の指定が誤り: …」を標準エラーに出して 1 を返す。
- **`parse_arg` の規則**: `true`/`false` は真偽値、符号つき 10 進で `i64` に読めれば整数、数字（と符号）だけで `i64` の範囲外なら「整数 {s} が Int の範囲外」の誤り、`f64` の `parse` で読めれば小数、`[`…`]` は中身をカンマで割って再帰に読む一覧、それ以外はそのまま文字列。評価器（`ir_eval`）も修正側の入口も、同じ `parse_arg` を使う。
- **使われていない読み取り群**: `lib.rs` には `arg_parse`（`RtArg`）と `arg_kind`・`arg_bool`・`arg_int`・`arg_float`・`arg_str`・`arg_list_len`・`arg_list_get`・`arg_name`・`arg_free` がある（doc には「宣言の型への合わせ込みは生成したコードが行う」とある）が、生成コードからの呼び出しはまだなく、`rt_spec` にもない。
- **停止の報告**: 文言の表は生成側（`cg.msg_table`）に既にある。`report_stop` は `rt_spec` の `stop`（`("pp", 'v')`）を呼び、実体 `sigil_rt_stop` は `sigil_rt_report`（Rust の標準出力を flush し、文言 + 改行を標準エラーへ）と `exit(70)`。呼び出し箇所は thunk の停止、`alloc_shim`（報告の後に raw の `exit_group(70)` も置くが、`stop` が戻らないため届かない）、`valops` の二重解放の検出（`stop` を直接）。
- **`exit`**: `sigil_rt_exit`（flush + `exit(code)`）は、棚卸しの範囲（codegen・runtime/os・処理系）で呼び出し箇所がない。プロセスの終わりは、生成した C の `main` が `enter` の戻りを返す形で足りている。
- **標準出力の flush**: `runtime/os` の `console_log` は呼び出しごとに `write_all` + `flush` する。生成側に溜まる標準出力はないので、報告の側で flush しなくても内容と順序は変わらない。
- **記述子の読み手**: H-7 のとおり入口の合わせ込みだけ。ほかに、PR #303 で `trace_struct` が記述子をオブジェクトへ残すため `desc` を呼ぶ（`tests/ct_rules.rs` の `clif_secret_descriptor_is_zeroizing_kind` の前提）。
- **`String.new_lossy`**: D-6 の「加える」は実施済みで、`std.core.string` に `new_lossy` がある（`valid_utf8`・`from_utf8`・`parse_int` も Sigil 側にある）。言語への追加は要らない。

#### I-1. 全体の方針

- 入口の引数の解釈は、今の 2 段（リテラルの読み取り → 型への合わせ込み）のまま、両段とも生成へ移す。第 1 段は、引数のバイト列からタグつきのリテラル値を作る。第 2 段は、型ごとの生成関数 `sigil_coerce_<k>` がリテラル値を語に変える。2 段に分けるのは、誤りの文言が「読めた値の種類」と「要る型」の組み合わせで決まるためで、今の文言をそのまま再現するには段を分けるのが自然だからである。
- リテラル値の表現は `[種類][中身…]` の塊とし、種類の番号は今の `arg_kind` と同じ（0 真偽値、1 整数、2 小数、3 文字列、4 一覧）にする。文字列の中身は（バイト列への番地、長さ）で持ち、`RtStr` への変換（`new_lossy`）は合わせ込みで `Str`/`Path` に決まったときだけ行う。今は読み取りの時点で文字列を作るが、差は外から見えない: 変換の規則は同じで、分割（カンマ）も分類（数かどうか）も、変換の前後で変わらない（変換で変わるのは不正な UTF-8 の部分だけで、それは数にも区切りにもならない）。
- リテラル値の塊は `raw_alloc` で取り、入口の間だけ生きる。合わせ込みが終わっても返さない（今の `entry_args` の経路も返していない）。

#### I-2. リテラルの読み取り（第 1 段）

- 規則と順序は `parse_arg` と完全一致させる: 真偽値 → 整数 →（数字だけで範囲外なら範囲外の誤り）→ 小数（I-3）→ 一覧 → 文字列。
- 整数は符号つき 10 進で、`i64` への蓄積とあふれの検査で読む。範囲外の誤りは今と同じ位置・同じ文言（「入口の指定が誤り: 整数 {s} が Int の範囲外」）で入口全体を 1 で終わらせる。
- 一覧は今の単純な形を癖まで含めて再現する: 前後の `[` `]` を外し、中身をカンマで割って、要素ごとに前後の空白を取り除いて再帰に読む。入れ子の一覧のカンマも最上位の分割で割れるため、要素が 2 つ以上の一覧は読めない、という今の癖もそのままにする（直すなら別の判断が要る。本補足では変えない）。

#### I-3. `Float` の解析

- 受け付ける形は、Rust の `f64` の `FromStr` と一致させる: 符号つきで、`inf`・`infinity`・`NaN`（大文字小文字を区別しない）、または仮数（「digits」「digits.」「digits.digits」「.digits」）に指数（`e`/`E`、符号つき）が続く形。受け付けの成否が値の種類（小数か文字列か）を決めるので、文法の一致そのものが仕様である。大きさによる誤りはない（あふれは無限大、あふれの逆は 0 または非正規化数になる。Rust と同じ）。
- 変換は正しく丸める（最も近い値へ、同点なら偶数へ）。Rust の `parse` と一致させる。
- アルゴリズムは正確な多倍長演算で、H-3 の表示と対にする。仮数の 10 進桁を多倍長整数に読み、10 の冪を掛けた有理数を `f64` の桁へ正確に丸める。有効桁は 1200 桁まで保持し、それ以降は sticky（0 でない桁があったこと）として丸めにだけ効かせる。隣り合う `f64` の中点は 10 進で有限に書け、その桁数が 1200 桁に収まるので、この上限で丸めの判定は変わらない。
- 一致は試験で固定する: Rust の `parse` を oracle にした行列（境界の指数、非正規化数の前後、中点とその前後、長い桁、符号と指数の形、`inf`/`NaN` の書き方）を `tests/entry_35g.rs` に置く（I-9）。

#### I-4. 合わせ込み（第 2 段）と誤りの文言

- `Gk` に `Coerce` を足し、`sigil_coerce_<k>(lit) -> (語, 成否)` を生成する。キーは表示・破棄と同じく `norm_with(strip(t), true)`（参照は読み飛ばす。今の `coerce` が記述子の参照を読み飛ばすのと同じ）。
- 型ごとの規則は今の `coerce` と同じ: `Int` は整数のリテラルだけ、`Float` は小数と整数、`Bool` は真偽値、`Str`/`Path` は文字列（`new_lossy` で文字列を作る）、`List` は一覧（要素を要素の型で再帰に合わせ込み、既存の `list_new`/`list_push` の rt を使う）。それ以外の型（組・構造体・列挙型・関数など）は常に不一致（今の `coerce` に腕がないのと同じ）。
- 不一致の文言は「{kind_name} が要るが、{arg_name}が渡された」。`kind_name` は要る型の種類から（`Int`・`Float`・`Bool`・`String`・`Path`、それ以外は `List` — 今の `kind_name` の既定の腕まで再現する）、`arg_name` はリテラルの種類から（整数・小数・真偽値・文字列・一覧）。全体は「入口の指定が誤り: 引数 {pname} の型が合わない: 」+ 文言 + 改行。組み立ては表示のバッファ（`sigil_show_*` と `SFn::Push`、`sigil_show_int`）に積み、`sigil_write_all` で fd 2 へ出す。
- 個数の不一致は「入口の指定が誤り: {func} は引数を {n} 個とる（{m} 個渡された）」で、同じ組み立て方にする。
- 引数の名前の表は thunk に残す（記述子の表とは別に、誤りの文言のためだけに使う）。
- 失敗したら thunk は 1 を返す（今と同じ）。成功したら、語の並びと参照の引数の渡し方（スロットの番地）は今の thunk の規則のままにする。

#### I-5. `String.new_lossy` の呼び方

- 言語への追加はない（I-0）。生成側からは、常駐のシム（`std.core.live` 用の `sigil_rt_sigil_live`）と同じ形で、`std.core.string.new_lossy` を C 呼び出し規約のシムで包んで thunk から直接呼ぶ。状態が 0 でなければ、ほかのシムと同じく停止の報告を出して終わる（F-10 の流儀）。
- `new_lossy` の変換の規則は Rust の `to_string_lossy` と同じ（不正な UTF-8 は U+FFFD への置き換え。最大の部分列単位）。Sigil 側の実装は既にあるので、本補足で新たに決めることはない。

#### I-6. 停止の報告

- 補助の関数 `sigil_report_stop(msg, len)` を生成側に持つ: `sigil_write_all` で文言を fd 2 へ出し、改行を出し、`raw_syscall` の `exit_group`（231）で 70 を返す。戻らない。
- 呼び出し箇所は今の `report_stop` と `stop` の全部: thunk の停止の段、`alloc_shim`（報告の後の `exit_group` は重なるので外す）、`valops` の二重解放の検出。
- 標準出力の flush は生成側ではしない（I-0: `console_log` は呼び出しごとに flush 済みで、内容と順序は変わらない）。
- `rt_spec` から `stop` を消し、`runtime/core` から `sigil_rt_report`・`sigil_rt_stop`・`sigil_rt_exit` を消す。

#### I-7. 記述子の削除

- `Cg::desc`・`desc_n`・`descs` と、thunk の記述子の表（`dtab`）を消す。`trace_struct` が記述子を残す呼び出し（PR #303）も一緒に消える。
- `tests/ct_rules.rs` の `clif_secret_descriptor_is_zeroizing_kind` は、記述子（種類 17 の在りか）という仕組み自体の監査だったので、仕組みの廃止と一緒に試験を消す。ゼロ化の担保は、生成した破棄の段が秘密の型に `zeroize` を呼ぶという生成の規則と、`runtime/core` の `secret_drop_zeroizes`、`tests/crypto_secret.rs` の秘匿の試験が持つ。この試験は暗号シリーズ 6-1b の完了条件だったものなので、削除は I-10 の 4 に置いて承認の対象にする。

#### I-8. Rust の消費者の線引き

| 対象 | 扱い |
|---|---|
| `val.rs` の `sigil_rt_entry_args`・`coerce`・`kind_name` | 消す（I-2〜I-4） |
| `val.rs` の `sigil_rt_zeroize_bytes` とその試験 | 残す。モジュールはゼロ化だけになり、doc を直す |
| `lib.rs` の `arg_parse`・`RtArg`・`arg_*`（読み取り群） | 消す（I-10 の 1） |
| `lib.rs` の `sigil_rt_report`・`sigil_rt_stop`・`sigil_rt_exit` | 消す（I-6） |
| `lib.rs` の `enter`・スタック域・`set_*_ops` ほか | 変えない（3-6 以降の範囲） |
| `src/shared/args.rs` | 残す（処理系と評価器が使う。実行ファイル向けの入口の写しが消えるだけ） |
| `rt_spec` | `entry_args` と `stop` を消す |

#### I-9. 分割と完了基準

| PR | 範囲 | 依存 | 要承認 |
|---|---|---|---|
| 補足 I | 本補足（docs のみ）。H-7 の記述子の範囲へ I-7 の注を付ける | 補足 H | 不要（ADR） |
| 3-5g-i | 停止の報告: `sigil_report_stop` の生成（I-6）と呼び出し箇所の付け替え（thunk・`alloc_shim`・二重解放）。`rt_spec` から `stop` を消し、`report`・`stop`・`exit` の Rust を消す | 補足 I | 不要（言語の振る舞いは変えない） |
| 3-5g-ii | 入口の引数: リテラルの読み取り（I-2）と `Float` の解析（I-3）、`sigil_coerce_<k>` の生成と thunk の付け替え（I-4）、`new_lossy` のシム（I-5）。`entry_args`・`arg_*` の Rust と記述子を消し（I-7・I-8）、`ct_rules` の記述子の試験を消す（I-7）。`tests/entry_35g.rs` を足す | 3-5g-i | 不要（言語の振る舞いは評価器と一致させる）。ただし `ct_rules` の試験の削除だけは補足 I の承認をもって行う |

- **各 PR の完了基準**（C-7 の共通の基準に加えて）:
  - 共通: 既存の全件の突き合わせと `scripts/native-report.sh` が通る。入口の引数の誤りを比べる既存の試験（`tests/interp.rs` など）が変わらず通る。
  - 3-5g-i: 停止の文言と終了コード 70 が今までどおりであること（既存の停止系の試験）。`rt_spec` に `stop` がなく、`runtime/core` に `sigil_rt_report`・`sigil_rt_stop`・`sigil_rt_exit` がない（`grep`）。
  - 3-5g-ii: `tests/entry_35g.rs` で、入口の引数を評価器とネイティブで突き合わせる: 型 × リテラルの行列（`Int`・`Float`・`Bool`・`String`・`Path`・`List` とその参照について、合う・合わない、誤りの文言と終了コード 1、個数の不一致、`Int` の範囲外）、`Float` の行列（I-3。Rust の `parse` を oracle にする）。`rt_spec` に `entry_args` がなく、`runtime/core` に `sigil_rt_entry_args`・`coerce`・`arg_parse` がなく、codegen に `Cg::desc` がない（`grep`）。
- **`unchecked` の要否**: 要らない（生成は codegen の中で閉じる。使うシムは `std.core.string.new_lossy` の包みだけ（I-5））。
- **試験の置き場所**: `tests/entry_35g.rs`（新しいファイル）。駆動は `tests/show_35f.rs` と同じく、評価器（`ir_eval::run_with`）とネイティブ（`common::native`）の両方で走らせる。引数の誤り（終了コードと標準エラー）はネイティブの実行結果を直接確かめる。

#### I-10. 選択肢と推奨（本文はすべて推奨案で書いた）

1. **`Float` の解析とリテラルの読み取りの置き場所**（I-2・I-3）: 生成コードに持つ（**推奨**。段階 3 の目的は暫定ランタイムの置き換えで、H-3 で表示を生成へ移した前例と同じ。文法と丸めの一致は oracle の行列で固定する）／`arg_parse` を Rust の境界として残す（合わせ込みだけ生成化する。暫定ランタイムの一部が実行ファイルに残り、3-7 で `runtime/` を消すときに再び判断が要る。採らない）。
2. **リテラルの文字列の中身**（I-1）: バイト列のまま持ち、合わせ込みで初めて文字列にする（**推奨**。I-1 の観察のとおり外からの差はない）／読み取りの時点で文字列を作る（今に近いが、型が合わずに捨てる引数でも変換が走る）。
3. **分割**（I-9）: 停止の報告（i）と入口の引数（ii）の 2 段（**推奨**。i は小さく、ii は `Float` の解析を単独でレビューできる）／1 つの PR にまとめる（入口まわりが一度に替わり、問題があったときに切り分けにくい）。
4. **`ct_rules` の記述子の試験**（I-7）: 記述子の廃止と一緒に消す（**推奨**。仕組みの監査だったため。ゼロ化の担保の置き場所は I-7）／残す（記述子を残す理由になり、G-3・H-7 の予定と食い違う。採らない）。

### 補足 J（PR 3-6a の実装中）: `std.os.console` への targets 照合の免除と C-7 の訂正

- **日付**: 2026-11-08
- **状態**: 所有者承認済み（Issue #318 の判断）。本文・補足 A〜I と食い違う箇所は、本補足が置き換える。
- 3-6a（`console.log` の Sigil 実装。C-7）の実装中に、暗黙に結ぶ `std.os.console` が入口の `targets` との照合に掛かること、C-7 の 3-6a の行が `os_buf_free` の廃止まで含めていることの 2 点が、実装と食い違うと分かった。2 件を決める。

#### J-1. `std.os.console` への入口の `targets` との照合の免除

- **決定**: C-2 が確保器（`std.os.alloc`）に決めた免除を、`std.os.console` へ広げる。理由は C-2 と同じ境界である: console は処理系が暗黙に結ぶプラットフォーム層のモジュールで、実体は出力先（常に Linux）が与えるため、入口の `targets` との照合の対象ではない。照合は次の 2 か所にあり、どちらも免除する。
  - (a) **プロジェクトの読み込み時の、入口の `targets` との照合**（`TargetMismatch`。`src/project/embedded.rs` の `visit_embedded_roots`）: 暗黙に結ぶ埋め込みモジュールのうち、`ALLOC_MODULE` に加えて `CONSOLE_MODULE` も、入口の `targets` に対する `targets` の包含の確認を飛ばす。
  - (b) **結合後の検査での、効果の実行対象の照合**（`EffectTarget`。`src/check/effects.rs` の `match_effects`）: プロジェクトは全モジュールを 1 つの `File` に結合し、入口の `targets` で全項目を検査するので、暗黙に結ばれた `std.os.console` の関数（`write_all` の `effects console` など）まで、入口の `targets`（例: `bare`）に対する効果の照合を受けてしまう。確保器は効果を持たないので表面化しなかっただけである。`src/check/items.rs` の `check_fn_origin` で、埋め込み由来の項目（`item_embedded`）を検査する間だけ `self.targets` を退避して空にし、終わったら戻す（`OsItemInCore` に対する `self.core` の一時的な解除と同じ形）。`self.targets` の読み手は `match_effects` の `EffectTarget` の照合だけである。
- **変わらないもの**:
  - console 自身の `targets`・層の検査は通常どおり行う。埋め込みモジュール自身の検査は、読み込み時の `check_module_embedded` が自分の `targets`（`server`）で行っており、(b) はそれを崩さない。
  - 利用者が OS 層の項目（`raw.*` など）を直接呼べないことは変わらない（`OsItemInCore`、`use std.*` の拒否。ADR 0016 決定 3）。
  - 利用者の関数への `EffectTarget`（`bare` のモジュールで `console` 効果を使う、など）は従来どおり拒否する。免除は `item_embedded` の項目に限る。

#### J-2. C-7 の 3-6a の行の訂正（`os_buf_free` の廃止は 3-6b へ）

- **決定**: 補足 C-7 の表の 3-6a の行にある「`os_buf_free` を廃止」を、3-6b へ移す。C-7 の表自体は書き換えず、本補足がその一点を置き換える。
- **理由**: `os_buf_free` の使用者として残るのは、fs・path の経路（3-6b の範囲）である。廃止は、使用者の移行と同時に行う。3-6a の時点で廃止すると、まだ移行していない使用者が壊れる。
- 3-6a の行のほかの内容（`write` を全部書き終えるまで繰り返す、末尾の改行を書く、など）と、依存（3-5b・3-7a。D-6 で置換）は変えない。

---

## 影響

- **アーキテクチャ**:
  - Sigil は、外部の C/Rust ランタイムに依存しない真の自己完結型言語となる。
  - 出力されるバイナリの肥大化が抑えられ、極小のコンテナイメージ（Scratch コンテナ）でそのまま動作可能となる。
- **セキュリティ**:
  - 信頼基盤（`trust base`）内の `raw.*` 呼び出しおよび大域状態アクセスが `trust-base.list` と `unchecked` 節により厳密に可視化・監査される。
- **CI / ビルド環境**:
  - `libc.a` などのホスト環境のライブラリ依存がなくなり、クロスコンパイルや環境差異に強いビルドが実現する。


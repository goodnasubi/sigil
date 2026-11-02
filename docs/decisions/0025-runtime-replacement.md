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
- 確保器を結ぶときだけ、codegen が C 呼び出し規約のシム `sigil_rt_raw_alloc`/`sigil_rt_raw_free` を公開し、Sigil 製の関数を呼ぶ。生成した Sigil 関数の戻り値（値と状態）のうち状態は捨てる。確保器の中の停止は A-4 のとおり `exit_group(70)` で直接止めるため。
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

- `Float` の表示（最短で往復できる表現）・入口の引数の解析・`%` を Sigil で書くために、`Float` のビット列を出し入れする関数を、**原始操作ではない通常の標準ライブラリの関数**として核の層に置く（原始操作として `RAW_OPS` に置く案は採らない）: `Float.to_bits(x: Float) -> Int`・`Float.from_bits(n: Int) -> Float`（効果 `none`、IEEE 754 binary64 のビット列を `Int` の 64 ビットとしてそのまま読み替える）。
  - 理由: ビット列の読み替えは、どの値に対しても安全で、前提の文が要らない。`RAW_OPS` に置くと、使う関数がすべて `unchecked` になり、検査を外す範囲が無用に広がる（ADR 0016 の評価の観点 2）。
  - 評価器は `f64::to_bits`/`f64::from_bits`、ネイティブは Cranelift の `bitcast` で実装する。標準ライブラリの表に関数を足すのは言語の仕様の変更なので、実装の PR（3-5a）は所有者の承認を要する。
- `memmove`・`memset`（`raw.copy`/`raw.set` が呼ぶ）は、codegen が自分のオブジェクトの中に関数として生成する（バイト単位のループ。`memmove` は重なりの向きで分岐する）。
- `fmod`（`Float` の `%`）は、埋め込みの Sigil の関数として書き、`Float.to_bits` を使って厳密に計算する（musl の `fmod` と同じ方式）。`unchecked` は要らない。評価器の Rust の `%` との突き合わせで確かめる。

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
- 3-5e（3-7a と 3-7b の両方の後）で、Rust の確保の経路を消す。それに合わせて、`strs=`・`blocks=` を外し、`live: sigil_blocks=<数>` にする。
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
| 3-5c | `List`（`list_*`・`list_sum_*`・`list_best`） | 3-5b・3-7a（D-6 で置換） |
| 3-5d | `Bytes`・`int_rot*` | 3-5b・3-7a（D-6 で置換） |
| 3-5e | 型の記述子の処理の生成（C-1）・`Shared`/`Cell`・`--trace-drops`。`SIGIL_RT_LIVE` の `strs=`/`blocks=` を外す（C-5） | 3-5c・3-5d・3-7a・3-7b（D-6 で置換） |
| 3-5f | 表示（`val_show`・`print_result`・`Float` の最短表現） | 3-5e |
| 3-5g | 入口の引数（`arg_*`・`entry_args`・`Float` の解析）と停止の報告（`stop`/`report`） | 3-5f |
| 3-6a | `console.log`。`write` は全部を書き終えるまで繰り返す（部分書き込みは残りから続け、−4（EINTR）は同じ位置から再試行し、その他の負の値は書くのをやめる。今の Rust 版と同じく失敗を呼び出し側へ返さない）。末尾の改行を書く。`os_buf_free` を廃止 | 3-5b・3-7a（D-6 で置換） |
| 3-6b | `fs.read`/`fs.write` と `Path`。詳細は下の表の後の項 | 3-6a |
| 3-6c | `env`（C-4 の大域 `std.os.env.ENVP` を読む。`RAW_OPS` は変えない） | 3-5b・3-7a・3-7b（D-6 で置換） |
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
  - `std.core.string` は、作る関数を 2 つ持つ。`new_lossy(p, n)` は今の `sigil_rt_str_new`（`from_utf8_lossy`）と同じく、不正な列を置換文字 U+FFFD にして作る（正しい列はそのまま複写する）。`new(p, n)` は検証せずに複写し、前提の文に「`p..p+n` は正しい UTF-8」を書く。あわせて `valid_utf8(p, n) -> Bool` を置く（3-5d の `bytes_to_string_utf8`、3-6b の `fs.read`、3-6c の `env` が使う）。この 3 つは 3-5b-i に入れる。
  - 生成コードと Rust ランタイムが呼ぶ `sigil_rt_str_new`（シムと関数表の `new`）は `new_lossy` を呼ぶ。外から来たバイト列（入口の引数・ファイル・環境変数）を含め、どの経路から作っても不変条件が成り立つ。検証の手間は今の `from_utf8_lossy` と同じく長さに比例する。
  - 検証なしの `new` を呼ぶのは、出力が構成上正しい UTF-8 になる `std.core.string` 自身の関数（`clone`・`trim`・`concat`、3-5c の `lines`）と、正しさを自分で確かめた埋め込みの `std.*` に限る。呼ぶ関数は `unchecked` で、前提の文に根拠を書く（S2 のレビューの対象）。
  - Rust の側の境界はそのまま残す: 入口の引数は今のとおり `to_string_lossy`、`fs.read` は Rust の検証、`bytes_to_string_utf8` は `String::from_utf8` で、それぞれ 3-5g・3-6b・3-5d で Sigil へ移すときに `new_lossy`・`valid_utf8` へ置き換える。
  - Rust の消費者が中身を `&str` として読むとき（今の `s(p)`）は、`std::str::from_utf8` で検査し、不正なら異常停止（終了コード 70）する。`from_utf8_unchecked` にすると、信頼の義務が `new` を呼ぶすべての箇所に掛かるため。検査の手間は 3-7d で Rust の消費者が無くなるまでの一時的なものとする。
- **利用者からは呼べない**: 利用者の `use std.*` は拒否される（ADR 0016 決定 3、ADR 0021。`src/stdlib/embedded.rs`）ので、`std.core.string` の関数は `pub` であっても利用者のモジュールからは呼べず、利用者が不変条件を破る経路はない。`pub` にするのは、ほかの埋め込みの `std.*`（3-5c 以降・3-6a 以降）が使う関数だけとする。

#### D-3. 接続（codegen・Rust ランタイム・停止）

- **Sigil 側**: 埋め込みのモジュール `std.core.string`（`src/stdlib/core_string.sigil`、`targets bare, browser, server`、`trust base`。C-1 の置き場所）に、`Addr`・`Int` だけを受け渡す関数を置く。バイトや頭を読み書きする関数は `unchecked` で、前提の文に「`s` は `std.core.string` が返し、まだ解放していない頭（したがって `len <= cap`、`len > 0` なら `ptr..ptr+len` は読める正しい UTF-8）」「`p..p+n` は読める」などを書く。
  - 3-5b-i: `new`・`new_lossy`・`valid_utf8`・`free`・`clone`・`data`・`bytes`。
  - 3-5b-ii: `len`（スカラー値の数。続きのバイト `0b10xxxxxx` でないバイトを数える）・`is_empty`・`trim`（Rust の `str::trim` と同じ `White_Space` の符号点の表）・`concat`・`parse_int(s: Addr, out: Addr) -> Int`・`eq`・`cmp`（バイト列の辞書順。Rust の `str::cmp` と同じ）。`eq`・`cmp` は 3-5b-ii で足して試験し、3-5e の生成する `val_eq`・`val_cmp` の文字列の腕が呼ぶ。それまで `val_eq`・`val_cmp` は Rust のまま関数表で読んで比べる。Rust の `sigil_rt_str_cmp`（生成コードからは呼ばれていない）は 3-5e で消す。
  - 長さと確保の検査: `new`・`new_lossy` は `n < 0` なら停止する（終了コード 70）。`n` = 0 なら `p` を読まない。`concat` の長さの和は、各長さが確保器の上限（2^46。A-4）未満なので `Int` であふれず、上限を超える確保は確保器が停止する。
  - 確保器（`std.os.alloc`）と同じく、すべてのプログラムに暗黙に結び（`implicit_roots`。C-2）、lowering は入口から辿れなくても SIR に入れる（`src/lower/mod.rs` の確保器の関数の扱いと同じ。`src/ir.rs` は変えない）。評価器でも結ぶが、評価器の `String` の組み込みは Rust のまま（C-7 の最後の項）で、これらの関数は呼ばない。
  - 試験用の差し込み（`Options::raw_ops`）で `raw.alloc`/`raw.free` を置き換えたときは、確保器と同じく `std.core.string` も結ばない（偽の原始操作で文字列を作らないため）。そのときにネイティブ出力を求めたら、codegen は「`std.core.string` が結ばれていない」として `CgError::Internal` で止める（差し込みは検査と評価器の試験だけで使う）。
- **codegen**: `sigil_rt_str_new`（`new_lossy` を呼ぶ）・`_free`・`_clone`・`_data`・`_bytes`（3-5b-ii で `_len`・`_is_empty`・`_trim`・`_concat`・`_parse_int` を加える）を、Sigil の関数を呼ぶ C の呼び出し規約のシムとして生成コードの中で定義し、`rt` に登録する。生成コードの呼び出し先の名前と形（`rt_spec` の形。`i` の戻りは 32 ビットへ切り詰める）は変えない。シムを定義した名前は `rt_spec` から消す（C-7 の完了基準）。
  - シムは `Linkage::Local`（公開しない）で定義する。Rust ランタイムは番地を関数表で受け取るので、シンボルとして公開する必要がない。確保器のシム（B-2）が `Linkage::Export` で公開されるのとはここが違う。
  - シムは、Sigil の関数の戻りの状態が 0 でなければ、生成コードの異常停止の経路（終了コード 70）で止める。確保器のシム（B-2）は状態を捨てるが、文字列の関数は算術などの停止を持ちうるので、黙って続けない。
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
| 3-5c | `List`（`list_*`・`list_sum_*`・`list_best`）に加えて `str_lines`（`List<String>` を返すので `List` と一緒に移す。**仮置き: D-8 の 6**） | 3-5b-i・3-7a |
| 3-5d | `Bytes`・`int_rot*`。`str_as_bytes`・`bytes_to_string_utf8`（`valid_utf8` を使う）も移す | 3-5b-i・3-7a |
| 3-5e | 型の記述子の処理の生成（C-1。`val_eq`・`val_cmp`・`val_clone`・`val_drop` の文字列の腕は `std.core.string` の `eq`・`cmp`・`clone`・`free` を呼ぶ）・`Shared`/`Cell`・`--trace-drops`（`trace_drop` を含む）。Rust の `sigil_rt_str_cmp` を消す。`SIGIL_RT_LIVE` の `strs=`/`blocks=` を外す（C-5。試験の期待の改めは 3-5b-i に前倒し済み） | 3-5b-ii・3-5c・3-5d・3-7a・3-7b |
| 3-5f | 表示（`val_show`・`show_*`・`print_result`・`Float` の最短表現） | 3-5e |
| 3-5g | 入口の引数（`arg_*`・`entry_args`・`Float` の解析。不正な UTF-8 は `new_lossy`）と停止の報告（`stop`/`report`） | 3-5f |
| 3-6a | `console.log`（C-7 のとおり） | 3-5b-i・3-7a |
| 3-6b | `fs.read`/`fs.write` と `Path`（C-7 のとおり。UTF-8 の検証は `valid_utf8`） | 3-6a |
| 3-6c | `env`（C-7 のとおり。UTF-8 の扱いは今の Rust 版と同じ結果になるよう `valid_utf8`・`new_lossy` から選ぶ） | 3-5b-i・3-7a・3-7b |

- 3-5b-i は、作る側と読む側を一度に替える必要があるので、それ以上には分けない。3-5b-ii は、関数ごとに分けてもよい（`len`・`is_empty` と `trim`・`concat`・`parse_int`・`eq`・`cmp` の 2 つなど）。
- 3-5c・3-5d・3-6a・3-6c は、文字列を作る・読むことだけが要るので、3-5b-i の後なら 3-5b-ii と並行してよい（3-5c の `lines` が要るのは `new` だけで、`len`・`trim` には依存しない）。3-5e は `eq`・`cmp` を使うので 3-5b-ii の後にする。
- 3-5b-i から 3-7d まで、Rust の消費者は関数表を通す。各 PR が消費者を Sigil 版や生成コードへ移すたびに、関数表の利用者は減る。
- 3-7c・3-7d・3-7e の依存は C-7 のとおり変えない。3-7c の依存の 3-5g は、3-5b-ii を含む全体の後になる。

#### D-7. 試験の戦略

- **評価器の上での直接の試験**（新しいファイル `tests/str_35b.rs` など）: `std.core.string` の関数を、`raw.alloc` で用意した入力で評価器から直接呼ぶ（A-2 の「影の表」と同じ）。頭の 3 つの欄、`cap` = `len`、空の文字列の空番地、`free` の後に塊の表が空に戻ること、`n < 0` の停止、多バイト（2〜4 バイト）・空・`White_Space` の各符号点（U+0085・U+00A0・U+2028・U+3000 など）・`parse_int` の境界（`i64` の最小・最大と、その 1 つ外、`-`・`+`・空）・不正な UTF-8 の列（`new_lossy`・`valid_utf8`。途中で切れた列・余分な続きのバイト・過長の表現・サロゲート）を、`src/shared/text.rs` と Rust の `from_utf8_lossy`・`from_utf8` の結果と突き合わせる。
- **評価器とネイティブの突き合わせ**: 3-5b-i は振る舞いを変えないので、既存の全件の突き合わせと `scripts/native-report.sh` がそのまま通ることを基準にする。3-5b-ii は、上の入力の組をネイティブでも走らせる。
- **`runtime/core` の単体の `cargo test`**: `#[cfg(test)]` の試験用の関数表（同じ 24B の頭を Rust の確保で持つ小さな実装）を、試験の始めに一度だけ登録する。生成コードのシンボルに依存しないので、スタブのシンボルは要らない。`LIVE_STRS` と文字列の二重解放の試験は D-5 のとおり消す。
- **`tests/runtime_parity.rs`**: C のドライバ（`DRIVER`）は今 `sigil_rt_str_new`・`_free`・`_data`・`_bytes` を Rust の関数として呼んでいるが、3-5b-i でこれらは Rust から消える。そのため、ドライバの側に同じ名前の C の関数（`malloc` で同じ 24B の頭を持つ。長さ 0 の扱いは D-2）を定義し、`main` の冒頭で `sigil_rt_set_str_ops` に登録する。`core_alone_links_without_os` の `d.c` も、`sigil_rt_show_int` → `sigil_rt_print_result` が関数表の `new`・`data`/`bytes` を通るので、同じ C の関数表の定義と登録を足す（リンクの対象と期待の出力 `42` は変えない）。3-5b-i の時点では `len`・`trim`・`parse_int`・`lines` は Rust の実装のままなので、突き合わせはそのまま意味を持つ。3-5b-ii で移した関数の組は、ドライバでは C の関数表を試すだけになるので、このファイルから外し、`tests/str_35b.rs` の突き合わせへ移す。
- 既存の試験ファイルの変更（`m5_free_strings.rs`・`fn_types_2_4d_exec.rs`・`runtime_parity.rs`）は、本補足を根拠として該当の PR で行う（AGENTS.md の「既存のテストファイルへの追記」の例外）。

#### D-8. 未決の選択肢（推奨つき。所有者の確認待ち）

本文の「**仮置き: D-8 の n**」の印は、次の n 番の推奨案で書いた箇所を指す。

1. **橋渡しの方式**（D-1）: (b2) Sigil が頭を持ち、codegen は読み替えとシムだけ（**推奨**）／(b1) codegen が頭を読み書きする／(a) `raw.str_*` を `RAW_OPS` に足す（契約変更と ADR 0016 の手続きが要る）。
2. **Rust ランタイムとのつなぎ方**（D-3）: 起動時に関数表を登録する（**推奨**。`core.a` が生成コードのシンボルに依存しない）／生成コードの `sigil_rt_str_*` を外部のシンボルとして参照する（単体の `cargo test` と `runtime_parity` にスタブのシンボルが要る）／Rust が `repr(C)` の頭を読むだけにし、作る・捨てるは関数表を通す（読む側の間接呼び出しは減るが、配置を知る場所が Rust にも増え、3-7d まで 2 か所を揃え続ける）／Rust が `repr(C)` の頭を持ち、`sigil_rt_raw_alloc`/`raw_free` を直接呼んで作る・捨てるも行う（配置を知る場所が 2 つになり、確保器のシンボルのスタブが要る）。
3. **`strs=` の扱い**（D-5）: 3-5b-i で数えるのをやめて常に 0 とし、書式は 3-5e まで変えない（**推奨**。C-5 の時期を守る）／3-5b-i で `strs=` を外す（C-5 の一部を前倒し。完全一致の期待を持つ試験を 2 度直すことになる）。
4. **頭と中身の持ち方**（D-2）: 頭（24B）と中身の 2 つの塊（**推奨**。本文の決定 4 の `{ptr, len, cap}` のまま）／`{len, cap, バイト列…}` の 1 つの塊（確保は 1 回で済むが、決定 4 の改訂が要る）。
5. **不正な UTF-8 の扱い**（D-2）: シムと関数表の `new` は `new_lossy`（今の `from_utf8_lossy` と同じ）とし、検証なしの `new` は `std.core.string` の内部と正しさを確かめた `std.*` だけが `unchecked` で呼ぶ（**推奨**。どの経路から作っても不変条件が成り立つ）／シムも検証なしの `new` とし、呼び出し元が正しい UTF-8 だけを渡すことを一覧で確かめる（検証の手間は省けるが、外から来たバイト列の経路を 1 つ見落とすと不変条件が崩れ、Rust の `s(p)` の検査で止まる）。
6. **`str_lines` の時期**（D-6）: `List` と一緒に 3-5c で移す（**推奨**）／3-5b-ii で移す。後者では `List` がまだ Rust の `RtList` なので、Sigil の `lines` が結果の一覧を作るために、`list_new`・`list_push` を呼ぶ 2 つ目の関数表（Rust の一覧を Sigil から作るためだけの一時的な橋）が要り、3-5c でその橋を捨てることになる。

---

## 影響

- **アーキテクチャ**:
  - Sigil は、外部の C/Rust ランタイムに依存しない真の自己完結型言語となる。
  - 出力されるバイナリの肥大化が抑えられ、極小のコンテナイメージ（Scratch コンテナ）でそのまま動作可能となる。
- **セキュリティ**:
  - 信頼基盤（`trust base`）内の `raw.*` 呼び出しおよび大域状態アクセスが `trust-base.list` と `unchecked` 節により厳密に可視化・監査される。
- **CI / ビルド環境**:
  - `libc.a` などのホスト環境のライブラリ依存がなくなり、クロスコンパイルや環境差異に強いビルドが実現する。


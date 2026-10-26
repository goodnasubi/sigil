# 0014: 標準ライブラリの核と OS 依存部分の分割（M4）

- **状態**: 承認・実装完了（2026-10-25 承認、PR1・PR2a・PR2b で実装。所有者の委任を受けて Claude が決定。ADR 0005 の「核」の定義を改めた点は所有者へ事後報告とする。計測による再検証の条件は末尾を参照）

## 背景

[ADR 0006](0006-p0-scope-and-bootstrap.md) の M4 の完了条件のうち、次の 2 点が未達のまま残っている（同 ADR の「M4 の完了条件の充足」）。

1. 標準ライブラリを、実行環境なしでも使える **核の部分** と、**OS の機能を使う部分** に分け、モジュールとして入れること（[ADR 0005](0005-long-term-goals.md) の影響「標準ライブラリの分割」）。
2. 決定 3 の条件 10 の M4 側: **核の部分のモジュールから OS の機能を使う部分を参照するとエラーになる** こと。

どちらも「核のモジュール」を言語と検査器でどう表すかが決まっていないために止まっている。本 ADR はそれを決め、あわせて ADR 0013 の決定 2 が M4 の前に繰り延べた「効果を持つ部分の正式な一覧と置き場所」を決める。

### 現状（main `d8cbb60`）

- モジュールの宣言は `module 名前` + `targets`（`browser`・`server`）+ `trust`（`none`・`base`）の 2 節（[ADR 0008](0008-m0-notation.md) の決定 6）。`trust base` は構文として読むだけで、検査器での意味はまだない。
- `targets` は ADR 0004 の規則 5（出力先による効果の制限）にだけ使われる（`src/effects.rs` の `EffectTable` の各カテゴリの `Targets`、`src/check.rs` の `match_effects` の `EffectTarget`）。
- 標準ライブラリは `src/stdlib.rs` の表で、すべて 1 つの仮のモジュール `std.std`（`targets browser, server`・`trust base`）として検査器に読み込まれる。核と OS の区別はない。
- モジュールどうしの参照（`import` に当たるもの）はない。標準ライブラリの名前は前置きとして常に見える。複数のファイルを合わせる手段は `interp::run_sources` だけで、最初のファイルの宣言を全体に使う。
- 「モジュールの単位と出力先の指定」の ADR（ADR 0006 の決定 4 で M1 の前）はまだない。本 ADR はそれに依存しない形で決める。

### 前提

- 書き手は AI、人間は仕様（モジュールの宣言・型・シグネチャと 4 節）だけを読む（[ADR 0001](0001-vision-and-scope.md)）。
- 同じ意味の書き方を 1 つにする（[ADR 0002](0002-syntax-family.md) の規則 1）。宣言の節は固定順で省略しない（ADR 0008 の決定 6）。
- 効果のカテゴリと出力先の語は、ADR に記録してから加える（ADR 0004 の改訂・ADR 0008 の決定 6）。
- 条件 10: 実行の割り当て（スケジューラ）などの並行処理の部品は OS 依存部分に置き、核の部分にスレッドや非同期の前提を持ち込まない（ADR 0006 の決定 3）。
- M5 の暫定ランタイムの境界は「M4 で決める標準ライブラリの OS 依存部分の宣言」とする（ADR 0006 の「M5 のランタイムとリンカ」）。

## 評価の観点

重要な順に並べる。

1. **仕様として読めるか**: モジュールが核かどうかが、人間の読む仕様（`sigil spec` の出力）に現れること。
2. **条件 10 を検査で保証できるか**: 核のモジュールが OS の機能に触れる経路を、検査器が漏れなく止めること。
3. **AI が誤りなく書けるか**: 新しい書き方を増やさず、エラーの直し方が 1 つに決まること。
4. **M4 の実装量と、後の段階（M5 の暫定ランタイム、P6 のプラットフォーム層、モジュールの単位の ADR）で作り直さずに済むか**。

## 決定 1: 「核のモジュール」の表し方

### 選択肢

- **A: 出力先の語 `bare`（実行環境なしのネイティブ）を `targets` に加え、`targets` に `bare` を含むモジュールを核のモジュールとする**
- **B: モジュールの宣言に 3 つ目の節（例: `layer core` / `layer os`）を加える**
- **C: モジュール名の名前空間（`std.core.*` / `std.os.*`）と、名前を取り込む宣言（`use`）で分ける**
- **D: 宣言はせず、OS の機能を使っていないモジュールを核とみなす（推論する）**

### 決定

**A** を採用する。

- 出力先の語に `bare` を加える。意味は「OS も実行環境もない環境（ADR 0005 の『実行環境なしのネイティブ出力』）で動かせる」。メモリの確保器は言語の外の環境が供給する（P0 では実行環境（インタプリタ・M5 の暫定ランタイム）、P6 ではプラットフォーム層。決定 2）。P0 で書ける語は `bare`・`browser`・`server` の 3 つになる。
- **`targets` に `bare` を含むモジュールを「核のモジュール」と呼ぶ**。`targets bare` だけでも、`targets bare, browser, server` のように他と並べてもよい。並べ方は従来どおり文字の順（`bare` < `browser` < `server`）。
- `targets` は「この出力先で使えるコードしか書かない」という制約の宣言であり、何を出力するかの指定ではない。P0 の `sigil build`（M5）は `bare` を含むモジュールも Linux x86-64 の実行ファイルとして出力する。実行環境なしの出力そのものは P6 で作る。
- `sigil spec` は `bare` を「実行環境なし」と訳す（例: `出力先: 実行環境なし, ブラウザ, サーバー`）。
- 核のモジュールには、次の 2 つの検査がかかる。
  1. **効果**（決定 3）: ADR 0004 の規則 5 のとおり、`bare` で使えない効果を宣言・推論するとエラー（既存の `EffectTarget`）。
  2. **参照**（決定 4）: 標準ライブラリの OS 層（決定 2）の型・関数・メソッドを名指しするとエラー（新しい `OsItemInCore`）。

### 理由

- 観点 1: ADR 0005 は核を「実行環境なしでも使える部分」と定義しており、これは出力先の性質そのものである。`targets` は仕様の先頭に「出力先」として出るので、人間は「このモジュールは OS なしで動く」をそのまま読める。
- 観点 3: 宣言の形（2 節）は変わらず、語が 1 つ増えるだけ。複数の出力先を並べたモジュールが共通部分の効果しか使えない規則（ADR 0004 の規則 5）が、核にもそのまま効く。
- 観点 4: 効果の表の「使える出力先」の列を 1 つ足せば、P6 で `bare` 向けのカテゴリ（ハードウェアの操作など）を加える手続き（ADR 0005 の改訂）がそのまま使える。

### 選ばなかった案と理由

- **B（3 つ目の節）**: すべてのファイルに節が 1 つ増える（省略不可の規則のため）。「OS なしで動く」を `targets` と別の節の 2 か所で表すことになり、`targets server` かつ `layer core` のような意味の重なる組合せの規則が要る。
- **C（名前空間と `use`）**: 取り込みの仕組みはモジュールの単位の ADR（未決）の範囲で、M4 で先に作ると後で作り直しになりうる。名前で分けても、核のモジュールが `std.os` を取り込んだことをエラーにする規則は別に要り、A の検査と同じものが必要になる。
- **D（推論）**: 何もエラーにならないので条件 10 の「参照するとエラー」を満たせない。仕様に核かどうかが現れず、OS の機能を 1 つ使っただけで黙って核でなくなる。

## 決定 2: 核と OS 依存部分の境界と、標準ライブラリの正式な置き場所

### 選択肢

- **A: 2 層。核（`std.core`）は OS に依存しない部分で、メモリの確保は使ってよい。OS 層（`std.os`）は OS の機能に意味が依存する部分。確保に依存しない最小部分（Rust の `core` と `alloc` の区別）は P6 の ADR で分ける**
- **B: ADR 0005 の文言どおり、核をメモリの確保に依存しない部分に限る**（`List`・`String`・`Shared`・`Cell` は核に入らない）
- **C: 3 層（確保なし / 確保あり / OS）を今作る**

### 決定

**A** を採用する。

- **核の部分（`std.core`）**: OS がなくても意味が決まる部分。メモリの確保と解放は使う（ADR 0004 の規則 2 により効果に数えない）。確保の実装は、P0 では実行環境（インタプリタ、M5 の暫定ランタイム）が、P6 ではプラットフォーム層が供給する。スレッドや非同期の前提を持たない（条件 10）。
- **OS 層（`std.os`）**: OS の機能（標準出力・ファイル・OS のパスの規則）に意味が依存する部分。M5 の暫定ランタイムの境界はこの層の宣言とする（ADR 0006 の「M5 のランタイムとリンカ」）。
- 層は効果だけでは決まらない。`Path.new` は効果を持たないが、区切りが OS の `std::path` に従うので OS 層に置く（ADR 0006 の M4 の記録「OS 依存のまま」）。`Shared<Cell<T>>.get` は `state` を起こすが、OS を使わないので核に置く。

#### 正式な置き場所の一覧

ADR 0013 の決定 2 の一覧（効果を持たない部分）はすべて核に置く。効果を持つ部分（同決定の「M4 の前に正式に決める」）は、ADR 0006 の M4 の記録で実装したシグネチャと意味のまま、次の置き場所で正式とする。

| 層 | 型・名前空間 | 項目 |
|---|---|---|
| 核 | 基本型 | `Int`・`Float`・`Bool`・`String`・`Unit`、`Int.min_value()` |
| 核 | 自由関数 | `to_float`・`to_int` |
| 核 | 組み込みのメソッド | `clone`（検査器の組み込みで層を持たない。核のモジュールで OS 層の型の値に `clone` を呼べるのは型引数を通したときだけ。決定 4 の「許すもの」） |
| 核 | 標準の型 | `List<T>`・`Option<T>`・`Result<T, E>`・`ParseError` とそのメソッド（ADR 0013 の決定 2 の一覧） |
| 核 | 共有所有 | `Shared<T>`（`new`）・`Cell<T>`（`new`）・`Shared<Cell<T>>`（`get`・`set`、効果 `state`） |
| OS | 名前空間 `console` | `log<T: Displayable>(x: &T)`（効果 `console`） |
| OS | 名前空間 `fs` | `read(p: &Path) -> Result<String, IoError>`（`fs.read`）、`write(p: &Path, s: &String) -> Result<Unit, IoError>`（`fs.write`） |
| OS | 型 `Path` | `new`・`is_empty`・`join`・`file_name`・`parent`（効果 `none`） |
| OS | 型 `IoError` | 腕 `NotFound`・`PermissionDenied`・`Other`（プログラムからは取り出せない。ADR 0006 の M4 の記録） |

- 性質（`Copyable`・`Displayable` など）は型検査器の組み込みで、層を持たない。
- ADR 0006 の M4 の記録の未決 (a)（`Path.new`・`join` の引数型）と (b)（`IoError` の腕の取り出し）は、本 ADR でも保留のまま変えない。

### 理由

- 観点 2・4: B では `List`・`String` が核に入らず、`String` のリテラルも確保を伴うので、核のモジュールにはほとんど何も書けない。条件 10 の検査が「何も書けないので何も参照しない」で満たされるだけになり、M5 の「核の部分だけを使うプログラム」も作れない。
- 観点 4: 確保の有無を検査するには、確保を型か効果で追う仕組みが要るが、ADR 0004 の規則 2 は確保を効果に数えないと決めている。確保なしの部分が必要になるのは、割り込み処理や確保器そのものを書く P6 であり、その時点で層を足せばよい（A の層の印は表の列なので、値を 1 つ増やすだけで足せる）。
- 観点 1: 層を効果ではなく「OS に意味が依存するか」で決めるので、`Path` のように効果のない OS 依存の操作も核から締め出せる。

### 選ばなかった案と理由

- **B（確保なしの核）**: 上のとおり、P0 で核のモジュールが実質書けない。
- **C（3 層を今）**: 確保なしの層に入れる項目と検査の対象が P0 にない（`Option`・`Result`・`Int` の演算だけ）。確保を追う仕組みも P0 にない。

## 決定 3: 効果の表の `bare` の列

### 決定

`src/effects.rs` の `Targets` に `bare` を加える。各カテゴリが `bare` で使えるかは次のとおりとし、`state` 以外はすべて使えない。

| カテゴリ | `bare` | 理由 |
|---|---|---|
| `state` | 使える | 核の `Shared<Cell<T>>` が起こす。OS を使わない |
| `console`・`fs.*`・`net.*`・`db`・`env`・`process` | 使えない | OS の機能そのもの |
| `clock`・`random` | 使えない | 時刻・乱数の源は OS かハードウェアで、P0 の核には源がない。P6 でプラットフォーム層が供給するときに ADR で改める |
| `dom`・`browser.storage` | 使えない | ブラウザの機能 |
| `foreign` | 使えない | 外部（Python など）の実行環境の上でしか起きない |
| `task.spawn`・`task.suspend`・`task.share` | 使えない | 実行の割り当ては OS 層に置く（条件 10）。核にタスクの前提を持ち込まない |

- ADR 0004 の表の「使える出力先」の「すべて」は「ブラウザ・サーバー」の意味になる。`state` だけ「すべて（実行環境なしを含む）」とする。
- 実装では、意味の取り違えを防ぐため、`Targets::ALL`（現在 `server`・`browser`）を `Targets::HOSTED` と改名し、3 つとも真の `Targets::ANY` を加えて `state` に使う。

### 理由

- 核のモジュールが `effects console` を宣言しただけ（呼ばなくても）でエラーになるので、宣言の側からも OS の機能が核に入らない。`task.*` を使えないことで、条件 10 の「核にスレッドや非同期の前提を持ち込まない」が検査で保証される。
- 列を足すだけなので、ADR 0006 の条件 4・11（表に足すと検査が働く）の仕組みをそのまま使う。

## 決定 4: 検査器の規則（条件 10 の M4 側）

### 選択肢

- **A: 名指しの検査**。核のモジュールの中で、OS 層の型・関数・メソッドを名指しした位置をエラーにする。値と型引数を通して呼び出し側から渡されるもの（関数の値、型引数に入った OS 層の型）は許す
- **B: 到達の検査**。核のモジュールから呼びうる関数をすべてたどり、OS 層に届けばエラーにする
- **C: 効果の検査だけに頼る**（決定 3 の列だけで済ませる）

### 決定

**A** を採用する。

#### 規則

検査器の状態 `Ck` に `core: bool`（`file.module.targets` に `"bare"` がある）を持つ。`core` のとき、次の位置で OS 層の項目を名指しすると `OsItemInCore` とする。

| 位置 | 例 | 検査する場所（`src/check.rs`） |
|---|---|---|
| 型の名前（引数・戻り値・`let` の注釈・構造体の欄・列挙の腕・篩型の基・型引数の実引数） | `p: &Path`、`-> Result<String, IoError>`、`struct S { p: Path }` | `resolve` の `Type::Path` |
| 名前空間の関数の呼び出し | `console.log(&x)`、`fs.read(&p)` | `call` の `Expr::Field` の名前空間の分岐 |
| 型に付く関数の呼び出し | `Path.new(&s)` | `call` の `type_path` の分岐 |
| メソッドの呼び出し（受け手の型の名前が OS 層） | `p.join(&s)` | メソッドの表（`methods`）を引く位置 |

- メソッドの行は、上の 3 つで OS 層の値が核のモジュールで作られない・受け取られないので本来は届かないが、規則を位置ごとに閉じるため置く（検査の漏れの備え）。
- 同じ箇所で `EffectTarget` も成り立つとき（`console.log` を呼ぶ）は、`OsItemInCore` を先に報告する。名前を解決する時点（本体の型検査）で見つかり、効果の照合（関数の終わり）より前だからである。説明に出る直し方が 1 つになる（「`bare` を外すか、OS 層を使わない書き方にする」）。
- エラーの文言: ``` `{名前}` は標準ライブラリの OS 層（std.os）にあり、`targets` に `bare` を持つモジュールからは使えない（このモジュールを OS なしで使わないなら `targets` から `bare` を外す） ```
- `ErrorKind::OsItemInCore` を `src/error.rs` に加え、`UnknownType` と同じ型エラーの分類に入れる。
- **`trust base` でも免除しない**。核かどうかと信頼基盤かどうかは別の軸である（決定 5）。

#### 許すもの

- **関数の値**: 核の関数 `fn apply_all(xs: &List<Int>, f: fn(Int) -> Int) -> ... effects of(f)` に、サーバーのモジュールが `console` を起こす関数を渡すことは許す。`of(f)` は呼び出し側で置き換わり（ADR 0004 の規則 4）、OS の機能を名指ししているのは呼び出し側である。
- **型引数**: 核の総称の関数に、呼び出し側が `T = Path` を渡すことは許す。核の関数は `T` に対して `clone`・移動・破棄しかできず、OS の機能を名指ししない。M5 では、型を具体化した本体は呼び出し側のプログラムに属するとみなす（暫定ランタイムの OS 層の型の `clone`・破棄に依存するのは、`Path` を渡したプログラムである）。

#### 複数のファイルを合わせるとき

> ADR 0021 の 2-2c で `ModuleMismatch` と `merge_sources` は廃止した（モジュール 1 つ = ファイル 1 つ）。

`interp::run_sources` は最初のファイルの宣言を全体に使うので、核のファイルを OS のファイルの後ろに並べると核の検査が黙って外れる。これを防ぐため、**合わせるファイルのモジュールの宣言（名前・`targets`・`trust`）がすべて同じでなければエラー**（新しい `ErrorKind::ModuleMismatch`。`RunError::Check` で返す。終了コードは検査のエラーと同じ 1）とする。現在合わせて使っている `examples/04-effects/effects.sigil` と `entry.sigil` は宣言が同じなので影響しない。ファイルごとに宣言を保ったまま合わせる仕組みは、モジュールの単位の ADR で決める。

#### 標準ライブラリの表の分け方（`src/stdlib.rs`）

- `pub enum Layer { Core, Os }` を加え、**表の各行に層の列を足す**（表を層ごとに分けない）。
  - `OPAQUE_TYPES: &[(&str, usize, Layer)]`（`Path`・`IoError` が `Os`）
  - `FREE_FNS: &[(Layer, &str, &str)]`（すべて `Core`）
  - `NAMESPACED_FNS: &[(Layer, &str, &str, &str)]`（`console`・`fs` は `Os`）
  - `IMPLS: &[(Layer, &str, &[Entry])]`（`Path` の `impl` が `Os`、他は `Core`）
- `free_source`・`impl_source` は層を受け取り、仮のモジュールの見出しを層ごとにする。
  - 核: `module std.core` / `targets bare, browser, server` / `trust base`
  - OS 層: `module std.os` / `targets browser, server` / `trust base`
- `register_std` と `prelude` は、`Os` の行の名前を `Ck` の `os_items` に入れる。鍵は既存の表の鍵と同じ形にする: 型は型名（`"Path"`）、名前空間の関数は `"console.log"`、メソッドと型に付く関数は `("Path", "new")` の組。
- 冒頭の注記を、本 ADR の決定 2 の一覧を参照する形に改める（「暫定」の記述を除く）。

列にした理由: 行を層の間で移すのが 1 語の変更で済み、差分で層の移動が読める。層ごとに表を分けると、同じ形の表が 8 つになり、行を移すたびに別の表へ切り貼りすることになる。

### 理由

- 観点 2: OS 層の機能に届く手段は、型の名前・名前空間の関数・型に付く関数・その型の値のメソッドしかない（標準ライブラリの名前は前置きで、他の参照の手段がない）。A はその全部の位置を閉じる。
- 観点 3: エラーの位置が名指しした箇所になり、直し方が明確。高階関数（`apply_all`・`List.map`）に OS の機能を使う関数を渡す書き方は禁じないので、核に置いた高階関数を OS のモジュールから使える。

### 選ばなかった案と理由

- **B（到達の検査）**: モジュールどうしの参照がない P0 では、到達しうる先は標準ライブラリだけで、A と結果が同じになる。関数の値を通した到達まで禁じると、核の高階関数に OS の機能を渡せなくなる（Rust の `core` が閉包を受け取れるのと同じ使い方ができない）。
- **C（効果だけ）**: `Path.new`・`Path.join` は効果を持たないので止められない。`IoError` を型として使うことも止められない。

## 決定 5: 信頼基盤（`trust`）との関係

- `trust`（検査を外せるか）と `bare`（OS なしで動くか）は **独立した 2 つの軸** とし、互いの規則を免除しない。

| `trust` | `bare` を含む | 例 |
|---|---|---|
| `none` | はい | 利用者の核のモジュール（決定 6 の `01-average`） |
| `none` | いいえ | 通常のアプリケーション（`02-read-port`・`04-effects`） |
| `base` | はい | `std.core`、P6 のプラットフォーム層（ハードウェアを直接操作する最小部分） |
| `base` | いいえ | `std.os`（M5 の暫定ランタイムが本体を供給する境界） |

- P6 のプラットフォーム層が OS の代わりの機能（シリアルの出力など）を供給するときも、`std.os` を核から使えるようにはせず、`bare` で使える項目とカテゴリを ADR で加える。
- `trust base` を宣言できる範囲、検査を外した箇所の契約の書き方、条件 12 の大域の状態の例外は、本 ADR では決めない（[ADR 0016](0016-trust-base-declaration.md) で決めた）。

## 決定 6: 核のモジュールのサンプル

- `examples/01-average/c-brace.sigil` の宣言を `targets bare, browser, server` に改め、核のモジュールのサンプルとする（`List`・`to_float`・`min` だけを使い、OS 層を使わない）。`01-average/c-brace.spec` の期待出力の出力先の行が `実行環境なし, ブラウザ, サーバー` になる。M5 の条件 10（核だけのプログラムの実行ファイル）の対象にもこのサンプルを使う。
- 他のサンプルは変えない（`03-ownership`・`05-control` は `console` を使う `main` を持つか持ちうるので、核にしない。変更の範囲を 1 つに絞る）。

## M4 の完了条件に加えるもの

ADR 0006 の M4 の完了条件の「核の部分と OS の機能を使う部分の分割」と「条件 10 のテスト」を、次のテストで満たすとする。テストは新しい `tests/m4_core_split.rs` に置く（構文と仕様の行は既存の `tests/errors.rs`・`tests/spec.rs` の流儀に合わせてよい）。

### 構文・整形・仕様

| テスト名（案） | 内容 | 期待 |
|---|---|---|
| `targets_bare_parses` | `targets bare`、`targets bare, server`、`targets bare, browser, server` | 構文解析を通る。`sigil fmt --check` で変わらない |
| `targets_bare_unsorted` | `targets browser, bare` | `TargetsUnsorted` |
| `targets_bare_duplicate` | `targets bare, bare` | `TargetsDuplicate` |
| `unknown_target_lists_bare` | `targets metal` | `UnknownTarget`。説明に `bare・browser・server` が出る |
| `spec_shows_bare` | `01-average` の `sigil spec` | `出力先: 実行環境なし, ブラウザ, サーバー` |

### 効果の表（決定 3）

| テスト名（案） | 内容 | 期待 |
|---|---|---|
| `bare_column_is_state_only` | `EffectTable::standard()` の全カテゴリについて `allowed_on(c, "bare")` | `state` だけ真 |
| `bare_rejects_declared_console` | 核のモジュールの関数が `effects console` を宣言（本体は空） | `EffectTarget` |
| `bare_rejects_task_effects` | 核のモジュールの関数が `effects task.suspend` を宣言 | `EffectTarget`（条件 10 の並行処理の側） |
| `bare_allows_state` | 核のモジュールで `Shared.new(Cell.new(1))` の `get`・`set`、`effects state` | 通る |

### 参照（決定 4。条件 10 の M4 側）

すべて `targets bare` のモジュールで書き、`OsItemInCore` を期待する。

| テスト名（案） | 書くもの |
|---|---|
| `core_rejects_console_log` | `console.log(&1);` を呼ぶ関数（`effects console` を宣言しても、`OsItemInCore` が先に出る） |
| `core_rejects_fs_read` / `core_rejects_fs_write` | `fs.read(&p)` / `fs.write(&p, &s)` |
| `core_rejects_path_param` | `fn f(p: &Path) -> Bool` |
| `core_rejects_ioerror_in_return` | `fn f() -> Result<Int, IoError>` |
| `core_rejects_path_new` | `let p = Path.new(&s);` |
| `core_rejects_path_annotation` | `let p: Option<Path> = None;` |
| `core_rejects_os_type_in_struct_and_enum` | `struct S { p: Path }`、`enum E { A(IoError) }` |
| `core_rejects_os_type_in_fn_type` | 関数の型の位置: `fn f(g: fn(&Path) -> Bool) -> Bool`、`fn f(g: fn(Int) -> Option<IoError>)` |
| `core_rejects_os_type_in_closure` | クロージャの中の型の位置: `xs.map(\|x\| { let p: Option<Path> = None; return x; })`。クロージャの引数には型注釈の文法がない（`Expr::Closure` の引数は名前だけ）ので、クロージャの本体の `let` の注釈で確かめる。引数の型注釈を文法に加えるときは、同じ `resolve` を通すので同じエラーになることをこのテストに足す |
| `core_with_server_still_rejects` | `targets bare, server` のモジュールで `console.log`（`server` があっても核なら拒否） |
| `core_trust_base_still_rejects` | `trust base` かつ `targets bare` で `console.log`（決定 5） |

### 通る例

| テスト名（案） | 内容 |
|---|---|
| `core_allows_core_items` | 核のモジュールで `List`・`Option`・`Result`・`String` のメソッド、`to_float`・`to_int`・`Int.min_value()`・`clone`、`List.map` にクロージャを渡す |
| `example_01_is_core` | `01-average` が `targets bare, browser, server` で `check` を通り、`average` の実行結果が M3 と同じ |
| `hosted_passes_console_fn_to_core_hof` | `targets server` のモジュールで、`console` を起こすクロージャを核の高階関数（`std.core` の `List.map`）に渡す。`effects console` で通る（決定 4 の「許すもの」の関数の値）。P0 には利用者のモジュールどうしの参照がないので、核の側は標準ライブラリの高階関数で確かめる |
| `core_generic_accepts_path_type_arg` | `targets server` のモジュールで、核の総称の型と関数に `Path` を型引数として渡す（`List<Path>` の `new`・`push`・`get`・`len`、`Option<Path>`）。通る（決定 4 の「許すもの」の型引数） |
| `hosted_modules_unchanged` | `targets server` のモジュールで `console.log`・`fs.read`・`Path.new` が従来どおり通る（既存のテスト `tests/m4_effects.rs`・`tests/m3_fs_cell.rs` がすべて通ることでも確かめる） |

### 表の不変条件（`src/stdlib.rs` の単体テスト）

| テスト名（案） | 内容 |
|---|---|
| `os_layer_is_exactly_the_adr_list` | `Os` の行の名前の集合が、決定 2 の一覧の OS 層（`console.log`・`fs.read`・`fs.write`・`Path` とその 5 関数・`IoError`）とちょうど一致する。行を層の間で移すと落ちるので、移すには ADR を改める必要がある |
| `core_rows_do_not_mention_os_types` | `Core` の行のシグネチャに `Path`・`IoError` が現れない（核の表が核だけで閉じている） |
| `core_rows_effects_allowed_on_bare` | `Core` の行の効果（`of(...)` を除く）がすべて `bare` で使える |

### 複数のファイル

| テスト名（案） | 内容 | 期待 |
|---|---|---|
| `run_sources_rejects_mismatched_modules` | `targets server` のファイルと `targets bare` のファイル（`console.log` を呼ぶ）を `run_sources` に渡す（どちらの順でも） | `RunError::Check`（`ModuleMismatch`） |
| `run_sources_rejects_mismatched_name_and_trust` | `name` だけ、または `trust` だけが違う 2 ファイル | `RunError::Check`（`ModuleMismatch`） |
| `run_sources_rejects_mismatched_targets_only` | `targets` だけが違う 2 ファイル | `RunError::Check`（`ModuleMismatch`） |
| `run_sources_accepts_same_module` | 宣言が同じ 2 ファイル（核の宣言どうし） | 通る |
| （既存）`tests/m3_fs_cell.rs` | `examples/04-effects` の 2 ファイル | 従来どおり通る |

ADR 0006 の条件の表の条件 10 の M4 の欄に、上の参照のテストの名前を書き加える。

## M5 に引き継ぐこと

ADR 0006 の条件 10 の M5 側（核の部分だけを使うプログラムの実行ファイルが、暫定ランタイムの `console`・`fs.read` に依存しない）を確かめやすくするため、M5 の前の「出力先とコンパイラ構成」の ADR で次を検討する（本 ADR では決めない）。

- 暫定ランタイムを、核の部分（確保と解放・破棄・異常停止と終了コード 70）と OS 層（`console`・`fs`・`Path`）の 2 つの静的ライブラリに分け、プログラムのモジュールがすべて核なら OS 層をリンクしない。テストは「`01-average` の入口の実行ファイルが核のライブラリだけでリンクできる」とする。

## 実装の手順

この ADR に従い、次の 3 つの PR に分ける（どの変更で何が変わったかを追えるようにするため）。

1. **PR1: 出力先 `bare` と効果の列**: `src/parser.rs` の `TARGETS` に `bare` を加え、`UnknownTarget` の説明を改める。`src/spec.rs` で `bare` を「実行環境なし」と訳す。`src/effects.rs` の `Targets` に `bare` を加え、`ALL` を `HOSTED` に改名、`ANY` を加える（決定 3）。上の「構文・整形・仕様」「効果の表」のテスト。
   - 注記: `TARGETS` 配列は辞書順で `bare` を先頭に置く（`&["bare", "browser", "server"]`）。並びの検査（`TargetsUnsorted`）と説明の列挙がこの順に従う。
   - 注記: `Targets` に欄を足すので、`Targets::SERVER`・`Targets::BROWSER` にも `bare: false` が要る（`HOSTED` も `bare: false`、`ANY` だけ `bare: true`）。`allows` に `"bare" => self.bare` の腕を足す。
2. **PR2a: 層と参照の検査**: `src/stdlib.rs` の `Layer` と各表の列、`std.core`・`std.os` の見出し。`src/check.rs` の `core`・`os_items` と 4 つの位置の検査、`src/error.rs` の `OsItemInCore`。`01-average` の宣言と `.spec`。上の「参照」「通る例」「表の不変条件」のテスト。ADR 0006・0004 などの追記（下の「影響」）。
3. **PR2b: 複数のファイルの宣言の一致**: `src/error.rs` の `ModuleMismatch`、`src/interp.rs` の `run_sources` の宣言の一致の検査。上の「複数のファイル」のテスト。PR2a と独立して入れられるが、PR2a の後に入れる（核の検査が外れる経路を塞ぐ変更なので、核の検査が先にある方が意味を確かめやすい）。

## 影響

この ADR に従い、実装の PR（PR1・PR2a・PR2b）で次を反映した。

- **ADR 0005**: 影響の「標準ライブラリの分割」の「核の部分（メモリ確保に依存しない部分）」に、本 ADR の決定 2 で「OS に依存しない部分（確保は使う）」と改め、確保なしの部分は P6 で分けることを追記する。
- **ADR 0004**: 規則 1 の表の「使える出力先」に「実行環境なし」を加える（`state` だけ）。規則 5 に、`bare` の語と核のモジュールの参照の規則（本 ADR の決定 4）への参照を追記する。
- **ADR 0006**: 決定 4 の表の「標準ライブラリの分割」を解決とする。M4 の「完了条件の充足」の未達の 2 点を、本 ADR の完了条件のテストに置き換える。条件 10 の行の M4 の欄にテスト名を書く。
- **ADR 0008**: 決定 6 の「P0 で書けるのは `server` と `browser`」に、本 ADR で `bare` を加えたことを追記する。
- **ADR 0013**: 決定 2 の「効果を持つ部分（M4 の前に正式に決める）」に、本 ADR の決定 2 で正式に決めたことを追記する。
- **docs/DESIGN.md**: 未決の論点の表の「標準ライブラリの分割」を決定とする。
- **README.md**: ステータスの ADR の行に本 ADR を加える。
- **examples**: `01-average/c-brace.sigil` の宣言と `c-brace.spec`（決定 6）。
- 依存の追加、CI の設定の変更はない。

## 委任による決定の記録

次の 3 点は、所有者の委任を受けて Claude が決めた。

1. **ADR 0005 の「核」の定義を改める**: ADR 0005 の影響「標準ライブラリの分割」の「核の部分（メモリ確保に依存しない部分）」を、「OS に依存しない部分（確保は使う）」に改める。確保なしの層は P6 の ADR で分ける（決定 2）。ADR 0005 は承認済みで長期目標（OS）に関わるため、この改めは所有者へ事後報告とする。
2. **出力先の語は `bare`、仕様の訳は「実行環境なし」とする**（決定 1）。ADR 0008 の決定 6 の「出力先の語は ADR に記録してから加える」の記録を本 ADR とする。
3. **信頼基盤の宣言方法の ADR は別途後で書く**: `trust base` を宣言できる範囲・検査を外した箇所の契約の書き方・条件 12 の大域の状態の例外は、本 ADR では決めない（決定 5）。本 ADR は `trust` を独立の軸とし、その ADR に依存しない。「モジュールの単位と出力先の指定」の ADR にも依存しない（複数ファイルは宣言の一致を求めるだけ。決定 4）。

## 根拠の限界と再検証

- 本 ADR の決定は、所有者の委任を受けて Claude が行った（ADR 0005 の「核」の定義の改めを含む。上の「委任による決定の記録」）。所有者への事後報告で異論が出た場合は、この ADR を見直す。
- 候補の案出しと採点を同じ者（Claude）が行っており、偏りがありうる。
- 決定 4 の「名指しの位置で漏れなく閉じる」は、P0 にモジュールどうしの参照がなく、標準ライブラリの名前が前置きで見えることに依存する。モジュールの単位の ADR で取り込みの仕組みを入れるときは、取り込んだ利用者のモジュールの `targets` に `bare` がなければ核から参照できない、という同じ規則に広げる（決定 4 の B の到達の検査に切り替えるかもそこで判断する）。
- 決定 2 の 2 層は、P6 で確保なしの部分が要ることを前提に繰り延べている。P1〜P5 の間に、確保できない環境（割り込みの処理など）で動くコードが必要になった場合は、その時点で層を加える。
- 決定 3 の `clock`・`random` を `bare` で使えないとしたのは、P0 の核に時刻・乱数の源がないためである。P6 のプラットフォーム層の設計で改める。
- M4 の完了後、次を計測する。結果が本決定と大きく食い違う場合は、この ADR を見直す。
  - `OsItemInCore` が出た回数と、そのうち `bare` を外して直したもの・OS 層を使わない書き方に直したものの数（`bare` を外す直しばかりなら、核の範囲が狭すぎる疑いがある）
  - 核のモジュールで `EffectTarget` が出た回数と、そのカテゴリ（`clock`・`random`・`task.*` が多ければ、決定 3 の列を P6 より前に見直す）
  - `ModuleMismatch` が出た回数（多ければ、モジュールの単位の ADR を前倒しする）
  - 核の行と OS 層の行の間で、表の行を移す変更が要った回数（`os_layer_is_exactly_the_adr_list` が落ちた回数）

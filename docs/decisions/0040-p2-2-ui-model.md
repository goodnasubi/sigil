# 0040: P2-2 UI の記述モデル

- **状態**: 提案（typesafe-ai の正式判定を末尾に記録。別系統のレビュー前）
- **日付**: 2026-11-17（JST）
- **対象**: [Issue #557](https://github.com/goodnasubi/sigil/issues/557)、レーン E

## 背景

[ADR 0038](0038-p2-scope.md) の二番目の段 P2-2 を、[ADR 0039](0039-p2-1-javascript-output.md) の JS 出力の上に具体化する。AI が実装し、人間が仕様だけをレビューする原則（[ADR 0001](0001-vision-and-scope.md)）に従い、表示・状態遷移・作用の境界を仕様で読めるようにする。

起草の基準は `main` の `3748e33`。0040 は main のファイル・索引とオープン PR で未使用を確認した。本 PR は本 ADR の新規作成と索引追加だけである。以下は将来の API・CLI の決定であり、現行の処理系や `sigil spec` の対応機能を意味しない。

## 選択肢と決定

| 判断 | 採用する案 | 他案を採用しない理由 |
|---|---|---|
| 表示 | 純粋な `view` が型付きの木を返し、明示的な `model`・`event`・`update` で更新 | DOM の直接変更を利用者の通常 API にすると状態と表示が乖離する。JSX・React 互換は新しい構文と互換性の責務が増える |
| イベント | 所有されたマウントの中で同期的に一件ずつ更新。保存するコールバックは純粋 | 任意の非同期クロージャや隠れた共有状態は寿命と効果を追えなくする |
| 安全性 | typed DOM、許可一覧、文字列のテキスト表示、入力の明示検証 | HTML／JS 文字列注入と任意属性は、表示値を実行権限に変えてしまう |
| 実装範囲 | P2-1 の核に下記 UI API と `dom` を加える | 通信・共有状態・待機を同時実装すると P2-3 と並行処理の責務を先取りする |

## 決定 1: 構文・型・標準操作の対応集合

新しい JSX 構文・効果カテゴリは加えない。`targets browser`、既存の関数の仕様四節・構造体・列挙型・総称型・篩型、リテラル・一覧・`match`・借用・局所代入・関数値・`move` クロージャを用いる。構文・型・標準操作の核は ADR 0039 決定 1 の許可一覧をそのまま含める（数値／Bool／String／Bytes／List／Option／Result／組／利用者型、契約、標準の文字列・一覧操作、console）。以下だけを追加する。

`std.ui` は browser 専用の標準モジュールとし、公開名は `ui`。`Root` と `Mount` は複製・比較・共有・送信できない所有資源、`Node<E>` と `Attr<E>` は所有された宣言値とする。`Tag`・`InputKind`・`UiError` は閉じた列挙型、`Url` は検査済みの不透明な所有文字列。`E` は借用・関数・ホスト資源を含まないイベントの型とする。`M` も借用・ホスト資源を含まない所有型とし、コールバックをモデルに格納しない。関数値を型引数に直接入れない既存規則を維持する。

保存する関数は、既存の関数型をフィールドに置く規則（[ADR 0022](0022-type-identity-traits-and-function-types.md) 決定 3）で次の入れ物に収める。

```sigil
struct Program<M, E> {
  view: fn(&M) -> Node<E> effects none,
  update: fn(&mut M, E) -> Unit effects none,
}
struct InputMap<E> {
  event: fn(String) -> E effects none,
}
struct CheckMap<E> {
  event: fn(Bool) -> E effects none,
}
```

これらは `std.ui` 内の宣言の形であり、利用者は `ui.Program` 等で参照する。`Node`／`Attr` の内部表現は公開しない。借用の捕捉は禁止、保存する `move` クロージャの捕捉値は既存どおり Sendable・Shareable で不変、関数値自体は複製しない。API の対応一覧は次で閉じる。表の型引数・借用は通常の Sigil のもの、`none` は効果なしを表す。

| 操作 | シグネチャ | 効果・条件 |
|---|---|---|
| テキスト | `ui.text<E>(value: String) -> Node<E>` | none。HTML として解釈しない |
| 表示への変換 | `ui.display<T: Displayable>(value: &T) -> String` | none。console と同じ標準表示 |
| 要素 | `ui.element<E>(tag: Tag, attrs: List<Attr<E>>, children: List<Node<E>>) -> Node<E>` | none。属性と子の適合は描画前に検査 |
| 一覧 | `ui.fragment<E>(children: List<Node<E>>) -> Node<E>` | none。List の `map` やループで作る。key／差分 API は加えない |
| 通常属性 | `ui.id<E>(String)`、`ui.class_name<E>(String)`、`ui.title<E>(String)`、`ui.label_for<E>(String)`、`ui.name<E>(String)`、`ui.placeholder<E>(String)` | すべて `-> Attr<E>`、none |
| 型付き属性 | `ui.disabled<E>(Bool)`、`ui.checked<E>(Bool)`、`ui.required<E>(Bool)`、`ui.value<E>(String)`、`ui.input_kind<E>(InputKind)`、`ui.href<E>(Url)` | すべて `-> Attr<E>`、none |
| クリック／送信 | `ui.on_click<E>(event: E)`、`ui.on_submit<E>(event: E)` | `-> Attr<E>`、none。`E: Copyable`、発火ごとにイベントを複製 |
| 入力／チェック | `ui.on_input<E>(map: InputMap<E>)`、`ui.on_check<E>(map: CheckMap<E>)` | `-> Attr<E>`、none。保存した変換関数の上限は none |
| URL の検査 | `ui.url(value: String) -> Result<Url, UiError>` | none。許可規則は決定 4 |
| 描画開始 | `ui.mount<M,E>(root: Root, model: M, program: Program<M,E>) -> Result<Mount, UiError>` | dom。成功なら root・model・program を所有、失敗なら消費した値を破棄 |
| 明示終了 | `ui.unmount(mount: Mount) -> Unit` | dom。登録解除して所有値を破棄 |

引数名を省略した表の単一型引数は `value` を引数名とする。`ui` の全操作は通常どおり intent・requires・ensures・effects を持つ標準操作として登録し、上記は簡略シグネチャである。新しい構文や標準 API は本 PR には実装しない。

`Tag` の腕は `Div, Span, P, H1, H2, Ul, Ol, Li, Button, Label, Input, Textarea, Form, A`、`InputKind` は `Text, Password, Checkbox, Submit, Button`。`UiError` は `InvalidTree, InvalidUrl, LimitExceeded, HostFailure`。任意タグ・任意属性・SVG・custom elements・生の DOM ノード・ファイル入力・contenteditable は未対応。セレクターで任意 DOM を取得する API も設けない。

## 決定 2: 純粋な view と明示的な更新

例えばカウンターは次の形で記述する（新設する ui API を用いた将来の構文例）。

```sigil
module counter
  targets browser
  trust none

struct Model { count: Int }
enum Event { Increment }

fn view(model: &Model) -> ui.Node<Event>
  intent "現在値と加算ボタンを表示する"
  requires none
  ensures none
  effects none
{
  return ui.element(ui.Tag.Div, [], [
    ui.text(ui.display(&model.count)),
    ui.element(ui.Tag.Button, [ui.on_click(Event.Increment)], [ui.text("増やす")]),
  ]);
}

fn update(model: &mut Model, event: Event) -> Unit
  intent "イベントに応じてカウンターを増やす"
  requires none
  ensures none
  effects none
{
  match event {
    Event.Increment => { model.count = model.count + 1; },
  }
}

fn main(root: ui.Root) -> Result<ui.Mount, ui.UiError>
  intent "所有された描画領域にカウンターを開始する"
  requires none
  ensures none
  effects dom
{
  return ui.mount(root, Model { count: 0 }, ui.Program { view: view, update: update });
}
```

初期化は `view(&model)`→木の検査→描画。イベント入口は DOM の最小ペイロードをコピーして `E` を作り、`update(&mut model, event)`→`view(&model)`→木の検査→再描画を一件ずつ同期実行する。`&mut M` の変更は既存規則どおり効果ではない。描画器・登録解除・DOM 読み出しを行うイベント入口には `dom` を宣言し、標準操作／生成する入口の効果情報にも残す。純粋な view が DOM の読み出しや I/O を行うことはない。

コールバックの効果上限は `none`。フィールド型への代入で上限を検査し、呼び出し側はそのフィールド型の効果を含めて完全一致検査する。`dom`・`console` を起こす view、`state` を起こす update、効果を持つ入力変換は負例として拒否する。`List.map` 等を経由しても同じである。`console` は通常の入口外の関数では P2-1 同様に利用できるが、この保存コールバックの上限を広げない。

`Shared`／`Cell` を使う書き換えには `state`、通信には `net.client`、応答待ちには `task.suspend`、タスクには既存 `task.*` を要求する規則を維持する。それらを宣言 UI の裏に隠さない。P2-2 の JS 対応集合は `none`・`console`・`dom` に限り、共有状態・通信・待機・タスクの実行 API は生成前に未対応として拒否する。単独所有 model の局所更新で三つの完了例は実現できる。通信を持つ更新や非同期コールバックは P2-3 等の別 ADR で対応集合を広げる。

## 決定 3: イベントの寿命・再描画・破棄

`Mount` が model・program・現行の木・イベント変換関数と捕捉値・DOM 登録を一意に所有する。DOM Event、target、JS の関数、借用は Sigil に渡さない。input の現在値を妥当性検査した String、checkbox を Bool にコピーする。クリックと submit は保存した E の複製を渡す。submit は常に `preventDefault` し、ブラウザの暗黙のフォーム送信を起こさない。on_click は Button、on_submit は Form、on_input は Input(Text/Password)／Textarea、on_check は Input(Checkbox) にだけ許可する。

初版は差分描画・key による同一性を保証しない。木を前順に描画し、更新時は古い登録を解除して root の子を置換する。入力中の操作を成立させるため、フォーカスと選択範囲は root 内の一意な `id` と適合する入力型が新旧で一致するときだけ復元する。value／checked は model から毎回設定し、入力直後に model へ反映する。IME の composition 中は中間 input を配送せず、compositionend の確定値を一回配送し、同じ確定を続く input で二重配送しない。マウント外のフォーカスを変更しない。

描画単位ごとの世代番号を登録に付ける。解除済み／古い世代のイベントは model に届かない。処理中に同じマウントへ再入しない。ホストが同期的に再入したら host failure とし、そのマウントを停止する。ブラウザの通常のイベントループは待機効果の隠蔽ではなく外部入口の呼び出しであり、Sigil の処理中に非同期処理を挟まない。

新しい木を検査してから DOM を変更する。重複 id／不適合属性／上限超過は `InvalidTree`／`LimitExceeded`。初回は Result の Err、イベント中は停止として controller に記録する。update／view の契約違反・算術停止も停止とし、更新済み model をロールバックして継続しない。DOM 例外で一部反映した場合も、イベント登録をすべて解除して root を空にし、停止させる。最初の失敗を cleanup の二次失敗で上書きしない。

`unmount` と controller の dispose は登録解除→DOM の切り離し→現行木と捕捉値・program・model の破棄→Root の権利解放を行う。通常のスコープ終了でも Mount の組み込み破棄で同じ cleanup を実行する。DOM を取り除く資源破棄には `dom` が必要で、暗黙 Drop の効果も呼び出し側に伝播させ完全一致検査する（メモリ解放自体に効果は加えない）。この契約を表すため必要な SIR の変更は実装前に別の契約変更 PR とする。

所有値の Move／Drop／cleanup、部分移動・逆局所順・欄順・異常停止と `--trace-drops` は ADR 0039 決定 3 を維持する。GC／FinalizationRegistry に登録解除や捕捉値の破棄時点を委ねない。切り離された古いノードの通知・二重 dispose は無害で、破棄を二重実行しない。タブ強制終了／ホストのメモリ枯渇による cleanup 非保証は通常停止と区別する。

## 決定 4: typed DOM・許可属性・URL・HTML/CSS

テキストは `createTextNode`／`textContent`、入力値は DOM の value／checked プロパティで設定する。`<>&"'` を含む値をマークアップやスクリプトとして解釈しない。HTML シリアライズ経路を将来設けるなら文脈に沿って escaping するが、初版に `innerHTML`・`outerHTML`・`insertAdjacentHTML`・`eval`・Function・任意 JS／HTML の文字列入口を設けない。

通常属性は id・class・title、Label の for、フォーム部品の name・placeholder・required・disabled、Input の type／value／checked、Textarea の value、A の href だけ。属性は要素ごとの上記用途に限り、一つの要素で同じ属性・イベントを重複させない。Input は子なし、Textarea は value を使い子なし。Button は常に DOM type=button とし、送信ボタンは Input(Submit) で表す。form の action／method、任意の on*・style・src・srcdoc・data-*・aria-*・target・download は許可しない。属性名を文字列から作る API はない。

`ui.url` は `/` で始まるオリジン内の絶対パス（`//` は不可）または `#` で始まるフラグメントだけを許す。空値、制御文字・空白・バックスラッシュ・不正な %HH、percent decode 後にこれらや先頭 `//` が現れる値、スキーム付き／相対 URL は拒否する。ページの base URL に依存させず、host が渡す document の origin を基準に解決し、同一 origin を再確認する。`javascript:`・`data:`・外部 origin を通さない。href はユーザーのリンク操作による移動だけを許し、自動 fetch・リソース読み込みを追加しない。HTTP の画面遷移と Sigil の `net.client` 操作を混同しない。

配置・装飾は通常の HTML と CSS、許可された class/id で行う。所有者が用意する静的 HTML に root と CSS を置く。ui は CSS 文字列・外部 stylesheet URL を受け取らず、ホストがロードした CSS 自体は信頼境界外である。CSS まで安全に隔離したと主張しない。SSR・hydration・ルーティング・UI ライブラリ互換・自動資産配信は必須にしない。

## 決定 5: 入力検証・数値・文字列 ABI

入力は常に未検証 String／Bool として event に入れ、update で `String.parse_int` と `type Age = Int where self >= 0 && self <= 150;` の `Age.check` 等を Result として処理する。不正入力はモデルのエラー欄に保存して ui.text で表示し、有効な値への更新と分ける。requires／ensures と篩型の実行時検査を省かず、型検査成功を入力条件の証明と表示しない。UI の required/type 表示を検証の根拠にしない。将来サーバーへ送信する際も、受信サーバーで構造・篩型・件数・サイズ・認証を再検証する。UI 検証をサーバーの信頼境界に使わない。

Int は P2-1 と同じ i64 BigInt、検査算術・添字変換を維持する。Float は binary64 Number、NaN／±inf／-0 を維持し、表示は標準 Displayable、入力との変換を DOM／JS の暗黙の Number 変換に任せない。文字列 ABI も ADR 0039 決定 2 を維持し、妥当な UTF-16（Unicode スカラー列）、正規化なし、len はスカラー数、UTF-8 は厳格変換とする。DOM からの孤立サロゲートは置換せず境界失敗にする。補助平面・結合文字・改行をそのまま表示する。

初版のマウント上限は木の深さ 256、Node 数 10000、各要素の属性数 32、単一文字列の UTF-8 サイズ 1 MiB、木全体の文字列合計 8 MiB、URL 4096 UTF-8 bytes とする。検査・描画・破棄は深さに応じた明示スタックを使い、JS スタックへの依存を避ける。DOM の選択範囲の UTF-16 添字は JS ホスト内の不透明な値で、Sigil の String.len／Int と混同しない。入力取り込みも割り当て前にサイズを検査する。

## 決定 6: ホスト呼び出し・CLI・診断

P2-1 の通常の `invoke(args, host)` と値 ABI を維持し、Root／Mount／Node／関数値をその汎用境界で受け渡さない。UI 用だけ明示モードを追加する。

```text
sigil build --emit js --ui [--root <根>] [--trace-drops] <ファイル> <関数> -o <出力.mjs>
```

`--ui` の入口は `fn(ui.Root) -> Result<ui.Mount, ui.UiError>`、effects dom の一つに限定し、引数の型・戻り値・効果を生成前に検査する。生成した module は `mount(rootElement, host)` を export し、一つの空の Element を Root として入口へ渡す。生成モジュールは document を暗黙取得せず、rootElement の ownerDocument だけを使う。rootElement は既存のマウントと重複せず、接続済み・同じ realm・空であることを検査する。html／head／body を root にしない。root の範囲外を変更する権利は渡さない。

戻りは同期 `{ok:true, controller}` または `{ok:false, failure}`。controller は JS 側の `dispose()` と `status()` のみを公開し、Sigil の値を外へ漏らさない。status は running／disposed／failed とコピーした failure 情報、dispose は冪等で同期の結果を返す。失敗の kind は P2-1 の entry／stop／host／internal を維持し、ui の Result の Err は kind=entry と UiError を含める。イベント中の失敗は status に保存し、新しいイベントを拒否する。イベントを任意 JS から注入する API は設けない。

DOM ホスト呼び出しは生成した閉じたアダプター内の createElement・createTextNode・setAttribute（許可一覧のみ）・value/checked 設定・replaceChildren・add/removeEventListener・preventDefault・focus/selection の読み書きだけ。アダプターは DOM Event を保持しない。これらは `dom` の実装で、`foreign` や利用者の trust base を追加しない。ホストの任意コードを Sigil の検査で隔離したことにはしない。Node.js／npm／任意 FFI・動的 import・Wasm を要求しない。`host.log` と `host.traceDrop` は P2-1 と同じ必要時だけの同期能力で、Promise・例外・再入は境界失敗。DOM API からの例外も cleanup に接続する。

通常の --emit js は P2-1 のまま、--emit native と既定 native、--root の解決も維持。native の --ui、未知・重複・矛盾したオプション、不正な入口は終了 1、成功は 0。`sigil run` をブラウザ起動コマンドに変えない。HTML/CSS 自動生成・配信 CLI は足さない。

通常の型・所有権・効果検査と SIR 検証後、生成前に入口・callback・契約・cleanup の依存閉包を走査する。未対応の型／操作／効果は位置・依存元・出力先 js を含めて診断し、部分成果物を残さず既存出力も上書きしない。Shared／Cell・通信・待機、サーバー専用操作、未対応イベントと任意属性をダミー値や別出力への fallback で補わない。Node の意味検査は描画前、静的に不正と判定できるものは生成前に診断する。

## 決定 7: P2-2 の完了の確かめ方

| 確認 | 必要な正常例・負例と観測 |
|---|---|
| カウンター | 実ブラウザで複数回クリックし状態と表示が一致。update のオーバーフロー・view の契約違反で停止し、登録と所有値が解放される |
| 入力検証 | 入力→event→parse_int／篩型→成功値またはエラー表示。空・負数・上限超過・巨大値、IME 確定、チェック状態、focus／選択を確認。submit で暗黙通信しない |
| 一覧更新 | model の List を追加・削除・並べ替えて再描画。古い要素のイベントが届かず、新しい要素だけが反応。key 最適化なしでも正しい表示になる |
| 効果検査 | 純粋 view/update/input へ dom／state／console／net.client／task.suspend を入れた負例、dom を欠く描画・イベント入口・暗黙破棄を拒否。余分な宣言も完全一致で拒否 |
| 安全な表示 | `<script>`・`<img onerror=...>`・引用符・&・補助平面・結合文字が文字として表示され、要素やコードにならない。禁止属性・URL・孤立サロゲート・上限超過は DOM 更新前に拒否 |
| 寿命とホスト | 初期化失敗、連続再描画、unmount、二重 dispose、古いイベント、ホスト例外／再入、言語停止時に登録解除と破棄を一回実施。root 外に変更なし |
| 核の一致 | P2-1 の数値・文字列・契約・所有権の比較試験を維持。純粋 update/view の結果（木を標準の型付き構造へ正規化）と破棄列を SIR 側の期待値・実行結果と比較。DOM 統合は実ブラウザで別に検証 |

Chromium／Firefox／WebKit で最低対応版を記録し、同じ三例を Node.js・Wasm なしで操作できることを要求する。UI 操作と拒否例は自動ブラウザ試験、標準操作の全許可一覧は正常例と必要な失敗例で確かめる。表示画面だけや Node.js での成功だけを完了としない。P2-4 で流用できるホスト境界と例を残すが、Wasm の実装を先取りしない。

## 影響

標準 ui の型と効果付き資源破棄、JS の対応集合・閉じた DOM アダプター・ブラウザ適合試験が必要になる。新しい信頼基盤や依存の包括承認ではない。必要な AST／SIR 契約変更は別 PR とし、契約変更→ADR→実装→サンプル・テストの順を守る。本 Issue は ADR だけの PR 作成で完了し、この検証表の実装は含めない。P2-3 以降や既存 ADR の改訂、P1 の公開前課題 #547〜#552 は本 PR で変更・再起票しない。

## 正式判定の記録

2026-11-16 の所有者指示に従い、Gemini Decisions は使わず typesafe-ai のみを使用した。2026-11-17（JST）、Codex Cloud（OpenAI 系統）から `jev-1.13.0` を二回呼び出した。初回の後、Button は常に type=button、送信は Input(Submit) とするよう許可属性の記述を明確にし、評価軸・質問を変えず最終案を再判定した。

両回とも Model=pure_owned、Safety=typed、Lifetime=owned、Completion=enumerated、各確率・confidence は 1.0。最終案の採用は二回目の結果に基づく。判定は正しさや安全性の証明ではなく、既存の検査規則と上記完了条件を採用理由とする。認証情報を含まない実際の送信 JSON（state は評価時の草稿全文）と応答 JSON を以下に保存する。

### 初回の入力

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間は仕様だけをレビューする（ADR0001）。所有者はIssue557でP2-2 UIの個別ADR0040と索引だけを指示し、実装・spec・既存ADR・バックログの変更を禁止。2026-11-16指示により正式判定はtypesafe-aiのみ。ADR0038の純粋view、model/event/update、dom等の実効果、typed DOMとtext escaping、既存state/net.client/task.suspend規則、入力のサーバー再検証、HTML/CSSを守る。ADR0039の核、BigInt i64、binary64、Unicode/UTF8、所有値Drop/cleanupとホスト失敗、明示CLIを維持。新JSXや効果カテゴリを追加せず、既存関数型フィールド規則でnone上限のcallbackを保存。以下草稿の判断がこの理念と指示に沿って明示的で検証可能かを評価する。確率は証明でなく設計判断の補助。\n\n# 0040: P2-2 UI の記述モデル\n\n- **状態**: 提案（typesafe-ai の正式判定を末尾に記録。別系統のレビュー前）\n- **日付**: 2026-11-17（JST）\n- **対象**: [Issue #557](https://github.com/goodnasubi/sigil/issues/557)、レーン E\n\n## 背景\n\n[ADR 0038](0038-p2-scope.md) の二番目の段 P2-2 を、[ADR 0039](0039-p2-1-javascript-output.md) の JS 出力の上に具体化する。AI が実装し、人間が仕様だけをレビューする原則（[ADR 0001](0001-vision-and-scope.md)）に従い、表示・状態遷移・作用の境界を仕様で読めるようにする。\n\n起草の基準は `main` の `3748e33`。0040 は main のファイル・索引とオープン PR で未使用を確認した。本 PR は本 ADR の新規作成と索引追加だけである。以下は将来の API・CLI の決定であり、現行の処理系や `sigil spec` の対応機能を意味しない。\n\n## 選択肢と決定\n\n| 判断 | 採用する案 | 他案を採用しない理由 |\n|---|---|---|\n| 表示 | 純粋な `view` が型付きの木を返し、明示的な `model`・`event`・`update` で更新 | DOM の直接変更を利用者の通常 API にすると状態と表示が乖離する。JSX・React 互換は新しい構文と互換性の責務が増える |\n| イベント | 所有されたマウントの中で同期的に一件ずつ更新。保存するコールバックは純粋 | 任意の非同期クロージャや隠れた共有状態は寿命と効果を追えなくする |\n| 安全性 | typed DOM、許可一覧、文字列のテキスト表示、入力の明示検証 | HTML／JS 文字列注入と任意属性は、表示値を実行権限に変えてしまう |\n| 実装範囲 | P2-1 の核に下記 UI API と `dom` を加える | 通信・共有状態・待機を同時実装すると P2-3 と並行処理の責務を先取りする |\n\n## 決定 1: 構文・型・標準操作の対応集合\n\n新しい JSX 構文・効果カテゴリは加えない。`targets browser`、既存の関数の仕様四節・構造体・列挙型・総称型・篩型、リテラル・一覧・`match`・借用・局所代入・関数値・`move` クロージャを用いる。構文・型・標準操作の核は ADR 0039 決定 1 の許可一覧をそのまま含める（数値／Bool／String／Bytes／List／Option／Result／組／利用者型、契約、標準の文字列・一覧操作、console）。以下だけを追加する。\n\n`std.ui` は browser 専用の標準モジュールとし、公開名は `ui`。`Root` と `Mount` は複製・比較・共有・送信できない所有資源、`Node<E>` と `Attr<E>` は所有された宣言値とする。`Tag`・`InputKind`・`UiError` は閉じた列挙型、`Url` は検査済みの不透明な所有文字列。`E` は借用・関数・ホスト資源を含まないイベントの型とする。`M` も借用・ホスト資源を含まない所有型とし、コールバックをモデルに格納しない。関数値を型引数に直接入れない既存規則を維持する。\n\n保存する関数は、既存の関数型をフィールドに置く規則（[ADR 0022](0022-type-identity-traits-and-function-types.md) 決定 3）で次の入れ物に収める。\n\n```sigil\nstruct Program<M, E> {\n  view: fn(&M) -> Node<E> effects none,\n  update: fn(&mut M, E) -> Unit effects none,\n}\nstruct InputMap<E> {\n  event: fn(String) -> E effects none,\n}\nstruct CheckMap<E> {\n  event: fn(Bool) -> E effects none,\n}\n```\n\nこれらは `std.ui` 内の宣言の形であり、利用者は `ui.Program` 等で参照する。`Node`／`Attr` の内部表現は公開しない。借用の捕捉は禁止、保存する `move` クロージャの捕捉値は既存どおり Sendable・Shareable で不変、関数値自体は複製しない。API の対応一覧は次で閉じる。表の型引数・借用は通常の Sigil のもの、`none` は効果なしを表す。\n\n| 操作 | シグネチャ | 効果・条件 |\n|---|---|---|\n| テキスト | `ui.text<E>(value: String) -> Node<E>` | none。HTML として解釈しない |\n| 表示への変換 | `ui.display<T: Displayable>(value: &T) -> String` | none。console と同じ標準表示 |\n| 要素 | `ui.element<E>(tag: Tag, attrs: List<Attr<E>>, children: List<Node<E>>) -> Node<E>` | none。属性と子の適合は描画前に検査 |\n| 一覧 | `ui.fragment<E>(children: List<Node<E>>) -> Node<E>` | none。List の `map` やループで作る。key／差分 API は加えない |\n| 通常属性 | `ui.id<E>(String)`、`ui.class_name<E>(String)`、`ui.title<E>(String)`、`ui.label_for<E>(String)`、`ui.name<E>(String)`、`ui.placeholder<E>(String)` | すべて `-> Attr<E>`、none |\n| 型付き属性 | `ui.disabled<E>(Bool)`、`ui.checked<E>(Bool)`、`ui.required<E>(Bool)`、`ui.value<E>(String)`、`ui.input_kind<E>(InputKind)`、`ui.href<E>(Url)` | すべて `-> Attr<E>`、none |\n| クリック／送信 | `ui.on_click<E>(event: E)`、`ui.on_submit<E>(event: E)` | `-> Attr<E>`、none。`E: Copyable`、発火ごとにイベントを複製 |\n| 入力／チェック | `ui.on_input<E>(map: InputMap<E>)`、`ui.on_check<E>(map: CheckMap<E>)` | `-> Attr<E>`、none。保存した変換関数の上限は none |\n| URL の検査 | `ui.url(value: String) -> Result<Url, UiError>` | none。許可規則は決定 4 |\n| 描画開始 | `ui.mount<M,E>(root: Root, model: M, program: Program<M,E>) -> Result<Mount, UiError>` | dom。成功なら root・model・program を所有、失敗なら消費した値を破棄 |\n| 明示終了 | `ui.unmount(mount: Mount) -> Unit` | dom。登録解除して所有値を破棄 |\n\n引数名を省略した表の単一型引数は `value` を引数名とする。`ui` の全操作は通常どおり intent・requires・ensures・effects を持つ標準操作として登録し、上記は簡略シグネチャである。新しい構文や標準 API は本 PR には実装しない。\n\n`Tag` の腕は `Div, Span, P, H1, H2, Ul, Ol, Li, Button, Label, Input, Textarea, Form, A`、`InputKind` は `Text, Password, Checkbox, Submit, Button`。`UiError` は `InvalidTree, InvalidUrl, LimitExceeded, HostFailure`。任意タグ・任意属性・SVG・custom elements・生の DOM ノード・ファイル入力・contenteditable は未対応。セレクターで任意 DOM を取得する API も設けない。\n\n## 決定 2: 純粋な view と明示的な更新\n\n例えばカウンターは次の形で記述する（新設する ui API を用いた将来の構文例）。\n\n```sigil\nmodule counter\n  targets browser\n  trust none\n\nstruct Model { count: Int }\nenum Event { Increment }\n\nfn view(model: &Model) -> ui.Node<Event>\n  intent \"現在値と加算ボタンを表示する\"\n  requires none\n  ensures none\n  effects none\n{\n  return ui.element(ui.Tag.Div, [], [\n    ui.text(ui.display(&model.count)),\n    ui.element(ui.Tag.Button, [ui.on_click(Event.Increment)], [ui.text(\"増やす\")]),\n  ]);\n}\n\nfn update(model: &mut Model, event: Event) -> Unit\n  intent \"イベントに応じてカウンターを増やす\"\n  requires none\n  ensures none\n  effects none\n{\n  match event {\n    Event.Increment => { model.count = model.count + 1; },\n  }\n}\n\nfn main(root: ui.Root) -> Result<ui.Mount, ui.UiError>\n  intent \"所有された描画領域にカウンターを開始する\"\n  requires none\n  ensures none\n  effects dom\n{\n  return ui.mount(root, Model { count: 0 }, ui.Program { view: view, update: update });\n}\n```\n\n初期化は `view(&model)`→木の検査→描画。イベント入口は DOM の最小ペイロードをコピーして `E` を作り、`update(&mut model, event)`→`view(&model)`→木の検査→再描画を一件ずつ同期実行する。`&mut M` の変更は既存規則どおり効果ではない。描画器・登録解除・DOM 読み出しを行うイベント入口には `dom` を宣言し、標準操作／生成する入口の効果情報にも残す。純粋な view が DOM の読み出しや I/O を行うことはない。\n\nコールバックの効果上限は `none`。フィールド型への代入で上限を検査し、呼び出し側はそのフィールド型の効果を含めて完全一致検査する。`dom`・`console` を起こす view、`state` を起こす update、効果を持つ入力変換は負例として拒否する。`List.map` 等を経由しても同じである。`console` は通常の入口外の関数では P2-1 同様に利用できるが、この保存コールバックの上限を広げない。\n\n`Shared`／`Cell` を使う書き換えには `state`、通信には `net.client`、応答待ちには `task.suspend`、タスクには既存 `task.*` を要求する規則を維持する。それらを宣言 UI の裏に隠さない。P2-2 の JS 対応集合は `none`・`console`・`dom` に限り、共有状態・通信・待機・タスクの実行 API は生成前に未対応として拒否する。単独所有 model の局所更新で三つの完了例は実現できる。通信を持つ更新や非同期コールバックは P2-3 等の別 ADR で対応集合を広げる。\n\n## 決定 3: イベントの寿命・再描画・破棄\n\n`Mount` が model・program・現行の木・イベント変換関数と捕捉値・DOM 登録を一意に所有する。DOM Event、target、JS の関数、借用は Sigil に渡さない。input の現在値を妥当性検査した String、checkbox を Bool にコピーする。クリックと submit は保存した E の複製を渡す。submit は常に `preventDefault` し、ブラウザの暗黙のフォーム送信を起こさない。on_click は Button、on_submit は Form、on_input は Input(Text/Password)／Textarea、on_check は Input(Checkbox) にだけ許可する。\n\n初版は差分描画・key による同一性を保証しない。木を前順に描画し、更新時は古い登録を解除して root の子を置換する。入力中の操作を成立させるため、フォーカスと選択範囲は root 内の一意な `id` と適合する入力型が新旧で一致するときだけ復元する。value／checked は model から毎回設定し、入力直後に model へ反映する。IME の composition 中は中間 input を配送せず、compositionend の確定値を一回配送し、同じ確定を続く input で二重配送しない。マウント外のフォーカスを変更しない。\n\n描画単位ごとの世代番号を登録に付ける。解除済み／古い世代のイベントは model に届かない。処理中に同じマウントへ再入しない。ホストが同期的に再入したら host failure とし、そのマウントを停止する。ブラウザの通常のイベントループは待機効果の隠蔽ではなく外部入口の呼び出しであり、Sigil の処理中に非同期処理を挟まない。\n\n新しい木を検査してから DOM を変更する。重複 id／不適合属性／上限超過は `InvalidTree`／`LimitExceeded`。初回は Result の Err、イベント中は停止として controller に記録する。update／view の契約違反・算術停止も停止とし、更新済み model をロールバックして継続しない。DOM 例外で一部反映した場合も、イベント登録をすべて解除して root を空にし、停止させる。最初の失敗を cleanup の二次失敗で上書きしない。\n\n`unmount` と controller の dispose は登録解除→DOM の切り離し→現行木と捕捉値・program・model の破棄→Root の権利解放を行う。通常のスコープ終了でも Mount の組み込み破棄で同じ cleanup を実行する。DOM を取り除く資源破棄には `dom` が必要で、暗黙 Drop の効果も呼び出し側に伝播させ完全一致検査する（メモリ解放自体に効果は加えない）。この契約を表すため必要な SIR の変更は実装前に別の契約変更 PR とする。\n\n所有値の Move／Drop／cleanup、部分移動・逆局所順・欄順・異常停止と `--trace-drops` は ADR 0039 決定 3 を維持する。GC／FinalizationRegistry に登録解除や捕捉値の破棄時点を委ねない。切り離された古いノードの通知・二重 dispose は無害で、破棄を二重実行しない。タブ強制終了／ホストのメモリ枯渇による cleanup 非保証は通常停止と区別する。\n\n## 決定 4: typed DOM・許可属性・URL・HTML/CSS\n\nテキストは `createTextNode`／`textContent`、入力値は DOM の value／checked プロパティで設定する。`<>&\"'` を含む値をマークアップやスクリプトとして解釈しない。HTML シリアライズ経路を将来設けるなら文脈に沿って escaping するが、初版に `innerHTML`・`outerHTML`・`insertAdjacentHTML`・`eval`・Function・任意 JS／HTML の文字列入口を設けない。\n\n通常属性は id・class・title、Label の for、フォーム部品の name・placeholder・required・disabled、Input の type／value／checked、Textarea の value、A の href だけ。属性は要素ごとの上記用途に限り、一つの要素で同じ属性・イベントを重複させない。Input は子なし、Textarea は value を使い子なし。Button は明示 Submit 型でない限り DOM type=button とする。form の action／method、任意の on*・style・src・srcdoc・data-*・aria-*・target・download は許可しない。属性名を文字列から作る API はない。\n\n`ui.url` は `/` で始まるオリジン内の絶対パス（`//` は不可）または `#` で始まるフラグメントだけを許す。空値、制御文字・空白・バックスラッシュ・不正な %HH、percent decode 後にこれらや先頭 `//` が現れる値、スキーム付き／相対 URL は拒否する。ページの base URL に依存させず、host が渡す document の origin を基準に解決し、同一 origin を再確認する。`javascript:`・`data:`・外部 origin を通さない。href はユーザーのリンク操作による移動だけを許し、自動 fetch・リソース読み込みを追加しない。HTTP の画面遷移と Sigil の `net.client` 操作を混同しない。\n\n配置・装飾は通常の HTML と CSS、許可された class/id で行う。所有者が用意する静的 HTML に root と CSS を置く。ui は CSS 文字列・外部 stylesheet URL を受け取らず、ホストがロードした CSS 自体は信頼境界外である。CSS まで安全に隔離したと主張しない。SSR・hydration・ルーティング・UI ライブラリ互換・自動資産配信は必須にしない。\n\n## 決定 5: 入力検証・数値・文字列 ABI\n\n入力は常に未検証 String／Bool として event に入れ、update で `String.parse_int` と `type Age = Int where self >= 0 && self <= 150;` の `Age.check` 等を Result として処理する。不正入力はモデルのエラー欄に保存して ui.text で表示し、有効な値への更新と分ける。requires／ensures と篩型の実行時検査を省かず、型検査成功を入力条件の証明と表示しない。UI の required/type 表示を検証の根拠にしない。将来サーバーへ送信する際も、受信サーバーで構造・篩型・件数・サイズ・認証を再検証する。UI 検証をサーバーの信頼境界に使わない。\n\nInt は P2-1 と同じ i64 BigInt、検査算術・添字変換を維持する。Float は binary64 Number、NaN／±inf／-0 を維持し、表示は標準 Displayable、入力との変換を DOM／JS の暗黙の Number 変換に任せない。文字列 ABI も ADR 0039 決定 2 を維持し、妥当な UTF-16（Unicode スカラー列）、正規化なし、len はスカラー数、UTF-8 は厳格変換とする。DOM からの孤立サロゲートは置換せず境界失敗にする。補助平面・結合文字・改行をそのまま表示する。\n\n初版のマウント上限は木の深さ 256、Node 数 10000、各要素の属性数 32、単一文字列の UTF-8 サイズ 1 MiB、木全体の文字列合計 8 MiB、URL 4096 UTF-8 bytes とする。検査・描画・破棄は深さに応じた明示スタックを使い、JS スタックへの依存を避ける。DOM の選択範囲の UTF-16 添字は JS ホスト内の不透明な値で、Sigil の String.len／Int と混同しない。入力取り込みも割り当て前にサイズを検査する。\n\n## 決定 6: ホスト呼び出し・CLI・診断\n\nP2-1 の通常の `invoke(args, host)` と値 ABI を維持し、Root／Mount／Node／関数値をその汎用境界で受け渡さない。UI 用だけ明示モードを追加する。\n\n```text\nsigil build --emit js --ui [--root <根>] [--trace-drops] <ファイル> <関数> -o <出力.mjs>\n```\n\n`--ui` の入口は `fn(ui.Root) -> Result<ui.Mount, ui.UiError>`、effects dom の一つに限定し、引数の型・戻り値・効果を生成前に検査する。生成した module は `mount(rootElement, host)` を export し、一つの空の Element を Root として入口へ渡す。生成モジュールは document を暗黙取得せず、rootElement の ownerDocument だけを使う。rootElement は既存のマウントと重複せず、接続済み・同じ realm・空であることを検査する。html／head／body を root にしない。root の範囲外を変更する権利は渡さない。\n\n戻りは同期 `{ok:true, controller}` または `{ok:false, failure}`。controller は JS 側の `dispose()` と `status()` のみを公開し、Sigil の値を外へ漏らさない。status は running／disposed／failed とコピーした failure 情報、dispose は冪等で同期の結果を返す。失敗の kind は P2-1 の entry／stop／host／internal を維持し、ui の Result の Err は kind=entry と UiError を含める。イベント中の失敗は status に保存し、新しいイベントを拒否する。イベントを任意 JS から注入する API は設けない。\n\nDOM ホスト呼び出しは生成した閉じたアダプター内の createElement・createTextNode・setAttribute（許可一覧のみ）・value/checked 設定・replaceChildren・add/removeEventListener・preventDefault・focus/selection の読み書きだけ。アダプターは DOM Event を保持しない。これらは `dom` の実装で、`foreign` や利用者の trust base を追加しない。ホストの任意コードを Sigil の検査で隔離したことにはしない。Node.js／npm／任意 FFI・動的 import・Wasm を要求しない。`host.log` と `host.traceDrop` は P2-1 と同じ必要時だけの同期能力で、Promise・例外・再入は境界失敗。DOM API からの例外も cleanup に接続する。\n\n通常の --emit js は P2-1 のまま、--emit native と既定 native、--root の解決も維持。native の --ui、未知・重複・矛盾したオプション、不正な入口は終了 1、成功は 0。`sigil run` をブラウザ起動コマンドに変えない。HTML/CSS 自動生成・配信 CLI は足さない。\n\n通常の型・所有権・効果検査と SIR 検証後、生成前に入口・callback・契約・cleanup の依存閉包を走査する。未対応の型／操作／効果は位置・依存元・出力先 js を含めて診断し、部分成果物を残さず既存出力も上書きしない。Shared／Cell・通信・待機、サーバー専用操作、未対応イベントと任意属性をダミー値や別出力への fallback で補わない。Node の意味検査は描画前、静的に不正と判定できるものは生成前に診断する。\n\n## 決定 7: P2-2 の完了の確かめ方\n\n| 確認 | 必要な正常例・負例と観測 |\n|---|---|\n| カウンター | 実ブラウザで複数回クリックし状態と表示が一致。update のオーバーフロー・view の契約違反で停止し、登録と所有値が解放される |\n| 入力検証 | 入力→event→parse_int／篩型→成功値またはエラー表示。空・負数・上限超過・巨大値、IME 確定、チェック状態、focus／選択を確認。submit で暗黙通信しない |\n| 一覧更新 | model の List を追加・削除・並べ替えて再描画。古い要素のイベントが届かず、新しい要素だけが反応。key 最適化なしでも正しい表示になる |\n| 効果検査 | 純粋 view/update/input へ dom／state／console／net.client／task.suspend を入れた負例、dom を欠く描画・イベント入口・暗黙破棄を拒否。余分な宣言も完全一致で拒否 |\n| 安全な表示 | `<script>`・`<img onerror=...>`・引用符・&・補助平面・結合文字が文字として表示され、要素やコードにならない。禁止属性・URL・孤立サロゲート・上限超過は DOM 更新前に拒否 |\n| 寿命とホスト | 初期化失敗、連続再描画、unmount、二重 dispose、古いイベント、ホスト例外／再入、言語停止時に登録解除と破棄を一回実施。root 外に変更なし |\n| 核の一致 | P2-1 の数値・文字列・契約・所有権の比較試験を維持。純粋 update/view の結果（木を標準の型付き構造へ正規化）と破棄列を SIR 側の期待値・実行結果と比較。DOM 統合は実ブラウザで別に検証 |\n\nChromium／Firefox／WebKit で最低対応版を記録し、同じ三例を Node.js・Wasm なしで操作できることを要求する。UI 操作と拒否例は自動ブラウザ試験、標準操作の全許可一覧は正常例と必要な失敗例で確かめる。表示画面だけや Node.js での成功だけを完了としない。P2-4 で流用できるホスト境界と例を残すが、Wasm の実装を先取りしない。\n\n## 影響\n\n標準 ui の型と効果付き資源破棄、JS の対応集合・閉じた DOM アダプター・ブラウザ適合試験が必要になる。新しい信頼基盤や依存の包括承認ではない。必要な AST／SIR 契約変更は別 PR とし、契約変更→ADR→実装→サンプル・テストの順を守る。本 Issue は ADR だけの PR 作成で完了し、この検証表の実装は含めない。P2-3 以降や既存 ADR の改訂、P1 の公開前課題 #547〜#552 は本 PR で変更・再起票しない。\n\n",
  "questions": {
    "Model": {
      "type": "choice",
      "criteria": {
        "pure_owned": "草稿の所有model、純粋view/update/入力変換、dom付き同期描画入口を採用。保存callback上限none、共有状態・通信・待機は規則維持し初版出力では拒否",
        "effectful": "共有状態・通信・非同期callbackも初版UIに含める",
        "mutable_dom": "利用者が直接DOMを変更するモデルを初版にする"
      }
    },
    "Safety": {
      "type": "choice",
      "criteria": {
        "typed": "草稿の閉じたTag/Attr、テキストDOM、限定URL、明示入力検証/サーバー再検証/上限、HTML/CSSの責務分離を採用",
        "raw": "任意属性とHTML/JS文字列も許可",
        "defer": "URL/属性/入力境界は実装者に一任"
      }
    },
    "Lifetime": {
      "type": "choice",
      "criteria": {
        "owned": "草稿のRoot/Mount所有、世代で古いイベント拒否、登録解除と明示cleanup、dom付き資源破棄契約と別契約PR、再入停止を採用",
        "gc": "GC任せで登録と所有値を回収",
        "retain": "古いcallbackを残し任意非同期再入を許す"
      }
    },
    "Completion": {
      "type": "choice",
      "criteria": {
        "enumerated": "草稿の構文/型/操作の閉じた一覧、P2-1値ABI維持、閉じたホスト、明示--uiと三ブラウザ三例/負例を採用",
        "samples": "サンプルが動けばAPIとABI詳細は未確定のまま完了",
        "expand": "P2-3通信やSSRをこのADR/実装で同時に含める"
      }
    }
  }
}
```

### 初回の応答

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "Model": {
      "type": "choice",
      "choice": "pure_owned",
      "confidence": 1.0,
      "probabilities": {
        "pure_owned": 1.0,
        "effectful": 0.0,
        "mutable_dom": 0.0
      }
    },
    "Safety": {
      "type": "choice",
      "choice": "typed",
      "confidence": 1.0,
      "probabilities": {
        "typed": 1.0,
        "raw": 0.0,
        "defer": 0.0
      }
    },
    "Lifetime": {
      "type": "choice",
      "choice": "owned",
      "confidence": 1.0,
      "probabilities": {
        "owned": 1.0,
        "gc": 0.0,
        "retain": 0.0
      }
    },
    "Completion": {
      "type": "choice",
      "choice": "enumerated",
      "confidence": 1.0,
      "probabilities": {
        "enumerated": 1.0,
        "samples": 0.0,
        "expand": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 9084,
    "output_tokens": 153
  }
}
```

### 最終案の入力

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間は仕様だけをレビューする（ADR0001）。所有者はIssue557でP2-2 UIの個別ADR0040と索引だけを指示し、実装・spec・既存ADR・バックログの変更を禁止。2026-11-16指示により正式判定はtypesafe-aiのみ。ADR0038の純粋view、model/event/update、dom等の実効果、typed DOMとtext escaping、既存state/net.client/task.suspend規則、入力のサーバー再検証、HTML/CSSを守る。ADR0039の核、BigInt i64、binary64、Unicode/UTF8、所有値Drop/cleanupとホスト失敗、明示CLIを維持。新JSXや効果カテゴリを追加せず、既存関数型フィールド規則でnone上限のcallbackを保存。以下草稿の判断がこの理念と指示に沿って明示的で検証可能かを評価する。確率は証明でなく設計判断の補助。\n\n# 0040: P2-2 UI の記述モデル\n\n- **状態**: 提案（typesafe-ai の正式判定を末尾に記録。別系統のレビュー前）\n- **日付**: 2026-11-17（JST）\n- **対象**: [Issue #557](https://github.com/goodnasubi/sigil/issues/557)、レーン E\n\n## 背景\n\n[ADR 0038](0038-p2-scope.md) の二番目の段 P2-2 を、[ADR 0039](0039-p2-1-javascript-output.md) の JS 出力の上に具体化する。AI が実装し、人間が仕様だけをレビューする原則（[ADR 0001](0001-vision-and-scope.md)）に従い、表示・状態遷移・作用の境界を仕様で読めるようにする。\n\n起草の基準は `main` の `3748e33`。0040 は main のファイル・索引とオープン PR で未使用を確認した。本 PR は本 ADR の新規作成と索引追加だけである。以下は将来の API・CLI の決定であり、現行の処理系や `sigil spec` の対応機能を意味しない。\n\n## 選択肢と決定\n\n| 判断 | 採用する案 | 他案を採用しない理由 |\n|---|---|---|\n| 表示 | 純粋な `view` が型付きの木を返し、明示的な `model`・`event`・`update` で更新 | DOM の直接変更を利用者の通常 API にすると状態と表示が乖離する。JSX・React 互換は新しい構文と互換性の責務が増える |\n| イベント | 所有されたマウントの中で同期的に一件ずつ更新。保存するコールバックは純粋 | 任意の非同期クロージャや隠れた共有状態は寿命と効果を追えなくする |\n| 安全性 | typed DOM、許可一覧、文字列のテキスト表示、入力の明示検証 | HTML／JS 文字列注入と任意属性は、表示値を実行権限に変えてしまう |\n| 実装範囲 | P2-1 の核に下記 UI API と `dom` を加える | 通信・共有状態・待機を同時実装すると P2-3 と並行処理の責務を先取りする |\n\n## 決定 1: 構文・型・標準操作の対応集合\n\n新しい JSX 構文・効果カテゴリは加えない。`targets browser`、既存の関数の仕様四節・構造体・列挙型・総称型・篩型、リテラル・一覧・`match`・借用・局所代入・関数値・`move` クロージャを用いる。構文・型・標準操作の核は ADR 0039 決定 1 の許可一覧をそのまま含める（数値／Bool／String／Bytes／List／Option／Result／組／利用者型、契約、標準の文字列・一覧操作、console）。以下だけを追加する。\n\n`std.ui` は browser 専用の標準モジュールとし、公開名は `ui`。`Root` と `Mount` は複製・比較・共有・送信できない所有資源、`Node<E>` と `Attr<E>` は所有された宣言値とする。`Tag`・`InputKind`・`UiError` は閉じた列挙型、`Url` は検査済みの不透明な所有文字列。`E` は借用・関数・ホスト資源を含まないイベントの型とする。`M` も借用・ホスト資源を含まない所有型とし、コールバックをモデルに格納しない。関数値を型引数に直接入れない既存規則を維持する。\n\n保存する関数は、既存の関数型をフィールドに置く規則（[ADR 0022](0022-type-identity-traits-and-function-types.md) 決定 3）で次の入れ物に収める。\n\n```sigil\nstruct Program<M, E> {\n  view: fn(&M) -> Node<E> effects none,\n  update: fn(&mut M, E) -> Unit effects none,\n}\nstruct InputMap<E> {\n  event: fn(String) -> E effects none,\n}\nstruct CheckMap<E> {\n  event: fn(Bool) -> E effects none,\n}\n```\n\nこれらは `std.ui` 内の宣言の形であり、利用者は `ui.Program` 等で参照する。`Node`／`Attr` の内部表現は公開しない。借用の捕捉は禁止、保存する `move` クロージャの捕捉値は既存どおり Sendable・Shareable で不変、関数値自体は複製しない。API の対応一覧は次で閉じる。表の型引数・借用は通常の Sigil のもの、`none` は効果なしを表す。\n\n| 操作 | シグネチャ | 効果・条件 |\n|---|---|---|\n| テキスト | `ui.text<E>(value: String) -> Node<E>` | none。HTML として解釈しない |\n| 表示への変換 | `ui.display<T: Displayable>(value: &T) -> String` | none。console と同じ標準表示 |\n| 要素 | `ui.element<E>(tag: Tag, attrs: List<Attr<E>>, children: List<Node<E>>) -> Node<E>` | none。属性と子の適合は描画前に検査 |\n| 一覧 | `ui.fragment<E>(children: List<Node<E>>) -> Node<E>` | none。List の `map` やループで作る。key／差分 API は加えない |\n| 通常属性 | `ui.id<E>(String)`、`ui.class_name<E>(String)`、`ui.title<E>(String)`、`ui.label_for<E>(String)`、`ui.name<E>(String)`、`ui.placeholder<E>(String)` | すべて `-> Attr<E>`、none |\n| 型付き属性 | `ui.disabled<E>(Bool)`、`ui.checked<E>(Bool)`、`ui.required<E>(Bool)`、`ui.value<E>(String)`、`ui.input_kind<E>(InputKind)`、`ui.href<E>(Url)` | すべて `-> Attr<E>`、none |\n| クリック／送信 | `ui.on_click<E>(event: E)`、`ui.on_submit<E>(event: E)` | `-> Attr<E>`、none。`E: Copyable`、発火ごとにイベントを複製 |\n| 入力／チェック | `ui.on_input<E>(map: InputMap<E>)`、`ui.on_check<E>(map: CheckMap<E>)` | `-> Attr<E>`、none。保存した変換関数の上限は none |\n| URL の検査 | `ui.url(value: String) -> Result<Url, UiError>` | none。許可規則は決定 4 |\n| 描画開始 | `ui.mount<M,E>(root: Root, model: M, program: Program<M,E>) -> Result<Mount, UiError>` | dom。成功なら root・model・program を所有、失敗なら消費した値を破棄 |\n| 明示終了 | `ui.unmount(mount: Mount) -> Unit` | dom。登録解除して所有値を破棄 |\n\n引数名を省略した表の単一型引数は `value` を引数名とする。`ui` の全操作は通常どおり intent・requires・ensures・effects を持つ標準操作として登録し、上記は簡略シグネチャである。新しい構文や標準 API は本 PR には実装しない。\n\n`Tag` の腕は `Div, Span, P, H1, H2, Ul, Ol, Li, Button, Label, Input, Textarea, Form, A`、`InputKind` は `Text, Password, Checkbox, Submit, Button`。`UiError` は `InvalidTree, InvalidUrl, LimitExceeded, HostFailure`。任意タグ・任意属性・SVG・custom elements・生の DOM ノード・ファイル入力・contenteditable は未対応。セレクターで任意 DOM を取得する API も設けない。\n\n## 決定 2: 純粋な view と明示的な更新\n\n例えばカウンターは次の形で記述する（新設する ui API を用いた将来の構文例）。\n\n```sigil\nmodule counter\n  targets browser\n  trust none\n\nstruct Model { count: Int }\nenum Event { Increment }\n\nfn view(model: &Model) -> ui.Node<Event>\n  intent \"現在値と加算ボタンを表示する\"\n  requires none\n  ensures none\n  effects none\n{\n  return ui.element(ui.Tag.Div, [], [\n    ui.text(ui.display(&model.count)),\n    ui.element(ui.Tag.Button, [ui.on_click(Event.Increment)], [ui.text(\"増やす\")]),\n  ]);\n}\n\nfn update(model: &mut Model, event: Event) -> Unit\n  intent \"イベントに応じてカウンターを増やす\"\n  requires none\n  ensures none\n  effects none\n{\n  match event {\n    Event.Increment => { model.count = model.count + 1; },\n  }\n}\n\nfn main(root: ui.Root) -> Result<ui.Mount, ui.UiError>\n  intent \"所有された描画領域にカウンターを開始する\"\n  requires none\n  ensures none\n  effects dom\n{\n  return ui.mount(root, Model { count: 0 }, ui.Program { view: view, update: update });\n}\n```\n\n初期化は `view(&model)`→木の検査→描画。イベント入口は DOM の最小ペイロードをコピーして `E` を作り、`update(&mut model, event)`→`view(&model)`→木の検査→再描画を一件ずつ同期実行する。`&mut M` の変更は既存規則どおり効果ではない。描画器・登録解除・DOM 読み出しを行うイベント入口には `dom` を宣言し、標準操作／生成する入口の効果情報にも残す。純粋な view が DOM の読み出しや I/O を行うことはない。\n\nコールバックの効果上限は `none`。フィールド型への代入で上限を検査し、呼び出し側はそのフィールド型の効果を含めて完全一致検査する。`dom`・`console` を起こす view、`state` を起こす update、効果を持つ入力変換は負例として拒否する。`List.map` 等を経由しても同じである。`console` は通常の入口外の関数では P2-1 同様に利用できるが、この保存コールバックの上限を広げない。\n\n`Shared`／`Cell` を使う書き換えには `state`、通信には `net.client`、応答待ちには `task.suspend`、タスクには既存 `task.*` を要求する規則を維持する。それらを宣言 UI の裏に隠さない。P2-2 の JS 対応集合は `none`・`console`・`dom` に限り、共有状態・通信・待機・タスクの実行 API は生成前に未対応として拒否する。単独所有 model の局所更新で三つの完了例は実現できる。通信を持つ更新や非同期コールバックは P2-3 等の別 ADR で対応集合を広げる。\n\n## 決定 3: イベントの寿命・再描画・破棄\n\n`Mount` が model・program・現行の木・イベント変換関数と捕捉値・DOM 登録を一意に所有する。DOM Event、target、JS の関数、借用は Sigil に渡さない。input の現在値を妥当性検査した String、checkbox を Bool にコピーする。クリックと submit は保存した E の複製を渡す。submit は常に `preventDefault` し、ブラウザの暗黙のフォーム送信を起こさない。on_click は Button、on_submit は Form、on_input は Input(Text/Password)／Textarea、on_check は Input(Checkbox) にだけ許可する。\n\n初版は差分描画・key による同一性を保証しない。木を前順に描画し、更新時は古い登録を解除して root の子を置換する。入力中の操作を成立させるため、フォーカスと選択範囲は root 内の一意な `id` と適合する入力型が新旧で一致するときだけ復元する。value／checked は model から毎回設定し、入力直後に model へ反映する。IME の composition 中は中間 input を配送せず、compositionend の確定値を一回配送し、同じ確定を続く input で二重配送しない。マウント外のフォーカスを変更しない。\n\n描画単位ごとの世代番号を登録に付ける。解除済み／古い世代のイベントは model に届かない。処理中に同じマウントへ再入しない。ホストが同期的に再入したら host failure とし、そのマウントを停止する。ブラウザの通常のイベントループは待機効果の隠蔽ではなく外部入口の呼び出しであり、Sigil の処理中に非同期処理を挟まない。\n\n新しい木を検査してから DOM を変更する。重複 id／不適合属性／上限超過は `InvalidTree`／`LimitExceeded`。初回は Result の Err、イベント中は停止として controller に記録する。update／view の契約違反・算術停止も停止とし、更新済み model をロールバックして継続しない。DOM 例外で一部反映した場合も、イベント登録をすべて解除して root を空にし、停止させる。最初の失敗を cleanup の二次失敗で上書きしない。\n\n`unmount` と controller の dispose は登録解除→DOM の切り離し→現行木と捕捉値・program・model の破棄→Root の権利解放を行う。通常のスコープ終了でも Mount の組み込み破棄で同じ cleanup を実行する。DOM を取り除く資源破棄には `dom` が必要で、暗黙 Drop の効果も呼び出し側に伝播させ完全一致検査する（メモリ解放自体に効果は加えない）。この契約を表すため必要な SIR の変更は実装前に別の契約変更 PR とする。\n\n所有値の Move／Drop／cleanup、部分移動・逆局所順・欄順・異常停止と `--trace-drops` は ADR 0039 決定 3 を維持する。GC／FinalizationRegistry に登録解除や捕捉値の破棄時点を委ねない。切り離された古いノードの通知・二重 dispose は無害で、破棄を二重実行しない。タブ強制終了／ホストのメモリ枯渇による cleanup 非保証は通常停止と区別する。\n\n## 決定 4: typed DOM・許可属性・URL・HTML/CSS\n\nテキストは `createTextNode`／`textContent`、入力値は DOM の value／checked プロパティで設定する。`<>&\"'` を含む値をマークアップやスクリプトとして解釈しない。HTML シリアライズ経路を将来設けるなら文脈に沿って escaping するが、初版に `innerHTML`・`outerHTML`・`insertAdjacentHTML`・`eval`・Function・任意 JS／HTML の文字列入口を設けない。\n\n通常属性は id・class・title、Label の for、フォーム部品の name・placeholder・required・disabled、Input の type／value／checked、Textarea の value、A の href だけ。属性は要素ごとの上記用途に限り、一つの要素で同じ属性・イベントを重複させない。Input は子なし、Textarea は value を使い子なし。Button は常に DOM type=button とし、送信ボタンは Input(Submit) で表す。form の action／method、任意の on*・style・src・srcdoc・data-*・aria-*・target・download は許可しない。属性名を文字列から作る API はない。\n\n`ui.url` は `/` で始まるオリジン内の絶対パス（`//` は不可）または `#` で始まるフラグメントだけを許す。空値、制御文字・空白・バックスラッシュ・不正な %HH、percent decode 後にこれらや先頭 `//` が現れる値、スキーム付き／相対 URL は拒否する。ページの base URL に依存させず、host が渡す document の origin を基準に解決し、同一 origin を再確認する。`javascript:`・`data:`・外部 origin を通さない。href はユーザーのリンク操作による移動だけを許し、自動 fetch・リソース読み込みを追加しない。HTTP の画面遷移と Sigil の `net.client` 操作を混同しない。\n\n配置・装飾は通常の HTML と CSS、許可された class/id で行う。所有者が用意する静的 HTML に root と CSS を置く。ui は CSS 文字列・外部 stylesheet URL を受け取らず、ホストがロードした CSS 自体は信頼境界外である。CSS まで安全に隔離したと主張しない。SSR・hydration・ルーティング・UI ライブラリ互換・自動資産配信は必須にしない。\n\n## 決定 5: 入力検証・数値・文字列 ABI\n\n入力は常に未検証 String／Bool として event に入れ、update で `String.parse_int` と `type Age = Int where self >= 0 && self <= 150;` の `Age.check` 等を Result として処理する。不正入力はモデルのエラー欄に保存して ui.text で表示し、有効な値への更新と分ける。requires／ensures と篩型の実行時検査を省かず、型検査成功を入力条件の証明と表示しない。UI の required/type 表示を検証の根拠にしない。将来サーバーへ送信する際も、受信サーバーで構造・篩型・件数・サイズ・認証を再検証する。UI 検証をサーバーの信頼境界に使わない。\n\nInt は P2-1 と同じ i64 BigInt、検査算術・添字変換を維持する。Float は binary64 Number、NaN／±inf／-0 を維持し、表示は標準 Displayable、入力との変換を DOM／JS の暗黙の Number 変換に任せない。文字列 ABI も ADR 0039 決定 2 を維持し、妥当な UTF-16（Unicode スカラー列）、正規化なし、len はスカラー数、UTF-8 は厳格変換とする。DOM からの孤立サロゲートは置換せず境界失敗にする。補助平面・結合文字・改行をそのまま表示する。\n\n初版のマウント上限は木の深さ 256、Node 数 10000、各要素の属性数 32、単一文字列の UTF-8 サイズ 1 MiB、木全体の文字列合計 8 MiB、URL 4096 UTF-8 bytes とする。検査・描画・破棄は深さに応じた明示スタックを使い、JS スタックへの依存を避ける。DOM の選択範囲の UTF-16 添字は JS ホスト内の不透明な値で、Sigil の String.len／Int と混同しない。入力取り込みも割り当て前にサイズを検査する。\n\n## 決定 6: ホスト呼び出し・CLI・診断\n\nP2-1 の通常の `invoke(args, host)` と値 ABI を維持し、Root／Mount／Node／関数値をその汎用境界で受け渡さない。UI 用だけ明示モードを追加する。\n\n```text\nsigil build --emit js --ui [--root <根>] [--trace-drops] <ファイル> <関数> -o <出力.mjs>\n```\n\n`--ui` の入口は `fn(ui.Root) -> Result<ui.Mount, ui.UiError>`、effects dom の一つに限定し、引数の型・戻り値・効果を生成前に検査する。生成した module は `mount(rootElement, host)` を export し、一つの空の Element を Root として入口へ渡す。生成モジュールは document を暗黙取得せず、rootElement の ownerDocument だけを使う。rootElement は既存のマウントと重複せず、接続済み・同じ realm・空であることを検査する。html／head／body を root にしない。root の範囲外を変更する権利は渡さない。\n\n戻りは同期 `{ok:true, controller}` または `{ok:false, failure}`。controller は JS 側の `dispose()` と `status()` のみを公開し、Sigil の値を外へ漏らさない。status は running／disposed／failed とコピーした failure 情報、dispose は冪等で同期の結果を返す。失敗の kind は P2-1 の entry／stop／host／internal を維持し、ui の Result の Err は kind=entry と UiError を含める。イベント中の失敗は status に保存し、新しいイベントを拒否する。イベントを任意 JS から注入する API は設けない。\n\nDOM ホスト呼び出しは生成した閉じたアダプター内の createElement・createTextNode・setAttribute（許可一覧のみ）・value/checked 設定・replaceChildren・add/removeEventListener・preventDefault・focus/selection の読み書きだけ。アダプターは DOM Event を保持しない。これらは `dom` の実装で、`foreign` や利用者の trust base を追加しない。ホストの任意コードを Sigil の検査で隔離したことにはしない。Node.js／npm／任意 FFI・動的 import・Wasm を要求しない。`host.log` と `host.traceDrop` は P2-1 と同じ必要時だけの同期能力で、Promise・例外・再入は境界失敗。DOM API からの例外も cleanup に接続する。\n\n通常の --emit js は P2-1 のまま、--emit native と既定 native、--root の解決も維持。native の --ui、未知・重複・矛盾したオプション、不正な入口は終了 1、成功は 0。`sigil run` をブラウザ起動コマンドに変えない。HTML/CSS 自動生成・配信 CLI は足さない。\n\n通常の型・所有権・効果検査と SIR 検証後、生成前に入口・callback・契約・cleanup の依存閉包を走査する。未対応の型／操作／効果は位置・依存元・出力先 js を含めて診断し、部分成果物を残さず既存出力も上書きしない。Shared／Cell・通信・待機、サーバー専用操作、未対応イベントと任意属性をダミー値や別出力への fallback で補わない。Node の意味検査は描画前、静的に不正と判定できるものは生成前に診断する。\n\n## 決定 7: P2-2 の完了の確かめ方\n\n| 確認 | 必要な正常例・負例と観測 |\n|---|---|\n| カウンター | 実ブラウザで複数回クリックし状態と表示が一致。update のオーバーフロー・view の契約違反で停止し、登録と所有値が解放される |\n| 入力検証 | 入力→event→parse_int／篩型→成功値またはエラー表示。空・負数・上限超過・巨大値、IME 確定、チェック状態、focus／選択を確認。submit で暗黙通信しない |\n| 一覧更新 | model の List を追加・削除・並べ替えて再描画。古い要素のイベントが届かず、新しい要素だけが反応。key 最適化なしでも正しい表示になる |\n| 効果検査 | 純粋 view/update/input へ dom／state／console／net.client／task.suspend を入れた負例、dom を欠く描画・イベント入口・暗黙破棄を拒否。余分な宣言も完全一致で拒否 |\n| 安全な表示 | `<script>`・`<img onerror=...>`・引用符・&・補助平面・結合文字が文字として表示され、要素やコードにならない。禁止属性・URL・孤立サロゲート・上限超過は DOM 更新前に拒否 |\n| 寿命とホスト | 初期化失敗、連続再描画、unmount、二重 dispose、古いイベント、ホスト例外／再入、言語停止時に登録解除と破棄を一回実施。root 外に変更なし |\n| 核の一致 | P2-1 の数値・文字列・契約・所有権の比較試験を維持。純粋 update/view の結果（木を標準の型付き構造へ正規化）と破棄列を SIR 側の期待値・実行結果と比較。DOM 統合は実ブラウザで別に検証 |\n\nChromium／Firefox／WebKit で最低対応版を記録し、同じ三例を Node.js・Wasm なしで操作できることを要求する。UI 操作と拒否例は自動ブラウザ試験、標準操作の全許可一覧は正常例と必要な失敗例で確かめる。表示画面だけや Node.js での成功だけを完了としない。P2-4 で流用できるホスト境界と例を残すが、Wasm の実装を先取りしない。\n\n## 影響\n\n標準 ui の型と効果付き資源破棄、JS の対応集合・閉じた DOM アダプター・ブラウザ適合試験が必要になる。新しい信頼基盤や依存の包括承認ではない。必要な AST／SIR 契約変更は別 PR とし、契約変更→ADR→実装→サンプル・テストの順を守る。本 Issue は ADR だけの PR 作成で完了し、この検証表の実装は含めない。P2-3 以降や既存 ADR の改訂、P1 の公開前課題 #547〜#552 は本 PR で変更・再起票しない。\n\n",
  "questions": {
    "Model": {
      "type": "choice",
      "criteria": {
        "pure_owned": "草稿の所有model、純粋view/update/入力変換、dom付き同期描画入口を採用。保存callback上限none、共有状態・通信・待機は規則維持し初版出力では拒否",
        "effectful": "共有状態・通信・非同期callbackも初版UIに含める",
        "mutable_dom": "利用者が直接DOMを変更するモデルを初版にする"
      }
    },
    "Safety": {
      "type": "choice",
      "criteria": {
        "typed": "草稿の閉じたTag/Attr、テキストDOM、限定URL、明示入力検証/サーバー再検証/上限、HTML/CSSの責務分離を採用",
        "raw": "任意属性とHTML/JS文字列も許可",
        "defer": "URL/属性/入力境界は実装者に一任"
      }
    },
    "Lifetime": {
      "type": "choice",
      "criteria": {
        "owned": "草稿のRoot/Mount所有、世代で古いイベント拒否、登録解除と明示cleanup、dom付き資源破棄契約と別契約PR、再入停止を採用",
        "gc": "GC任せで登録と所有値を回収",
        "retain": "古いcallbackを残し任意非同期再入を許す"
      }
    },
    "Completion": {
      "type": "choice",
      "criteria": {
        "enumerated": "草稿の構文/型/操作の閉じた一覧、P2-1値ABI維持、閉じたホスト、明示--uiと三ブラウザ三例/負例を採用",
        "samples": "サンプルが動けばAPIとABI詳細は未確定のまま完了",
        "expand": "P2-3通信やSSRをこのADR/実装で同時に含める"
      }
    }
  }
}
```

### 最終案の応答

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "Model": {
      "type": "choice",
      "choice": "pure_owned",
      "confidence": 1.0,
      "probabilities": {
        "pure_owned": 1.0,
        "effectful": 0.0,
        "mutable_dom": 0.0
      }
    },
    "Safety": {
      "type": "choice",
      "choice": "typed",
      "confidence": 1.0,
      "probabilities": {
        "typed": 1.0,
        "raw": 0.0,
        "defer": 0.0
      }
    },
    "Lifetime": {
      "type": "choice",
      "choice": "owned",
      "confidence": 1.0,
      "probabilities": {
        "owned": 1.0,
        "gc": 0.0,
        "retain": 0.0
      }
    },
    "Completion": {
      "type": "choice",
      "choice": "enumerated",
      "confidence": 1.0,
      "probabilities": {
        "enumerated": 1.0,
        "samples": 0.0,
        "expand": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 9091,
    "output_tokens": 153
  }
}
```

# 0037: HMAC ベアラートークンによる認証（段階 6-6）

- **状態**: 提案（Issue #501）。細目は typesafe-ai 正式判定で決定済み。実装完了・マージ・外部公開の承認を意味しない。
- **日付**: 2026-11-15（JST）
- **起草・判定実行**: Codex Cloud（OpenAI 系統）
- **対象**: `std.web` の認証差し込み口に接続する具体的な検証方式 1 つ。後続の実装は 6-6a〜6-6d。

## 背景と範囲

[ADR 0018](0018-p1-scope.md) の段階表の 6-6 は 6-4・6-1 に依存する。両段は完了済みで、[ADR 0024](0024-crypto-primitives.md) の `Secret`・`verify_hmac_sha256`・`ct_eq` と、[ADR 0033](0033-p1-framework.md) の既定拒否ルーターを再利用する。

所有者が決めた判断 6c=A を継承する。HMAC 署名のベアラートークンを検証し、鍵は環境変数から受け取る。トークン発行、ログイン、パスワード照合、cookie、セッション、失効 DB、OAuth/OIDC、JWT 汎用実装、TLS は範囲外。独自形式を JWT 互換とは称さない。暗号方式を利用者の入力で選ばない。

## 判断点と正式判定

理念は、AI が実装し人間が仕様をレビューできること（ADR 0001）、既存言語のライブラリを持ち込まないこと（ADR 0005）、明示能力・最小信頼基盤・曖昧さ削減（ADR 0027）。これらと所有者の委任・既決事項・ADR 0020 の検証要件を固定して判定した。

| 判断点 | 採用案 | 比較した案 | 採用確率 / confidence |
|---|---|---|---|
| Profile | 期限と対象サービスを必須にした、上限付き固定形式 | 汎用 JWT／無期限トークン | 1.00 / 1.00 |
| Integration | 最小の壁時計入口と重複検査、起動時設定、純粋検証核、4 実装段 | 要求ごとの環境読込と大域鍵／汎用日時・発行・失効への拡大 | 1.00 / 1.00 |

判定は `jev-1.13.0`、2026-11-15（JST）。次点はいずれも 0.00 で近接なし。入力は [501-typesafe-input.json](records/501-typesafe-input.json)、応答は [501-typesafe-output.json](records/501-typesafe-output.json)。所有者は後から確認し覆せる。

## 決定 1: トークンの形式

ASCII の `sigil1.<payload>.<tag>` に固定する。区切りは `.` 2 個だけ。方式名・`alg`・鍵 ID・任意の拡張欄は持たない。未知の版は拒否する。トークン全体は 1024 バイト以下。

- `payload` と `tag` は RFC 4648 の base64url（`A-Z a-z 0-9 - _`）、パディング `=` なし。空欄、標準 base64 の `+ /`、空白、非 ASCII、不可能な長さ、未使用下位ビットが非ゼロの表現を拒否する。復号して再符号化した文字列と一致する正規形だけを受け付ける。
- 署名対象は **受信した ASCII の `sigil1.<payload>` 全体**。区切りを含み、末尾の `.<tag>` は含まない。復号・再構成した payload を署名対象にしない。
- 署名は HMAC-SHA256 の全 32 バイト。切り詰め禁止。`tag` は 43 文字、復号後ちょうど 32 バイト。
- これは署名だけで、payload を暗号化しない。個人情報・権限一覧・秘密を入れない。

復号した payload は次の順のバイナリ列。整数は符号なし大端表現。再帰・JSON・可変長整数を使わない。

| 順 | フィールド | 幅・制約 |
|---|---|---|
| 1 | `iat` | 8 バイト。発行時刻、Unix 秒（UTC 起点、うるう秒を数えない） |
| 2 | `exp` | 8 バイト。有効期限、同じ時間軸 |
| 3 | `aud_len`、`aud` | 2 バイトの長さと、そのバイト数の対象サービス識別子。1〜128 バイト |
| 4 | `sub_len`、`sub` | 2 バイトの長さと、そのバイト数の主体識別子。1〜256 バイト |

`aud`・`sub` の文字は ASCII の英数字および `._:-` のみ。大文字小文字を区別し、正規化しない。余剰バイトは禁止。payload は最大 404 バイトで、正規の最大トークンは 590 バイトになる。1024 バイトは意味解釈前の入口上限であり、404 バイト超の復号 payload も拒否する。

`iat`・`exp` はそれぞれ `0..=253402300799`（9999 年末）の範囲に限る。符号付き `Int` に組み立てる前に桁あふれを検査する。必須条件は `iat <= now && now < exp` と `0 < exp - iat && exp - iat <= 3600`。減算は範囲と大小を検査してから行う。時計誤差の猶予は 0 秒で、`now == exp` は失敗。`aud` は検証器の設定値と完全一致させ、成功時は `sub` だけを `AuthIdentity` にする。署名が正しくても他サービス向けは拒否する。

有効期間中の持参者による再使用は防がない。鍵と audience はサービス・環境ごとに分離し、期限は最長 1 時間。鍵が漏れた場合は鍵交換と再起動で旧トークンをすべて失効させる。無停止ローテーションと個別失効は後続 ADR の対象。

## 決定 2: 鍵と起動時の設定

- `SIGIL_AUTH_HMAC_KEY_HEX` は **小文字 hex 64 桁（復号後ちょうど 32 バイト）**。未設定・空・非正規形・長さ違いを起動時の `Err` とし、待ち受けを始めない。ADR 0020 の「32 バイト未満を拒否」より狭い固定長プロファイルである。既定鍵、起動引数による鍵、平文のパスワードからの代用は禁止。
- 鍵は運用側が暗号乱数から作る。長さだけでは十分なエントロピーを証明できない。リポジトリ・CI・サンプルには実鍵を置かない。試験の鍵は公開の使い捨て値で、製品の既定値にしない。
- `SIGIL_AUTH_AUDIENCE` も必須で、決定 1 の `aud` と同じ制約。鍵・audience は `env.get`（`effects env`）で起動時に 1 回読む。読み込み関数は `Result<BearerConfig, ConfigError>` 相当、エラーは変数名と固定の分類のみ。エラーに設定値を保持・表示しない。
- 復号鍵は直ちに `Secret` に入れ、設定と検証器に表示 API を付けない。環境変数の `String`、復号作業用の通常 `Bytes` のコピーは最短寿命にし、明示的な上書きが必要な一時領域は後続実装でゼロ化する。`Secret` の破棄だけでコピーや OS 環境まで消えるとは主張しない。OS の環境、ダンプ、プロセスへのアクセス制御は配備側の責務。
- 読み込み後は immutable な設定を `move` クロージャが所有し、再読込や大域の可変鍵を作らない。`Secret` の共有・破棄を含む `Sendable` / `Shareable` と型・効果検査を統合段で確認する。

## 決定 3: 検証手順と失敗

`std.web` 内に `trust none` の具体的検証器を置く。純粋な検証核は検証済み設定・入力バイト列・`now` を明示的に受け取り、`effects none`、不正入力は `Err`。悪い入力を `requires` 違反や異常停止にしない。

1. 認証必須経路では `Authorization` の **全出現数** を調べる。0 件も 2 件以上も失敗（同値の重複も拒否）。大文字小文字を無視する名前の比較は HTTP の正規化規則に合わせる。HTTP parser を通らない直接 dispatch でも確かめる。
2. HTTP が値の外側の OWS を除いた後、ASCII 大小不問の `Bearer`、SP 1 個、トークンのみを受け付ける。タブ区切り、追加 SP、カンマ結合、後続の空白や別の値は失敗。query、cookie、`X-Forwarded-*` から補わない。
3. 長さ・区切り・版・base64url 正規形・タグ長を検査する。payload の復号はサイズと正規形の確認だけで、フィールドはまだ読まない。
4. 検証済み鍵で受信署名対象を `verify_hmac_sha256` に渡す。署名が合うまでは `iat`・`exp`・`aud`・`sub` を解釈しない。不一致なら同じ認証失敗を返す。
5. 署名が合った場合だけ payload を決定 1 の順に解釈し、時刻・audience を検査する。成功後にのみ `AuthIdentity.new(sub)` を返す。無署名クレームをハンドラ・フックへ渡さない。

認証失敗（欠落、重複、形式不正、不明版、署名不一致、期限切れ、未来発行、対象違い、時計取得不能）は、すべて **401、本文 `unauthorized`、`Content-Type: text/plain; charset=utf-8`、`WWW-Authenticate: Bearer`、`Cache-Control: no-store`** にする。理由別の status・本文・見出し・ログを作らない。鍵・トークン・署名・`Authorization`・未検証クレームをログやエラーに含めない。

一様性の範囲は、HTTP とルーティングが受け付けた同じ認証必須経路の認証結果。HTTP の構文不正・見出し全体上限超過等の既存 400/431、経路なしの 404、明示公開経路の動作は変えない。公開経路は認証を要求せず、この方式を通して identity を付与しない。認証失敗時はハンドラ・before フックを呼ばない。after フックを含め最終出力で理由差・秘密が出ないことを統合試験する。

全 HTTP 処理時間の一致は保証しない。公開の長さ・形式・認証成否で早期に拒否してよい。署名の一致部分の長さ・期待署名・鍵で実行経路を変えないことを定数時間検査で守る。

## 決定 4: 差し込み口・時計・定数時間の境界

| 入口 | 効果・責務 |
|---|---|
| 起動時設定読込 | `env`。鍵と audience を検証して所有する |
| 純粋トークン検証核 | `none`。設定と公開時刻を明示受取、成功時だけ主体を返す |
| 認証アダプタ | `clock`。起動時設定を所有する `move` クロージャを `AuthVerifier.verify` に入れ、`Router.with_auth` に渡す |
| 壁時計入口（新規） | `clock`。Unix 秒の `Result` / `Option`、取得失敗と範囲外は失敗値 |

上表は後続 API の責務であり、実装済み宣言ではない。現実装の `AuthVerifier` は `fn(&AppRequest) -> Result<AuthIdentity, HttpResponse>` のフィールドである（ADR 0033 の素描にある `HttpRequest` ではない）。既存の効果上限に `clock` は含まれる。言語の効果を広げたり `env` / `clock` を `none` と偽ったりしない。

不足する入口を 6-6a で先に入れる。

- 現在の `AppRequest.header` は最初の値だけを返す。全件を数えられる `header_count` 相当を追加し、単一値 API だけでは重複検査済みと扱わない。
- 単調な時計や `task.timeout` の時刻は Unix 時刻の代用にしない。`targets server` の最小壁時計入口（例: `clock.unix_seconds`）を追加する。OS の壁時計取得だけを信頼境界とし、Sigil 側で範囲検査する。OS エラーは値で返し、推測値や 0 で成功させない。評価器は明示的な固定時刻・失敗の注入、ネイティブは Linux の壁時計を使い、両方に同じ契約を置く。一般的な日時・タイムゾーン API は作らない。
- 時計は起動時にも確認し、要求時は 1 回取得して検証核へ渡す。同期済みの壁時計が配備前提。時計の巻き戻りによる再有効化は状態なし方式では防げないため、時計監視と G3 の報告に残す。

秘密を扱う HMAC と照合は 6-1 の実装を再利用する。`ct_eq` の代わりに通常の `==` や先頭からの比較を使わない。秘密を新しく扱うラッパーは `ct_` 命名・`Secret` 型・モジュール基準とともに `tests/ct_rules.rs` へ登録し、公開引数と秘密の領域の流れを記録する。署名成否の結果で分岐する検証器全体や公開入力の解釈を定数時間関数とは称さない。型基準の適用は設定・I/O と暗号の核を分離して検証し、暗号関数の登録漏れを検査の除外で回避しない。

`ct_rules_static`・`ct_rules_clif` と登録漏れ検査を必須にし、静的規則 R1〜R6 と最適化後検査を維持する。定数時間の保証はネイティブだけで、評価器は機能の比較用。計測は ADR 0020・0024 の方法と時期（暗号実装変更・Cranelift 更新・G3）に従い、測定だけを合流の根拠にしない。

## 脅威と対策（G1）

| 分類 | 何から何を守るか | 対策と ADR 0020 の根拠 |
|---|---|---|
| 入力 | 任意のバイト列・重複・巨大な長さから処理系と主体解釈を守る | 署名前の意味解釈禁止、厳格な形式、非再帰、検査後の整数演算、`Err`、評価器とネイティブの突き合わせ。決定 1・2・4 |
| 認証 | 偽造・方式差替え・別サービスのトークンから保護経路と identity を守る | HMAC-SHA256 固定、全文署名、定数時間照合、期限と audience、既定拒否、公開経路一覧の監査。決定 1・2・3 |
| 秘密 | タイミング・ログ・応答・一時コピーから鍵と期待署名を守る | 32 バイト鍵、`Secret`、一時領域の扱い、CT 検査、固定失敗、秘密を表示しない。決定 1・3 |
| 資源 | 長大入力・再帰・大量の暗号処理から接続とサービスの CPU / メモリを守る | HTTP の既存見出し長・数・時間・接続上限に加え、token 1024、payload 404、鍵 32 バイト固定、HMAC は入力当たり最大 1 回、再帰なし。決定 2・4 |

TLS は前段終端とし、前段からサービスまで信頼できる内部網を前提にする。HMAC は盗聴や持参者の再使用を防がない。レート制限・個別失効・強い時計巻戻り対策を実装済みとは扱わず、公開前の G3 で運用上の受容を記録する。

## 決定 5: ファジング・境界試験

`fuzz_bearer_token` を既存 `tests/common/fuzz.rs` の仕組みに追加する。外部クレートは加えない。対象は Authorization 抽出・重複検査、token 枠、base64url、署名後 payload 解釈と純粋検証核。固定公開試験鍵・audience・時刻を使い、鍵を乱数入力や失敗ファイルに含めない。

- ランダムバイト、コーパス変異、文法からの生成を併用。正しい署名を付けた payload も生成し、署名後の分岐まで届かせる。
- 切断、区切り過不足、非正規 base64url、末尾ビット、空、重複、カンマ、NUL、各長さ境界、整数最大、`iat == now`、`exp == now`、寿命 3600/3601、audience 不一致を種にする。
- 署名の 32 位置をそれぞれ壊す。署名不一致なら payload の内容によらず同一の失敗値・外部応答。署名検証前に意味解釈へ進まないことも試験用の観測で確認する。
- どの入力も `Ok` / `Err`、契約違反・範囲外・桁あふれ・panic・スタック枯渇なし。同じ入力・設定・時刻について評価器とネイティブの結果が一致する。ネイティブはまとめて実行する。
- corpus は `tests/fuzz/corpus/bearer_token/`、縮小回帰は `tests/fuzz/regress/bearer_token/`、失敗保存は `target/fuzz-failures/bearer_token/`。種・入力番号・生成方法・件数を記録し、異常終了時の current 保存も既存枠を使う。
- 短い実行は通常の `cargo test` に固定種で追加。CI 全体の増分 60 秒以内になる回数を実測して記録する。長い実行は同じ入口で `SIGIL_FUZZ_ITERS` / `SIGIL_FUZZ_SEED` を利用し、既存の定期実行へ接続されることを確認する。ワークフロー変更が必要なら人間担当の別 PR とし、接続前に完了としない。G3 では ADR 0020 に従い全対象を 1 時間以上実行する。

`limit_` 試験は token 1024/1025、payload 404/405、audience 128/129、subject 256/257、鍵 31/32/33 バイト、寿命 3600/3601 秒を含む。外側上限ちょうどでも内側形式不正なら拒否する（正規形式の最大 token 590 は成功例を置く）。鍵 32 バイトも不正 hex なら拒否。設定失敗時の待受禁止、時計失敗、ハンドラ未実行、秘密がログ・エラーに出ないことを試験する。

## 実装段の分割案と完了条件

本 PR は文書のみ。後続 Issue は下表の単位で切り、既存段の完了を変更しない。前の段が通るまで後の段を完了としない。

| 段 | 依存 | 内容と完了条件 |
|---|---|---|
| 6-6a: 境界の入口 | 本 ADR、6-4・6-1 | 最小壁時計入口と評価器の時刻・失敗差替え、ネイティブ取得、効果の完全一致、時刻範囲、`AppRequest` のヘッダ全件数確認。実際の差し込み口と型の検査。必要な信頼基盤差分・原始操作一覧・前提文は独立した小 PR にする。日時一般へ拡大しない |
| 6-6b: 純粋検証と CT | 6-6a | 厳密 base64url・固定 binary・署名対象・署名前意味解釈禁止、時刻・audience・subject の検証。独立に計算した公開固定ベクタ（署名対象・鍵・payload・タグの全値）と否定例を記録し、評価器/ネイティブで一致。CT 登録・漏れ検査・静的/CLIF 検査が通る |
| 6-6c: 設定と web 統合 | 6-6b | 環境変数を起動時一回、短鍵等は待受前拒否。設定所有クロージャを既存 `with_auth` へ接続し、成功 identity、重複拒否、固定 401、公開経路、時計失敗、秘密非表示、破棄・共有と最小効果を直接 dispatch とネイティブ接続で確認 |
| 6-6d: fuzz・上限・G2 | 6-6c | `fuzz_bearer_token`、短い実行と定期長い実行の接続、回帰 corpus、全上限と失敗一様性、CT 検査、秘密非表示、統合試験を確認。以下の G2 記録を本 ADR の補足 A に追加して初めて 6-6 完了とする |

G2 の記録場所は **本 ADR の補足 A（6-6d の最後の PR で追加）**。記載項目は、各段の PR/SHA、実際の API/効果/信頼境界、`ct_rules_static` / `ct_rules_clif` / 登録漏れ検査、fuzz 回数・種・生成方法・短い実行時間増分・定期実行接続、境界試験名、評価器/ネイティブの突き合わせ、秘密非表示と一時領域の扱い、未解決の問題と範囲外への送り先。ADR 0020 決定 1・2・4 の根拠が欠けた状態を完了としない。G2 は G3 の外部公開承認を代替しない。

## 人間の判断が要る点

- 現在、近接した仕様判断はない。上記の細目は委任に基づく正式判定の結果。起草 PR は CI と別系統（agy、Gemini 系統）のレビューへ出し、本作業ではマージしない。
- 後続実装で必要な CODEOWNERS 対象（CT 表・信頼基盤・上限の置き場所等）や CI の変更は、その範囲に応じた既存手続きで扱う。本 ADR によって人間専用ファイルの変更権限を与えない。
- G3 では配備の時計、TLS 終端・内部網、短命でも残る replay、鍵交換運用を所有者へ提示する。

## 根拠の限界と範囲外の問題

独自形式は汎用 JWT 実装の複雑さを避ける一方、外部の発行側がこのバイト仕様を正確に実装する必要がある。6-6b の独立ベクタと別系統レビューで照合する。自前 fuzz は網羅誘導ではなく、静的 CT 検査・計測にも ADR 0020 の限界がある。

既存の ADR 一覧に 0034・0035 の行がないこと、および ADR 0033 補足 A に記載されたメモリ上限ログの出力先の整合課題は本件の範囲外。既存 ADR の記録や実装は修正しない。

## 補足 A: 6-6 の G2 記録（定期 fuzz 接続待ち）

**判定は保留**。6-6d の実装試験は以下のとおり通過したが、定期の長い実行への接続は人間担当の別 PR が必要で、接続成功を確認するまで Issue #512・6-6・G2 を完了とは扱わない。G3 の外部公開承認も与えない。

### 対象の履歴と実際の境界

| 段 | PR / 試験対象 SHA |
|---|---|
| ADR | [#502](https://github.com/goodnasubi/sigil/pull/502) |
| 6-6a 信頼基盤 | [#504](https://github.com/goodnasubi/sigil/pull/504) / `d1ce17d4d692cc23c38a900ff5ed27284c465d65` |
| 6-6a 境界入口 | [#505](https://github.com/goodnasubi/sigil/pull/505) / `f72f8ed698c1b910b8c55b9915d5b710143de7ef` |
| 6-6b 純粋検証・CT | [#508](https://github.com/goodnasubi/sigil/pull/508) / `ae7b3ffd30a0247240a602cb727c1b03804af17c` |
| 6-6c 設定・統合 | [#511](https://github.com/goodnasubi/sigil/pull/511) / `21368418f72bda1bf1a0739161dbd15b7f13afc2` |
| 6-6d fuzz・上限 | [#518](https://github.com/goodnasubi/sigil/pull/518) / `9bf4bd69c41febeb4e3e7958fa2db0ac3613794e` |

試験の基点は main `ae0d736fa71646aacb46e8a6a504dc56d0bf344e`。提出前に `421a146` へrebaseし、間の差分が AGENTS.md と ADR 0034 のみで製品・試験コードが不変であることを確認した。6-6d は新規試験・fixture・本記録のみで、製品コード、信頼基盤、上限、依存、ワークフローを変更しない。

- `AppRequest.header_count(&self, &String) -> Int` は `effects none`。全出現数を大小文字不問で数え、認証必須経路は1件だけ許す。HTTP parser の正規化を通らない直接 dispatch でも検査する。
- `clock.unix_seconds() -> Option<Int>` は `effects clock`。OS 境界は `std.os.clock.os_clock_realtime_seconds` の Linux `clock_gettime(CLOCK_REALTIME)` だけ。評価器は固定時刻・失敗注入、native は OS 取得。戻り値の範囲を Sigil 側で確認する。今回の fuzz では両側とも公開固定時刻へ試験差替えする。
- `web.verify_bearer(&Secret, &String, &Bytes, Int) -> Result<AuthIdentity, String>` は `effects none`。受信 ASCII を署名し、正規形と上限の確認後、署名が合って初めて payload を解釈する。成功時だけ `AuthIdentity.subject()` を返す。
- `web.load_bearer_config() -> Result<BearerConfig, ConfigError>` は `effects env`。二つの変数を起動時各一回読み、`BearerConfig.into_verifier() -> Result<AuthVerifier, ConfigError>`（`effects clock`）が起動時計を確認し、設定を `move` クロージャで所有する。既存 `Router.with_auth` / `AuthVerifier.verify` に接続し、要求時計も一回だけ取得する。鍵の再読込・大域の可変鍵は作らない。

### ADR 0020 決定 1: CT と秘密の流れ

`cargo test --test ct_rules -- --nocapture` の全11試験が成功。`ct_rules_static` に当たる実名は `static_real_targets_are_clean`、`ct_rules_clif` は `clif_real_ct_equal_is_clean` と `clif_bearer_signature_adds_only_public_contract_branches` 等。登録漏れ検査 `completeness_no_missing_targets` と表の `table_is_well_formed` も成功した。

既存表は `std.web.ct_bearer_signature` の `message`・受信 `tag` を公開、鍵を秘密として R1〜R6 を検査する。`verify_bearer` も `audience`・`token`・`now` を公開とした上で鍵の流れを検査し、照合 Bool だけを公開比較結果として扱う。鍵は既存 HMAC と CT 照合へ渡る。秘密領域から読み戻した値も秘密とする登録・領域追跡を維持し、検証器全体や公開入力の形式分岐を CT とは称さない。今回、暗号実装と Cranelift の版を変更していないため CT 計測の再実施はなく、G3 の計測は残る。静的/CLIF 検査を計測だけで置き換えない。

`claims_are_unreachable_until_signature_matches_eval_native` は、本物の意味解釈入口だけを標識に置き換え、正しい署名の短い payload のみ到達し、無効署名なら到達しないことを両実行系で観測する。独立固定ベクタは署名32位置の各破壊と常に `unauthorized` となることを検査し、fuzz corpus の同じ32入力は実際の認証アダプタの固定401も検査する。不正 payload かつ署名不一致の入力も同じ応答となる。

### ADR 0020 決定 2: fuzz・回帰・突き合わせ

新規入口 `tests/fuzz_bearer_token.rs` は既存 `tests/common/fuzz.rs` を利用し、通常の `cargo test` で実行される。外部クレートは加えていない。公開固定鍵 `00 01 ... 1f`、audience `sigil.api`、Unix 秒 `1700000000` を用い、鍵は乱数入力・失敗入力へ入れない。

- corpus は `tests/fuzz/corpus/bearer_token/` の138件。切断・区切り・非正規 base64url・末尾ビット・空・重複・カンマ・NUL・各長さ境界・整数最大・時刻境界・寿命・audience 不一致を含む。回帰は `tests/fuzz/regress/bearer_token/` の3件で、既知の否定入力を縮小した空 token、正しく署名する短い payload、空値の重複。今回発見した障害とは称さず、すべて毎回生成前に replay する。入力形式と固定ベクタからの由来は [試験入力の説明](../../tests/data/bearer_fuzz_512.md) に記録した。
- 生成は random bytes / corpus変異 / 文法からの生成を順に回す。文法入力の payload は試験ドライバで正しく署名して意味解釈まで到達させ、受理可否と主体を Rust の独立 payload 判定にも突き合わせる。固定 token は6-6bの独立計算値を再利用する。
- native は一度だけビルドし、64件以下・転送48KB以下でバッチ実行する。評価器の件数と全結果、native の終了状態・標準出力・標準エラーを比較する。異常停止・契約違反・範囲外・桁あふれ・スタック枯渇は試験失敗となる。評価器の CT は主張しない。
- 保存先は `target/fuzz-failures/bearer_token/`。既存枠の `current` / `fail-<seed>-<index>` に加え、バッチ全入力のhexを `batch`、種・絶対入力番号・生成方法・mode・件数を `batch.meta` に実行前保存する。端数バッチの失敗も回収可能。成功した一時入力と生成実行ファイルは削除する。

Codex Cloud、Linux x86-64、`rustc 1.100.0 (0ef1bc386 2026-11-09)`、`CARGO_BUILD_JOBS=2` での実測:

| 実行 | 種 | 生成件数と内訳 | replay込み総数 | nativeビルド込み試験時間 |
|---|---|---|---|---|
| 既定の短い実行 | `5865676808348631041` | 12（random / mutation / grammar 各4） | 153（corpus138＋回帰3） | 3.735秒 |
| 環境変数による追加実行 | `7` | 600（各200） | 741（同上） | 9.070秒 |

追加の2テストファイルだけを実行した `cargo test --test fuzz_bearer_token --test bearer_limits_512 -- --nocapture` はビルドを含む wall **7.221秒**（fuzz3.735秒＋limit3.41秒）で成功した。ローカルでの追加分は60秒以内。通常 `cargo test` による全量CIの増分そのものは未実測で、共有runnerの負荷・コンパイル条件に依存するため、PRのCIで最終確認する。ローカル全量テストは実行していない。

長い実行も同じ入口で `SIGIL_FUZZ_ITERS` / `SIGIL_FUZZ_SEED` を受け取ることは上記600件で確認した。ただし既存 `.github/workflows/fuzz.yml` は `fuzz_json` / `fuzz_parse` を個別指定し、本対象は未接続。必要な人間担当の別 PR と、同じseed/scale/失敗artifactを使う追加コマンドを [Issue #512 の記録](https://github.com/goodnasubi/sigil/issues/512#issuecomment-6897495321) に示した。別 PR のマージ・定期実行成功の確認前に G2 を閉じない。G3 の全対象1時間以上の実行も今回の600件で代替しない。

### ADR 0020 決定 4: 全上限・統合・一時領域

`cargo test --test bearer_limits_512` の2試験が成功した。

- `limit_token_payload_audience_subject_key_lifetime_eval_native`: token1024/1025は内側形式不正なら拒否。payload404 / audience128 / subject256 の正規最大token590は成功し、payload405 / audience129 / subject257は拒否。鍵31/32/33バイト、寿命3600/3601も同じ固定ベクタ群で確認する。
- `limit_key_hex_31_32_33_and_invalid_32`: 起動設定の31/32/33バイトと、32バイト相当64桁でも不正hexを確認する。成功以外は変数名と固定分類のみ。評価器/native両方で値・空ログ・空stderrを確認する。

既存 `bearer_verify_507` の2試験と `bearer_config_509` の7試験も全成功。特に `native_connections_startup_rejection_sharing_and_fixed_response` は設定/起動時計失敗時の待受禁止とnative接続、`direct_dispatch_without_http_normalization_and_request_clock_failure` は正規化前の重複と要求時計失敗、`verified_identity_and_uniform_rejections_eval_native` は成功主体・全固定401・公開経路・before/after/ハンドラ未実行を確認する。`minimal_effects_ownership_and_traits` は最小効果、所有設定の共有・破棄、`authentication_is_hidden_in_evaluator_and_native_drop_traces` は両実行系の秘密非表示を確認する。

復号用 `Bytes` と不正hexの部分復号領域は `Secret` へ移して破棄時にゼロ化する。鍵の環境変数 `String`、OS環境、ほかの通常コピーまで消去するとは主張しない。`BearerConfig` / 検証器は表示・鍵取出しAPIを持たず、`ConfigError` は値を保持しない。HTTPヘッダ・要求・`AppRequest` の破棄ログは内容を伏せ、トークンや未検証クレームもログへ出さない。OS環境・ダンプ・プロセスアクセスの制御は配備責務として G3 に残す。

fmt と `cargo clippy --all-targets -- -D warnings` は成功。未解決の送り先は、定期 fuzz 接続と実行証跡（人間担当の別PR・Issue #512）、全量CI（本PR）、外部公開前の時計監視・TLS終端/内部網・replay受容・鍵交換運用・CT計測/長時間fuzz（G3）。本記録は仕様の追加・外部公開承認ではない。

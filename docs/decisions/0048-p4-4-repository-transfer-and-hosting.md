# 0048: P4-4 リポジトリの転送とホスティング

- **状態**: 提案（typesafe-ai 正式判定済み。別系統のレビュー前。実装完了・外部公開の許可を意味しない）
- **日付**: 2026-11-17（JST）
- **対象**: [Issue #582](https://github.com/goodnasubi/sigil/issues/582)、レーン E

## 背景と前提

[ADR 0044](0044-p4-scope.md) の P4-4 として、[ADR 0047](0047-p4-3-repository-core.md) の核に HTTP の clone・fetch・push と小さな管理画面を載せる。P1 の `std.web`・[認証](0037-p1-authentication.md)、P2 の [UI](0040-p2-2-ui-model.md)・[型付き通信](0041-p2-3-fullstack.md) を再利用する。[ADR 0045](0045-p4-1-database-core.md) 決定 4 の永続化規則、0047 の SHA-256 形式・オブジェクト検証・参照 CAS を継承する。先行 ADR のマージは、そのアプリケーションの実装済みを意味しない。

起草と依存の仕様基準は `goodnasubi/sigil` の完全 SHA `b521f0532c34855f2f7fb3361b8147fa2a938148`（PR #581 を含む main）。0048 はファイル・索引・オープン PR に未使用で、本 PR の索引行で予約する。取得はこの SHA を checkout し、同梱 `Cargo.lock` で `cargo build --locked --release`。Rust 版・ターゲット・成果物の検査値も実装側で記録する。浮動 main に依存しない。fs・圧縮等の不足を含むため、この SHA で永続ホスティングが完成できるとは扱わない。不足が揃った時点で、依存更新 PR により必要機能を含む完全 SHA と検証記録を固定する。

仕様は本リポジトリに置き、実装は ADR 0047 と同じ、**所有者が作成・命名する別のリポジトリ管理用リポジトリ**に置く。作成・権限・CI・保護設定は所有者の操作で、作成までは実装しない。アプリケーションは純 Sigil／`trust none` とし、公開標準 API を使う。Git コマンドの呼出し、`raw.*`／`unchecked` の追加、言語の不足の暗黙の回避で完成扱いにしない。

本 PR は本 ADR と索引だけ。処理系・実装・バックログは変更せず、起草 PR 作成まででマージしない。

## 選択肢と決定

評価軸は、AI が実装し人間が仕様をレビューできること、最小の信頼基盤、明示的な拒否、段階ごとの使える成果物、本物の Git との比較、所有者の範囲・非公開方針で固定した。正式判定の入力・応答を末尾に記録する。

| 判断点 | 採用 | 他案を採らない理由 | 確率／confidence |
|---|---|---|---|
| 転送 | SHA-256 の Smart HTTP v0、転送専用 pack 出力、単一参照 push | v2・SHA-1・SSH・多参照 atomic の同時対応は検証範囲が大きい。独自 JSON 転送だけでは本物の Git が接続できない | 1.00／1.00 |
| ホスティングと認可 | 単一ノードの bare 群、private 既定、Bearer とリポジトリ ACL、小さな同一 origin UI | 匿名書込みは既定拒否に反する。汎用 forge・OAuth・課題管理は本段に不要 | 1.00／1.00 |
| 上限・持続性・検証 | 既存 HTTP 上限の維持、隔離検証後に同期・CAS、固定上限、Git 比較と障害注入 | 無上限は資源を守れず、正常系だけでは認可とデータ保全を確認できない | 1.00／1.00 |

## 決定 1: 転送の対応集合

サーバーは **Git Smart HTTP の v0** を実装する。`Git-Protocol: version=2` があっても v0 の参照広告を返す既存 Git のフォールバックを用い、v2 コマンドを v0 として解釈しない。オブジェクト形式は SHA-256 のみ。広告の `object-format=sha256` と、要求された能力・OID の形式を検査し、SHA-1 を推測変換しない。

| HTTP の入口 | 要求・応答 |
|---|---|
| `GET /repos/<id>/info/refs?service=git-upload-pack` | read 認可後、`application/x-git-upload-pack-advertisement`。pkt-line の `# service=git-upload-pack\n`、flush、参照広告、flush |
| 同上の `service=git-receive-pack` | write 認可後、`application/x-git-receive-pack-advertisement`。同じ構造 |
| `POST /repos/<id>/git-upload-pack` | `application/x-git-upload-pack-request` を検査し、`application/x-git-upload-pack-result` を返す |
| `POST /repos/<id>/git-receive-pack` | `application/x-git-receive-pack-request` を検査し、`application/x-git-receive-pack-result` を返す |

上記 service 以外、重複・余分な query、未知の能力・不正な pkt-line・余剰データは拒否する。URL の id は決定 3 の正規形だけを受理し、別名経路や URL の token を認めない。dumb HTTP による objects・refs の直接配信はしない。要求の `Content-Encoding` は identity／未指定を受理し、**upload-pack の POST に限り gzip も受理する**。receive-pack の gzip、その他の符号化・多重符号化は415で拒否する。応答の HTTP 圧縮は使わず、すべての広告・結果は `Cache-Control: no-store` とする。

認証・認可後に gzip を上限付きで展開し、**展開後の要求本文は1 MiB、符号化要求本文は2 MiB**を超えない。展開途中にも累計を検査し、上限超過を検出した時点で停止して413を返す。ISIZE の宣言だけを信頼して割り当てない。単一の gzip member のみを受理し、ヘッダ（任意項目・FHCRC を含む）、deflate の終端、trailer の **CRC-32 と ISIZE**、本文の全消費を検証する。破損・打切り・連結 member・余剰データは400で拒否し、全検証が済むまで pkt-line の要求処理を始めない。ADR 0045 の共通圧縮契約に、この gzip フレーム検査と CRC-32 を要求する。zlib の Adler-32 で代用しない。純 Sigil／`trust none`、入力・展開量・メモリ上限、比較・ファジングの規則を継承し、不足する契約・標準 API の実装は別作業とする。

- upload-pack の広告能力は `object-format=sha256`、`symref=HEAD:refs/heads/<default>`（有効な場合）、`side-band-64k`。receive-pack は `object-format=sha256`、`report-status`、`ofs-delta`。実装していない能力を広告しない。空リポジトリの能力広告にはゼロ OID と `capabilities^{}` を使い、存在しない HEAD を生成しない。
- upload-pack では、注釈付きタグの `refs/tags/<name>` 行の直後に、タグを再帰的に剥いた非 tag オブジェクトの OID と `refs/tags/<name>^{}` の peeled 行を含める。核の型・参照・循環・深さ検査を通す。軽量タグには付けず、receive-pack の広告には peeled 行を出さない。peeled 行は取得用の補助情報であり、実在する参照や新しい取得権限とは扱わない。広告全体（peeled 行を含む）にも応答1 MiBの上限を適用する。
- pkt-line は 4 桁の hex 長（長さ自身を含む）、`0000` flush を検査する。v0 に無い delimiter・response-end を解釈しない。side-band 要求時は channel 1 の pack、失敗は channel 3、最後に flush。要求されなければ ACK／NAK 後に生 pack を返す。進捗通知は必須にしない。
- **未対応**: v1／v2 の専用コマンド、SHA-1、SSH、dumb HTTP、thin pack、shallow／deepen、partial clone／filter、任意 OID の取得能力、複数参照 push、atomic push、参照削除、force push、push-options、署名 push、LFS、サブモジュールの再帰取得、hooks。対応表に残し、黙って成功させない。`thin-pack`・`delete-refs`・`atomic` 等は広告しない。

### clone・fetch

1. read 認可して、そのリポジトリの `refs/heads/*`・`refs/tags/*` と有効な HEAD のスナップショットを広告する。内部参照は公開しない。private の広告と pack に同じ認可を適用する。
2. `want`（64 桁 OID、初行だけ能力）、flush、`have`・flush／`done` を v0 の基本 ACK／NAK で処理する。multi_ack は広告しない。stateless RPC の各要求で認証・認可をやり直す。want は公開可能な広告の実参照の先端に限定する（peeled 行だけを根拠にした OID は許可しない）。have は存在確認の応答で未公開 OID の情報を漏らさない。
3. 初版は共通祖先による pack 削減を必須にせず、want からの完全な到達集合を送ってよい。重複を除き、blob・tree・commit・tag の型・参照・OID を核で検証する。未到達／他リポジトリ／内部参照のオブジェクトを取得できない。
4. wire 用に pack v2（magic・version・件数・オブジェクト群・SHA-256 trailer）を生成する。初版の送信は非 thin・非 delta、zlib stored ブロックでよい。これは **転送応答専用**であり、0047 のローカル保存に pack 書込み・repack を足すものではない。応答全体が上限内かを送信前に確かめ、超えれば pack を送らず明示的に失敗する。
5. 本物の Git クライアントは受信検証後にローカル参照を更新する。Sigil 側の clone は核の init と空の宛先を使い、fetch の検証・永続化後に remote-tracking の `refs/remotes/<remote>/*` を CAS 更新する。remote 名は単一の正規な構成要素、refspec は heads から同名追跡先への写像に限定し、任意パス・任意参照の上書きを許さない。clone の checkout は0047の規則に従う。複数の追跡参照の一括原子性は保証せず、各結果を返す。通信失敗を成功扱いにしない。

### push

1. write 認可後の広告を基に、`<old-oid> <new-oid> <ref>\0<capabilities>` と flush、必要な pack を受け取る。**1 要求 1 参照**、`refs/heads/*` または `refs/tags/*` のみ。新規参照は old がゼロ、新規ブランチの new は commit を要求する。既存ブランチは old の子孫への fast-forward のみ。タグは新規のみ。削除・タグ上書き・非 fast-forward・複数参照は全体を拒否する。
2. pack を専用の非公開 staging に置き、pack v2、件数・長さ・SHA-256 trailer・zlib・OID・参照グラフを検証する。OFS_DELTA と REF_DELTA は0047の読取り核を使い、thin pack の外部基底は受け付けない。new から必要な全オブジェクトが受信集合または同じリポジトリの検証済みオブジェクトで閉じ、branch の先端が commit であることを確認する。検証中のオブジェクトを upload-pack から読ませない。
3. リポジトリの排他を取り、最新 ACL・可視性を再検査する。old と現在値の CAS、fast-forward、参照名・上限を再検査する。参照の `.lock` と0047の規約も維持し、外部 Git の変更とは CAS／ロック競合として失敗させる。管理変更は同じ排他に直列化する。
4. 必要オブジェクトを loose として同期・原子的公開し親ディレクトリを同期する。**到達集合の永続化後にだけ**単一参照を同期・rename・親ディレクトリ同期する。DB の WAL を強制しない。`report-status` は unpack と ref の結果を Git の `unpack ok`／`ok <ref>` または `ng <ref> <固定理由>` で返し、同期完了前に `ok` を返さない。
5. 参照公開前の拒否では参照を変更しない。途中に永続化した未到達オブジェクトは残り得るので容量に数え、成功や安全消去と称さない。同期失敗では当該リポジトリの利用を止め、復旧確認する。公開後の応答喪失は成否不明であり、fetch して先端を確認してから再試行する。旧値不一致を成功とみなさない。

本物の Git を client とする接続が必須。Sigil client を作っただけでは完成としない。Git に標準の Bearer 自動ログインを仮定せず、`http.extraHeader` または credential helper が送る `Authorization: Bearer ...` を明示的に設定する。試験は使い捨て token と隔離環境で行い、実 token を URL・コマンド履歴・ログへ残さない。

## 決定 2: 固定上限とエラー

既存 `std.web` の HTTP 上限・タイムアウトを緩めない。要求本文は gzip 展開後を含め1 MiB、応答は1 MiB、符号化要求本文は2 MiBで、HTTP 接続数・ヘッダ・時間は [ADR 0032](0032-p1-http-server.md) を継承する。小さいリポジトリを対象とし、大きい Git リポジトリを扱えるとは称さない。

| 対象 | 初版の上限 |
|---|---|
| 1 pkt-line | 65520 byte（長さ欄を含む） |
| 1 転送のオブジェクト数／展開累計／単体展開サイズ | 4096／16 MiB／8 MiB |
| 1 操作のグラフ走査件数／広告する参照数 | 100000／1024 |
| リポジトリ数／1 repo の保存実体／保存オブジェクト数 | 16／64 MiB／4096 |
| 同時 Git 処理／同じ repo の Git 処理 | 全体 2／1 |
| アプリケーション全体の作業メモリ／管理 API 本文 | 256 MiB／16 KiB |

0047の長さ・深さ・delta連鎖等との**より厳しい上限**を適用する。HTTP 応答1MiBには ACK・side-band・pack trailer も含む。非圧縮 pack ではこれが先に効くことがある。上限を避けて自動分割・自動再試行・検証の省略をしない。UI はページングで1回の応答を抑える。

保存実体は metadata・staging・一時・未到達オブジェクトも含める。処理前に必要な作業領域と保存量を予約し、同時処理の合計を制限する。既存内容を検証して重複を除く場合も、作業領域の予約を省かない。再起動時に計数し直し、上限超過・不整合では新しい書込みを拒否する。GC は未対応なので空きを自動生成しない。staging の除去は0047の明示 cleanup 手順で行い、古いロックを自動削除しない。

長さ・加算・offset は割当て前に検査する。グラフ・delta 展開・ハッシュ処理には件数・累計・循環検査と協調中断を置く。30秒の既存ハンドラ制限を維持するが、中断点のない CPU 処理の厳密な実時間保証とは称さない。256 MiB はアプリケーションの管理対象で、HTTP 接続バッファ等を含むプロセス全体の最悪値は実装時に別途測定・記録する。

HTTP 段階は認証失敗401、private の存在秘匿404、認証済み権限不足403、上限超過413、競合409、不正形式400、未対応媒体415とする。read 権限のない認証済み主体が private の repo に来た場合は404、read はあるが write がない主体が push に来た場合は403とする。private への不明・無権限主体には repo の存在・参照・使用量を返さない。認証必須経路の token 失敗自体は0037の一様な401を維持する。Git 応答開始後は pkt-line `ERR` または receive-pack `ng` で固定理由を返し、応答途中の切断を成功に変換しない。エラーに token・内容・作者情報・内部パスを含めない。

## 決定 3: ホスティング・認証・認可・管理画面

単一ノードで専用 OS 利用者が bare リポジトリ群を所有する。root は所有者が起動設定で許可した専用ディレクトリのみ。repo id は `[a-z0-9_-]{1,64}` の正規形とし、表示名を FS パスに使わない。URL の percent escape・区切りの別表現を受け入れず、パスの各構成要素で symlink を拒否する。Web から任意の既存ディレクトリを import しない。

認証は0037の固定 HMAC Bearer 検証器を再利用し、署名・期限・audience を通った `sub` だけを主体とする。認証成功は権限付与を意味しない。起動設定の管理者だけがリポジトリを作成でき、作成時に owner を指定する。ACL は repo 単位で主体に `read`／`write`／`owner` を明示する（write は read、owner は全権限を含む）。匿名は既定拒否。トークン payload・作者・コミッター名から権限を推測しない。

| 操作 | 必要な権限 |
|---|---|
| private の広告・clone・fetch・一覧と使用量 | read |
| push の広告と更新 | write |
| ACL・owner・既定ブランチ・可視性の変更 | owner |
| 新規 repo 作成 | 起動設定の管理者 |
| 匿名読取り | owner が明示した public-read repo の広告・clone・fetch のみ |

作成時は private、owner は必ず1人以上とし最後の owner を消す変更を拒否する。管理者の例外権限を暗黙に付けず、管理者も通常アクセスには ACL を必要とする。public-read は**隔離された開発環境で匿名読取りを許す設定**であり、インターネットへの公開許可ではない。可視性の変更で既に渡ったデータを回収できない。read は repo 全体の権限で、ブランチ別の秘密分離は提供しない。秘密の履歴は別 repo に分ける。

metadata は repo id・owner／ACL・可視性・既定ブランチ・形式版を持つ上限付きファイルとし、同じ repo 排他の下で一時ファイル同期→同一 FS rename→親ディレクトリ同期する。新規作成は初期化と metadata の同期が済むまで一覧に出さない。認可は常に確定済み metadata を参照し、push 公開直前にも再確認する。不明版・破損・欠落時はサービス対象から外して拒否し、public の既定値で継続しない。バックアップは新規操作を止めて排他を保ち、objects・refs・metadata を同じ確定点でコピー・検証する（0045の完了印・別空ディレクトリ復元規則）。ACL を欠く復元を公開しない。

管理 UI は P2 の JS 出力・型付き RPC で、認可された一覧、作成、可視性・ACL・既定ブランチ変更、上限と使用量の表示だけを持つ。任意コマンド・ファイル編集・課題管理・コード実行・Web clone は含めない。repo の削除、名前変更、quota の引上げ、token 発行・ログイン・cookie session は未対応。

Bearer は外部で発行し、UI は明示入力をページ内メモリにだけ置く。URL・localStorage・sessionStorage・cookie に保存しない。画面から離れたら再入力を要する。管理 API は同一 origin のみ、CORS は有効にせず、変更は POST＋JSON＋Origin の完全一致を必要とする。Origin 欠落も拒否する。GET に副作用を持たせない。サーバーで毎回 ACL を検査し、ボタン非表示を認可の代わりにしない。表示文字列は text としてエスケープし、HTML 挿入や任意 URL を許さない。

## 脅威と対策（公開関門 G1）

| 脅威 | 対策と残る限界 |
|---|---|
| 他 repo／private の参照・隠れた OID の取得、権限競合 | 広告とpackの同一read検査、repo単位ACL、広告先端へのwant限定、他repoオブジェクト共有なし、更新直前再認可。認可済みで進行中の読取りは完了し得る。失効後に既に受け取った内容は回収不能 |
| token 偽造・盗聴・再使用 | 0037のHMAC・期限・audience・Secret・定数時間照合を再利用。TLSは前段終端で内部網も保護。Basic／query／cookieによる代用なし。個別失効・盗難tokenの期限内再使用は防げない |
| パス横断・symlink・フック・設定による実行 | 固定rootと正規id、各構成要素検査、裸repoのみ。hook・filter・外部Git／外部プロセスを実行しない。OS同権限の悪意ある主体は保証外 |
| gzip要求／pack／zlib／delta爆弾、循環、資源枯渇 | staging、割当て前検査、累計展開量・探索・時間・容量・並行数上限、予約、破損は拒否。HTTP上限を黙って拡大しない |
| 中断・同時push・応答喪失による破損 | 単一参照、CASとロック、オブジェクト同期先行、参照同期後応答。成否不明を認めてfetch確認。I/O失敗時停止と冪等な復旧 |
| CSRF・XSS・キャッシュ・ログからの秘密漏れ | Bearerのメモリ保持、同一origin変更API、Origin検査、text表示、no-store、秘密・内容・個人情報をログに出さない。ブラウザ侵害自体は防げない |
| 保存データ・バックアップ漏洩、削除後残留 | OS権限、バックアップを原本と同じ秘密として扱う。保存暗号化は初版未提供で公開前の所有者判断。GCなしで未到達内容が残り、安全消去と称さない |

「いまは外部公開しない」を維持し、CI と手元の隔離コンテナで、使い捨てのデータ・秘密だけを使う。G2 は以下の正負例・秘密・上限・信頼基盤監査を含む。新しい暗号方式は足さず、秘密を扱うラッパーの CT 登録・静的／最適化後検査・計測の対象と結果を記録する。公開 OID の比較は秘密の定数時間保証の対象外。G3 は別セッションの公開前レビュー報告を `docs/security/` に残し、P1 残件 [#547](https://github.com/goodnasubi/sigil/issues/547)〜[#552](https://github.com/goodnasubi/sigil/issues/552) と合わせ所有者が公開可否を判断する。本 ADR はそれらを解決・再起票しない。

## 完了条件と後続の切り方

**本作業の完了は ADR と索引のみの起草 PR 作成**。別系統レビューと CI、マージは後続であり、本作業ではマージしない。P4-4 の実装完了は以下の全てで確かめ、未対応表を残す。

1. 比較相手の本物の Git の版・ビルド／コンテナ検査値を固定し、SHA-256 client から本サーバーへ clone／fetch／push が通る。空repo、初回push、追加commit、branch作成・fast-forward、タグ新規（注釈付き・軽量、upload-pack の peeled 行と receive-pack での不在）、再clone、既存pack/deltaからの読取り、デフォルトv2要求からのv0フォールバック、`-c protocol.version=0` を含める。小さい identity 要求に加え、多数の want／have により実際に `Content-Encoding: gzip` が送られる clone と追加 commit 後の増分 fetch を通し、符号化前後の本文サイズを記録する。認証付きprivateと匿名public-readの両方でOID・参照・内容・モードを比較し、再起動後に `git fsck --strict`／`git log`／`git diff` を通す。Sigil client も本物の Git サーバーおよび本サーバーと突き合わせる。
2. 無認証・不正／期限切れ／別audience・別主体・読取専用のpush・private一覧・広告・任意OID・peeled 行だけを根拠にした want・他repo取得を拒否する。ACL／可視性変更とpushの競合、CAS競合、非fast-forward、タグ上書き、削除、複数参照、SHA-1、未知能力、thin／shallow等の未対応を負例にし、拒否後の参照不変を確かめる。
3. 各上限の直下・一致・超過、宣言と実長不一致、overflow、爆弾・循環・深さ、満杯・同時予約・再起動後の再計数を試験する。gzip の展開後1 MiB・符号化2 MiBの境界、CRC-32／ISIZE 不一致、不正ヘッダ／FHCRC、deflate 終端不正、打切り、連結 member、余剰データ、未対応／多重符号化、receive-pack の gzip 拒否を含め、展開途中での上限停止と検証前に要求処理しないことを確かめる。HTTP側上限にも達する例を含め、応答1MiB超過がHTTP層の曖昧な500になる前にGit操作の失敗として分かり、成功表示が無いことを確認する。メモリ・CPU・時間・並行数を実測し、固定版・装置・件数で両経路の性能を記録する。速度は初版の合否にしない。
4. staging、オブジェクト同期、参照／metadata rename、親同期、応答の前後でkillし、short I/O・ENOSPC・EIO・同期失敗も注入する。永続化順序モデルで未同期書込みの喪失・並べ替えを再現する。成功応答済み参照は必ず残り、未応答は旧／新のどちらかで、欠落オブジェクトや破損ACLによる権限拡大が無い。再復旧・バックアップ／復元・旧版fixtureも検証する。
5. pkt-line、能力交渉、want/have、参照更新コマンド、pack／delta／zlib、gzip フレームと展開、URL／id、metadata／ACLのファジングで、期間・seed・corpus・回帰例を残す。検証省略・同期順逆転・ACL省略を個別に埋めた変異が試験で検出されることを記録する。
6. UIの実ブラウザ試験で権限別の一覧・変更・拒否・上限表示、XSS／Origin不正・GET副作用なし、token非永続化・ログ非漏洩を確認する。ネイティブとSIR評価器で同じ操作列・エラー・上限の結果を比較し、依存fsと認証の信頼差分監査を結び付ける。

後続は、転送codecとwire専用pack出力→upload-pack→隔離受信と単一参照push→metadata／ACLと管理UI→比較・故障注入・ファジングの記録の順で分割する。所有者作成の実装リポジトリ、0047の核、0045共通fsと圧縮の契約・実装が必要。0047の「リモート追跡参照はP4-4」とした部分は決定1で扱う。必要な標準APIの不足は別のIssue／契約ADR→実装として扱い、本PRで追加しない。

## Git の gzip 要求の実測（レビュー対応）

2026-11-17（JST）、Cloud の **Git 2.52.0** を使い、loopback HTTP で要求を採取して gzip を検証・展開し、本物の `git upload-pack --stateless-rpc` へ渡した。SHA-256 bare repo の独立した40ブランチを `-c protocol.version=0` で clone し、各先端に1 commit を追加して fetch した。clone／fetch と取得後の `git fsck --strict` は成功した。

| 操作 | Content-Encoding | 符号化本文 → 展開後本文 | want／have |
|---|---|---|---|
| 40参照 clone | gzip | 1700 → 3093 byte | 40／0 |
| 増分 fetch の3要求 | gzip | 2343 → 4284、2977 → 5468、3296 → 6065 byte | 40／16、40／32、40／40 |
| 単一 branch の小さい clone | 未指定 | 219 → 219 byte | 1／0 |

実行バイナリの SHA-256 は `git`: `8a2ae22e7898e163330a96319b541f92ed6f90fc87576414f72c5a89aa1dc352`、`git-remote-http`: `4cd7c54f3030073faea51cee9ccb7ba58c6b46d4636674d646604c3e53c10174`。これは Git クライアントの gzip 送信挙動の確認であり、未実装の本サーバーとの接続試験ではない。正確な圧縮開始閾値はこの実測では特定していない。

## 正式判定の記録

2026-11-16 の所有者指示に従い Gemini Decisions は使っていない。

2026-11-17 **12:33:02 JST**、Codex Cloud（OpenAI 系統）が `POST https://api.typesafe.ai/v1/systemone`、モデル `jev-1.13.0` で1回実行し、HTTP 200 を受信した。対象は上の転送・ホスティング／認可・上限／持続性／検証の3判断点。結果はそれぞれ `bounded_v0`・`explicit_acl`・`bounded_safe` が確率1.00、confidence1.00、他案0.00。判定途中に評価軸を変えた再実行はしていない。認証は環境の `TYPESAFE_API_KEY` をプロセス内だけで使い、値・ヘッダーを記録していない。

根拠は選択肢表のとおり、既存の核・認証・HTTP境界を保ち、本物のGitとの相互運用と認可・データ保全を検証可能な範囲に限定できること。確率は実装の正しさや安全の証明ではない。次の入力と応答を原記録として、所有者が後から追跡し覆せるようにする。

<details><summary>typesafe-ai 入力 JSON（秘密情報なし）</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間が仕様レビューする。ADR0005の段階ごとの使える成果物、最小の信頼基盤、明示的な制約、参照製品との比較を優先する。所有者のIssue582の範囲はP4-4のADR0048起草のみ、HTTP clone/fetch/pushと小さいSigil管理UI、SSHなし、実装/処理系/バックログ変更なし、PR作成まででマージ禁止。ADR0044は仕様sigil/実装は所有者が作成する別リポジトリ、外部公開しない方針維持。前提ADR0047はSHA256 Gitのみ、pack読取のみ、loose書込、単一参照CASと厳密同期。0045の永続化規則を継承。現sigil基準完全SHA b521f0532c34855f2f7fb3361b8147fa2a938148 をCargo.lock付きlocked buildで取得、不足fs/圧縮実装後は完全SHAへの明示依存更新が必要。提案A: Smart HTTP v0（v2要求にもv0広告）とSHA256 object-format capabilityに限定、Git本体を実行せず純Sigil。GET info/refs?service=git-upload-pack/git-receive-packとPOST同名。upload-packは基本want/have/done ACK/NAK、共通祖先なくても完全な到達集合packを返す。非thin/非delta pack v2を新規に純Sigilで生成（zlib stored可）、0047のpack書込なしはローカル保存に限るのでネットワーク出力のみ追加。受信はpack v2/OFS_DELTA/REF_DELTAを検証し隔離からlooseへ。advertise最小object-format=sha256、upload symref/side-band-64k、receive report-status/ofs-delta、他capabilityなし。thin/atomic/delete-refs/filter/shallow/v2/SHA1/SSHは未対応、無視せず拒否。pushは1要求1refs/headsまたはrefs/tags、新規とbranch fast-forwardのみ、タグ上書き/削除/force/複数参照拒否。認証/ACL/上限検査、隔離packと到達グラフ検証、repo排他下でACL再検査/旧OID CAS/fast-forward確認、オブジェクト同期→参照同期→report-status、応答喪失はfetchで確認し再push。提案B: 単一ノードのbare repo群、OS専用ディレクトリ、repoIDは小文字ASCII英数字と-_ 1..64、表示名をパスに使わずsymlink/任意パス拒否。初版全repo private、管理者が明示したrepoのみ匿名read可（公開フラグは開発隔離内の匿名読取設定であり外部公開許可でない）。private広告とpackは同じread権限、pushはwrite、writeはread含む、ownerのみメタデータ/ACL/可視性変更、createは起動設定の管理者のみ。ADR0037のHMAC Bearer検証とsubに紐づく明示ACL、トークンを権限と見なさない。Gitはhttp.extraHeaderまたはcredential helperが設定するAuthorization Bearerで利用、Basic/URL token/query/cookie未対応。UIはP2 JS/typed RPCで一覧/作成/公開設定/ACL/上限使用量のみ、ブラウザBearerはメモリのみで外部発行、ログイン/発行/保管なし。同一origin、CORSなし、mutationはPOSTとOrigin照合/JSON要求でCSRF拒否。ACL設定はrepoロック下同期原子的ファイル更新、旧/新版どちらか、復旧不正時は非公開既定でなくサービス停止。提案C: 既存std.web上限を緩めずrequest/response各1MiB、pkt-line65520byte、各要求オブジェクト4096/展開累計16MiB/単体8MiB（0047のより厳しい方）、探索100000、refs1024、repo実体64MiBと4096objects、repo数16、同時Git処理2かつrepo毎1、作業メモリ全体256MiB、UI body16KiB。保存量はstaging/未到達/一時も算入し処理前予約、復旧で再計数し上限超なら書込禁止。pack応答が1MiBに収まらないfetchは参照変更なしで明示Limitエラー、分割による隠れた迂回禁止。既存HTTP timeout維持しCPUループは件数上限と協調中断、非割込計算のwallclock保証を称さない。拒否は401認証/404 private存在秘匿/403権限/413上限/409競合、プロトコル開始後はGit ERRやng。Content-Type/encoding検査、キャッシュno-store、HTTP圧縮未対応、全失敗で参照無変更。脅威は権限漏れ/隠しOID取得/圧縮差分爆弾/パス/フック/秘密log/認証再使用/CSRF/破損とクラッシュ、read ACLはrepo単位で到達オブジェクトのみ送信。TLSは前段、現時点は隔離CIだけ、保存暗号化未提供/削除安全消去なしを明記しG1/G2/G3と既存547-552を維持。完成検証は固定版の本物Git SHA256 clientで匿名と認証clone/fetch/push、空repo/複数往復/branch/tag/packed入力/競合/再起動後fsckと比較、権限と境界値の拒否、同期前後killと永続化順序モデル、parser/fuzz/旧fixture/ネイティブSIR一致/UI権限とXSS/秘密CT監査。PRは起草のみ、実装完了と混同しない。",
  "questions": {
    "transport": {
      "type": "choice",
      "criteria": {
        "bounded_v0": "提案A: SHA256のSmart HTTP v0、wire専用非delta pack出力、単一参照push",
        "all_git": "初版からv2、SHA1、SSH、thin、複数参照atomicまで実装",
        "custom_http": "独自JSON転送でGit本体の接続試験を省略"
      }
    },
    "hosting_auth": {
      "type": "choice",
      "criteria": {
        "explicit_acl": "提案B: bare単一ノード、private既定、Bearerとrepo ACL、小さい同一origin UI",
        "anonymous_write": "全repo匿名writeで開始",
        "full_forge": "初版にOAuth、cookie、課題管理、hooksも含める"
      }
    },
    "limits_verification": {
      "type": "choice",
      "criteria": {
        "bounded_safe": "提案C:既存HTTP上限維持、隔離検証と同期CAS、固定上限、Git比較と故障注入",
        "unbounded": "互換性を優先して入力とメモリ上限解除",
        "happy_path": "正常clone/fetch/pushだけで完成"
      }
    }
  }
}
```

</details>

<details><summary>typesafe-ai 応答 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "transport": {
      "type": "choice",
      "choice": "bounded_v0",
      "confidence": 1.0,
      "probabilities": {
        "bounded_v0": 1.0,
        "all_git": 0.0,
        "custom_http": 0.0
      }
    },
    "hosting_auth": {
      "type": "choice",
      "choice": "explicit_acl",
      "confidence": 1.0,
      "probabilities": {
        "explicit_acl": 1.0,
        "anonymous_write": 0.0,
        "full_forge": 0.0
      }
    },
    "limits_verification": {
      "type": "choice",
      "choice": "bounded_safe",
      "confidence": 1.0,
      "probabilities": {
        "bounded_safe": 1.0,
        "unbounded": 0.0,
        "happy_path": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 2365,
    "output_tokens": 130
  }
}
```

</details>

### レビュー対応の追加判定

2026-11-17 **13:01:19 JST**、Codex Cloud（OpenAI 系統）が同じ API・モデルで1回実行し、HTTP 200。対象は gzip 要求の受理と peeled 広告で、`bounded_gzip` と `peeled_upload` がそれぞれ確率0.99／confidence0.98、他案0.01。固定評価軸のまま判定し、起草時の入力・応答は変更していない。gzip 拒否は実測した Git の clone／fetch を妨げるため、上限とフレーム検証を維持して受理する。peeled は upload-pack の補助情報として広告し、実参照に限る want の認可を維持する。確率は正しさや安全の証明ではない。

<details><summary>レビュー対応の typesafe-ai 入力 JSON（秘密情報なし）</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "評価軸（固定）: SigilはAIが実装し人間が仕様レビューする。最小の信頼基盤、明示的拒否、段階ごとの使える成果物、本物のGitとの比較を優先する。所有者指示: PR583のADR0048だけ修正、実装/索引/他文書変更なし、既存ブランチへpushと日本語報告まで、マージ禁止、Gemini Decisions不使用。既存SHA256 Smart HTTP v0とHTTP要求/応答1MiB・符号化要求2MiBを維持。Claude指摘: identityのみではGitのupload-pack gzip要求を拒否する。Git2.52.0をloopback HTTPと本物のupload-packで実測: 小さいclone219byteはencoding未指定、40参照cloneは展開3093byte/符号化1700byteでgzip、増分fetchは40want/16-40haveで展開4284-6065byte/符号化2343-3296byteでgzip、clone/fetch/fsck成功。案A: upload-pack POSTだけidentity/未指定/gzip単一memberを受理。展開後1MiB、符号化2MiB、展開途中に上限検査し超過413、認可後に展開してCRC32/ISIZE/ヘッダ(任意項目とFHCRC含む)/deflate終端と本文全消費検証後だけpkt-line処理、破損/打切り/連結member/余剰は400、他encodingとreceive-pack gzipは415。ADR0045の純Sigil trust none共通圧縮契約にgzip CRC32とフレーム検査を要求（zlib Adler32代用禁止）、不足APIをこのPRで実装しない。案B: gzip拒否を維持し大きいclone/fetch失敗を未対応とし完成条件を縮小。peeled案: upload-packは注釈付きタグ行の直後に再帰的に剥いた非tag OIDのrefs/tags/name^{}行を広告、lightweightに付けずreceive-packには出さない。wantは実参照先端のみを維持しpeeled行を新参照や任意OID取得権限にしない。状態コードは認証済みread無しprivateは404、read有りwrite無しpushは403。Gemini不使用一文とgzip正負例/広告試験をADRに追記、既存正式判定の原記録は変更せず追加判定を別記する。",
  "questions": {
    "request_encoding": {
      "type": "choice",
      "criteria": {
        "bounded_gzip": "案A: upload-pack gzipをCRC32/フレームと上限付きで受理し本物Git比較を維持",
        "reject_gzip": "案B: gzip拒否を維持し大きいclone/fetchを未対応にする"
      }
    },
    "advertisement": {
      "type": "choice",
      "criteria": {
        "peeled_upload": "upload-packのみ注釈付きタグのpeeled行を含め、receive-packには含めない",
        "omit_peeled": "両広告からpeeled行を除外する"
      }
    }
  }
}
```

</details>

<details><summary>レビュー対応の typesafe-ai 応答 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "request_encoding": {
      "type": "choice",
      "choice": "bounded_gzip",
      "confidence": 0.98,
      "probabilities": {
        "bounded_gzip": 0.99,
        "reject_gzip": 0.01
      }
    },
    "advertisement": {
      "type": "choice",
      "choice": "peeled_upload",
      "confidence": 0.98,
      "probabilities": {
        "peeled_upload": 0.99,
        "omit_peeled": 0.01
      }
    }
  },
  "usage": {
    "input_tokens": 1169,
    "output_tokens": 76
  }
}
```

</details>

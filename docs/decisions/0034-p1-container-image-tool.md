# 0034: P1 のコンテナイメージの道具

- **状態**: 提案（Issue #447）。新規判断 A〜C は 2026-11-14 JST に Codex Cloud（OpenAI 系統）が typesafe-ai `jev-1.13.0` の正式判定で決定。A はバイナリ I/O の不足を明示して再判定した。所有者が後から確認し、覆せる記録を末尾に残す。実装の着手・マージの承認を意味しない。
- **範囲**: 段階 6-2 の設計と後続の分割。コード・依存・CI・信頼基盤の設定は変更しない。

## 背景と既存の決定

[ADR 0018](0018-p1-scope.md) の段階表 6-2 は、3-7（暫定ランタイムの削除）と 6-1（暗号）に依存する。目的は、Sigil の静的実行ファイルを Docker でリリースできる形にすること。

次は新しい判断ではなく、既存の決定を継承する。

| 論点 | 継承する内容・出典 |
|---|---|
| 形式 | OCI、静的実行ファイル 1 つ、圧縮しない層（ADR 0018 決定 2）を採用。独自形式は既存決定と互換性目標に反するため採らない |
| 再現性 | 同じ入力から 2 回生成したイメージの digest が一致する（ADR 0018 完了条件）。時刻を含めて毎回変わる案は採らない |
| 判断 4: CI | **所有者の決定（2026-10-26）は A**。Docker と PostgreSQL のテストを CI に入れ、PostgreSQL の版を固定する。本 ADR ではイメージの load/run と既存 `runs_in_scratch_container` の CI 化を段分けする。PostgreSQL の接続テストは 6-5 の担当 |
| 判断 8: 送信 | **所有者の決定（2026-10-26）は A**。P1 は `docker load` で読めるファイルを出すまで。レジストリ認証・送信は実装しない |

ADR 0018 の末尾の「判断 4・8」の選択肢は資料として残っており、未決とは扱わない。上の既存判断は再判定しない。

[ADR 0015](0015-m5-native-output.md) の成果物は Linux x86-64 の静的 ELF。[ADR 0025](0025-runtime-replacement.md) と現行 `src/native.rs` に従い、現在は `cc -nostdlib -static -no-pie` でリンクする。古い glibc 前提を新たに持ち込まない。生成器は既にビルドした実行ファイルを入力とし、コンパイラやリンカを呼ばない。

[ADR 0024](0024-crypto-primitives.md) の `src/stdlib/crypto.sigil` にある `sha256` / `Sha256` を digest に使う。HMAC・PBKDF2 はイメージ生成に不要。現行 `fs.read` / `fs.write` は UTF-8 文字列を扱い、任意の ELF や tar のバイト I/O には使えない（`src/stdlib/os_fs.sigil`）。この不足は後続の別 ADR の前提とし、本 PR では修正しない。

## 判断点・選択肢・決定

評価順は、宣言効果と推論効果の完全一致、信頼基盤の小ささと列挙可能性、守れない層の除去、所有者の既存方針、独立に確かめられる小さい段階とする。

| 判断点 | 推奨と採用案 | 代案と採らない理由 | 正式判定 |
|---|---|---|---|
| A: 実装と信頼基盤 | 通常の検査済み Sigil プログラムとして生成器を作る。先にバイナリ I/O の別 ADR を作る | Rust CLI 内に形式処理を追加すると効果検査外が増える。外部 Docker/OCI ライブラリへの委譲は Sigil で作る目標から外れる | 訂正後 `implementation_revised.A`: 確率 1.0、confidence 1.0 |
| B: 署名・検証 | P1 は SHA-256 による内容の整合性まで。発行者署名は別 ADR | HMAC は共有鍵を持つ受信者も発行できる。公開鍵署名は新しい暗号・鍵管理・レビューが必要 | `signatures.A`: 確率 1.0、confidence 1.0 |
| C: 実行設定と入力範囲 | 固定の単一 `/app`、非 root、追加ファイルなし | 任意 rootfs・root 既定・自由な環境変数の埋込みは、パス検査と秘密混入の組合せを増やす | `runtime_config.A`: 確率 1.0、confidence 1.0 |

近接した結果はなく、所有者への選択の差し戻しはない。判断 A の初回入力は「既存 I/O だけで可能」との誤った前提を含んだため、その結論をバイナリ I/O の追加許可には使わない。訂正前後の全入力・応答を末尾に残す。

## 決定 1: 入出力と形式

生成器は `tools/image/` 以下に置く通常の Sigil プログラムとし、既存の `sigil build` でビルドする。Rust CLI への専用サブコマンドの統合は P1 の必須範囲にしない。入口・引数の渡し方は既存の起動引数 API の範囲で後続 PR に定め、入口のために言語機能を増やさない。

入力は静的 ELF のファイルと出力先だけ。入力のバイト列そのものを再現性の入力とする。ELF の magic、64 bit・little endian・x86-64・実行可能形式、ヘッダ/プログラムヘッダの範囲、`PT_INTERP` / `PT_DYNAMIC` がないことを検査する。これは Sigil が生成した証明、プログラムの安全性、全命令の正しさの検証ではない。入力元とビルドの信頼は利用者の責任として明示する。

出力は **OCI Image Layout 1.0.0 を入れた非圧縮 tar** とする。OCI image/index/manifest/config の形式は 1.0 系を使い、仕様に定義された `mediaType`、`schemaVersion: 2`、descriptor の `digest` と `size` を記録する。

- `oci-layout`: `imageLayoutVersion` は `1.0.0`。
- `index.json`: 単一 manifest descriptor、platform は `os=linux`、`architecture=amd64`。参照名は固定の `sigil-app:latest`（`org.opencontainers.image.ref.name`）。
- `blobs/sha256/<小文字16進数>`: manifest、config、単一の非圧縮 tar layer。`size` は参照対象の実際のバイト長。
- manifest は `application/vnd.oci.image.manifest.v1+json`、config は `application/vnd.oci.image.config.v1+json`、layer は `application/vnd.oci.image.layer.v1.tar`。
- config は `rootfs.type=layers` と単一 `diff_ids` を持ち、`Entrypoint=["/app"]`、`User="65532:65532"`、`WorkingDir="/"`。層には相対パス `app` の通常ファイル 1 つだけを置き、mode は `0555`、uid/gid は 0 とする。

Docker 独自の `manifest.json`、圧縮層、ベースイメージの取得、複数アーキテクチャ、任意 rootfs の読込み・展開、リンク、デバイス、whiteout は扱わない。`docker load` の OCI archive 対応は Docker の版や image store に依存するため、OCI 準拠だけで互換性を達成したとは扱わない。後続 CI の固定した Docker 環境で、出力を変換せず load/run できることを完了条件とし、その版・image store 設定を記録する。通らない場合は独自形式や二重形式へ黙って切り替えず、本 ADR の改訂を先に出す。

実行時の環境変数・ポート・引数は利用者が `docker run` に渡す。秘密、ビルド元のパス、ホストの uid/gid、ホストの環境変数はイメージに入れない。証明書・timezone・シェル・追加ファイルを要するサービス、特権操作を要する用途はこの最小構成の対象外。

## 決定 2: 再現性と検証の意味

同じ ELF バイト列と同じ生成形式の版から、**外側の tar 全体を含めて同じバイト列**を生成する。コンパイラの出力自体が異なる場合の一致は保証しない。

- JSON は UTF-8、固定キー順・空白なし・改行なしの一つのエンコードにする。任意文字列を受け取る場合も必ず JSON のエスケープを通す。`created`、`history`、無作為値、ホスト情報は省略する。
- 内外の tar は USTAR の通常ファイルに限定する。mtime=0、uid/gid=0、uname/gname は空。固定した名前順、外側ファイルの mode=0644、数値欄・checksum・512 バイト境界のゼロ詰め・終端のゼロブロック数を固定し、PAX/GNU 拡張を出さない。
- 各 blob の SHA-256 と size を、実際に出すバイト列から求める。非圧縮層なので layer descriptor の digest と config の `diff_ids` は同じ値。manifest digest、config digest（Docker image ID）、外側 archive digest は異なる対象であり、混同しない。生成完了の出力には対象名と digest を対で表示する。
- ファイル長・ヘッダのオフセット・足算/掛算の桁あふれ、USTAR の数値欄の表現範囲を出力前に検査する。読込み前に効く上限付きバイナリ I/O が前提であり、全量確保後の長さ判定で代用しない。具体的な上限値・エラー型・途中書込みと再実行の扱いは前提のバイナリ I/O ADR で決め、無制限のまま実装を進めない。失敗した出力は成功として表示・loadしない。

P1 は外部の任意 archive を取り込む検証 CLI を作らない。生成した Bytes と参照の整合性は生成器の検査とテストで確認し、外部実装との照合も行う。SHA-256 の一致は発行者を認証しない。受信者が整合性確認に使う期待 digest は、信頼できる別経路から取得する必要がある。イメージ署名・公開鍵検証・鍵管理・レジストリ送信は別 ADR に回す。

## 決定 3: 検査の内側と外側

| 範囲 | 扱い |
|---|---|
| ELF のヘッダ確認、USTAR、JSON、OCI、digest と参照の組立て | 通常の Sigil の型・所有権・効果検査の内側。形式の計算は `effects none`。新しい `unchecked` を形式処理に置かない |
| ファイル入力・出力、診断 | 既存の効果カテゴリ `fs.read` / `fs.write` / `console` で宣言・推論を一致させる。バイナリ I/O の API はまだ存在せず、別 ADR が前提 |
| バイナリ I/O の実体 | OS の既存 read/write 経路を再利用する方針。`Bytes` と生の番地の変換、`unchecked`・前提の文、評価器側の組込み、`RAW_OPS`・許可表への影響を別 ADR で列挙し、所有者レビューを受ける。本 ADR はその追加を承認しない |
| コンパイラ・評価器・既存核/OS原始操作・カーネル・Docker | 信頼基盤または外部実行基盤に残る。Docker は生成器の依存にせず、利用者と互換性テストが実行する |

型・効果検査は OCI の意味や暗号の正しさを証明しない。純粋な実装を検査の内側に置くことと、形式仕様に合うことを分けて確かめる。生成器自身の効果宣言の過不足を検査器が拒否することもテストする。新しい効果カテゴリや信頼基盤の許可を、イメージ出力のために暗黙に追加しない。

## 実装の段分けと後続 Issue の方針

本 ADR のマージ後、完了した PR の作成者が `scripts/sync-issues.sh --dry-run` でバックログを確認し、既存の `spawn_rules` と重複しない形でスクリプト経由で後続 Issue を起票する。今回起票や BACKLOG の編集は行わない。

| 段 | 内容 | 依存・確認 |
|---|---|---|
| 6-2a: バイナリ I/O の ADR | 任意 Bytes の読み書き、確保前の上限、所有権、失敗時の出力、効果と信頼基盤の差分を設計。起動引数で必要な機能も調査 | 本 ADR。コードと混ぜず、raw/unchecked/許可表が変わるなら所有者レビューを必須とする |
| 6-2b: バイナリ I/O の実装 | 承認済み 6-2a の範囲のみ。文字列 I/O の意味は変えず、バイナリと不正 UTF-8、境界値、部分 I/O、失敗、評価器/ネイティブ一致を確認 | 6-2a と必要な所有者承認。依存クレートは足さない |
| 6-2c: 純粋な形式処理 | ELF 確認・USTAR・固定 JSON・OCI descriptor と SHA-256 を、通常の Sigil モジュールとして追加 | 6-2a。既知バイト列、長さと桁あふれ、不正ヘッダ、`PT_INTERP`/`PT_DYNAMIC` 拒否、既知 SHA-256 と参照を検査。外部実装でも tar/JSON を読む |
| 6-2d: 生成器の入口と結線 | 単一 ELF から archive を出す。再現性と出力失敗の扱い、非 root 設定、効果の完全一致を確かめる | 6-2b・c。別ディレクトリ・別ホスト時刻・別入力名で同一 Bytes、入力の1バイト変更で対象 digest が変わること |
| 6-2e: Docker 互換性と既存テストの CI 化 | 新規テストは固定 Docker 環境で load/run し、stdout/終了値・image config・非 root を確かめる。既存 `runs_in_scratch_container` も毎回実行する | テスト追加は 6-2d の後。**CI 設定のみの別 PR は人間が担当**（AGENTS.md）。Docker の版・store を記録。PostgreSQL の固定イメージと services の設定は 6-5 と調整 |
| 6-2f: 完了の記録 | load/run・再現性・信頼基盤差分・外部ツール版・CI の実行記録を ADR に追記 | 全前段の完了と CI 緑。7-1 のサービス結合は後続 |

CI 設定の人間担当 PR が入るまでは、手動で Docker が動いても 6-2 の完了とはしない。新しい言語機能・原始操作・外部依存が必要になった場合も、担当範囲外をそのまま実装せず、別 ADR/Issue として扱う。

## 影響と参照

- この PR は新規 ADR だけ。README の番号予約はユーザー指定の変更範囲外なので行わず、PR #449 が 0033 を使用しているため、次の空き番号 0034 を選び、PR に例外を記録する。
- バイナリ I/O と CI の不足を明示した段階計画であり、生成器が既に動くという主張ではない。Docker 互換性の実測、上限値と I/O API は後続の完了条件に残る。
- 形式の参照: [OCI Image Layout](https://github.com/opencontainers/image-spec/blob/v1.0.0/image-layout.md)、[Manifest](https://github.com/opencontainers/image-spec/blob/v1.0.0/manifest.md)、[Configuration](https://github.com/opencontainers/image-spec/blob/v1.0.0/config.md)、[Layer](https://github.com/opencontainers/image-spec/blob/v1.0.0/layer.md)。

## 正式判定の入力・応答と決定の記録

2026-11-14 JST、実行環境 Codex Cloud。ユーザーから提示された呼出手順に従い `POST https://api.typesafe.ai/v1/systemone`、`Content-Type: application/json` を使用。認証は環境変数で行い、キーや認証ヘッダーは記録しない。以下は実際の body と応答全体である。双方 HTTP 200。判定は推奨の追認ではなく選択肢ごとの確率と confidence を確認した。

### 初回

```json
{
  "model": "jev-1.13.0",
  "state": "固定評価軸（優先順）: 1. Sigil の理念: 宣言効果と推論効果の完全一致を検査系が担保し、信頼基盤を小さく明示的・列挙可能にし、守れない層を無くす。2. 所有者の明示方針: ADR0018でP1はLinux x86-64の静的実行ファイル1つをOCI形式の非圧縮層に格納しdocker load/run可能にする。同一入力のdigest一致は完了条件。判断4は2026-10-26にDocker/PostgreSQLテストのCI化（版固定、設定は人間の別PR）、判断8はP1のレジストリ送信なしで決定済み。既存言語のライブラリを利用せずSigilで作る（ADR0005）。3. 小さい検査可能な実装と独立した外部互換性テスト。\nIssue447は新規ADRだけの起草。実装・CI変更・マージはしない。既存SHA-256/HMAC/PBKDF2はsrc/stdlib/crypto.sigilにあり、ネイティブ出力はcc -nostdlib -static -no-pieのELF。新規の判断だけを比較する。推奨は各問のA。digestは内容の整合性であり発行者認証ではない。既存fsの原始操作と検査系・コンパイラ・OSは信頼基盤に残り、形式パーサを検査済みにしても意味の正しさの証明にはならない。",
  "questions": {
    "implementation": {
      "type": "choice",
      "criteria": {
        "A": "生成器をtools/image/以下の通常のSigilプログラムとし、ELF確認・USTAR・JSON・OCI組立て・SHA-256を検査済みコードで実装。fs.read/fs.writeと診断console以外の効果、unchecked、新原始操作を追加せず既存OS窓口を使う。Rust CLIに形式処理を重複実装せず、外部Dockerは利用者/CI側でload/runする。CLI統合は後回し。",
        "B": "Rustのsigil CLIにOCI生成/検証とSHA-256を実装し、Sigilの効果検査外の信頼範囲として追加・列挙する。配布は容易だが形式処理を検査外に増やす。",
        "C": "Docker CLIや既存OCIライブラリを生成器の必須依存にしDockerfileで組み立てる。実績ある外部実装だが既存部品を置換する目標と検査範囲を縮小する。"
      }
    },
    "signatures": {
      "type": "choice",
      "criteria": {
        "A": "P1はSHA-256 digestによる整合性まで。公開鍵署名・鍵配布・HMACによるイメージ認証は実装しない。digest単独では発行者を認証しないと明記し、信頼できる経路でdigestを別途取得する運用とする。署名方式と鍵管理は必要時の別ADR。",
        "B": "P1でHMAC-SHA-256によるイメージ認証を加える。既存部品を再利用できるが配布者と受信者が秘密を共有し誰でも発行できる。",
        "C": "P1で公開鍵署名と検証を実装する。配布認証が可能だが未実装の公開鍵暗号、鍵管理、署名形式とレビュー範囲が増える。"
      }
    },
    "runtime_config": {
      "type": "choice",
      "criteria": {
        "A": "P1は単一の/app（0555）とlinux/amd64、Entrypoint=[/app]、User=65532:65532、WorkingDir=/に固定。層は1つ、環境変数や秘密・時刻を組み込まず、実行時にdocker runで渡す。汎用的なrootfs入力は受け付けない。最小非rootのサービスに絞り、特権や追加ファイルを要する用途は別ADR。",
        "B": "rootを既定とし、任意rootfs、UID、entrypoint、env、追加ファイルをP1から自由に設定できる汎用イメージ生成器とする。用途は広いがパスの安全検査と再現性/秘密混入の組合せが増える。"
      }
    }
  }
}
```

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "implementation": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    },
    "signatures": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    },
    "runtime_config": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 1569,
    "output_tokens": 103
  }
}
```

初回の B・C の決定を採用する。A は I/O の前提不足が判明したため、次の訂正判定で置き換える。

### バイナリ I/O の不足を踏まえた訂正

```json
{
  "model": "jev-1.13.0",
  "state": "固定評価軸（優先順）: 1. Sigil の理念: 宣言効果と推論効果の完全一致を検査系が担保し、信頼基盤を小さく明示的・列挙可能にし、守れない層を無くす。2. 所有者の明示方針: ADR0018でP1はLinux x86-64の静的実行ファイル1つをOCI形式の非圧縮層に格納しdocker load/run可能にする。同一入力のdigest一致は完了条件。判断4は2026-10-26にDocker/PostgreSQLテストのCI化（版固定、設定は人間の別PR）、判断8はP1のレジストリ送信なしで決定済み。既存言語のライブラリを利用せずSigilで作る（ADR0005）。3. 小さい検査可能な実装と独立した外部互換性テスト。\nIssue447は新規ADRだけの起草。実装・CI変更・マージはしない。既存SHA-256/HMAC/PBKDF2はsrc/stdlib/crypto.sigilにあり、ネイティブ出力はcc -nostdlib -static -no-pieのELF。新規の判断だけを比較する。推奨は各問のA。digestは内容の整合性であり発行者認証ではない。既存fsの原始操作と検査系・コンパイラ・OSは信頼基盤に残り、形式パーサを検査済みにしても意味の正しさの証明にはならない。\n訂正: 現行のfs.read/writeはUTF-8文字列APIでELF/tarの任意バイトI/Oを提供しない。最初のimplementation判定はこの前提を見落としたため再判定する。第一判定を隠さず残す。所有者は新規ADRのみ変更可、実装は後続Issue。新規バイナリI/Oの必要性を本ADRに明記し、そのAPI・上限・信頼基盤差分は後続の別ADRで決める。検査済みという理由でOS窓口の信頼範囲を無視しない。",
  "questions": {
    "implementation_revised": {
      "type": "choice",
      "criteria": {
        "A": "Sigilの検査済み生成器を目標とし、最初の後続段をバイナリI/Oの別ADRにする。fs.read_bytes/write_bytes相当のAPI、Bytesの所有権、上限付き読込み、既存read/write syscall経路の再利用、unchecked/RAW_OPS/許可表の差分、評価器との一致を列挙して承認後に実装する。新原始操作は必要性が証明されるまで追加せず、所有者レビューを守る。形式処理とSHA-256は検査済みSigilに保つ。前提ADRなしに実装開始しない。",
        "B": "バイナリI/Oの不足を理由に生成器全体をRust CLIに実装し、OCI/ELF/tar/JSON/digestをSigil効果検査外の新たな範囲とする。既存RustファイルI/Oを使えて短期実装が容易だが検査外の形式処理が増える。",
        "C": "既存Docker/OCIライブラリを必須依存として呼び出し、Sigil実装をP1から外す。互換性とバイトI/Oは外部実装に任せられるが置換目標と検査可能な範囲が縮む。"
      }
    }
  }
}
```

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "implementation_revised": {
      "type": "choice",
      "choice": "A",
      "confidence": 1.0,
      "probabilities": {
        "A": 1.0,
        "B": 0.0,
        "C": 0.0
      }
    }
  },
  "usage": {
    "input_tokens": 1284,
    "output_tokens": 41
  }
}
```

**最終決定**: `implementation_revised.A` を採用する（確率 1.0、confidence 1.0、他案 0.0）。検査外の形式処理を増やす代わりに、バイナリ I/O を別 ADR の前提として可視化する。初回の「新 unchecked 追加なし」という前提をそのまま実装許可に使わず、差分の列挙と所有者レビューを残す。B・C は初回の A を採用し、いずれも確率 1.0・confidence 1.0。

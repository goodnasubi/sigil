# 0057: UDP ソケットの追加

- **状態**: 提案（予約: Issue #677、設計の正式判定済み）
- **日付**: 2026-11-23（JST）
- **対象**: [Issue #677](https://github.com/goodnasubi/sigil/issues/677)、レーン E

## 背景

sigil-atlas-dns（DNS）と sigil-hourglass-time（NTP）が必要とするデータグラム通信を、両部品の実装に先立って標準ライブラリに追加する。[ADR 0031](0031-p1-first-wait-ops-and-socket-api.md) の決定 3・判断点 E が定めた範囲に UDP を加える。現行の TCP クライアントは [ADR 0034](0034-p1-postgresql-client.md) 決定 4、TCP の数値 IPv6 は [ADR 0051](0051-ipv6-listen.md) に従う。

## 決定 1　公開 API と所有権

サーバーターゲットの OS 層に、相手をデータグラムごとに指定する非接続型 UDP を公開する。固定の受信先を開く `UdpSocket` と、OS が選んだエフェメラルポートで問い合わせと応答を扱う `UdpClient` を分ける。型と効果を対応させ、サーバーとして開いたソケットの操作に `net.server` が現れるようにする。接続済み UDP、TCP の `accept` に相当する操作、ブロードキャスト・マルチキャスト用の設定は初版の公開範囲外とする。

| 操作 | シグネチャ | 効果 |
|---|---|---|
| `net.udp_bind` | `udp_bind(host: &String, port: Int) -> Result<UdpSocket, NetError>` | `net.server` |
| `net.udp_open` | `udp_open() -> Result<UdpClient, NetError>` | `net.client` |
| `UdpSocket.local_port` | `local_port(self: &UdpSocket) -> Int` | `net.server` |
| `UdpClient.local_port` | `local_port(self: &UdpClient) -> Int` | `net.client` |
| `UdpSocket.send_to` | `send_to(self: &mut UdpSocket, data: &Bytes, host: &String, port: Int) -> Result<Unit, NetError>` | `net.server, task.suspend` |
| `UdpClient.send_to` | `send_to(self: &mut UdpClient, data: &Bytes, host: &String, port: Int) -> Result<Unit, NetError>` | `net.client, task.suspend` |
| `UdpSocket.recv_from` | `recv_from(self: &mut UdpSocket, max: Int) -> Result<(Bytes, String, Int), UdpReceiveError>` | `net.server, task.suspend` |
| `UdpClient.recv_from` | `recv_from(self: &mut UdpClient, max: Int) -> Result<(Bytes, String, Int), UdpReceiveError>` | `net.client, task.suspend` |

- `udp_bind` の契約は `0 <= port && port <= 65535`。0 は OS に割り当てを任せる。`udp_open` は IPv4 ワイルドカードのポート 0 に bind する。`local_port` は割り当て済みのポートを返す。生成時（bind 直後）に `getsockname` で取得して検査し、取得に失敗した場合（読めない、または番号が 0）は fd を閉じて `NetError` を返す。値には番号を保持せず、`local_port` は同じ `getsockname` を読んで返す（bind 後の番号は変わらないので、観測できる結果は保持する場合と同じ）。保持しないのは、表の `local_port` が `net.server`／`net.client` の効果を持ち、効果検査（ADR 0004 の規則 3。宣言した効果は本体が起こすものと一致する）では、効果を起こす `raw.syscall` を本体で呼ぶ関数だけがそれを宣言できるためである。保持した整数を返すだけの関数は効果を宣言できない。TCP の `local_port`（`port_of`）も同じ作法である。
- `send_to` の契約は `1 <= port && port <= 65535 && data.byte_len() <= 65507`。`recv_from` の契約は `1 <= max && max <= 65507`。65,507 は IPv4 の最小 IP ヘッダと UDP ヘッダを差し引いたペイロードの上限であり、OS や経路はそれより小さいサイズでも送信を拒否しうる。
- ホストは既存の IPv4 ドット 10 進表記だけを受け付け、パースに失敗したら `NetError.Other` を返す。受信成功値はペイロード、通常のドット 10 進表記の送信元アドレス、送信元ポートの順とする。送信元ポート 0 も受信値には表現できる。
- どちらの型も中身を公開せず、fd を所有する。既存の TCP 資源と同様に `Sendable` とし、`Copyable`・複製・`Shareable`・型間の変換は提供しない。送受信は排他的な借用を取る。寿命を終えた所有値の破棄は fd を 1 回閉じる。確保後、所有値になる前の失敗経路でも fd を閉じる。

## 決定 2　データグラムの境界と待機

- `send_to` の成功は空のペイロードも含め、1 データグラム全体を OS が受け付けたことを表す。配送は保証しない。成功後の再送や、正の短い戻り値から残部を送る処理は行わない。長さが一致しない場合は `NetError.Other` を返す。
- `recv_from` は 1 回につき 1 データグラムを消費する。長さ 0 も成功する。`max` を超えるデータグラムは全体を消費し、断片を渡さず `UdpReceiveError.DatagramTooLarge` を返す。既存 TCP の `NetError` の腕は維持する。受信固有の列挙型は `DatagramTooLarge` と、既存の分類に対応する `AddrInUse`、`PermissionDenied`、`Closed`、`Other` を持つ。`Closed` は OS エラーの分類に限り、空のデータグラムは終了の合図として扱わない。
- fd は非ブロッキングかつ close-on-exec で作る。`EINTR` では操作を再試行し、`EAGAIN`／`EWOULDBLOCK` では既存の `raw.io_wait_fd` で送信可能または受信可能を待ってから操作を再試行する。準備通知の直後に再び待ちが必要になる場合も同じ手順を使う。待機の失敗は各エラー型の `Other` に写す。時間制限は既存の `task.timeout` を使い、取り消しは通常のタスク取り消しとして伝播する。取消時は ADR 0031 決定 1-2 と同様に待ち登録を解除してから所有値を破棄する。
- Linux の `recvfrom` は `MSG_TRUNC` を指定して元のデータグラム長を得る。戻り長が `max` を超えたらペイロードはコピーせず破棄する。成功時にコピーするのは `0..=max` に収まる戻り長の範囲だけとする。生のバッファは各 syscall 試行の中で確保と解放を完結させ、待機をまたがせない。

## 決定 3　数値アドレスと対象ファミリー

初版の UDP は IPv4 の数値アドレスに限る。必要な UDP の土台を提供しつつ、初版の信頼境界では `sockaddr_in` 一種類の入力・出力だけを扱う。IPv6 の入力は IPv4 のパース失敗として扱う。後から IPv6 を加える場合は同じ API の形を使い、ADR 0051 の数値表記、明示的な V6-only、IPv4 射影アドレスとスコープ ID の扱いを踏まえて、アドレスの出力と許可表も含めて設計する。TCP の IPv6 は ADR 0051 の対象である。

名前解決を分離する ADR 0031 の方針を維持する。数値アドレスの書式を検査・変換するだけで、DNS や OS のリゾルバは呼ばない。DNS 問い合わせの解釈と解決は sigil-atlas-dns の責務とする。

## 決定 4　評価器とネイティブの共通実装

ADR 0031 判断点 D に従い、`std.os.net` に両実行系で使う Sigil 実装を置く。役割別の公開メソッドは所有権と効果の境界を示し、生の I/O の処理を共用する。評価器は `src/ir_eval/call.rs` の `raw.syscall` 許可表で次の呼び出し形だけを追加する。既存の TCP 用の引数形は維持する。新しい syscall 番号や、利用者向けの fd／待機プリミティブは要らない。

| 呼び出し | 追加する引数制約 |
|---|---|
| `socket` | `AF_INET`、`SOCK_DGRAM \| SOCK_NONBLOCK \| SOCK_CLOEXEC`、protocol 0 の組だけを追加する。 |
| `bind`・`getsockname` | 既存の IPv4 形（16 バイトの `sockaddr_in`、`getsockname` の長さ入出力は 4 バイト）を使う。 |
| `sendto` | `MSG_NOSIGNAL`、0〜65,507 バイトのペイロード、長さ 16 の `AF_INET` 宛先。送信バッファと宛先の範囲が生きていることを検査する。 |
| `recvfrom` | `MSG_TRUNC`、1〜65,507 バイトの受信バッファ、16 に初期化した 4 バイトの長さ領域、16 バイトの送信元領域。三領域の生存・境界・非重複を syscall の前に検査し、共通実装は返却後に送信元の長さとファミリーを確認してから読む。 |

ネイティブの `unchecked` の前提にも同じ番地・長さ・所有の条件を明記する。包括的な許可に置き換えず、既存のアドレス引数が NULL の TCP 形と、追加する UDP 形を区別する。[ADR 0018](0018-p1-scope.md) 判断 2 の信頼基盤レビューを両変更に適用する。

## 実装 PR の分割

ADR マージ後に別 Issue を起票し、次の順で進める。

| PR | 内容と確認 |
|---|---|
| 1　評価器の許可表 | UDP の引数形と検査だけを追加し、`tests/raw_syscall_table.rs` で許可する形、拒否するファミリー・フラグ・長さ・範囲外や解放済みの番地・出力領域の重複、既存 TCP 形を検証する。公開 API の追加に先立ち、信頼境界の差分を所有者レビューに出す。 |
| 2　共通 UDP API | `std.os.net` の共通実装、標準ライブラリの型・操作・効果・契約の表と固定テスト、検査器の資源型の性質、評価器とネイティブ双方の fd 破棄を結ぶ。公開 API は両実行系を揃えて入れ、CODEOWNERS 対象の変更には所有者承認を得る。 |

PR 2 は Rust の `std::net::UdpSocket` を外部の相手とするループバック試験を評価器とネイティブで実行する。エフェメラル bind、送信元と宛先、複数データグラムの境界、空と上限ちょうどの受信、上限超過の消費と次の受信、数値以外の拒否、待機後の成功、タイムアウト・取り消し後の資源回収を確認する。別のタスクへの所有移動は通り、複製・共有、bare とブラウザでの使用、効果宣言の不足は検査で止まることも確認する。ネイティブ側は Linux x86-64 を対象とする。

## 判断記録

2026-11-23（JST）、プロジェクトの理念である型と効果による機械的な検査、明示された所有権、共用実装、最小の信頼境界を評価軸とし、typesafe-ai `jev-1.13.0` で正式判定した。

| 問い | 採用した案 | 確率 | confidence |
|---|---|---:|---:|
| API | 非接続型 | 0.95 | 0.92 |
| 効果 | 役割別の資源型 | 0.73 | 0.60 |
| 切り捨て | データグラムを消費して型付きエラー | 0.97 | 0.96 |
| 実行系 | 共通 Sigil 実装 | 0.99 | 0.98 |
| アドレス | 初版は IPv4 数値 | 0.86 | 0.72 |
| 実装 PR | 許可表を先行、両実行系の API を同時公開 | 0.54 | 0.31 |

PR 分割は単一 PR 案（確率 0.45）と接近した。信頼境界の差分だけを独立して所有者レビューに出せて、利用者の API は両実行系で同時に提供できるため、先行分割を選んだ。

<details>
<summary>typesafe-ai 入力 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "state": "固定評価軸（優先順）: Sigil は AI がコードを書き、人間は仕様をレビューする言語。(1) 機械で検査できる安全・所有権と最小の効果・信頼基盤を守る。(2) 1 実装で評価器とネイティブの意味を揃え、公開契約を小さく明示する。(3) 所有者の指示 #677 に従い、sigil-atlas-dns と sigil-hourglass-time の前に標準 UDP を ADR 化し、名前解決は含めず、ADR PR は文書のみとする。ADR 0031 決定 3/E は UDP・IPv6・DNS を除外、D は評価器の引数制限付き許可表と共通 Sigil 実装、ADR 0034 は TCP の役割別型、ADR 0051 は TCP の数値 IPv6 を V6-only で採用し現行 TCP 実装・許可表にも反映済み。現行 sendto/recvfrom はアドレスなしの TCP 形だけを許す。UDP は接続の確立や EOF がなく、問い合わせと応答を同じソケットでやりとりする。初版は既存効果・型機構・非ブロッキング待機を使い、依存クレートや保護された上限ファイルを変更しない。",
  "questions": {
    "api": {
      "type": "choice",
      "criteria": {
        "unconnected": "非接続型で始める。入口 net.udp_bind(host, port)、受信 recv_from と送信 send_to、OS が選んだポートを得る local_port。クライアント専用の入口を用意するかは効果の問いで判定する。",
        "connected": "UDP connect を必須にして既定の相手との send/recv だけを出す。",
        "both": "非接続と接続を同時に公開する。"
      }
    },
    "effects": {
      "type": "choice",
      "criteria": {
        "role_types": "TCP と同じく役割別の所有型。net.udp_bind(host,port) は UdpSocket + net.server（明示的な受信ポートの公開を含む）、net.udp_open() はエフェメラルポートだけに bind する UdpClient + net.client。両型の send_to/recv_from はそれぞれの効果と task.suspend。内部は型検査できる単一実装を共用し、サーバーソケットを client 効果へ変換しない。公開先は server のみ。",
        "one_type": "UdpSocket 一型。net.udp_bind を net.server、エフェメラル専用の net.udp_open を net.client とし、両方の send_to/recv_from は net.client。ソケット移動後も受信は net.client 扱い。公開先は server のみ。",
        "server_only": "UdpSocket と net.udp_bind の一入口。bind も全操作も net.server とし、DNS/NTP クライアントも明示的に server 効果を要する。公開先は server のみ。"
      }
    },
    "receive": {
      "type": "choice",
      "criteria": {
        "reject": "受信単位を一データグラムとし、max 超過は全体を消費して型付き DatagramTooLarge エラーを返し、断片は渡さない。空 UDP は成功。Linux MSG_TRUNC で元長を知り max 以下のみコピー。上限は IPv4/IPv6 に共通の 65507 bytes を採る。",
        "flag": "切り捨てた接頭辞とフラグを成功値で返す。",
        "silently": "切り捨てを成功として返し、フラグを出さない。"
      }
    },
    "backend": {
      "type": "choice",
      "criteria": {
        "shared": "std.os.net を共用し、評価器で既存 syscall のソケット種別・アドレス付き引数の形だけを制限付き追加。評価器とネイティブを同じ外部相手との試験で突き合わせ、負の許可表テストも行う。",
        "separate": "Rust の UDP 組み込みを評価器に別実装し、ネイティブは Sigil で実装する。"
      }
    },
    "family": {
      "type": "choice",
      "criteria": {
        "v4": "初版 IPv4 数値のみ。IPv6 は API の host 形状を保って別段階で ADR 0051 の V6-only 等に従う。必要部品の当面の UDP 土台と許可表差分の最小化を優先。",
        "v4_v6": "初版から TCP と同じ IPv4 と IPv6 数値を、明示的に分離した V6-only でサポート。パーサと既存 setsockopt を再利用するが UDP にもアドレスの出力と許可表の両サイズの検証が必要。"
      }
    },
    "split": {
      "type": "choice",
      "criteria": {
        "boundary_then_api": "ADR マージ後、制限付き評価器の許可表と信頼境界の負の試験の小 PR を先に出し所有者レビューを得る。次に共通 UDP API・値の破棄・公開表と評価器対ネイティブの外部試験を同時に入れる。",
        "single": "ADR マージ後に許可表・標準 API・両実行系の試験を同じ実装 PR でレビューする。",
        "native_first": "ネイティブ実装を公開してから評価器を別 PR で付ける。"
      }
    }
  }
}
```

</details>

<details>
<summary>typesafe-ai 応答 JSON</summary>

```json
{
  "model": "jev-1.13.0",
  "answers": {
    "api": {
      "type": "choice",
      "choice": "unconnected",
      "confidence": 0.92,
      "probabilities": {
        "unconnected": 0.95,
        "connected": 0.05,
        "both": 0.0
      }
    },
    "effects": {
      "type": "choice",
      "choice": "role_types",
      "confidence": 0.6,
      "probabilities": {
        "role_types": 0.73,
        "one_type": 0.24,
        "server_only": 0.03
      }
    },
    "receive": {
      "type": "choice",
      "choice": "reject",
      "confidence": 0.96,
      "probabilities": {
        "reject": 0.97,
        "flag": 0.01,
        "silently": 0.02
      }
    },
    "backend": {
      "type": "choice",
      "choice": "shared",
      "confidence": 0.98,
      "probabilities": {
        "shared": 0.99,
        "separate": 0.01
      }
    },
    "family": {
      "type": "choice",
      "choice": "v4",
      "confidence": 0.72,
      "probabilities": {
        "v4": 0.86,
        "v4_v6": 0.14
      }
    },
    "split": {
      "type": "choice",
      "choice": "boundary_then_api",
      "confidence": 0.31,
      "probabilities": {
        "boundary_then_api": 0.54,
        "single": 0.45,
        "native_first": 0.01
      }
    }
  },
  "usage": {
    "input_tokens": 1907,
    "output_tokens": 221
  }
}
```

</details>

# 0051: net.listen における IPv6 数値アドレスの受け付け

- **状態**: 提案（typesafe-ai 正式判定は未実施。起草者の案として明記。末尾に判定用の評価軸と問いを記録。別系統のレビュー前。実装完了を意味しない）
- **日付**: 2026-11-18（JST）
- **対象**: [Issue #613](https://github.com/goodnasubi/sigil/issues/613)、レーン E
- **関連**: [Issue #612](https://github.com/goodnasubi/sigil/issues/612)、[ADR 0031](0031-p1-first-wait-ops-and-socket-api.md)、[ADR 0046](0046-p4-2-database-server.md)

## 背景と課題

ADR 0031（決定 3-2、判断点 E）では、P1 におけるソケット API の最小スコープとして IPv4 のみ（`AF_INET`、ドット 10 進表記）を対象とし、IPv6 は当初の範囲外（未決事項）としていた。

一方、P4-2 データベースサーバー（ADR 0046 決定 2、Issue #612）では、sigil-db において純 Sigil（`trust none`）で IPv4/IPv6 数値アドレスを受け付ける TCP サーバーを実装することが完了条件となっている。

現状の実装（`src/stdlib/os_net.sigil`）では:
- `net.listen` はホスト文字列に対して `parse_ipv4` のみを実行し、失敗時は `NetError.Other` を返す。
- ソケットは `AF_INET`（ドメイン 2）固定で生成され、`sockaddr_in`（16 バイト）のみを `bind` する。
- そのため、`::1` や `2001:db8::1` などの IPv6 数値アドレスを渡しても待ち受けを開始できない。
- また、SIR 評価器（`src/ir_eval/call.rs`）の `raw.syscall` 許可表も `SYS_SOCKET` が `AF_INET` のみ、`SYS_BIND`・`SYS_CONNECT`・`SYS_GETSOCKNAME` が 16 バイトの `sockaddr_in` のみに制限されている。

本 ADR は、`net.listen` および関連するソケット API において IPv6 数値アドレスを受け付けるための仕様・API・契約（アドレス形式、待ち受けモデル、エラー処理、評価器整合）を定める。

## 決定事項

### 決定 1: アドレスの形式とパース規則

1. **スコープ**: 数値アドレスのみを対象とする。DNS 名前解決（ホスト名解決）は ADR 0018 および ADR 0031 に従い含めない。
2. **表記形式**:
   - RFC 4291 / RFC 5952 に準拠した標準的なコロン区切り 16 進表記（例: `::1`, `::`, `2001:db8::1`）を受け付ける。
   - 16 ビットの 16 進数グループ（0〜ffff、各 1〜4 桁、大文字小文字は不問）を `:` で区切る。
   - `::` による 0 の連続の圧縮表記は、文字列全体で最大 1 回のみ許可する。
   - 角括弧表記（例: `[::1]`）は、`host` 引数としては**受け付けない**（パース失敗として `NetError.Other` を返す）。URL や設定構文の角括弧の除去は呼び出し側の責務とする。
   - スコープ ID（例: `fe80::1%eth0`）は、初版では扱わずパースエラー（`NetError.Other`）とする。
   - IPv4 射影アドレス（IPv4-mapped IPv6 アドレス、例: `::ffff:192.0.2.1`）は、セキュリティ設計原則（ADR 0020、ADR 0027）に基づきパースエラーとして拒絶する。IPv4 と IPv6 の境界を曖昧にせず、IPv4 通信には明示的な IPv4 数値アドレスを用いる。
3. **判別規則**:
   - `host` 文字列に `:` が含まれる場合は IPv6 アドレスとしてパースする。
   - `host` 文字列に `.` が含まれ `:` が含まれない場合は既存の `parse_ipv4` でパースする。
   - どちらのパースにも失敗した場合、または不正な文字が含まれる場合は直ちに `NetError.Other` を返す。

### 決定 2: ソケットファミリーと待ち受けモデル（分離型・V6-only）

IPv6 ソケットにおける待ち受けモデルとして、**分離型（`IPV6_V6ONLY = 1`）を採用する**。

1. **分離型の採用理由**:
   - デュアルスタック（1 つの IPv6 ソケットで IPv4 と IPv6 の両方を待ち受ける動作）は、OS やカーネルパラメータ（`/proc/sys/net/ipv6/bindv6only`）の既定値に依存し、実行環境によって挙動が非決定論的になる。
   - IPv4 射影アドレス経由での予期しない接続受理や、アドレス制限・アクセス制御の迂回といったセキュリティ上の懸念（ADR 0020、ADR 0027）を排除する。
   - 明示性の原則に基づき、`net.listen("::1", port, backlog)` は IPv6 ループバックのみを、`net.listen("::", port, backlog)` はすべての IPv6 アドレスのみを待ち受ける。
   - IPv4 と IPv6 の両方で待ち受けたいアプリケーション（例: sigil-db のサーバー）は、IPv4 アドレス（`0.0.0.0` 等）と IPv6 アドレス（`::` 等）のそれぞれで `net.listen` を明示的に呼び出し、2 つの `TcpListener` を管理する。
2. **システムコールとソケット設定**:
   - ドメイン: Linux x86-64 における `AF_INET6`（値 10）。
   - タイプ・フラグ: `SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC`（値 526337）。
   - プロトコル: 0（IPPROTO_IP）。
   - オプション設定:
     - `SO_REUSEADDR`: `setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, 1, 4)`（既存 IPv4 と同様）。
     - `IPV6_V6ONLY`: ソケット作成直後、bind 前に `setsockopt(fd, IPPROTO_IPV6, IPV6_V6ONLY, 1, 4)`（level 41, optname 26, optval 1, optlen 4）を必ず発行する。
   - アドレス構造体: `sockaddr_in6`（28 バイト）。
     - `sin6_family`: `AF_INET6`（10, 2 バイト、リトルエンディアン）。
     - `sin6_port`: ポート番号（2 バイト、ビッグエンディアン）。
     - `sin6_flowinfo`: 0（4 バイト）。
     - `sin6_addr`: パースした 128 ビット（16 バイト）のアドレス。
     - `sin6_scope_id`: 0（4 バイト）。

### 決定 3: 公開 API・型・効果の不変性

1. **シグネチャの維持**:
   - `pub fn listen(host: &String, port: Int, backlog: Int) -> Result<TcpListener, NetError>`
   - 型・引数・効果（`effects net.server`）は一切変更しない。
2. **`TcpListener` の操作**:
   - `local_port(self: &TcpListener) -> Int`:
     - 内部で呼ぶ `port_of(self.fd)` において、`getsockname` に渡す sockaddr バッファを 28 バイト（`sockaddr_in6` を収容可能なサイズ）に拡張する。
     - ポート番号は `sockaddr_in`（offset 2〜3）でも `sockaddr_in6`（offset 2〜3）でも先頭 2 バイトのファミリ直後にビッグエンディアンで配置されているため、共通のオフセットから抽出可能である。
   - `accept(self: &TcpListener) -> Result<TcpStream, NetError>`:
     - `accept4` は相手アドレス構造体の格納を行わず（番地 0）、非ブロッキングかつ CLOEXEC な新規接続 fd を取得する現行実装のまま変更なく動作する。
3. **`TcpStream` の操作**:
   - `read`、`write_all`、`shutdown` はソケットのプロトコルファミリに依存せず、現行の実装・契約をそのまま維持する。

### 決定 4: クライアント接続（net.connect）の対称的拡張

`net.listen` との整合性・対称性を保つため、`net.connect` においても IPv6 数値アドレスを受け付ける。

1. **シグネチャの維持**:
   - `pub fn connect(host: &String, port: Int) -> Result<TcpClient, NetError>`（`effects net.client, task.suspend`）。
2. **動作**:
   - `host` が IPv6 数値アドレスの場合、`AF_INET6` ソケットを作成し、`sockaddr_in6`（28 バイト）を組み立てて非ブロッキング `connect(42)` を開始する。
   - `io_wait_fd` による待機、`SO_ERROR` の検査、および後続の `TcpClient` 操作（`read`, `write_all`）は IPv4 と同一の安全なフローで動作する。
   - これにより、`trust none` な Sigil アプリケーションやテストコードから `net.connect("::1", port)` で IPv6 サーバーへ接続・検証できる。

### 決定 5: SIR 評価器（raw.syscall 許可表）の拡張

ADR 0031（判断点 D 案 B）に従い、SIR 評価器（`src/ir_eval/call.rs`）の `raw.syscall` 許可表を拡張し、評価器上でも `std.os.net` のコードをそのまま実行可能とする。

許可表の拡張条件:
1. `SYS_SOCKET`:
   - ドメイン `a[0]` として `AF_INET(2)` に加え `AF_INET6(10)` を許可する。
2. `SYS_SETSOCKOPT`:
   - `a[1] == SOL_SOCKET(1) && a[2] == SO_REUSEADDR(2) && a[4] == 4` に加え、
   - `a[1] == IPPROTO_IPV6(41) && a[2] == IPV6_V6ONLY(26) && a[4] == 4` を許可する。
3. `SYS_BIND` / `SYS_CONNECT`:
   - `a[2] == 16` かつ `family == AF_INET(2)`、または
   - `a[2] == 28` かつ `family == AF_INET6(10)` を許可する。
4. `SYS_GETSOCKNAME`:
   - 引数バッファ長 `*addrlen` として 28（`sockaddr_in6` 収容サイズ）を許可する。

### 決定 6: エラー分類

1. アドレスのパース失敗（不正文字、コロン・ドットの混在、不正グループ数等）: `NetError.Other`。
2. ポート重複（`EADDRINUSE` = -98）: `NetError.AddrInUse`。
3. 権限不足（特権ポートへの非特権バインド等、`EACCES` = -13 / `EPERM` = -1）: `NetError.PermissionDenied`。
4. カーネルが IPv6 を無効化している環境でのソケット生成・バインド失敗（`EAFNOSUPPORT` = -97 等）: `NetError.Other`。

## 実装の段分け

ADR 承認後、以下の段分けで進める。

| 段 | レーン | 担当内容 | 完了条件 |
|---|---|---|---|
| 段 1 | E | 本 ADR の起草・合意（`docs/decisions/0051-ipv6-listen.md`） | 本 PR のマージ |
| 段 2 | C1 / D2 | 評価器許可表の拡張（`src/ir_eval/call.rs`）および標準ライブラリ実装（`src/stdlib/os_net.sigil`） | IPv6 アドレスパース、sockaddr_in6、IPV6_V6ONLY、AF_INET6 対応が評価器とネイティブの両方で clippy/fmt を通過 |
| 段 3 | E | 新規統合試験の追加（`tests/net_ipv6.rs`） | `::1` での待ち受け・接続・送受信試験、および IPv4 待ち受けとの共存試験が CI で緑 |

## 正式判定の記録

2026-11-16 の所有者指示に従い、判定には typesafe-ai のみを用いる。

**起草時点では未実施である。** 起草環境（cloud / agy）には `TYPESAFE_API_KEY` が設定されておらず、通信が遮断されている（AGENTS.md の「キーが無い環境」。ADR 0042・0043・0044・0046・0047・0049 起草時と同様）。そのため、本起草 PR では採用案を「判定を経ていない起草者の案」として明記し、以下に判定用の評価軸と問いを記録する。後続のレビューまたは見張りセッション（Codex Cloud 等のキー設定済み環境）において正式判定を実施可能とする。

### 判定用の評価軸と問い（再現用）

- **評価軸（固定）**: Sigil は AI が実装し人間が仕様だけをレビューする原則（ADR 0001）。ADR 0016（明示許可と信頼基盤最小化）、ADR 0020・ADR 0027（AI 時代の安全原則、暗黙の挙動の排除と境界の明確化）、ADR 0031（ソケット API の最小性、評価器での単一実装）、ADR 0046（P4-2 データベースサーバーの IPv4/IPv6 数値アドレス要件）。
- **判定 API 呼び出し形式**:
  `POST https://api.typesafe.ai/v1/systemone`
  `{"model": "jev-1.13.0", "state": "...", "questions": {...}}`

```json
{
  "q1_listen_model": {
    "type": "choice",
    "criteria": {
      "separated_v6only": "分離型（IPV6_V6ONLY=1 を明示設定）。IPv4/IPv6 の境界を明確にし、環境依存と射影アドレスのセキュリティ懸念を排除する（推奨）",
      "dual_stack": "デュアルスタック（単一ソケットで IPv4 と IPv6 の両方を待ち受ける）。OS 既定値依存がありアクセス制御迂回の懸念がある"
    }
  },
  "q2_connect_scope": {
    "type": "choice",
    "criteria": {
      "include_connect": "net.listen と net.connect の両方で IPv6 数値アドレスに対応する。対称性を保ち trust none クライアントからの結合試験を可能にする（推奨）",
      "listen_only": "net.listen のみに限定し net.connect は IPv4 のまま残す。テストは外部の Rust std::net に依存する"
    }
  },
  "q3_address_format": {
    "type": "choice",
    "criteria": {
      "raw_colon_hex_only": "RFC 4291/5952 のコロン区切り 16 進表記（::1 等）のみ受け付け、角括弧 [::1] やスコープ ID は拒絶する（推奨）",
      "bracket_allowed": "角括弧 [::1] も許容してパースする"
    }
  }
}
```

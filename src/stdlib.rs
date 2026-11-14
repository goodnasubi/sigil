//! 標準ライブラリの最小部分のシグネチャと効果の表（M1）。
//!
//! 効果を持たない部分の一覧は ADR 0013 の決定 2 が、効果を持つ部分（`Path`・`IoError`・`console`・`fs`・
//! `Shared`・`Cell`）を含む全体の層への振り分けは ADR 0014 の決定 2 が正式に決めたもの（M3 で実行系が
//! 本体を持つ）。この表は本体を持たず、シグネチャと効果だけを持つ。型検査器はこの表を `Sigil` のシグネチャとして読み、利用者の宣言と同じ表に入れる。
//!
//! 層（ADR 0014 の決定 2・4）: 各表の行に `Layer` の列を持つ。`Core` は OS に依存しない部分（仮のモジュール
//! `std.core`、`targets bare, browser, server`）、`Os` は OS の機能に意味が依存する部分（`std.os`、`targets browser, server`）。
//! 行を層の間で移すのは 1 語の変更だが、`Os` の行の集合は単体テストで ADR の一覧に固定してある。
//!
//! 表の書き方:
//! - 自由関数（`FREE_FNS`）: 名前空間のない関数。`Copyable` な値だけを受け取るものに限る。
//!   標準の自由関数の名前は予約名で、利用者は同じ名前の関数を宣言できない（ADR 0012 の決定 5）。
//! - 名前空間の関数（`NAMESPACED_FNS`）: `fs.read(&p)`・`console.log(&x)` のように呼ぶ。
//! - メソッドと型に付く関数（`IMPLS`）: `impl` の見出し（型引数と型）と、その中の関数。
//!
//! 規則（ADR 0012 の決定 6）:
//! - 読むだけの操作は `&T` の受け手のメソッドにする（`xs.len()`）。受け手は自動で借用される。
//! - 値を複製して返す操作は `T: Copyable` を要求する。
//! - 値を決められない入力は `requires` で除く（`REQUIRES`）。本体は M3 まで持たない。
//!
//! 仮の決めごと（標準ライブラリの最小部分を決める ADR まで暫定）:
//! - `List` の取り出しは参照を返す（`get`: `Option<&T>`。ADR 0012 の決定 3）。

/// 標準ライブラリの層（ADR 0014 の決定 2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// OS に依存しない部分（メモリの確保は使う）。
    Core,
    /// OS の機能に意味が依存する部分。
    Os,
}

/// (シグネチャ, 効果)。効果は `effects` 節の中身（`none`・`console`・`of(f)` など）。
pub type Entry = (&'static str, &'static str);

/// 標準の型で、中身を見せないもの（名前, 型引数の数）。
pub const OPAQUE_TYPES: &[(&str, usize, Layer)] = &[
    ("JsonValue", 0, Layer::Core),
    ("JsonNode", 0, Layer::Core),
    ("JsonField", 0, Layer::Core),
    ("Bytes", 0, Layer::Core),
    ("List", 1, Layer::Core),
    ("Path", 0, Layer::Os),
    ("IoError", 0, Layer::Os),
    ("BinaryIoError", 0, Layer::Os),
    ("ParseError", 0, Layer::Core),
    ("Shared", 1, Layer::Core),
    ("Cell", 1, Layer::Core),
    // 番地（ADR 0016 の決定 1）。`Copyable` で演算子を持たない。番地の計算は原始操作で行う。
    ("Addr", 0, Layer::Core),
    // 秘密（ADR 0024 決定 1、PR 6-1b）。中身は `std.crypto` の Sigil 実装が持つ
    // （埋め込みモジュールの構造体）。表示不可・破棄時ゼロ化は型の性質と
    // ランタイムの破棄で担保する。検査器には中身を見せない型として登録し、
    // 実装の構造体と名前で結ぶ。
    ("Secret", 0, Layer::Core),
    // SHA-256 のストリーミング文脈（ADR 0024 決定 1、PR 6-1c）。`Secret` と
    // 同じく中身は `std.crypto` の Sigil 実装が持ち、検査器には中身を
    // 見せない型として登録して実装の構造体と名前で結ぶ（秘密ではないので、
    // 破棄時ゼロ化や表示不可の性質は持たない）。
    ("Sha256", 0, Layer::Core),
    // 並行処理の前置き型（ADR 0026 の決定 3）。性質の導出は検査器が型名ごとに持つ
    // （`check` の `holds`）。API の部品の登録は PR 4-2c、本 PR（4-2a）は型だけ。
    ("Scope", 0, Layer::Core),
    ("TaskHandle", 1, Layer::Core),
    ("Sender", 1, Layer::Core),
    ("Receiver", 1, Layer::Core),
    ("SyncShared", 1, Layer::Core),
    ("Lock", 1, Layer::Core),
    ("LockGuard", 1, Layer::Core),
    ("LockBroken", 0, Layer::Core),
    ("Atomic", 1, Layer::Core),
    // 複数ルートの入口の前置き型（ADR 0031 決定 4-1、PR 6-3d）。`Scope` と同じく性質を持たない。
    ("RootGroup", 0, Layer::Core),
    ("RootSlot", 0, Layer::Core),
    // TCP のソケット（ADR 0031 決定 3、PR 6-3e）。中身は `std.os.net` の Sigil 実装が持つ（`Secret` と同じ作法）。
    // `TcpListener`・`TcpStream`・`TcpClient` は 1 つの値が 1 つの fd を所有し、破棄で閉じる。複製できず、`Sendable` だが
    // `Shareable` でない（性質は検査器が型名ごとに持つ）。`NetError` は腕を `match` できる列挙で、検査器は
    // この行を列挙として登録し直す（`NET_ERROR_ARMS`）。
    ("TcpListener", 0, Layer::Os),
    ("TcpStream", 0, Layer::Os),
    // ADR 0034 決定 4: 接続側専用の所有型。
    ("TcpClient", 0, Layer::Os),
    ("NetError", 0, Layer::Os),
    // HTTP/1.1 の解釈と型（ADR 0032 決定 2、PR 6-3g-b）。中身は `std.http`・`std.http.limits` の Sigil 実装が持つ（`TcpStream` と
    // 同じ作法）。型の名前は平らな名前空間（ADR 0021）で利用者の型と衝突しないよう `Http` で始め、前置きの名前にする
    // （利用者は宣言できない）。`HttpError`・`HttpProgress` は腕を `match` できる列挙で、検査器はこの行を列挙として登録し直す
    // （`HTTP_ERROR_ARMS`・`HTTP_PROGRESS_ARMS`）。ほかは中身を見せない型で、`Sendable`・`Shareable` だけを持つ。
    ("PgConfig", 0, Layer::Os),
    ("PgConnectLimits", 0, Layer::Os),
    ("PgConn", 0, Layer::Os),
    ("PgError", 0, Layer::Os),
    ("HttpLimits", 0, Layer::Os),
    ("HttpParser", 0, Layer::Os),
    // 接続（ADR 0032 決定 3-2、PR 6-3g-c）。`TcpStream` を持つので `Sendable` だけで、`Shareable` でない。
    ("HttpConn", 0, Layer::Os),
    ("HttpProgress", 0, Layer::Os),
    ("HttpRequest", 0, Layer::Os),
    ("HttpHeader", 0, Layer::Os),
    ("HttpResponse", 0, Layer::Os),
    ("HttpError", 0, Layer::Os),
    // フレームワークの設定（ADR 0033 E）。不変の値で、表示・直接構築は公開しない。
    ("AppConfig", 0, Layer::Os),
    ("AppLimits", 0, Layer::Os),
    ("ConfigError", 0, Layer::Os),
];

/// `std.web` の設定と安全な設定エラー（ADR 0033 E、Issue #460）。
pub const WEB_TYPES: &[&str] = &["AppConfig", "AppLimits", "ConfigError"];
/// PostgreSQL 接続の内部定義と結ぶ公開型（ADR 0034）。
pub const PG_TYPES: &[&str] = &["PgConfig", "PgConnectLimits", "PgConn", "PgError"];
pub const PG_ERROR_ARMS: &[&str] = &[
    "UnsupportedCredential",
    "Authentication",
    "Connection",
    "Timeout",
    "Protocol",
    "LimitExceeded",
    "TypeMismatch",
    "UnsupportedType",
];

/// `std.json` が実体を宣言する、平坦な JSON 表の型（ADR 0033、Issue #455）。
pub const JSON_TYPES: &[&str] = &["JsonValue", "JsonNode", "JsonField"];

/// `NetError` の腕（ADR 0031 決定 3-3、判断点 B）。並びは `std.os.net` の宣言と同じ。
pub const NET_ERROR_ARMS: &[&str] = &["AddrInUse", "PermissionDenied", "Closed", "Other"];
/// `std.os.net` が実体を持つ、fd を所有する型（結合後の名前）。値の破棄で `close(2)` を 1 回呼ぶ
/// （ネイティブは破棄の関数が `NET_CLOSE_FN` を呼び、評価器は値の破棄で閉じる）。
pub const NET_FD_TYPES: &[&str] = &[
    "std.os.net.TcpListener",
    "std.os.net.TcpStream",
    "std.os.net.TcpClient",
];
/// `HttpError` の腕（ADR 0032 決定 2-2）。並びは `std.http` の宣言と同じ（単体テストで固定する）。
pub const HTTP_ERROR_ARMS: &[&str] = &[
    "BadRequest",
    "UriTooLong",
    "HeadersTooLarge",
    "BodyTooLarge",
    "NotImplemented",
    "VersionNotSupported",
    "ExpectationFailed",
    "Timeout",
];
/// `HttpProgress` の腕と中身の型（ADR 0032 決定 2-3）。`Done` は完成した要求と、`feed` に渡したバイト列の中の次の位置。
/// 並びは `std.http` の宣言と同じ（単体テストで固定する）。
pub const HTTP_PROGRESS_ARMS: &[(&str, &[&str])] =
    &[("NeedMore", &[]), ("Done", &["HttpRequest", "Int"])];
/// `std.http`・`std.http.limits` が実体を宣言する前置きの型（表の `Http` で始まる行）。
pub const HTTP_TYPES: &[&str] = &[
    "HttpLimits",
    "HttpParser",
    "HttpConn",
    "HttpProgress",
    "HttpRequest",
    "HttpHeader",
    "HttpResponse",
    "HttpError",
];

pub mod core_sigil;
pub mod embedded;

/// 原始操作の表の効果の列で、起こす作用が引数で決まること（システムコール）を表す語（ADR 0016 の決定 4）。
/// この語の操作を呼ぶ関数に限り、効果の検査は「宣言が推論を含む」になる。
pub const DELEGATED: &str = "宣言に委ねる";

/// 原始操作の表の 1 行（ADR 0016 の決定 1）。名前空間 `raw` に置く。
#[derive(Debug, Clone, Copy)]
pub struct RawRow {
    pub layer: Layer,
    /// `raw.` を除いたシグネチャ（例: `alloc(size: Int, align: Int) -> Addr`）。
    pub signature: &'static str,
    /// `effects` 節の中身か、`DELEGATED`。
    pub effects: &'static str,
    /// 実行時に検査する `requires`（式。ない行は `none`）。
    pub requires: &'static str,
    /// 式で書けない前提の文（日本語。空にできない）。
    pub premise: &'static str,
}

/// 原始操作の表。行を足す・変える・消すときは、先に ADR に記録する（ADR 0016 の決定 1）。
/// 実際の一覧は ADR 0018 決定 6 および ADR 0025 決定 1 に基づく。
pub const RAW_OPS: &[RawRow] = &[
    RawRow {
        layer: Layer::Core,
        signature: "addr_null() -> Addr",
        effects: "none",
        requires: "none",
        premise: "常にヌル番地（0）を返す",
    },
    RawRow {
        layer: Layer::Core,
        signature: "addr_to_int(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地のビットパターンを整数として読む",
    },
    RawRow {
        layer: Layer::Core,
        signature: "int_to_addr(v: Int) -> Addr",
        effects: "none",
        requires: "none",
        premise: "非負の整数を番地として解釈する",
    },
    RawRow {
        layer: Layer::Core,
        signature: "offset(a: Addr, n: Int) -> Addr",
        effects: "none",
        requires: "none",
        premise: "同一チャンクの境界内でのみオフセットを適用する",
    },
    RawRow {
        layer: Layer::Core,
        signature: "load_u8(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、1 バイト読み取り可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "store_u8(a: Addr, v: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、値は 0..=255 である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "load_i64(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、8 バイト境界かつ読み取り可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "store_i64(a: Addr, v: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ書き込み可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "load_addr(a: Addr) -> Addr",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、8 バイト境界かつ読み取り可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "store_addr(a: Addr, v: Addr)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ書き込み可能である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "copy(dst: Addr, src: Addr, n: Int)",
        effects: "none",
        requires: "none",
        premise: "転送元および転送先がそれぞれ n バイトの有効な領域である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "set(dst: Addr, byte: Int, n: Int)",
        effects: "none",
        requires: "none",
        premise: "対象番地から n バイトが有効な書き込み可能領域である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "alloc(size: Int, align: Int) -> Addr",
        effects: "none",
        requires: "none",
        premise: "確保器が初期化済みであり、要求サイズが有効である",
    },
    RawRow {
        layer: Layer::Core,
        signature: "free(a: Addr, size: Int, align: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が alloc で得られた生きた領域の先頭であり、サイズ・アライメントが一致する",
    },
    RawRow {
        layer: Layer::Core,
        signature: "atomic_load_i64(a: Addr) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた確保済み領域内にあり、8 バイト境界かつ読み取り可能である。他のスレッドが同じ番地を並行に操作しうる。メモリ順序は逐次一貫",
    },
    RawRow {
        layer: Layer::Core,
        signature: "atomic_store_i64(a: Addr, v: Int)",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ書き込み可能である。他のスレッドが同じ番地を並行に操作しうる。メモリ順序は逐次一貫",
    },
    RawRow {
        layer: Layer::Core,
        signature: "atomic_add_i64(a: Addr, v: Int) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ読み書き可能である。加える前の値を返す（加算は 2 の補数で巻き戻る）。他のスレッドが同じ番地を並行に操作しうる。メモリ順序は逐次一貫",
    },
    RawRow {
        layer: Layer::Core,
        signature: "atomic_swap_i64(a: Addr, v: Int) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ読み書き可能である。書き込む前の値を返す。他のスレッドが同じ番地を並行に操作しうる。メモリ順序は逐次一貫",
    },
    RawRow {
        layer: Layer::Core,
        signature: "atomic_cas_i64(a: Addr, expected: Int, new: Int) -> Int",
        effects: "none",
        requires: "none",
        premise: "番地が生きた書き込み可能領域内にあり、8 バイト境界かつ読み書き可能である。値が expected と等しければ new を書き、常に操作の前の値を返す（成功は戻りが expected と等しいことで分かる）。他のスレッドが同じ番地を並行に操作しうる。メモリ順序は逐次一貫",
    },
    RawRow {
        layer: Layer::Core,
        signature: "atomic_fence()",
        effects: "none",
        requires: "none",
        premise: "逐次一貫の壁。他のスレッドの操作との順序だけに作用する",
    },
    // fd の準備完了を待つ内部の原始操作（ADR 0031 決定 1-1、PR 6-3c）。`raw.*` の枠（`unchecked` の関数の本体だけ）に
    // 置くので、`trust base` のモジュールからしか呼べない。ADR は `io.wait_fd` と書くが、名前空間を足さずに既存の
    // `raw.*` の制限を再利用するため `raw.io_wait_fd` とする。
    RawRow {
        layer: Layer::Os,
        signature: "io_wait_fd(fd: Int, events: Int) -> Int",
        effects: "task.suspend",
        requires: "none",
        premise: "fd は呼び出し側が所有する開いた記述子で、待っている間は閉じない。events は EPOLLIN(1)・EPOLLOUT(4) の組み合わせ。戻りは準備のできたイベントの印で、負の値はエラー",
    },
    RawRow {
        layer: Layer::Os,
        signature: "syscall(n: Int, a0: Int, a1: Int, a2: Int, a3: Int, a4: Int, a5: Int) -> Int",
        effects: DELEGATED,
        requires: "none",
        premise: "システムコール番号と引数がカーネル ABI に合致し、ポインタ引数は生きた領域を指す",
    },
];

/// 名前空間のない関数。読むだけの操作はメソッドに移したので、`Copyable` な値の関数だけが残る。
pub const FREE_FNS: &[(Layer, &str, &str)] = &[
    (Layer::Core, "to_float(n: Int) -> Float", "none"),
    (Layer::Core, "to_int(x: Float) -> Option<Int>", "none"),
    (Layer::Core, "ct_eq(a: &Bytes, b: &Bytes) -> Bool", "none"),
    // SHA-256 ワンショット（ADR 0024 決定 1、PR 6-1c）。実体は `std.crypto`
    // モジュールの Sigil 実装（埋め込み）。`ct_eq` と違い組み込みではないため、
    // lower が名前で実体へ結ぶ。
    (Layer::Core, "sha256(data: &Bytes) -> Bytes", "none"),
    // HMAC-SHA-256 と照合、PBKDF2（ADR 0024 決定 1、PR 6-1d）。実体は
    // `sha256` と同じく `std.crypto` モジュールの Sigil 実装（埋め込み）。
    // `pbkdf2_sha256` の効果は `task.suspend`: 反復ごとの `task.yield`
    // （ADR 0024 決定 2）は、効果 `task.suspend` の対象に bare が含まれて
    // いなかったため 6-1d では入れられなかったが、ADR 0028 で bare が
    // 許可され、宣言できるようになった（crypto.sigil の注記）。
    (
        Layer::Core,
        "hmac_sha256(key: &Secret, message: &Bytes) -> Secret",
        "none",
    ),
    (
        Layer::Core,
        "verify_hmac_sha256(key: &Secret, message: &Bytes, expected: &Bytes) -> Bool",
        "none",
    ),
    (
        Layer::Core,
        "pbkdf2_sha256(password: &Secret, salt: &Bytes, iterations: Int, dk_len: Int) -> Secret",
        "task.suspend",
    ),
];

/// 名前空間の関数（名前空間, シグネチャ, 効果）。読むだけの引数は `&T` で受ける。
pub const NAMESPACED_FNS: &[(Layer, &str, &str, &str)] = &[
    (Layer::Os, "web", "load_config() -> Result<AppConfig, ConfigError>", "env"),
    (Layer::Os, "web", "error_response(status: Int) -> HttpResponse", "none"),
    (Layer::Core, "json", "parse(data: &Bytes) -> Result<JsonValue, Int>", "none"),
    (Layer::Core, "json", "stringify(value: &JsonValue) -> Result<Bytes, Int>", "none"),
    (
        Layer::Os,
        "console",
        "log<T: Displayable>(x: &T)",
        "console",
    ),
    (
        Layer::Os,
        "env",
        "get(name: &String) -> Option<String>",
        "env",
    ),
    (
        Layer::Os,
        "fs",
        "read(p: &Path) -> Result<String, IoError>",
        "fs.read",
    ),
    (
        Layer::Os,
        "fs",
        "write(p: &Path, s: &String) -> Result<Unit, IoError>",
        "fs.write",
    ),
    (
        Layer::Os,
        "fs",
        "read_bytes(p: &Path, max: Int) -> Result<Bytes, BinaryIoError>",
        "fs.read",
    ),
    (
        Layer::Os,
        "fs",
        "write_bytes(p: &Path, data: &Bytes, max: Int) -> Result<Unit, BinaryIoError>",
        "fs.write",
    ),
    // TCP の待ち受け（ADR 0031 決定 3-2、PR 6-3e）。実体は `std.os.net.listen`（lower が名前で結ぶ）。
    (
        Layer::Os,
        "net",
        "listen(host: &String, port: Int, backlog: Int) -> Result<TcpListener, NetError>",
        "net.server",
    ),
    // ADR 0034 決定 4: 実効果を接続側と乱数で分ける。既存サーバー側 API の意味は変えない。
    (Layer::Os, "net", "connect(host: &String, port: Int) -> Result<TcpClient, NetError>", "net.client, task.suspend"),
    (Layer::Os, "random", "bytes(len: Int) -> Option<Bytes>", "random"),
    (Layer::Os, "pg", "config(host: String, port: Int, user: String, database: String, password: Secret) -> Result<PgConfig, PgError>", "none"),
    (Layer::Os, "pg", "default_connect_limits() -> PgConnectLimits", "none"),
    (Layer::Os, "pg", "connect(config: PgConfig, limits: &PgConnectLimits) -> Result<PgConn, PgError>", "net.client, random, clock, task.spawn, task.suspend"),
    // HTTP の上限の既定値と増分の解釈器（ADR 0032 決定 2-3・4-3、PR 6-3g-b）。実体は `std.http`（lower が名前で結ぶ）。
    // I/O をしない（効果なし）。
    (Layer::Os, "http", "default_limits() -> HttpLimits", "none"),
    (
        Layer::Os,
        "http",
        "parser(limits: &HttpLimits) -> HttpParser",
        "none",
    ),
    // 接続（ADR 0032 決定 2-3・3-2、PR 6-3g-c）。`serve_conn` の効果はハンドラの効果を足す（`of(h)`）。
    (Layer::Os, "http", "conn(s: TcpStream) -> HttpConn", "none"),
    (
        Layer::Os,
        "http",
        "serve_conn(c: &HttpConn, limits: &HttpLimits, h: fn(&HttpRequest) -> HttpResponse) -> Unit",
        "net.server, clock, task.spawn, task.suspend, task.share, of(h)",
    ),
    (
        Layer::Os,
        "http",
        "abort_conn(c: HttpConn, limits: &HttpLimits) -> Unit",
        "net.server, clock, task.spawn, task.suspend, task.share",
    ),
];

/// シグネチャ（`Entry` の最初の要素）に対応する `requires` 節。ない関数は `none`。
pub const REQUIRES: &[(&str, &str)] = &[
    (
        "connect(host: &String, port: Int) -> Result<TcpClient, NetError>",
        "1 <= port && port <= 65535",
    ),
    (
        "read(self: &mut TcpClient, max: Int) -> Result<Bytes, NetError>",
        "1 <= max && max <= 65536",
    ),
    (
        "bytes(len: Int) -> Option<Bytes>",
        "1 <= len && len <= 65536",
    ),
    // ソケットの契約（ADR 0031 決定 3-2）。1 回の読みの大きさの上限を、API の側で破れなくする（ADR 0020 決定 4）。
    (
        "listen(host: &String, port: Int, backlog: Int) -> Result<TcpListener, NetError>",
        "0 <= port && port <= 65535 && 1 <= backlog && backlog <= 1024",
    ),
    (
        "read(self: &TcpStream, max: Int) -> Result<Bytes, NetError>",
        "1 <= max && max <= 65536",
    ),
    // HTTP（ADR 0032 決定 2-3・4-3）。上限の上限は `std.http.limits` の `requires` と同じ（単体テストで固定する）。
    (
        "feed(self: &mut HttpParser, data: &Bytes, start: Int) -> Result<HttpProgress, HttpError>",
        "0 <= start && start <= data.len()",
    ),
    (
        "new(status: Int, body: Bytes) -> HttpResponse",
        "200 <= status && status <= 599",
    ),
    (
        "with_request_line(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 65536",
    ),
    (
        "with_head(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 262144",
    ),
    (
        "with_headers(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 1000",
    ),
    (
        "with_body(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 67108864",
    ),
    (
        "with_chunk_line(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 1024",
    ),
    (
        "with_encoded_body(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 134217728",
    ),
    (
        "with_conns(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 4096",
    ),
    (
        "with_backlog(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 1024",
    ),
    (
        "with_head_ms(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 600000",
    ),
    (
        "with_body_ms(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 600000",
    ),
    (
        "with_handler_ms(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 600000",
    ),
    (
        "with_send_ms(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 600000",
    ),
    (
        "with_idle_ms(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 600000",
    ),
    (
        "with_requests(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 100000",
    ),
    (
        "with_response_body(self: HttpLimits, v: Int) -> HttpLimits",
        "1 <= v && v <= 67108864",
    ),
    ("min(self: &List<T>) -> &T", "self.len() > 0"),
    ("max(self: &List<T>) -> &T", "self.len() > 0"),
    // 暗号の部品の契約（ADR 0024 決定 2、PR 6-1d）。短すぎる鍵と、
    // 少なすぎる・多すぎる反復回数を、呼び出し側の責任として弾く。
    (
        "hmac_sha256(key: &Secret, message: &Bytes) -> Secret",
        "key.len() >= 32",
    ),
    (
        "verify_hmac_sha256(key: &Secret, message: &Bytes, expected: &Bytes) -> Bool",
        "key.len() >= 32",
    ),
    (
        "pbkdf2_sha256(password: &Secret, salt: &Bytes, iterations: Int, dk_len: Int) -> Secret",
        "iterations >= 4096 && iterations <= 100000 && dk_len > 0",
    ),
];

/// `impl` の見出し（例: `<T> List<T>`）と、その中の関数。
pub const IMPLS: &[(Layer, &str, &[Entry])] = &[
    (Layer::Os, "AppConfig", &[
        ("host(self: &AppConfig) -> String", "none"),
        ("port(self: &AppConfig) -> Int", "none"),
        ("http_limits(self: &AppConfig) -> HttpLimits", "none"),
        ("app_limits(self: &AppConfig) -> AppLimits", "none"),
        ("log_memory_bound(self: &AppConfig)", "console"),
    ]),
    (Layer::Os, "AppLimits", &[
        ("query_params_max(self: &AppLimits) -> Int", "none"),
        ("json_max_depth(self: &AppLimits) -> Int", "none"),
        ("shutdown_grace_ms(self: &AppLimits) -> Int", "none"),
    ]),
    (Layer::Os, "ConfigError", &[
        ("name(self: &ConfigError) -> String", "none"),
    ]),

    (Layer::Core, "JsonValue", &[
        ("new(nodes: List<JsonNode>, root: Int) -> JsonValue", "none"),
        ("nodes(self: &JsonValue) -> List<JsonNode>", "none"),
        ("root(self: &JsonValue) -> Int", "none"),
    ]),
    (Layer::Core, "JsonField", &[
        ("new(name: String, index: Int) -> JsonField", "none"),
        ("name(self: &JsonField) -> String", "none"),
        ("index(self: &JsonField) -> Int", "none"),
    ]),
    (
        Layer::Core,
        "Secret",
        &[
            ("from_bytes(b: Bytes) -> Secret", "none"),
            ("expose(self: &Secret) -> Bytes", "none"),
            ("len(self: &Secret) -> Int", "none"),
        ],
    ),
    (
        Layer::Core,
        "Sha256",
        &[
            ("new() -> Sha256", "none"),
            ("update(self: &mut Sha256, data: &Bytes)", "none"),
            ("finalize(self: &mut Sha256) -> Bytes", "none"),
        ],
    ),
    (
        Layer::Core,
        "<T> List<T>",
        &[
            ("new() -> List<T>", "none"),
            ("push(self: &mut List<T>, x: T)", "none"),
            ("get(self: &List<T>, i: Int) -> Option<&T>", "none"),
            (
                "get_mut(self: &mut List<T>, i: Int) -> Option<&mut T>",
                "none",
            ),
            ("len(self: &List<T>) -> Int", "none"),
            ("is_empty(self: &List<T>) -> Bool", "none"),
            ("map<U>(self: &List<T>, f: fn(T) -> U) -> List<U>", "of(f)"),
            (
                "find(self: &List<T>, pred: fn(&T) -> Bool) -> Option<&T>",
                "of(pred)",
            ),
            (
                "max_by<K: Comparable>(self: &List<T>, key: fn(&T) -> K) -> Option<&T>",
                "of(key)",
            ),
            (
                "min_by<K: Comparable>(self: &List<T>, key: fn(&T) -> K) -> Option<&T>",
                "of(key)",
            ),
        ],
    ),
    (
        Layer::Core,
        "<T: Numeric> List<T>",
        &[("sum(self: &List<T>) -> T", "none")],
    ),
    (
        Layer::Core,
        "<T: Comparable> List<T>",
        &[
            ("min(self: &List<T>) -> &T", "none"),
            ("max(self: &List<T>) -> &T", "none"),
        ],
    ),
    (
        Layer::Core,
        "Int",
        &[
            ("min_value() -> Int", "none"),
            ("rotl(self: Int, n: Int, width: Int) -> Int", "none"),
            ("rotr(self: Int, n: Int, width: Int) -> Int", "none"),
            ("rotr32(self: Int, n: Int) -> Int", "none"),
            ("ashr(self: Int, n: Int) -> Int", "none"),
        ],
    ),
    (
        Layer::Core,
        "Float",
        &[
            // IEEE 754 binary64 のビット列を `Int` の 64 ビットとして読み替える（ADR 0025 補足 C-3）。
            ("to_bits(x: Float) -> Int", "none"),
            ("from_bits(n: Int) -> Float", "none"),
        ],
    ),
    (
        Layer::Core,
        "<T> Option<T>",
        &[
            ("is_some(self: &Option<T>) -> Bool", "none"),
            ("is_none(self: &Option<T>) -> Bool", "none"),
            ("unwrap_or(self: Option<T>, default: T) -> T", "none"),
            (
                "map<U>(self: Option<T>, f: fn(T) -> U) -> Option<U>",
                "of(f)",
            ),
            (
                "ok_or<E>(self: Option<T>, error: E) -> Result<T, E>",
                "none",
            ),
        ],
    ),
    (
        Layer::Core,
        "<T, E> Result<T, E>",
        &[
            ("is_ok(self: &Result<T, E>) -> Bool", "none"),
            ("is_err(self: &Result<T, E>) -> Bool", "none"),
            ("unwrap_or(self: Result<T, E>, default: T) -> T", "none"),
            ("ok(self: Result<T, E>) -> Option<T>", "none"),
            (
                "map<U>(self: Result<T, E>, f: fn(T) -> U) -> Result<U, E>",
                "of(f)",
            ),
            (
                "map_err<F>(self: Result<T, E>, f: fn(E) -> F) -> Result<T, F>",
                "of(f)",
            ),
        ],
    ),
    (
        Layer::Core,
        "String",
        &[
            ("len(self: &String) -> Int", "none"),
            ("is_empty(self: &String) -> Bool", "none"),
            ("trim(self: &String) -> String", "none"),
            ("lines(self: &String) -> List<String>", "none"),
            (
                "parse_int(self: &String) -> Result<Int, ParseError>",
                "none",
            ),
            ("as_bytes(self: &String) -> Bytes", "none"),
        ],
    ),
    (
        Layer::Core,
        "Bytes",
        &[
            ("new() -> Bytes", "none"),
            ("with_capacity(capacity: Int) -> Bytes", "none"),
            (
                "from_list(bytes: &List<Int>) -> Result<Bytes, String>",
                "none",
            ),
            ("len(self: &Bytes) -> Int", "none"),
            ("is_empty(self: &Bytes) -> Bool", "none"),
            ("get(self: &Bytes, index: Int) -> Option<Int>", "none"),
            (
                "set(self: &mut Bytes, index: Int, byte: Int) -> Result<Unit, String>",
                "none",
            ),
            ("push(self: &mut Bytes, byte: Int)", "none"),
            (
                "slice(self: &Bytes, start: Int, end: Int) -> Result<Bytes, String>",
                "none",
            ),
            ("concat(self: &Bytes, other: &Bytes) -> Bytes", "none"),
            ("extend(self: &mut Bytes, other: &Bytes)", "none"),
            (
                "to_string_utf8(self: &Bytes) -> Result<String, String>",
                "none",
            ),
            ("push_u16_be(self: &mut Bytes, value: Int)", "none"),
            ("push_u32_be(self: &mut Bytes, value: Int)", "none"),
            ("push_u64_be(self: &mut Bytes, value: Int)", "none"),
            ("push_u32_le(self: &mut Bytes, value: Int)", "none"),
            (
                "read_u32_be(self: &Bytes, offset: Int) -> Option<Int>",
                "none",
            ),
        ],
    ),
    (
        Layer::Os,
        "Path",
        &[
            ("new(s: &String) -> Path", "none"),
            ("is_empty(self: &Path) -> Bool", "none"),
            ("join(self: &Path, name: &String) -> Path", "none"),
            ("file_name(self: &Path) -> Option<String>", "none"),
            ("parent(self: &Path) -> Option<Path>", "none"),
        ],
    ),
    // TCP のソケットの操作（ADR 0031 決定 3-2、PR 6-3e）。実体は `std.os.net` の `impl`。
    (
        Layer::Os,
        "TcpListener",
        &[
            ("local_port(self: &TcpListener) -> Int", "net.server"),
            (
                "accept(self: &TcpListener) -> Result<TcpStream, NetError>",
                "net.server, task.suspend",
            ),
        ],
    ),
    (
        Layer::Os,
        "TcpStream",
        &[
            (
                "read(self: &TcpStream, max: Int) -> Result<Bytes, NetError>",
                "net.server, task.suspend",
            ),
            (
                "write_all(self: &TcpStream, data: &Bytes) -> Result<Unit, NetError>",
                "net.server, task.suspend",
            ),
            (
                "shutdown(self: &TcpStream) -> Result<Unit, NetError>",
                "net.server",
            ),
        ],
    ),
    (Layer::Os, "PgConnectLimits", &[("with_timeout(self: PgConnectLimits, ms: Int) -> Result<PgConnectLimits, PgError>", "none")]),
    (Layer::Os, "TcpClient", &[
        ("read(self: &mut TcpClient, max: Int) -> Result<Bytes, NetError>", "net.client, task.suspend"),
        ("write_all(self: &mut TcpClient, data: &Bytes) -> Result<Unit, NetError>", "net.client, task.suspend"),
    ]),
    // HTTP の型（ADR 0032 決定 2-2・2-3、PR 6-3g-b）。実体は `std.http`・`std.http.limits` の `impl`。取り出す関数は写しを返す。
    (
        Layer::Os,
        "HttpLimits",
        &[
            ("request_line(self: &HttpLimits) -> Int", "none"),
            (
                "with_request_line(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("head(self: &HttpLimits) -> Int", "none"),
            (
                "with_head(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("headers(self: &HttpLimits) -> Int", "none"),
            (
                "with_headers(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("body(self: &HttpLimits) -> Int", "none"),
            (
                "with_body(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("chunk_line(self: &HttpLimits) -> Int", "none"),
            (
                "with_chunk_line(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("encoded_body(self: &HttpLimits) -> Int", "none"),
            (
                "with_encoded_body(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("conns(self: &HttpLimits) -> Int", "none"),
            (
                "with_conns(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("backlog(self: &HttpLimits) -> Int", "none"),
            (
                "with_backlog(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("head_ms(self: &HttpLimits) -> Int", "none"),
            (
                "with_head_ms(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("body_ms(self: &HttpLimits) -> Int", "none"),
            (
                "with_body_ms(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("handler_ms(self: &HttpLimits) -> Int", "none"),
            (
                "with_handler_ms(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("send_ms(self: &HttpLimits) -> Int", "none"),
            (
                "with_send_ms(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("idle_ms(self: &HttpLimits) -> Int", "none"),
            (
                "with_idle_ms(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("requests(self: &HttpLimits) -> Int", "none"),
            (
                "with_requests(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("response_body(self: &HttpLimits) -> Int", "none"),
            (
                "with_response_body(self: HttpLimits, v: Int) -> HttpLimits",
                "none",
            ),
            ("memory_bound(self: &HttpLimits) -> Int", "none"),
        ],
    ),
    (
        Layer::Os,
        "HttpParser",
        &[
            (
                "feed(self: &mut HttpParser, data: &Bytes, start: Int) -> Result<HttpProgress, HttpError>",
                "none",
            ),
            ("consumed(self: &HttpParser) -> Int", "none"),
            ("held(self: &HttpParser) -> Int", "none"),
            ("in_body(self: &HttpParser) -> Bool", "none"),
            ("wants_continue(self: &HttpParser) -> Bool", "none"),
        ],
    ),
    (
        Layer::Os,
        "HttpRequest",
        &[
            ("method(self: &HttpRequest) -> String", "none"),
            ("path(self: &HttpRequest) -> Bytes", "none"),
            ("query(self: &HttpRequest) -> Bytes", "none"),
            ("version(self: &HttpRequest) -> Int", "none"),
            ("headers(self: &HttpRequest) -> List<HttpHeader>", "none"),
            (
                "header(self: &HttpRequest, name: &String) -> Option<Bytes>",
                "none",
            ),
            ("body(self: &HttpRequest) -> Bytes", "none"),
            ("body_len(self: &HttpRequest) -> Int", "none"),
        ],
    ),
    (
        Layer::Os,
        "HttpHeader",
        &[
            ("name(self: &HttpHeader) -> String", "none"),
            ("value(self: &HttpHeader) -> Bytes", "none"),
        ],
    ),
    (
        Layer::Os,
        "HttpResponse",
        &[
            ("new(status: Int, body: Bytes) -> HttpResponse", "none"),
            (
                "with_header(self: HttpResponse, name: &String, value: &Bytes) -> HttpResponse",
                "none",
            ),
            ("status(self: &HttpResponse) -> Int", "none"),
            ("headers(self: &HttpResponse) -> List<HttpHeader>", "none"),
            ("body(self: &HttpResponse) -> Bytes", "none"),
        ],
    ),
    (
        Layer::Os,
        "HttpError",
        &[("status(self: &HttpError) -> Int", "none")],
    ),
    (
        Layer::Core,
        "<T> Cell<T>",
        &[("new(value: T) -> Cell<T>", "none")],
    ),
    (
        Layer::Core,
        "<T> Shared<T>",
        &[("new(value: T) -> Shared<T>", "none")],
    ),
    (
        Layer::Core,
        "<T: Copyable> Shared<Cell<T>>",
        &[("get(self: &Shared<Cell<T>>) -> T", "state")],
    ),
    (
        Layer::Core,
        "<T> Shared<Cell<T>>",
        &[("set(self: &Shared<Cell<T>>, value: T)", "state")],
    ),
];

fn clauses(sig: &str, effects: &str) -> String {
    let requires = REQUIRES
        .iter()
        .find(|(s, _)| *s == sig)
        .map_or("none", |(_, r)| r);
    clauses_with(requires, effects)
}

fn clauses_with(requires: &str, effects: &str) -> String {
    format!(
        "\n  intent \"s\"\n  requires {requires}\n  ensures none\n  effects {effects}\n{{\n}}\n"
    )
}

fn module_head(layer: Layer) -> &'static str {
    match layer {
        Layer::Core => "module std.core\n  targets bare, browser, server\n  trust base\n\n",
        Layer::Os => "module std.os\n  targets browser, server\n  trust base\n\n",
    }
}

/// 原始操作の 1 行を、型検査器が読む `fn` のソースにする（効果の `DELEGATED` は `none` として読む）。
pub fn raw_source(layer: Layer, sig: &str, effects: &str, requires: &str) -> String {
    let effects = if effects == DELEGATED {
        "none"
    } else {
        effects
    };
    format!(
        "{}fn {sig}{}",
        module_head(layer),
        clauses_with(requires, effects)
    )
}

/// 型検査器が読む `Sigil` のソース。自由関数と名前空間の関数は `fn` の並び、メソッドは `impl` の並び。
pub fn free_source(layer: Layer, sig: &str, effects: &str) -> String {
    format!("{}fn {sig}{}", module_head(layer), clauses(sig, effects))
}

pub fn impl_source(layer: Layer, head: &str, fns: &[Entry]) -> String {
    let mut s = String::from(module_head(layer));
    s.push_str(&format!("impl{} {{\n", head_with_space(head)));
    for (sig, effects) in fns {
        s.push_str(&format!("fn {sig}{}", clauses(sig, effects)));
    }
    s.push_str("}\n");
    s
}

fn head_with_space(head: &str) -> String {
    // `<T> List<T>` は `impl<T> List<T>`、`String` は `impl String`。
    if head.starts_with('<') {
        head.to_string()
    } else {
        format!(" {head}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::EffectTable;
    use std::collections::BTreeSet;

    /// 型名・`ns.name`・`Type.name` の形で名前を集める。
    fn names(layer: Layer) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let fn_name = |sig: &str| sig.split(['(', '<']).next().unwrap().to_string();
        for (n, _, l) in OPAQUE_TYPES {
            if *l == layer {
                out.insert((*n).to_string());
            }
        }
        for (l, sig, _) in FREE_FNS {
            if *l == layer {
                out.insert(fn_name(sig));
            }
        }
        for (l, ns, sig, _) in NAMESPACED_FNS {
            if *l == layer {
                out.insert(format!("{ns}.{}", fn_name(sig)));
            }
        }
        for (l, head, fns) in IMPLS {
            if *l == layer {
                let ty = head.rsplit('>').next().unwrap().trim();
                let ty = ty.split('<').next().unwrap();
                for (sig, _) in *fns {
                    out.insert(format!("{ty}.{}", fn_name(sig)));
                }
            }
        }
        out
    }

    #[test]
    fn every_raw_op_has_premise() {
        // 本 ADR（0016）の時点では `RAW_OPS` は空。テスト用の差し込み（`Options::raw_ops`）の行は、
        // 検査器が登録するときに同じ規則で調べる（tests/trust_base.rs の `raw_op_without_premise_rejected`）。
        for r in RAW_OPS {
            assert!(!r.premise.trim().is_empty(), "{}", r.signature);
        }
    }

    #[test]
    fn std_modules_always_allowed() {
        // 標準ライブラリの仮のモジュール（`std.core`・`std.os`。どちらも `trust base`）は処理系が内部で読むので、
        // 許可（`trust_base`）が空でも、利用者のファイルの検査の前にそれを登録して通る。
        let opts = crate::effects::Options::default();
        assert!(opts.trust_base.is_empty());
        let src = "module m.a\n  targets server\n  trust none\n\nfn f() -> Int\n  intent \"x\"\n  requires none\n  ensures none\n  effects none\n{\n  return 1;\n}\n";
        crate::check::check_with(src, &opts).unwrap();
        for layer in [Layer::Core, Layer::Os] {
            let f = crate::parser::parse_std(&free_source(layer, "f() -> Int", "none")).unwrap();
            assert_eq!(f.module.trust, crate::ast::Trust::Base);
        }
    }

    #[test]
    fn os_layer_is_exactly_the_adr_list() {
        // ADR 0014 決定 2 の OS 層。行を層の間で移すには ADR を改める。
        let want: BTreeSet<String> = [
            "console.log",
            "env.get",
            "fs.read",
            "fs.write",
            "fs.read_bytes",
            "fs.write_bytes",
            "Path",
            "Path.new",
            "Path.is_empty",
            "Path.join",
            "Path.file_name",
            "Path.parent",
            "IoError",
            "BinaryIoError",
            // ADR 0031 決定 3（PR 6-3e）。
            "net.listen",
            "TcpListener",
            "TcpListener.local_port",
            "TcpListener.accept",
            "TcpStream",
            "TcpStream.read",
            "TcpStream.write_all",
            "TcpStream.shutdown",
            "NetError",
            // ADR 0034 決定 4。
            "net.connect",
            "TcpClient",
            "TcpClient.read",
            "TcpClient.write_all",
            "random.bytes",
            // ADR 0033 E・I・J（Issue #460）。
            "web.load_config",
            "web.error_response",
            "AppConfig",
            "AppConfig.host",
            "AppConfig.port",
            "AppConfig.http_limits",
            "AppConfig.app_limits",
            "AppConfig.log_memory_bound",
            "AppLimits",
            "AppLimits.query_params_max",
            "AppLimits.json_max_depth",
            "AppLimits.shutdown_grace_ms",
            "ConfigError",
            "ConfigError.name",
            "pg.config",
            "pg.default_connect_limits",
            "pg.connect",
            "PgConfig",
            "PgConnectLimits",
            "PgConnectLimits.with_timeout",
            "PgConn",
            "PgError",
            // ADR 0032 決定 2（PR 6-3g-b）。
            "http.default_limits",
            "http.parser",
            "HttpLimits",
            "HttpLimits.request_line",
            "HttpLimits.with_request_line",
            "HttpLimits.head",
            "HttpLimits.with_head",
            "HttpLimits.headers",
            "HttpLimits.with_headers",
            "HttpLimits.body",
            "HttpLimits.with_body",
            "HttpLimits.chunk_line",
            "HttpLimits.with_chunk_line",
            "HttpLimits.encoded_body",
            "HttpLimits.with_encoded_body",
            "HttpLimits.conns",
            "HttpLimits.with_conns",
            "HttpLimits.backlog",
            "HttpLimits.with_backlog",
            "HttpLimits.head_ms",
            "HttpLimits.with_head_ms",
            "HttpLimits.body_ms",
            "HttpLimits.with_body_ms",
            "HttpLimits.handler_ms",
            "HttpLimits.with_handler_ms",
            "HttpLimits.send_ms",
            "HttpLimits.with_send_ms",
            "HttpLimits.idle_ms",
            "HttpLimits.with_idle_ms",
            "HttpLimits.requests",
            "HttpLimits.with_requests",
            "HttpLimits.response_body",
            "HttpLimits.with_response_body",
            "HttpParser",
            "HttpConn",
            "HttpLimits.memory_bound",
            "http.conn",
            "http.serve_conn",
            "http.abort_conn",
            "HttpParser.feed",
            "HttpParser.consumed",
            "HttpParser.held",
            "HttpParser.in_body",
            "HttpParser.wants_continue",
            "HttpProgress",
            "HttpRequest",
            "HttpRequest.method",
            "HttpRequest.path",
            "HttpRequest.query",
            "HttpRequest.version",
            "HttpRequest.headers",
            "HttpRequest.header",
            "HttpRequest.body",
            "HttpRequest.body_len",
            "HttpHeader",
            "HttpHeader.name",
            "HttpHeader.value",
            "HttpResponse",
            "HttpResponse.new",
            "HttpResponse.with_header",
            "HttpResponse.status",
            "HttpResponse.headers",
            "HttpResponse.body",
            "HttpError",
            "HttpError.status",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(names(Layer::Os), want);
    }

    /// `std.http`・`std.http.limits` のソース（ADR 0032 決定 2-1・6）。
    fn http_sources() -> Vec<(&'static str, &'static str)> {
        use super::embedded::{lookup, HTTP_LIMITS_MODULE, HTTP_MODULE};
        [HTTP_MODULE, HTTP_LIMITS_MODULE]
            .iter()
            .map(|m| (*m, lookup(m, &[]).expect("登録されている")))
            .collect()
    }

    #[test]
    fn http_is_trust_none_and_has_no_fs_effects() {
        // 信頼基盤の増分は 0（`trust none`、`unchecked`・`raw.*` なし）。ファイルの効果（`fs.read`・`fs.write`）を持たない
        // （ディレクトリの横断を持ち込まない。ADR 0032 決定 6）。接続（6-3g-c）の関数が持てる効果は、ソケットと時間と
        // タスクの効果だけ（`net.server`・`clock`・`task.*`・`of(h)`）。解釈器と型の関数は I/O をしない（`none`）。
        let allowed = [
            "none",
            "net.server",
            "clock",
            "task.spawn",
            "task.suspend",
            "task.share",
        ];
        let check_effects = |m: &str, eff: &str| {
            for e in eff.split(',').map(str::trim) {
                assert!(
                    allowed.contains(&e) || e.starts_with("of("),
                    "{m}: 許していない効果 {e}"
                );
            }
        };
        for (m, src) in http_sources() {
            let f = crate::parser::parse(src).unwrap();
            assert_eq!(f.module.name.join("."), m);
            assert_eq!(f.module.trust, crate::ast::Trust::None, "{m}");
            for line in src.lines().map(str::trim).filter(|l| !l.starts_with("//")) {
                assert!(!line.starts_with("unchecked"), "{m}: {line}");
                assert!(!line.contains("raw."), "{m}: {line}");
                assert!(!line.contains("fs."), "{m}: {line}");
                if let Some(eff) = line.strip_prefix("effects ") {
                    check_effects(m, eff);
                }
            }
        }
        // 表の行（名前空間 `http` と `Http` で始まる型のメソッド）の効果。`fs.*` を持たない。
        for (_, ns, sig, eff) in NAMESPACED_FNS {
            if *ns == "http" {
                check_effects(sig, eff);
                let io = !sig.starts_with("default_limits") && !sig.starts_with("parser");
                assert_eq!(*eff != "none", io && !sig.starts_with("conn("), "{sig}");
            }
        }
        for (_, head, fns) in IMPLS {
            if HTTP_TYPES.contains(head) {
                for (sig, eff) in *fns {
                    assert_eq!(*eff, "none", "{head}.{sig}");
                }
            }
        }
    }

    #[test]
    fn http_limits_requires_match_the_embedded_source() {
        // 上限の上限は `std.http.limits` の `requires` にあり、表の `requires` は同じ文字列（ADR 0032 決定 4-3）。
        let (_, src) = http_sources()[1];
        let mut n = 0;
        for (head, fns) in IMPLS
            .iter()
            .filter(|(_, h, _)| *h == "HttpLimits")
            .map(|(_, h, f)| (h, f))
        {
            for (sig, _) in fns.iter() {
                let name = sig.split('(').next().unwrap();
                let at = src
                    .find(&format!("pub fn {name}("))
                    .unwrap_or_else(|| panic!("{head}.{name} の実体が無い"));
                let req = src[at..]
                    .lines()
                    .find_map(|l| l.trim().strip_prefix("requires "))
                    .unwrap();
                let want = REQUIRES
                    .iter()
                    .find(|(s, _)| s == sig)
                    .map_or("none", |(_, r)| r);
                assert_eq!(req, want, "{name}");
                if name.starts_with("with_") {
                    assert!(req.starts_with("1 <= v && v <= "), "{name}: {req}");
                    n += 1;
                }
            }
        }
        assert_eq!(n, 15);
    }

    #[test]
    fn http_enum_arms_match_the_embedded_declaration() {
        // 検査器が登録する腕の並び（`HTTP_ERROR_ARMS`・`HTTP_PROGRESS_ARMS`）は、`std.http` の宣言と同じ。
        let (_, src) = http_sources()[0];
        let arms = |name: &str| -> Vec<String> {
            src.split(&format!("pub enum {name} {{"))
                .nth(1)
                .unwrap()
                .split('}')
                .next()
                .unwrap()
                .split(",\n")
                .map(|s| s.trim().trim_end_matches(',').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        };
        assert_eq!(arms("HttpError"), HTTP_ERROR_ARMS);
        let progress: Vec<String> = HTTP_PROGRESS_ARMS
            .iter()
            .map(|(a, ts)| {
                if ts.is_empty() {
                    a.to_string()
                } else {
                    format!("{a}({})", ts.join(", "))
                }
            })
            .collect();
        assert_eq!(arms("HttpProgress"), progress);
        // 前置きの型の一覧（検査器）と表の行が同じ。
        for t in HTTP_TYPES {
            assert!(
                OPAQUE_TYPES
                    .iter()
                    .any(|(n, _, l)| n == t && *l == Layer::Os),
                "{t}"
            );
        }
    }

    fn mentions_os_type(sig: &str) -> bool {
        sig.split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|w| {
                matches!(
                    w,
                    "AppConfig"
                        | "AppLimits"
                        | "ConfigError"
                        | "Path"
                        | "IoError"
                        | "BinaryIoError"
                        | "TcpListener"
                        | "TcpStream"
                        | "TcpClient"
                        | "NetError"
                        | "PgConfig"
                        | "PgConnectLimits"
                        | "PgConn"
                        | "PgError"
                        | "HttpLimits"
                        | "HttpParser"
                        | "HttpProgress"
                        | "HttpRequest"
                        | "HttpHeader"
                        | "HttpResponse"
                        | "HttpError"
                )
            })
    }

    #[test]
    fn core_rows_do_not_mention_os_types() {
        for (l, sig, _) in FREE_FNS {
            assert!(*l != Layer::Core || !mentions_os_type(sig), "{sig}");
        }
        for (l, _, sig, _) in NAMESPACED_FNS {
            assert!(*l != Layer::Core || !mentions_os_type(sig), "{sig}");
        }
        for (l, head, fns) in IMPLS {
            if *l == Layer::Core {
                assert!(!mentions_os_type(head), "{head}");
                for (sig, _) in *fns {
                    assert!(!mentions_os_type(sig), "{sig}");
                }
            }
        }
    }

    #[test]
    fn core_rows_effects_allowed_on_bare() {
        let table = EffectTable::standard();
        let mut effects: Vec<&str> = vec![];
        effects.extend(FREE_FNS.iter().filter(|r| r.0 == Layer::Core).map(|r| r.2));
        effects.extend(
            NAMESPACED_FNS
                .iter()
                .filter(|r| r.0 == Layer::Core)
                .map(|r| r.3),
        );
        for (l, _, fns) in IMPLS {
            if *l == Layer::Core {
                effects.extend(fns.iter().map(|(_, e)| *e));
            }
        }
        for e in effects {
            if e == "none" || e.starts_with("of(") {
                continue;
            }
            for atom in e.split(',').map(str::trim) {
                assert!(table.allowed_on(atom, "bare"), "{atom}");
            }
        }
    }
}

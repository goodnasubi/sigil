//! PR 4-2a（ADR 0026）: 性質の導出の固定・並行処理の前置き型の登録・`spawn` の検査。
//!
//! - ADR 0007 決定 2 の表の各型について、`Sendable`・`Shareable` の導出結果を固定する。
//! - ADR 0026 決定 3 の前置き型（`Scope`・`TaskHandle`・`Sender`・`Receiver`・
//!   `SyncShared`・`Lock`・`LockGuard`・`LockBroken`・`Atomic`）の導出を固定する。
//! - `Scope.spawn` に渡すクロージャの検査（借用捕獲・非 `Sendable` の捕獲・
//!   非 `Sendable` の戻り値）を固定する。
//!
//! 性質の固定は、総称関数の制約（`need<T: Sendable>(a: T)`）に対象の型の値を
//! 通して行う（tests/check.rs の `derived_properties_follow_the_adr_table` と同じ流儀）。
//! なお ADR 0007 の表の `Map` の行は、検査器の単体チェックでは `Map` 型そのものが
//! 未登録（`UnknownType` になる）ため、本ファイルの対象から外している。

use sigil::check::check;
use sigil::error::ErrorKind;

const HEAD: &str = "module m.t\n  targets server\n  trust none\n\n";

fn func(sig: &str, effects: &str, body: &str) -> String {
    format!(
        "fn {sig}\n  intent \"x\"\n  requires none\n  ensures none\n  effects {effects}\n{{\n{body}\n}}\n\n"
    )
}

fn kind(src: &str) -> ErrorKind {
    check(src).expect_err("通るはずがない").kind
}

/// 型 `ty` が性質 `bound` を持つか。`pre` は追加の宣言（構造体など）。
fn has(pre: &str, ty: &str, bound: &str) -> bool {
    let src = format!(
        "{HEAD}{pre}{}{}",
        func(
            &format!("need<T: {bound}>(a: T) -> Int"),
            "none",
            "  return 0;"
        ),
        func(
            &format!("use_it(x: {ty}) -> Int"),
            "none",
            "  return need(x);"
        ),
    );
    match check(&src) {
        Ok(()) => true,
        Err(e) if e.kind == ErrorKind::ConstraintUnsatisfied => false,
        Err(e) => panic!(
            "{ty} の {bound} の確認で想定外のエラー: {:?} {}\n{src}",
            e.kind, e.message
        ),
    }
}

/// `Sendable`・`Shareable` の導出結果を表の 1 行として固定する。
fn assert_props(pre: &str, ty: &str, sendable: bool, shareable: bool) {
    assert_eq!(has(pre, ty, "Sendable"), sendable, "{ty} の Sendable");
    assert_eq!(has(pre, ty, "Shareable"), shareable, "{ty} の Shareable");
}

// ---- ADR 0007 決定 2 の表: 基本の型 ----

#[test]
fn adr0007_basic_types() {
    assert_props("", "Int", true, true);
    assert_props("", "Float", true, true);
    assert_props("", "Bool", true, true);
    assert_props("", "String", true, true);
    assert_props("", "Bytes", true, true);
    // 表の対象外だが既存の固定として: 番地はどちらの性質も持たない（ADR 0016）。
    assert_props("", "Addr", false, false);
}

// ---- ADR 0007 決定 2 の表: 組・列挙型・コレクション（中身で決まる行） ----

#[test]
fn adr0007_composite_types() {
    assert_props("", "(Int, String)", true, true);
    assert_props("", "(Int, Shared<Int>)", false, false);
    assert_props("", "Option<Int>", true, true);
    assert_props("", "Option<Cell<Int>>", true, false);
    assert_props("", "Result<Int, String>", true, true);
    assert_props("", "Result<Shared<Int>, Int>", false, false);
    assert_props("", "List<Int>", true, true);
    assert_props("", "List<Cell<Int>>", true, false);
    assert_props("", "List<Shared<Int>>", false, false);
}

// ---- ADR 0007 決定 2 の表: 利用者の構造体・列挙型・関数の型 ----

#[test]
fn adr0007_user_types() {
    let point = "struct Point {\n  id: Int,\n  name: String,\n}\n\n";
    assert_props(point, "Point", true, true);
    let has_cell = "struct HasCell {\n  c: Cell<Int>,\n}\n\n";
    assert_props(has_cell, "HasCell", true, false);
    let has_shared = "struct HasShared {\n  s: Shared<Int>,\n}\n\n";
    assert_props(has_shared, "HasShared", false, false);
    let shape = "enum Shape {\n  Circle(Int),\n  Empty,\n}\n\n";
    assert_props(shape, "Shape", true, true);
    let shape2 = "enum Shape2 {\n  A(Shared<Int>),\n  B,\n}\n\n";
    assert_props(shape2, "Shape2", false, false);
    // 関数の型（効果節なし）は、値として渡しても性質を持たない。
    assert_props("", "fn() -> Int", false, false);
    // 関数の型（効果節あり）は `Sendable`・`Shareable` のみを持つ（ADR 0022 決定 3）。
    let fnbox = "struct FnBox {\n  f: fn() -> Int effects none,\n}\n\n";
    assert_props(fnbox, "FnBox", true, true);
}

// ---- ADR 0007 決定 2 の表: `Shared` と `Cell` ----

#[test]
fn adr0007_shared_and_cell() {
    // `Shared` は、参照カウントの増減が原子的でないので、移すことも共有することもできない。
    assert_props("", "Shared<Int>", false, false);
    assert_props("", "Shared<Cell<Int>>", false, false);
    // `Cell` は移せるが、排他の検査が原子的でないので共有できない。
    assert_props("", "Cell<Int>", true, false);
    assert_props("", "Cell<Shared<Int>>", false, false);
    assert_props("", "Cell<Cell<Int>>", true, false);
}

// ---- ADR 0026 決定 3: 前置き型の性質 ----

#[test]
fn adr0026_task_types() {
    // チャネルの口は、中身が移せれば移せて、常に共有できる。
    assert_props("", "Sender<Int>", true, true);
    assert_props("", "Sender<Shared<Int>>", false, true);
    assert_props("", "Receiver<Int>", true, true);
    assert_props("", "Receiver<Cell<Int>>", true, true);
    // `SyncShared` と `Atomic` は、中身が移せて共有できるときだけ、どちらにもなる。
    assert_props("", "SyncShared<Int>", true, true);
    assert_props("", "SyncShared<Cell<Int>>", false, false);
    assert_props("", "SyncShared<Sender<Int>>", true, true);
    assert_props("", "Atomic<Int>", true, true);
    assert_props("", "Atomic<Cell<Int>>", false, false);
    // `Lock` は、中身が移せれば、移せて共有できる。
    assert_props("", "Lock<Int>", true, true);
    assert_props("", "Lock<Cell<Int>>", true, true);
    assert_props("", "Lock<Shared<Int>>", false, false);
    // 性質を持たない型: `TaskHandle` と `LockGuard` は作った（取得した）タスクだけが持つ。
    // `Scope` は値として現れない。
    assert_props("", "TaskHandle<Int>", false, false);
    assert_props("", "LockGuard<Int>", false, false);
    assert_props("", "Scope", false, false);
    // 壊れた錠の印は、引数を持たない不透明型の通常の導出（どちらも持つ）。
    assert_props("", "LockBroken", true, true);
}

// ---- 前置き型の登録そのもの ----

#[test]
fn task_types_are_registered() {
    // 9 つの型すべてが、引数の型として名前で引ける。
    let src = format!(
        "{HEAD}{}",
        func(
            "take(a: Sender<Int>, b: Receiver<Int>, c: SyncShared<Int>, d: Lock<Int>, e: LockGuard<Int>, f: LockBroken, g: Atomic<Int>, h: TaskHandle<Int>, s: &Scope) -> Int",
            "none",
            "  return 0;"
        )
    );
    check(&src).unwrap_or_else(|e| panic!("{:?} {}\n{src}", e.kind, e.message));
    // 利用者は同じ名前の型を宣言できない（前置きの名前）。
    let src = format!(
        "{HEAD}struct Sender {{\n  x: Int,\n}}\n\n{}",
        func("f() -> Int", "none", "  return 0;")
    );
    assert_eq!(kind(&src), ErrorKind::DuplicateDefinition);
    // 型引数の数が合わなければエラー。
    let src = format!(
        "{HEAD}{}",
        func("f(x: Sender) -> Int", "none", "  return 0;")
    );
    assert_eq!(kind(&src), ErrorKind::TypeArgCount);
}

// ---- `spawn` の検査: 通る例 ----

fn drive(effects: &str, body: &str) -> String {
    format!("{HEAD}{}", func("drive(s: &Scope) -> Int", effects, body))
}

fn drive_ok(effects: &str, body: &str) {
    let src = drive(effects, body);
    check(&src).unwrap_or_else(|e| panic!("{:?} {}\n{src}", e.kind, e.message));
}

#[test]
fn spawn_ok() {
    // `move` クロージャが値をとらえて、移せる型を返す。
    drive_ok(
        "task.spawn",
        "  let x = 5;\n  let h = s.spawn(move || \"hi\");\n  return x;",
    );
    drive_ok(
        "task.spawn",
        "  let mut xs = List.new();\n  xs.push(1);\n  let h = s.spawn(move || xs.len());\n  return 0;",
    );
    drive_ok(
        "task.spawn",
        "  let h = s.spawn(move || (1, \"x\"));\n  return 0;",
    );
    // `let` で束縛したクロージャを渡す。
    drive_ok(
        "task.spawn",
        "  let x = 5;\n  let g = move || x + 1;\n  let h = s.spawn(g);\n  return 0;",
    );
    // 名前関数を渡す（とらえる値は無い）。
    let src = format!(
        "{HEAD}{}{}",
        func("job() -> Int", "none", "  return 1;"),
        func(
            "drive(s: &Scope) -> Int",
            "task.spawn",
            "  let h = s.spawn(job);\n  return 0;"
        ),
    );
    check(&src).unwrap_or_else(|e| panic!("{:?} {}\n{src}", e.kind, e.message));
    // チャネルの口をとらえる（`Sender` は中身が移せれば移せる）。
    let src = format!(
        "{HEAD}{}{}",
        func("use_ref<T>(x: &T) -> Int", "none", "  return 0;"),
        func(
            "drive(s: &Scope, tx: Sender<Int>) -> Int",
            "task.spawn",
            "  let h = s.spawn(move || use_ref(&tx));\n  return 0;"
        ),
    );
    check(&src).unwrap_or_else(|e| panic!("{:?} {}\n{src}", e.kind, e.message));
    // 総称の文脈で、制約のある型引数をとらえる。
    let src = format!(
        "{HEAD}{}{}",
        func("use_ref<T>(x: &T) -> Int", "none", "  return 0;"),
        func(
            "gen<T: Sendable>(s: &Scope, v: T) -> Int",
            "task.spawn",
            "  let h = s.spawn(move || use_ref(&v));\n  return 0;"
        ),
    );
    check(&src).unwrap_or_else(|e| panic!("{:?} {}\n{src}", e.kind, e.message));
    // 戻り値の受け口（`TaskHandle`）をそのまま関数の戻り値にできる。
    let src = format!(
        "{HEAD}fn drive(s: &Scope) -> TaskHandle<Int>\n  intent \"x\"\n  requires none\n  ensures none\n  effects task.spawn\n{{\n  return s.spawn(move || 1);\n}}\n"
    );
    check(&src).unwrap_or_else(|e| panic!("{:?} {}\n{src}", e.kind, e.message));
}

// ---- `spawn` の検査: (a) 借用をとらえたらエラー ----

#[test]
fn spawn_borrow_capture_rejected() {
    // 読むだけの借りる捕獲。
    let src = drive(
        "task.spawn",
        "  let x = 1;\n  let h = s.spawn(|| x);\n  return 0;",
    );
    assert_eq!(kind(&src), ErrorKind::SpawnCaptureBorrow, "{src}");
    // 書き換える借りる捕獲。
    let src = format!(
        "{HEAD}struct Account {{\n  balance: Int,\n  owner: String,\n}}\n\n{}",
        func(
            "drive(s: &Scope) -> Int",
            "task.spawn",
            "  let mut a = Account { balance: 1, owner: \"x\" };\n  let h = s.spawn(|| { a.balance = 6; return a.balance; });\n  return 0;"
        ),
    );
    assert_eq!(kind(&src), ErrorKind::SpawnCaptureBorrow, "{src}");
    // 参照の値そのものを `move` で持つことも、借用をとらえたことになる。
    let src = format!(
        "{HEAD}struct Account {{\n  balance: Int,\n  owner: String,\n}}\n\n{}",
        func(
            "drive(s: &Scope, a: &Account) -> Int",
            "task.spawn",
            "  let h = s.spawn(move || a.balance);\n  return 0;"
        ),
    );
    assert_eq!(kind(&src), ErrorKind::SpawnCaptureBorrow, "{src}");
}

// ---- `spawn` の検査: (b) とらえた値が `Sendable` でなければエラー ----

#[test]
fn spawn_nonsendable_capture_rejected() {
    // `Shared` を含む構造体をとらえる。
    let src = format!(
        "{HEAD}struct HasShared {{\n  s: Shared<Int>,\n  n: Int,\n}}\n\n{}",
        func(
            "drive(s: &Scope) -> Int",
            "task.spawn",
            "  let h = HasShared { s: Shared.new(1), n: 3 };\n  let hh = s.spawn(move || h.n);\n  return 0;"
        ),
    );
    assert_eq!(kind(&src), ErrorKind::SpawnCaptureNotSendable, "{src}");
    // `Shared` そのものをとらえる（戻り値は `Int` なので (c) とは別に効く）。
    let src = drive(
        "state, task.spawn",
        "  let sh = Shared.new(Cell.new(1));\n  let h = s.spawn(move || sh.get());\n  return 0;",
    );
    assert_eq!(kind(&src), ErrorKind::SpawnCaptureNotSendable, "{src}");
    // 総称の文脈で、制約のない型引数をとらえる。
    let src = format!(
        "{HEAD}{}{}",
        func("use_ref<T>(x: &T) -> Int", "none", "  return 0;"),
        func(
            "gen<T>(s: &Scope, v: T) -> Int",
            "task.spawn",
            "  let h = s.spawn(move || use_ref(&v));\n  return 0;"
        ),
    );
    assert_eq!(kind(&src), ErrorKind::SpawnCaptureNotSendable, "{src}");
}

// ---- `spawn` の検査: (c) 戻り値が `Sendable` でなければエラー ----

#[test]
fn spawn_nonsendable_return_rejected() {
    // クロージャリテラルが `Shared` を作って返す。
    let src = drive(
        "task.spawn",
        "  let h = s.spawn(move || Shared.new(1));\n  return 0;",
    );
    assert_eq!(kind(&src), ErrorKind::SpawnReturnNotSendable, "{src}");
    // 名前関数の戻り値も見る。
    let src = format!(
        "{HEAD}{}{}",
        func("bad() -> Shared<Int>", "none", "  return Shared.new(1);"),
        func(
            "drive(s: &Scope) -> Int",
            "task.spawn",
            "  let h = s.spawn(bad);\n  return 0;"
        ),
    );
    assert_eq!(kind(&src), ErrorKind::SpawnReturnNotSendable, "{src}");
}

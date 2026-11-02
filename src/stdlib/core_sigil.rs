pub const LIST_SIGIL: &str = r#"module std.core
  targets bare, browser, server
  trust base

pub struct List<T> {
  ptr: Addr,
  len: Int,
  cap: Int,
}

properties List: Sendable, Shareable "Addr が指す領域はこの値が所有し、他の値と共有されない。要素の型に依存する性質は導出で担保される"

impl<T> List<T> {
  pub fn len(self: &List<T>) -> Int
    intent "要素数を返す"
    requires none
    ensures none
    effects none
  {
    return self.len;
  }

  pub fn is_empty(self: &List<T>) -> Bool
    intent "一覧が空かを返す"
    requires none
    ensures none
    effects none
  {
    return self.len == 0;
  }
}
"#;

pub const MAP_SIGIL: &str = r#"module std.core
  targets bare, browser, server
  trust base

pub struct Map<K, V> {
  ptr: Addr,
  len: Int,
  cap: Int,
}

properties Map: Sendable, Shareable "Addr が指す領域はこの値が所有し、他の値と共有されない。キーと値の型に依存する性質は導出で担保される"

impl<K, V> Map<K, V> {
  pub fn len(self: &Map<K, V>) -> Int
    intent "要素数を返す"
    requires none
    ensures none
    effects none
  {
    return self.len;
  }

  pub fn is_empty(self: &Map<K, V>) -> Bool
    intent "写像が空かを返す"
    requires none
    ensures none
    effects none
  {
    return self.len == 0;
  }
}
"#;

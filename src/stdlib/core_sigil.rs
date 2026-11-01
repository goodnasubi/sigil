pub const STRING_SIGIL: &str = r#"module std.core
  targets bare, browser, server
  trust base

pub struct String {
  ptr: Addr,
  len: Int,
  cap: Int,
}

impl String {
  pub fn len(self: &String) -> Int
    intent "文字列の長さをバイト数で返す"
    requires none
    ensures none
    effects none
  {
    return self.len;
  }

  pub fn is_empty(self: &String) -> Bool
    intent "文字列が空であるかを返す"
    requires none
    ensures none
    effects none
  {
    return self.len == 0;
  }
}
"#;

pub const LIST_SIGIL: &str = r#"module std.core
  targets bare, browser, server
  trust base

pub struct List<T> {
  ptr: Addr,
  len: Int,
  cap: Int,
}

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

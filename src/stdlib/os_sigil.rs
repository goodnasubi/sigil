pub const FS_PATH_SIGIL: &str = r#"module std.os
  targets browser, server
  trust base

pub struct Path {
  ptr: Addr,
  len: Int,
  cap: Int,
}

impl Path {
  pub fn is_empty(self: &Path) -> Bool
    intent "パスが空であるかを返す"
    requires none
    ensures none
    effects none
  {
    return self.len == 0;
  }
}
"#;

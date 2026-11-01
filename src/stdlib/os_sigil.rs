pub const CONSOLE_SIGIL: &str = r#"module std.os
  targets browser, server
  trust base

pub struct Console {}

impl Console {
  pub fn log(buf: &Bytes)
    intent "標準出力（fd 1）にバイト列を出力する"
    requires none
    ensures none
    effects console
    unchecked "SYS_write システムコールを fd 1 に対して発行する"
  {
    raw.syscall(1, 1, raw.addr_to_int(buf.ptr), buf.len(), 0, 0, 0);
  }

  pub fn err(buf: &Bytes)
    intent "標準エラー出力（fd 2）にバイト列を出力する"
    requires none
    ensures none
    effects console
    unchecked "SYS_write システムコールを fd 2 に対して発行する"
  {
    raw.syscall(1, 2, raw.addr_to_int(buf.ptr), buf.len(), 0, 0, 0);
  }
}
"#;

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


//! Sigil 処理系（M0: 字句解析・構文解析）。
//!
//! 整形器（`sigil fmt`）と仕様の抽出（`sigil spec`）は次の PR で加える。

pub mod ast;
pub mod error;
pub mod lexer;
pub mod parser;

pub use error::{Error, ErrorKind, Phase, Span};
pub use parser::{parse, parse_with_comments, Parsed};

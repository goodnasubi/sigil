//! Sigil 処理系（M0: 字句解析・構文解析、M1: 型検査）。
//!
//! 整形器（`sigil fmt`）を含む。仕様の抽出（`sigil spec`）も含む。

pub mod ast;
pub mod borrow;
pub mod check;
pub mod codegen;
pub mod effects;
pub mod error;
pub mod exec;
pub mod exhaustive;
pub mod fmt;
pub mod ir;
pub mod ir_eval;
pub mod json;
pub mod lexer;
pub mod lower;
pub mod lsp;
pub mod modpath;
pub mod native;
pub mod parser;
pub mod report;
pub mod runtime_lib;
#[path = "shared/args.rs"]
pub mod shared_args;
#[path = "shared/path.rs"]
pub mod shared_path;
#[path = "shared/text.rs"]
pub mod shared_text;
pub mod spec;
pub mod stdlib;
pub mod verify;

pub use error::{Error, ErrorKind, Phase, Span};
pub use parser::{parse, parse_with_comments, Parsed};

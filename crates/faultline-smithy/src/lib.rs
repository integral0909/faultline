//! The Smithy IDL front end.
//!
//! [`load`] reads a model's files, [`lexer`] and [`parser`] turn each into
//! an [`ast::Model`], and [`resolve`] resolves their names into one model
//! of absolute shape ids.

pub mod ast;
pub mod codes;
pub mod lexer;
pub mod load;
pub mod parser;
pub mod resolve;

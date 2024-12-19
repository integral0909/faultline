//! The Smithy IDL front end.
//!
//! [`lexer`] splits Smithy IDL into tokens, which [`parser`] turns into a
//! [`ast::Model`].

pub mod ast;
pub mod codes;
pub mod lexer;
pub mod parser;

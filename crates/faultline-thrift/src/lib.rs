//! The Apache Thrift front end.
//!
//! [`lexer`] splits Thrift IDL into tokens, which [`parser`] turns into a
//! [`ast::Document`].

pub mod ast;
pub mod codes;
pub mod lexer;
pub mod linker;
pub mod load;
pub mod parser;

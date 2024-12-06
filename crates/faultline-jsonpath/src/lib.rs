//! JSONPath (RFC 9535) queries over Faultline document trees.
//!
//! [`parse`] turns a query into its syntax tree, rejecting everything the
//! RFC does not allow, including ill-typed uses of functions.

pub mod ast;
mod parse;

pub use parse::{MAX_INT, ParseError, parse};

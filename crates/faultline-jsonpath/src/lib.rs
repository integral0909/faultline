//! JSONPath (RFC 9535) queries over Faultline document trees.
//!
//! [`parse`] turns a query into its syntax tree, rejecting everything the
//! RFC does not allow, including ill-typed uses of functions, and
//! [`ast::Query::select`] evaluates it, returning each selected node with
//! its [`Location`], so that callers can change the nodes a query selects.

pub mod ast;
mod eval;
mod parse;

pub use eval::{Location, Match, Step};
pub use parse::{MAX_INT, ParseError, parse};

//! The RAML 1.0 front end.
//!
//! [`load`] reads RAML documents with their `!include`s expanded, and
//! [`expr`] parses type expressions.

pub mod codes;
pub mod expr;
pub mod load;

pub use load::{Document, RamlLoader};

//! The RAML 1.0 front end.
//!
//! [`load`] reads RAML documents with their `!include`s expanded, and
//! [`expr`] parses type expressions, which [`types`] lowers into the IR
//! with the rest of RAML's type system. [`lower_raml`] lowers an API's
//! resources and methods into operations.

pub mod codes;
pub mod expr;
pub mod load;
mod lower;
pub mod types;

pub use load::{Document, RamlLoader};
pub use lower::{Lowered, lower_raml};

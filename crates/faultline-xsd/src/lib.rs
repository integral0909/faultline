//! The XML Schema front end.
//!
//! [`reader`] reads schema documents, following `include` and `import`
//! through the workspace loader, into the global [`model::Components`] of
//! a schema set.

pub mod codes;
pub mod model;
pub mod reader;

pub use reader::{Reader, XS};

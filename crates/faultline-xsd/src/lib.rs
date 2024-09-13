//! The XML Schema front end.
//!
//! [`reader`] reads schema documents, following `include` and `import`
//! through the workspace loader, into the global [`model::Components`] of
//! a schema set, and [`lower`] lowers them into the IR.

mod builtins;
pub mod codes;
pub mod lower;
pub mod model;
pub mod reader;

pub use lower::{Lowerer, type_name};
pub use reader::{Reader, XS};

//! XML documents for the front ends of XML-based API formats (XML Schema,
//! WSDL).
//!
//! [`parse`] reads a document into a tree of [`Element`]s whose names and
//! attributes have their namespaces resolved and carry source spans, so
//! diagnostics about a schema point at the construct they concern. The
//! reader refuses document type declarations (see [`parser`]), which
//! removes entity expansion and external entities from reach.

pub mod codes;
pub mod parser;
mod tree;

pub use parser::{XML_NAMESPACE, XMLNS_NAMESPACE, parse};
pub use tree::{Attribute, Content, Document, Element, QName};

/// Nesting deeper than this is refused whatever the configured limit.
pub const HARD_MAX_DEPTH: u32 = 1024;

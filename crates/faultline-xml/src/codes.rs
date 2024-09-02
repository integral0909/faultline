//! Diagnostic codes emitted while reading XML (`FLE07xx`).

use faultline_source::DiagnosticCode;

/// The document is not well-formed XML.
pub const SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE0701");
/// The document has a document type declaration, which is not processed.
pub const DTD: DiagnosticCode = DiagnosticCode::new("FLE0702");
/// An entity reference other than the predefined ones, or an invalid
/// character reference.
pub const ENTITY: DiagnosticCode = DiagnosticCode::new("FLE0703");
/// A namespace prefix is used without being declared.
pub const UNBOUND_PREFIX: DiagnosticCode = DiagnosticCode::new("FLE0704");
/// An element has the same attribute twice.
pub const DUPLICATE_ATTRIBUTE: DiagnosticCode = DiagnosticCode::new("FLE0705");
/// An end tag does not match the element it closes.
pub const MISMATCHED_TAG: DiagnosticCode = DiagnosticCode::new("FLE0706");
/// Elements nest deeper than the limit.
pub const DEPTH_LIMIT: DiagnosticCode = DiagnosticCode::new("FLE0707");
/// The document has more nodes than the limit.
pub const NODE_LIMIT: DiagnosticCode = DiagnosticCode::new("FLE0708");

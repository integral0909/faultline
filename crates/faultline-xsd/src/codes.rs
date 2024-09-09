//! Diagnostic codes emitted by the XML Schema front end (`FLE72xx`).

use faultline_source::DiagnosticCode;

/// A document meant to be a schema is not one.
pub const NOT_A_SCHEMA: DiagnosticCode = DiagnosticCode::new("FLE7201");
/// A `QName` in an attribute value uses an undeclared prefix.
pub const UNBOUND_PREFIX: DiagnosticCode = DiagnosticCode::new("FLE7202");
/// A global component is declared twice.
pub const DUPLICATE: DiagnosticCode = DiagnosticCode::new("FLE7203");
/// A reference to a type, element, group or attribute names nothing.
pub const UNRESOLVED: DiagnosticCode = DiagnosticCode::new("FLE7204");
/// An included or imported document cannot be loaded.
pub const LOAD: DiagnosticCode = DiagnosticCode::new("FLE7205");
/// A declaration is malformed.
pub const INVALID: DiagnosticCode = DiagnosticCode::new("FLE7206");
/// A construct that is not supported (a warning).
pub const UNSUPPORTED: DiagnosticCode = DiagnosticCode::new("FLE7207");

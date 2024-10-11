//! Diagnostic codes emitted by the RAML front end (`FLE76xx`).

use faultline_source::DiagnosticCode;

/// The document is not RAML.
pub const NOT_RAML: DiagnosticCode = DiagnosticCode::new("FLE7601");
/// A RAML version other than 1.0.
pub const UNSUPPORTED_VERSION: DiagnosticCode = DiagnosticCode::new("FLE7602");
/// An included file cannot be loaded, is outside the workspace, or includes
/// itself.
pub const INCLUDE: DiagnosticCode = DiagnosticCode::new("FLE7603");
/// A declaration is malformed.
pub const INVALID: DiagnosticCode = DiagnosticCode::new("FLE7604");
/// A type, trait, resource type or library name refers to nothing.
pub const UNRESOLVED: DiagnosticCode = DiagnosticCode::new("FLE7605");
/// A type expression cannot be parsed.
pub const TYPE_EXPRESSION: DiagnosticCode = DiagnosticCode::new("FLE7606");

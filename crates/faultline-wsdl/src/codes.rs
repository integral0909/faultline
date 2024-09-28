//! Diagnostic codes emitted by the WSDL front end (`FLE74xx`).

use faultline_source::DiagnosticCode;

/// The document is not a WSDL description.
pub const NOT_WSDL: DiagnosticCode = DiagnosticCode::new("FLE7401");
/// A WSDL 2.0 description, which is not supported.
pub const UNSUPPORTED_VERSION: DiagnosticCode = DiagnosticCode::new("FLE7402");
/// A reference to a message, port type, binding, element or type names
/// nothing.
pub const UNRESOLVED: DiagnosticCode = DiagnosticCode::new("FLE7403");
/// A declaration is malformed.
pub const INVALID: DiagnosticCode = DiagnosticCode::new("FLE7404");
/// An imported document cannot be loaded.
pub const LOAD: DiagnosticCode = DiagnosticCode::new("FLE7405");
/// SOAP encoding is compared as literal (a warning).
pub const ENCODED: DiagnosticCode = DiagnosticCode::new("FLE7406");

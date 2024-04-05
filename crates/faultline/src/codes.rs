//! Diagnostic codes raised by the engine itself (`FLE09xx`).

use faultline_source::DiagnosticCode;

/// An entry document could not be located or is outside the workspace.
pub const ENTRY_NOT_FOUND: DiagnosticCode = DiagnosticCode::new("FLE0901");
/// The workspace root could not be opened.
pub const INVALID_ROOT: DiagnosticCode = DiagnosticCode::new("FLE0902");

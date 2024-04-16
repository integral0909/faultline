//! Diagnostic codes raised by the engine itself (`FLE09xx`).

use faultline_source::DiagnosticCode;

/// An entry document could not be located or is outside the workspace.
pub const ENTRY_NOT_FOUND: DiagnosticCode = DiagnosticCode::new("FLE0901");
/// The workspace root could not be opened.
pub const INVALID_ROOT: DiagnosticCode = DiagnosticCode::new("FLE0902");
/// The git repository holding the workspace could not be opened or read.
pub const GIT: DiagnosticCode = DiagnosticCode::new("FLE0903");
/// A revision could not be resolved, or two revisions share no history.
pub const UNKNOWN_REVISION: DiagnosticCode = DiagnosticCode::new("FLE0904");
/// A configured API does not exist in the old version (a note, not an
/// error).
pub const NEW_API: DiagnosticCode = DiagnosticCode::new("FLE0905");

//! Diagnostic codes emitted by the JSON Schema front end (`FLE1xxx`).

use faultline_source::DiagnosticCode;

/// A keyword's value has the wrong JSON type or is out of range.
pub const INVALID_KEYWORD: DiagnosticCode = DiagnosticCode::new("FLE1001");
/// `type` names something other than the seven JSON Schema types.
pub const UNKNOWN_TYPE: DiagnosticCode = DiagnosticCode::new("FLE1002");
/// A reference does not resolve to anything.
pub const UNRESOLVED_REFERENCE: DiagnosticCode = DiagnosticCode::new("FLE1003");
/// A reference points outside the workspace root.
pub const REFERENCE_OUTSIDE_ROOT: DiagnosticCode = DiagnosticCode::new("FLE1004");
/// A reference needs network access, which is disabled.
pub const REMOTE_REFERENCE: DiagnosticCode = DiagnosticCode::new("FLE1005");
/// A referenced document could not be read.
pub const LOAD_FAILED: DiagnosticCode = DiagnosticCode::new("FLE1006");
/// `$schema` names a dialect Faultline does not know.
pub const UNKNOWN_DIALECT: DiagnosticCode = DiagnosticCode::new("FLE1007");
/// Keywords next to `$ref` are ignored in this dialect.
pub const REF_SIBLINGS_IGNORED: DiagnosticCode = DiagnosticCode::new("FLE1008");
/// A schema is defined in terms of itself without an object or array in
/// between, so it describes no well-founded set of values.
pub const UNGUARDED_RECURSION: DiagnosticCode = DiagnosticCode::new("FLE1009");
/// A keyword's semantics can only be approximated statically.
pub const APPROXIMATED: DiagnosticCode = DiagnosticCode::new("FLE1010");
/// A reference points at a value that is not a schema.
pub const NOT_A_SCHEMA: DiagnosticCode = DiagnosticCode::new("FLE1011");
/// The same `$id` or anchor is declared twice.
pub const DUPLICATE_IDENTIFIER: DiagnosticCode = DiagnosticCode::new("FLE1012");
/// A reference or identifier is not a valid URI reference.
pub const INVALID_URI: DiagnosticCode = DiagnosticCode::new("FLE1013");
/// An internal invariant was violated while lowering; always a bug.
pub const INTERNAL: DiagnosticCode = DiagnosticCode::new("FLE1099");

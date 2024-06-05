//! Diagnostic codes emitted by the Avro front end (`FLE5xxx`).

use faultline_source::DiagnosticCode;

/// A schema is not valid Avro: a missing or ill-typed attribute.
pub const INVALID_SCHEMA: DiagnosticCode = DiagnosticCode::new("FLE5001");
/// A type name refers to no primitive or earlier definition.
pub const UNKNOWN_TYPE: DiagnosticCode = DiagnosticCode::new("FLE5002");
/// A name, field or enum symbol is defined twice.
pub const DUPLICATE: DiagnosticCode = DiagnosticCode::new("FLE5003");
/// A name or symbol does not match `[A-Za-z_][A-Za-z0-9_]*`.
pub const INVALID_NAME: DiagnosticCode = DiagnosticCode::new("FLE5004");
/// A union holds another union, or two branches of the same type.
pub const INVALID_UNION: DiagnosticCode = DiagnosticCode::new("FLE5005");
/// A logical type does not fit its underlying type (a warning).
pub const LOGICAL_TYPE: DiagnosticCode = DiagnosticCode::new("FLE5006");
/// A default value does not match the field's type, or for a union, its
/// first branch.
pub const INVALID_DEFAULT: DiagnosticCode = DiagnosticCode::new("FLE5007");

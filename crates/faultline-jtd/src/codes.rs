//! Diagnostic codes emitted by the JSON Type Definition front end
//! (`FLE82xx`).

use faultline_source::DiagnosticCode;

/// A schema is not a valid JSON Type Definition: an unknown keyword, a
/// keyword of the wrong type, or keywords of several forms together.
pub const INVALID_SCHEMA: DiagnosticCode = DiagnosticCode::new("FLE8201");
/// A `ref` names no definition.
pub const UNKNOWN_DEFINITION: DiagnosticCode = DiagnosticCode::new("FLE8202");
/// An enum value, or a property in both `properties` and
/// `optionalProperties`, is given twice.
pub const DUPLICATE: DiagnosticCode = DiagnosticCode::new("FLE8203");
/// A discriminator mapping is not a non-nullable properties schema, or
/// declares the discriminator's own property.
pub const INVALID_MAPPING: DiagnosticCode = DiagnosticCode::new("FLE8204");
/// Definitions refer to each other without ever reaching a type.
pub const CYCLE: DiagnosticCode = DiagnosticCode::new("FLE8205");

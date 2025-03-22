//! Diagnostic codes emitted by the AsyncAPI front end (`FLE60xx`).

use faultline_source::DiagnosticCode;

/// The document is not an AsyncAPI 2.x or 3.0 description.
pub const NOT_ASYNCAPI: DiagnosticCode = DiagnosticCode::new("FLE6001");
/// A field has the wrong JSON type or a value the specification forbids.
pub const INVALID_FIELD: DiagnosticCode = DiagnosticCode::new("FLE6002");
/// A reference to a non-schema object does not resolve.
pub const UNRESOLVED_REFERENCE: DiagnosticCode = DiagnosticCode::new("FLE6003");
/// References between non-schema objects form a cycle.
pub const REFERENCE_CYCLE: DiagnosticCode = DiagnosticCode::new("FLE6004");
/// A payload or headers schema uses a schema format that is not supported;
/// it is compared as accepting any value.
pub const UNSUPPORTED_SCHEMA_FORMAT: DiagnosticCode = DiagnosticCode::new("FLE6005");
/// No longer emitted: traits are applied. The code is not reused.
pub const TRAITS_IGNORED: DiagnosticCode = DiagnosticCode::new("FLE6006");
/// A 3.0 operation's `action` is neither `send` nor `receive`.
pub const INVALID_ACTION: DiagnosticCode = DiagnosticCode::new("FLE6007");
/// An embedded Avro schema is invalid.
pub const INVALID_AVRO: DiagnosticCode = DiagnosticCode::new("FLE6008");
/// An embedded Protobuf schema is invalid, or is not written as a string.
pub const INVALID_PROTOBUF: DiagnosticCode = DiagnosticCode::new("FLE6009");
/// An embedded RAML type is invalid.
pub const INVALID_RAML: DiagnosticCode = DiagnosticCode::new("FLE6010");

//! Diagnostic codes emitted by the Protobuf front end (`FLE3xxx`).

use faultline_source::DiagnosticCode;

/// The file does not follow the Protobuf grammar.
pub const SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE3001");
/// An imported file could not be found or read.
pub const IMPORT_NOT_FOUND: DiagnosticCode = DiagnosticCode::new("FLE3002");
/// A type name does not resolve to a message or enum.
pub const UNRESOLVED_TYPE: DiagnosticCode = DiagnosticCode::new("FLE3003");
/// Two declarations have the same fully-qualified name.
pub const DUPLICATE_SYMBOL: DiagnosticCode = DiagnosticCode::new("FLE3004");
/// A field or enum number is out of range or in a range Protobuf reserves.
pub const INVALID_NUMBER: DiagnosticCode = DiagnosticCode::new("FLE3005");
/// Two fields, or two enum values without `allow_alias`, share a number.
pub const DUPLICATE_NUMBER: DiagnosticCode = DiagnosticCode::new("FLE3006");
/// A field or value uses a number or name its declaration reserves.
pub const RESERVED_CONFLICT: DiagnosticCode = DiagnosticCode::new("FLE3007");
/// A map key type is not an integral type, `bool` or `string`.
pub const INVALID_MAP_KEY: DiagnosticCode = DiagnosticCode::new("FLE3008");
/// A name resolves, but to the wrong kind of declaration (an enum as an RPC
/// input, a service as a field type).
pub const WRONG_KIND: DiagnosticCode = DiagnosticCode::new("FLE3009");
/// Files import each other in a cycle.
pub const IMPORT_CYCLE: DiagnosticCode = DiagnosticCode::new("FLE3010");
/// A construct the syntax level does not allow (`required` in proto3, an
/// enum whose first value is not zero in proto3, an unknown syntax).
pub const INVALID_FOR_SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE3011");
/// An import is not used (a warning).
pub const UNUSED_IMPORT: DiagnosticCode = DiagnosticCode::new("FLE3012");
/// An HTTP rule cannot be used for the HTTP/JSON view (a warning): it is
/// malformed, names fields the request does not have, or binds a path
/// twice.
pub const HTTP_RULE: DiagnosticCode = DiagnosticCode::new("FLE3013");
/// A schema embedded in another document (an AsyncAPI payload) declares no
/// message for the document to use.
pub const NO_MESSAGE: DiagnosticCode = DiagnosticCode::new("FLE3014");

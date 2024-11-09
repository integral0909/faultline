//! Diagnostic codes emitted by the Thrift front end (`FLE80xx`).

use faultline_source::DiagnosticCode;

/// The file does not follow the Thrift grammar.
pub const SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE8001");
/// An included file could not be found or read.
pub const INCLUDE_NOT_FOUND: DiagnosticCode = DiagnosticCode::new("FLE8002");
/// A type or service name does not resolve.
pub const UNRESOLVED: DiagnosticCode = DiagnosticCode::new("FLE8003");
/// Two definitions in a file have the same name.
pub const DUPLICATE_DEFINITION: DiagnosticCode = DiagnosticCode::new("FLE8004");
/// Two fields share an id or a name, or an id is out of range.
pub const INVALID_FIELD: DiagnosticCode = DiagnosticCode::new("FLE8005");
/// A name resolves, but to the wrong kind of definition (a service as a
/// field type, a struct as a service's parent).
pub const WRONG_KIND: DiagnosticCode = DiagnosticCode::new("FLE8006");
/// Files include each other in a cycle, or typedefs refer to themselves.
pub const CYCLE: DiagnosticCode = DiagnosticCode::new("FLE8007");
/// A field without an explicit id (a warning): the compiler assigns a
/// negative one that changes when fields are reordered.
pub const IMPLICIT_ID: DiagnosticCode = DiagnosticCode::new("FLE8008");
/// A deprecated construct (`senum`, `slist`, `async`), read as its
/// replacement or ignored (a warning).
pub const DEPRECATED_SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE8009");

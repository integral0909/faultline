//! Diagnostic codes emitted by the GraphQL front end (`FLE4xxx`).

use faultline_source::DiagnosticCode;

/// The document does not follow the GraphQL grammar.
pub const SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE4001");
/// A type name refers to no definition.
pub const UNKNOWN_TYPE: DiagnosticCode = DiagnosticCode::new("FLE4002");
/// Two definitions of a type, field, argument, enum value or directive.
pub const DUPLICATE: DiagnosticCode = DiagnosticCode::new("FLE4003");
/// A type is used where its kind is not allowed: an output type as an
/// argument, a scalar as a union member, an extension of the wrong kind.
pub const WRONG_KIND: DiagnosticCode = DiagnosticCode::new("FLE4004");
/// An object or interface does not provide a field its interface declares.
pub const INTERFACE: DiagnosticCode = DiagnosticCode::new("FLE4005");
/// The schema has no query root type.
pub const NO_QUERY_TYPE: DiagnosticCode = DiagnosticCode::new("FLE4006");
/// A query, mutation or fragment appears in a schema document.
pub const EXECUTABLE: DiagnosticCode = DiagnosticCode::new("FLE4007");
/// A name starting with `__`, which introspection reserves.
pub const RESERVED_NAME: DiagnosticCode = DiagnosticCode::new("FLE4008");
/// A type or directive definition has no fields, values or members.
pub const EMPTY: DiagnosticCode = DiagnosticCode::new("FLE4009");
/// A field of a `@oneOf` input object is non-null or has a default.
pub const ONE_OF: DiagnosticCode = DiagnosticCode::new("FLE4010");

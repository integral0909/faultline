//! Diagnostic codes emitted by the OpenAPI front end (`FLE2xxx`).

use faultline_source::DiagnosticCode;

/// The document is not an OpenAPI 3.x description.
pub const NOT_OPENAPI: DiagnosticCode = DiagnosticCode::new("FLE2001");
/// A field has the wrong JSON type.
pub const INVALID_FIELD: DiagnosticCode = DiagnosticCode::new("FLE2002");
/// A reference to a non-schema object does not resolve.
pub const UNRESOLVED_REFERENCE: DiagnosticCode = DiagnosticCode::new("FLE2003");
/// Two operations have the same method and normalized path.
pub const DUPLICATE_OPERATION: DiagnosticCode = DiagnosticCode::new("FLE2004");
/// A path parameter is undeclared, not required, or not in the template.
pub const PATH_PARAMETER: DiagnosticCode = DiagnosticCode::new("FLE2005");
/// A response key is not a status code, range or `default`.
pub const INVALID_STATUS: DiagnosticCode = DiagnosticCode::new("FLE2006");
/// A parameter's `in` is not path, query, header or cookie.
pub const INVALID_LOCATION: DiagnosticCode = DiagnosticCode::new("FLE2007");
/// References between non-schema objects form a cycle.
pub const REFERENCE_CYCLE: DiagnosticCode = DiagnosticCode::new("FLE2008");
/// A parameter or header has neither `schema` nor `content`.
pub const MISSING_SCHEMA: DiagnosticCode = DiagnosticCode::new("FLE2009");
/// Swagger 2.0 descriptions are not supported yet.
pub const SWAGGER_2: DiagnosticCode = DiagnosticCode::new("FLE2010");
/// A path template does not start with `/`.
pub const INVALID_PATH: DiagnosticCode = DiagnosticCode::new("FLE2011");

//! Diagnostic codes emitted by the Smithy front end (`FLE84xx`).

use faultline_source::DiagnosticCode;

/// The file does not follow the Smithy IDL grammar.
pub const SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE8401");
/// A shape id resolves to no shape.
pub const UNRESOLVED: DiagnosticCode = DiagnosticCode::new("FLE8402");
/// Two shapes, or two members of a shape, have the same name.
pub const DUPLICATE: DiagnosticCode = DiagnosticCode::new("FLE8403");
/// A shape id resolves to a shape of the wrong kind (a structure as an
/// operation, a string as a mixin).
pub const WRONG_KIND: DiagnosticCode = DiagnosticCode::new("FLE8404");
/// A construct the front end does not model, or a Smithy version other
/// than 2 (a warning).
pub const UNSUPPORTED: DiagnosticCode = DiagnosticCode::new("FLE8405");
/// Mixins or resources refer to each other in a cycle, or a member's
/// elided target cannot be found.
pub const INVALID: DiagnosticCode = DiagnosticCode::new("FLE8406");

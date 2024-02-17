//! Diagnostic codes emitted while reading documents.
//!
//! `FLE01xx` codes belong to the JSON reader, `FLE02xx` to the YAML loader,
//! and `FLE00xx` to problems common to both. Codes are stable identifiers:
//! once published they are never reassigned.

use faultline_source::DiagnosticCode;

pub const INVALID_ENCODING: DiagnosticCode = DiagnosticCode::new("FLE0001");
pub const FILE_TOO_LARGE: DiagnosticCode = DiagnosticCode::new("FLE0002");
pub const DEPTH_LIMIT: DiagnosticCode = DiagnosticCode::new("FLE0003");
pub const NODE_LIMIT: DiagnosticCode = DiagnosticCode::new("FLE0004");
pub const DUPLICATE_KEY: DiagnosticCode = DiagnosticCode::new("FLE0005");
pub const NUMBER_OUT_OF_RANGE: DiagnosticCode = DiagnosticCode::new("FLE0006");

pub const JSON_UNEXPECTED_CHARACTER: DiagnosticCode = DiagnosticCode::new("FLE0101");
pub const JSON_UNEXPECTED_END: DiagnosticCode = DiagnosticCode::new("FLE0102");
pub const JSON_INVALID_ESCAPE: DiagnosticCode = DiagnosticCode::new("FLE0103");
pub const JSON_LONE_SURROGATE: DiagnosticCode = DiagnosticCode::new("FLE0104");
pub const JSON_CONTROL_CHARACTER: DiagnosticCode = DiagnosticCode::new("FLE0105");
pub const JSON_INVALID_NUMBER: DiagnosticCode = DiagnosticCode::new("FLE0106");
pub const JSON_TRAILING_CONTENT: DiagnosticCode = DiagnosticCode::new("FLE0107");

pub const YAML_SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE0201");
pub const YAML_UNDEFINED_ALIAS: DiagnosticCode = DiagnosticCode::new("FLE0202");
pub const YAML_MULTIPLE_DOCUMENTS: DiagnosticCode = DiagnosticCode::new("FLE0203");
pub const YAML_COMPLEX_KEY: DiagnosticCode = DiagnosticCode::new("FLE0204");
pub const YAML_UNSUPPORTED_TAG: DiagnosticCode = DiagnosticCode::new("FLE0205");
pub const YAML_TAG_MISMATCH: DiagnosticCode = DiagnosticCode::new("FLE0206");
pub const YAML_INVALID_MERGE: DiagnosticCode = DiagnosticCode::new("FLE0207");

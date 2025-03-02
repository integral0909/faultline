//! Diagnostic codes emitted while reading documents.
//!
//! `FLE01xx` codes belong to the JSON reader, `FLE02xx` to the YAML loader,
//! and `FLE00xx` to problems common to both. Codes are stable identifiers:
//! once published they are never reassigned.

use faultline_source::DiagnosticCode;

/// A file is not text in UTF-8 (or UTF-16 or UTF-32 with a byte order mark).
pub const INVALID_ENCODING: DiagnosticCode = DiagnosticCode::new("FLE0001");
/// A file is larger than the input size limit.
pub const FILE_TOO_LARGE: DiagnosticCode = DiagnosticCode::new("FLE0002");
/// Values nest deeper than the input depth limit.
pub const DEPTH_LIMIT: DiagnosticCode = DiagnosticCode::new("FLE0003");
/// A document has more values than the node limit, counting the values YAML aliases expand to.
pub const NODE_LIMIT: DiagnosticCode = DiagnosticCode::new("FLE0004");
/// An object or mapping has the same key twice.
pub const DUPLICATE_KEY: DiagnosticCode = DiagnosticCode::new("FLE0005");
/// A number's exponent is too large to represent.
pub const NUMBER_OUT_OF_RANGE: DiagnosticCode = DiagnosticCode::new("FLE0006");

/// JSON: a character that cannot start or continue a value here.
pub const JSON_UNEXPECTED_CHARACTER: DiagnosticCode = DiagnosticCode::new("FLE0101");
/// JSON: the text ends inside a value.
pub const JSON_UNEXPECTED_END: DiagnosticCode = DiagnosticCode::new("FLE0102");
/// JSON: a string has an escape other than those JSON defines.
pub const JSON_INVALID_ESCAPE: DiagnosticCode = DiagnosticCode::new("FLE0103");
/// JSON: a `\u` escape is half of a surrogate pair without the other half.
pub const JSON_LONE_SURROGATE: DiagnosticCode = DiagnosticCode::new("FLE0104");
/// JSON: a string holds a control character that must be escaped.
pub const JSON_CONTROL_CHARACTER: DiagnosticCode = DiagnosticCode::new("FLE0105");
/// JSON: a number is not written as JSON requires (leading zeros, a missing digit).
pub const JSON_INVALID_NUMBER: DiagnosticCode = DiagnosticCode::new("FLE0106");
/// JSON: text follows the document's value.
pub const JSON_TRAILING_CONTENT: DiagnosticCode = DiagnosticCode::new("FLE0107");

/// YAML: the text does not follow the YAML grammar.
pub const YAML_SYNTAX: DiagnosticCode = DiagnosticCode::new("FLE0201");
/// YAML: an alias (`*name`) refers to no anchor defined before it.
pub const YAML_UNDEFINED_ALIAS: DiagnosticCode = DiagnosticCode::new("FLE0202");
/// YAML: a file holds more than one document; one is expected.
pub const YAML_MULTIPLE_DOCUMENTS: DiagnosticCode = DiagnosticCode::new("FLE0203");
/// YAML: a mapping key is a collection rather than a scalar.
pub const YAML_COMPLEX_KEY: DiagnosticCode = DiagnosticCode::new("FLE0204");
/// YAML: a tag other than the core schema's is ignored (a warning).
pub const YAML_UNSUPPORTED_TAG: DiagnosticCode = DiagnosticCode::new("FLE0205");
/// YAML: a value does not match its core schema tag, such as `!!int` on text.
pub const YAML_TAG_MISMATCH: DiagnosticCode = DiagnosticCode::new("FLE0206");
/// YAML: a merge key (`<<`) whose value is not a mapping or a list of mappings.
pub const YAML_INVALID_MERGE: DiagnosticCode = DiagnosticCode::new("FLE0207");

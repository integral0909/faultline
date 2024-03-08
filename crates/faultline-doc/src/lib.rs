//! Documents for Faultline: JSON and YAML parsed into one spanned tree.
//!
//! Every schema format Faultline reads, except Protobuf and GraphQL, is
//! serialized as JSON or YAML. This crate turns such text into a [`Node`] tree
//! that keeps the [`Span`](faultline_source::Span) of every value and key, so
//! later stages can point at exact locations. It also provides the two
//! addressing schemes schema references are built on: JSON Pointer
//! ([`JsonPointer`]) and URI references ([`UriRef`]).
//!
//! Parsing never panics on malformed input and enforces the ceilings in
//! [`Limits`]; problems are reported as
//! [`Diagnostic`](faultline_source::Diagnostic)s with stable codes from
//! [`codes`].

pub mod codes;
mod json;
pub mod loader;
mod node;
pub mod number;
mod pointer;
mod store;
pub mod uri;
mod write;
mod yaml;

use std::fmt;

use faultline_source::{Diagnostics, Limits, SourceFile, SourceId};

pub use node::{Kind, Member, Node, Object, Value};
pub use number::{Decimal, Number};
pub use pointer::{JsonPointer, PointerError};
pub use store::{DocumentStore, StoreError, StoredDocument};
pub use uri::{UriError, UriRef};
pub use write::write_json_string;

/// Absolute ceiling on nesting depth, applied even if
/// [`Limits::max_depth`](faultline_source::Limits::max_depth) is configured
/// higher. The JSON reader is recursive and tree destruction is recursive, so
/// this bounds stack usage regardless of configuration.
pub const HARD_MAX_DEPTH: u32 = 1024;

/// The serialization format of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Json,
    Yaml,
}

impl Format {
    /// Maps a file extension (without the dot, any case) to a format.
    pub fn from_extension(extension: &str) -> Option<Format> {
        match extension.to_ascii_lowercase().as_str() {
            "json" => Some(Format::Json),
            "yaml" | "yml" => Some(Format::Yaml),
            _ => None,
        }
    }

    /// Guesses the format of text with no informative extension.
    ///
    /// Text whose first significant character opens a JSON object or array is
    /// read as JSON, because the JSON reader gives more precise errors for
    /// JSON input. Everything else is read as YAML, which also accepts most
    /// JSON.
    pub fn sniff(text: &str) -> Format {
        match text.trim_start().as_bytes().first() {
            Some(b'{' | b'[') => Format::Json,
            _ => Format::Yaml,
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Format::Json => "JSON",
            Format::Yaml => "YAML",
        })
    }
}

/// A successfully parsed document.
#[derive(Clone, Debug)]
pub struct Document {
    pub root: Node,
    pub format: Format,
    pub source: SourceId,
}

/// The outcome of parsing: a document if parsing got far enough to produce
/// one, and every diagnostic raised along the way.
///
/// A document can be present alongside errors (for example duplicate keys);
/// callers decide whether to continue by checking
/// [`Diagnostics::has_errors`].
#[derive(Debug)]
pub struct Parsed {
    pub document: Option<Document>,
    pub diagnostics: Diagnostics,
}

/// Parses `file` as `format`.
pub fn parse(file: &SourceFile, format: Format, limits: &Limits) -> Parsed {
    let mut diagnostics = Diagnostics::new();
    let root = match format {
        Format::Json => json::parse(file.text(), file.id(), limits, &mut diagnostics),
        Format::Yaml => yaml::load(file.text(), file.id(), limits, &mut diagnostics),
    };
    Parsed {
        document: root.map(|root| Document {
            root,
            format,
            source: file.id(),
        }),
        diagnostics,
    }
}

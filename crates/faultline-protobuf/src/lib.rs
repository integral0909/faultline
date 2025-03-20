//! Protocol Buffers front end.
//!
//! [`lower_protobuf`] loads a `.proto` file and its imports, resolves names,
//! checks the rules `protoc` enforces, and lowers the result into the IR:
//! the entry file's services become operations, or, for files that only
//! describe data, its top-level messages become schema roots.
//!
//! [`lower_protobuf_http`] lowers the services instead as the HTTP/JSON API
//! their `google.api.http` rules expose (see [`http`]), for comparison with
//! an OpenAPI description of that API.

pub mod ast;
pub mod codes;
pub mod http;
mod json;
pub mod lexer;
pub mod linker;
pub mod load;
mod lower;
pub mod parser;
mod wkt;

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loaded, Loader};
use faultline_ir::{Api, QualName, Role, TypeId};
use faultline_source::{Diagnostic, Diagnostics, SourceMap, SourceName, Span};

pub use lower::RPC_MEDIA_TYPE;

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the files have errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Loads `entry` and its imports through `loader`, registering the files in
/// `sources`, and lowers them into an [`Api`] named `name`. `role` is used
/// for the schema roots of files without services.
pub fn lower_protobuf(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
    role: Role,
) -> Lowered {
    let files = load::load(entry, loader, sources);
    let mut diagnostics = files.diagnostics;
    if files.files.is_empty() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut linked = linker::link(&files.files);
    diagnostics.extend(std::mem::take(&mut linked.diagnostics));
    diagnostics.sort();
    if diagnostics.has_errors() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let api = lower::Lowerer::new(&files.files, &linked, name).lower(role);
    Lowered {
        api: Some(api),
        diagnostics,
    }
}

/// Loads `entry` and its imports like [`lower_protobuf`], and lowers its
/// services as the HTTP/JSON API their `google.api.http` rules expose.
pub fn lower_protobuf_http(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
) -> Lowered {
    let files = load::load(entry, loader, sources);
    let mut diagnostics = files.diagnostics;
    if files.files.is_empty() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut linked = linker::link(&files.files);
    diagnostics.extend(std::mem::take(&mut linked.diagnostics));
    if diagnostics.has_errors() {
        diagnostics.sort();
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let (api, view_diagnostics) = http::lower(&files.files, &linked, name);
    diagnostics.extend(view_diagnostics);
    diagnostics.sort();
    Lowered {
        api: Some(api),
        diagnostics,
    }
}

/// A Protobuf schema embedded in another document, lowered on its own.
#[derive(Debug)]
pub struct Embedded {
    /// The types, and the message the schema describes; `None` when the
    /// schema has errors or declares no message.
    pub lowered: Option<(Api, TypeId)>,
    pub diagnostics: Diagnostics,
}

/// Lowers the Protobuf source `text` found at `uri` in another document (an
/// AsyncAPI payload, for example) and returns the type of its first
/// top-level message. Imports are read through `loader`, relative to the
/// directory of `uri`; `span` locates the text in its document, for
/// diagnostics about the schema as a whole.
pub fn lower_embedded(
    text: &str,
    uri: &UriRef,
    span: Span,
    loader: &dyn Loader,
    sources: &SourceMap,
) -> Embedded {
    let embedding = Embedding {
        uri,
        text,
        inner: loader,
    };
    let files = load::load(uri, &embedding, sources);
    let mut diagnostics = files.diagnostics;
    let Some(entry) = files.files.first() else {
        return Embedded {
            lowered: None,
            diagnostics,
        };
    };
    let package = entry.ast.package.as_ref().map(|p| p.text.as_str());
    let Some(first) = entry.ast.messages.first() else {
        diagnostics.push(
            Diagnostic::error("the Protobuf schema declares no message".to_owned())
                .with_code(codes::NO_MESSAGE)
                .with_primary(span, ""),
        );
        return Embedded {
            lowered: None,
            diagnostics,
        };
    };
    let full_name = match package {
        Some(package) if !package.is_empty() => format!("{package}.{}", first.name.text),
        _ => first.name.text.clone(),
    };
    let mut linked = linker::link(&files.files);
    diagnostics.extend(std::mem::take(&mut linked.diagnostics));
    diagnostics.sort();
    if diagnostics.has_errors() {
        return Embedded {
            lowered: None,
            diagnostics,
        };
    }
    let api = lower::Lowerer::new(&files.files, &linked, "embedded").lower(Role::Both);
    let id = api.named.get(&QualName::new(&full_name)).copied();
    Embedded {
        lowered: id.map(|id| (api, id)),
        diagnostics,
    }
}

/// A loader that answers for the embedded schema itself and passes every
/// other request (its imports) on.
struct Embedding<'a> {
    uri: &'a UriRef,
    text: &'a str,
    inner: &'a dyn Loader,
}

impl Loader for Embedding<'_> {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        if uri == self.uri {
            return Ok(Loaded {
                name: SourceName::Virtual(self.uri.to_string()),
                text: self.text.to_owned(),
            });
        }
        self.inner.load(uri)
    }

    fn list(&self, uri: &UriRef) -> Result<Vec<String>, LoadError> {
        self.inner.list(uri)
    }
}

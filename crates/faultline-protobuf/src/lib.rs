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
use faultline_doc::loader::Loader;
use faultline_ir::{Api, Role};
use faultline_source::{Diagnostics, SourceMap};

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

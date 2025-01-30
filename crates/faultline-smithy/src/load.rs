//! Loading a Smithy model: the entry file and the `.smithy` files beside
//! it, which is how Smithy projects lay out a model. Loaders that cannot
//! list directories give the entry file alone.

use std::fmt::Write as _;

use faultline_doc::UriRef;
use faultline_doc::loader::{Loader, directory_of};
use faultline_source::{Diagnostic, Diagnostics, SourceId, SourceMap};

use crate::ast::Model;
use crate::codes;
use crate::parser::parse;

/// More files than this in one model is treated as hostile input.
pub const MAX_FILES: usize = 10_000;

/// A parsed file.
#[derive(Clone, Debug)]
pub struct SmithyFile {
    pub uri: UriRef,
    pub source: SourceId,
    pub model: Model,
}

/// The entry file (index 0) and the other files of its directory.
#[derive(Debug, Default)]
pub struct Files {
    pub files: Vec<SmithyFile>,
    pub diagnostics: Diagnostics,
}

/// Loads `entry` and the `.smithy` files in its directory; or, for a
/// `.json` entry, the JSON AST it holds.
pub fn load(entry: &UriRef, loader: &dyn Loader, sources: &SourceMap) -> Files {
    let mut files = Files::default();
    let entry = entry.without_fragment().normalized();
    if entry.path().to_ascii_lowercase().ends_with(".json") {
        read_json(&entry, loader, sources, &mut files);
        return files;
    }
    read(&entry, loader, sources, &mut files, true);
    if files.files.is_empty() {
        return files;
    }
    let directory = directory_of(&entry);
    let names = loader.list(&directory).unwrap_or_default();
    for name in names.iter().filter(|n| n.ends_with(".smithy")) {
        let Ok(reference) = UriRef::parse(&encode_name(name)) else {
            continue;
        };
        let uri = directory.resolve(&reference).normalized();
        if uri == entry {
            continue;
        }
        if files.files.len() >= MAX_FILES {
            files.diagnostics.push(
                Diagnostic::error(format!("the model has more than {MAX_FILES} files"))
                    .with_code(codes::UNSUPPORTED),
            );
            break;
        }
        read(&uri, loader, sources, &mut files, false);
    }
    files
}

fn read(uri: &UriRef, loader: &dyn Loader, sources: &SourceMap, files: &mut Files, entry: bool) {
    let loaded = match loader.load(uri) {
        Ok(loaded) => loaded,
        Err(e) => {
            // A sibling that cannot be read is reported, not fatal.
            let diagnostic = Diagnostic::error(format!("cannot read {uri}: {e}"));
            files.diagnostics.push(if entry {
                diagnostic.with_code(codes::UNRESOLVED)
            } else {
                Diagnostic::warning(format!("cannot read {uri}: {e}")).with_code(codes::UNSUPPORTED)
            });
            return;
        }
    };
    let file = match sources.add(loaded.name, loaded.text) {
        Ok(file) => file,
        Err(e) => {
            files
                .diagnostics
                .push(Diagnostic::error(e.to_string()).with_code(codes::UNSUPPORTED));
            return;
        }
    };
    let (model, diagnostics) = parse(file.id(), file.text());
    files.diagnostics.extend(diagnostics);
    if let Some(version) = &model.version
        && !version.starts_with('2')
    {
        files.diagnostics.push(
            Diagnostic::warning(format!(
                "Smithy IDL {version} is read as version 2; some constructs may differ"
            ))
            .with_code(codes::UNSUPPORTED),
        );
    }
    files.files.push(SmithyFile {
        uri: uri.clone(),
        source: file.id(),
        model,
    });
}

/// Reads a JSON AST model: one file per namespace, a namespace with a
/// service first, as the entry.
fn read_json(uri: &UriRef, loader: &dyn Loader, sources: &SourceMap, files: &mut Files) {
    let loaded = match loader.load(uri) {
        Ok(loaded) => loaded,
        Err(e) => {
            files.diagnostics.push(
                Diagnostic::error(format!("cannot read {uri}: {e}")).with_code(codes::UNRESOLVED),
            );
            return;
        }
    };
    let file = match sources.add(loaded.name, loaded.text) {
        Ok(file) => file,
        Err(e) => {
            files
                .diagnostics
                .push(Diagnostic::error(e.to_string()).with_code(codes::UNSUPPORTED));
            return;
        }
    };
    let parsed = faultline_doc::parse(
        &file,
        faultline_doc::Format::Json,
        &faultline_source::Limits::default(),
    );
    files.diagnostics.extend(parsed.diagnostics);
    let Some(document) = parsed.document else {
        return;
    };
    let mut models = crate::json_ast::read(&document.root, &mut files.diagnostics);
    models.sort_by_key(|m| {
        !m.shapes
            .iter()
            .any(|s| s.kind == crate::ast::ShapeKind::Service)
    });
    for model in models {
        files.files.push(SmithyFile {
            uri: uri.clone(),
            source: file.id(),
            model,
        });
    }
}

/// A file name as a relative reference: every byte but unreserved ones
/// percent-encoded, so that no name reads as a scheme, query or fragment.
fn encode_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for &b in name.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

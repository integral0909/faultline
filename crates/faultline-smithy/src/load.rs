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
    if entry.path().ends_with("/smithy-build.json") || entry.path() == "smithy-build.json" {
        read_build_config(&entry, loader, sources, &mut files);
        return files;
    }
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

/// Reads the model a `smithy-build.json` describes: the files named in its
/// `sources` and `imports`, and the `.smithy` and JSON files directly in the
/// directories named there. The file declaring a service comes first, as
/// the entry.
fn read_build_config(uri: &UriRef, loader: &dyn Loader, sources: &SourceMap, files: &mut Files) {
    let loaded = match loader.load(uri) {
        Ok(loaded) => loaded,
        Err(e) => {
            files.diagnostics.push(
                Diagnostic::error(format!("cannot read {uri}: {e}")).with_code(codes::UNRESOLVED),
            );
            return;
        }
    };
    let Ok(file) = sources.add(loaded.name, loaded.text) else {
        return;
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
    let paths = build_paths(&document.root, files);
    for model in model_files(&directory_of(uri), paths, loader, files) {
        if has_extension(model.path(), "json") {
            read_json(&model, loader, sources, files);
        } else {
            read(&model, loader, sources, files, false);
        }
    }
    // The file declaring a service is the entry.
    if let Some(i) = files.files.iter().position(|f| {
        f.model
            .shapes
            .iter()
            .any(|s| s.kind == crate::ast::ShapeKind::Service)
    }) {
        let entry = files.files.remove(i);
        files.files.insert(0, entry);
    }
}

/// The paths a build configuration lists in `sources` and `imports`.
fn build_paths(
    root: &faultline_doc::Node,
    files: &mut Files,
) -> Vec<(String, faultline_source::Span)> {
    let mut paths = Vec::new();
    for key in ["sources", "imports"] {
        for item in root.get(key).and_then(|n| n.as_array()).unwrap_or_default() {
            match item.as_str() {
                Some(path) => paths.push((path.to_owned(), item.span)),
                None => files.diagnostics.push(
                    Diagnostic::error(format!("`{key}` lists paths"))
                        .with_code(codes::SYNTAX)
                        .with_primary(item.span, ""),
                ),
            }
        }
    }
    if paths.is_empty() {
        files.diagnostics.push(
            Diagnostic::error(
                "smithy-build.json lists no model files in `sources` or `imports`".to_owned(),
            )
            .with_code(codes::UNRESOLVED)
            .with_primary(root.span, ""),
        );
    }
    paths
}

/// The model files `paths` name: files, and the model files directly in
/// directories.
fn model_files(
    base: &UriRef,
    paths: Vec<(String, faultline_source::Span)>,
    loader: &dyn Loader,
    files: &mut Files,
) -> Vec<UriRef> {
    let mut models = Vec::new();
    for (path, span) in paths {
        let Some(target) = UriRef::parse(path.trim_end_matches('/'))
            .ok()
            .and_then(|r| base.resolve_strict(&r))
        else {
            files.diagnostics.push(
                Diagnostic::error(format!("{path} is outside the workspace"))
                    .with_code(codes::UNRESOLVED)
                    .with_primary(span, ""),
            );
            continue;
        };
        if is_model_file(target.path()) {
            models.push(target);
            continue;
        }
        let directory = UriRef::parse(&format!("{target}/")).unwrap_or(target);
        let before = models.len();
        for name in loader.list(&directory).unwrap_or_default() {
            if is_model_file(&name)
                && let Ok(reference) = UriRef::parse(&encode_name(&name))
            {
                models.push(directory.resolve(&reference).normalized());
            }
        }
        if models.len() == before {
            files.diagnostics.push(
                Diagnostic::warning(format!("{path} holds no model files"))
                    .with_code(codes::UNSUPPORTED)
                    .with_primary(span, ""),
            );
        }
    }
    models
}

fn has_extension(path: &str, extension: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(extension))
}

/// Whether a file holds a model: IDL or a JSON AST.
fn is_model_file(path: &str) -> bool {
    has_extension(path, "smithy") || has_extension(path, "json")
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

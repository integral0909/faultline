//! Loading a `.proto` file and everything it imports.
//!
//! Import paths are relative to an import root, as with `protoc -I`. Without
//! configuration, the roots tried are the directory of the entry file and
//! each of its ancestors up to the workspace root, nearest first: an entry at
//! `proto/pets/v1/pets.proto` importing `pets/v1/common.proto` finds
//! `proto/pets/v1/common.proto`, which is what `-I proto` would find. The
//! well-known types under `google/protobuf/` are built in.

use std::collections::HashMap;
use std::fmt::Write as _;

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loader};
use faultline_source::{Diagnostic, Diagnostics, SourceId, SourceMap, SourceName, Span};

use crate::ast::File;
use crate::codes;
use crate::parser::parse;
use crate::wkt;

/// More files than this in one API is treated as hostile input.
pub const MAX_FILES: usize = 10_000;

/// A parsed file and where it came from.
#[derive(Clone, Debug)]
pub struct ProtoFile {
    /// The path other files import it by; for the entry file, its path in
    /// the workspace.
    pub name: String,
    pub source: SourceId,
    pub ast: File,
    /// One of the built-in well-known types.
    pub builtin: bool,
    /// Indices of the files this one imports, parallel to `ast.imports`
    /// (`None` where the import failed).
    pub imports: Vec<Option<usize>>,
}

/// The entry file (index 0) and its transitive imports.
#[derive(Debug, Default)]
pub struct Files {
    pub files: Vec<ProtoFile>,
    pub diagnostics: Diagnostics,
}

/// Loads `entry` and its imports. The entry must exist; a failure to load it
/// is reported without a location.
pub fn load(entry: &UriRef, loader: &dyn Loader, sources: &SourceMap) -> Files {
    let mut state = State {
        loader,
        sources,
        roots: import_roots(entry),
        files: Files::default(),
        by_key: HashMap::new(),
    };
    let Some(first) = state.read(entry, None) else {
        return state.files;
    };
    state.walk(first);
    state.files
}

struct State<'a> {
    loader: &'a dyn Loader,
    sources: &'a SourceMap,
    roots: Vec<UriRef>,
    files: Files,
    /// Resolved location (URI, or `builtin:` path) to file index.
    by_key: HashMap<String, usize>,
}

impl State<'_> {
    /// Loads and parses the file at `uri`.
    fn read(&mut self, uri: &UriRef, import: Option<(&str, Span)>) -> Option<usize> {
        let loaded = match self.loader.load(uri) {
            Ok(loaded) => loaded,
            Err(e) => {
                let mut d = Diagnostic::error(format!("cannot read {uri}: {e}"))
                    .with_code(codes::IMPORT_NOT_FOUND);
                if let Some((_, span)) = import {
                    d = d.with_primary(span, "imported here");
                }
                self.files.diagnostics.push(d);
                return None;
            }
        };
        let name = match (&loaded.name, import) {
            (_, Some((path, _))) => path.to_owned(),
            (SourceName::Path(path), None) => path
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
            (other, None) => other.to_string(),
        };
        self.add(uri.to_string(), name, loaded.name, loaded.text, false)
    }

    fn add(
        &mut self,
        key: String,
        name: String,
        source_name: SourceName,
        text: String,
        builtin: bool,
    ) -> Option<usize> {
        let file = match self.sources.add(source_name, text) {
            Ok(file) => file,
            Err(e) => {
                self.files
                    .diagnostics
                    .push(Diagnostic::error(e.to_string()).with_code(codes::IMPORT_NOT_FOUND));
                return None;
            }
        };
        let (ast, diagnostics) = parse(file.id(), file.text());
        self.files.diagnostics.extend(diagnostics);
        let index = self.files.files.len();
        self.files.files.push(ProtoFile {
            name,
            source: file.id(),
            imports: vec![None; ast.imports.len()],
            ast,
            builtin,
        });
        self.by_key.insert(key, index);
        Some(index)
    }

    /// Loads imports depth-first, reporting cycles. Iterative, so a long
    /// import chain cannot exhaust the stack.
    fn walk(&mut self, first: usize) {
        // (file, next import to visit)
        let mut stack: Vec<(usize, usize)> = vec![(first, 0)];
        let mut on_path: Vec<bool> = vec![true];
        let mut done: Vec<bool> = vec![false];
        while let Some(&mut (file, ref mut next)) = stack.last_mut() {
            let imports = &self.files.files[file].ast.imports;
            if *next >= imports.len() {
                on_path[file] = false;
                done[file] = true;
                stack.pop();
                continue;
            }
            let i = *next;
            *next += 1;
            let (path, span) = (imports[i].path.clone(), imports[i].span);
            let Some(target) = self.resolve(&path, span) else {
                continue;
            };
            self.files.files[file].imports[i] = Some(target);
            on_path.resize(self.files.files.len(), false);
            done.resize(self.files.files.len(), false);
            if on_path[target] {
                let cycle: Vec<String> = stack
                    .iter()
                    .skip_while(|(f, _)| *f != target)
                    .map(|(f, _)| self.files.files[*f].name.clone())
                    .chain(std::iter::once(self.files.files[target].name.clone()))
                    .collect();
                self.files.diagnostics.push(
                    Diagnostic::error(format!("import cycle: {}", cycle.join(" -> ")))
                        .with_code(codes::IMPORT_CYCLE)
                        .with_primary(span, "this import closes the cycle"),
                );
                self.files.files[file].imports[i] = None;
            } else if !done[target] {
                on_path[target] = true;
                stack.push((target, 0));
            }
        }
    }

    /// Finds, loads if new, and returns the file an import path names.
    fn resolve(&mut self, path: &str, span: Span) -> Option<usize> {
        if let Some(text) = wkt::builtin(path) {
            let key = format!("builtin:{path}");
            if let Some(&index) = self.by_key.get(&key) {
                return Some(index);
            }
            return self.add(
                key,
                path.to_owned(),
                SourceName::Virtual(path.to_owned()),
                text.to_owned(),
                true,
            );
        }
        if self.files.files.len() >= MAX_FILES {
            self.files.diagnostics.push(
                Diagnostic::error(format!("more than {MAX_FILES} files are imported"))
                    .with_code(codes::IMPORT_NOT_FOUND)
                    .with_primary(span, "while importing this"),
            );
            return None;
        }
        let Ok(reference) = UriRef::parse(&encode_import(path)) else {
            self.not_found(path, span, "the path is not valid");
            return None;
        };
        let mut failure = None;
        for root in &self.roots {
            let Some(candidate) = root.resolve_strict(&reference) else {
                failure = Some("it would leave the workspace".to_owned());
                continue;
            };
            let key = candidate.to_string();
            if let Some(&index) = self.by_key.get(&key) {
                return Some(index);
            }
            match self.loader.load(&candidate) {
                Ok(loaded) => {
                    return self.add(key, path.to_owned(), loaded.name, loaded.text, false);
                }
                Err(LoadError::NotFound) => {}
                Err(e) => {
                    failure = Some(e.to_string());
                    break;
                }
            }
        }
        let why = failure.unwrap_or_else(|| {
            format!(
                "searched {}",
                self.roots
                    .iter()
                    .map(display_root)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        });
        self.not_found(path, span, &why);
        None
    }

    fn not_found(&mut self, path: &str, span: Span, why: &str) {
        self.files.diagnostics.push(
            Diagnostic::error(format!("cannot find import \"{path}\""))
                .with_code(codes::IMPORT_NOT_FOUND)
                .with_primary(span, "imported here")
                .with_note(why.to_owned()),
        );
    }
}

/// The entry's directory and each ancestor up to the root, nearest first.
fn import_roots(entry: &UriRef) -> Vec<UriRef> {
    let path = entry.path();
    let mut roots = Vec::new();
    let mut end = path.rfind('/').map_or(0, |i| i + 1);
    loop {
        if let Ok(root) = UriRef::parse(&format!(
            "{}:{}{}",
            entry.scheme().unwrap_or("file"),
            entry
                .authority()
                .map(|a| format!("//{a}"))
                .unwrap_or_default(),
            &path[..end]
        )) {
            roots.push(root);
        }
        if end <= 1 {
            break;
        }
        end = path[..end - 1].rfind('/').map_or(0, |i| i + 1);
    }
    roots
}

fn display_root(root: &UriRef) -> String {
    let path = root.path().trim_start_matches('/');
    if path.is_empty() {
        "the workspace root".to_owned()
    } else {
        path.trim_end_matches('/').to_owned()
    }
}

/// An import path as a relative URI reference: characters that are special
/// in URIs are percent-encoded so that `a#b.proto` stays a file name.
fn encode_import(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/!$&'()*+,;=:@".contains(&b) {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_are_the_entry_directory_and_its_ancestors() {
        let roots: Vec<String> =
            import_roots(&UriRef::parse("file:///proto/pets/v1/pets.proto").unwrap())
                .iter()
                .map(ToString::to_string)
                .collect();
        assert_eq!(
            roots,
            [
                "file:///proto/pets/v1/",
                "file:///proto/pets/",
                "file:///proto/",
                "file:///"
            ]
        );
        let roots = import_roots(&UriRef::parse("file:///a.proto").unwrap());
        assert_eq!(roots.len(), 1);
        assert_eq!(display_root(&roots[0]), "the workspace root");
    }

    #[test]
    fn import_paths_are_encoded() {
        assert_eq!(encode_import("a b/c#d.proto"), "a%20b/c%23d.proto");
        assert_eq!(encode_import("pets/v1/x.proto"), "pets/v1/x.proto");
    }
}

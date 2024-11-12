//! Loading a `.thrift` file and everything it includes.
//!
//! An include path is resolved against the directory of the including
//! file, as the Thrift compiler does first, and then against the entry
//! file's directory, which is where `thrift -I` usually points. Each file
//! is read once, however often it is included, so files that include each
//! other are read without looping.

use std::collections::HashMap;

use faultline_doc::UriRef;
use faultline_doc::loader::{Loaded, Loader};
use faultline_source::{Diagnostic, Diagnostics, SourceId, SourceMap, Span};

use crate::ast::Document;
use crate::codes;
use crate::parser::parse;

/// More files than this in one API is treated as hostile input.
pub const MAX_FILES: usize = 10_000;

/// A parsed file and where it came from.
#[derive(Clone, Debug)]
pub struct ThriftFile {
    pub uri: UriRef,
    /// The name other files qualify its definitions with: the file name
    /// without `.thrift` (`shared` for `common/shared.thrift`).
    pub prefix: String,
    pub source: SourceId,
    pub document: Document,
    /// Indices of the files this one includes, parallel to
    /// `document.includes` (`None` where the include failed).
    pub includes: Vec<Option<usize>>,
}

/// The entry file (index 0) and its transitive includes.
#[derive(Debug, Default)]
pub struct Files {
    pub files: Vec<ThriftFile>,
    pub diagnostics: Diagnostics,
}

/// Loads `entry` and its includes. A failure to load the entry is
/// reported without a location.
pub fn load(entry: &UriRef, loader: &dyn Loader, sources: &SourceMap) -> Files {
    let mut state = State {
        loader,
        sources,
        entry: entry.clone(),
        files: Files::default(),
        by_uri: HashMap::new(),
    };
    if state.read(entry, None).is_some() {
        state.walk();
    }
    state.files
}

struct State<'a> {
    loader: &'a dyn Loader,
    sources: &'a SourceMap,
    entry: UriRef,
    files: Files,
    by_uri: HashMap<String, usize>,
}

impl State<'_> {
    fn error(&mut self, message: String, span: Option<Span>) {
        let mut diagnostic = Diagnostic::error(message).with_code(codes::INCLUDE_NOT_FOUND);
        if let Some(span) = span {
            diagnostic = diagnostic.with_primary(span, "included here");
        }
        self.files.diagnostics.push(diagnostic);
    }

    /// Loads and parses the file at `uri`, unless it already is.
    fn read(&mut self, uri: &UriRef, at: Option<Span>) -> Option<usize> {
        if let Some(&index) = self.by_uri.get(&key(uri)) {
            return Some(index);
        }
        match self.loader.load(uri) {
            Ok(loaded) => self.add(uri, loaded, at),
            Err(e) => {
                self.error(format!("cannot read {uri}: {e}"), at);
                None
            }
        }
    }

    fn add(&mut self, uri: &UriRef, loaded: Loaded, at: Option<Span>) -> Option<usize> {
        if self.files.files.len() >= MAX_FILES {
            self.error(format!("more than {MAX_FILES} files are included"), at);
            return None;
        }
        let file = match self.sources.add(loaded.name, loaded.text) {
            Ok(file) => file,
            Err(e) => {
                self.error(e.to_string(), at);
                return None;
            }
        };
        let (document, diagnostics) = parse(file.id(), file.text());
        self.files.diagnostics.extend(diagnostics);
        let index = self.files.files.len();
        self.files.files.push(ThriftFile {
            uri: uri.clone(),
            prefix: prefix(uri.path()),
            source: file.id(),
            document,
            includes: Vec::new(),
        });
        self.by_uri.insert(key(uri), index);
        Some(index)
    }

    /// Reads the includes of every file, including those they bring in.
    fn walk(&mut self) {
        let mut next = 0;
        while next < self.files.files.len() {
            let base = self.files.files[next].uri.clone();
            let includes = self.files.files[next].document.includes.clone();
            let resolved = includes
                .iter()
                .map(|include| self.include(&base, &include.path, include.span))
                .collect();
            self.files.files[next].includes = resolved;
            next += 1;
        }
    }

    /// The file an include names: next to the including file, or else
    /// next to the entry.
    fn include(&mut self, base: &UriRef, path: &str, span: Span) -> Option<usize> {
        let Ok(reference) = UriRef::parse(path) else {
            self.error(format!("{path:?} is not a valid path"), Some(span));
            return None;
        };
        let candidates = [base.resolve(&reference), self.entry.resolve(&reference)];
        if let Some(&index) = candidates.iter().find_map(|uri| self.by_uri.get(&key(uri))) {
            return Some(index);
        }
        let mut first_error = None;
        for uri in &candidates {
            match self.loader.load(uri) {
                Ok(loaded) => return self.add(uri, loaded, Some(span)),
                Err(e) => {
                    first_error.get_or_insert_with(|| format!("cannot read {uri}: {e}"));
                }
            }
        }
        self.error(first_error.expect("two candidates were tried"), Some(span));
        None
    }
}

fn key(uri: &UriRef) -> String {
    uri.normalized().to_string()
}

/// The name a file's definitions are qualified with.
fn prefix(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.strip_suffix(".thrift").unwrap_or(name);
    faultline_doc::uri::percent_decode(name).unwrap_or_else(|| name.to_owned())
}

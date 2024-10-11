//! Loading RAML documents, with `!include` expanded.
//!
//! A RAML document is YAML whose first line names the RAML version and
//! the kind of document (`#%RAML 1.0`, `#%RAML 1.0 Library`,
//! `#%RAML 1.0 DataType`, ...). `!include path` embeds another file: a RAML
//! or YAML file becomes its tree, a JSON file its parsed value, anything
//! else its text. Includes are resolved against the including file,
//! confined to the workspace, and expanded where they occur, so the rest of
//! the front end sees one tree whose nodes keep the spans of the files they
//! came from.

use std::collections::HashSet;

use faultline_doc::loader::Loader;
use faultline_doc::{Format, Node, UriRef, Value};
use faultline_source::{Diagnostic, Diagnostics, Limits, SourceMap, Span};

use crate::codes;

/// The tag that embeds another file.
pub const INCLUDE: &str = "!include";

/// Includes nest at most this deep.
const MAX_INCLUDE_DEPTH: usize = 32;
/// At most this many files are included for one API.
const MAX_INCLUDES: usize = 1000;

/// A loaded RAML document.
#[derive(Clone, Debug)]
pub struct Document {
    /// The fragment kind from the header: `""` for an API, `"Library"`,
    /// `"DataType"`, ...
    pub kind: String,
    pub root: Node,
    pub uri: UriRef,
}

/// Loads RAML documents through the workspace loader.
pub struct RamlLoader<'a> {
    loader: &'a dyn Loader,
    sources: &'a SourceMap,
    limits: Limits,
    /// Documents being expanded, to report include cycles.
    stack: Vec<String>,
    includes: usize,
    pub diagnostics: Diagnostics,
}

impl std::fmt::Debug for RamlLoader<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RamlLoader")
            .field("stack", &self.stack)
            .finish_non_exhaustive()
    }
}

impl<'a> RamlLoader<'a> {
    pub fn new(loader: &'a dyn Loader, sources: &'a SourceMap, limits: Limits) -> Self {
        Self {
            loader,
            sources,
            limits,
            stack: Vec::new(),
            includes: 0,
            diagnostics: Diagnostics::new(),
        }
    }

    fn error(
        &mut self,
        code: faultline_source::DiagnosticCode,
        message: String,
        span: Option<Span>,
    ) {
        let mut diagnostic = Diagnostic::error(message).with_code(code);
        if let Some(span) = span {
            diagnostic = diagnostic.with_primary(span, "");
        }
        self.diagnostics.push(diagnostic);
    }

    /// Loads a RAML document (the API's root, or a library) with its
    /// includes expanded.
    pub fn document(&mut self, uri: &UriRef, at: Option<Span>) -> Option<Document> {
        let file = self.read(uri, at)?;
        let text = file.text();
        let header = text.lines().next().unwrap_or_default().trim();
        let Some(rest) = header.strip_prefix("#%RAML ") else {
            let span = Span::new(file.id(), 0, u32::try_from(header.len()).unwrap_or(0));
            self.error(
                codes::NOT_RAML,
                format!("{uri} is not RAML: its first line must be `#%RAML 1.0`"),
                Some(span),
            );
            return None;
        };
        let mut words = rest.split_whitespace();
        let version = words.next().unwrap_or_default();
        if version != "1.0" {
            let span = Span::new(file.id(), 0, u32::try_from(header.len()).unwrap_or(0));
            self.error(
                codes::UNSUPPORTED_VERSION,
                format!("RAML {version} is not supported; RAML 1.0 is"),
                Some(span),
            );
            return None;
        }
        let kind = words.collect::<Vec<_>>().join(" ");
        let root = self.yaml(&file, uri)?;
        Some(Document {
            kind,
            root,
            uri: uri.clone(),
        })
    }

    fn read(
        &mut self,
        uri: &UriRef,
        at: Option<Span>,
    ) -> Option<std::sync::Arc<faultline_source::SourceFile>> {
        let loaded = match self.loader.load(uri) {
            Ok(loaded) => loaded,
            Err(e) => {
                self.error(codes::INCLUDE, format!("cannot load {uri}: {e}"), at);
                return None;
            }
        };
        if let Err(e) = self.limits.check_file_size(loaded.text.len() as u64) {
            self.error(codes::INCLUDE, format!("{uri}: {e}"), at);
            return None;
        }
        match self.sources.add(loaded.name, loaded.text) {
            Ok(file) => Some(file),
            Err(e) => {
                self.error(codes::INCLUDE, e.to_string(), at);
                None
            }
        }
    }

    /// Parses a YAML file and expands its includes.
    fn yaml(&mut self, file: &faultline_source::SourceFile, uri: &UriRef) -> Option<Node> {
        let key = uri.without_fragment().normalized().to_string();
        if self.stack.contains(&key) {
            return None;
        }
        let (parsed, kept) = faultline_doc::parse_yaml_keeping(file, &self.limits, &[INCLUDE]);
        self.diagnostics.extend(parsed.diagnostics);
        let mut root = parsed.document?.root;
        let includes: HashSet<Span> = kept.iter().map(|t| t.span).collect();
        self.stack.push(key);
        self.expand(&mut root, &includes, uri);
        self.stack.pop();
        Some(root)
    }

    /// Replaces included scalars by what they include. The tree's depth is
    /// bounded by the YAML reader's limit.
    fn expand(&mut self, node: &mut Node, includes: &HashSet<Span>, base: &UriRef) {
        match &mut node.value {
            Value::String(path) if includes.contains(&node.span) => {
                let path = path.clone();
                if let Some(included) = self.include(&path, base, node.span) {
                    *node = included;
                }
            }
            Value::Array(items) => {
                for item in items {
                    self.expand(item, includes, base);
                }
            }
            Value::Object(object) => {
                for (_, value) in object.iter_mut() {
                    self.expand(value, includes, base);
                }
            }
            _ => {}
        }
    }

    /// The node an `!include` stands for.
    fn include(&mut self, path: &str, base: &UriRef, span: Span) -> Option<Node> {
        self.includes += 1;
        if self.includes > MAX_INCLUDES {
            self.error(
                codes::INCLUDE,
                format!("more than {MAX_INCLUDES} included files"),
                Some(span),
            );
            return None;
        }
        if self.stack.len() > MAX_INCLUDE_DEPTH {
            self.error(
                codes::INCLUDE,
                format!("includes nest more than {MAX_INCLUDE_DEPTH} deep"),
                Some(span),
            );
            return None;
        }
        let target = self.resolve(path.trim(), base, span)?;
        let key = target.without_fragment().normalized().to_string();
        if self.stack.contains(&key) {
            self.error(
                codes::INCLUDE,
                format!("{path} includes itself, directly or through other files"),
                Some(span),
            );
            return None;
        }
        let file = self.read(&target, Some(span))?;
        let extension = target
            .path()
            .rsplit('.')
            .next()
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match extension.as_str() {
            "raml" | "yaml" | "yml" => self.yaml(&file, &target),
            "json" => {
                let parsed = faultline_doc::parse(&file, Format::Json, &self.limits);
                self.diagnostics.extend(parsed.diagnostics);
                parsed.document.map(|d| d.root)
            }
            _ => Some(Node::new(
                Value::String(file.text().to_owned()),
                file.full_span(),
            )),
        }
    }

    /// Resolves a path against the including document, inside the
    /// workspace.
    pub(crate) fn resolve(&mut self, path: &str, base: &UriRef, span: Span) -> Option<UriRef> {
        let Ok(reference) = UriRef::parse(path) else {
            self.error(
                codes::INCLUDE,
                format!("{path} is not a valid path"),
                Some(span),
            );
            return None;
        };
        let is_file = base
            .scheme()
            .is_some_and(|s| s.eq_ignore_ascii_case("file"));
        if is_file && !reference.has_scheme() {
            if let Some(target) = base.resolve_strict(&reference) {
                return Some(target);
            }
            self.error(
                codes::INCLUDE,
                format!("{path} is outside the workspace root"),
                Some(span),
            );
            return None;
        }
        Some(base.resolve(&reference))
    }
}

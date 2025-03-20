use std::collections::HashMap;
use std::sync::Arc;

use faultline_source::{Diagnostic, Diagnostics, Limits, SourceMap, SourceName};

use crate::codes;
use crate::loader::{LoadError, Loader};
use crate::uri::UriRef;
use crate::{Document, Format, parse};

/// A parsed document together with the absolute URI it was loaded from.
#[derive(Debug)]
pub struct StoredDocument {
    /// Normalized, without fragment.
    pub uri: UriRef,
    pub document: Document,
}

/// Why a document is unavailable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    /// The loader could not provide the text.
    Load(LoadError),
    /// The text was loaded but did not parse; the parse diagnostics are in
    /// the store's diagnostics.
    Parse,
}

/// Loads, parses and caches documents by URI for one lowering session.
///
/// Each document is loaded and parsed at most once, however many references
/// point into it, and failures are cached too so that a missing file referenced
/// a hundred times produces one load attempt. Parse diagnostics accumulate in
/// the store; load failures are returned to the caller, which knows which
/// reference caused them and reports them there.
pub struct DocumentStore<'a> {
    loader: &'a dyn Loader,
    sources: &'a SourceMap,
    limits: Limits,
    allow_remote: bool,
    documents: HashMap<UriRef, Result<Arc<StoredDocument>, StoreError>>,
    diagnostics: Diagnostics,
}

impl<'a> DocumentStore<'a> {
    pub fn new(loader: &'a dyn Loader, sources: &'a SourceMap, limits: Limits) -> Self {
        Self {
            loader,
            sources,
            limits,
            allow_remote: false,
            documents: HashMap::new(),
            diagnostics: Diagnostics::new(),
        }
    }

    /// Permits `http`/`https` documents to be requested from the loader.
    /// Off by default; the loader must itself be able to fetch them.
    pub fn allow_remote(&mut self, allow: bool) {
        self.allow_remote = allow;
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    pub fn sources(&self) -> &SourceMap {
        self.sources
    }

    /// The loader documents are read with.
    pub fn loader(&self) -> &'a dyn Loader {
        self.loader
    }

    /// The document at `uri` (fragment ignored), loading it on first use.
    pub fn get(&mut self, uri: &UriRef) -> Result<Arc<StoredDocument>, StoreError> {
        let key = uri.without_fragment().normalized();
        if let Some(cached) = self.documents.get(&key) {
            return cached.clone();
        }
        // Network access is a policy decision, not a property of a loader:
        // refuse network schemes here so that no loader can bypass it.
        if !self.allow_remote
            && let Some(scheme) = key.scheme().filter(|s| matches!(*s, "http" | "https"))
        {
            let result = Err(StoreError::Load(LoadError::RemoteDisabled {
                scheme: scheme.to_owned(),
            }));
            self.documents.insert(key, result.clone());
            return result;
        }
        let result = match self.loader.load(&key) {
            Ok(loaded) => self.parse_text(key.clone(), &loaded.name, loaded.text),
            Err(e) => Err(StoreError::Load(e)),
        };
        self.documents.insert(key, result.clone());
        result
    }

    /// Registers text under `uri` without going through the loader, for
    /// documents that were obtained some other way (standard input, tests).
    pub fn insert_text(
        &mut self,
        uri: &UriRef,
        name: &SourceName,
        text: String,
    ) -> Result<Arc<StoredDocument>, StoreError> {
        let key = uri.without_fragment().normalized();
        let result = self.parse_text(key.clone(), name, text);
        self.documents.insert(key, result.clone());
        result
    }

    /// Replaces the tree of the document at `uri`, loading it first, for
    /// callers that change a document before it is read (applying
    /// overlays). Later reads see the new tree.
    pub fn replace_root(&mut self, uri: &UriRef, root: crate::Node) -> Result<(), StoreError> {
        let stored = self.get(uri)?;
        let key = uri.without_fragment().normalized();
        let document = Document {
            root,
            format: stored.document.format,
            source: stored.document.source,
        };
        self.documents.insert(
            key,
            Ok(Arc::new(StoredDocument {
                uri: stored.uri.clone(),
                document,
            })),
        );
        Ok(())
    }

    /// Diagnostics raised while parsing documents so far.
    pub fn take_diagnostics(&mut self) -> Diagnostics {
        std::mem::take(&mut self.diagnostics)
    }

    fn parse_text(
        &mut self,
        uri: UriRef,
        name: &SourceName,
        text: String,
    ) -> Result<Arc<StoredDocument>, StoreError> {
        let file = match self.sources.add(name.clone(), text) {
            Ok(file) => file,
            Err(e) => {
                self.diagnostics.push(
                    Diagnostic::error(format!("cannot read {name}: {e}"))
                        .with_code(codes::FILE_TOO_LARGE),
                );
                return Err(StoreError::Parse);
            }
        };
        let format = format_for(&uri).unwrap_or_else(|| Format::sniff(file.text()));
        let parsed = parse(&file, format, &self.limits);
        self.diagnostics.extend(parsed.diagnostics);
        let document = parsed.document.ok_or(StoreError::Parse)?;
        Ok(Arc::new(StoredDocument { uri, document }))
    }
}

impl std::fmt::Debug for DocumentStore<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentStore")
            .field("documents", &self.documents.len())
            .finish_non_exhaustive()
    }
}

/// The format implied by a URI's file extension.
fn format_for(uri: &UriRef) -> Option<Format> {
    let last = uri.path().rsplit('/').next()?;
    let (_, extension) = last.rsplit_once('.')?;
    Format::from_extension(extension)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::MemoryLoader;

    fn u(s: &str) -> UriRef {
        UriRef::parse(s).unwrap()
    }

    #[test]
    fn documents_are_parsed_once_and_cached_including_failures() {
        let loader = MemoryLoader::new()
            .with("file:///a.yaml", "x: 1\n")
            .with("file:///bad.json", "{");
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, Limits::default());

        let first = store.get(&u("file:///a.yaml#/x")).unwrap();
        let second = store.get(&u("file:///./a.yaml")).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(sources.len(), 1);

        assert_eq!(
            store.get(&u("file:///bad.json")).unwrap_err(),
            StoreError::Parse
        );
        assert_eq!(
            store.get(&u("file:///bad.json")).unwrap_err(),
            StoreError::Parse
        );
        assert_eq!(
            store.take_diagnostics().len(),
            1,
            "parse error reported once"
        );

        assert_eq!(
            store.get(&u("file:///missing.json")).unwrap_err(),
            StoreError::Load(LoadError::NotFound)
        );
    }

    #[test]
    fn remote_documents_are_refused_unless_allowed() {
        let loader = MemoryLoader::new().with("https://example.com/s.json", "{}");
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, Limits::default());
        assert_eq!(
            store.get(&u("https://example.com/s.json")).unwrap_err(),
            StoreError::Load(LoadError::RemoteDisabled {
                scheme: "https".into()
            })
        );
        let mut store = DocumentStore::new(&loader, &sources, Limits::default());
        store.allow_remote(true);
        assert!(store.get(&u("https://example.com/s.json")).is_ok());
    }

    #[test]
    fn format_follows_extension_then_content() {
        let loader = MemoryLoader::new()
            .with("file:///x.json", "{\"a\": 1}")
            .with("file:///schema", "{\"a\": 1}")
            .with("file:///plain", "a: 1\n");
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, Limits::default());
        for (uri, format) in [
            ("file:///x.json", Format::Json),
            ("file:///schema", Format::Json),
            ("file:///plain", Format::Yaml),
        ] {
            assert_eq!(store.get(&u(uri)).unwrap().document.format, format, "{uri}");
        }
    }
}

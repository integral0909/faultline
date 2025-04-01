use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, PoisonError, RwLock};

use crate::file::{SourceFile, SourceName};
use crate::span::{SourceId, Span};

/// Errors raised when registering a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// The text is too long to be addressed with 32-bit offsets.
    TooLarge { name: SourceName, len: usize },
    /// The map already holds `u32::MAX` files.
    TooManyFiles,
}

impl fmt::Display for SourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceError::TooLarge { name, len } => {
                write!(
                    f,
                    "{name} is {len} bytes, which exceeds the 4 GiB source limit"
                )
            }
            SourceError::TooManyFiles => f.write_str("too many source files in one session"),
        }
    }
}

impl std::error::Error for SourceError {}

/// Owns every source file loaded during a session.
///
/// Front ends load and parse files in parallel, so registration takes a short
/// write lock; lookups dominate and share a read lock. Files are never removed,
/// which keeps every issued [`SourceId`] and [`Span`] valid for the lifetime of
/// the map.
#[derive(Default)]
pub struct SourceMap {
    files: RwLock<Vec<Arc<SourceFile>>>,
    /// For text embedded in another file (a schema written as a string in
    /// a YAML document), the span of that string.
    hosts: RwLock<HashMap<SourceId, Span>>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a file and returns a handle to it.
    ///
    /// Adding the same name twice creates two distinct files. Callers that load
    /// by name are expected to cache; the map itself stays a plain arena so
    /// that two revisions of the same path can coexist.
    pub fn add(
        &self,
        name: SourceName,
        text: impl Into<Arc<str>>,
    ) -> Result<Arc<SourceFile>, SourceError> {
        let text = text.into();
        if u32::try_from(text.len()).is_err() {
            return Err(SourceError::TooLarge {
                name,
                len: text.len(),
            });
        }
        let mut files = self.files.write().unwrap_or_else(PoisonError::into_inner);
        let id = u32::try_from(files.len()).map_err(|_| SourceError::TooManyFiles)?;
        let file = Arc::new(SourceFile::new(SourceId::from_raw(id), name, text));
        files.push(Arc::clone(&file));
        Ok(file)
    }

    /// Records that the text of `id` was embedded in another file at `host`,
    /// so that integrations which can only point at files on disk point
    /// there.
    pub fn set_host(&self, id: SourceId, host: Span) {
        if host.source() != id {
            self.hosts
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, host);
        }
    }

    /// The span a file's text was embedded at, if it was embedded.
    pub fn host(&self, id: SourceId) -> Option<Span> {
        self.hosts
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .copied()
    }

    /// The outermost span `span` was embedded in: `span` itself for text
    /// read from a file.
    pub fn outermost(&self, mut span: Span) -> Span {
        // Embedding is shallow in practice; the bound guards against cycles.
        for _ in 0..8 {
            match self.host(span.source()) {
                Some(host) => span = host,
                None => break,
            }
        }
        span
    }

    pub fn get(&self, id: SourceId) -> Option<Arc<SourceFile>> {
        let files = self.files.read().unwrap_or_else(PoisonError::into_inner);
        files.get(id.index()).cloned()
    }

    /// The text covered by `span`.
    pub fn slice(&self, span: Span) -> Option<String> {
        let file = self.get(span.source())?;
        file.slice(span).map(str::to_owned)
    }

    /// Every file registered so far, in registration order.
    pub fn files(&self) -> Vec<Arc<SourceFile>> {
        self.files
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn len(&self) -> usize {
        self.files
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl fmt::Debug for SourceMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let files = self.files.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_list()
            .entries(files.iter().map(|file| file.name()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn embedded_text_resolves_to_its_host() {
        let map = SourceMap::new();
        let yaml = map
            .add(
                SourceName::Virtual("doc.yaml".into()),
                "schema: |\n  message A {}\n",
            )
            .unwrap();
        let proto = map
            .add(
                SourceName::Virtual("doc.yaml#/schema".into()),
                "message A {}\n",
            )
            .unwrap();
        let host = Span::new(yaml.id(), 8, 26);
        map.set_host(proto.id(), host);
        let inner = Span::new(proto.id(), 8, 9);
        assert_eq!(map.host(proto.id()), Some(host));
        assert_eq!(map.outermost(inner), host);
        assert_eq!(
            map.outermost(host),
            host,
            "files on disk are their own host"
        );
        // A file cannot host itself.
        map.set_host(yaml.id(), Span::new(yaml.id(), 0, 1));
        assert_eq!(map.host(yaml.id()), None);
    }

    #[test]
    fn ids_are_dense_and_resolvable() {
        let map = SourceMap::new();
        let a = map.add(SourceName::Virtual("a".into()), "alpha").unwrap();
        let b = map.add(SourceName::Virtual("b".into()), "beta").unwrap();
        assert_eq!(a.id().as_raw(), 0);
        assert_eq!(b.id().as_raw(), 1);
        assert_eq!(map.get(b.id()).unwrap().text(), "beta");
        assert!(map.get(SourceId::from_raw(2)).is_none());
    }

    #[test]
    fn slices_respect_file_boundaries() {
        let map = SourceMap::new();
        let a = map.add(SourceName::Stdin, "hello world").unwrap();
        assert_eq!(
            map.slice(Span::new(a.id(), 6, 11)).as_deref(),
            Some("world")
        );
        assert_eq!(map.slice(Span::new(a.id(), 6, 40)), None);
        assert_eq!(a.slice(Span::new(SourceId::from_raw(9), 0, 1)), None);
    }

    #[test]
    fn concurrent_registration_issues_unique_ids() {
        let map = SourceMap::new();
        thread::scope(|scope| {
            for t in 0..8 {
                let map = &map;
                scope.spawn(move || {
                    for i in 0..50 {
                        let name = SourceName::Virtual(format!("{t}-{i}"));
                        let text = format!("{t}-{i}");
                        let file = map.add(name, text.as_str()).unwrap();
                        assert_eq!(map.get(file.id()).unwrap().text(), text);
                    }
                });
            }
        });
        assert_eq!(map.len(), 400);
    }
}

//! Unsaved editor text layered over the files on disk.

use std::collections::HashMap;
use std::path::Path;

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loaded, Loader, display_name, workspace_uri};

/// Serves the text of open documents in place of the files underneath, so
/// a comparison sees what the editor shows rather than what was last saved.
pub(crate) struct OverlayLoader<'a> {
    inner: &'a dyn Loader,
    /// Text by normalized workspace URI.
    documents: HashMap<String, &'a str>,
}

impl<'a> OverlayLoader<'a> {
    pub(crate) fn new(inner: &'a dyn Loader) -> Self {
        Self {
            inner,
            documents: HashMap::new(),
        }
    }

    /// Serves `text` for the workspace file at `relative`.
    pub(crate) fn insert(&mut self, relative: &Path, text: &'a str) {
        self.documents.insert(key(&workspace_uri(relative)), text);
    }
}

fn key(uri: &UriRef) -> String {
    uri.without_fragment().normalized().to_string()
}

impl Loader for OverlayLoader<'_> {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        match self.documents.get(&key(uri)) {
            Some(text) => Ok(Loaded {
                name: display_name(uri),
                text: (*text).to_owned(),
            }),
            None => self.inner.load(uri),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use faultline_doc::loader::MemoryLoader;

    #[test]
    fn open_documents_shadow_the_disk() {
        let disk = MemoryLoader::new()
            .with("file:///api/a.yaml", "saved")
            .with("file:///api/b.yaml", "other");
        let mut overlay = OverlayLoader::new(&disk);
        overlay.insert(Path::new("api/a.yaml"), "unsaved");
        overlay.insert(Path::new("api/new.yaml"), "created");
        let load = |uri: &str| overlay.load(&UriRef::parse(uri).unwrap()).map(|l| l.text);
        assert_eq!(load("file:///api/a.yaml#/paths").unwrap(), "unsaved");
        assert_eq!(load("file:///api/./b.yaml").unwrap(), "other");
        assert_eq!(load("file:///api/new.yaml").unwrap(), "created");
        assert_eq!(load("file:///api/c.yaml"), Err(LoadError::NotFound));
    }
}

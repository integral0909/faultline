//! Loading referenced documents by URI.
//!
//! Front ends follow references (`$ref`, imports) by absolute URI and ask a
//! [`Loader`] for the text. Loaders are where the security boundary on file
//! access lives:
//!
//! * [`FsLoader`] serves `file:` URIs whose path is relative to a workspace
//!   root (`file:///api/pet.yaml` is `<root>/api/pet.yaml`). It refuses paths
//!   that resolve outside the root, including through symbolic links, and
//!   enforces the file-size limit before reading.
//! * [`MemoryLoader`] serves fixed contents, for tests and for text that did
//!   not come from disk.
//!
//! Neither loader fetches from the network. Remote references are reported as
//! [`LoadError::RemoteDisabled`] unless remote access is enabled, in which
//! case a [`SchemeRouter`] sends them to a separate, policy-enforcing loader.

use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use faultline_source::{DecodeError, LimitExceeded, Limits, SourceName, decode_text};

use crate::uri::{UriRef, percent_decode};

/// Decoded text and a display name for a loaded document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Loaded {
    pub name: SourceName,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    NotFound,
    /// The URI names a location outside the workspace root.
    OutsideRoot,
    /// The URI uses a network scheme and network access is not enabled.
    RemoteDisabled {
        scheme: String,
    },
    /// Network policy forbids fetching the URI (a host not on the allowlist,
    /// plain HTTP, a private address, ...).
    Refused(String),
    /// Fetching a remote document failed.
    Network(String),
    /// The URI uses a scheme this loader cannot serve.
    UnsupportedScheme {
        scheme: Option<String>,
    },
    TooLarge(LimitExceeded),
    Decode(DecodeError),
    Io(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::NotFound => f.write_str("file not found"),
            LoadError::OutsideRoot => f.write_str("the location is outside the workspace root"),
            LoadError::RemoteDisabled { scheme } => {
                write!(
                    f,
                    "{scheme} references are disabled; enable remote references in [network]"
                )
            }
            LoadError::UnsupportedScheme { scheme: Some(s) } => {
                write!(f, "unsupported URI scheme '{s}'")
            }
            LoadError::UnsupportedScheme { scheme: None } => {
                f.write_str("the reference has no scheme")
            }
            LoadError::Refused(reason) => write!(f, "refused by the network policy: {reason}"),
            LoadError::TooLarge(e) => e.fmt(f),
            LoadError::Decode(e) => e.fmt(f),
            LoadError::Network(message) | LoadError::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for LoadError {}

/// A source of documents addressed by absolute URI.
pub trait Loader: Send + Sync {
    /// Loads the document at `uri`. The fragment, if any, is ignored.
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError>;
}

/// Sends `http`/`https` URIs to a remote loader, when there is one, and
/// everything else to the workspace loader.
pub struct SchemeRouter<'a> {
    pub workspace: &'a dyn Loader,
    pub remote: Option<&'a dyn Loader>,
}

impl fmt::Debug for SchemeRouter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SchemeRouter")
            .field("remote", &self.remote.is_some())
            .finish_non_exhaustive()
    }
}

impl Loader for SchemeRouter<'_> {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        match (uri.scheme(), self.remote) {
            (Some(scheme), Some(remote)) if is_network_scheme(scheme) => remote.load(uri),
            _ => self.workspace.load(uri),
        }
    }
}

/// Serves documents from a fixed map of URI to text.
#[derive(Clone, Debug, Default)]
pub struct MemoryLoader {
    documents: HashMap<String, String>,
}

impl MemoryLoader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a document. `uri` is normalized, so equivalent spellings match.
    pub fn insert(&mut self, uri: &str, text: impl Into<String>) {
        let key = UriRef::parse(uri).map_or_else(
            |_| uri.to_owned(),
            |u| u.without_fragment().normalized().to_string(),
        );
        self.documents.insert(key, text.into());
    }

    #[must_use]
    pub fn with(mut self, uri: &str, text: impl Into<String>) -> Self {
        self.insert(uri, text);
        self
    }
}

impl Loader for MemoryLoader {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        let key = uri.without_fragment().normalized().to_string();
        self.documents
            .get(&key)
            .map(|text| Loaded {
                name: display_name(uri),
                text: text.clone(),
            })
            .ok_or(LoadError::NotFound)
    }
}

/// Serves workspace files from disk, confined to a root directory.
#[derive(Clone, Debug)]
pub struct FsLoader {
    root: PathBuf,
    limits: Limits,
}

impl FsLoader {
    /// Creates a loader for the workspace at `root`, which must exist.
    pub fn new(root: impl AsRef<Path>, limits: Limits) -> Result<Self, LoadError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|e| LoadError::Io(format!("cannot open workspace root: {e}")))?;
        Ok(Self { root, limits })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The workspace URI for a filesystem path, which may be absolute (inside
    /// the root) or relative to the root.
    pub fn uri_for(&self, path: &Path) -> Result<UriRef, LoadError> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            // The canonical root may be a verbatim path (`\\?\C:\...` on
            // Windows), where `/` is not a separator; rebuilding the relative
            // path from its components uses the native one.
            self.root.join(path.components().collect::<PathBuf>())
        };
        let canonical = absolute.canonicalize().map_err(|e| io_error(&e))?;
        let relative = canonical
            .strip_prefix(&self.root)
            .map_err(|_| LoadError::OutsideRoot)?;
        Ok(workspace_uri(relative))
    }

    fn path_for(&self, uri: &UriRef) -> Result<PathBuf, LoadError> {
        let mut path = self.root.clone();
        for segment in workspace_path(uri)? {
            path.push(segment);
        }
        Ok(path)
    }
}

/// The path segments a workspace `file:` URI names, relative to the
/// workspace root: `file:///api/pet%20v1.yaml` is `["api", "pet v1.yaml"]`.
///
/// This is the check every workspace loader shares: other schemes, `file:`
/// URIs with an authority and paths that climb out of the root are refused.
pub fn workspace_path(uri: &UriRef) -> Result<Vec<String>, LoadError> {
    match uri.scheme() {
        Some(s) if s.eq_ignore_ascii_case("file") => {}
        Some(s) if is_network_scheme(s) => {
            return Err(LoadError::RemoteDisabled {
                scheme: s.to_ascii_lowercase(),
            });
        }
        other => {
            return Err(LoadError::UnsupportedScheme {
                scheme: other.map(str::to_owned),
            });
        }
    }
    if uri.authority().is_some_and(|a| !a.is_empty()) {
        return Err(LoadError::UnsupportedScheme {
            scheme: Some("file".into()),
        });
    }
    let decoded = percent_decode(uri.path()).ok_or(LoadError::NotFound)?;
    let mut segments = Vec::new();
    for component in Path::new(decoded.trim_start_matches('/')).components() {
        match component {
            Component::Normal(part) => segments.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            // Dot segments were removed during URI resolution; anything left
            // (or a drive prefix on Windows) is not a workspace path.
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(LoadError::OutsideRoot);
            }
        }
    }
    Ok(segments)
}

impl Loader for FsLoader {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        let path = self.path_for(uri)?;
        // Canonicalize to resolve symbolic links, then check containment:
        // a link inside the workspace must not expose files outside it.
        let canonical = path.canonicalize().map_err(|e| io_error(&e))?;
        if !canonical.starts_with(&self.root) {
            return Err(LoadError::OutsideRoot);
        }
        let file = std::fs::File::open(&canonical).map_err(|e| io_error(&e))?;
        let metadata = file.metadata().map_err(|e| io_error(&e))?;
        if !metadata.is_file() {
            return Err(LoadError::NotFound);
        }
        self.limits
            .check_file_size(metadata.len())
            .map_err(LoadError::TooLarge)?;
        // Read at most one byte past the limit so a file that grows between
        // the metadata check and the read is still caught.
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
        file.take(self.limits.max_file_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|e| io_error(&e))?;
        self.limits
            .check_file_size(bytes.len() as u64)
            .map_err(LoadError::TooLarge)?;
        let text = decode_text(&bytes).map_err(LoadError::Decode)?;
        let relative = canonical
            .strip_prefix(&self.root)
            .expect("containment checked above");
        Ok(Loaded {
            name: SourceName::Path(relative.to_path_buf()),
            text: text.to_owned(),
        })
    }
}

/// The `file:` URI for a path relative to the workspace root.
pub fn workspace_uri(relative: &Path) -> UriRef {
    let mut path = String::new();
    for component in relative.components() {
        if let Component::Normal(part) = component {
            path.push('/');
            path.push_str(&encode_path_segment(&part.to_string_lossy()));
        }
    }
    if path.is_empty() {
        path.push('/');
    }
    UriRef::parse(&format!("file://{path}")).expect("encoded paths contain no control characters")
}

fn encode_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for &b in segment.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Whether `scheme` is fetched over the network (`http`, `https`).
pub fn is_network_scheme(scheme: &str) -> bool {
    ["http", "https"]
        .iter()
        .any(|s| s.eq_ignore_ascii_case(scheme))
}

fn io_error(e: &std::io::Error) -> LoadError {
    match e.kind() {
        std::io::ErrorKind::NotFound => LoadError::NotFound,
        _ => LoadError::Io(e.to_string()),
    }
}

/// A readable name for a URI: workspace paths without the scheme, anything
/// else verbatim.
pub fn display_name(uri: &UriRef) -> SourceName {
    let without_fragment = uri.without_fragment();
    match uri.scheme() {
        Some(s) if s.eq_ignore_ascii_case("file") && uri.authority().is_none_or(str::is_empty) => {
            let decoded = percent_decode(uri.path()).unwrap_or_else(|| uri.path().to_owned());
            SourceName::Path(PathBuf::from(decoded.trim_start_matches('/')))
        }
        _ => SourceName::Url(without_fragment.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> UriRef {
        UriRef::parse(s).unwrap()
    }

    #[test]
    fn memory_loader_matches_normalized_uris() {
        let loader = MemoryLoader::new().with("file:///a/b.json", "{}");
        assert_eq!(loader.load(&u("FILE:///a/./b.json#/x")).unwrap().text, "{}");
        assert_eq!(
            loader.load(&u("file:///a/c.json")),
            Err(LoadError::NotFound)
        );
    }

    #[test]
    fn workspace_uris_encode_special_characters() {
        let uri = workspace_uri(Path::new("api specs/pet#1.yaml"));
        assert_eq!(uri.to_string(), "file:///api%20specs/pet%231.yaml");
        assert_eq!(
            display_name(&uri),
            SourceName::Path(PathBuf::from("api specs/pet#1.yaml"))
        );
    }

    #[test]
    fn routers_send_network_schemes_to_the_remote_loader() {
        let workspace = MemoryLoader::new().with("file:///a.json", "local");
        let remote = MemoryLoader::new().with("https://example.com/a.json", "remote");
        let router = SchemeRouter {
            workspace: &workspace,
            remote: Some(&remote),
        };
        assert_eq!(router.load(&u("file:///a.json")).unwrap().text, "local");
        assert_eq!(
            router.load(&u("HTTPS://example.com/a.json")).unwrap().text,
            "remote"
        );
        let offline = SchemeRouter {
            workspace: &workspace,
            remote: None,
        };
        assert_eq!(
            offline.load(&u("https://example.com/a.json")),
            Err(LoadError::NotFound)
        );
    }

    #[test]
    fn display_names_for_remote_uris_drop_fragments() {
        assert_eq!(
            display_name(&u("https://example.com/s.json#/defs")),
            SourceName::Url("https://example.com/s.json".into())
        );
    }
}

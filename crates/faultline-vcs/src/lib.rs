//! Reading API descriptions from git revisions.
//!
//! Comparing a pull request with its base branch should not need a second
//! checkout. [`Repository`] resolves revisions (and merge bases) in the
//! repository containing a workspace, and [`GitLoader`] serves the
//! workspace's `file:` URIs straight from a revision's object database.
//!
//! The loader keeps the guarantees of the filesystem loader: every document
//! comes from inside the workspace root, and the size limit is checked
//! against the object header before a blob is read. It is stricter in two
//! ways, because a revision has no filesystem to resolve against: symbolic
//! links are refused rather than followed, and submodules are not entered.

mod paths;

use std::fmt;
use std::path::{Path, PathBuf};

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loaded, Loader, workspace_path};
use faultline_source::{Limits, SourceName, decode_text};
use gix::ObjectId;

pub use paths::normalize_lexically;

/// Why a repository or revision could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VcsError {
    /// No repository contains the path.
    NotARepository { path: PathBuf, message: String },
    /// The repository has no working tree, so workspace paths mean nothing.
    Bare { path: PathBuf },
    /// The revision specification does not name a commit.
    UnknownRevision { spec: String, message: String },
    /// The two revisions share no history.
    NoMergeBase { one: String, two: String },
    /// A path lies outside the repository's working tree.
    OutsideRepository { path: PathBuf },
    /// Any other failure reported by the object database.
    Git(String),
}

impl fmt::Display for VcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VcsError::NotARepository { path, message } => {
                write!(
                    f,
                    "{} is not inside a git repository: {message}",
                    path.display()
                )
            }
            VcsError::Bare { path } => write!(
                f,
                "the repository at {} has no working tree",
                path.display()
            ),
            VcsError::UnknownRevision { spec, message } => {
                write!(f, "cannot resolve revision '{spec}': {message}")
            }
            VcsError::NoMergeBase { one, two } => {
                write!(f, "{one} and {two} have no common ancestor")
            }
            VcsError::OutsideRepository { path } => write!(
                f,
                "{} is outside the repository's working tree",
                path.display()
            ),
            VcsError::Git(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for VcsError {}

/// A resolved commit and the tree it records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revision {
    commit: ObjectId,
    tree: ObjectId,
    label: String,
}

impl Revision {
    /// How the revision is named in reports: the specification it was
    /// resolved from, or an abbreviated commit id.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The full hexadecimal commit id.
    pub fn commit(&self) -> String {
        self.commit.to_string()
    }

    /// The same revision under another name.
    #[must_use]
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

/// A git repository with a working tree.
#[derive(Clone)]
pub struct Repository {
    inner: gix::ThreadSafeRepository,
    /// Canonical path of the working tree.
    workdir: PathBuf,
}

impl fmt::Debug for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Repository")
            .field("workdir", &self.workdir)
            .finish_non_exhaustive()
    }
}

impl Repository {
    /// Finds the repository containing `path`, searching parent directories.
    pub fn discover(path: &Path) -> Result<Self, VcsError> {
        let repo = gix::discover(path).map_err(|e| VcsError::NotARepository {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
        let workdir = repo
            .workdir()
            .ok_or_else(|| VcsError::Bare {
                path: repo.git_dir().to_path_buf(),
            })?
            .canonicalize()
            .map_err(|e| VcsError::Git(format!("cannot open the working tree: {e}")))?;
        Ok(Self {
            inner: repo.into_sync(),
            workdir,
        })
    }

    /// The canonical path of the working tree.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Resolves a revision specification (`main`, `HEAD~2`, `v1.4.0`, a
    /// commit id) to a commit.
    pub fn resolve(&self, spec: &str) -> Result<Revision, VcsError> {
        let repo = self.inner.to_thread_local();
        let unknown = |message: String| VcsError::UnknownRevision {
            spec: spec.to_owned(),
            message,
        };
        let id = repo
            .rev_parse_single(spec)
            .map_err(|e| unknown(e.to_string()))?;
        let commit = id
            .object()
            .map_err(|e| unknown(e.to_string()))?
            .peel_to_commit()
            .map_err(|e| unknown(e.to_string()))?;
        let tree = commit.tree_id().map_err(|e| unknown(e.to_string()))?;
        Ok(Revision {
            commit: commit.id,
            tree: tree.detach(),
            label: spec.to_owned(),
        })
    }

    /// The best common ancestor of two revisions, labelled with its
    /// abbreviated commit id.
    pub fn merge_base(&self, one: &Revision, two: &Revision) -> Result<Revision, VcsError> {
        let repo = self.inner.to_thread_local();
        let base = repo
            .merge_base(one.commit, two.commit)
            .map_err(|_| VcsError::NoMergeBase {
                one: one.label.clone(),
                two: two.label.clone(),
            })?;
        let hex = base.to_string();
        let revision = self.resolve(&hex)?;
        Ok(revision.with_label(hex[..12].to_owned()))
    }

    /// The path segments of `path` relative to the working tree. `path` is
    /// absolute or relative to the current directory; it need not exist,
    /// since a file may be present in a revision and absent from the checkout.
    pub fn repo_path(&self, path: &Path) -> Result<Vec<String>, VcsError> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| VcsError::Git(format!("cannot read the current directory: {e}")))?
                .join(path)
        };
        let resolved = paths::resolve_existing_prefix(&absolute).ok_or_else(|| {
            VcsError::OutsideRepository {
                path: path.to_path_buf(),
            }
        })?;
        let relative =
            resolved
                .strip_prefix(&self.workdir)
                .map_err(|_| VcsError::OutsideRepository {
                    path: path.to_path_buf(),
                })?;
        Ok(relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect())
    }

    /// A loader serving the workspace rooted at `root` (segments relative to
    /// the working tree, as [`Repository::repo_path`] returns) from
    /// `revision`.
    pub fn loader(&self, revision: &Revision, root: Vec<String>, limits: Limits) -> GitLoader {
        GitLoader {
            repo: self.inner.clone(),
            tree: revision.tree,
            label: revision.label.clone(),
            root,
            limits,
        }
    }
}

/// Serves a workspace's `file:` URIs from the tree of one revision.
pub struct GitLoader {
    repo: gix::ThreadSafeRepository,
    tree: ObjectId,
    label: String,
    /// The workspace root inside the repository.
    root: Vec<String>,
    limits: Limits,
}

impl fmt::Debug for GitLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitLoader")
            .field("tree", &self.tree)
            .field("label", &self.label)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Loader for GitLoader {
    fn load(&self, uri: &UriRef) -> Result<Loaded, LoadError> {
        let relative = workspace_path(uri)?;
        if relative.is_empty() {
            return Err(LoadError::NotFound);
        }
        let repo = self.repo.to_thread_local();
        let git = |e: gix::Error| LoadError::Io(e.to_string());
        let tree = repo.find_tree(self.tree).map_err(git)?;
        let path: PathBuf = self.root.iter().chain(&relative).collect();
        // Every component but the last must be a directory in the tree: a
        // symbolic link part-way along the path is not followed, and the
        // lookup simply finds nothing.
        let entry = tree
            .lookup_entry_by_path(&path)
            .map_err(git)?
            .ok_or(LoadError::NotFound)?;
        let display = relative.join("/");
        let mode = entry.mode();
        if mode.is_link() {
            return Err(LoadError::Io(format!(
                "{display} is a symbolic link in {}; links are not followed in git revisions",
                self.label
            )));
        }
        if mode.is_commit() {
            return Err(LoadError::Io(format!(
                "{display} is a submodule in {}; submodules are not read",
                self.label
            )));
        }
        if !mode.is_blob() {
            return Err(LoadError::NotFound);
        }
        let id = entry.object_id();
        let header = repo.find_header(id).map_err(git)?;
        self.limits
            .check_file_size(header.size())
            .map_err(LoadError::TooLarge)?;
        let blob = repo.find_blob(id).map_err(git)?;
        let text = decode_text(&blob.data).map_err(LoadError::Decode)?;
        Ok(Loaded {
            name: SourceName::Revision {
                rev: self.label.clone(),
                path: display,
            },
            text: text.to_owned(),
        })
    }

    /// The blobs in a directory of the revision's tree; links and
    /// submodules are left out, as they are not read.
    fn list(&self, uri: &UriRef) -> Result<Vec<String>, LoadError> {
        let relative = workspace_path(uri)?;
        let repo = self.repo.to_thread_local();
        let git = |e: gix::Error| LoadError::Io(e.to_string());
        let mut tree = repo.find_tree(self.tree).map_err(git)?;
        let path: PathBuf = self.root.iter().chain(&relative).collect();
        if !path.as_os_str().is_empty() {
            let entry = tree
                .lookup_entry_by_path(&path)
                .map_err(git)?
                .ok_or(LoadError::NotFound)?;
            if !entry.mode().is_tree() {
                return Err(LoadError::NotFound);
            }
            tree = repo.find_tree(entry.object_id()).map_err(git)?;
        }
        let decoded = tree.decode().map_err(|e| LoadError::Io(e.to_string()))?;
        let mut names: Vec<String> = decoded
            .entries
            .iter()
            .filter(|e| e.mode.is_blob())
            .map(|e| e.filename.to_string())
            .collect();
        names.sort();
        Ok(names)
    }
}

use std::path::{Path, PathBuf};

use faultline_doc::loader::{FsLoader, LoadError, Loader, workspace_uri};
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Role};
use faultline_jsonschema::Options;
use faultline_policy::{ApiFormat, SchemaRole};
use faultline_report::ApiSummary;
use faultline_rules::{Finding, classify, suggest_bump};
use faultline_source::{Diagnostic, DiagnosticCode, Diagnostics, Limits, SourceMap};
use faultline_vcs::{Repository, VcsError};

use crate::codes;

/// Where to find one version of an API: an entry document inside a workspace
/// root, read from disk or from a git revision. References may reach any
/// file under the root and nothing outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// A directory on disk. For a revision, the directory's place in the
    /// repository's working tree says which part of the revision's tree is
    /// the workspace.
    pub root: PathBuf,
    /// Relative to `root`, or absolute inside it.
    pub entry: PathBuf,
    /// Read the files from this revision instead of the working tree.
    pub revision: Option<Revision>,
}

impl Source {
    pub fn new(root: impl Into<PathBuf>, entry: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entry: entry.into(),
            revision: None,
        }
    }

    /// The same files, read from `revision`.
    #[must_use]
    pub fn at(mut self, revision: Revision) -> Self {
        self.revision = Some(revision);
        self
    }
}

/// A git revision to read a version from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revision {
    /// Anything `git rev-parse` accepts: `main`, `v1.2.0`, `HEAD~1`, a
    /// commit id.
    Named(String),
    /// The best common ancestor of two revisions: the version a change
    /// branched from, which is what a pull request should be compared with
    /// even after its target branch has moved on.
    MergeBase { base: String, head: String },
}

/// One API to compare between two versions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub old: Source,
    pub new: Source,
    /// Detected from the entry document when `None`.
    pub format: Option<ApiFormat>,
    /// For standalone JSON Schemas: which direction documents flow.
    pub role: SchemaRole,
    /// Whether the API may be new: when set, a missing old entry document
    /// is noted instead of being an error.
    pub may_be_new: bool,
}

impl Target {
    pub fn new(name: impl Into<String>, old: Source, new: Source) -> Self {
        Self {
            name: name.into(),
            old,
            new,
            format: None,
            role: SchemaRole::Both,
            may_be_new: false,
        }
    }
}

/// The lowered form of one version.
#[derive(Debug)]
pub struct Lowered {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
    /// The entry document does not exist (as opposed to failing to parse or
    /// lower).
    pub missing_entry: bool,
}

/// Everything a comparison produced. Spans in diagnostics and findings refer
/// to `sources`.
#[derive(Debug, Default)]
pub struct Comparison {
    pub sources: SourceMap,
    pub diagnostics: Diagnostics,
    pub apis: Vec<ApiSummary>,
    pub findings: Vec<Finding>,
}

/// Loads, lowers and compares APIs.
#[derive(Clone, Debug, Default)]
pub struct Engine {
    pub limits: Limits,
}

impl Engine {
    /// Compares every target. Targets whose versions cannot be lowered
    /// without errors contribute diagnostics and no findings.
    pub fn compare(&self, targets: &[Target]) -> Comparison {
        let mut comparison = Comparison::default();
        for target in targets {
            let old = self.lower(
                &target.old,
                &target.name,
                target.format,
                target.role,
                &comparison.sources,
            );
            if old.missing_entry && target.may_be_new {
                comparison.diagnostics.push(
                    Diagnostic::note(format!(
                        "{}: {} does not exist in {}; the API is new, so there is nothing to compare",
                        target.name,
                        target.old.entry.display(),
                        describe(&target.old)
                    ))
                    .with_code(codes::NEW_API),
                );
                continue;
            }
            let new = self.lower(
                &target.new,
                &target.name,
                target.format,
                target.role,
                &comparison.sources,
            );
            let clean = !old.diagnostics.has_errors() && !new.diagnostics.has_errors();
            comparison.diagnostics.extend(old.diagnostics);
            comparison.diagnostics.extend(new.diagnostics);
            let (Some(old), Some(new), true) = (old.api, new.api, clean) else {
                continue;
            };
            let changes = faultline_diff::diff(&old, &new);
            let findings: Vec<Finding> =
                changes.iter().map(|c| classify(&target.name, c)).collect();
            comparison.apis.push(ApiSummary {
                name: target.name.clone(),
                old_version: old.version.clone(),
                new_version: new.version.clone(),
                bump: suggest_bump(&findings),
            });
            comparison.findings.extend(findings);
        }
        comparison.diagnostics.sort();
        comparison
    }

    /// Lowers one version of an API, registering its files in `sources`.
    pub fn lower(
        &self,
        source: &Source,
        name: &str,
        format: Option<ApiFormat>,
        role: SchemaRole,
        sources: &SourceMap,
    ) -> Lowered {
        let (loader, entry) = match self.open(source) {
            Ok(opened) => opened,
            Err(failure) => {
                return Lowered {
                    api: None,
                    diagnostics: [Diagnostic::error(failure.message).with_code(failure.code)]
                        .into_iter()
                        .collect(),
                    missing_entry: failure.missing_entry,
                };
            }
        };
        let mut store = DocumentStore::new(loader.as_ref(), sources, self.limits.clone());
        let format = match format {
            Some(format) => format,
            None => match detect_format(&mut store, &entry) {
                Some(format) => format,
                // The document failed to load or parse; lowering reports why.
                None => ApiFormat::JsonSchema,
            },
        };
        let lowered = match format {
            ApiFormat::OpenApi => faultline_openapi::lower_openapi(&entry, &mut store, name),
            ApiFormat::JsonSchema => faultline_jsonschema::lower_document(
                &entry,
                &mut store,
                Options::default(),
                ir_role(role),
            ),
        };
        let mut api = lowered.api;
        if let Some(api) = &mut api {
            name.clone_into(&mut api.name);
        }
        Lowered {
            api,
            diagnostics: lowered.diagnostics,
            missing_entry: false,
        }
    }

    /// A loader for a source's workspace and the URI of its entry document.
    fn open(&self, source: &Source) -> Result<(Box<dyn Loader>, UriRef), Failure> {
        match &source.revision {
            None => self.open_worktree(source),
            Some(revision) => self.open_revision(source, revision),
        }
    }

    fn open_worktree(&self, source: &Source) -> Result<(Box<dyn Loader>, UriRef), Failure> {
        let loader = FsLoader::new(&source.root, self.limits.clone()).map_err(|e| {
            Failure::new(
                codes::INVALID_ROOT,
                format!(
                    "cannot use {} as the workspace root: {e}",
                    source.root.display()
                ),
            )
        })?;
        let entry = loader.uri_for(&source.entry).map_err(|e| match e {
            LoadError::OutsideRoot => outside_root(source),
            LoadError::NotFound => missing(source),
            e => Failure::new(
                codes::ENTRY_NOT_FOUND,
                format!("cannot read {}: {e}", source.entry.display()),
            ),
        })?;
        Ok((Box::new(loader), entry))
    }

    fn open_revision(
        &self,
        source: &Source,
        revision: &Revision,
    ) -> Result<(Box<dyn Loader>, UriRef), Failure> {
        let root = source.root.canonicalize().map_err(|e| {
            Failure::new(
                codes::INVALID_ROOT,
                format!(
                    "cannot use {} as the workspace root: {e}",
                    source.root.display()
                ),
            )
        })?;
        let git = |e: VcsError| {
            let code = match e {
                VcsError::UnknownRevision { .. } | VcsError::NoMergeBase { .. } => {
                    codes::UNKNOWN_REVISION
                }
                _ => codes::GIT,
            };
            Failure::new(code, e.to_string())
        };
        let repo = Repository::discover(&root).map_err(git)?;
        let resolved = match revision {
            Revision::Named(spec) => repo.resolve(spec).map_err(git)?,
            Revision::MergeBase { base, head } => {
                let base = repo.resolve(base).map_err(git)?;
                let head = repo.resolve(head).map_err(git)?;
                repo.merge_base(&base, &head).map_err(git)?
            }
        };
        let root_in_repo = repo.repo_path(&root).map_err(git)?;
        // The entry need not exist on disk: it may have been renamed or
        // deleted since the revision.
        // As in `FsLoader::uri_for`, rebuild the entry from its components so
        // it joins correctly onto a verbatim canonical root.
        let entry_in_repo = repo
            .repo_path(&root.join(source.entry.components().collect::<PathBuf>()))
            .map_err(|_| outside_root(source))?;
        let relative: PathBuf = entry_in_repo
            .strip_prefix(root_in_repo.as_slice())
            .ok_or_else(|| outside_root(source))?
            .iter()
            .collect();
        let entry = workspace_uri(&relative);
        let loader = repo.loader(&resolved, root_in_repo, self.limits.clone());
        if loader.load(&entry) == Err(LoadError::NotFound) {
            return Err(missing(source));
        }
        Ok((Box::new(loader), entry))
    }
}

/// Why a source could not be opened.
struct Failure {
    code: DiagnosticCode,
    message: String,
    missing_entry: bool,
}

impl Failure {
    fn new(code: DiagnosticCode, message: String) -> Self {
        Self {
            code,
            message,
            missing_entry: false,
        }
    }
}

fn outside_root(source: &Source) -> Failure {
    Failure::new(
        codes::ENTRY_NOT_FOUND,
        format!(
            "{} is outside the workspace root {}",
            source.entry.display(),
            source.root.display()
        ),
    )
}

fn missing(source: &Source) -> Failure {
    Failure {
        code: codes::ENTRY_NOT_FOUND,
        message: format!(
            "{} does not exist in {}",
            source.entry.display(),
            describe(source)
        ),
        missing_entry: true,
    }
}

/// "the working tree", "revision main" or "the merge base of main and HEAD".
fn describe(source: &Source) -> String {
    match &source.revision {
        None => "the working tree".to_owned(),
        Some(Revision::Named(spec)) => format!("revision {spec}"),
        Some(Revision::MergeBase { base, head }) => {
            format!("the merge base of {base} and {head}")
        }
    }
}

/// Recognizes OpenAPI (and Swagger, so the OpenAPI front end can say it is
/// unsupported) by its version field; everything else is JSON Schema.
pub fn detect_format(store: &mut DocumentStore<'_>, entry: &UriRef) -> Option<ApiFormat> {
    let document = store.get(entry).ok()?;
    let root = &document.document.root;
    if root.get("openapi").is_some() || root.get("swagger").is_some() {
        Some(ApiFormat::OpenApi)
    } else {
        Some(ApiFormat::JsonSchema)
    }
}

fn ir_role(role: SchemaRole) -> Role {
    match role {
        SchemaRole::Input => Role::Input,
        SchemaRole::Output => Role::Output,
        SchemaRole::Both => Role::Both,
    }
}

/// The API name used when none is configured: the entry file's stem.
pub fn default_api_name(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "api".to_owned(), |s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::default_api_name as stem;
    use std::path::Path;

    #[test]
    fn stems() {
        assert_eq!(stem(Path::new("api/openapi.yaml")), "openapi");
        assert_eq!(stem(Path::new("/")), "api");
    }
}

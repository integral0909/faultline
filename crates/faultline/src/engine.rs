use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use faultline_doc::loader::{FsLoader, LoadError, Loader, SchemeRouter, workspace_uri};
use faultline_doc::{DocumentStore, StoreError, UriRef};
use faultline_ir::{Api, Role};
use faultline_jsonschema::Options;
use faultline_net::{HostPattern, NetworkPolicy, RemoteLoader};
use faultline_policy::{ApiFormat, NetworkConfig, SchemaRole};
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
    /// Overlays applied to the entry document before it is lowered, in
    /// order; paths like `entry`.
    pub overlays: Vec<PathBuf>,
}

impl Source {
    pub fn new(root: impl Into<PathBuf>, entry: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            entry: entry.into(),
            revision: None,
            overlays: Vec::new(),
        }
    }

    /// The same files, with `overlays` applied to the entry document.
    #[must_use]
    pub fn with_overlays(mut self, overlays: Vec<PathBuf>) -> Self {
        self.overlays = overlays;
        self
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
    /// Overrides `format` for the old version, for comparisons across
    /// formats (an OpenAPI description against a gRPC service's HTTP view).
    pub old_format: Option<ApiFormat>,
    /// Overrides `format` for the new version.
    pub new_format: Option<ApiFormat>,
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
            old_format: None,
            new_format: None,
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
#[derive(Clone, Default)]
pub struct Engine {
    pub limits: Limits,
    /// Serves `http`/`https` references. Without one, remote references
    /// are reported as disabled. Shared by every version and API compared,
    /// so each remote document is fetched once.
    remote: Option<Arc<dyn Loader>>,
}

impl fmt::Debug for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Engine")
            .field("limits", &self.limits)
            .field("remote", &self.remote.is_some())
            .finish()
    }
}

impl Engine {
    /// Enables remote references, served by `loader`.
    #[must_use]
    pub fn with_remote(mut self, loader: Arc<dyn Loader>) -> Self {
        self.remote = Some(loader);
        self
    }

    /// Applies a workspace's `[network]` settings: when remote references
    /// are allowed, they are fetched over HTTPS under the configured policy.
    pub fn with_network(self, config: &NetworkConfig) -> Result<Self, String> {
        match network_policy(config)? {
            Some(policy) => {
                let loader = RemoteLoader::https(policy, self.limits.clone());
                Ok(self.with_remote(Arc::new(loader)))
            }
            None => Ok(self),
        }
    }

    /// Compares every target. Targets whose versions cannot be lowered
    /// without errors contribute diagnostics and no findings.
    ///
    /// Targets are compared on worker threads, as many as there are cores,
    /// and the two versions of each target are lowered in parallel. Results
    /// are assembled in target order and diagnostics sorted by file name and
    /// position, so the output does not depend on scheduling.
    pub fn compare(&self, targets: &[Target]) -> Comparison {
        let mut comparison = Comparison::default();
        let workers = std::thread::available_parallelism()
            .map_or(1, std::num::NonZeroUsize::get)
            .min(targets.len())
            .max(1);
        let next = AtomicUsize::new(0);
        let outcomes: Vec<Mutex<Option<Outcome>>> =
            targets.iter().map(|_| Mutex::new(None)).collect();
        let sources = &comparison.sources;
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let (next, outcomes) = (&next, &outcomes);
                spawn(scope, move || {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(target) = targets.get(i) else {
                            break;
                        };
                        let outcome = self.compare_one(target, sources);
                        *outcomes[i].lock().expect("no worker holds the lock") = Some(outcome);
                    }
                });
            }
        });
        for outcome in outcomes {
            let outcome = outcome
                .into_inner()
                .expect("workers finished")
                .expect("every target was compared");
            comparison.diagnostics.extend(outcome.diagnostics);
            if let Some(summary) = outcome.summary {
                comparison.apis.push(summary);
            }
            comparison.findings.extend(outcome.findings);
        }
        comparison.diagnostics.sort_in(&comparison.sources);
        comparison
    }

    fn compare_one(&self, target: &Target, sources: &SourceMap) -> Outcome {
        let lower = |source: &Source, format: Option<ApiFormat>| {
            self.lower(
                source,
                &target.name,
                format.or(target.format),
                target.role,
                sources,
            )
        };
        let (old, new) = std::thread::scope(|scope| {
            let new = spawn(scope, || lower(&target.new, target.new_format));
            let old = lower(&target.old, target.old_format);
            (
                old,
                new.join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            )
        });
        let mut outcome = Outcome::default();
        if old.missing_entry && target.may_be_new {
            outcome.diagnostics.push(
                Diagnostic::note(format!(
                    "{}: {} does not exist in {}; the API is new, so there is nothing to compare",
                    target.name,
                    target.old.entry.display(),
                    describe(&target.old)
                ))
                .with_code(codes::NEW_API),
            );
            return outcome;
        }
        let clean = !old.diagnostics.has_errors() && !new.diagnostics.has_errors();
        outcome.diagnostics.extend(old.diagnostics);
        outcome.diagnostics.extend(new.diagnostics);
        let (Some(old), Some(new), true) = (old.api, new.api, clean) else {
            return outcome;
        };
        let (findings, summary) = compare_apis(&target.name, &old, &new);
        outcome.findings = findings;
        outcome.summary = Some(summary);
        outcome
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
        let opened = match self.open(source) {
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
        let mut lowered = self.lower_overlaid(
            opened.loader.as_ref(),
            &opened.entry,
            &opened.overlays,
            name,
            format,
            role,
            sources,
        );
        lowered.diagnostics.extend(opened.notes);
        lowered
    }

    /// Lowers the API whose entry document is `entry`, read through
    /// `loader`: for callers that serve documents their own way, such as an
    /// editor serving unsaved text.
    pub fn lower_entry(
        &self,
        loader: &dyn Loader,
        entry: &UriRef,
        name: &str,
        format: Option<ApiFormat>,
        role: SchemaRole,
        sources: &SourceMap,
    ) -> Lowered {
        self.lower_overlaid(loader, entry, &[], name, format, role, sources)
    }

    /// Lowers the API at `entry` with `overlays` applied to its entry
    /// document first.
    #[allow(clippy::too_many_arguments)]
    fn lower_overlaid(
        &self,
        loader: &dyn Loader,
        entry: &UriRef,
        overlays: &[UriRef],
        name: &str,
        format: Option<ApiFormat>,
        role: SchemaRole,
        sources: &SourceMap,
    ) -> Lowered {
        let entry = entry.clone();
        let router = SchemeRouter {
            workspace: loader,
            remote: self.remote.as_deref(),
        };
        // Protobuf, GraphQL, XML Schema, WSDL and others are not JSON or
        // YAML: they are read by their own front ends.
        let format = format.or_else(|| format_by_extension(&entry));
        if let Some(format) = format.filter(|f| !reads_documents(*f)) {
            return self.lower_elsewhere(format, loader, &entry, overlays, name, role, sources);
        }
        let mut store = DocumentStore::new(&router, sources, self.limits.clone());
        store.allow_remote(self.remote.is_some());
        let overlaid = apply_overlays(&mut store, &entry, overlays);
        if overlaid.has_errors() {
            let mut diagnostics = store.take_diagnostics();
            diagnostics.extend(overlaid);
            return Lowered {
                api: None,
                diagnostics,
                missing_entry: false,
            };
        }
        let format = match format {
            Some(format) => format,
            None => match detect_format(&mut store, &entry) {
                Some(format) => format,
                // The document failed to load or parse; lowering reports why.
                None => ApiFormat::JsonSchema,
            },
        };
        // A JSON document that turned out to be read by its own front end
        // (a Smithy JSON AST).
        if !reads_documents(format) {
            let mut lowered =
                self.lower_elsewhere(format, loader, &entry, overlays, name, role, sources);
            lowered.diagnostics.extend(overlaid);
            return lowered;
        }
        let (mut api, mut diagnostics) = lower_document(format, &mut store, &entry, name, role);
        if let Some(api) = &mut api {
            name.clone_into(&mut api.name);
        }
        diagnostics.extend(overlaid);
        Lowered {
            api,
            diagnostics,
            missing_entry: false,
        }
    }

    /// Lowers a format read by its own front end, which overlays cannot
    /// change.
    #[allow(clippy::too_many_arguments)]
    fn lower_elsewhere(
        &self,
        format: ApiFormat,
        loader: &dyn Loader,
        entry: &UriRef,
        overlays: &[UriRef],
        name: &str,
        role: SchemaRole,
        sources: &SourceMap,
    ) -> Lowered {
        let (api, diagnostics) = if overlays.is_empty() {
            self.lower_other(format, loader, entry, name, role, sources)
                .expect("formats that are not documents are lowered by their front ends")
        } else {
            let message = format!(
                "overlays apply to JSON and YAML descriptions, not to {}",
                format.as_str()
            );
            (
                None,
                [Diagnostic::error(message).with_code(codes::OVERLAY)]
                    .into_iter()
                    .collect(),
            )
        };
        Lowered {
            api,
            diagnostics,
            missing_entry: false,
        }
    }

    /// Lowers the formats that are not JSON or YAML documents, which read
    /// their files themselves. `None` for the formats the document store
    /// reads.
    fn lower_other(
        &self,
        format: ApiFormat,
        loader: &dyn Loader,
        entry: &UriRef,
        name: &str,
        role: SchemaRole,
        sources: &SourceMap,
    ) -> Option<(Option<Api>, Diagnostics)> {
        let limits = || self.limits.clone();
        Some(match format {
            ApiFormat::GraphQl => {
                let l = faultline_graphql::lower_graphql(entry, loader, sources, name);
                (l.api, l.diagnostics)
            }
            ApiFormat::Protobuf => {
                let l =
                    faultline_protobuf::lower_protobuf(entry, loader, sources, name, ir_role(role));
                (l.api, l.diagnostics)
            }
            ApiFormat::ProtobufHttp => {
                let l = faultline_protobuf::lower_protobuf_http(entry, loader, sources, name);
                (l.api, l.diagnostics)
            }
            ApiFormat::Xsd => {
                let l =
                    faultline_xsd::lower_xsd(entry, loader, sources, name, ir_role(role), limits());
                (l.api, l.diagnostics)
            }
            ApiFormat::Wsdl => {
                let l = faultline_wsdl::lower_wsdl(entry, loader, sources, name, limits());
                (l.api, l.diagnostics)
            }
            ApiFormat::Raml => {
                let l = faultline_raml::lower_raml(entry, loader, sources, name, limits());
                (l.api, l.diagnostics)
            }
            ApiFormat::Thrift => {
                let l = faultline_thrift::lower_thrift(entry, loader, sources, name, ir_role(role));
                (l.api, l.diagnostics)
            }
            ApiFormat::Smithy => {
                let l = faultline_smithy::lower_smithy(entry, loader, sources, name, ir_role(role));
                (l.api, l.diagnostics)
            }
            ApiFormat::OpenApi
            | ApiFormat::JsonSchema
            | ApiFormat::Avro
            | ApiFormat::AsyncApi
            | ApiFormat::Jtd => {
                return None;
            }
        })
    }

    /// A loader for a source's workspace, the URI of its entry document and
    /// of its overlays.
    fn open(&self, source: &Source) -> Result<Opened, Failure> {
        match &source.revision {
            None => self.open_worktree(source),
            Some(revision) => self.open_revision(source, revision),
        }
    }

    fn open_worktree(&self, source: &Source) -> Result<Opened, Failure> {
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
        let overlays = source
            .overlays
            .iter()
            .map(|path| {
                loader.uri_for(path).map_err(|e| {
                    let problem = match e {
                        LoadError::OutsideRoot => "it is outside the workspace root".to_owned(),
                        e => e.to_string(),
                    };
                    Failure::new(
                        codes::OVERLAY,
                        format!("cannot read the overlay {}: {problem}", path.display()),
                    )
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(Opened {
            loader: Box::new(loader),
            entry,
            overlays,
            notes: Vec::new(),
        })
    }

    fn open_revision(&self, source: &Source, revision: &Revision) -> Result<Opened, Failure> {
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
        // Files need not exist on disk: they may have been renamed or
        // deleted since the revision.
        // As in `FsLoader::uri_for`, rebuild paths from their components so
        // they join correctly onto a verbatim canonical root.
        let in_workspace = |path: &Path| -> Option<UriRef> {
            let in_repo = repo
                .repo_path(&root.join(path.components().collect::<PathBuf>()))
                .ok()?;
            let relative: PathBuf = in_repo
                .strip_prefix(root_in_repo.as_slice())?
                .iter()
                .collect();
            Some(workspace_uri(&relative))
        };
        let entry = in_workspace(&source.entry).ok_or_else(|| outside_root(source))?;
        let mut overlays = Vec::new();
        let mut notes = Vec::new();
        let loader = repo.loader(&resolved, root_in_repo.clone(), self.limits.clone());
        for path in &source.overlays {
            let Some(uri) = in_workspace(path) else {
                return Err(Failure::new(
                    codes::OVERLAY,
                    format!(
                        "the overlay {} is outside the workspace root {}",
                        path.display(),
                        source.root.display()
                    ),
                ));
            };
            // An overlay added since the revision did not apply to it.
            if loader.load(&uri) == Err(LoadError::NotFound) {
                notes.push(
                    Diagnostic::note(format!(
                        "the overlay {} does not exist in {}, which is compared without it",
                        path.display(),
                        describe(source)
                    ))
                    .with_code(codes::OVERLAY),
                );
                continue;
            }
            overlays.push(uri);
        }
        if loader.load(&entry) == Err(LoadError::NotFound) {
            return Err(missing(source));
        }
        Ok(Opened {
            loader: Box::new(loader),
            entry,
            overlays,
            notes,
        })
    }
}

/// Lowers a format whose descriptions are JSON or YAML documents, from
/// `store`.
fn lower_document(
    format: ApiFormat,
    store: &mut DocumentStore<'_>,
    entry: &UriRef,
    name: &str,
    role: SchemaRole,
) -> (Option<Api>, Diagnostics) {
    match format {
        ApiFormat::OpenApi => {
            let lowered = faultline_openapi::lower_openapi(entry, store, name);
            (lowered.api, lowered.diagnostics)
        }
        ApiFormat::JsonSchema => {
            let lowered = faultline_jsonschema::lower_document(
                entry,
                store,
                Options::default(),
                ir_role(role),
            );
            (lowered.api, lowered.diagnostics)
        }
        ApiFormat::AsyncApi => {
            let lowered = faultline_asyncapi::lower_asyncapi(entry, store, name);
            (lowered.api, lowered.diagnostics)
        }
        ApiFormat::Avro => {
            let lowered = faultline_avro::lower_avro(entry, store, name, ir_role(role));
            (lowered.api, lowered.diagnostics)
        }
        ApiFormat::Jtd => {
            let lowered = faultline_jtd::lower_jtd(entry, store, name, ir_role(role));
            (lowered.api, lowered.diagnostics)
        }
        ApiFormat::Protobuf
        | ApiFormat::ProtobufHttp
        | ApiFormat::GraphQl
        | ApiFormat::Xsd
        | ApiFormat::Wsdl
        | ApiFormat::Raml
        | ApiFormat::Thrift
        | ApiFormat::Smithy => unreachable!("read by their own front ends"),
    }
}

/// Whether a format's descriptions are JSON or YAML documents, which
/// overlays can change.
fn reads_documents(format: ApiFormat) -> bool {
    matches!(
        format,
        ApiFormat::OpenApi
            | ApiFormat::AsyncApi
            | ApiFormat::JsonSchema
            | ApiFormat::Avro
            | ApiFormat::Jtd
    )
}

/// Applies `overlays` to the entry document held by `store`, which later
/// reads then see. Problems with the entry itself are left to lowering.
fn apply_overlays(
    store: &mut DocumentStore<'_>,
    entry: &UriRef,
    overlays: &[UriRef],
) -> Diagnostics {
    let mut diagnostics = Diagnostics::new();
    if overlays.is_empty() {
        return diagnostics;
    }
    let Ok(document) = store.get(entry) else {
        return diagnostics;
    };
    let mut root = document.document.root.clone();
    for uri in overlays {
        match store.get(uri) {
            Ok(overlay) => {
                if let Some(overlay) =
                    faultline_overlay::read(&overlay.document.root, &mut diagnostics)
                {
                    faultline_overlay::apply(&overlay, &mut root, &mut diagnostics);
                }
            }
            Err(StoreError::Load(e)) => diagnostics.push(
                Diagnostic::error(format!("cannot read the overlay {uri}: {e}"))
                    .with_code(codes::OVERLAY),
            ),
            // The parse errors are among the store's diagnostics.
            Err(StoreError::Parse) => diagnostics.push(
                Diagnostic::error(format!("the overlay {uri} does not parse"))
                    .with_code(codes::OVERLAY),
            ),
        }
    }
    let _ = store.replace_root(entry, root);
    diagnostics
}

/// Compares two lowered versions of the API `name`, classifying every
/// change.
pub fn compare_apis(name: &str, old: &Api, new: &Api) -> (Vec<Finding>, ApiSummary) {
    let changes = faultline_diff::diff(old, new);
    let findings: Vec<Finding> = changes.iter().map(|c| classify(name, c)).collect();
    let summary = ApiSummary {
        name: name.to_owned(),
        old_version: old.version.clone(),
        new_version: new.version.clone(),
        bump: suggest_bump(&findings),
    };
    (findings, summary)
}

/// The network policy a `[network]` section describes, or `None` when remote
/// references stay disabled.
pub fn network_policy(config: &NetworkConfig) -> Result<Option<NetworkPolicy>, String> {
    if !config.allow_remote {
        return Ok(None);
    }
    let allowed_hosts = config
        .allowed_hosts
        .iter()
        .map(|h| HostPattern::parse(h).map_err(|e| format!("[network] allowed_hosts: {e}")))
        .collect::<Result<Vec<_>, _>>()?;
    let defaults = NetworkPolicy::default();
    Ok(Some(NetworkPolicy {
        allowed_hosts,
        max_bytes: config.max_bytes.unwrap_or(defaults.max_bytes),
        timeout: config
            .timeout_seconds
            .map_or(defaults.timeout, Duration::from_secs),
        max_redirects: config.max_redirects.unwrap_or(defaults.max_redirects),
    }))
}

/// What comparing one target produced.
#[derive(Default)]
struct Outcome {
    diagnostics: Diagnostics,
    summary: Option<ApiSummary>,
    findings: Vec<Finding>,
}

/// Stack size for worker threads: parsing and comparison recurse, within
/// their depth limits, deeper than a default thread stack allows for.
const WORKER_STACK: usize = 32 * 1024 * 1024;

/// Spawns `f` on a scoped thread with a [`WORKER_STACK`]-sized stack.
fn spawn<'scope, 'env, T: Send + 'scope>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    f: impl FnOnce() -> T + Send + 'scope,
) -> std::thread::ScopedJoinHandle<'scope, T> {
    std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn_scoped(scope, f)
        .expect("threads can be spawned")
}

/// Why a source could not be opened.
/// A version's files, ready to be read.
struct Opened {
    loader: Box<dyn Loader>,
    entry: UriRef,
    overlays: Vec<UriRef>,
    /// Things to know about how the version is read.
    notes: Vec<Diagnostic>,
}

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

/// Formats recognized by the entry's file extension: `.proto`, `.thrift`
/// and `.smithy` files, XML Schemas, WSDL descriptions and GraphQL schemas are not JSON or YAML
/// documents, and are read by their own front ends; RAML is YAML with its
/// own tags and includes; Avro schemas and JSON Type Definitions
/// (`.jtd.json`) are JSON but cannot be told from JSON Schema by their
/// content alone.
pub fn format_by_extension(entry: &UriRef) -> Option<ApiFormat> {
    let path = entry.path().to_ascii_lowercase();
    if path.ends_with(".jtd.json") {
        return Some(ApiFormat::Jtd);
    }
    let extension = std::path::Path::new(&path)
        .extension()?
        .to_str()?
        .to_owned();
    match extension.as_str() {
        "proto" => Some(ApiFormat::Protobuf),
        "graphql" | "graphqls" | "gql" => Some(ApiFormat::GraphQl),
        "avsc" => Some(ApiFormat::Avro),
        "xsd" => Some(ApiFormat::Xsd),
        "wsdl" => Some(ApiFormat::Wsdl),
        "raml" => Some(ApiFormat::Raml),
        "thrift" => Some(ApiFormat::Thrift),
        "smithy" => Some(ApiFormat::Smithy),
        _ => None,
    }
}

/// Recognizes OpenAPI (and Swagger, so the OpenAPI front end can say it is
/// unsupported) and AsyncAPI by their version fields; everything else is
/// JSON Schema.
pub fn detect_format(store: &mut DocumentStore<'_>, entry: &UriRef) -> Option<ApiFormat> {
    let document = store.get(entry).ok()?;
    let root = &document.document.root;
    if root.get("openapi").is_some() || root.get("swagger").is_some() {
        Some(ApiFormat::OpenApi)
    } else if root.get("asyncapi").is_some() {
        Some(ApiFormat::AsyncApi)
    } else if root.get("smithy").is_some() && root.get("shapes").is_some() {
        Some(ApiFormat::Smithy)
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

/// The API name used when none is configured: the entry file's stem,
/// without the `.jtd` of a compound `.jtd.json` extension.
pub fn default_api_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map_or_else(|| "api".to_owned(), |s| s.to_string_lossy().into_owned());
    let end = stem.len().saturating_sub(4);
    match (stem.get(..end), stem.get(end..)) {
        (Some(name), Some(suffix)) if !name.is_empty() && suffix.eq_ignore_ascii_case(".jtd") => {
            name.to_owned()
        }
        _ => stem,
    }
}

#[cfg(test)]
mod tests {
    use super::default_api_name as stem;
    use std::path::Path;

    #[test]
    fn stems() {
        assert_eq!(stem(Path::new("api/openapi.yaml")), "openapi");
        assert_eq!(stem(Path::new("/")), "api");
        assert_eq!(stem(Path::new("schemas/user.jtd.json")), "user");
        assert_eq!(stem(Path::new(".jtd.json")), ".jtd");
        assert_eq!(stem(Path::new("ééa.json")), "ééa");
    }
}

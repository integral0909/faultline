//! One analysis of a workspace: every API compared with the base revision,
//! with open documents read from the editor, and the results placed in the
//! files they concern.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use faultline::{Engine, Revision, Source, codes, compare_apis, default_api_name};
use faultline_doc::loader::{FsLoader, workspace_uri};
use faultline_policy::{
    ApiFormat, Baseline, Config, Date, Evaluated, Evaluation, Release, SchemaRole, Status, evaluate,
};
use faultline_rules::{Finding, Rule, Severity as FindingSeverity};
use faultline_source::{Diagnostic, Severity, SourceMap, SourceName, Span};
use faultline_usage::UsageIndex;

use crate::overlay::OverlayLoader;

/// The name of the configuration file, at the workspace folder's root.
pub(crate) const CONFIG_FILE: &str = "faultline.toml";

/// A position as LSP counts it: zero-based line and UTF-16 column.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Position {
    pub(crate) line: u32,
    pub(crate) character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Range {
    pub(crate) start: Position,
    pub(crate) end: Position,
}

impl Range {
    /// Whether `position` falls within the range, ends included, so that a
    /// cursor just after a name still counts.
    pub(crate) fn contains(self, position: Position) -> bool {
        self.start <= position && position <= self.end
    }

    pub(crate) fn overlaps(self, other: Range) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// LSP diagnostic severities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Level {
    Error = 1,
    Warning = 2,
    Information = 3,
    Hint = 4,
}

/// A finding shown in a file, with what hover and code actions need.
#[derive(Clone, Debug)]
pub(crate) struct Shown {
    pub(crate) id: String,
    pub(crate) rule: &'static Rule,
    pub(crate) severity: FindingSeverity,
    pub(crate) api: String,
    pub(crate) path: String,
    pub(crate) witness: Option<String>,
    /// Waived, baselined, or softened by usage, in words.
    pub(crate) status: Option<String>,
    /// Whether a waiver would change anything.
    pub(crate) waivable: bool,
    /// Where the change was in the base revision, when it is not shown
    /// there.
    pub(crate) was_at: Option<String>,
}

/// One diagnostic to publish.
#[derive(Clone, Debug)]
pub(crate) struct Item {
    pub(crate) range: Range,
    pub(crate) level: Level,
    pub(crate) code: Option<String>,
    pub(crate) message: String,
    pub(crate) finding: Option<Shown>,
    /// Other places the diagnostic mentions, by workspace-relative path.
    pub(crate) related: Vec<(PathBuf, Range, String)>,
}

/// An open document.
#[derive(Clone, Debug)]
pub(crate) struct Open<'a> {
    /// Relative to the workspace root.
    pub(crate) relative: PathBuf,
    pub(crate) text: &'a str,
}

/// What an analysis reads.
#[derive(Debug)]
pub(crate) struct Workspace<'a> {
    /// The workspace folder: where `faultline.toml` is.
    pub(crate) folder: &'a Path,
    /// The canonical root references may reach: the folder, or the
    /// configuration's `root` below it.
    pub(crate) root: &'a Path,
    pub(crate) config: &'a Config,
    pub(crate) base: &'a str,
    pub(crate) open: &'a [Open<'a>],
    pub(crate) engine: &'a Engine,
    pub(crate) today: Date,
}

/// What an analysis found.
#[derive(Debug, Default)]
pub(crate) struct Analysis {
    /// Diagnostics by path relative to the workspace root.
    pub(crate) files: BTreeMap<PathBuf, Vec<Item>>,
    /// Problems that belong to no file, such as a missing base revision.
    pub(crate) messages: Vec<String>,
}

impl Analysis {
    fn push(&mut self, file: PathBuf, item: Item) {
        let items = self.files.entry(file).or_default();
        let duplicate = items.iter().any(|i| {
            i.range == item.range
                && i.code == item.code
                && i.message == item.message
                && i.finding.as_ref().map(|f| &f.id) == item.finding.as_ref().map(|f| &f.id)
        });
        if !duplicate {
            items.push(item);
        }
    }

    pub(crate) fn message(&mut self, text: String) {
        if !self.messages.contains(&text) {
            self.messages.push(text);
        }
    }
}

/// One API to check.
struct Target {
    name: String,
    /// Relative to the root.
    entry: PathBuf,
    /// Relative to the root.
    overlays: Vec<PathBuf>,
    format: Option<ApiFormat>,
    role: SchemaRole,
}

/// A target's comparison, before policy.
struct Compared {
    name: String,
    /// Where findings without a place in the working tree are shown.
    anchor: PathBuf,
    findings: Vec<Finding>,
    release: Option<Release>,
}

/// Compares every target and places the results.
pub(crate) fn analyze(workspace: &Workspace<'_>) -> Analysis {
    let mut analysis = Analysis::default();
    let targets = targets(workspace);
    let sources = SourceMap::new();
    let mut compared = Vec::new();
    let mut complete = true;
    for target in &targets {
        match compare(workspace, target, &sources, &mut analysis) {
            Some(c) => compared.push(c),
            None => complete = false,
        }
    }

    let baseline = read_policy_file(workspace, workspace.config.baseline.as_deref(), |text| {
        Baseline::parse(text).map_err(|e| e.to_string())
    });
    let baseline = match baseline {
        Ok(baseline) => baseline,
        Err(message) => {
            analysis.message(message);
            None
        }
    };
    let mut anchors = BTreeMap::new();
    let mut findings = Vec::new();
    let mut releases = Vec::new();
    for c in compared {
        findings.extend(c.findings);
        releases.extend(c.release.map(|r| (c.anchor.clone(), r)));
        anchors.insert(c.name, c.anchor);
    }
    let mut evaluation = evaluate(
        workspace.config,
        baseline.as_ref(),
        findings,
        workspace.today,
    );
    evaluation.check_releases(workspace.config.versioning, releases.iter().map(|(_, r)| r));
    let usage = read_policy_file(workspace, workspace.config.usage.index.as_deref(), |text| {
        UsageIndex::parse(text)
    });
    match usage {
        Ok(Some(index)) => faultline_usage::apply(
            &mut evaluation,
            &index,
            workspace.config.usage.unused_severity,
        ),
        Ok(None) => {}
        Err(message) => analysis.message(message),
    }

    for evaluated in &evaluation.findings {
        let anchor = anchors
            .get(&evaluated.finding.api)
            .cloned()
            .unwrap_or_else(|| config_relative(workspace));
        place_finding(workspace, &sources, evaluated, anchor, &mut analysis);
    }
    place_policy(workspace, &evaluation, &releases, complete, &mut analysis);
    analysis
}

/// Declared versions that increase too little, and waivers that need
/// attention.
fn place_policy(
    workspace: &Workspace<'_>,
    evaluation: &Evaluation,
    releases: &[(PathBuf, Release)],
    complete: bool,
    analysis: &mut Analysis,
) {
    for violation in &evaluation.version_violations {
        let Some((anchor, _)) = releases.iter().find(|(_, r)| r.api == violation.api) else {
            continue;
        };
        let declared = match violation.declared {
            faultline_rules::Bump::None => "no version increase".to_owned(),
            bump => format!("a {bump} release"),
        };
        analysis.push(
            anchor.clone(),
            Item {
                range: Range::default(),
                level: Level::Error,
                code: None,
                message: format!(
                    "{} declares {} -> {}, {declared}, but its changes need a {} release",
                    violation.api, violation.old_version, violation.new_version, violation.required
                ),
                finding: None,
                related: Vec::new(),
            },
        );
    }
    let config_file = config_relative(workspace);
    for waiver in &evaluation.expired_waivers {
        let (range, file) = waiver_range(workspace, &config_file, &waiver.finding);
        analysis.push(
            file,
            Item {
                range,
                level: Level::Error,
                code: None,
                message: format!(
                    "the waiver for {} expired on {}; renew it or remove it",
                    waiver.finding, waiver.expires
                ),
                finding: None,
                related: Vec::new(),
            },
        );
    }
    // A waiver only looks unused when every API could be compared.
    if complete {
        for waiver in &evaluation.unused_waivers {
            let (range, file) = waiver_range(workspace, &config_file, &waiver.finding);
            analysis.push(
                file,
                Item {
                    range,
                    level: Level::Hint,
                    code: None,
                    message: format!(
                        "the waiver for {} matches no finding; it can be removed",
                        waiver.finding
                    ),
                    finding: None,
                    related: Vec::new(),
                },
            );
        }
    }
}

/// The configured APIs, or, without any, each open document that looks
/// like an API description on its own.
fn targets(workspace: &Workspace<'_>) -> Vec<Target> {
    if !workspace.config.apis.is_empty() {
        return workspace
            .config
            .apis
            .iter()
            .map(|api| Target {
                name: api.name.clone(),
                entry: relative_to(workspace.root, &api.entry),
                overlays: api
                    .overlays
                    .iter()
                    .map(|o| relative_to(workspace.root, o))
                    .collect(),
                format: api.format,
                role: api.role,
            })
            .collect();
    }
    workspace
        .open
        .iter()
        .filter(|doc| looks_like_api(&doc.relative, doc.text))
        .map(|doc| Target {
            name: default_api_name(&doc.relative),
            entry: doc.relative.clone(),
            overlays: Vec::new(),
            format: None,
            role: SchemaRole::Both,
        })
        .collect()
}

/// A configured path relative to the root, if it is inside it.
fn relative_to(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.strip_prefix(root)
            .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
    } else {
        path.to_path_buf()
    }
}

/// Whether a document is worth comparing without configuration: schema
/// languages by extension (RAML only for APIs, not fragments such as
/// libraries, which are compared through the API using them), JSON and
/// YAML only when they declare themselves OpenAPI, AsyncAPI, JSON Schema or
/// a Smithy JSON AST (so that `package.json` stays quiet).
pub(crate) fn looks_like_api(path: &Path, text: &str) -> bool {
    let entry = workspace_uri(path);
    if let Some(format) = faultline::format_by_extension(&entry) {
        if format == faultline_policy::ApiFormat::Raml {
            let header = text.lines().next().unwrap_or_default().trim();
            return matches!(header, "#%RAML 1.0" | "#%RAML 0.8");
        }
        return true;
    }
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if !matches!(extension.as_deref(), Some("yaml" | "yml" | "json")) {
        return false;
    }
    // Only the start of the document is examined: these keys come first
    // by convention, and a key deep inside is more likely an example.
    let head: String = text.chars().take(4096).collect();
    head.lines().any(|line| {
        let key = line.trim_start().trim_start_matches(['{', '"', '\'']);
        ["openapi", "swagger", "asyncapi", "$schema", "smithy"]
            .iter()
            .any(|k| key.starts_with(k))
    })
}

/// Lowers both versions of `target` and compares them. `None` when the
/// comparison could not be made.
fn compare(
    workspace: &Workspace<'_>,
    target: &Target,
    sources: &SourceMap,
    analysis: &mut Analysis,
) -> Option<Compared> {
    let engine = workspace.engine;
    let anchor = if workspace.root.join(&target.entry).is_file()
        || workspace.open.iter().any(|d| d.relative == target.entry)
    {
        target.entry.clone()
    } else {
        config_relative(workspace)
    };

    let disk = match FsLoader::new(workspace.root, engine.limits.clone()) {
        Ok(loader) => loader,
        Err(e) => {
            analysis.message(format!(
                "cannot use {} as the workspace root: {e}",
                workspace.root.display()
            ));
            return None;
        }
    };
    let mut overlay = OverlayLoader::new(&disk);
    for doc in workspace.open {
        overlay.insert(&doc.relative, doc.text);
    }
    let new = engine.lower_through(
        &overlay,
        &Source::new(workspace.root, &target.entry).with_overlays(target.overlays.clone()),
        &target.name,
        target.format,
        target.role,
        sources,
    );
    let old_source = Source::new(workspace.root, &target.entry)
        .at(Revision::Named(workspace.base.to_owned()))
        .with_overlays(target.overlays.clone());
    let old = engine.lower(
        &old_source,
        &target.name,
        target.format,
        target.role,
        sources,
    );

    let new_clean = !new.diagnostics.has_errors();
    for diagnostic in &new.diagnostics {
        place_diagnostic(sources, diagnostic, &anchor, &target.name, analysis);
    }
    if old.missing_entry {
        // A new API: nothing to compare it with.
        return None;
    }
    let unreadable = old.diagnostics.iter().find(|d| {
        d.is_error()
            && matches!(
                d.code,
                Some(c) if c == codes::GIT || c == codes::UNKNOWN_REVISION || c == codes::INVALID_ROOT
            )
    });
    if let Some(diagnostic) = unreadable {
        analysis.message(format!(
            "cannot compare with {}: {}",
            workspace.base, diagnostic.message
        ));
        return None;
    }
    if let Some(error) = old.diagnostics.iter().find(|d| d.is_error()) {
        let place = error
            .primary_span()
            .and_then(|span| describe(sources, span))
            .map_or_else(String::new, |at| format!(" ({at})"));
        analysis.push(
            anchor.clone(),
            Item {
                range: Range::default(),
                level: Level::Warning,
                code: error.code.map(|c| c.to_string()),
                message: format!(
                    "{}: the version in {} has errors, so it was not compared: {}{place}",
                    target.name, workspace.base, error.message
                ),
                finding: None,
                related: Vec::new(),
            },
        );
        return None;
    }
    let (Some(old_api), Some(new_api), true) = (old.api, new.api, new_clean) else {
        return None;
    };
    let (findings, summary) = compare_apis(&target.name, &old_api, &new_api);
    Some(Compared {
        name: target.name.clone(),
        anchor,
        findings,
        release: Some(Release {
            api: summary.name,
            old_version: summary.old_version,
            new_version: summary.new_version,
            required: summary.bump,
        }),
    })
}

/// Where a span is, for a working-tree file: its relative path and range.
fn locate(sources: &SourceMap, span: Span) -> Option<(PathBuf, Range)> {
    let span = sources.outermost(span);
    let file = sources.get(span.source())?;
    let SourceName::Path(path) = file.name() else {
        return None;
    };
    let text = file.text();
    let lines = file.lines();
    let start = lines.line_col_utf16(span.start(), text);
    let end = lines.line_col_utf16(span.end(), text);
    Some((
        path.clone(),
        Range {
            start: Position {
                line: start.line,
                character: start.col,
            },
            end: Position {
                line: end.line,
                character: end.col,
            },
        },
    ))
}

/// A span in words, `rev:path:line`, for places no editor shows.
fn describe(sources: &SourceMap, span: Span) -> Option<String> {
    let file = sources.get(span.source())?;
    let line = file.lines().line_col(span.start()).line + 1;
    Some(format!("{}:{line}", file.name()))
}

fn place_diagnostic(
    sources: &SourceMap,
    diagnostic: &Diagnostic,
    anchor: &Path,
    api: &str,
    analysis: &mut Analysis,
) {
    let level = match diagnostic.severity {
        Severity::Error => Level::Error,
        Severity::Warning => Level::Warning,
        Severity::Note => Level::Information,
    };
    let mut message = diagnostic.message.clone();
    for note in &diagnostic.notes {
        message.push_str("\nnote: ");
        message.push_str(note);
    }
    if let Some(help) = &diagnostic.help {
        message.push_str("\nhelp: ");
        message.push_str(help);
    }
    let place = diagnostic.primary_span().and_then(|s| locate(sources, s));
    let (file, range) = place.unwrap_or_else(|| {
        message = format!("{api}: {message}");
        (anchor.to_path_buf(), Range::default())
    });
    let related = diagnostic
        .labels
        .iter()
        .filter(|l| !l.primary)
        .filter_map(|l| {
            let (path, range) = locate(sources, l.span)?;
            Some((path, range, l.message.clone().unwrap_or_default()))
        })
        .collect();
    analysis.push(
        file,
        Item {
            range,
            level,
            code: diagnostic.code.map(|c| c.to_string()),
            message,
            finding: None,
            related,
        },
    );
}

fn place_finding(
    workspace: &Workspace<'_>,
    sources: &SourceMap,
    evaluated: &Evaluated,
    anchor: PathBuf,
    analysis: &mut Analysis,
) {
    let finding = &evaluated.finding;
    let change = &finding.change;
    let here = change.new_span.and_then(|s| locate(sources, s));
    let was_at = change
        .old_span
        .and_then(|s| describe(sources, s))
        .filter(|_| here.is_none());
    let (file, range) = here.unwrap_or((anchor, Range::default()));
    let status = match &evaluated.status {
        Status::Active if evaluated.unused => {
            Some("the usage index shows this part of the API unused".to_owned())
        }
        Status::Active => None,
        Status::Waived(waiver) => Some(format!(
            "waived until {}: {}",
            waiver.expires, waiver.reason
        )),
        Status::Baselined => Some("accepted in the baseline".to_owned()),
    };
    let level = match (&evaluated.status, finding.severity) {
        (Status::Waived(_) | Status::Baselined, _) | (_, FindingSeverity::Info) => Level::Hint,
        (_, FindingSeverity::Breaking) => Level::Error,
        (_, FindingSeverity::Risky) => Level::Warning,
    };
    let mut message = format!("{}: {}", finding.api, finding.message);
    if let Some(at) = &was_at {
        let _ = write!(message, " (in {}: {at})", workspace.base);
    }
    if let Some(status) = &status {
        let _ = write!(message, " [{status}]");
    }
    let shown = Shown {
        id: finding.id.clone(),
        rule: finding.rule,
        severity: finding.severity,
        api: finding.api.clone(),
        path: change.path.to_string(),
        witness: finding.witness().map(ToString::to_string),
        status,
        waivable: matches!(evaluated.status, Status::Active)
            && finding.severity != FindingSeverity::Info,
        was_at,
    };
    analysis.push(
        file,
        Item {
            range,
            level,
            code: Some(finding.rule.code.to_owned()),
            message,
            finding: Some(shown),
            related: Vec::new(),
        },
    );
}

/// Reads a file the policy names (a baseline, a usage index), relative to
/// the workspace root. Absent files are not an error: a new workspace has
/// no baseline yet.
fn read_policy_file<T>(
    workspace: &Workspace<'_>,
    path: Option<&Path>,
    parse: impl FnOnce(&str) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let path = workspace.root.join(path);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    parse(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// `faultline.toml`, relative to the root.
fn config_relative(workspace: &Workspace<'_>) -> PathBuf {
    let config = workspace.folder.join(CONFIG_FILE);
    config
        .strip_prefix(workspace.root)
        .map_or_else(|_| PathBuf::from(CONFIG_FILE), Path::to_path_buf)
}

/// The line of `faultline.toml` naming a waiver's finding.
fn waiver_range(workspace: &Workspace<'_>, config_file: &Path, id: &str) -> (Range, PathBuf) {
    let text = workspace
        .open
        .iter()
        .find(|d| d.relative == config_file)
        .map(|d| d.text.to_owned())
        .or_else(|| std::fs::read_to_string(workspace.root.join(config_file)).ok())
        .unwrap_or_default();
    let quoted = format!("\"{id}\"");
    let range = text
        .lines()
        .position(|line| line.contains(&quoted))
        .map_or_else(Range::default, |line| {
            let line = u32::try_from(line).unwrap_or(u32::MAX);
            let length = text
                .lines()
                .nth(line as usize)
                .map_or(0, |l| l.encode_utf16().count());
            Range {
                start: Position { line, character: 0 },
                end: Position {
                    line,
                    character: u32::try_from(length).unwrap_or(u32::MAX),
                },
            }
        });
    (range, config_file.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_api_documents() {
        let yes = |p: &str, t: &str| looks_like_api(Path::new(p), t);
        assert!(yes("a.proto", ""));
        assert!(yes("schema.graphql", ""));
        assert!(yes("user.avsc", "{}"));
        assert!(yes("api.yaml", "openapi: 3.1.0\ninfo: {}\n"));
        assert!(yes("events.yaml", "asyncapi: 3.0.0\n"));
        assert!(yes("api.json", "{\n  \"openapi\": \"3.0.3\"\n}"));
        assert!(yes(
            "pet.json",
            "{\"$schema\": \"https://json-schema.org/draft/2020-12/schema\"}"
        ));
        assert!(!yes("package.json", "{\"name\": \"x\"}"));
        assert!(!yes("compose.yaml", "services:\n  api: {}\n"));
        assert!(!yes("notes.md", "openapi"));
        assert!(yes("pets.thrift", ""));
        assert!(yes("model/weather.smithy", ""));
        assert!(yes(
            "model.json",
            "{\n  \"smithy\": \"2.0\",\n  \"shapes\": {}\n}"
        ));
        assert!(yes("api.raml", "#%RAML 1.0\ntitle: T\n"));
        assert!(yes("old.raml", "#%RAML 0.8\ntitle: T\n"));
        assert!(!yes("lib.raml", "#%RAML 1.0 Library\n"), "a fragment");
    }

    #[test]
    fn ranges() {
        let p = |line, character| Position { line, character };
        let r = Range {
            start: p(1, 4),
            end: p(1, 9),
        };
        assert!(r.contains(p(1, 4)) && r.contains(p(1, 9)));
        assert!(!r.contains(p(1, 10)) && !r.contains(p(0, 5)));
        assert!(r.overlaps(Range {
            start: p(1, 9),
            end: p(2, 0)
        }));
        assert!(!r.overlaps(Range {
            start: p(2, 0),
            end: p(2, 1)
        }));
    }
}

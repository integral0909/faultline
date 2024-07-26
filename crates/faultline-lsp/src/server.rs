//! The language server's state and its handling of each message.
//!
//! [`Server::handle`] takes one incoming message and returns the messages
//! to send back; it never analyzes. Edits only mark the workspace dirty, and
//! the caller runs [`Server::analyze`] once the editor has been quiet for a
//! moment, so a burst of keystrokes costs one comparison.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use faultline::Engine;
use faultline_policy::{Config, Date, NetworkConfig};
use serde_json::{Value, json};

use crate::analysis::{
    self, Analysis, CONFIG_FILE, Item, Level, Open, Position, Range, Shown, Workspace,
};
use crate::protocol::{code, error_response, notification, response};
use crate::uri;

/// The base revision compared with when the editor does not choose one:
/// the last commit, so the diagnostics show what uncommitted edits change.
pub const DEFAULT_BASE: &str = "HEAD";

/// How long a new waiver lasts.
const WAIVER_DAYS: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Uninitialized,
    Running,
    ShutDown,
}

#[derive(Debug)]
struct Document {
    /// The path the editor's URI names, as given.
    path: PathBuf,
    text: String,
}

/// A language server for one workspace folder.
pub struct Server {
    phase: Phase,
    /// The canonical workspace folder, holding `faultline.toml`.
    folder: Option<PathBuf>,
    base: String,
    documents: BTreeMap<String, Document>,
    /// The engine and the network settings it was built for, reused while
    /// they stay the same so remote documents are fetched once.
    engine: Option<(NetworkConfig, Engine)>,
    today: Date,
    dirty: bool,
    /// Results of the last analysis by document URI, for hover and code
    /// actions.
    shown: HashMap<String, Vec<Item>>,
    /// Messages already shown, so each is shown once.
    told: HashSet<String>,
    exit_code: Option<i32>,
}

impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("phase", &self.phase)
            .field("folder", &self.folder)
            .field("base", &self.base)
            .field("documents", &self.documents.len())
            .finish_non_exhaustive()
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Self {
            phase: Phase::Uninitialized,
            folder: None,
            base: DEFAULT_BASE.to_owned(),
            documents: BTreeMap::new(),
            engine: None,
            today: Date::today(),
            dirty: false,
            shown: HashMap::new(),
            told: HashSet::new(),
            exit_code: None,
        }
    }

    /// Compares with `base` unless the editor chooses another revision.
    #[must_use]
    pub fn with_base(mut self, base: &str) -> Self {
        base.clone_into(&mut self.base);
        self
    }

    /// Uses `today` for waiver expiry instead of the clock.
    #[must_use]
    pub fn with_today(mut self, today: Date) -> Self {
        self.today = today;
        self
    }

    /// Whether edits are waiting for [`Server::analyze`].
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The process exit code, once the client has sent `exit`.
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Handles one message, returning responses and notifications to send.
    pub fn handle(&mut self, message: &Value) -> Vec<Value> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to a request of ours; the server sends none that
            // need an answer.
            return Vec::new();
        };
        let params = message.get("params").unwrap_or(&Value::Null);
        if let Some(id) = message.get("id") {
            vec![self.request(id, method, params)]
        } else {
            self.notify(method, params);
            Vec::new()
        }
    }

    fn request(&mut self, id: &Value, method: &str, params: &Value) -> Value {
        match (self.phase, method) {
            (Phase::Uninitialized, "initialize") => response(id, &self.initialize(params)),
            (Phase::Uninitialized, _) => error_response(
                id,
                code::SERVER_NOT_INITIALIZED,
                "the server has not been initialized",
            ),
            (Phase::ShutDown, _) => {
                error_response(id, code::INVALID_REQUEST, "the server is shutting down")
            }
            (Phase::Running, "initialize") => error_response(
                id,
                code::INVALID_REQUEST,
                "the server is already initialized",
            ),
            (Phase::Running, "shutdown") => {
                self.phase = Phase::ShutDown;
                response(id, &Value::Null)
            }
            (Phase::Running, "textDocument/hover") => match position_params(params) {
                Some((uri, position)) => response(id, &self.hover(&uri, position)),
                None => invalid_params(id),
            },
            (Phase::Running, "textDocument/codeAction") => match self.code_actions(params) {
                Some(actions) => response(id, &actions),
                None => invalid_params(id),
            },
            (Phase::Running, _) => error_response(
                id,
                code::METHOD_NOT_FOUND,
                format!("unsupported request {method}"),
            ),
        }
    }

    fn notify(&mut self, method: &str, params: &Value) {
        if method == "exit" {
            self.exit_code = Some(i32::from(self.phase != Phase::ShutDown));
            return;
        }
        if self.phase != Phase::Running {
            return;
        }
        match method {
            "initialized" | "workspace/didChangeWatchedFiles" | "textDocument/didSave" => {
                self.dirty = true;
            }
            "workspace/didChangeConfiguration" => {
                if let Some(base) = params
                    .pointer("/settings/faultline/base")
                    .and_then(Value::as_str)
                {
                    base.clone_into(&mut self.base);
                    self.dirty = true;
                }
            }
            "textDocument/didOpen" => {
                let document = &params["textDocument"];
                if let (Some(uri), Some(text)) =
                    (document["uri"].as_str(), document["text"].as_str())
                {
                    self.set_text(uri, text.to_owned());
                }
            }
            "textDocument/didChange" => {
                let uri = params.pointer("/textDocument/uri").and_then(Value::as_str);
                // Full synchronization: the last change holds the whole text.
                let text = params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.last())
                    .and_then(|change| change["text"].as_str());
                if let (Some(uri), Some(text)) = (uri, text) {
                    self.set_text(uri, text.to_owned());
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = params.pointer("/textDocument/uri").and_then(Value::as_str)
                    && self.documents.remove(uri).is_some()
                {
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    fn set_text(&mut self, uri: &str, text: String) {
        // Documents that are not files (unsaved buffers, other schemes)
        // cannot be part of a workspace comparison.
        let Some(path) = uri::to_path(uri) else {
            return;
        };
        self.documents
            .insert(uri.to_owned(), Document { path, text });
        self.dirty = true;
    }

    fn initialize(&mut self, params: &Value) -> Value {
        let folder = params["workspaceFolders"]
            .as_array()
            .and_then(|folders| folders.first())
            .and_then(|folder| folder["uri"].as_str())
            .or_else(|| params["rootUri"].as_str())
            .and_then(uri::to_path)
            .or_else(|| params["rootPath"].as_str().map(PathBuf::from));
        self.folder = folder.map(|f| f.canonicalize().unwrap_or(f));
        if let Some(base) = params
            .pointer("/initializationOptions/base")
            .and_then(Value::as_str)
        {
            base.clone_into(&mut self.base);
        }
        self.phase = Phase::Running;
        json!({
            "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {
                    "openClose": true,
                    "change": 1,
                    "save": {"includeText": false},
                },
                "hoverProvider": true,
                "codeActionProvider": {"codeActionKinds": ["quickfix"]},
            },
            "serverInfo": {"name": "faultline", "version": env!("CARGO_PKG_VERSION")},
        })
    }

    /// Compares the workspace and returns the notifications publishing the
    /// results: diagnostics for every file that has some or had some
    /// before, and each new message about the workspace as a whole.
    pub fn analyze(&mut self) -> Vec<Value> {
        self.dirty = false;
        let Some(folder) = self.folder.clone() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let (analysis, root) = self.run_analysis(&folder);
        for text in &analysis.messages {
            if self.told.insert(text.clone()) {
                out.push(notification(
                    "window/showMessage",
                    &json!({"type": 2, "message": format!("faultline: {text}")}),
                ));
            }
        }
        let mut shown: HashMap<String, Vec<Item>> = HashMap::new();
        for (relative, items) in analysis.files {
            let uri = self.uri_for(&root.join(&relative));
            let diagnostics: Vec<Value> = items
                .iter()
                .map(|item| self.diagnostic(&root, item))
                .collect();
            out.push(notification(
                "textDocument/publishDiagnostics",
                &json!({"uri": uri, "diagnostics": diagnostics}),
            ));
            shown.entry(uri).or_default().extend(items);
        }
        // Clear files whose diagnostics are gone.
        let mut stale: Vec<&String> = self
            .shown
            .keys()
            .filter(|uri| !shown.contains_key(*uri))
            .collect();
        stale.sort();
        for uri in stale {
            out.push(notification(
                "textDocument/publishDiagnostics",
                &json!({"uri": uri, "diagnostics": []}),
            ));
        }
        self.shown = shown;
        out
    }

    /// Runs one analysis, returning it with the root its paths are
    /// relative to.
    fn run_analysis(&mut self, folder: &Path) -> (Analysis, PathBuf) {
        let config_path = folder.join(CONFIG_FILE);
        let config_text = self
            .documents
            .values()
            .find(|d| same_file(&d.path, &config_path))
            .map(|d| d.text.clone())
            .or_else(|| std::fs::read_to_string(&config_path).ok());
        let config = match config_text.as_deref().map(Config::parse) {
            None => Config::default(),
            Some(Ok(config)) => config,
            Some(Err(e)) => {
                let mut analysis = Analysis::default();
                analysis.files.insert(
                    PathBuf::from(CONFIG_FILE),
                    vec![Item {
                        range: Range::default(),
                        level: Level::Error,
                        code: None,
                        message: e.to_string(),
                        finding: None,
                        related: Vec::new(),
                    }],
                );
                return (analysis, folder.to_path_buf());
            }
        };
        let root = config.root.as_ref().map_or_else(
            || folder.to_path_buf(),
            |r| {
                let joined = folder.join(r);
                joined.canonicalize().unwrap_or(joined)
            },
        );
        let engine = match &self.engine {
            Some((network, engine)) if *network == config.network => engine.clone(),
            _ => match Engine::default().with_network(&config.network) {
                Ok(engine) => {
                    self.engine = Some((config.network.clone(), engine.clone()));
                    engine
                }
                Err(e) => {
                    let mut analysis = Analysis::default();
                    analysis.message(format!("invalid configuration: {e}"));
                    return (analysis, root);
                }
            },
        };
        let open: Vec<Open<'_>> = self
            .documents
            .values()
            .filter_map(|d| {
                Some(Open {
                    relative: relative_path(&root, &d.path)?,
                    text: &d.text,
                })
            })
            .collect();
        let workspace = Workspace {
            folder,
            root: &root,
            config: &config,
            base: &self.base,
            open: &open,
            engine: &engine,
            today: self.today,
        };
        let analysis = analysis::analyze(&workspace);
        (analysis, root)
    }

    /// The URI to publish for `path`: the editor's own spelling when the
    /// document is open, so its diagnostics land on the right tab.
    fn uri_for(&self, path: &Path) -> String {
        self.documents
            .iter()
            .find(|(_, d)| same_file(&d.path, path))
            .map_or_else(|| uri::from_path(path), |(uri, _)| uri.clone())
    }

    fn diagnostic(&self, root: &Path, item: &Item) -> Value {
        let mut diagnostic = json!({
            "range": range_json(item.range),
            "severity": item.level as u8,
            "source": "faultline",
            "message": item.message,
        });
        if let Some(code) = &item.code {
            diagnostic["code"] = json!(code);
        }
        if let Some(finding) = &item.finding {
            diagnostic["data"] = json!({"findingId": finding.id});
        }
        if !item.related.is_empty() {
            diagnostic["relatedInformation"] = item
                .related
                .iter()
                .map(|(path, range, message)| {
                    json!({
                        "location": {
                            "uri": self.uri_for(&root.join(path)),
                            "range": range_json(*range),
                        },
                        "message": message,
                    })
                })
                .collect();
        }
        diagnostic
    }

    fn hover(&self, uri: &str, position: Position) -> Value {
        let found = self
            .shown
            .get(uri)
            .into_iter()
            .flatten()
            .filter(|item| item.range.contains(position))
            .filter_map(|item| Some((item.range, item.finding.as_ref()?)))
            .collect::<Vec<_>>();
        if found.is_empty() {
            return Value::Null;
        }
        let text = found
            .iter()
            .map(|(_, shown)| hover_text(shown))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        json!({
            "contents": {"kind": "markdown", "value": text},
            "range": range_json(found[0].0),
        })
    }

    fn code_actions(&self, params: &Value) -> Option<Value> {
        let uri = params.pointer("/textDocument/uri")?.as_str()?;
        let range = parse_range(params.get("range")?)?;
        let context: &[Value] = params
            .pointer("/context/diagnostics")
            .and_then(Value::as_array)
            .map_or(&[], Vec::as_slice);
        let items = self.shown.get(uri).map_or(&[][..], Vec::as_slice);
        let mut seen = HashSet::new();
        let mut actions = Vec::new();
        for item in items {
            let Some(shown) = &item.finding else {
                continue;
            };
            let named = context
                .iter()
                .any(|d| d.pointer("/data/findingId").and_then(Value::as_str) == Some(&shown.id));
            if !(named || item.range.overlaps(range)) || !shown.waivable {
                continue;
            }
            if !seen.insert(shown.id.clone()) {
                continue;
            }
            let diagnostics: Vec<&Value> = context
                .iter()
                .filter(|d| d.pointer("/data/findingId").and_then(Value::as_str) == Some(&shown.id))
                .collect();
            actions.push(json!({
                "title": format!("Waive {} ({}) for {WAIVER_DAYS} days in {CONFIG_FILE}", shown.id, shown.rule.name),
                "kind": "quickfix",
                "diagnostics": diagnostics,
                "edit": self.waiver_edit(shown)?,
            }));
        }
        Some(Value::Array(actions))
    }

    /// An edit appending a waiver for `shown` to `faultline.toml`, creating
    /// the file if there is none.
    fn waiver_edit(&self, shown: &Shown) -> Option<Value> {
        let path = self.folder.as_ref()?.join(CONFIG_FILE);
        let open = self
            .documents
            .iter()
            .find(|(_, d)| same_file(&d.path, &path));
        let uri = open.map_or_else(|| uri::from_path(&path), |(uri, _)| uri.clone());
        let existing = open
            .map(|(_, d)| d.text.clone())
            .or_else(|| std::fs::read_to_string(&path).ok());
        let expires = self.today.plus_days(WAIVER_DAYS);
        let mut block = format!(
            "[[waiver]]\n# {} {} in {}: {}\nfinding = \"{}\"\nreason = \"<why this change is acceptable>\"\nexpires = \"{expires}\"\n",
            shown.rule.code, shown.rule.name, shown.api, shown.path, shown.id
        );
        let Some(text) = existing else {
            return Some(json!({
                "documentChanges": [
                    {"kind": "create", "uri": uri, "options": {"ignoreIfExists": true}},
                    {
                        "textDocument": {"uri": uri, "version": null},
                        "edits": [{"range": range_json(Range::default()), "newText": block}],
                    },
                ],
            }));
        };
        if !text.is_empty() {
            let separator = if text.ends_with("\n\n") {
                ""
            } else if text.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            };
            block.insert_str(0, separator);
        }
        let end = end_of(&text);
        Some(json!({
            "changes": {
                uri: [{"range": range_json(Range { start: end, end }), "newText": block}],
            },
        }))
    }
}

fn invalid_params(id: &Value) -> Value {
    error_response(id, code::INVALID_PARAMS, "invalid parameters")
}

fn hover_text(shown: &Shown) -> String {
    let severity = match shown.severity {
        faultline_rules::Severity::Breaking => "breaking",
        faultline_rules::Severity::Risky => "risky",
        faultline_rules::Severity::Info => "info",
    };
    let mut text = format!(
        "**{} {}** ({severity}): {}\n\n",
        shown.rule.code, shown.rule.name, shown.rule.title
    );
    if let Some(status) = &shown.status {
        let _ = write!(text, "*{status}*\n\n");
    }
    let _ = write!(text, "API `{}`, at `{}`", shown.api, shown.path);
    if let Some(at) = &shown.was_at {
        let _ = write!(text, ", previously {at}");
    }
    text.push_str("\n\n");
    if let Some(witness) = &shown.witness {
        let _ = write!(text, "Example value: `{witness}`\n\n");
    }
    // The first paragraph of the rule's explanation says what it means.
    if let Some(paragraph) = shown
        .rule
        .explanation
        .split("\n\n")
        .map(str::trim)
        .find(|p| !p.is_empty())
    {
        text.push_str(paragraph);
        text.push_str("\n\n");
    }
    let _ = write!(
        text,
        "Finding id `{}`; `faultline explain {}` has more.",
        shown.id, shown.rule.code
    );
    text
}

fn position_params(params: &Value) -> Option<(String, Position)> {
    let uri = params.pointer("/textDocument/uri")?.as_str()?.to_owned();
    Some((uri, parse_position(params.get("position")?)?))
}

fn parse_position(value: &Value) -> Option<Position> {
    Some(Position {
        line: u32::try_from(value["line"].as_u64()?).ok()?,
        character: u32::try_from(value["character"].as_u64()?).ok()?,
    })
}

fn parse_range(value: &Value) -> Option<Range> {
    Some(Range {
        start: parse_position(value.get("start")?)?,
        end: parse_position(value.get("end")?)?,
    })
}

fn range_json(range: Range) -> Value {
    json!({
        "start": {"line": range.start.line, "character": range.start.character},
        "end": {"line": range.end.line, "character": range.end.character},
    })
}

/// The position just past the last character of `text`.
fn end_of(text: &str) -> Position {
    let line = text.matches('\n').count();
    let last = text.rsplit('\n').next().unwrap_or("");
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(last.encode_utf16().count()).unwrap_or(u32::MAX),
    }
}

/// `path` relative to `root`, when it is inside it.
fn relative_path(root: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = canonical(path)?;
    canonical.strip_prefix(root).ok().map(Path::to_path_buf)
}

/// The canonical form of `path`. Files and directories that do not exist
/// yet (a document created in the editor and never saved) are placed by
/// their nearest existing ancestor.
fn canonical(path: &Path) -> Option<PathBuf> {
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(found) = existing.canonicalize() {
            return Some(missing.iter().rev().fold(found, |p, name| p.join(name)));
        }
        missing.push(existing.file_name()?);
        existing = existing.parent()?;
    }
}

/// Whether two paths name the same file, allowing for symbolic links and
/// other spellings.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b || matches!((canonical(a), canonical(b)), (Some(x), Some(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_positions() {
        let p = |line, character| Position { line, character };
        assert_eq!(end_of(""), p(0, 0));
        assert_eq!(end_of("a = 1\n"), p(1, 0));
        assert_eq!(end_of("a = 1\nb = \"ü😀\""), p(1, 9));
    }

    #[test]
    fn lifecycle() {
        let mut server = Server::new();
        let out = server.handle(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "textDocument/hover", "params": {}}),
        );
        assert_eq!(out[0]["error"]["code"], code::SERVER_NOT_INITIALIZED);
        let out = server
            .handle(&json!({"jsonrpc": "2.0", "id": 2, "method": "initialize", "params": {}}));
        assert_eq!(out[0]["result"]["capabilities"]["hoverProvider"], true);
        let out = server.handle(
            &json!({"jsonrpc": "2.0", "id": 3, "method": "workspace/symbol", "params": {}}),
        );
        assert_eq!(out[0]["error"]["code"], code::METHOD_NOT_FOUND);
        let out = server.handle(
            &json!({"jsonrpc": "2.0", "id": 4, "method": "textDocument/hover", "params": {}}),
        );
        assert_eq!(out[0]["error"]["code"], code::INVALID_PARAMS);
        assert!(
            server
                .handle(
                    &json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": 1}})
                )
                .is_empty()
        );
        let out = server.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown"}));
        assert_eq!(out[0]["result"], Value::Null);
        let out = server.handle(
            &json!({"jsonrpc": "2.0", "id": 6, "method": "textDocument/hover", "params": {}}),
        );
        assert_eq!(out[0]["error"]["code"], code::INVALID_REQUEST);
        assert_eq!(server.exit_code(), None);
        server.handle(&json!({"jsonrpc": "2.0", "method": "exit"}));
        assert_eq!(server.exit_code(), Some(0));
    }

    #[test]
    fn exit_without_shutdown_fails() {
        let mut server = Server::new();
        server.handle(&json!({"jsonrpc": "2.0", "method": "exit"}));
        assert_eq!(server.exit_code(), Some(1));
    }
}

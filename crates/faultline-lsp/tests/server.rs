//! End-to-end tests of the language server against real git repositories,
//! driven one message at a time through `Server::handle`.

use std::path::Path;
use std::process::Command;

use faultline_lsp::Server;
use faultline_policy::Date;
use serde_json::{Value, json};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Faultline Tests",
            "-c",
            "user.email=tests@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git is installed");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A repository with `files` committed.
fn repository(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    for (path, text) in files {
        write(dir.path(), path, text);
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "Initial version"]);
    dir
}

/// The `file:` URI of `path`, spelled as editors spell it on every
/// platform (`file:///C:/...` on Windows).
fn uri_of(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.trim_start_matches("//?/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

fn file_uri(dir: &Path, path: &str) -> String {
    uri_of(&dir.join(path))
}

const CONFIG: &str = r#"[[api]]
name = "pets"
entry = "api/openapi.yaml"
"#;

const V1: &str = "openapi: 3.1.0
info:
  title: Pets
  version: 1.0.0
paths:
  /pets:
    get:
      parameters:
        - name: limit
          in: query
          schema:
            type: integer
            maximum: 100
      responses:
        '200':
          description: ok
";

struct Client {
    server: Server,
    next_id: i64,
}

impl Client {
    fn start(dir: &Path, options: &Value) -> Client {
        let mut client = Client {
            server: Server::new().with_today(Date::parse("2026-09-28").unwrap()),
            next_id: 0,
        };
        let result = client.request(
            "initialize",
            &json!({
                "rootUri": uri_of(dir),
                "capabilities": {},
                "initializationOptions": options,
            }),
        );
        assert_eq!(result["capabilities"]["textDocumentSync"]["change"], 1);
        client.notify("initialized", &json!({}));
        client
    }

    fn request(&mut self, method: &str, params: &Value) -> Value {
        self.next_id += 1;
        let replies = self.server.handle(&json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": method,
            "params": params,
        }));
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0]["id"], self.next_id);
        assert!(replies[0].get("error").is_none(), "{replies:?}");
        replies[0]["result"].clone()
    }

    fn notify(&mut self, method: &str, params: &Value) {
        let replies = self.server.handle(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
        assert!(replies.is_empty(), "{replies:?}");
    }

    fn open(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            &json!({"textDocument": {"uri": uri, "languageId": "yaml", "version": 1, "text": text}}),
        );
    }

    fn change(&mut self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didChange",
            &json!({
                "textDocument": {"uri": uri, "version": 2},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    /// Runs an analysis, returning published diagnostics by URI and the
    /// messages shown.
    fn analyze(&mut self) -> (Vec<(String, Vec<Value>)>, Vec<String>) {
        assert!(self.server.is_dirty());
        let mut published = Vec::new();
        let mut messages = Vec::new();
        for notification in self.server.analyze() {
            match notification["method"].as_str().unwrap() {
                "textDocument/publishDiagnostics" => published.push((
                    notification["params"]["uri"].as_str().unwrap().to_owned(),
                    notification["params"]["diagnostics"]
                        .as_array()
                        .unwrap()
                        .clone(),
                )),
                "window/showMessage" => messages.push(
                    notification["params"]["message"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                ),
                other => panic!("unexpected notification {other}"),
            }
        }
        assert!(!self.server.is_dirty());
        (published, messages)
    }
}

fn diagnostics_for<'a>(published: &'a [(String, Vec<Value>)], uri: &str) -> &'a [Value] {
    published
        .iter()
        .find(|(u, _)| u == uri)
        .map_or(&[], |(_, d)| d.as_slice())
}

/// A workspace whose API document has an unsaved breaking edit, analyzed:
/// the client, the document's URI and the finding's diagnostic.
fn edited() -> (tempfile::TempDir, Client, String, Value) {
    let dir = repository(&[("faultline.toml", CONFIG), ("api/openapi.yaml", V1)]);
    let mut client = Client::start(dir.path(), &json!({}));
    let (published, messages) = client.analyze();
    assert!(messages.is_empty(), "{messages:?}");
    assert!(
        published.iter().all(|(_, d)| d.is_empty()),
        "an unchanged workspace has no findings: {published:?}"
    );

    let api = file_uri(dir.path(), "api/openapi.yaml");
    client.open(&api, V1);
    client.change(&api, &V1.replace("maximum: 100", "maximum: 50"));
    let (published, _) = client.analyze();
    let finding = diagnostics_for(&published, &api)
        .iter()
        .find(|d| d["data"]["findingId"].is_string())
        .unwrap_or_else(|| panic!("no finding in {published:?}"))
        .clone();
    (dir, client, api, finding)
}

#[test]
fn unsaved_edits_are_compared_with_the_base_revision() {
    let (_dir, mut client, api, finding) = edited();
    assert_eq!(
        finding["severity"], 1,
        "a tighter input bound breaks clients"
    );
    assert_eq!(finding["source"], "faultline");
    assert!(finding["code"].as_str().unwrap().starts_with("FL"));
    let line = finding["range"]["start"]["line"].as_u64().unwrap();
    assert!(
        (8..=12).contains(&line),
        "shown at the parameter, not line {line}"
    );

    // Hovering explains the finding.
    let hover = client.request(
        "textDocument/hover",
        &json!({"textDocument": {"uri": api}, "position": finding["range"]["start"]}),
    );
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains(finding["code"].as_str().unwrap()), "{text}");
    assert!(text.contains("Example value"), "{text}");
    let id = finding["data"]["findingId"].as_str().unwrap();
    assert!(text.contains(id), "{text}");
    let nothing = client.request(
        "textDocument/hover",
        &json!({"textDocument": {"uri": api}, "position": {"line": 1, "character": 0}}),
    );
    assert_eq!(nothing, Value::Null);
}

#[test]
fn quick_fixes_add_waivers() {
    let (dir, mut client, api, finding) = edited();
    let id = finding["data"]["findingId"].as_str().unwrap();
    // The quick fix appends a waiver to faultline.toml.
    let actions = client.request(
        "textDocument/codeAction",
        &json!({
            "textDocument": {"uri": api},
            "range": finding["range"],
            "context": {"diagnostics": [finding]},
        }),
    );
    let actions = actions.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["kind"], "quickfix");
    let changes = actions[0]["edit"]["changes"].as_object().unwrap();
    let (config_uri, edits) = changes.iter().next().unwrap();
    assert!(config_uri.ends_with("/faultline.toml"), "{config_uri}");
    let edit = &edits[0];
    assert_eq!(edit["range"]["start"]["line"], 3, "after the last line");
    let inserted = edit["newText"].as_str().unwrap();
    assert!(inserted.starts_with("\n[[waiver]]\n"), "{inserted}");
    assert!(inserted.contains(&format!("finding = \"{id}\"")));
    assert!(inserted.contains("expires = \"2026-10-28\""), "{inserted}");

    // With the waiver in the configuration, the finding is only a hint.
    let config = file_uri(dir.path(), "faultline.toml");
    client.open(&config, &format!("{CONFIG}{inserted}"));
    let (published, _) = client.analyze();
    let waived = diagnostics_for(&published, &api)
        .iter()
        .find(|d| d["data"]["findingId"] == id)
        .unwrap();
    assert_eq!(waived["severity"], 4);
    assert!(
        waived["message"]
            .as_str()
            .unwrap()
            .contains("waived until 2026-10-28"),
        "{waived}"
    );
    let actions = client.request(
        "textDocument/codeAction",
        &json!({"textDocument": {"uri": api}, "range": waived["range"], "context": {"diagnostics": []}}),
    );
    assert_eq!(actions, json!([]), "a waived finding needs no waiver");

    // Closing the edited document goes back to the saved text, and the
    // stale diagnostics are cleared.
    client.notify(
        "textDocument/didClose",
        &json!({"textDocument": {"uri": api}}),
    );
    let (published, _) = client.analyze();
    assert!(diagnostics_for(&published, &api).is_empty());
    assert!(
        published
            .iter()
            .any(|(uri, d)| uri.ends_with("/api/openapi.yaml") && d.is_empty()),
        "{published:?}"
    );
    let config_diagnostics = diagnostics_for(&published, &config);
    assert!(
        config_diagnostics.iter().any(|d| d["message"]
            .as_str()
            .unwrap()
            .contains("matches no finding")),
        "the waiver is now unused: {published:?}"
    );
}

#[test]
fn parse_errors_are_shown_where_they_are() {
    let dir = repository(&[("faultline.toml", CONFIG), ("api/openapi.yaml", V1)]);
    let mut client = Client::start(dir.path(), &json!({}));
    let api = file_uri(dir.path(), "api/openapi.yaml");
    client.open(
        &api,
        &V1.replace("          in: query\n", "          in: [query\n"),
    );
    let (published, _) = client.analyze();
    let diagnostics = diagnostics_for(&published, &api);
    assert!(!diagnostics.is_empty(), "{published:?}");
    assert!(diagnostics.iter().all(|d| d["data"].is_null()));
    assert_eq!(diagnostics[0]["severity"], 1);
    assert!(diagnostics[0]["code"].as_str().unwrap().starts_with("FLE"));
}

#[test]
fn invalid_configuration_is_reported_on_the_file() {
    let dir = repository(&[("faultline.toml", "[[api]]\nname = 1\n")]);
    let mut client = Client::start(dir.path(), &json!({}));
    let (published, _) = client.analyze();
    let config = published
        .iter()
        .find(|(uri, _)| uri.ends_with("/faultline.toml"))
        .unwrap();
    assert_eq!(config.1[0]["severity"], 1);
    assert!(
        config.1[0]["message"]
            .as_str()
            .unwrap()
            .contains("invalid configuration")
    );
}

#[test]
fn without_configuration_open_api_documents_are_compared() {
    let v1 = "syntax = \"proto3\";\npackage pets;\nmessage Pet {\n  string name = 1;\n  int32 age = 2;\n}\n";
    let dir = repository(&[("pets.proto", v1), ("package.json", "{\"name\": \"x\"}")]);
    let mut client = Client::start(dir.path(), &json!({"base": "main"}));
    let proto = file_uri(dir.path(), "pets.proto");
    let package = file_uri(dir.path(), "package.json");
    client.open(&package, "{\"name\": \"y\"}");
    client.open(&proto, &v1.replace("int32 age = 2;", "string age = 2;"));
    let (published, messages) = client.analyze();
    assert!(messages.is_empty(), "{messages:?}");
    assert!(diagnostics_for(&published, &package).is_empty());
    let findings = diagnostics_for(&published, &proto);
    let changed = findings
        .iter()
        .find(|d| d["data"]["findingId"].is_string())
        .unwrap_or_else(|| panic!("{published:?}"));
    assert_eq!(changed["range"]["start"]["line"], 4);
    assert!(
        changed["message"].as_str().unwrap().starts_with("pets: "),
        "{changed}"
    );
}

#[test]
fn an_unknown_base_revision_is_reported_once() {
    let dir = repository(&[("faultline.toml", CONFIG), ("api/openapi.yaml", V1)]);
    let mut client = Client::start(dir.path(), &json!({"base": "no-such-branch"}));
    let (published, messages) = client.analyze();
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].contains("no-such-branch"), "{messages:?}");
    assert!(published.iter().all(|(_, d)| d.is_empty()));
    client.notify("workspace/didChangeWatchedFiles", &json!({"changes": []}));
    let (_, messages) = client.analyze();
    assert!(messages.is_empty(), "shown once: {messages:?}");

    // The editor can switch to another base.
    client.notify(
        "workspace/didChangeConfiguration",
        &json!({"settings": {"faultline": {"base": "main"}}}),
    );
    let (_, messages) = client.analyze();
    assert!(messages.is_empty(), "{messages:?}");
}

#[test]
fn a_new_api_is_not_an_error() {
    let dir = repository(&[("faultline.toml", CONFIG)]);
    let mut client = Client::start(dir.path(), &json!({}));
    let api = file_uri(dir.path(), "api/openapi.yaml");
    client.open(&api, V1);
    let (published, messages) = client.analyze();
    assert!(messages.is_empty(), "{messages:?}");
    assert!(published.iter().all(|(_, d)| d.is_empty()), "{published:?}");
}

#[test]
fn stdio_session() {
    let dir = repository(&[("faultline.toml", CONFIG), ("api/openapi.yaml", V1)]);
    let frame = |value: Value| {
        let body = value.to_string();
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    };
    let root = uri_of(dir.path());
    let input = [
        frame(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"rootUri": root}}),
        ),
        frame(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})),
        "Content-Length: 2\r\n\r\n{x".to_owned(),
        frame(json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"})),
        frame(json!({"jsonrpc": "2.0", "method": "exit"})),
    ]
    .concat();
    let mut output = Vec::new();
    let code = faultline_lsp::run(
        std::io::Cursor::new(input.into_bytes()),
        &mut output,
        Server::new(),
    );
    assert_eq!(code, 0);
    let mut reader = output.as_slice();
    let mut replies = Vec::new();
    while let Some(message) = faultline_lsp::protocol::read_message(&mut reader).unwrap() {
        replies.push(message);
    }
    assert_eq!(replies[0]["id"], 1);
    assert!(replies.iter().any(|r| r["error"]["code"] == -32700));
    assert!(
        replies
            .iter()
            .any(|r| r["id"] == 2 && r["result"].is_null())
    );
}

#[test]
fn configured_overlays_and_patterns_apply_in_the_editor() {
    let config = "[[api]]\nname = \"pets\"\nentry = \"api/openapi.yaml\"\noverlays = [\"api/public.yaml\"]\n\n[[api]]\nname = \"graph\"\nentry = \"graph/*.graphql\"\n";
    let internal = format!(
        "{V1}  /admin:\n    delete:\n      x-internal: true\n      responses:\n        '204':\n          description: gone\n"
    );
    let overlay = "overlay: 1.0.0\ninfo: {title: Public, version: 1.0.0}\nactions:\n  - target: $.paths.*[?@['x-internal'] == true]\n    remove: true\n  - target: $.paths[?length(@) == 0]\n    remove: true\n";
    let dir = repository(&[
        ("faultline.toml", config),
        ("api/openapi.yaml", &internal),
        ("api/public.yaml", overlay),
        ("graph/query.graphql", "type Query { pet: Pet }\n"),
        ("graph/pet.graphql", "type Pet { name: String }\n"),
    ]);
    let mut client = Client::start(dir.path(), &json!({}));
    let (published, messages) = client.analyze();
    assert!(messages.is_empty(), "{messages:?}");
    assert!(published.iter().all(|(_, d)| d.is_empty()), "{published:?}");

    // Removing the internal operation is not a finding: the overlay hides it.
    let api = file_uri(dir.path(), "api/openapi.yaml");
    client.open(&api, &internal);
    client.change(&api, V1);
    // Removing a field from the split schema is, in the file that has it.
    let pet = file_uri(dir.path(), "graph/pet.graphql");
    client.open(&pet, "type Pet { name: String }\n");
    client.change(&pet, "type Pet { id: ID }\n");
    let (published, messages) = client.analyze();
    assert!(messages.is_empty(), "{messages:?}");
    assert!(
        diagnostics_for(&published, &api).is_empty(),
        "{published:?}"
    );
    assert!(
        diagnostics_for(&published, &pet)
            .iter()
            .any(|d| d["severity"] == 1),
        "{published:?}"
    );
}

//! Sending versions to a registry: bundling a workspace's files and
//! talking to the HTTP interface.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use faultline::{Engine, Source};
use faultline_doc::loader::{FsLoader, display_name};
use faultline_policy::{ApiFormat, SchemaRole};
use faultline_source::{SourceMap, SourceName};
use serde_json::Value;

use crate::store::{Bundle, validate_path, validate_subject};

/// The largest response read from a registry.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// Bundles the API whose entry document is `entry` under `root`: the entry
/// and every workspace file it references, as lowering reads them. The
/// description must lower without errors, since a registry would refuse it
/// anyway and the local diagnostics say more.
pub fn bundle(
    engine: &Engine,
    root: &Path,
    entry: &Path,
    format: Option<ApiFormat>,
    role: SchemaRole,
) -> Result<Bundle, String> {
    let sources = SourceMap::new();
    let lowered = engine.lower(&Source::new(root, entry), "bundle", format, role, &sources);
    if lowered.api.is_none() || lowered.diagnostics.has_errors() {
        let rendered: Vec<String> = lowered
            .diagnostics
            .iter()
            .map(|d| faultline_source::render::render(d, &sources))
            .collect();
        return Err(format!(
            "{} has errors:\n{}",
            entry.display(),
            rendered.join("\n")
        ));
    }
    let loader = FsLoader::new(root, engine.limits.clone())
        .map_err(|e| format!("cannot open {}: {e}", root.display()))?;
    let entry_uri = loader
        .uri_for(entry)
        .map_err(|e| format!("cannot read {}: {e}", entry.display()))?;
    let SourceName::Path(entry_path) = display_name(&entry_uri) else {
        return Err(format!("{} is not a workspace file", entry.display()));
    };
    let entry = slash_path(&entry_path);
    let mut files = BTreeMap::new();
    for file in sources.files() {
        let SourceName::Path(path) = file.name() else {
            // Remote documents and built-in definitions are not part of
            // the workspace; a registry cannot resolve remote references.
            continue;
        };
        let path = slash_path(path);
        validate_path(&path)?;
        files.insert(path, file.text().to_owned());
    }
    if !files.contains_key(&entry) {
        return Err(format!("{entry} was not read while lowering it"));
    }
    Ok(Bundle {
        entry,
        format,
        role,
        files,
    })
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// A registry's HTTP interface.
#[derive(Debug)]
pub struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

/// A registry's answer: the status and the JSON body.
#[derive(Clone, Debug, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: Value,
}

impl Client {
    /// A client for the registry at `url` (`http` or `https`). A token is
    /// only sent over plain HTTP to the local machine.
    pub fn new(url: &str, token: Option<String>) -> Result<Client, String> {
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            return Err(format!(
                "the registry URL `{url}` must start with http:// or https://"
            ));
        }
        let token = token.filter(|t| !t.is_empty());
        if token.is_some()
            && lower.starts_with("http://")
            && !is_loopback(&lower["http://".len()..])
        {
            return Err(format!(
                "refusing to send the registry token to {url} over unencrypted HTTP; use https://"
            ));
        }
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .max_redirects(0)
            .user_agent(concat!("faultline/", env!("CARGO_PKG_VERSION")))
            .build();
        Ok(Client {
            base: url.trim_end_matches('/').to_owned(),
            token,
            agent: ureq::Agent::new_with_config(config),
        })
    }

    /// Registers `bundle` as the next version of `subject`.
    pub fn register(&self, subject: &str, bundle: &Bundle) -> Result<Reply, String> {
        self.post(subject, "versions", bundle)
    }

    /// Checks `bundle` against `subject` without registering it.
    pub fn check(&self, subject: &str, bundle: &Bundle) -> Result<Reply, String> {
        self.post(subject, "check", bundle)
    }

    fn post(&self, subject: &str, action: &str, bundle: &Bundle) -> Result<Reply, String> {
        validate_subject(subject)?;
        let url = format!("{}/subjects/{subject}/{action}", self.base);
        let mut request = self
            .agent
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json");
        if let Some(token) = &self.token {
            request = request.header("Authorization", &format!("Bearer {token}"));
        }
        let body = bundle.to_json().to_string();
        let mut response = request
            .send(body.as_str())
            .map_err(|e| format!("cannot reach the registry at {}: {e}", self.base))?;
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(|e| format!("cannot read the registry's response: {e}"))?;
        let body = serde_json::from_str(&text).map_err(|_| {
            format!(
                "the registry answered HTTP {status} with a body that is not JSON: {}",
                text.chars().take(200).collect::<String>()
            )
        })?;
        Ok(Reply { status, body })
    }
}

/// Whether the authority at the start of `rest` (`host[:port]/...`) names
/// the local machine.
fn is_loopback(rest: &str) -> bool {
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        Some(bracketed) => bracketed.split(']').next().unwrap_or_default(),
        None => authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host),
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_collect_referenced_files() {
        let dir = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "api/openapi.yaml",
            "openapi: 3.1.0\ninfo: {title: t, version: 1.0.0}\npaths:\n  /pets:\n    get:\n      \
             responses:\n        '200':\n          description: ok\n          content:\n            \
             application/json:\n              schema: {$ref: '../schemas/pet.json'}\n",
        );
        write("schemas/pet.json", r#"{"type": "object"}"#);
        write("schemas/unused.json", "{}");
        let bundle = bundle(
            &Engine::default(),
            dir.path(),
            Path::new("api/openapi.yaml"),
            None,
            SchemaRole::Both,
        )
        .unwrap();
        assert_eq!(bundle.entry, "api/openapi.yaml");
        assert_eq!(
            bundle.files.keys().collect::<Vec<_>>(),
            ["api/openapi.yaml", "schemas/pet.json"]
        );

        write("api/broken.yaml", "openapi: [");
        let error = super::bundle(
            &Engine::default(),
            dir.path(),
            Path::new("api/broken.yaml"),
            None,
            SchemaRole::Both,
        )
        .unwrap_err();
        assert!(error.contains("has errors"), "{error}");
    }

    #[test]
    fn urls_must_be_http() {
        assert!(Client::new("ftp://registry.example.com", None).is_err());
        assert!(Client::new("registry.example.com", None).is_err());
        let token = || Some("placeholder-token".to_owned());
        assert!(Client::new("http://registry.example.com", token()).is_err());
        assert!(Client::new("http://registry.example.com", None).is_ok());
        for local in [
            "http://localhost:8080",
            "http://127.0.0.1:9/x",
            "http://127.1.2.3",
            "http://[::1]:8080/",
        ] {
            assert!(Client::new(local, token()).is_ok(), "{local}");
        }
        assert!(Client::new("http://localhost.example.com", token()).is_err());
        assert!(Client::new("https://registry.example.com", token()).is_ok());
        let client = Client::new("https://registry.example.com/", None).unwrap();
        assert_eq!(client.base, "https://registry.example.com");
    }
}

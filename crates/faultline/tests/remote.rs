//! Remote references through the engine.

use std::sync::Arc;
use std::time::Duration;

use faultline::{Engine, Source, Target, network_policy};
use faultline_doc::loader::MemoryLoader;
use faultline_policy::NetworkConfig;

const PET: &str = r#"{"type": "object", "properties": {"name": {"type": "string"}}}"#;

/// Two versions of a schema that reference a shared remote definition; the
/// new one also tightens a local property.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let schema = |max: u32| {
        format!(
            r#"{{
  "type": "object",
  "properties": {{
    "pet": {{"$ref": "https://schemas.example.com/pet.json"}},
    "count": {{"type": "integer", "maximum": {max}}}
  }}
}}"#
        )
    };
    for (version, max) in [("old", 10), ("new", 5)] {
        std::fs::create_dir(dir.path().join(version)).unwrap();
        std::fs::write(dir.path().join(version).join("api.json"), schema(max)).unwrap();
    }
    dir
}

fn target(dir: &tempfile::TempDir) -> Target {
    Target::new(
        "api",
        Source::new(dir.path(), "old/api.json"),
        Source::new(dir.path(), "new/api.json"),
    )
}

#[test]
fn remote_references_are_disabled_by_default() {
    let dir = workspace();
    let comparison = Engine::default().compare(&[target(&dir)]);
    assert!(comparison.diagnostics.has_errors());
    let codes: Vec<String> = comparison
        .diagnostics
        .iter()
        .filter_map(|d| d.code.map(|c| c.to_string()))
        .collect();
    assert!(codes.iter().any(|c| c == "FLE1005"), "{codes:?}");
    assert!(comparison.findings.is_empty());
}

#[test]
fn remote_loaders_serve_references_for_both_versions() {
    let dir = workspace();
    let remote = MemoryLoader::new().with("https://schemas.example.com/pet.json", PET);
    let engine = Engine::default().with_remote(Arc::new(remote));
    let comparison = engine.compare(&[target(&dir)]);
    assert!(
        !comparison.diagnostics.has_errors(),
        "{:?}",
        comparison.diagnostics
    );
    assert_eq!(comparison.findings.len(), 1, "{:?}", comparison.findings);
    assert_eq!(comparison.findings[0].rule.code, "FL1007");
    // The remote document is registered under its URL.
    assert!(
        comparison
            .sources
            .files()
            .iter()
            .any(|f| f.name().to_string() == "https://schemas.example.com/pet.json")
    );
}

#[test]
fn workspace_files_are_not_served_by_the_remote_loader() {
    let dir = workspace();
    // A remote loader that would answer for file URIs must never be asked.
    let remote = MemoryLoader::new()
        .with("https://schemas.example.com/pet.json", PET)
        .with("file:///old/api.json", "{\"not\": {}}");
    let engine = Engine::default().with_remote(Arc::new(remote));
    let comparison = engine.compare(&[target(&dir)]);
    assert_eq!(comparison.findings.len(), 1);
}

#[test]
fn network_configuration_becomes_a_policy() {
    assert_eq!(network_policy(&NetworkConfig::default()), Ok(None));
    let policy = network_policy(&NetworkConfig {
        allow_remote: true,
        allowed_hosts: vec!["*.example.com".into()],
        timeout_seconds: Some(3),
        ..NetworkConfig::default()
    })
    .unwrap()
    .unwrap();
    assert_eq!(policy.timeout, Duration::from_secs(3));
    assert_eq!(policy.max_redirects, 5);
    assert_eq!(policy.allowed_hosts[0].to_string(), "*.example.com");

    let err = network_policy(&NetworkConfig {
        allow_remote: true,
        allowed_hosts: vec!["https://example.com".into()],
        ..NetworkConfig::default()
    })
    .unwrap_err();
    assert!(err.contains("allowed_hosts"), "{err}");
}

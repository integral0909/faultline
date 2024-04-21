//! `RemoteLoader` policy enforcement against a scripted transport.

use std::collections::HashMap;
use std::sync::Mutex;

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loader};
use faultline_net::{FetchError, Fetched, HostPattern, NetworkPolicy, RemoteLoader, Transport};
use faultline_source::{Limits, SourceName};

/// Answers from a fixed table and records every request.
#[derive(Default)]
struct Script {
    answers: HashMap<String, Result<Fetched, FetchError>>,
    requests: Mutex<Vec<String>>,
}

impl Script {
    fn answer(mut self, url: &str, answer: Result<Fetched, FetchError>) -> Self {
        self.answers.insert(url.to_owned(), answer);
        self
    }

    fn body(self, url: &str, text: &str) -> Self {
        self.answer(url, Ok(Fetched::Body(text.as_bytes().to_vec())))
    }

    fn redirect(self, url: &str, location: &str) -> Self {
        self.answer(url, Ok(Fetched::Redirect(location.to_owned())))
    }
}

impl Transport for Script {
    fn get(&self, url: &str, max_bytes: u64) -> Result<Fetched, FetchError> {
        self.requests.lock().unwrap().push(url.to_owned());
        match self.answers.get(url).cloned() {
            Some(Ok(Fetched::Body(bytes))) if bytes.len() as u64 > max_bytes => {
                Err(FetchError::TooLarge { limit: max_bytes })
            }
            Some(answer) => answer,
            None => Ok(Fetched::Status(404)),
        }
    }
}

fn policy(hosts: &[&str]) -> NetworkPolicy {
    NetworkPolicy {
        allowed_hosts: hosts
            .iter()
            .map(|h| HostPattern::parse(h).unwrap())
            .collect(),
        ..NetworkPolicy::default()
    }
}

fn loader(script: Script) -> RemoteLoader<Script> {
    RemoteLoader::new(
        policy(&["schemas.example.com", "*.cdn.example.net"]),
        Limits::default(),
        script,
    )
}

fn load(loader: &RemoteLoader<Script>, url: &str) -> Result<String, LoadError> {
    loader
        .load(&UriRef::parse(url).unwrap())
        .map(|loaded| loaded.text)
}

fn requests(loader: &RemoteLoader<Script>) -> Vec<String> {
    loader.transport().requests.lock().unwrap().clone()
}

#[test]
fn fetches_allowlisted_documents_once() {
    let loader = loader(Script::default().body("https://schemas.example.com/pet.json", "{}"));
    let loaded = loader
        .load(&UriRef::parse("https://schemas.example.com/pet.json#/defs/a").unwrap())
        .unwrap();
    assert_eq!(loaded.text, "{}");
    assert_eq!(
        loaded.name,
        SourceName::Url("https://schemas.example.com/pet.json".into())
    );
    // Equivalent spellings share the cache entry.
    assert_eq!(
        load(&loader, "HTTPS://Schemas.Example.com/./pet.json").unwrap(),
        "{}"
    );
    assert_eq!(requests(&loader).len(), 1);
}

#[test]
fn refused_urls_never_reach_the_transport() {
    let loader = loader(Script::default());
    for url in [
        "https://evil.example.org/x.json",
        "http://schemas.example.com/x.json",
        "https://user@schemas.example.com/x.json",
        "https://10.0.0.1/x.json",
    ] {
        assert!(
            matches!(load(&loader, url), Err(LoadError::Refused(_))),
            "{url}"
        );
    }
    assert!(requests(&loader).is_empty());
}

#[test]
fn redirects_are_checked_hop_by_hop() {
    let loader = loader(
        Script::default()
            .redirect("https://schemas.example.com/v1/pet.json", "/v2/pet.json")
            .redirect(
                "https://schemas.example.com/v2/pet.json",
                "https://eu.cdn.example.net/pet.json",
            )
            .body("https://eu.cdn.example.net/pet.json", "{\"ok\": true}")
            .redirect(
                "https://schemas.example.com/internal.json",
                "https://169.254.169.254/latest/meta-data",
            )
            .redirect(
                "https://schemas.example.com/downgrade.json",
                "http://schemas.example.com/downgrade.json",
            ),
    );
    let loaded = loader
        .load(&UriRef::parse("https://schemas.example.com/v1/pet.json").unwrap())
        .unwrap();
    assert_eq!(loaded.text, "{\"ok\": true}");
    // The document keeps the name it was requested by: relative references
    // inside it resolve against that URI.
    assert_eq!(
        loaded.name.to_string(),
        "https://schemas.example.com/v1/pet.json"
    );

    let err = load(&loader, "https://schemas.example.com/internal.json").unwrap_err();
    assert!(
        matches!(err, LoadError::Refused(ref r) if r.contains("non-public")),
        "{err}"
    );
    let err = load(&loader, "https://schemas.example.com/downgrade.json").unwrap_err();
    assert!(
        matches!(err, LoadError::Refused(ref r) if r.contains("plain HTTP")),
        "{err}"
    );
    assert!(
        !requests(&loader).iter().any(|r| r.contains("169.254")),
        "the metadata address was contacted"
    );
}

#[test]
fn redirect_loops_are_bounded() {
    let loader = loader(
        Script::default()
            .redirect("https://schemas.example.com/a", "/b")
            .redirect("https://schemas.example.com/b", "/a"),
    );
    let err = load(&loader, "https://schemas.example.com/a").unwrap_err();
    assert!(
        err.to_string().contains("redirected more than 5 times"),
        "{err}"
    );
    assert_eq!(requests(&loader).len(), 6);
}

#[test]
fn statuses_and_failures_are_reported() {
    let loader = loader(
        Script::default()
            .answer("https://schemas.example.com/gone", Ok(Fetched::Status(410)))
            .answer("https://schemas.example.com/down", Ok(Fetched::Status(503)))
            .answer(
                "https://schemas.example.com/tls",
                Err(FetchError::Failed("certificate expired".into())),
            )
            .answer(
                "https://schemas.example.com/bad-location",
                Ok(Fetched::Redirect("/next\npage".into())),
            ),
    );
    assert_eq!(
        load(&loader, "https://schemas.example.com/missing"),
        Err(LoadError::NotFound)
    );
    assert_eq!(
        load(&loader, "https://schemas.example.com/gone"),
        Err(LoadError::NotFound)
    );
    let down = load(&loader, "https://schemas.example.com/down").unwrap_err();
    assert_eq!(
        down.to_string(),
        "fetching https://schemas.example.com/down failed with HTTP status 503"
    );
    let tls = load(&loader, "https://schemas.example.com/tls").unwrap_err();
    assert!(tls.to_string().contains("certificate expired"));
    let bad = load(&loader, "https://schemas.example.com/bad-location").unwrap_err();
    assert!(bad.to_string().contains("invalid location"), "{bad}");

    // Failures are cached like successes: one request each.
    let before = requests(&loader).len();
    let _ = load(&loader, "https://schemas.example.com/down");
    assert_eq!(requests(&loader).len(), before);
}

#[test]
fn size_limits_apply_to_bodies() {
    let big = "x".repeat(64);
    let loader = RemoteLoader::new(
        NetworkPolicy {
            max_bytes: 16,
            ..policy(&["schemas.example.com"])
        },
        Limits::default(),
        Script::default().body("https://schemas.example.com/big.json", &big),
    );
    let err = load(&loader, "https://schemas.example.com/big.json").unwrap_err();
    assert!(err.to_string().contains("16-byte limit"), "{err}");

    // The file-size limit applies too, whichever is smaller.
    let loader = RemoteLoader::new(
        policy(&["schemas.example.com"]),
        Limits {
            max_file_bytes: 8,
            ..Limits::default()
        },
        Script::default().body("https://schemas.example.com/big.json", &big),
    );
    assert!(load(&loader, "https://schemas.example.com/big.json").is_err());
}

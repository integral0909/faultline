//! End-to-end tests of `faultline registry`: a real server process and
//! real clients.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A running `faultline registry serve`, stopped when dropped.
struct Serving {
    child: Child,
    url: String,
}

impl Drop for Serving {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const TOKEN: &str = "placeholder-token-for-tests";

fn serve(dir: &Path) -> Serving {
    let mut child = Command::new(env!("CARGO_BIN_EXE_faultline"))
        .args(["registry", "serve", "--listen", "127.0.0.1:0", "--dir"])
        .arg(dir)
        .env("FAULTLINE_REGISTRY_TOKEN", TOKEN)
        .env("FAULTLINE_TODAY", "2026-09-28")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let url = line
        .trim()
        .rsplit(' ')
        .next()
        .filter(|u| u.starts_with("http://"))
        .unwrap_or_else(|| panic!("unexpected output: {line}"))
        .to_owned();
    Serving { child, url }
}

fn faultline(dir: &Path, args: &[&str], token: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_faultline"));
    command.args(args).current_dir(dir);
    match token {
        Some(token) => command.env("FAULTLINE_REGISTRY_TOKEN", token),
        None => command.env_remove("FAULTLINE_REGISTRY_TOKEN"),
    };
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// A workspace with the pet store's first version at `api/openapi.yaml`.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("api")).unwrap();
    use_version(dir.path(), "v1");
    dir
}

fn use_version(dir: &Path, version: &str) {
    std::fs::copy(
        repo().join(format!("examples/petstore/{version}/openapi.yaml")),
        dir.join("api/openapi.yaml"),
    )
    .unwrap();
}

fn get(url: &str, path: &str) -> String {
    let address = url.trim_start_matches("http://");
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[test]
fn versions_are_registered_and_breaking_ones_refused() {
    let storage = tempfile::tempdir().unwrap();
    let server = serve(storage.path());
    let work = workspace();
    let push = |token| {
        faultline(
            work.path(),
            &[
                "registry",
                "push",
                &server.url,
                "petstore",
                "api/openapi.yaml",
            ],
            token,
        )
    };

    let anonymous = push(None);
    assert_eq!(anonymous.status.code(), Some(2), "{}", stdout(&anonymous));
    assert!(stdout(&anonymous).contains("HTTP 401"));

    let first = push(Some(TOKEN));
    assert_eq!(first.status.code(), Some(0), "{}", stdout(&first));
    assert_eq!(stdout(&first), "registered petstore version 1\n");
    let again = push(Some(TOKEN));
    assert_eq!(again.status.code(), Some(0));
    assert!(stdout(&again).contains("identical"));

    // Version 2 of the pet store breaks clients of version 1.
    use_version(work.path(), "v2");
    let check = faultline(
        work.path(),
        &[
            "registry",
            "check",
            &server.url,
            "petstore",
            "api/openapi.yaml",
        ],
        None,
    );
    assert_eq!(check.status.code(), Some(1), "{}", stdout(&check));
    assert!(stdout(&check).starts_with("incompatible with petstore version 1\n"));
    assert!(stdout(&check).contains("* breaking FL"));
    let refused = push(Some(TOKEN));
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        stdout(&refused).contains("HTTP 409"),
        "{}",
        stdout(&refused)
    );

    let json = faultline(
        work.path(),
        &[
            "registry",
            "check",
            &server.url,
            "petstore",
            "api/openapi.yaml",
            "--json",
        ],
        None,
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["compatible"], false);
    assert!(body["report"]["summary"]["breaking"].as_u64().unwrap() > 0);

    let versions = get(&server.url, "/subjects/petstore/versions");
    assert!(versions.starts_with("HTTP/1.1 200 OK"), "{versions}");
    assert!(
        versions.contains("\"versions\": [\n    1\n  ]"),
        "{versions}"
    );
    let file = get(
        &server.url,
        "/subjects/petstore/versions/latest/files/api/openapi.yaml",
    );
    assert!(file.starts_with("HTTP/1.1 200 OK"), "{file}");
    assert!(file.contains("Content-Type: application/yaml"));
    assert!(file.contains("openapi:"));
}

#[test]
fn unreachable_registries_are_input_errors() {
    let work = workspace();
    let output = faultline(
        work.path(),
        &[
            "registry",
            "push",
            "http://127.0.0.1:9",
            "petstore",
            "api/openapi.yaml",
        ],
        Some(TOKEN),
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot reach the registry"),
        "{output:?}"
    );
    let invalid = faultline(
        work.path(),
        &[
            "registry",
            "push",
            "http://127.0.0.1:9",
            "../pets",
            "api/openapi.yaml",
        ],
        Some(TOKEN),
    );
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid subject"));
}

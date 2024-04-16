//! Reading workspaces from revisions of real repositories, built with the
//! `git` command at test time.

use std::path::Path;
use std::process::Command;

use faultline_doc::UriRef;
use faultline_doc::loader::{LoadError, Loader};
use faultline_source::{Limits, SourceName};
use faultline_vcs::{Repository, VcsError};

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn write(dir: &Path, path: &str, text: &str) {
    let path = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit(dir: &Path, message: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
    git(dir, &["rev-parse", "HEAD"])
}

fn uri(s: &str) -> UriRef {
    UriRef::parse(s).unwrap()
}

/// A repository whose first commit has `api/openapi.yaml` at version 1 and
/// whose working tree has version 2.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    write(dir.path(), "api/openapi.yaml", "version: 1\n");
    write(dir.path(), "api/common.yaml", "shared: true\n");
    commit(dir.path(), "first");
    write(dir.path(), "api/openapi.yaml", "version: 2\n");
    dir
}

#[test]
fn loads_files_as_they_were_in_a_revision() {
    let dir = fixture();
    let repo = Repository::discover(dir.path()).unwrap();
    let head = repo.resolve("HEAD").unwrap();
    assert_eq!(head.label(), "HEAD");
    let loader = repo.loader(&head, Vec::new(), Limits::default());
    let loaded = loader.load(&uri("file:///api/openapi.yaml")).unwrap();
    assert_eq!(loaded.text, "version: 1\n");
    assert_eq!(
        loaded.name,
        SourceName::Revision {
            rev: "HEAD".into(),
            path: "api/openapi.yaml".into()
        }
    );
    assert_eq!(
        loader.load(&uri("file:///api/missing.yaml")),
        Err(LoadError::NotFound)
    );
    // Directories are not documents.
    assert_eq!(loader.load(&uri("file:///api")), Err(LoadError::NotFound));
}

#[test]
fn discovery_works_from_subdirectories() {
    let dir = fixture();
    let repo = Repository::discover(&dir.path().join("api")).unwrap();
    assert_eq!(repo.workdir(), dir.path().canonicalize().unwrap());
}

#[test]
fn workspace_roots_below_the_repository_root_are_prefixes() {
    let dir = fixture();
    let repo = Repository::discover(dir.path()).unwrap();
    let root = repo.repo_path(&dir.path().join("api")).unwrap();
    assert_eq!(root, ["api"]);
    let head = repo.resolve("HEAD").unwrap();
    let loader = repo.loader(&head, root, Limits::default());
    let loaded = loader.load(&uri("file:///common.yaml")).unwrap();
    assert_eq!(loaded.text, "shared: true\n");
    // Names are relative to the workspace root, like the filesystem loader's.
    assert_eq!(loaded.name.to_string(), "HEAD:common.yaml");
    // The workspace cannot reach its parent through the URI.
    assert_eq!(
        loader.load(&uri("file:///../api/common.yaml")),
        Err(LoadError::OutsideRoot)
    );
}

#[test]
fn paths_absent_from_the_checkout_still_map_into_the_repository() {
    let dir = fixture();
    let repo = Repository::discover(dir.path()).unwrap();
    assert_eq!(
        repo.repo_path(&dir.path().join("renamed/old.yaml"))
            .unwrap(),
        ["renamed", "old.yaml"]
    );
    let outside = tempfile::tempdir().unwrap();
    assert!(matches!(
        repo.repo_path(outside.path()),
        Err(VcsError::OutsideRepository { .. })
    ));
}

#[test]
fn unknown_revisions_and_non_repositories_are_reported() {
    let dir = fixture();
    let repo = Repository::discover(dir.path()).unwrap();
    let err = repo.resolve("no-such-branch").unwrap_err();
    assert!(matches!(err, VcsError::UnknownRevision { ref spec, .. } if spec == "no-such-branch"));
    assert!(err.to_string().contains("no-such-branch"));

    let plain = tempfile::tempdir().unwrap();
    assert!(matches!(
        Repository::discover(plain.path()),
        Err(VcsError::NotARepository { .. })
    ));
}

#[test]
fn merge_bases_are_found_across_branches() {
    let dir = fixture();
    let base = git(dir.path(), &["rev-parse", "HEAD"]);
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    commit(dir.path(), "feature work");
    git(dir.path(), &["checkout", "-q", "main"]);
    write(dir.path(), "api/openapi.yaml", "version: 3\n");
    let main_tip = commit(dir.path(), "main moves on");

    let repo = Repository::discover(dir.path()).unwrap();
    let main = repo.resolve("main").unwrap();
    let feature = repo.resolve("feature").unwrap();
    assert_eq!(main.commit(), main_tip);
    let merge_base = repo.merge_base(&main, &feature).unwrap();
    assert_eq!(merge_base.commit(), base);
    assert_eq!(merge_base.label(), &base[..12]);

    // The merge base still has version 1, although main has moved on.
    let loader = repo.loader(&merge_base, Vec::new(), Limits::default());
    assert_eq!(
        loader.load(&uri("file:///api/openapi.yaml")).unwrap().text,
        "version: 1\n"
    );
}

#[test]
fn unrelated_histories_have_no_merge_base() {
    let dir = fixture();
    git(dir.path(), &["checkout", "-q", "--orphan", "island"]);
    git(dir.path(), &["rm", "-rqf", "--cached", "."]);
    commit(dir.path(), "unrelated");
    let repo = Repository::discover(dir.path()).unwrap();
    let main = repo.resolve("main").unwrap();
    let island = repo.resolve("island").unwrap();
    assert!(matches!(
        repo.merge_base(&main, &island),
        Err(VcsError::NoMergeBase { .. })
    ));
}

#[test]
fn tags_and_relative_revisions_resolve_to_commits() {
    let dir = fixture();
    git(dir.path(), &["tag", "-a", "v1", "-m", "release"]);
    commit(dir.path(), "second");
    let repo = Repository::discover(dir.path()).unwrap();
    let tagged = repo.resolve("v1").unwrap();
    let parent = repo.resolve("HEAD~1").unwrap();
    assert_eq!(tagged.commit(), parent.commit());
}

#[test]
fn blobs_over_the_size_limit_are_refused_before_reading() {
    let dir = fixture();
    let repo = Repository::discover(dir.path()).unwrap();
    let head = repo.resolve("HEAD").unwrap();
    let limits = Limits {
        max_file_bytes: 4,
        ..Limits::default()
    };
    let loader = repo.loader(&head, Vec::new(), limits);
    assert!(matches!(
        loader.load(&uri("file:///api/openapi.yaml")),
        Err(LoadError::TooLarge(_))
    ));
}

#[test]
fn non_file_uris_are_refused() {
    let dir = fixture();
    let repo = Repository::discover(dir.path()).unwrap();
    let head = repo.resolve("HEAD").unwrap();
    let loader = repo.loader(&head, Vec::new(), Limits::default());
    assert!(matches!(
        loader.load(&uri("https://example.com/api/openapi.yaml")),
        Err(LoadError::RemoteDisabled { .. })
    ));
    assert!(matches!(
        loader.load(&uri("file://host/api/openapi.yaml")),
        Err(LoadError::UnsupportedScheme { .. })
    ));
}

#[cfg(unix)]
#[test]
fn symbolic_links_in_revisions_are_not_followed() {
    let dir = fixture();
    std::os::unix::fs::symlink("/etc/hosts", dir.path().join("api/escape.yaml")).unwrap();
    std::os::unix::fs::symlink("openapi.yaml", dir.path().join("api/alias.yaml")).unwrap();
    std::os::unix::fs::symlink(".", dir.path().join("api/loop")).unwrap();
    commit(dir.path(), "links");
    let repo = Repository::discover(dir.path()).unwrap();
    let head = repo.resolve("HEAD").unwrap();
    let loader = repo.loader(&head, Vec::new(), Limits::default());
    for path in ["api/escape.yaml", "api/alias.yaml"] {
        let err = loader.load(&uri(&format!("file:///{path}"))).unwrap_err();
        assert!(err.to_string().contains("symbolic link"), "{path}: {err}");
    }
    assert_eq!(
        loader.load(&uri("file:///api/loop/openapi.yaml")),
        Err(LoadError::NotFound)
    );
}

#[test]
fn undecodable_blobs_are_reported() {
    let dir = fixture();
    std::fs::write(dir.path().join("api/binary.yaml"), [0xff, 0xfe, 0x00, 0xd8]).unwrap();
    commit(dir.path(), "binary");
    let repo = Repository::discover(dir.path()).unwrap();
    let head = repo.resolve("HEAD").unwrap();
    let loader = repo.loader(&head, Vec::new(), Limits::default());
    assert!(matches!(
        loader.load(&uri("file:///api/binary.yaml")),
        Err(LoadError::Decode(_))
    ));
}

//! Filesystem loader sandboxing, exercised against a real temporary directory.

use std::fs;
use std::path::Path;

use faultline_doc::UriRef;
use faultline_doc::loader::{FsLoader, LoadError, Loader, workspace_uri};
use faultline_source::{LimitKind, Limits, SourceName};

fn workspace() -> (tempfile::TempDir, FsLoader) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    fs::create_dir_all(root.join("api/common")).unwrap();
    fs::write(root.join("api/openapi.yaml"), "openapi: 3.1.0\n").unwrap();
    fs::write(root.join("api/common/pet.json"), "{}").unwrap();
    fs::write(dir.path().join("secret.txt"), "top secret").unwrap();
    let loader = FsLoader::new(&root, Limits::default()).unwrap();
    (dir, loader)
}

fn u(s: &str) -> UriRef {
    UriRef::parse(s).unwrap()
}

#[test]
fn loads_files_inside_the_root_with_relative_names() {
    let (_dir, loader) = workspace();
    let base = loader.uri_for(Path::new("api/openapi.yaml")).unwrap();
    assert_eq!(base.to_string(), "file:///api/openapi.yaml");
    let target = base.resolve_strict(&u("common/pet.json#/x")).unwrap();
    let loaded = loader.load(&target).unwrap();
    assert_eq!(loaded.text, "{}");
    assert_eq!(loaded.name, SourceName::Path("api/common/pet.json".into()));
}

#[test]
fn refuses_paths_outside_the_root() {
    let (_dir, loader) = workspace();
    // Strict resolution already rejects the escape...
    let base = u("file:///api/openapi.yaml");
    assert!(base.resolve_strict(&u("../../secret.txt")).is_none());
    // ...and the loader refuses leftover parent components or foreign paths.
    assert_eq!(
        loader.load(&u("file:///../secret.txt")),
        Err(LoadError::OutsideRoot)
    );
    assert_eq!(
        loader.uri_for(&loader.root().join("../secret.txt")),
        Err(LoadError::OutsideRoot)
    );
}

#[cfg(unix)]
#[test]
fn refuses_symlinks_that_leave_the_root() {
    let (dir, loader) = workspace();
    let link = loader.root().join("api/leak.txt");
    std::os::unix::fs::symlink(dir.path().join("secret.txt"), &link).unwrap();
    assert_eq!(
        loader.load(&u("file:///api/leak.txt")),
        Err(LoadError::OutsideRoot)
    );

    // Links that stay inside the workspace are fine.
    let inner = loader.root().join("api/pet-link.json");
    std::os::unix::fs::symlink(loader.root().join("api/common/pet.json"), &inner).unwrap();
    assert_eq!(
        loader.load(&u("file:///api/pet-link.json")).unwrap().text,
        "{}"
    );
}

#[test]
fn remote_and_unknown_schemes_are_refused() {
    let (_dir, loader) = workspace();
    assert_eq!(
        loader.load(&u("HTTPS://example.com/schema.json")),
        Err(LoadError::RemoteDisabled {
            scheme: "https".into()
        })
    );
    assert_eq!(
        loader.load(&u("urn:example:schema")),
        Err(LoadError::UnsupportedScheme {
            scheme: Some("urn".into())
        })
    );
    assert_eq!(
        loader.load(&u("file://server/share/x.json")),
        Err(LoadError::UnsupportedScheme {
            scheme: Some("file".into())
        })
    );
}

#[test]
fn missing_files_directories_and_size_limits() {
    let (_dir, loader) = workspace();
    assert_eq!(
        loader.load(&u("file:///api/nope.yaml")),
        Err(LoadError::NotFound)
    );
    assert_eq!(
        loader.load(&u("file:///api/common")),
        Err(LoadError::NotFound)
    );

    let small = FsLoader::new(
        loader.root(),
        Limits {
            max_file_bytes: 4,
            ..Limits::default()
        },
    )
    .unwrap();
    match small.load(&u("file:///api/openapi.yaml")) {
        Err(LoadError::TooLarge(e)) => assert_eq!(e.kind, LimitKind::FileBytes),
        other => panic!("{other:?}"),
    }
}

#[test]
fn encoded_names_round_trip_through_the_filesystem() {
    let (_dir, loader) = workspace();
    fs::write(loader.root().join("api/pet store#1.yaml"), "a: 1\n").unwrap();
    let uri = workspace_uri(Path::new("api/pet store#1.yaml"));
    assert_eq!(loader.load(&uri).unwrap().text, "a: 1\n");
}

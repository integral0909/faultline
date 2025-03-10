//! End-to-end tests of comparisons against git revisions: `faultline check`
//! and `diff --old-rev`/`--new-rev`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn example(version: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/petstore")
        .join(version)
        .join("openapi.yaml");
    std::fs::read_to_string(path).unwrap()
}

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
    let path: PathBuf = dir.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

const CONFIG: &str = r#"
[[api]]
name = "petstore"
entry = "api/openapi.yaml"
"#;

/// A repository whose `main` has version 1 of the pet store, checked out on
/// a `feature` branch whose working tree has version 2.
fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    write(dir.path(), "faultline.toml", CONFIG);
    write(dir.path(), "api/openapi.yaml", &example("v1"));
    commit(dir.path(), "pet store v1");
    git(dir.path(), &["checkout", "-q", "-b", "feature"]);
    write(dir.path(), "api/openapi.yaml", &example("v2"));
    dir
}

fn faultline(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faultline"))
        .args(args)
        .current_dir(dir)
        .env("FAULTLINE_TODAY", "2026-09-28")
        // Pull and merge request builds set these; the tests choose for
        // themselves.
        .env_remove("GITHUB_BASE_REF")
        .env_remove("CI_MERGE_REQUEST_DIFF_BASE_SHA")
        .env_remove("CI_MERGE_REQUEST_TARGET_BRANCH_NAME")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn check_compares_the_working_tree_with_the_merge_base() {
    let dir = repository();
    let output = faultline(dir.path(), &["check", "--base", "main"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    for fragment in [
        "breaking[FL2001]: op[DELETE /pets/{}]: operation removed",
        "--> api/openapi.yaml:",
        "petstore 1.0.0 -> 2.0.0: 5 breaking, 2 risky, 2 compatible",
    ] {
        assert!(text.contains(fragment), "missing {fragment:?} in:\n{text}");
    }
    // The removed operation exists only in the merge base, which is named
    // by its abbreviated commit id.
    let removed = text
        .lines()
        .skip_while(|l| !l.contains("FL2001"))
        .find(|l| l.contains("-->"))
        .unwrap();
    assert!(
        removed.contains(":api/openapi.yaml:"),
        "expected a revision-qualified location, got {removed}"
    );
}

#[test]
fn committed_changes_are_compared_too() {
    let dir = repository();
    commit(dir.path(), "pet store v2");
    let output = faultline(dir.path(), &["check", "--base", "main", "--output", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["summary"]["breaking"], 5);
    let removed = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "FL2001")
        .unwrap();
    assert_eq!(removed["new_location"], serde_json::Value::Null);
    assert_eq!(removed["old_location"]["file"], "api/openapi.yaml");
    assert_eq!(
        removed["old_location"]["revision"].as_str().unwrap().len(),
        12
    );
    let added = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "FL2010")
        .unwrap();
    assert_eq!(
        added["new_location"]["revision"],
        serde_json::Value::Null,
        "the working tree has no revision"
    );
}

#[test]
fn changes_made_on_the_base_branch_are_not_blamed_on_the_head() {
    // main moves on to version 2 after the feature branched from version 1;
    // the feature branch itself changes nothing.
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    write(dir.path(), "faultline.toml", CONFIG);
    write(dir.path(), "api/openapi.yaml", &example("v1"));
    commit(dir.path(), "pet store v1");
    git(dir.path(), &["branch", "feature"]);
    write(dir.path(), "api/openapi.yaml", &example("v2"));
    commit(dir.path(), "pet store v2");
    git(dir.path(), &["checkout", "-q", "feature"]);

    let output = faultline(dir.path(), &["check", "--base", "main"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(stdout(&output).contains("result: passed"));

    // Against main itself, the branch looks like it reverts main's changes.
    let direct = faultline(dir.path(), &["check", "--base", "main", "--no-merge-base"]);
    assert_ne!(direct.status.code(), Some(0), "{}", stdout(&direct));
    assert!(stdout(&direct).contains("main:api/openapi.yaml"));
}

#[test]
fn head_revisions_can_replace_the_working_tree() {
    let dir = repository();
    commit(dir.path(), "pet store v2");
    // The working tree goes back to version 1, but --head reads the commit.
    write(dir.path(), "api/openapi.yaml", &example("v1"));
    let output = faultline(
        dir.path(),
        &["check", "--base", "main", "--head", "feature"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("--> feature:api/openapi.yaml:"));
}

#[test]
fn apis_missing_from_the_base_are_new() {
    let dir = repository();
    write(
        dir.path(),
        "faultline.toml",
        &format!("{CONFIG}\n[[api]]\nname = \"events\"\nentry = \"events/schema.json\"\n"),
    );
    write(dir.path(), "events/schema.json", r#"{"type": "object"}"#);
    let output = faultline(dir.path(), &["check", "--base", "main", "--api", "events"]);
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(
        text.contains("[FLE0905]") && text.contains("the API is new"),
        "{text}"
    );

    let unknown = faultline(dir.path(), &["check", "--base", "main", "--api", "nope"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("no [[api]] named nope"));
}

#[test]
fn github_base_ref_supplies_the_default_base() {
    let dir = repository();
    let missing = faultline(dir.path(), &["check"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(
        stderr(&missing).contains("pass --base"),
        "{}",
        stderr(&missing)
    );

    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    let output = Command::new(env!("CARGO_BIN_EXE_faultline"))
        .arg("check")
        .current_dir(dir.path())
        .env("GITHUB_BASE_REF", "main")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    // GitLab merge request pipelines name the diff's base commit.
    let sha = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "main"])
            .current_dir(dir.path())
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_faultline"))
        .arg("check")
        .current_dir(dir.path())
        .env_remove("GITHUB_BASE_REF")
        .env("CI_MERGE_REQUEST_DIFF_BASE_SHA", sha.trim())
        .env("CI_MERGE_REQUEST_TARGET_BRANCH_NAME", "does-not-exist")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
}

#[test]
fn diff_reads_either_side_from_a_revision() {
    let dir = repository();
    // NEW defaults to OLD, so this compares main's copy with the working
    // tree's.
    let output = faultline(
        dir.path(),
        &["diff", "--old-rev", "main", "api/openapi.yaml"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(stdout(&output).contains("openapi 1.0.0 -> 2.0.0: 5 breaking"));

    commit(dir.path(), "pet store v2");
    let reverse = faultline(
        dir.path(),
        &[
            "diff",
            "--old-rev",
            "feature",
            "--new-rev",
            "main",
            "api/openapi.yaml",
            "--fail-on",
            "never",
        ],
    );
    assert_eq!(reverse.status.code(), Some(0), "{}", stderr(&reverse));
    assert!(stdout(&reverse).contains("openapi 2.0.0 -> 1.0.0"));
}

#[test]
fn revision_errors_exit_with_2() {
    let dir = repository();
    let output = faultline(dir.path(), &["check", "--base", "no-such-branch"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("[FLE0904]"), "{}", stdout(&output));

    let output = faultline(
        dir.path(),
        &[
            "diff",
            "--old-rev",
            "main",
            "api/missing.yaml",
            "api/openapi.yaml",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stdout(&output).contains("api/missing.yaml does not exist in revision main"),
        "{}",
        stdout(&output)
    );

    let plain = tempfile::tempdir().unwrap();
    write(plain.path(), "api.json", "{}");
    let output = faultline(plain.path(), &["diff", "--old-rev", "HEAD", "api.json"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("[FLE0903]"), "{}", stdout(&output));
}

#[test]
fn overlays_missing_from_the_base_are_noted() {
    let dir = repository();
    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    // The overlay and its configuration are new on the branch.
    write(
        dir.path(),
        "faultline.toml",
        &format!("{CONFIG}overlays = [\"api/public.yaml\"]\n"),
    );
    write(
        dir.path(),
        "api/public.yaml",
        "overlay: 1.0.0\ninfo: {title: Public, version: 1.0.0}\nactions:\n  - target: $.info\n    update: {description: The public API.}\n",
    );
    let output = faultline(dir.path(), &["check", "--base", "origin/main"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    assert!(
        text.contains("the overlay api/public.yaml does not exist in the merge base of origin/main and HEAD, which is compared without it"),
        "{text}"
    );
}

#[test]
fn entry_patterns_are_read_from_revisions() {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    write(
        dir.path(),
        "faultline.toml",
        "[[api]]\nname = \"graph\"\nentry = \"schema/*.graphql\"\n",
    );
    write(
        dir.path(),
        "schema/query.graphql",
        "type Query { pet: Pet }\n",
    );
    write(
        dir.path(),
        "schema/pet.graphql",
        "type Pet { name: String }\n",
    );
    commit(dir.path(), "schema");
    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    // The branch removes a field, in a file of the split schema.
    write(dir.path(), "schema/pet.graphql", "type Pet { id: ID }\n");
    let output = faultline(dir.path(), &["check", "--base", "origin/main"]);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    assert!(
        stdout(&output).contains("graph: 1 breaking"),
        "{}",
        stdout(&output)
    );
}

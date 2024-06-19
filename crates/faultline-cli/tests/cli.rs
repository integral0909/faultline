//! End-to-end tests of the `faultline` binary.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A workspace containing the pet store example's two versions.
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for version in ["v1", "v2"] {
        std::fs::create_dir_all(dir.path().join(version)).unwrap();
        std::fs::copy(
            repo().join(format!("examples/petstore/{version}/openapi.yaml")),
            dir.path().join(format!("{version}/openapi.yaml")),
        )
        .unwrap();
    }
    dir
}

fn faultline(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_faultline"))
        .args(args)
        .current_dir(dir)
        .env("FAULTLINE_TODAY", "2026-09-28")
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

const DIFF: [&str; 3] = ["diff", "v1/openapi.yaml", "v2/openapi.yaml"];

#[test]
fn text_report_and_exit_code() {
    let dir = workspace();
    let output = faultline(dir.path(), &DIFF);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let text = stdout(&output);
    for fragment in [
        "breaking[FL2001]: op[DELETE /pets/{}]: operation removed",
        "breaking[FL2010]: op[GET /pets]/param[query:species]: required query parameter `species` added",
        "example: 100 was accepted and is now rejected",
        "risky[FL2040]: op[GET /pets]/response[429]: response 429 added",
        "--> v2/openapi.yaml:",
        "openapi 1.0.0 -> 2.0.0: 5 breaking, 2 risky, 2 compatible; suggested version bump: major",
        "result: failed (5 findings at or above breaking)",
    ] {
        assert!(text.contains(fragment), "missing {fragment:?} in:\n{text}");
    }
}

#[test]
fn thresholds() {
    let dir = workspace();
    assert_eq!(
        faultline(dir.path(), &[&DIFF[..], &["--fail-on", "never"]].concat())
            .status
            .code(),
        Some(0)
    );
    let same = faultline(
        dir.path(),
        &[
            "diff",
            "v1/openapi.yaml",
            "v1/openapi.yaml",
            "--fail-on",
            "risky",
        ],
    );
    assert_eq!(same.status.code(), Some(0));
    assert!(stdout(&same).contains("result: passed"));
}

#[test]
fn json_report() {
    let dir = workspace();
    let output = faultline(dir.path(), &[&DIFF[..], &["--output", "json"]].concat());
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["exit_code"], 1);
    assert_eq!(report["summary"]["breaking"], 5);
    assert_eq!(report["summary"]["risky"], 2);
    assert_eq!(report["apis"][0]["suggested_bump"], "major");
    let limit = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "op[GET /pets]/param[query:limit]")
        .unwrap();
    assert_eq!(limit["rule"], "FL1007");
    assert_eq!(limit["witness"], 100);
    assert_eq!(limit["new_location"]["file"], "v2/openapi.yaml");
    assert_eq!(limit["id"].as_str().unwrap().len(), 12);
}

#[test]
fn sarif_report() {
    let dir = workspace();
    let output = faultline(dir.path(), &[&DIFF[..], &["--output", "sarif"]].concat());
    let sarif: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let run = &sarif["runs"][0];
    let results = run["results"].as_array().unwrap();
    assert_eq!(results.len(), 9);
    let rules = run["tool"]["driver"]["rules"].as_array().unwrap();
    for result in results {
        let index = result["ruleIndex"].as_u64().unwrap() as usize;
        assert_eq!(
            rules[index]["id"], result["ruleId"],
            "ruleIndex must point at the rule"
        );
        assert!(result["partialFingerprints"]["faultlineFindingId/v1"].is_string());
    }
    assert!(results.iter().any(|r| r["level"] == "error"));
}

#[test]
fn markdown_github_and_junit() {
    let dir = workspace();
    let markdown = stdout(&faultline(
        dir.path(),
        &[&DIFF[..], &["--output", "markdown"]].concat(),
    ));
    assert!(
        markdown.starts_with("### Faultline found 5 failing changes"),
        "{markdown}"
    );
    assert!(
        markdown.contains("| breaking | FL2001 | `op[DELETE /pets/{}]` | operation removed |"),
        "{markdown}"
    );

    let github = stdout(&faultline(
        dir.path(),
        &[&DIFF[..], &["--output", "github"]].concat(),
    ));
    assert!(
        github
            .lines()
            .any(|l| l.starts_with("::error file=v2/openapi.yaml,line=")),
        "{github}"
    );
    assert!(
        github.lines().any(|l| l.starts_with("::warning ")),
        "{github}"
    );

    let junit = stdout(&faultline(
        dir.path(),
        &[&DIFF[..], &["--output", "junit"]].concat(),
    ));
    assert!(
        junit.contains("<testsuite name=\"openapi\" tests=\"9\" failures=\"5\">"),
        "{junit}"
    );
}

#[test]
fn baselines_accept_existing_findings() {
    let dir = workspace();
    let write = faultline(
        dir.path(),
        &["baseline", "v1/openapi.yaml", "v2/openapi.yaml"],
    );
    assert_eq!(write.status.code(), Some(0), "{}", stderr(&write));
    assert!(dir.path().join(".faultline/baseline.json").is_file());
    let output = faultline(
        dir.path(),
        &[&DIFF[..], &["--baseline", ".faultline/baseline.json"]].concat(),
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(stdout(&output).contains("accepted changes:"));
}

#[test]
fn waivers_from_configuration() {
    let dir = workspace();
    let report: serde_json::Value = serde_json::from_str(&stdout(&faultline(
        dir.path(),
        &[&DIFF[..], &["--output", "json"]].concat(),
    )))
    .unwrap();
    let failing: Vec<&str> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["failing"] == true)
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    let waivers = |expires: &str| {
        failing.iter().fold(String::new(), |mut text, id| {
            let _ = writeln!(
                text,
                "[[waiver]]\nfinding = \"{id}\"\nreason = \"planned for 2.0\"\nexpires = \"{expires}\""
            );
            text
        })
    };

    std::fs::write(dir.path().join("faultline.toml"), waivers("2026-12-31")).unwrap();
    assert_eq!(faultline(dir.path(), &DIFF).status.code(), Some(0));

    std::fs::write(dir.path().join("faultline.toml"), waivers("2026-01-01")).unwrap();
    let expired = faultline(dir.path(), &DIFF);
    assert_eq!(expired.status.code(), Some(1));
    assert!(stdout(&expired).contains("expired on 2026-01-01"));
}

#[test]
fn input_and_configuration_errors_exit_with_2() {
    let dir = workspace();
    let missing = faultline(dir.path(), &["diff", "v1/openapi.yaml", "v2/nope.yaml"]);
    assert_eq!(missing.status.code(), Some(2));

    let outside = faultline(dir.path(), &["diff", "v1/openapi.yaml", "/etc/hosts"]);
    assert_eq!(outside.status.code(), Some(2));
    assert!(
        stdout(&outside).contains("outside the workspace root"),
        "{}",
        stdout(&outside)
    );

    std::fs::write(
        dir.path().join("faultline.toml"),
        "[policy]\nfail-on = \"never\"\n",
    )
    .unwrap();
    let config = faultline(dir.path(), &DIFF);
    assert_eq!(config.status.code(), Some(2));
    assert!(stderr(&config).contains("fail-on"), "{}", stderr(&config));
}

#[test]
fn remote_references_follow_the_network_policy() {
    let dir = tempfile::tempdir().unwrap();
    for version in ["v1", "v2"] {
        std::fs::create_dir(dir.path().join(version)).unwrap();
        std::fs::write(
            dir.path().join(version).join("api.json"),
            r#"{"properties": {"pet": {"$ref": "https://localhost/pet.json"}}}"#,
        )
        .unwrap();
    }
    let diff = ["diff", "v1/api.json", "v2/api.json"];

    // Off by default.
    let output = faultline(dir.path(), &diff);
    assert_eq!(output.status.code(), Some(2));
    assert!(stdout(&output).contains("[FLE1005]"), "{}", stdout(&output));

    // Host patterns are validated.
    std::fs::write(
        dir.path().join("faultline.toml"),
        "[network]\nallow_remote = true\nallowed_hosts = [\"https://localhost\"]\n",
    )
    .unwrap();
    let output = faultline(dir.path(), &diff);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("invalid configuration: [network] allowed_hosts"),
        "{}",
        stderr(&output)
    );

    // An allowlisted name that resolves to a loopback address is refused
    // when connecting; nothing is fetched.
    std::fs::write(
        dir.path().join("faultline.toml"),
        "[network]\nallow_remote = true\nallowed_hosts = [\"localhost\"]\ntimeout_seconds = 5\n",
    )
    .unwrap();
    let output = faultline(dir.path(), &diff);
    assert_eq!(output.status.code(), Some(2));
    let text = stdout(&output);
    assert!(
        text.contains("[FLE1005]") && text.contains("non-public address"),
        "{text}"
    );
}

#[test]
fn explain_rules_and_ir_dump() {
    let dir = workspace();
    let explain = faultline(dir.path(), &["explain", "bound-changed"]);
    assert_eq!(explain.status.code(), Some(0));
    assert!(
        stdout(&explain).starts_with("FL1007 bound-changed: A numeric or size constraint changed")
    );
    assert_eq!(
        faultline(dir.path(), &["explain", "FL9999"]).status.code(),
        Some(2)
    );

    let rules = stdout(&faultline(dir.path(), &["rules"]));
    assert_eq!(rules.lines().count(), faultline_rules::RULES.len());
    assert!(rules.starts_with("FL1001  type-changed"), "{rules}");

    let dump = faultline(dir.path(), &["ir", "dump", "v1/openapi.yaml"]);
    assert_eq!(dump.status.code(), Some(0));
    let dump = stdout(&dump);
    assert!(
        dump.starts_with("api openapi (openapi-3.0) version 1.0.0\n"),
        "{dump}"
    );
    assert!(
        dump.contains("op[GET /pets]/param[query:limit] [in] -> t1\n"),
        "{dump}"
    );
}

#[test]
fn committed_rule_reference_is_current() {
    let generated = stdout(&faultline(&repo(), &["rules", "--markdown"]));
    let path = repo().join("docs/rules.md");
    if std::env::var_os("FAULTLINE_UPDATE_DOCS").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "docs/rules.md is out of date; regenerate it with FAULTLINE_UPDATE_DOCS=1 cargo test -p faultline-cli"
    );
}

#[test]
fn protobuf_descriptions_are_detected_and_compared() {
    let dir = repo().join("examples/grpc");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/pets/v1/pets.proto",
            "v2/pets/v1/pets.proto",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let mut breaking: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "breaking")
        .map(|f| {
            format!(
                "{} {}",
                f["rule"].as_str().unwrap(),
                f["path"].as_str().unwrap()
            )
        })
        .collect();
    breaking.sort();
    assert_eq!(
        breaking,
        [
            "FL1029 op[pets.v1.PetService/GetPet]/response[default]/body[application/grpc].species",
            "FL1031 op[pets.v1.PetService/ListPets]/request[application/grpc].page_size",
            "FL2001 op[pets.v1.PetService/DeletePet]",
            "FL3001 op[pets.v1.PetService/ListPets]",
        ]
    );
    // Imports resolve from the entry's ancestors; the well-known types are
    // built in.
    let text = stdout(&faultline(
        &dir,
        &["diff", "v1/pets/v1/pets.proto", "v2/pets/v1/pets.proto"],
    ));
    assert!(text.contains("--> v2/pets/v1/species.proto:"), "{text}");
    assert!(text.contains("pets: 4 breaking"), "{text}");
}

#[test]
fn graphql_schemas_are_detected_and_compared() {
    let dir = repo().join("examples/graphql");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/schema.graphql",
            "v2/schema.graphql",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["summary"]["breaking"], 5, "{report:#}");
    let union = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["path"]
                .as_str()
                .unwrap()
                .starts_with("op[query/search]/response")
        })
        .unwrap();
    assert_eq!(union["witness"]["__typename"], "Shelter");
}

#[test]
fn avro_schemas_are_compared_in_each_compatibility_mode() {
    let dir = repo().join("examples/avro");
    let breaking = |role: &str| -> Vec<String> {
        let output = faultline(
            &dir,
            &[
                "diff",
                "v1/pet.avsc",
                "v2/pet.avsc",
                "--role",
                role,
                "--output",
                "json",
            ],
        );
        let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["severity"] == "breaking")
            .map(|f| f["message"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(breaking("input"), ["required property `weightGrams` added"]);
    assert_eq!(breaking("output"), ["property `shelter` removed"]);
}

#[test]
fn declared_versions_are_checked_with_semver_versioning() {
    let dir = workspace();
    // Version 2 claims to be a minor release despite its breaking changes.
    let v2 = dir.path().join("v2/openapi.yaml");
    let text = std::fs::read_to_string(&v2)
        .unwrap()
        .replace("version: 2.0.0", "version: 1.1.0");
    std::fs::write(&v2, text).unwrap();
    std::fs::write(
        dir.path().join("faultline.toml"),
        "[policy]\nversioning = \"semver\"\nfail_on = \"never\"\n",
    )
    .unwrap();
    let output = faultline(dir.path(), &DIFF);
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let text = stdout(&output);
    assert!(
        text.contains(
            "error: openapi declares 1.0.0 -> 1.1.0, a minor release, but its changes need a major release"
        ),
        "{text}"
    );
    assert!(
        text.contains("result: failed (the declared version does not match the changes)"),
        "{text}"
    );

    let json = faultline(dir.path(), &[&DIFF[..], &["--output", "json"]].concat());
    let report: serde_json::Value = serde_json::from_str(&stdout(&json)).unwrap();
    assert_eq!(report["version_violations"][0]["required_bump"], "major");
    assert_eq!(report["passed"], false);

    let notes = faultline(
        dir.path(),
        &[&DIFF[..], &["--output", "changelog"]].concat(),
    );
    let notes = stdout(&notes);
    assert!(
        notes.starts_with("# Changelog\n\n## openapi 1.1.0 (from 1.0.0)"),
        "{notes}"
    );
    assert!(
        notes.contains("### Breaking changes\n\n- `op[DELETE /pets/{}]`: operation removed"),
        "{notes}"
    );
}

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
fn html_reports_are_self_contained() {
    let dir = workspace();
    let output = faultline(dir.path(), &[&DIFF[..], &["--output", "html"]].concat());
    assert_eq!(output.status.code(), Some(1));
    let page = stdout(&output);
    assert!(page.starts_with("<!DOCTYPE html>"), "{page}");
    assert!(
        page.contains("<h1>Faultline found 5 failing changes</h1>"),
        "{page}"
    );
    assert!(page.contains("<code>op[DELETE /pets/{}]</code>"), "{page}");
    assert!(
        page.contains("enum value &quot;adopted&quot; added"),
        "{page}"
    );
    assert!(!page.contains("<script"), "no scripts");
    assert!(
        !page.contains("src=\"http") && !page.contains("<link"),
        "nothing is loaded from elsewhere"
    );
    assert!(page.trim_end().ends_with("</html>"));
}

#[test]
fn gitlab_code_quality() {
    let dir = workspace();
    let output = faultline(dir.path(), &[&DIFF[..], &["--output", "gitlab"]].concat());
    let issues: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let issues = issues.as_array().unwrap();
    let removed = issues
        .iter()
        .find(|i| i["check_name"] == "FL2001")
        .expect("the removed operation");
    assert_eq!(removed["severity"], "critical");
    assert_eq!(removed["location"]["path"], "v1/openapi.yaml");
    assert!(removed["location"]["lines"]["begin"].as_u64().unwrap() > 1);
    assert_eq!(removed["fingerprint"].as_str().unwrap().len(), 12);
    let mut fingerprints: Vec<&str> = issues
        .iter()
        .map(|i| i["fingerprint"].as_str().unwrap())
        .collect();
    fingerprints.sort_unstable();
    fingerprints.dedup();
    assert_eq!(fingerprints.len(), issues.len(), "fingerprints are unique");
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
fn diagnostic_codes_are_explained() {
    let output = faultline(&repo(), &["explain", "fle7605"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        stdout(&output).starts_with("FLE7605 (faultline-raml)\n\nA type, trait"),
        "{}",
        stdout(&output)
    );
    let unknown = faultline(&repo(), &["explain", "FLE9999"]);
    assert_eq!(unknown.status.code(), Some(2));

    // Diagnostics point at their explanation.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("api.raml"),
        "#%RAML 1.0\ntitle: T\n/a:\n  get:\n    is: [missing]\n",
    )
    .unwrap();
    let output = faultline(dir.path(), &["diff", "api.raml", "api.raml"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stdout(&output).contains("= help: `faultline explain FLE7605` describes this code"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn completion_scripts() {
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let output = faultline(&repo(), &["completions", shell]);
        assert_eq!(output.status.code(), Some(0), "{shell}");
        let script = stdout(&output);
        assert!(
            script.contains("faultline") && script.contains("diff"),
            "{shell}"
        );
    }
    assert_eq!(
        faultline(&repo(), &["completions", "tcsh"]).status.code(),
        Some(2)
    );
}

#[test]
fn swagger_migrations_change_nothing() {
    let dir = repo().join("examples/swagger");
    let migrated = faultline(&dir, &["diff", "swagger.yaml", "openapi.yaml"]);
    assert_eq!(migrated.status.code(), Some(0), "{}", stdout(&migrated));
    assert!(stdout(&migrated).contains("0 breaking, 0 risky, 0 compatible"));
    // Against the next version, the findings are the OpenAPI versions'.
    // Both files in the workspace: run from the examples directory.
    let from_swagger = faultline(
        &repo().join("examples"),
        &[
            "diff",
            "swagger/swagger.yaml",
            "petstore/v2/openapi.yaml",
            "--output",
            "json",
        ],
    );
    assert_eq!(
        from_swagger.status.code(),
        Some(1),
        "{}",
        stdout(&from_swagger)
    );
    let from_openapi = faultline(
        &repo().join("examples/petstore"),
        &[
            "diff",
            "v1/openapi.yaml",
            "v2/openapi.yaml",
            "--output",
            "json",
        ],
    );
    let rules = |output: &Output| -> Vec<String> {
        let report: serde_json::Value = serde_json::from_str(&stdout(output)).unwrap();
        let mut rules: Vec<String> = report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                format!(
                    "{} {}",
                    f["rule"].as_str().unwrap(),
                    f["path"].as_str().unwrap()
                )
            })
            .collect();
        rules.sort();
        rules
    };
    assert_eq!(rules(&from_swagger), rules(&from_openapi));
}

#[test]
fn graphql_schemas_split_over_files() {
    let dir = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    for version in ["v1", "v2"] {
        write(
            &format!("{version}/schema/query.graphql"),
            "type Query { pet(id: ID!): Pet }\n",
        );
        write(&format!("{version}/schema/notes.md"), "not a schema");
    }
    write(
        "v1/schema/pet.graphql",
        "type Pet { id: ID! name: String }\n",
    );
    // The new version extends the type in a file of its own.
    write("v2/schema/pet.graphql", "type Pet { id: ID! }\n");
    write(
        "v2/schema/pet-name.graphql",
        "extend type Pet { name: Int }\n",
    );

    let output = faultline(
        dir.path(),
        &[
            "diff",
            "v1/schema/*.graphql",
            "v2/schema/*.graphql",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let findings: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            format!(
                "{} {}",
                f["api"].as_str().unwrap(),
                f["rule"].as_str().unwrap()
            )
        })
        .collect();
    assert!(
        findings.iter().all(|f| f.starts_with("schema ")),
        "named after the directory: {findings:?}"
    );
    assert!(!findings.is_empty(), "name became an Int");

    let nothing = faultline(dir.path(), &["diff", "v1/schema/*.gql", "v2/schema/*.gql"]);
    assert_eq!(nothing.status.code(), Some(2), "{}", stdout(&nothing));
    let other = faultline(dir.path(), &["diff", "v1/schema/*.md", "v2/schema/*.md"]);
    assert_eq!(other.status.code(), Some(2));
    assert!(
        stdout(&other).contains("read as one GraphQL schema"),
        "{}",
        stdout(&other)
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
fn asyncapi_migrations_are_compared_by_meaning() {
    let dir = repo().join("examples/asyncapi");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/asyncapi.yaml",
            "v2/asyncapi.yaml",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let findings: Vec<(String, String)> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["severity"].as_str().unwrap().to_owned(),
                f["path"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let expected = [
        (
            "breaking",
            "op[receive payments]/message[PaymentReceived].payload",
        ),
        (
            "breaking",
            "webhook[sent]/op[send orders/{}/placed]/message[OrderPlaced].payload",
        ),
        (
            "risky",
            "webhook[sent]/op[send orders/{}/placed]/message[OrderCancelled]",
        ),
        (
            "info",
            "op[receive shipments]/message[ShipmentRequested].payload",
        ),
        (
            "info",
            "webhook[sent]/op[send orders/{}/placed]/message[OrderPlaced].payload",
        ),
    ];
    let mut sorted = findings.clone();
    sorted.sort();
    let mut wanted: Vec<(String, String)> = expected
        .iter()
        .map(|(s, p)| ((*s).to_owned(), (*p).to_owned()))
        .collect();
    wanted.sort();
    assert_eq!(sorted, wanted);
    assert_eq!(report["apis"][0]["suggested_bump"], "major");
}

#[test]
fn grpc_services_are_checked_against_their_openapi_description() {
    let dir = repo().join("examples/grpc-gateway");
    let output = faultline(
        &dir,
        &[
            "diff",
            "openapi.yaml",
            "proto/pets.proto",
            "--new-format",
            "protobuf-http",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let mut breaking: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "breaking")
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    breaking.sort();
    assert_eq!(
        breaking,
        [
            "op[DELETE /v1/{}]",
            "op[GET /v1/{}]/response[200]/body[application/json].weightGrams",
            "op[POST /v1/pets]/request[application/json].weightGrams",
        ]
    );
    assert!(report["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn swagger_migrations_compare_cleanly() {
    let dir = repo().join("examples");
    let migrated = faultline(
        &dir,
        &["diff", "swagger/swagger.yaml", "swagger/openapi.yaml"],
    );
    assert_eq!(migrated.status.code(), Some(0), "{}", stdout(&migrated));
    // Swagger 2.0 against the next OpenAPI 3 version finds what the OpenAPI
    // 3 versions do.
    let json = |old: &str| -> Vec<String> {
        let output = faultline(
            &dir,
            &["diff", old, "petstore/v2/openapi.yaml", "--output", "json"],
        );
        let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
        let mut ids: Vec<String> = report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| format!("{} {}", f["rule"], f["path"]))
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        json("swagger/swagger.yaml"),
        json("petstore/v1/openapi.yaml")
    );
}

#[test]
fn xml_schemas_are_compared() {
    let dir = repo().join("examples/xsd");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/order.xsd",
            "v2/order.xsd",
            "--role",
            "input",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let mut breaking: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "breaking")
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    breaking.sort();
    assert_eq!(
        breaking,
        ["schema[order].@currency", "schema[order].line[]"]
    );
    assert_eq!(
        report["findings"][0]["new_location"]["file"],
        "v2/order.xsd"
    );
}

#[test]
fn wsdl_services_are_compared() {
    let dir = repo().join("examples/wsdl");
    let output = faultline(
        &dir,
        &["diff", "v1/pets.wsdl", "v2/pets.wsdl", "--output", "json"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let mut breaking: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "breaking")
        .map(|f| f["path"].as_str().unwrap().to_owned())
        .collect();
    breaking.sort();
    assert_eq!(
        breaking,
        [
            "op[PetPortType/CountPets]/param[header:soapaction]",
            "op[PetPortType/GetPet]/request[text/xml]",
        ]
    );
}

#[test]
fn raml_apis_are_compared() {
    let dir = repo().join("examples/raml");
    let output = faultline(
        &dir,
        &["diff", "v1/pets.raml", "v2/pets.raml", "--output", "json"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let mut breaking: Vec<(String, String)> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] == "breaking")
        .map(|f| {
            let file = f["old_location"]["file"].as_str().unwrap_or_default();
            (f["path"].as_str().unwrap().to_owned(), file.to_owned())
        })
        .collect();
    breaking.sort();
    assert_eq!(
        breaking,
        [
            ("op[DELETE /pets/{}]".into(), "v1/pets.raml".into()),
            (
                "op[GET /pets]/param[query:limit]".into(),
                "v1/lib/common.raml".into()
            ),
            (
                "op[GET /pets]/param[query:species]".into(),
                "v1/pets.raml".into()
            ),
        ]
    );
}

#[test]
fn thrift_services_are_compared() {
    let dir = repo().join("examples/thrift");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/pets.thrift",
            "v2/pets.thrift",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
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
            "FL1026 op[Pets/search]/request[application/x-thrift].limit",
            "FL1031 op[Pets/get]/response[default]/body[application/x-thrift].success.age",
            "FL1031 op[Pets/get]/response[default]/body[application/x-thrift].success.tags",
            "FL3002 op[Pets/refresh]",
        ]
    );
}

#[test]
fn json_type_definitions_are_compared() {
    let dir = repo().join("examples/jtd");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/event.jtd.json",
            "v2/event.jtd.json",
            "--role",
            "output",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let findings = report["findings"].as_array().unwrap();
    assert!(
        findings.iter().all(|f| f["api"] == "event"),
        "named without .jtd"
    );
    let breaking: Vec<&str> = findings
        .iter()
        .filter(|f| f["severity"] == "breaking")
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(breaking, ["schema[event].change"]);
}

#[test]
fn overlays_apply_before_comparing() {
    let dir = repo().join("examples/overlay");
    let without = faultline(&dir, &["diff", "v1/openapi.yaml", "v2/openapi.yaml"]);
    assert_eq!(without.status.code(), Some(1), "{}", stdout(&without));
    let with = faultline(
        &dir,
        &[
            "diff",
            "v1/openapi.yaml",
            "v2/openapi.yaml",
            "--overlay",
            "public.yaml",
        ],
    );
    assert_eq!(with.status.code(), Some(0), "{}", stdout(&with));
    assert!(stdout(&with).contains("0 breaking, 0 risky, 1 compatible"));

    let missing = faultline(
        &dir,
        &[
            "diff",
            "v1/openapi.yaml",
            "v2/openapi.yaml",
            "--overlay",
            "none.yaml",
        ],
    );
    assert_eq!(missing.status.code(), Some(2));
    assert!(stdout(&missing).contains("FLE0906"), "{}", stdout(&missing));
}

#[test]
fn overlays_only_apply_to_documents() {
    let dir = repo().join("examples/thrift");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/pets.thrift",
            "v2/pets.thrift",
            "--overlay",
            "v1/shared.thrift",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stdout(&output).contains("overlays apply to JSON and YAML descriptions, not to thrift"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn smithy_models_are_compared() {
    let dir = repo().join("examples/smithy");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/model/weather.smithy",
            "v2/model/weather.smithy",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let mut found: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["severity"] != "info")
        .map(|f| {
            format!(
                "{} {}",
                f["rule"].as_str().unwrap(),
                f["path"].as_str().unwrap()
            )
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            "FL1007 op[GET /cities]/param[query:pageSize]",
            "FL2012 op[GET /cities]/param[query:nextToken]",
            "FL2040 op[GET /cities/{}]/response[410]",
        ],
        "the error defined in the other file is found"
    );
}

#[test]
fn avro_idl_protocols_are_compared() {
    let dir = repo().join("examples/avro-idl");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/orders.avdl",
            "v2/orders.avdl",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let breaking: Vec<String> = report["findings"]
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
    assert_eq!(
        breaking,
        ["FL1012 op[Orders/get]/response[default]/body[avro/binary]"]
    );
}

#[test]
fn init_lists_the_api_descriptions_of_a_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let write = |path: &str, text: &str| {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "api/openapi.yaml",
        "openapi: 3.1.0\ninfo: {title: T, version: 1.0.0}\npaths: {}\n",
    );
    write("api/lib.raml", "#%RAML 1.0 Library\n");
    write(
        "proto/pets.proto",
        "syntax = \"proto3\";\nservice Pets {}\n",
    );
    write(
        "proto/types.proto",
        "syntax = \"proto3\";\nmessage Pet {}\n",
    );
    write(
        "idl/main.thrift",
        "include \"shared.thrift\"\nservice Main {}\n",
    );
    write("idl/shared.thrift", "service Shared {}\n");
    write("config/app.yaml", "port: 8080\n");
    write("node_modules/dep/openapi.yaml", "openapi: 3.0.0\n");
    write(".hidden/openapi.yaml", "openapi: 3.0.0\n");

    write("graph/schema.graphql", "type Query { pet: Pet }\n");
    write("graph/pet.graphql", "type Pet { name: String }\n");
    write("graph/queries.graphql", "query Pets { pet { name } }\n");
    write("single/schema.graphql", "type Query { ok: Boolean }\n");
    write(
        "weather/smithy-build.json",
        "{\"version\": \"1.0\", \"sources\": [\"model\"]}\n",
    );
    write(
        "weather/model/main.smithy",
        "namespace w\nservice Weather {}\n",
    );
    write("single/ops.graphql", "query Ok { ok }\n");

    let printed = faultline(dir.path(), &["init", "--print"]);
    assert_eq!(printed.status.code(), Some(0), "{}", stderr(&printed));
    let config = stdout(&printed);
    let entries: Vec<&str> = config
        .lines()
        .filter_map(|l| l.strip_prefix("entry = "))
        .collect();
    assert_eq!(
        entries,
        [
            "\"api/openapi.yaml\"",
            "\"graph/*.graphql\"",
            "\"idl/main.thrift\"",
            "\"proto/pets.proto\"",
            "\"single/schema.graphql\"",
            "\"weather/smithy-build.json\"",
        ],
        "a split schema is a pattern; operations beside a schema are not part of it: {config}"
    );
    assert!(config.contains("name = \"graph\""), "{config}");
    assert!(
        !dir.path().join("faultline.toml").exists(),
        "--print writes nothing"
    );

    let written = faultline(dir.path(), &["init"]);
    assert_eq!(written.status.code(), Some(0), "{}", stderr(&written));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("faultline.toml")).unwrap(),
        config
    );
    let again = faultline(dir.path(), &["init"]);
    assert_eq!(again.status.code(), Some(2));
    assert!(stderr(&again).contains("--force"), "{}", stderr(&again));
    assert_eq!(
        faultline(dir.path(), &["init", "--force"]).status.code(),
        Some(0)
    );
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

#[test]
fn usage_indexes_soften_changes_nobody_uses() {
    let dir = workspace();
    std::fs::write(
        dir.path().join("access.log"),
        concat!(
            "10.0.0.1 - - [01/Sep/2026:10:00:00 +0000] \"GET /v1/pets?limit=4711&species=zebracat HTTP/1.1\" 200 512\n",
            "10.0.0.2 - - [01/Sep/2026:10:00:01 +0000] \"POST /v1/pets HTTP/1.1\" 201 64\n",
            "{\"method\": \"GET\", \"path\": \"/v1/pets/42\"}\n",
            "10.0.0.3 - - [01/Sep/2026:10:00:02 +0000] \"GET /v1/unknown HTTP/1.1\" 404 0\n",
        ),
    )
    .unwrap();
    let import = faultline(
        dir.path(),
        &[
            "usage",
            "import",
            "v1/openapi.yaml",
            "--access-log",
            "access.log",
            "--base-path",
            "/v1",
            "--window",
            "September 2026",
        ],
    );
    assert_eq!(import.status.code(), Some(0), "{}", stderr(&import));
    assert!(
        stderr(&import).contains("1 unmatched"),
        "{}",
        stderr(&import)
    );
    let index: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".faultline/usage.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(index["operations"]["GET /pets"]["calls"], 1);
    assert_eq!(
        index["operations"]["GET /pets"]["parameters"]["query:limit"],
        1
    );
    assert_eq!(index["operations"]["DELETE /pets/{}"]["calls"], 0);
    assert_eq!(index["window"], "September 2026");
    assert!(
        !index.to_string().contains("4711") && !index.to_string().contains("zebracat"),
        "query values are not stored: {index}"
    );

    std::fs::write(
        dir.path().join("faultline.toml"),
        "[usage]\nindex = \".faultline/usage.json\"\n",
    )
    .unwrap();
    let output = faultline(dir.path(), &[&DIFF[..], &["--output", "json"]].concat());
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let removed = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "FL2001")
        .unwrap();
    assert_eq!(
        removed["severity"], "risky",
        "nobody calls DELETE /pets/{{id}}"
    );
    assert_eq!(removed["original_severity"], "breaking");
    assert_eq!(removed["unused"], true);
    let limit = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["rule"] == "FL1007" && f["path"] == "op[GET /pets]/param[query:limit]")
        .unwrap();
    assert_eq!(limit["severity"], "breaking", "clients do send `limit`");
}

#[test]
fn recorded_traffic_shows_which_request_fields_are_sent() {
    let dir = workspace();
    let entry = |method: &str, url: &str, body: Option<&str>| {
        let post = body.map_or(
            serde_json::Value::Null,
            |text| serde_json::json!({"mimeType": "application/json", "text": text}),
        );
        serde_json::json!({
            "request": {
                "method": method,
                "url": url,
                "headers": [{"name": "Cookie", "value": "session=placeholder"}],
                "queryString": [],
                "postData": post,
            },
            "response": {"status": 200, "content": {"mimeType": "application/json", "text": "[]"}},
        })
    };
    let har = serde_json::json!({"log": {"version": "1.2", "entries": [
        entry("GET", "https://pets.example.com/v1/pets?species=cat", None),
        entry("POST", "https://pets.example.com/v1/pets", Some(r#"{"name": "Rex"}"#)),
        entry("GET", "https://pets.example.com/v1/elsewhere", None),
    ]}});
    std::fs::write(dir.path().join("traffic.har"), har.to_string()).unwrap();
    let import = faultline(
        dir.path(),
        &[
            "usage",
            "import",
            "v1/openapi.yaml",
            "--har",
            "traffic.har",
            "--base-path",
            "/v1",
        ],
    );
    assert_eq!(import.status.code(), Some(0), "{}", stderr(&import));
    let text = std::fs::read_to_string(dir.path().join(".faultline/usage.json")).unwrap();
    for secret in ["Rex", "placeholder", "cat", "Cookie", "cookie"] {
        assert!(!text.contains(secret), "{secret} is stored: {text}");
    }
    let index: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(index["source"], "har");
    assert_eq!(index["unmatched"], 1);
    assert_eq!(index["operations"]["POST /pets"]["fields"]["()name"], 1);

    std::fs::write(
        dir.path().join("faultline.toml"),
        "[usage]\nindex = \".faultline/usage.json\"\n",
    )
    .unwrap();
    let output = faultline(dir.path(), &[&DIFF[..], &["--output", "json"]].concat());
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let finding = |path: &str| {
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("no finding at {path}"))
            .clone()
    };
    // Nobody sends `limit`: its tighter bound is softened.
    let limit = finding("op[GET /pets]/param[query:limit]");
    assert_eq!(limit["unused"], true);
    assert_eq!(limit["severity"], "risky");
    // Clients send `name`: its shorter maximum length still breaks them.
    let name = finding("op[POST /pets]/request[application/json].name");
    assert_eq!(name["unused"], false);
    assert_eq!(name["severity"], "breaking");
}

#[test]
fn graphql_client_documents_are_usage() {
    let dir = tempfile::tempdir().unwrap();
    for version in ["v1", "v2"] {
        std::fs::create_dir(dir.path().join(version)).unwrap();
        std::fs::copy(
            repo().join(format!("examples/graphql/{version}/schema.graphql")),
            dir.path().join(format!("{version}/schema.graphql")),
        )
        .unwrap();
    }
    std::fs::write(
        dir.path().join("client.graphql"),
        "query Pet($id: ID!) { pet(id: $id) { name species ...Owner } }\nfragment Owner on Pet { owner { name } }\nmutation { addPet(input: {name: \"x\", species: CAT}) { id } }\n",
    )
    .unwrap();
    let import = faultline(
        dir.path(),
        &[
            "usage",
            "import",
            "v1/schema.graphql",
            "--graphql",
            "client.graphql",
        ],
    );
    assert_eq!(import.status.code(), Some(0), "{}", stderr(&import));
    std::fs::write(
        dir.path().join("faultline.toml"),
        "[usage]\nindex = \".faultline/usage.json\"\nunused_severity = \"info\"\n",
    )
    .unwrap();
    let output = faultline(
        dir.path(),
        &[
            "diff",
            "v1/schema.graphql",
            "v2/schema.graphql",
            "--output",
            "json",
        ],
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let severity = |path_start: &str| {
        report["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| f["path"].as_str().unwrap().starts_with(path_start))
            .map(|f| f["severity"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    // No client queries `search` or calls `removePet`.
    assert!(
        severity("op[query/search]").iter().all(|s| s == "info"),
        "{report:#}"
    );
    assert_eq!(severity("op[mutation/removePet]"), ["info"]);
    // Clients send `NewPet`, which gained a required field.
    assert!(
        severity("op[mutation/addPet]/request").contains(&"breaking".to_owned()),
        "{report:#}"
    );
    // `listedAt` became nullable but no client selects it.
    let listed = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"].as_str().unwrap().ends_with(".listedAt"))
        .unwrap();
    assert_eq!(listed["unused"], true);
}

#[test]
fn asyncapi_protobuf_payloads_are_compared() {
    let dir = repo().join("examples/asyncapi-protobuf");
    let output = faultline(
        &dir,
        &[
            "diff",
            "v1/asyncapi.yaml",
            "v2/asyncapi.yaml",
            "--output",
            "json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stdout(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    let breaking: Vec<String> = report["findings"]
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
    assert_eq!(
        breaking,
        [
            "FL1001 webhook[sent]/op[send shop.orders.placed]/message[OrderPlaced].payload.lines[].sku"
        ]
    );
}

#[test]
fn reports_are_also_written_to_files() {
    let dir = workspace();
    let output = faultline(
        dir.path(),
        &[
            "diff",
            "v1/openapi.yaml",
            "v2/openapi.yaml",
            "--report",
            "sarif=out/faultline.sarif",
            "--report",
            "JSON=report.json",
        ],
    );
    // The report directory does not exist yet.
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(stderr(&output).contains("cannot write out/faultline.sarif"));

    std::fs::create_dir(dir.path().join("out")).unwrap();
    let output = faultline(
        dir.path(),
        &[
            "diff",
            "v1/openapi.yaml",
            "v2/openapi.yaml",
            "--report",
            "sarif=out/faultline.sarif",
            "--report",
            "JSON=report.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stdout(&output).contains("breaking[FL"),
        "text goes to stdout"
    );
    let sarif: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("out/faultline.sarif")).unwrap(),
    )
    .unwrap();
    assert_eq!(sarif["version"], "2.1.0");
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("report.json")).unwrap())
            .unwrap();
    assert!(!json["findings"].as_array().unwrap().is_empty());

    let output = faultline(
        dir.path(),
        &["diff", "v1/openapi.yaml", "--report", "sarif"],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("expected FORMAT=FILE"));
}

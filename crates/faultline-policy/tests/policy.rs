use faultline_diff::{Change, ChangeKind};
use faultline_ir::{IrPath, Polarity};
use faultline_policy::{Baseline, Config, Date, FailOn, Status, evaluate, exit};
use faultline_rules::{Finding, Severity, classify};

fn finding(api: &str, kind: ChangeKind, path: &str, polarity: Polarity) -> Finding {
    let change = Change {
        kind,
        path: path.parse::<IrPath>().unwrap(),
        polarity,
        old_span: None,
        new_span: None,
        effect: None,
    };
    classify(api, &change)
}

fn removed(path: &str) -> Finding {
    finding("public", ChangeKind::OperationRemoved, path, Polarity::IN)
}

fn added(path: &str) -> Finding {
    finding(
        "public",
        ChangeKind::ResponseAdded {
            status: faultline_ir::StatusKey::Code(429),
        },
        path,
        Polarity::OUT,
    )
}

fn today() -> Date {
    Date::parse("2026-09-28").unwrap()
}

#[test]
fn full_configuration_parses() {
    let config = Config::parse(
        r#"
[workspace]
root = "."

[[api]]
name = "public"
entry = "api/openapi.yaml"

[[api]]
name = "events"
entry = "schemas/event.json"
format = "jsonschema"
role = "output"

[policy]
fail_on = "risky"
baseline = ".faultline/baseline.json"

[[policy.override]]
rule = "response-added"
paths = ["op[* /internal/**]**"]
severity = "info"

[[waiver]]
finding = "0123456789AB"
reason = "Field unused since May; confirmed with the mobile team"
owner = "api-platform"
expires = "2026-12-31"
"#,
    )
    .unwrap();
    assert_eq!(config.apis.len(), 2);
    assert_eq!(config.fail_on, FailOn::Risky);
    assert_eq!(config.overrides[0].rule, Some("FL2040"));
    assert_eq!(config.waivers[0].finding, "0123456789ab");
}

#[test]
fn problems_are_reported_together() {
    let error = Config::parse(
        r#"
[[api]]
name = "a"
entry = "x"
format = "wsdl"

[[api]]
name = "a"
entry = "y"

[policy]
fail_on = "sometimes"

[[policy.override]]
rule = "FL9999"
severity = "info"

[[policy.override]]
rule = "*"
severity = "fatal"

[[waiver]]
finding = "nope"
reason = ""
expires = "tomorrow"
"#,
    )
    .unwrap_err();
    let text = error.to_string();
    for fragment in [
        "format must be",
        "another api has the same name",
        "fail_on must be",
        "unknown rule \"FL9999\"",
        "severity must be",
        "12-digit finding id",
        "needs a reason",
        "expires must be a date",
    ] {
        assert!(text.contains(fragment), "missing {fragment:?} in {text}");
    }
}

#[test]
fn network_settings_are_validated() {
    let config = Config::parse(
        r#"
[network]
allow_remote = true
allowed_hosts = ["schemas.example.com", "*.example.net"]
timeout_seconds = 5
max_redirects = 2
"#,
    )
    .unwrap();
    assert!(config.network.allow_remote);
    assert_eq!(config.network.allowed_hosts.len(), 2);
    assert_eq!(config.network.timeout_seconds, Some(5));
    assert!(!Config::default().network.allow_remote);

    let error = Config::parse(
        "[network]\nallow_remote = true\nmax_bytes = 0\ntimeout_seconds = 0\nmax_redirects = 99\n",
    )
    .unwrap_err();
    assert_eq!(error.problems.len(), 4, "{error}");
    assert!(error.to_string().contains("needs at least one entry"));

    let error = Config::parse("[network]\nallow = true\n").unwrap_err();
    assert!(error.to_string().contains("allow"), "{error}");
}

#[test]
fn unknown_keys_are_rejected() {
    let error = Config::parse("[policy]\nfail-on = \"never\"\n").unwrap_err();
    assert!(error.to_string().contains("fail-on"), "{error}");
}

#[test]
fn overrides_apply_by_rule_path_and_api_with_later_ones_winning() {
    let config = Config::parse(
        r#"
[[api]]
name = "public"
entry = "a.yaml"

[[policy.override]]
rule = "*"
paths = ["op[* /internal/**]"]
severity = "info"

[[policy.override]]
rule = "FL2001"
paths = ["op[DELETE /internal/keep]"]
severity = "breaking"
"#,
    )
    .unwrap();
    let evaluation = evaluate(
        &config,
        None,
        vec![
            removed("op[GET /internal/x]"),
            removed("op[DELETE /internal/keep]"),
            removed("op[GET /public]"),
        ],
        today(),
    );
    let severities: Vec<(Severity, Option<Severity>)> = evaluation
        .findings
        .iter()
        .map(|e| (e.finding.severity, e.original_severity))
        .collect();
    assert_eq!(
        severities,
        [
            (Severity::Info, Some(Severity::Breaking)),
            (Severity::Breaking, None),
            (Severity::Breaking, None),
        ]
    );
    assert_eq!(evaluation.failing(), 2);
    assert_eq!(evaluation.exit_code(), exit::FINDINGS);
}

#[test]
fn fail_on_thresholds() {
    for (fail_on, expected) in [("breaking", 1), ("risky", 2), ("never", 0)] {
        let config = Config::parse(&format!("[policy]\nfail_on = \"{fail_on}\"\n")).unwrap();
        let evaluation = evaluate(
            &config,
            None,
            vec![removed("op[GET /a]"), added("op[GET /b]/response[429]")],
            today(),
        );
        assert_eq!(evaluation.failing(), expected, "{fail_on}");
    }
}

#[test]
fn waivers_accept_findings_until_they_expire() {
    let finding = removed("op[GET /a]");
    let waiver = |expires: &str| {
        format!(
            "[[waiver]]\nfinding = \"{}\"\nreason = \"planned\"\nexpires = \"{expires}\"\n",
            finding.id
        )
    };

    let live = Config::parse(&waiver("2026-09-28")).unwrap();
    let evaluation = evaluate(&live, None, vec![finding.clone()], today());
    assert!(
        matches!(evaluation.findings[0].status, Status::Waived(_)),
        "the expiry day is still valid"
    );
    assert_eq!(evaluation.exit_code(), exit::PASSED);

    let expired = Config::parse(&waiver("2026-09-27")).unwrap();
    let evaluation = evaluate(&expired, None, vec![finding.clone()], today());
    assert_eq!(evaluation.findings[0].status, Status::Active);
    assert_eq!(evaluation.expired_waivers.len(), 1);
    assert_eq!(evaluation.exit_code(), exit::FINDINGS);

    // The finding is gone but the expired waiver lingers.
    let evaluation = evaluate(&expired, None, Vec::new(), today());
    assert_eq!(evaluation.exit_code(), exit::EXPIRED_WAIVER);

    let evaluation = evaluate(&live, None, Vec::new(), today());
    assert_eq!(evaluation.unused_waivers.len(), 1);
    assert_eq!(evaluation.exit_code(), exit::PASSED);
}

#[test]
fn baselines_accept_existing_findings_only() {
    let old = removed("op[GET /old]");
    let baseline = Baseline::from_findings([&old]);
    let text = baseline.to_json();
    let parsed = Baseline::parse(&text).unwrap();
    assert_eq!(parsed, baseline);

    let evaluation = evaluate(
        &Config::default(),
        Some(&parsed),
        vec![old, removed("op[GET /new]")],
        today(),
    );
    assert_eq!(evaluation.findings[0].status, Status::Baselined);
    assert_eq!(evaluation.findings[1].status, Status::Active);
    assert_eq!(evaluation.failing(), 1);

    assert!(Baseline::parse("{\"version\": 9, \"findings\": []}").is_err());
    assert!(Baseline::parse("{\"version\": 1, \"findings\": [], \"extra\": 1}").is_err());
}

#[test]
fn declared_versions_must_cover_the_changes() {
    use faultline_policy::{Release, Versioning};
    use faultline_rules::Bump;
    let release = |old: &str, new: &str, required: Bump| Release {
        api: "public".into(),
        old_version: Some(old.into()),
        new_version: Some(new.into()),
        required,
    };
    let config = Config::parse("[policy]\nversioning = \"semver\"\n").unwrap();
    assert_eq!(config.versioning, Versioning::Semver);
    let mut evaluation = evaluate(
        &config,
        None,
        Vec::new(),
        Date::parse("2026-01-01").unwrap(),
    );
    evaluation.check_releases(
        config.versioning,
        &[
            release("1.0.0", "1.1.0", Bump::Major),
            release("1.0.0", "2.0.0", Bump::Major),
            release("1.0.0", "1.0.1", Bump::Patch),
            release("0.4.0", "0.5.0", Bump::Major),
            release("1.0.0", "1.0.0", Bump::None),
            release("2024-01", "2024-02", Bump::Major),
        ],
    );
    assert_eq!(
        evaluation.version_violations.len(),
        1,
        "{:?}",
        evaluation.version_violations
    );
    let violation = &evaluation.version_violations[0];
    assert_eq!(
        (violation.declared, violation.required),
        (Bump::Minor, Bump::Major)
    );
    assert_eq!(evaluation.exit_code(), exit::FINDINGS);

    // Off by default.
    let mut unchecked = evaluate(
        &Config::default(),
        None,
        Vec::new(),
        Date::parse("2026-01-01").unwrap(),
    );
    unchecked.check_releases(Versioning::Off, &[release("1.0.0", "1.0.1", Bump::Major)]);
    assert!(unchecked.version_violations.is_empty());

    let error = Config::parse("[policy]\nversioning = \"calver\"\n").unwrap_err();
    assert!(error.to_string().contains("versioning"), "{error}");
}

#[test]
fn names_round_trip() {
    use faultline_policy::{ApiFormat, FailOn, SchemaRole};
    for format in [
        ApiFormat::OpenApi,
        ApiFormat::JsonSchema,
        ApiFormat::Protobuf,
        ApiFormat::GraphQl,
        ApiFormat::Avro,
    ] {
        assert_eq!(ApiFormat::parse(format.as_str()), Some(format));
    }
    for role in [SchemaRole::Input, SchemaRole::Output, SchemaRole::Both] {
        assert_eq!(SchemaRole::parse(role.as_str()), Some(role));
    }
    for fail_on in [FailOn::Breaking, FailOn::Risky, FailOn::Never] {
        assert_eq!(FailOn::parse(fail_on.as_str()), Some(fail_on));
    }
}

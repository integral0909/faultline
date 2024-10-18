//! RAML APIs lowered and compared.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, ApiFormat, ParamLocation, TypeKind, validate};
use faultline_raml::lower_raml;
use faultline_rules::{Finding, Severity, classify};
use faultline_source::{Limits, SourceMap};

const PETS: &str = include_str!("data/pets.raml");

fn lowered(text: &str) -> faultline_raml::Lowered {
    let loader = MemoryLoader::new().with("file:///api.raml", text);
    let sources = SourceMap::new();
    lower_raml(
        &UriRef::parse("file:///api.raml").unwrap(),
        &loader,
        &sources,
        "pets",
        Limits::default(),
    )
}

fn api(text: &str) -> Api {
    let lowered = lowered(text);
    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
    let api = lowered.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

fn findings(new: &str) -> Vec<Finding> {
    faultline_diff::diff(&api(PETS), &api(new))
        .iter()
        .map(|c| classify("pets", c))
        .collect()
}

fn summary(found: &[Finding]) -> Vec<String> {
    found
        .iter()
        .map(|f| format!("{} {} {}", f.severity, f.rule.name, f.change.path))
        .collect()
}

fn worst(found: &[Finding]) -> Severity {
    found
        .iter()
        .map(|f| f.severity)
        .max()
        .unwrap_or(Severity::Info)
}

#[test]
fn resources_become_operations() {
    let api = api(PETS);
    assert_eq!(api.format, ApiFormat::Raml);
    assert_eq!(api.version.as_deref(), Some("v1"));
    let ops: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(
        ops,
        [
            "GET /pets",
            "POST /pets",
            "GET /pets/{petId}",
            "DELETE /pets/{petId}"
        ]
    );
    let list = &api.operations[0];
    let params: Vec<(&str, ParamLocation, bool)> = list
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location, p.required))
        .collect();
    assert_eq!(
        params,
        [
            ("limit", ParamLocation::Query, false),
            ("species", ParamLocation::Query, false)
        ]
    );
    let TypeKind::Array(_) = &api.types.get(list.responses[0].content[0].ty).kind else {
        panic!("Pet[] is an array")
    };
    assert_eq!(
        list.responses[0].content[0].media_type, "application/json",
        "the default media type"
    );
    assert_eq!(
        list.security.as_ref().unwrap()[0]
            .schemes
            .keys()
            .collect::<Vec<_>>(),
        ["oauth"]
    );

    let create = &api.operations[1];
    assert!(create.request_body.as_ref().unwrap().required);
    assert_eq!(create.responses[0].headers[0].name, "Location");

    let get = &api.operations[2];
    assert_eq!(get.parameters[0].name, "petId");
    assert!(matches!(
        api.types.get(get.parameters[0].ty).kind,
        TypeKind::Integer(_)
    ));
    let media: Vec<&str> = get.responses[0]
        .content
        .iter()
        .map(|c| c.media_type.as_str())
        .collect();
    assert_eq!(media, ["application/json", "application/xml"]);
    assert_eq!(
        get.security.as_ref().unwrap().len(),
        2,
        "null means no credentials needed"
    );
}

#[test]
fn an_unchanged_api_has_no_findings() {
    assert!(findings(PETS).is_empty());
}

#[test]
fn changes_are_classified() {
    // Query parameters are required unless marked optional.
    let required = PETS.replace(
        "      species:\n        type: string\n        required: false\n",
        "      species:\n        type: string\n",
    );
    let found = findings(&required);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));

    let tighter = PETS.replace("maximum: 100", "maximum: 50");
    assert_eq!(worst(&findings(&tighter)), Severity::Breaking);

    let removed = PETS.replace("    delete:\n      responses:\n        204:\n", "");
    let found = findings(&removed);
    assert!(
        found.iter().any(|f| f.rule.name == "operation-removed"),
        "{:?}",
        summary(&found)
    );

    // Responses lose a required property: clients reading it break.
    let fewer = PETS.replace("      id: integer\n", "");
    let found = findings(&fewer);
    assert_eq!(worst(&found), Severity::Breaking, "{:?}", summary(&found));
}

#[test]
fn problems_are_reported() {
    let codes = |text: &str| -> Vec<String> {
        lowered(text)
            .diagnostics
            .iter()
            .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
            .collect()
    };
    assert_eq!(codes("#%RAML 1.0 Library\ntypes: {}\n"), ["FLE7601"]);
    assert_eq!(
        codes("#%RAML 1.0\ntitle: T\n/a:\n  get:\n    responses:\n      ok:\n"),
        ["FLE7604"]
    );
    assert_eq!(
        codes("#%RAML 1.0\ntitle: T\n/a:\n  get:\n    body: Missing\n"),
        ["FLE7605"]
    );
}

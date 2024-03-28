//! Severity of realistic OpenAPI changes, end to end: lower, diff, classify.

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::Api;
use faultline_rules::{Bump, Severity, classify, suggest_bump};
use faultline_source::{Limits, SourceMap};

fn api(text: &str) -> Api {
    let loader = MemoryLoader::new().with("file:///openapi.yaml", text);
    let sources = SourceMap::new();
    let mut store = DocumentStore::new(&loader, &sources, Limits::default());
    let lowered = faultline_openapi::lower_openapi(
        &UriRef::parse("file:///openapi.yaml").unwrap(),
        &mut store,
        "api",
    );
    assert!(
        !lowered.diagnostics.has_errors(),
        "{text}\n{:?}",
        lowered.diagnostics
    );
    lowered.api.unwrap()
}

/// `(code severity path)` for every finding, plus the suggested bump.
fn findings(old: &str, new: &str) -> (Vec<String>, Bump) {
    let changes = faultline_diff::diff(&api(old), &api(new));
    let findings: Vec<_> = changes.iter().map(|c| classify("api", c)).collect();
    let summary = findings
        .iter()
        .map(|f| format!("{} {} {}", f.rule.code, f.severity, f.change.path))
        .collect();
    (summary, suggest_bump(&findings))
}

const HEAD: &str = "openapi: 3.0.3\ninfo: {title: T, version: '1'}\n";

/// An operation whose request and response bodies are `schema`.
fn echo(schema: &str) -> String {
    format!(
        "{HEAD}paths:\n  /x:\n    post:\n      requestBody: {{content: {{application/json: {{schema: {schema}}}}}}}\n      responses:\n        '200': {{description: ok, content: {{application/json: {{schema: {schema}}}}}}}\n"
    )
}

#[test]
fn the_same_change_means_different_things_in_requests_and_responses() {
    let (f, bump) = findings(
        &echo("{type: string, maxLength: 64}"),
        &echo("{type: string, maxLength: 32}"),
    );
    assert_eq!(
        f,
        [
            "FL1007 breaking op[POST /x]/request[application/json]",
            "FL1007 info op[POST /x]/response[200]/body[application/json]",
        ]
    );
    assert_eq!(bump, Bump::Major);

    let (f, bump) = findings(
        &echo("{type: string, maxLength: 32}"),
        &echo("{type: string, maxLength: 64}"),
    );
    assert_eq!(
        f,
        [
            "FL1007 info op[POST /x]/request[application/json]",
            "FL1007 risky op[POST /x]/response[200]/body[application/json]",
        ]
    );
    assert_eq!(bump, Bump::Minor);
}

#[test]
fn properties() {
    let obj = |props: &str, required: &str| {
        format!("{{type: object, properties: {{{props}}}, required: [{required}]}}")
    };
    let base = obj("a: {type: string}", "a");

    let (f, _) = findings(
        &echo(&base),
        &echo(&obj("a: {type: string}, b: {type: string}", "a, b")),
    );
    assert_eq!(
        f,
        [
            "FL1002 breaking op[POST /x]/request[application/json]",
            "FL1002 info op[POST /x]/response[200]/body[application/json]",
        ],
        "a new required property breaks senders only"
    );

    let (f, _) = findings(
        &echo(&base),
        &echo(&obj("a: {type: string}, b: {type: string}", "a")),
    );
    assert_eq!(
        f[0], "FL1002 info op[POST /x]/request[application/json]",
        "optional request properties are compatible"
    );

    let (f, _) = findings(&echo(&base), &echo(&obj("a: {type: string}", "")));
    assert_eq!(
        f,
        [
            "FL1004 info op[POST /x]/request[application/json]",
            "FL1004 breaking op[POST /x]/response[200]/body[application/json]",
        ],
        "a response property that may now be missing breaks readers"
    );
}

#[test]
fn enums_and_nullability() {
    let (f, _) = findings(
        &echo("{type: string, enum: [a, b]}"),
        &echo("{type: string, enum: [a, b, c]}"),
    );
    assert_eq!(
        f,
        [
            "FL1005 info op[POST /x]/request[application/json]",
            "FL1005 risky op[POST /x]/response[200]/body[application/json]",
        ]
    );
    let (f, _) = findings(
        &echo("{type: string}"),
        &echo("{type: string, nullable: true}"),
    );
    assert_eq!(
        f,
        [
            "FL1012 info op[POST /x]/request[application/json]",
            "FL1012 breaking op[POST /x]/response[200]/body[application/json]",
        ]
    );
}

#[test]
fn surfaces() {
    let old = format!(
        "{HEAD}paths:\n  /pets:\n    get:\n      responses: {{'200': {{description: ok}}}}\n    delete:\n      responses: {{'204': {{description: gone}}}}\n"
    );
    let new = format!(
        "{HEAD}security: [{{token: []}}]\npaths:\n  /pets:\n    get:\n      parameters: [{{name: q, in: query, required: true, schema: {{type: string}}}}]\n      responses: {{'200': {{description: ok}}, '429': {{description: slow down}}}}\n"
    );
    let (f, bump) = findings(&old, &new);
    assert_eq!(
        f,
        [
            "FL2001 breaking op[DELETE /pets]",
            "FL2061 breaking op[GET /pets]",
            "FL2060 info op[GET /pets]",
            "FL2010 breaking op[GET /pets]/param[query:q]",
            "FL2040 risky op[GET /pets]/response[429]",
        ]
    );
    assert_eq!(bump, Bump::Major);
}

#[test]
fn callbacks_turn_the_perspective_around() {
    let spec = |required: &str| {
        format!(
            "{HEAD}paths:\n  /subscribe:\n    post:\n      responses: {{'201': {{description: ok}}}}\n      callbacks:\n        onEvent:\n          '{{$request.body#/url}}':\n            post:\n              parameters: [{{name: sig, in: header, required: {required}, schema: {{type: string}}}}]\n              responses: {{'200': {{description: ok}}}}\n"
        )
    };
    // The API sends callback requests; making a header optional means the
    // client may no longer receive it.
    let (f, _) = findings(&spec("true"), &spec("false"));
    assert_eq!(
        f,
        ["FL2012 breaking op[POST /subscribe]/callback[onEvent]/op[POST {}]/param[header:sig]"]
    );
    let (f, _) = findings(&spec("false"), &spec("true"));
    assert_eq!(
        f,
        ["FL2012 info op[POST /subscribe]/callback[onEvent]/op[POST {}]/param[header:sig]"]
    );
}

#[test]
fn unreachable_definitions_are_informational() {
    let spec = |max: u32| {
        format!(
            "{HEAD}paths: {{}}\ncomponents:\n  schemas:\n    Tag: {{type: string, maxLength: {max}}}\n"
        )
    };
    let (f, bump) = findings(&spec(10), &spec(5));
    assert_eq!(f, ["FL1007 info type[Tag]"]);
    assert_eq!(bump, Bump::Patch);
}

#[test]
fn witnesses_explain_breaking_findings() {
    let changes = faultline_diff::diff(
        &api(&echo("{type: integer, minimum: 0}")),
        &api(&echo("{type: integer, minimum: 1}")),
    );
    let finding = classify("api", &changes.changes[0]);
    assert_eq!(finding.severity, Severity::Breaking);
    assert_eq!(finding.message, "minimum changed from 0 to 1");
    assert_eq!(finding.witness().unwrap().to_string(), "0");
}

//! What OpenAPI 3.2 adds: the QUERY method, additionalOperations,
//! querystring parameters and itemSchema.

mod common;

use common::lower;
use faultline_ir::{Api, ApiFormat, ParamLocation, TypeKind, validate};
use faultline_rules::{Severity, classify};

fn api(text: &str) -> Api {
    let outcome = lower(&[("openapi.yaml", text)]);
    assert!(outcome.codes().is_empty(), "{}", outcome.rendered());
    let api = outcome.api.unwrap();
    assert_eq!(validate(&api), []);
    api
}

const SEARCH: &str = r#"openapi: 3.2.0
info: {title: Search, version: 1.0.0}
paths:
  /pets:
    query:
      requestBody:
        required: true
        content:
          application/json:
            schema:
              type: object
              properties: {species: {type: string}}
      responses:
        "200":
          description: Matches, one per line.
          content:
            application/jsonl:
              itemSchema:
                type: object
                required: [name]
                properties:
                  name: {type: string, maxLength: 64}
  /pets/list:
    get:
      parameters:
        - in: querystring
          name: filter
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
                required: [species]
                properties:
                  species: {type: string}
                  limit: {type: integer, maximum: 100}
      responses:
        "200": {description: OK}
"#;

#[test]
fn the_query_method_and_item_schemas() {
    let api = api(SEARCH);
    assert_eq!(api.format, ApiFormat::OpenApi32);
    let keys: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(keys, ["QUERY /pets", "GET /pets/list"]);
    let query = &api.operations[0];
    let stream = query.responses[0].content[0].ty;
    assert!(
        matches!(api.types.get(stream).kind, TypeKind::Array(_)),
        "a stream of items"
    );
}

#[test]
fn querystring_parameters_are_query_parameters() {
    let api = api(SEARCH);
    let list = &api.operations[1];
    let parameters: Vec<(ParamLocation, &str, bool)> = list
        .parameters
        .iter()
        .map(|p| (p.location, p.name.as_str(), p.required))
        .collect();
    assert_eq!(
        parameters,
        [
            (ParamLocation::Query, "species", true),
            (ParamLocation::Query, "limit", false),
        ]
    );
    // The same parameters written separately compare as unchanged.
    let separate = SEARCH.replace(
        r"        - in: querystring
          name: filter
          content:
            application/x-www-form-urlencoded:
              schema:
                type: object
                required: [species]
                properties:
                  species: {type: string}
                  limit: {type: integer, maximum: 100}",
        r"        - {in: query, name: species, required: true, schema: {type: string}}
        - {in: query, name: limit, schema: {type: integer, maximum: 100}}",
    );
    let changes = faultline_diff::diff(&api, &self::api(&separate));
    let serious: Vec<_> = changes
        .iter()
        .map(|c| classify("api", c))
        .filter(|f| f.severity > Severity::Info)
        .collect();
    assert!(
        serious.is_empty(),
        "{:?}",
        serious.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}

#[test]
fn item_changes_are_found() {
    let old = api(SEARCH);
    let new = api(&SEARCH.replace("maxLength: 64", "maxLength: 128"));
    let worst = faultline_diff::diff(&old, &new)
        .iter()
        .map(|c| classify("api", c).severity)
        .max();
    assert_eq!(
        worst,
        Some(Severity::Risky),
        "responses may now carry longer names"
    );
}

#[test]
fn additional_operations() {
    let text = r#"openapi: 3.2.0
info: {title: T, version: 1.0.0}
paths:
  /files/{id}:
    parameters: [{name: id, in: path, required: true, schema: {type: string}}]
    additionalOperations:
      COPY:
        responses: {"201": {description: Copied}}
"#;
    let outcome = lower(&[("openapi.yaml", text)]);
    assert_eq!(outcome.codes(), ["FLE2012"]);
    assert!(!outcome.diagnostics.has_errors(), "a warning");
    assert!(outcome.api.unwrap().operations.is_empty());

    let fixed = text.replace("COPY", "GET");
    let outcome = lower(&[("openapi.yaml", &fixed)]);
    assert_eq!(outcome.codes(), ["FLE2012"]);
    assert!(outcome.diagnostics.has_errors(), "GET has its own field");
}

#[test]
fn query_is_only_a_field_in_3_2() {
    let text = SEARCH.replace("openapi: 3.2.0", "openapi: 3.1.0");
    let outcome = lower(&[("openapi.yaml", &text)]);
    let api = outcome.api.unwrap();
    let keys: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(keys, ["GET /pets/list"]);
}

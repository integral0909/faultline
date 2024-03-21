//! End-to-end comparison of OpenAPI descriptions.

mod common;

use common::lower;
use faultline_diff::{Change, diff};

fn changes(old: &str, new: &str) -> Vec<Change> {
    let (a, b) = (
        lower(&[("openapi.yaml", old)]),
        lower(&[("openapi.yaml", new)]),
    );
    diff(a.clean(), b.clean()).changes
}

fn summary(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|c| format!("{} {} [{}]", c.kind.code(), c.path, c.polarity))
        .collect()
}

const HEAD: &str = "openapi: 3.0.3\ninfo: {title: T, version: '1'}\n";

#[test]
fn renamed_path_parameters_are_not_a_change() {
    let old = format!(
        "{HEAD}paths:\n  /pets/{{id}}:\n    get:\n      parameters: [{{name: id, in: path, required: true, schema: {{type: string}}}}]\n      responses: {{'200': {{description: ok}}}}\n"
    );
    let new = old
        .replace("{id}", "{petId}")
        .replace("name: id", "name: petId");
    // The parameter itself is renamed, which is visible to clients that name
    // it (for example generated SDKs), but the endpoint is the same.
    let c = changes(&old, &new);
    assert_eq!(
        summary(&c),
        [
            "parameter-removed op[GET /pets/{}]/param[path:id] [in]",
            "parameter-added op[GET /pets/{}]/param[path:petId] [in]",
        ]
    );
}

#[test]
fn request_and_response_narrowing_have_opposite_meanings() {
    let spec = |max: u32| {
        format!(
            "{HEAD}paths:\n  /pets:\n    post:\n      requestBody:\n        content: {{application/json: {{schema: {{$ref: '#/components/schemas/Name'}}}}}}\n      responses:\n        '200': {{description: ok, content: {{application/json: {{schema: {{$ref: '#/components/schemas/Name'}}}}}}}}\ncomponents:\n  schemas:\n    Name: {{type: string, maxLength: {max}}}\n"
        )
    };
    let c = changes(&spec(64), &spec(32));
    // One shared named type used in both directions: compared once per
    // direction.
    assert_eq!(
        summary(&c),
        [
            "bound-changed op[POST /pets]/request[application/json] [in]",
            "bound-changed op[POST /pets]/response[200]/body[application/json] [out]",
        ]
    );
    assert!(
        c.iter()
            .all(|x| x.effect.as_ref().unwrap().lost.witness().is_some())
    );
}

#[test]
fn serialization_and_security() {
    let old = format!(
        "{HEAD}paths:\n  /pets:\n    get:\n      parameters: [{{name: tags, in: query, schema: {{type: array, items: {{type: string}}}}}}]\n      responses: {{'200': {{description: ok}}}}\n"
    );
    let new = format!(
        "{HEAD}security: [{{apiKey: []}}]\npaths:\n  /pets:\n    get:\n      parameters: [{{name: tags, in: query, explode: false, schema: {{type: array, items: {{type: string}}}}}}]\n      responses: {{'200': {{description: ok}}}}\n"
    );
    let c = changes(&old, &new);
    assert_eq!(
        summary(&c),
        [
            "security-requirement-removed op[GET /pets] [in]",
            "security-requirement-added op[GET /pets] [in]",
            "parameter-serialization-changed op[GET /pets]/param[query:tags] [in]",
        ]
    );
    // Spelling out a default is not a change.
    let explicit = old.replace("in: query,", "in: query, style: form, explode: true,");
    assert!(changes(&old, &explicit).is_empty());
}

#[test]
fn nullable_and_discriminators() {
    let spec = |nullable: bool, extra: &str| {
        format!(
            "{HEAD}paths: {{}}\ncomponents:\n  schemas:\n    Pet:\n      oneOf: [{{$ref: '#/components/schemas/Cat'}}, {{$ref: '#/components/schemas/Dog'}}{extra}]\n      discriminator: {{propertyName: kind}}\n    Cat: {{type: object, properties: {{name: {{type: string, nullable: {nullable}}}}}}}\n    Dog: {{type: object}}\n    Bird: {{type: object}}\n"
        )
    };
    let c = changes(
        &spec(false, ""),
        &spec(true, ", {$ref: '#/components/schemas/Bird'}"),
    );
    assert_eq!(
        summary(&c),
        [
            "discriminator-value-added type[Pet] [none]",
            "variant-added type[Pet] [none]",
            "variant-added type[Pet]|0.name [none]",
        ]
    );
}

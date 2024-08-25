//! Swagger 2.0: lowering, and migration to OpenAPI 3.

mod common;

use common::lower;
use faultline_diff::{Change, diff};
use faultline_ir::{Api, ApiFormat, ParamLocation, TypeKind};

fn api(text: &str) -> Api {
    lower(&[("swagger.yaml", text)]).clean().clone()
}

fn summary(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .map(|c| format!("{} {}", c.kind.code(), c.path))
        .collect()
}

fn changes(old: &str, new: &str) -> Vec<String> {
    summary(&diff(&api(old), &api(new)).changes)
}

const HEAD: &str = "swagger: '2.0'\ninfo: {title: T, version: '1'}\n";

#[test]
fn body_and_inline_parameters() {
    let text = format!(
        r"{HEAD}consumes: [application/json, application/xml]
produces: [application/json]
paths:
  /pets:
    post:
      operationId: createPet
      parameters:
        - {{name: pet, in: body, required: true, schema: {{$ref: '#/definitions/Pet'}}}}
        - {{name: dryRun, in: query, type: boolean}}
        - {{name: X-Trace, in: header, type: string, pattern: '^[a-f0-9]+$'}}
        - {{name: Accept, in: header, type: string}}
      responses:
        '201':
          description: created
          schema: {{$ref: '#/definitions/Pet'}}
          headers:
            Location: {{type: string, format: uri}}
        default: {{description: error}}
definitions:
  Pet:
    type: object
    properties:
      name: {{type: string}}
      tag: {{type: string, x-nullable: true}}
"
    );
    let api = api(&text);
    assert_eq!(api.format, ApiFormat::Swagger2);
    let op = &api.operations[0];
    assert_eq!(op.key.identity(), "POST /pets");
    let names: Vec<(&str, ParamLocation)> = op
        .parameters
        .iter()
        .map(|p| (p.name.as_str(), p.location))
        .collect();
    assert_eq!(
        names,
        [
            ("dryRun", ParamLocation::Query),
            ("X-Trace", ParamLocation::Header)
        ],
        "Accept is defined by `produces`"
    );
    let body = op.request_body.as_ref().unwrap();
    assert!(body.required);
    let media: Vec<&str> = body.content.iter().map(|c| c.media_type.as_str()).collect();
    assert_eq!(media, ["application/json", "application/xml"]);
    let created = &op.responses[0];
    assert_eq!(created.content[0].media_type, "application/json");
    assert_eq!(created.headers[0].name, "Location");
    assert!(op.responses[1].content.is_empty());

    let pet = api.types.resolve(body.content[0].ty);
    let TypeKind::Object(object) = &api.types.get(pet).kind else {
        panic!()
    };
    let tag = api.types.resolve(object.field("tag").unwrap().ty);
    assert!(
        matches!(&api.types.get(tag).kind, TypeKind::Union(u) if u.variants.len() == 2),
        "x-nullable adds null"
    );
}

#[test]
fn form_data_becomes_an_object_body() {
    let text = format!(
        r"{HEAD}paths:
  /uploads:
    post:
      parameters:
        - {{name: file, in: formData, type: file, required: true}}
        - {{name: note, in: formData, type: string}}
      responses: {{'204': {{description: ok}}}}
  /login:
    post:
      consumes: [application/x-www-form-urlencoded]
      parameters:
        - {{name: user, in: formData, type: string, required: true}}
      responses: {{'204': {{description: ok}}}}
"
    );
    let api = api(&text);
    let upload = api.operations[0].request_body.as_ref().unwrap();
    assert!(upload.required);
    assert_eq!(upload.content[0].media_type, "multipart/form-data");
    let TypeKind::Object(form) = &api.types.get(upload.content[0].ty).kind else {
        panic!()
    };
    let fields: Vec<(&str, bool)> = form
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(fields, [("file", true), ("note", false)]);
    assert!(matches!(
        api.types.get(api.types.resolve(form.fields[0].ty)).kind,
        TypeKind::String(_)
    ));
    let login = api.operations[1].request_body.as_ref().unwrap();
    assert_eq!(
        login.content[0].media_type,
        "application/x-www-form-urlencoded"
    );
}

#[test]
fn collection_formats_become_styles() {
    let style = |format: Option<&str>, location: &str| {
        let format = format.map_or(String::new(), |f| format!(", collectionFormat: {f}"));
        let required = if location == "path" {
            ", required: true"
        } else {
            ""
        };
        let path = if location == "path" { "/a/{ids}" } else { "/a" };
        let text = format!(
            "{HEAD}paths:\n  {path}:\n    get:\n      parameters:\n        - {{name: ids, in: {location}, type: array, items: {{type: string}}{format}{required}}}\n      responses: {{'200': {{description: ok}}}}\n"
        );
        let api = api(&text);
        let s = api.operations[0].parameters[0]
            .serialization
            .clone()
            .unwrap();
        (s.style, s.explode)
    };
    let pair = |style: &str, explode: bool| (style.to_owned(), explode);
    assert_eq!(style(None, "query"), pair("form", false));
    assert_eq!(style(Some("multi"), "query"), pair("form", true));
    assert_eq!(style(Some("ssv"), "query"), pair("spaceDelimited", false));
    assert_eq!(style(Some("pipes"), "query"), pair("pipeDelimited", false));
    assert_eq!(style(None, "path"), pair("simple", false));
    assert_eq!(style(None, "header"), pair("simple", false));
}

#[test]
fn a_migration_to_openapi_3_changes_nothing() {
    let swagger = format!(
        r"{HEAD}produces: [application/json]
paths:
  /pets:
    get:
      operationId: listPets
      parameters:
        - {{name: limit, in: query, type: integer, maximum: 100}}
        - {{name: tags, in: query, type: array, items: {{type: string}}}}
      responses:
        '200':
          description: ok
          schema: {{type: array, items: {{$ref: '#/definitions/Pet'}}}}
          headers:
            X-Next: {{type: string}}
definitions:
  Pet:
    type: object
    required: [name]
    properties:
      name: {{type: string, maxLength: 64}}
"
    );
    let openapi = r"openapi: 3.0.3
info: {title: T, version: '1'}
paths:
  /pets:
    get:
      operationId: listPets
      parameters:
        - {name: limit, in: query, schema: {type: integer, maximum: 100}}
        - {name: tags, in: query, style: form, explode: false, schema: {type: array, items: {type: string}}}
      responses:
        '200':
          description: ok
          headers:
            X-Next: {schema: {type: string}}
          content:
            application/json:
              schema: {type: array, items: {$ref: '#/components/schemas/Pet'}}
components:
  schemas:
    Pet:
      type: object
      required: [name]
      properties:
        name: {type: string, maxLength: 64}
";
    let old = api(&swagger);
    let new = lower(&[("openapi.yaml", openapi)]).clean().clone();
    let found = diff(&old, &new).changes;
    assert!(found.is_empty(), "{:?}", summary(&found));

    // OpenAPI 3's default for arrays in the query explodes them; Swagger's
    // `csv` did not. A migration that drops the style changes the wire.
    let exploded = openapi.replace("style: form, explode: false, ", "");
    let new = lower(&[("openapi.yaml", &exploded)]).clean().clone();
    assert_eq!(
        summary(&diff(&old, &new).changes),
        ["parameter-serialization-changed op[GET /pets]/param[query:tags]"]
    );
}

#[test]
fn swagger_versions_compare_with_each_other() {
    let v1 = format!(
        "{HEAD}paths:\n  /pets:\n    get:\n      parameters:\n        - {{name: limit, in: query, type: integer, maximum: 100}}\n      responses: {{'200': {{description: ok}}}}\n"
    );
    let v2 = v1.replace("maximum: 100", "maximum: 50");
    assert_eq!(
        changes(&v1, &v2),
        ["bound-changed op[GET /pets]/param[query:limit]"]
    );
}

#[test]
fn problems_are_reported() {
    let codes = |text: &str| lower(&[("swagger.yaml", text)]).codes();
    let body_and_form = format!(
        "{HEAD}paths:\n  /a:\n    post:\n      parameters:\n        - {{name: b, in: body, schema: {{type: object}}}}\n        - {{name: f, in: formData, type: string}}\n      responses: {{'200': {{description: ok}}}}\n"
    );
    assert_eq!(codes(&body_and_form), ["FLE2002"]);
    let cookie = format!(
        "{HEAD}paths:\n  /a:\n    get:\n      parameters: [{{name: c, in: cookie, type: string}}]\n      responses: {{'200': {{description: ok}}}}\n"
    );
    assert_eq!(codes(&cookie), ["FLE2007"]);
    let range =
        format!("{HEAD}paths:\n  /a:\n    get:\n      responses: {{'2XX': {{description: ok}}}}\n");
    assert_eq!(codes(&range), ["FLE2006"]);
    let no_schema = format!(
        "{HEAD}paths:\n  /a:\n    post:\n      parameters: [{{name: b, in: body}}]\n      responses: {{'200': {{description: ok}}}}\n"
    );
    assert_eq!(codes(&no_schema), ["FLE2009"]);
    assert_eq!(codes("swagger: '3.0'\n"), ["FLE2010"]);
    let unquoted = lower(&[(
        "swagger.yaml",
        "swagger: 2.0\ninfo: {title: T, version: '1'}\npaths: {}\n",
    )]);
    assert_eq!(unquoted.clean().format, ApiFormat::Swagger2);
}

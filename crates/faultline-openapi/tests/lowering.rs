//! Lowering of OpenAPI descriptions into the IR.

mod common;

use common::lower;
use faultline_ir::{Access, ApiFormat, Operation, ParamLocation, StatusKey, TypeKind};

const PETSTORE: &str = r##"
openapi: 3.0.3
info: {title: Pets, version: 1.2.0}
security: [{apiKey: []}]
paths:
  /pets:
    get:
      operationId: listPets
      parameters:
        - {name: limit, in: query, schema: {type: integer, format: int32, maximum: 100}}
        - {name: tags, in: query, style: pipeDelimited, schema: {type: array, items: {type: string}}}
        - {name: X-Request-ID, in: header, schema: {type: string}}
        - {name: Accept, in: header, schema: {type: string}}
      responses:
        "200":
          description: ok
          headers:
            x-next: {schema: {type: string}}
            Content-Type: {schema: {type: string}}
          content:
            application/json:
              schema: {type: array, items: {$ref: "#/components/schemas/Pet"}}
        default: {$ref: "#/components/responses/Error"}
    post:
      security: []
      requestBody:
        required: true
        content: {Application/JSON: {schema: {$ref: "#/components/schemas/Pet"}}}
      responses: {"201": {description: created}, "4XX": {description: client error}}
  /pets/{petId}:
    parameters: [{name: petId, in: path, required: true, schema: {type: string}}]
    get:
      security: [{oauth: [write, read, read]}, {apiKey: []}]
      responses: {"200": {description: ok}}
components:
  responses:
    Error:
      description: error
      content: {application/json: {schema: {$ref: "#/components/schemas/Error"}}}
  schemas:
    Pet:
      type: object
      required: [name]
      properties:
        id: {type: integer, format: int64, readOnly: true}
        name: {type: string}
        tag: {type: string, nullable: true}
    Error:
      type: object
      properties: {code: {type: integer, exclusiveMinimum: true, minimum: 0}}
    Unused: {type: boolean}
"##;

fn op<'a>(api: &'a faultline_ir::Api, identity: &str) -> &'a Operation {
    api.operations
        .iter()
        .find(|o| o.key.identity() == identity)
        .unwrap_or_else(|| panic!("no operation {identity}"))
}

#[test]
fn petstore_operations_and_surfaces() {
    let out = lower(&[("openapi.yaml", PETSTORE)]);
    let api = out.clean();
    assert_eq!(api.format, ApiFormat::OpenApi30);
    assert_eq!(api.version.as_deref(), Some("1.2.0"));
    let identities: Vec<String> = api.operations.iter().map(|o| o.key.identity()).collect();
    assert_eq!(identities, ["GET /pets", "POST /pets", "GET /pets/{}"]);

    let list = op(api, "GET /pets");
    assert_eq!(list.operation_id.as_deref(), Some("listPets"));
    let names: Vec<&str> = list.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["limit", "tags", "X-Request-ID"],
        "Accept headers are ignored"
    );
    let limit = list.parameters[0].serialization.as_ref().unwrap();
    assert_eq!((limit.style.as_str(), limit.explode), ("form", true));
    let tags = list.parameters[1].serialization.as_ref().unwrap();
    assert_eq!(
        (tags.style.as_str(), tags.explode),
        ("pipeDelimited", false)
    );
    let header = list.parameters[2].serialization.as_ref().unwrap();
    assert_eq!((header.style.as_str(), header.explode), ("simple", false));

    let ok = &list.responses[0];
    assert_eq!(ok.status, StatusKey::Code(200));
    let headers: Vec<&str> = ok.headers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(
        headers,
        ["x-next"],
        "Content-Type response headers are ignored"
    );
    assert_eq!(list.responses[1].status, StatusKey::Default);
    assert_eq!(list.responses[1].content[0].media_type, "application/json");

    let create = op(api, "POST /pets");
    let body = create.request_body.as_ref().unwrap();
    assert!(body.required);
    assert_eq!(
        body.content[0].media_type, "application/json",
        "media types are normalized"
    );
    assert_eq!(create.responses[1].status, StatusKey::Range(4));
    assert_eq!(
        create.security.as_deref(),
        Some(&[][..]),
        "security: [] removes the global requirement"
    );

    let get = op(api, "GET /pets/{}");
    assert_eq!(
        get.parameters[0].location,
        ParamLocation::Path,
        "path-level parameters are inherited"
    );
    let security = get.security.as_ref().unwrap();
    assert_eq!(
        security.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["apiKey", "oauth[read,write]"]
    );
    assert_eq!(
        op(api, "GET /pets").security.as_ref().unwrap()[0].to_string(),
        "apiKey"
    );
}

#[test]
fn schemas_use_the_openapi_30_dialect() {
    let out = lower(&[("openapi.yaml", PETSTORE)]);
    let api = out.clean();
    let pet = api.types.get(api.named_type("Pet").unwrap());
    let TypeKind::Object(pet) = &pet.kind else {
        panic!("{}", out.dump())
    };
    let id = api.types.get(pet.field("id").unwrap().ty);
    assert_eq!(id.meta.access, Access::ReadOnly);
    let tag = &api.types.get(pet.field("tag").unwrap().ty).kind;
    assert!(
        matches!(tag, TypeKind::Union(u) if u.variants.len() == 2),
        "nullable adds null: {}",
        out.dump()
    );
    assert!(
        api.named_type("Unused").is_some(),
        "every component schema is lowered"
    );
    let dump = out.dump();
    assert!(dump.contains("integer exclusiveMin=0"), "{dump}");
}

#[test]
fn openapi_31_webhooks_and_json_schema_2020() {
    let out = lower(&[(
        "openapi.yaml",
        r##"
openapi: 3.1.0
info: {title: Hooks, version: "2"}
webhooks:
  newPet:
    post:
      requestBody:
        content: {application/json: {schema: {type: [string, "null"], const: "x"}}}
      responses: {"200": {description: ok}}
components:
  schemas:
    Choice:
      oneOf: [{$ref: "#/components/schemas/Cat"}, {$ref: "#/components/schemas/Dog"}]
      discriminator: {propertyName: kind, mapping: {kitty: Cat}}
    Cat: {type: object, properties: {kind: {type: string}}}
    Dog: {type: object, properties: {kind: {type: string}}}
"##,
    )]);
    let api = out.clean();
    assert_eq!(api.format, ApiFormat::OpenApi31);
    assert_eq!(api.webhooks.len(), 1);
    let roots = api.roots();
    assert_eq!(
        roots[0].path.to_string(),
        "webhook[newPet]/op[POST newPet]/request[application/json]"
    );
    assert_eq!(
        roots[0].polarity,
        faultline_ir::Polarity::OUT,
        "the API sends webhook requests"
    );

    let choice = api.types.get(api.named_type("Choice").unwrap());
    let TypeKind::Union(u) = &choice.kind else {
        panic!("{}", out.dump())
    };
    let d = u.discriminator.as_ref().unwrap();
    assert_eq!(d.property, "kind");
    let values: Vec<&str> = d.mapping.iter().map(|(v, _)| v.as_str()).collect();
    assert_eq!(
        values,
        ["kitty", "Dog"],
        "explicit mapping plus implicit names for unmapped variants"
    );
}

#[test]
fn callback_expressions_are_not_path_templates() {
    let out = lower(&[(
        "openapi.yaml",
        r"
openapi: 3.0.3
info: {title: T, version: '1'}
paths:
  /subscribe:
    post:
      responses: {'201': {description: ok}}
      callbacks:
        onEvent:
          '{$request.body#/url}':
            post: {responses: {'200': {description: ok}}}
",
    )]);
    let api = out.clean();
    let callback = &api.operations[0].callbacks[0];
    assert_eq!(callback.operations[0].key.identity(), "POST {}");
}

#[test]
fn references_across_files() {
    let out = lower(&[
        (
            "api/openapi.yaml",
            r#"
openapi: 3.0.3
info: {title: T, version: "1"}
paths:
  /pets: {$ref: "paths/pets.yaml"}
"#,
        ),
        (
            "api/paths/pets.yaml",
            r#"
get:
  parameters: [{$ref: "../common.yaml#/parameters/Limit"}]
  responses:
    "200":
      description: ok
      content: {application/json: {schema: {$ref: "../common.yaml#/schemas/Pet"}}}
"#,
        ),
        (
            "api/common.yaml",
            r"
parameters:
  Limit: {name: limit, in: query, schema: {type: integer}}
schemas:
  Pet: {type: object, properties: {name: {type: string}}}
",
        ),
    ]);
    let api = out.clean();
    let get = op(api, "GET /pets");
    assert_eq!(get.parameters[0].name, "limit");
    let body = get.responses[0].content[0].ty;
    assert!(matches!(
        api.types.get(api.types.resolve(body)).kind,
        TypeKind::Object(_)
    ));
}

#[test]
fn diagnostics() {
    let cases: &[(&str, &str)] = &[
        ("swagger: '2.0'\n", "FLE2010"),
        ("info: {title: x}\n", "FLE2001"),
        ("openapi: 2.5.0\n", "FLE2001"),
        (
            "openapi: 3.0.0\npaths:\n  /a/{id}:\n    get: {responses: {'200': {description: ok}}}\n",
            "FLE2005",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a/{id}:\n    get:\n      parameters: [{name: id, in: path, schema: {type: string}}]\n      responses: {}\n",
            "FLE2005",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a:\n    get: {responses: {'700': {description: bad}}}\n",
            "FLE2006",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      parameters: [{name: q, in: body, schema: {}}]\n      responses: {}\n",
            "FLE2007",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a/{x}:\n    get:\n      parameters: [{name: x, in: path, required: true, schema: {}}]\n      responses: {}\n  /a/{y}:\n    get:\n      parameters: [{name: y, in: path, required: true, schema: {}}]\n      responses: {}\n",
            "FLE2004",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      parameters: [{$ref: '#/components/parameters/Nope'}]\n      responses: {}\n",
            "FLE2003",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      parameters: [{$ref: '#/components/parameters/A'}]\n      responses: {}\ncomponents:\n  parameters:\n    A: {$ref: '#/components/parameters/B'}\n    B: {$ref: '#/components/parameters/A'}\n",
            "FLE2008",
        ),
        (
            "openapi: 3.0.0\npaths:\n  /a:\n    get:\n      parameters: [{name: q, in: query}]\n      responses: {}\n",
            "FLE2009",
        ),
        (
            "openapi: 3.0.0\npaths:\n  a:\n    get: {responses: {}}\n",
            "FLE2011",
        ),
    ];
    for (text, code) in cases {
        let out = lower(&[("openapi.yaml", text)]);
        assert!(
            out.codes().contains(code),
            "{text}\nexpected {code}, got:\n{}",
            out.rendered()
        );
    }
}

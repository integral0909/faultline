//! JSON Schemas used as RAML types, included or written inline.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, ObjectType, TypeId, TypeKind, validate};
use faultline_raml::lower_raml;
use faultline_rules::{Severity, classify};
use faultline_source::{Limits, SourceMap};

const PET: &str = r#"{
  "$schema": "http://json-schema.org/draft-04/schema#",
  "type": "object",
  "properties": {
    "id": {"type": "integer"},
    "name": {"type": "string"},
    "owner": {"$ref": "owner.json"}
  },
  "required": ["id"]
}"#;

const OWNER: &str = r#"{"type": "object", "properties": {"email": {"type": "string"}}}"#;

fn lowered(api: &str, pet: &str) -> (Option<Api>, Vec<String>) {
    let loader = MemoryLoader::new()
        .with("file:///api.raml", api)
        .with("file:///schemas/pet.json", pet)
        .with("file:///schemas/owner.json", OWNER);
    let sources = SourceMap::new();
    let lowered = lower_raml(
        &UriRef::parse("file:///api.raml").unwrap(),
        &loader,
        &sources,
        "t",
        Limits::default(),
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

const INCLUDED: &str = "#%RAML 1.0
title: T
types:
  Pet: !include schemas/pet.json
/pets:
  post:
    body: Pet
  get:
    responses:
      200:
        body:
          type: !include schemas/pet.json
";

fn object(api: &Api, ty: TypeId) -> &ObjectType {
    match &api.types.get(api.types.resolve(ty)).kind {
        TypeKind::Object(object) => object,
        other => panic!("{other:?}"),
    }
}

#[test]
fn included_schemas_have_json_schema_semantics() {
    let (api, codes) = lowered(INCLUDED, PET);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    assert_eq!(validate(&api), []);
    let post = &api.operations[1];
    let body = object(&api, post.request_body.as_ref().unwrap().content[0].ty);
    let required: Vec<(&str, bool)> = body
        .fields
        .iter()
        .map(|f| (f.name.as_str(), f.required))
        .collect();
    assert_eq!(
        required,
        [("id", true), ("name", false), ("owner", false)],
        "only `required` properties are required, unlike in RAML"
    );
    // The `$ref` is resolved against the schema's own file.
    let owner = object(&api, body.fields[2].ty);
    assert_eq!(owner.fields[0].name, "email");
    // Both uses of the file are one type.
    let get = &api.operations[0];
    assert_eq!(
        api.types.resolve(get.responses[0].content[0].ty),
        api.types
            .resolve(post.request_body.as_ref().unwrap().content[0].ty)
    );
}

#[test]
fn inline_schemas_are_lowered() {
    let api = "#%RAML 1.0
title: T
types:
  Tag: |
    {\"type\": \"string\", \"maxLength\": 10}
/tags:
  post:
    body:
      application/json:
        type: Tag
";
    let (api, codes) = lowered(api, PET);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    let body = api.operations[0].request_body.as_ref().unwrap().content[0].ty;
    let TypeKind::String(string) = &api.types.get(api.types.resolve(body)).kind else {
        panic!()
    };
    assert_eq!(string.max_length, Some(10));
}

#[test]
fn schema_changes_are_found() {
    let worst = |new_pet: &str| {
        let (old, _) = lowered(INCLUDED, PET);
        let (new, codes) = lowered(INCLUDED, new_pet);
        assert!(codes.is_empty(), "{codes:?}");
        faultline_diff::diff(&old.unwrap(), &new.unwrap())
            .iter()
            .map(|c| classify("t", c).severity)
            .max()
            .unwrap_or(Severity::Info)
    };
    assert_eq!(
        worst(&PET.replace(r#"["id"]"#, r#"["id", "name"]"#)),
        Severity::Breaking,
        "a newly required property breaks requests"
    );
    assert_eq!(worst(PET), Severity::Info);
}

#[test]
fn schema_errors_are_reported() {
    let (api, codes) = lowered(INCLUDED, &PET.replace("owner.json", "missing.json"));
    assert!(api.is_none());
    assert!(codes.iter().all(|c| c.starts_with("FLE")), "{codes:?}");
    assert!(!codes.is_empty());
}

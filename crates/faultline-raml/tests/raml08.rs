//! RAML 0.8, compared as the RAML 1.0 it amounts to.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, ParamLocation, validate};
use faultline_raml::lower_raml;
use faultline_source::{Limits, SourceMap};

fn lowered(files: &[(&str, &str)]) -> (Option<Api>, Vec<String>) {
    let mut loader = MemoryLoader::new();
    for (uri, text) in files {
        loader = loader.with(uri, *text);
    }
    let sources = SourceMap::new();
    let lowered = lower_raml(
        &UriRef::parse(files[0].0).unwrap(),
        &loader,
        &sources,
        "pets",
        Limits::default(),
    );
    let codes = lowered
        .diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (lowered.api, codes)
}

const PET_SCHEMA: &str = r#"{"type": "object", "required": ["name"],
  "properties": {"name": {"type": "string"}, "tag": {"type": "string"}}}"#;

const RAML_08: &str = r"#%RAML 0.8
title: Pets
mediaType: application/json
schemas:
  - pet: !include pet.json
traits:
  - paged:
      queryParameters:
        limit:
          type: integer
          maximum: 100
/pets:
  get:
    is: [paged]
    queryParameters:
      species:
        type: string
        enum: [cat, dog]
      tags:
        type: string
        repeat: true
      since:
        type: date
    headers:
      X-Trace:
        required: true
    responses:
      200:
        body:
          application/json:
            schema: pet
  post:
    body:
      application/json:
        schema: pet
      application/x-www-form-urlencoded:
        formParameters:
          name:
            type: string
            required: true
          tag:
            type: string
  /{petId}:
    uriParameters:
      petId:
        type: integer
    delete:
";

const RAML_10: &str = r"#%RAML 1.0
title: Pets
mediaType: application/json
types:
  pet: !include pet.json
traits:
  paged:
    queryParameters:
      limit?:
        type: integer
        maximum: 100
/pets:
  get:
    is: [paged]
    queryParameters:
      species?:
        enum: [cat, dog]
      tags?:
        type: array
        items: string
      since?: datetime
    headers:
      X-Trace: string
    responses:
      200:
        body:
          application/json:
            type: pet
  post:
    body:
      application/json:
        type: pet
      application/x-www-form-urlencoded:
        properties:
          name: string
          tag?: string
  /{petId}:
    uriParameters:
      petId: integer
    delete:
";

#[test]
fn a_raml_08_api_is_its_raml_10_equivalent() {
    let (old, codes) = lowered(&[
        ("file:///v08/api.raml", RAML_08),
        ("file:///v08/pet.json", PET_SCHEMA),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    let old = old.unwrap();
    assert_eq!(validate(&old), []);
    let (new, codes) = lowered(&[
        ("file:///v10/api.raml", RAML_10),
        ("file:///v10/pet.json", PET_SCHEMA),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    let new = new.unwrap();

    let get = old
        .operations
        .iter()
        .find(|o| o.key.to_string() == "GET /pets")
        .unwrap();
    let parameters: Vec<(ParamLocation, &str, bool)> = get
        .parameters
        .iter()
        .map(|p| (p.location, p.name.as_str(), p.required))
        .collect();
    assert_eq!(
        parameters,
        [
            (ParamLocation::Query, "species", false),
            (ParamLocation::Query, "tags", false),
            (ParamLocation::Query, "since", false),
            (ParamLocation::Query, "limit", false),
            (ParamLocation::Header, "X-Trace", true),
        ],
        "optional unless required, traits applied"
    );
    let changes = faultline_diff::diff(&old, &new);
    assert!(changes.is_empty(), "{:?}", changes.changes);
}

#[test]
fn other_versions_are_refused() {
    let (api, codes) = lowered(&[("file:///api.raml", "#%RAML 0.9\ntitle: T\n")]);
    assert!(api.is_none());
    assert_eq!(codes, ["FLE7602"]);
}

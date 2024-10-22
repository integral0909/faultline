//! Resource types and traits applied to resources and methods.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, Operation, TypeKind};
use faultline_raml::lower_raml;
use faultline_source::{Limits, SourceMap};

fn lowered(text: &str) -> (Option<Api>, Vec<String>) {
    let loader = MemoryLoader::new().with("file:///api.raml", text);
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

const API: &str = r"#%RAML 1.0
title: Pets
types:
  Pet:
    properties:
      name: string
resourceTypes:
  collection:
    description: A collection of <<resourcePathName>>.
    get?:
      is: [paged]
      responses:
        200:
          body: <<item>>[]
    post?:
      body: <<item>>
  member:
    type: { base: { note: <<resourcePathName | !singularize>> } }
    delete?:
      responses:
        204:
  base:
    get?:
      headers:
        X-Note:
          type: string
          default: <<note>>
traits:
  paged:
    queryParameters:
      limit?:
        type: integer
        maximum: 100
      offset?: integer
  traced:
    headers:
      X-Trace?: string
/pets:
  type: { collection: { item: Pet } }
  is: [traced]
  get:
    queryParameters:
      limit?:
        type: integer
        maximum: 50
  /{petId}:
    type: member
    get:
      responses:
        200:
          body: Pet
    delete:
";

fn op<'a>(api: &'a Api, id: &str) -> &'a Operation {
    api.operations
        .iter()
        .find(|o| o.key.to_string() == id)
        .unwrap_or_else(|| panic!("no {id}"))
}

#[test]
fn templates_are_applied() {
    let (api, codes) = lowered(API);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    let ops: Vec<String> = api.operations.iter().map(|o| o.key.to_string()).collect();
    assert_eq!(
        ops,
        ["GET /pets", "GET /pets/{petId}", "DELETE /pets/{petId}"],
        "optional methods of a resource type only fill in methods the resource has"
    );

    let list = op(&api, "GET /pets");
    let names: Vec<&str> = list.parameters.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["limit", "offset", "X-Trace"],
        "own, then the method's traits, then the resource's"
    );
    // The method's own `limit` wins over the trait's.
    let TypeKind::Integer(limit) = &api.types.get(list.parameters[0].ty).kind else {
        panic!()
    };
    assert_eq!(limit.maximum.as_ref().unwrap().value.to_string(), "50");
    // `<<item>>[]` became `Pet[]`.
    let body = api.types.get(list.responses[0].content[0].ty);
    assert!(matches!(body.kind, TypeKind::Array(_)));

    // A resource type naming another, with a transformed parameter.
    let get = op(&api, "GET /pets/{petId}");
    let note = get.parameters.iter().find(|p| p.name == "X-Note").unwrap();
    assert_eq!(note.meta.default, None, "parameter defaults are not kept");
    let delete = op(&api, "DELETE /pets/{petId}");
    assert_eq!(delete.responses[0].status.to_string(), "204");
}

#[test]
fn unknown_templates_are_reported() {
    let (_, codes) = lowered("#%RAML 1.0\ntitle: T\n/a:\n  type: missing\n  get:\n");
    assert_eq!(codes, ["FLE7605"]);
    let (_, codes) = lowered("#%RAML 1.0\ntitle: T\n/a:\n  get:\n    is: [missing]\n");
    assert_eq!(codes, ["FLE7605"]);
    let looping = "#%RAML 1.0\ntitle: T\nresourceTypes:\n  a:\n    type: b\n  b:\n    type: a\n/x:\n  type: a\n  get:\n";
    assert_eq!(lowered(looping).1, ["FLE7605"]);
}

//! Libraries used with `uses:`: qualified types, traits and resource types.

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::{Api, TypeKind};
use faultline_raml::lower_raml;
use faultline_source::{Limits, SourceMap};

const COMMON: &str = r"#%RAML 1.0 Library
types:
  Pet:
    properties:
      name: string
      tag?: Tag
  Tag: string
traits:
  paged:
    queryParameters:
      limit?: integer
resourceTypes:
  collection:
    get?:
      is: [paged]
      responses:
        200:
          body: <<item>>[]
";

fn lowered(api: &str, files: &[(&str, &str)]) -> (Option<Api>, Vec<String>) {
    let mut loader = MemoryLoader::new().with("file:///api.raml", api);
    for (uri, text) in files {
        loader = loader.with(uri, *text);
    }
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

#[test]
fn library_declarations_are_qualified() {
    let api = "#%RAML 1.0\ntitle: T\nuses:\n  common: lib/common.raml\n/pets:\n  type: { common.collection: { item: common.Pet } }\n  get:\n    is: [common.paged]\n";
    let (api, codes) = lowered(api, &[("file:///lib/common.raml", COMMON)]);
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    let get = &api.operations[0];
    assert_eq!(get.key.to_string(), "GET /pets");
    assert_eq!(get.parameters[0].name, "limit");
    let body = api
        .types
        .get(api.types.resolve(get.responses[0].content[0].ty));
    let TypeKind::Array(array) = &body.kind else {
        panic!("{:?}", body.kind)
    };
    let item = api.types.get(array.items);
    assert_eq!(
        item.name.as_ref().map(|n| n.0.as_str()),
        Some("common.Pet"),
        "a library's types are named with the library's name"
    );
}

#[test]
fn library_templates_name_their_own_templates() {
    // `collection` names `paged` unqualified: the library's trait.
    let api = "#%RAML 1.0\ntitle: T\nuses:\n  common: common.raml\ntraits:\n  paged:\n    headers:\n      X-Wrong: string\n/pets:\n  type: { common.collection: { item: common.Tag } }\n  get:\n";
    let (api, codes) = lowered(api, &[("file:///common.raml", COMMON)]);
    assert!(codes.is_empty(), "{codes:?}");
    let names: Vec<String> = api.unwrap().operations[0]
        .parameters
        .iter()
        .map(|p| p.name.clone())
        .collect();
    assert_eq!(names, ["limit"]);
}

#[test]
fn operations_point_at_their_methods() {
    let api = "#%RAML 1.0\ntitle: T\n/pets:\n  delete:\n";
    let (api, _) = lowered(api, &[]);
    let origin = api.unwrap().operations[0].origin.unwrap();
    assert_eq!((origin.start(), origin.end()), (29, 35), "the `delete` key");
}

#[test]
fn libraries_use_libraries() {
    let api = "#%RAML 1.0\ntitle: T\nuses:\n  outer: outer.raml\n/pets:\n  get:\n    responses:\n      200:\n        body: outer.inner.Pet\n";
    let outer = "#%RAML 1.0 Library\nuses:\n  inner: common.raml\n";
    let (api, codes) = lowered(
        api,
        &[
            ("file:///outer.raml", outer),
            ("file:///common.raml", COMMON),
        ],
    );
    assert!(codes.is_empty(), "{codes:?}");
    let api = api.unwrap();
    let body = api.types.get(api.operations[0].responses[0].content[0].ty);
    assert_eq!(
        body.name.as_ref().map(|n| n.0.as_str()),
        Some("outer.inner.Pet")
    );
}

#[test]
fn bad_libraries_are_reported() {
    let api = "#%RAML 1.0\ntitle: T\nuses:\n  lib: missing.raml\n";
    let (api, codes) = lowered(api, &[]);
    assert!(api.is_none());
    assert_eq!(codes, ["FLE7603"]);

    let api = "#%RAML 1.0\ntitle: T\nuses:\n  lib: other.raml\n";
    let (_, codes) = lowered(api, &[("file:///other.raml", "#%RAML 1.0\ntitle: O\n")]);
    assert_eq!(codes, ["FLE7605"], "an API is not a library");

    let looping = "#%RAML 1.0 Library\nuses:\n  me: self.raml\n";
    let api = "#%RAML 1.0\ntitle: T\nuses:\n  lib: self.raml\n";
    let (_, codes) = lowered(api, &[("file:///self.raml", looping)]);
    assert!(!codes.is_empty(), "libraries that use themselves end");
}

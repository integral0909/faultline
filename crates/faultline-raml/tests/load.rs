//! Loading RAML documents and expanding includes.

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{Kind, UriRef};
use faultline_raml::RamlLoader;
use faultline_source::{Limits, SourceMap};

fn load(files: &[(&str, &str)]) -> (Option<faultline_raml::Document>, Vec<String>, SourceMap) {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let sources = SourceMap::new();
    let (document, codes) = {
        let mut raml = RamlLoader::new(&loader, &sources, Limits::default());
        let document = raml.document(
            &UriRef::parse(&format!("file:///{}", files[0].0)).unwrap(),
            None,
        );
        let codes = raml
            .diagnostics
            .iter()
            .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
            .collect();
        (document, codes)
    };
    (document, codes, sources)
}

#[test]
fn includes_are_expanded_with_their_own_spans() {
    let (document, codes, sources) = load(&[
        (
            "api.raml",
            "#%RAML 1.0\ntitle: Pets\ntypes:\n  Pet: !include types/pet.raml\n/pets:\n  get:\n    description: !include docs/list.md\n    responses:\n      200:\n        body:\n          application/json:\n            example: !include examples/pets.json\n",
        ),
        (
            "types/pet.raml",
            "#%RAML 1.0 DataType\ntype: object\nproperties:\n  name: string\n  owner: !include owner.raml\n",
        ),
        ("types/owner.raml", "#%RAML 1.0 DataType\ntype: string\n"),
        ("docs/list.md", "Lists **pets**."),
        ("examples/pets.json", r#"[{"name": "Rex"}]"#),
    ]);
    assert!(codes.is_empty(), "{codes:?}");
    let document = document.unwrap();
    assert_eq!(document.kind, "");
    let root = &document.root;
    let pet = root.get("types").unwrap().get("Pet").unwrap();
    assert_eq!(pet.get("type").unwrap().as_str(), Some("object"));
    let owner = pet.get("properties").unwrap().get("owner").unwrap();
    assert_eq!(
        owner.get("type").unwrap().as_str(),
        Some("string"),
        "nested includes"
    );
    assert_eq!(
        sources.get(owner.span.source()).unwrap().name().to_string(),
        "types/owner.raml",
        "included nodes keep their file's spans"
    );
    let get = root.get("/pets").unwrap().get("get").unwrap();
    assert_eq!(
        get.get("description").unwrap().as_str(),
        Some("Lists **pets**.")
    );
    let example = get
        .get("responses")
        .unwrap()
        .get("200")
        .unwrap()
        .get("body")
        .unwrap()
        .get("application/json")
        .unwrap()
        .get("example")
        .unwrap();
    assert_eq!(example.kind(), Kind::Array, "JSON includes are parsed");
}

#[test]
fn headers_name_the_document_kind() {
    let (document, codes, _) = load(&[("lib.raml", "#%RAML 1.0 Library\ntypes: {}\n")]);
    assert!(codes.is_empty());
    assert_eq!(document.unwrap().kind, "Library");
}

#[test]
fn problems_are_reported() {
    let codes = |files: &[(&str, &str)]| load(files).1;
    assert_eq!(codes(&[("a.raml", "title: Pets\n")]), ["FLE7601"]);
    assert_eq!(
        codes(&[("a.raml", "#%RAML 0.8\ntitle: Pets\n")]),
        ["FLE7602"]
    );
    assert_eq!(
        codes(&[("a.raml", "#%RAML 1.0\ntypes:\n  A: !include missing.raml\n")]),
        ["FLE7603"]
    );
    assert_eq!(
        codes(&[(
            "a.raml",
            "#%RAML 1.0\ntypes:\n  A: !include ../../secret.raml\n"
        )]),
        ["FLE7603"]
    );
    assert_eq!(
        codes(&[
            ("a.raml", "#%RAML 1.0\ntypes:\n  A: !include b.raml\n"),
            (
                "b.raml",
                "#%RAML 1.0 DataType\nproperties:\n  x: !include a.raml\n"
            ),
        ]),
        ["FLE7603"],
        "an include cycle"
    );
    assert_eq!(
        codes(&[("a.raml", "#%RAML 1.0\ntypes:\n  A: !unknown x\n")]),
        ["FLE0205"]
    );
}

//! Reading overlays and applying their actions.

use faultline_doc::{Format, Node};
use faultline_overlay::{apply, read};
use faultline_source::{Diagnostics, Limits, SourceMap, SourceName};

fn yaml(sources: &SourceMap, name: &str, text: &str) -> Node {
    let file = sources
        .add(SourceName::Virtual(name.into()), text.to_owned())
        .unwrap();
    faultline_doc::parse(&file, Format::Yaml, &Limits::default())
        .document
        .unwrap()
        .root
}

const API: &str = "openapi: 3.1.0
info: {title: Pets, version: 1.0.0}
tags: [{name: pets}]
paths:
  /pets:
    get:
      summary: List pets
      tags: [pets]
    post:
      summary: Add a pet
      x-internal: true
  /admin:
    delete:
      x-internal: true
";

/// The API after applying `overlay`, as YAML-ish text of what matters,
/// and the diagnostic codes.
fn applied(overlay: &str) -> (Node, Vec<String>) {
    let sources = SourceMap::new();
    let mut document = yaml(&sources, "api.yaml", API);
    let overlay_root = yaml(&sources, "overlay.yaml", overlay);
    let mut diagnostics = Diagnostics::new();
    if let Some(overlay) = read(&overlay_root, &mut diagnostics) {
        apply(&overlay, &mut document, &mut diagnostics);
    }
    let codes = diagnostics
        .iter()
        .map(|d| d.code.map(|c| c.to_string()).unwrap_or_default())
        .collect();
    (document, codes)
}

fn keys(node: &Node) -> Vec<String> {
    node.as_object()
        .unwrap()
        .keys()
        .map(str::to_owned)
        .collect()
}

#[test]
fn internal_operations_are_removed() {
    let (api, codes) = applied(
        "overlay: 1.0.0
info: {title: Public API, version: 1.0.0}
actions:
  - target: $.paths.*[?@['x-internal'] == true]
    remove: true
  - target: $.paths[?length(@) == 0]
    remove: true
",
    );
    assert!(codes.is_empty(), "{codes:?}");
    assert_eq!(
        keys(api.get("paths").unwrap()),
        ["/pets"],
        "paths left empty are removed next"
    );
    assert_eq!(
        keys(api.get("paths").unwrap().get("/pets").unwrap()),
        ["get"]
    );
}

#[test]
fn updates_merge_objects_and_append_to_arrays() {
    let (api, codes) = applied(
        "overlay: 1.0.0
info: {title: Docs, version: 1.0.0}
actions:
  - target: $.info
    update: {title: Pet Store, contact: {name: Team}}
  - target: $.paths['/pets'].get
    update: {parameters: [{name: limit, in: query}], summary: List all pets}
  - target: $.tags
    update: {name: admin}
",
    );
    assert!(codes.is_empty(), "{codes:?}");
    let info = api.get("info").unwrap();
    assert_eq!(info.get("title").unwrap().as_str(), Some("Pet Store"));
    assert_eq!(info.get("version").unwrap().as_str(), Some("1.0.0"), "kept");
    assert_eq!(keys(info), ["title", "version", "contact"]);
    let get = api
        .get("paths")
        .unwrap()
        .get("/pets")
        .unwrap()
        .get("get")
        .unwrap();
    assert_eq!(get.get("summary").unwrap().as_str(), Some("List all pets"));
    assert_eq!(get.get("parameters").unwrap().as_array().unwrap().len(), 1);
    assert_eq!(api.get("tags").unwrap().as_array().unwrap().len(), 2);
}

#[test]
fn removing_several_items_of_an_array() {
    let (api, codes) = applied(
        "overlay: 1.0.0
info: {title: T, version: 1.0.0}
actions:
  - target: $.tags
    update: {name: b}
  - target: $.tags
    update: {name: c}
  - target: $.tags[?@.name != 'b']
    remove: true
",
    );
    assert!(codes.is_empty(), "{codes:?}");
    let tags = api.get("tags").unwrap().as_array().unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0].get("name").unwrap().as_str(), Some("b"));
}

#[test]
fn problems_are_reported() {
    let cases: &[(&str, &[&str])] = &[
        ("openapi: 3.1.0\n", &["FLE1201"]),
        ("overlay: 2.0.0\nactions: []\n", &["FLE1201"]),
        ("overlay: 1.0.0\n", &["FLE1201"]),
        ("overlay: 1.0.0\nactions: [{update: {}}]\n", &["FLE1201"]),
        (
            "overlay: 1.0.0\nactions: [{target: $.info}]\n",
            &["FLE1201"],
        ),
        (
            "overlay: 1.0.0\nactions: [{target: 'paths', remove: true}]\n",
            &["FLE1202"],
        ),
        (
            "overlay: 1.0.0\nactions: [{target: $.nothing, remove: true}]\n",
            &["FLE1203"],
        ),
        (
            "overlay: 1.0.0\nactions: [{target: $.info.title, update: {a: 1}}]\n",
            &["FLE1201"],
        ),
        (
            "overlay: 1.0.0\nactions: [{target: $.info, update: 1}]\n",
            &["FLE1201"],
        ),
        (
            "overlay: 1.0.0\nactions: [{target: $, remove: true}]\n",
            &["FLE1201"],
        ),
    ];
    for (overlay, expected) in cases {
        let (_, codes) = applied(overlay);
        assert_eq!(&codes, expected, "{overlay}");
    }
}

#[test]
fn updates_keep_their_spans() {
    let sources = SourceMap::new();
    let mut document = yaml(&sources, "api.yaml", API);
    let overlay = yaml(
        &sources,
        "overlay.yaml",
        "overlay: 1.0.0\nactions:\n  - target: $.info\n    update: {summary: Added}\n",
    );
    let mut diagnostics = Diagnostics::new();
    apply(
        &read(&overlay, &mut diagnostics).unwrap(),
        &mut document,
        &mut diagnostics,
    );
    let summary = document.get("info").unwrap().get("summary").unwrap();
    assert_eq!(summary.span.source(), overlay.span.source());
}

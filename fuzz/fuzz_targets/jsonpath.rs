//! Parses arbitrary text as a JSONPath query and evaluates it against a
//! fixed document: neither may panic, and every selected node must be at
//! the location reported for it.
#![no_main]

use std::sync::OnceLock;

use faultline_doc::{Format, Node, Value};
use faultline_jsonpath::{Location, Step, parse};
use faultline_source::{Limits, SourceMap, SourceName};
use libfuzzer_sys::fuzz_target;

const DOCUMENT: &str = r#"{"store": {"book": [
  {"category": "reference", "author": "Nigel Rees", "price": 8.95, "tags": []},
  {"category": "fiction", "author": "Evelyn Waugh", "price": 12.99, "tags": ["war", "it's"]},
  {"category": "fiction", "isbn": "0-553-21311-3", "price": 8.99, "nested": {"a": [1, [2, {"b": null}]]}}
], "bicycle": {"color": "red", "price": 399, "unicode": "é☺", "flag": true}}}"#;

fn document() -> &'static Node {
    static ROOT: OnceLock<Node> = OnceLock::new();
    ROOT.get_or_init(|| {
        let sources = SourceMap::new();
        let file = sources
            .add(SourceName::Virtual("document.json".into()), DOCUMENT.to_owned())
            .unwrap();
        faultline_doc::parse(&file, Format::Json, &Limits::default())
            .document
            .unwrap()
            .root
    })
}

fn at<'n>(root: &'n Node, location: &Location) -> Option<&'n Node> {
    let mut node = root;
    for step in &location.0 {
        node = match (step, &node.value) {
            (Step::Name(name), Value::Object(object)) => object.get(name)?,
            (Step::Index(i), Value::Array(items)) => items.get(*i)?,
            _ => return None,
        };
    }
    Some(node)
}

fuzz_target!(|text: &str| {
    let Ok(query) = parse(text) else {
        return;
    };
    let root = document();
    for selected in query.select(root) {
        let found = at(root, &selected.location).expect("the location exists");
        assert!(std::ptr::eq(found, selected.node), "{} is elsewhere", selected.location);
    }
});

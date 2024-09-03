//! Reads arbitrary text as XML: the reader must never panic, every span it
//! reports must lie inside the text on character boundaries, and a
//! document it accepts must have resolved every namespace prefix.
#![no_main]

use faultline_source::{Diagnostics, Limits, SourceMap, SourceName};
use faultline_xml::{Content, Element, parse};
use libfuzzer_sys::fuzz_target;

fn check(element: &Element, map: &SourceMap) {
    assert!(map.slice(element.span).is_some());
    assert!(map.slice(element.name_span).is_some());
    for attribute in &element.attributes {
        assert!(map.slice(attribute.span).is_some());
        assert!(map.slice(attribute.value_span).is_some());
    }
    if element.raw_name.contains(':') && !element.raw_name.starts_with(':') && !element.raw_name.ends_with(':') {
        assert!(element.name.namespace.is_some(), "prefixed names are resolved");
    }
    for child in &element.children {
        match child {
            Content::Element(e) => check(e, map),
            Content::Text(_, span) => assert!(map.slice(*span).is_some()),
        }
    }
}

fuzz_target!(|text: &str| {
    let map = SourceMap::new();
    let Ok(file) = map.add(SourceName::Virtual("fuzz".into()), text) else {
        return;
    };
    let limits = Limits {
        max_depth: 64,
        max_document_nodes: 20_000,
        ..Limits::default()
    };
    let mut diagnostics = Diagnostics::new();
    let document = parse(text, file.id(), &limits, &mut diagnostics);
    for diagnostic in &diagnostics {
        for label in &diagnostic.labels {
            assert!(map.slice(label.span).is_some(), "diagnostic span out of bounds");
        }
    }
    match document {
        Some(document) => {
            assert!(diagnostics.is_empty());
            check(&document.root, &map);
        }
        None => assert!(diagnostics.has_errors()),
    }
});

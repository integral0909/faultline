//! Loads arbitrary bytes as YAML with tight limits and checks that loading
//! never panics, never exceeds the node limit, and reports errors whenever it
//! gives up.
#![no_main]

use faultline_doc::{Format, parse};
use faultline_source::{Limits, SourceMap, SourceName, decode_text};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = decode_text(data) else { return };
    let limits = Limits {
        max_document_nodes: 50_000,
        max_depth: 64,
        ..Limits::default()
    };
    let map = SourceMap::new();
    let file = map.add(SourceName::Virtual("fuzz".into()), text).unwrap();
    let parsed = parse(&file, Format::Yaml, &limits);

    for diagnostic in &parsed.diagnostics {
        for label in &diagnostic.labels {
            assert!(file.slice(label.span).is_some(), "span out of bounds or not on a char boundary");
        }
    }
    match parsed.document {
        Some(document) => assert!(document.root.count_nodes() <= limits.max_document_nodes),
        None => assert!(parsed.diagnostics.has_errors()),
    }
});

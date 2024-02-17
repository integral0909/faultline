//! Parses arbitrary bytes as JSON and checks the reader's invariants:
//! it never panics, spans stay on character boundaries, a missing document
//! always comes with an error, and valid documents survive a round trip
//! through the serializer.
#![no_main]

use faultline_doc::{Format, parse};
use faultline_source::{Limits, SourceMap, SourceName, decode_text};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = decode_text(data) else { return };
    let map = SourceMap::new();
    let file = map.add(SourceName::Virtual("fuzz".into()), text).unwrap();
    let parsed = parse(&file, Format::Json, &Limits::default());

    for diagnostic in &parsed.diagnostics {
        for label in &diagnostic.labels {
            assert!(file.slice(label.span).is_some(), "span out of bounds or not on a char boundary");
        }
    }
    let Some(document) = parsed.document else {
        assert!(parsed.diagnostics.has_errors());
        return;
    };
    if parsed.diagnostics.has_errors() {
        return; // duplicate keys: the tree is not a faithful copy of the text
    }

    let serialized = document.root.to_json();
    let again = map.add(SourceName::Virtual("round-trip".into()), serialized.as_str()).unwrap();
    let reparsed = parse(&again, Format::Json, &Limits::default());
    let reparsed = reparsed.document.expect("serializer output must parse");
    assert!(reparsed.root.same_value(&document.root), "round trip changed the value: {serialized}");
});

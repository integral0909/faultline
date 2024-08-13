//! Lowers arbitrary text as an AsyncAPI description and diffs the result
//! against itself: lowering must never panic, an error-free result must
//! satisfy the IR invariants, and a description never differs from itself.
#![no_main]

use faultline_asyncapi::lower_asyncapi;
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_source::{Limits, SourceMap};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new().with("file:///asyncapi.yaml", text);
    let limits = Limits {
        max_document_nodes: 20_000,
        max_depth: 64,
        ..Limits::default()
    };
    let lower = || {
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, limits.clone());
        lower_asyncapi(&UriRef::parse("file:///asyncapi.yaml").unwrap(), &mut store, "fuzz")
    };
    let lowered = lower();
    if lowered.diagnostics.has_errors() {
        return;
    }
    let Some(api) = lowered.api else {
        return;
    };
    assert!(faultline_ir::validate(&api).is_empty());
    let second = lower().api.expect("lowering is deterministic");
    let changes = faultline_diff::diff(&api, &second);
    assert!(changes.is_empty(), "a description differs from itself: {:?}", changes.changes);
});

//! Lowers arbitrary text as a GraphQL schema, then diffs the result against
//! itself: parsing and validation must never panic, spans must stay inside
//! the document, an error-free result must satisfy the IR invariants, and a
//! schema never differs from itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_graphql::lower_graphql;
use faultline_source::SourceMap;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new().with("file:///schema.graphql", text);
    let entry = UriRef::parse("file:///schema.graphql").unwrap();
    let sources = SourceMap::new();
    let first = lower_graphql(&entry, &loader, &sources, "fuzz");
    for diagnostic in &first.diagnostics {
        for label in &diagnostic.labels {
            assert!(
                sources.slice(label.span).is_some(),
                "span out of bounds or not on a char boundary"
            );
        }
    }
    let Some(api) = first.api else {
        assert!(first.diagnostics.has_errors());
        return;
    };
    assert!(faultline_ir::validate(&api).is_empty());
    let second = lower_graphql(&entry, &loader, &SourceMap::new(), "fuzz")
        .api
        .expect("lowering is deterministic");
    let changes = faultline_diff::diff(&api, &second);
    assert!(changes.is_empty(), "a schema differs from itself: {:?}", changes.changes);
});

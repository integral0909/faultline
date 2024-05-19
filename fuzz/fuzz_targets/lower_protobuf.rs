//! Lowers arbitrary text as a `.proto` file, then diffs the result against
//! itself: parsing and linking must never panic, spans must stay inside the
//! file, an error-free result must satisfy the IR invariants, and a file
//! never differs from itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::Role;
use faultline_protobuf::lower_protobuf;
use faultline_source::SourceMap;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    // A second file lets the input exercise imports.
    let loader = MemoryLoader::new()
        .with("file:///api.proto", text)
        .with(
            "file:///common.proto",
            "syntax = \"proto3\"; package common; message Id { string value = 1; }",
        );
    let lower = |sources: &SourceMap| {
        lower_protobuf(
            &UriRef::parse("file:///api.proto").unwrap(),
            &loader,
            sources,
            "fuzz",
            Role::Both,
        )
    };
    let sources = SourceMap::new();
    let first = lower(&sources);
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
    let second = lower(&SourceMap::new()).api.expect("lowering is deterministic");
    let changes = faultline_diff::diff(&api, &second);
    assert!(changes.is_empty(), "a file differs from itself: {:?}", changes.changes);
});

//! Lowers arbitrary text as an Avro schema, then diffs the result against
//! itself in every role: lowering must never panic, an error-free result
//! must satisfy the IR invariants, and a schema never differs from itself.
#![no_main]

use faultline_avro::lower_avro;
use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::Role;
use faultline_source::{Limits, SourceMap};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new().with("file:///schema.avsc", text);
    let limits = Limits {
        max_document_nodes: 20_000,
        max_depth: 64,
        ..Limits::default()
    };
    let lower = |role| {
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, limits.clone());
        lower_avro(&UriRef::parse("file:///schema.avsc").unwrap(), &mut store, "fuzz", role)
    };
    for role in [Role::Input, Role::Output, Role::Both] {
        let Some(api) = lower(role).api else {
            return;
        };
        assert!(faultline_ir::validate(&api).is_empty());
        let second = lower(role).api.expect("lowering is deterministic");
        let changes = faultline_diff::diff(&api, &second);
        assert!(changes.is_empty(), "a schema differs from itself: {:?}", changes.changes);
    }
});

//! Lowers arbitrary text as a JSON Type Definition, then diffs the result
//! against itself: checking and lowering must never panic, an error-free
//! result must satisfy the IR invariants, and a schema never differs from
//! itself.
#![no_main]

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::Role;
use faultline_jtd::lower_jtd;
use faultline_source::{Limits, SourceMap};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new().with("file:///schema.jtd.json", text);
    let limits = Limits {
        max_depth: 64,
        max_document_nodes: 20_000,
        ..Limits::default()
    };
    let lower = || {
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, limits.clone());
        lower_jtd(
            &UriRef::parse("file:///schema.jtd.json").unwrap(),
            &mut store,
            "fuzz",
            Role::Both,
        )
    };
    let Some(api) = lower().api else {
        return;
    };
    assert!(faultline_ir::validate(&api).is_empty());
    let second = lower().api.expect("lowering is deterministic");
    let changes = faultline_diff::diff(&api, &second);
    assert!(changes.is_empty(), "a schema differs from itself: {:?}", changes.changes);
});

//! Lowers arbitrary JSON as a schema document (with a second document it may
//! reference) and checks that lowering never panics and that any API it
//! produces without errors satisfies the IR invariants.
//!
//! The input is split at the first NUL byte into the entry document and a
//! document it may reference as `other.json`, so real schemas work as seeds
//! (see `fuzz/seeds/lower_jsonschema`).
#![no_main]

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::Role;
use faultline_jsonschema::{Options, lower_document};
use faultline_source::{Limits, SourceMap};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let (entry, other) = match data.iter().position(|&b| b == 0) {
        Some(i) => (&data[..i], &data[i + 1..]),
        None => (data, &[][..]),
    };
    let (Ok(entry), Ok(other)) = (std::str::from_utf8(entry), std::str::from_utf8(other)) else {
        return;
    };
    let loader = MemoryLoader::new()
        .with("file:///entry.json", entry)
        .with("file:///other.json", other);
    let sources = SourceMap::new();
    let limits = Limits {
        max_document_nodes: 20_000,
        max_depth: 64,
        ..Limits::default()
    };
    let mut store = DocumentStore::new(&loader, &sources, limits);
    let lowered = lower_document(
        &UriRef::parse("file:///entry.json").unwrap(),
        &mut store,
        Options::default(),
        Role::Both,
    );
    if let Some(api) = lowered.api
        && !lowered.diagnostics.has_errors()
    {
        assert!(faultline_ir::validate(&api).is_empty());
        let _ = faultline_ir::dump(&api);
    }
});

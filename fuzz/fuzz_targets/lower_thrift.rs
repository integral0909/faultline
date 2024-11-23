//! Lowers arbitrary text as a Thrift file, then diffs the result against
//! itself: parsing, linking and lowering must never panic, an error-free
//! result must satisfy the IR invariants, and a file never differs from
//! itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::Role;
use faultline_source::SourceMap;
use faultline_thrift::lower_thrift;
use libfuzzer_sys::fuzz_target;

/// An included file exercises qualified names and inherited services.
const SHARED: &str = "typedef i64 Id
enum Kind { A = 1, B }
exception Oops { 1: string message }
struct Tag { 1: required string name, 2: optional list<Id> ids }
service Base { string version() throws (1: Oops oops) }
";

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new()
        .with("file:///api.thrift", text)
        .with("file:///shared.thrift", SHARED);
    let lower = || {
        let sources = SourceMap::new();
        lower_thrift(
            &UriRef::parse("file:///api.thrift").unwrap(),
            &loader,
            &sources,
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
    assert!(changes.is_empty(), "a file differs from itself: {:?}", changes.changes);
});

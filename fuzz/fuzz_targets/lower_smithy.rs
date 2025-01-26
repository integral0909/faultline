//! Lowers arbitrary text as a Smithy model file, beside a fixed second
//! file, then diffs the result against itself: parsing, resolution and
//! lowering must never panic, an error-free result must satisfy the IR
//! invariants, and a model never differs from itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::Role;
use faultline_smithy::lower_smithy;
use faultline_source::SourceMap;
use libfuzzer_sys::fuzz_target;

/// Shapes the fuzzed file can use, mix in and bind.
const SHARED: &str = r#"$version: "2"
namespace fuzz
@mixin
structure Audited { changedBy: String }
@error("client")
@httpError(404)
structure NotFound { @required id: String }
resource Thing { identifiers: { id: String } properties: { name: String } }
enum Color { RED = "red", GREEN }
"#;

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new()
        .with("file:///model/main.smithy", text)
        .with("file:///model/shared.smithy", SHARED);
    let lower = || {
        let sources = SourceMap::new();
        lower_smithy(
            &UriRef::parse("file:///model/main.smithy").unwrap(),
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
    assert!(changes.is_empty(), "a model differs from itself: {:?}", changes.changes);
});

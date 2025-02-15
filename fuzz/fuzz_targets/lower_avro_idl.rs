//! Lowers arbitrary text as an Avro IDL file, beside a fixed file it can
//! import, then diffs the result against itself: reading and lowering must
//! never panic, an error-free result must satisfy the IR invariants, and a
//! protocol never differs from itself.
#![no_main]

use faultline_avro::lower_avro_idl;
use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::Role;
use faultline_source::SourceMap;
use libfuzzer_sys::fuzz_target;

const COMMON: &str = r#"@namespace("common")
protocol Common {
  enum Color { RED, GREEN } = RED;
  record Money { long units; string currency = "EUR"; }
}
"#;

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new()
        .with("file:///idl/main.avdl", text)
        .with("file:///idl/common.avdl", COMMON);
    let lower = || {
        let sources = SourceMap::new();
        lower_avro_idl(
            &UriRef::parse("file:///idl/main.avdl").unwrap(),
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
    assert!(changes.is_empty(), "a protocol differs from itself: {:?}", changes.changes);
});

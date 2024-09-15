//! Lowers arbitrary text as an XML Schema, then diffs the result against
//! itself: reading and lowering must never panic, an error-free result must
//! satisfy the IR invariants, and a schema never differs from itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_ir::Role;
use faultline_source::{Limits, SourceMap};
use faultline_xsd::lower_xsd;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    // A second schema lets the input exercise includes and imports.
    let loader = MemoryLoader::new().with("file:///schema.xsd", text).with(
        "file:///common.xsd",
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema"><xs:simpleType name="Id"><xs:restriction base="xs:string"><xs:maxLength value="8"/></xs:restriction></xs:simpleType></xs:schema>"#,
    );
    let limits = Limits {
        max_depth: 64,
        max_document_nodes: 20_000,
        ..Limits::default()
    };
    let lower = || {
        let sources = SourceMap::new();
        lower_xsd(
            &UriRef::parse("file:///schema.xsd").unwrap(),
            &loader,
            &sources,
            "fuzz",
            Role::Both,
            limits.clone(),
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

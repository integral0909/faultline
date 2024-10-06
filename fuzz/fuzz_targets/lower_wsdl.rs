//! Lowers arbitrary text as a WSDL description, then diffs the result
//! against itself: reading and lowering must never panic, an error-free
//! result must satisfy the IR invariants, and a description never differs
//! from itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_source::{Limits, SourceMap};
use faultline_wsdl::lower_wsdl;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|text: &str| {
    // A schema to import exercises the WSDL-to-schema path.
    let loader = MemoryLoader::new().with("file:///service.wsdl", text).with(
        "file:///types.xsd",
        r#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema" targetNamespace="urn:t"><xs:element name="ping" type="xs:string"/></xs:schema>"#,
    );
    let limits = Limits {
        max_depth: 64,
        max_document_nodes: 20_000,
        ..Limits::default()
    };
    let lower = || {
        let sources = SourceMap::new();
        lower_wsdl(
            &UriRef::parse("file:///service.wsdl").unwrap(),
            &loader,
            &sources,
            "fuzz",
            limits.clone(),
        )
    };
    let Some(api) = lower().api else {
        return;
    };
    assert!(faultline_ir::validate(&api).is_empty());
    let second = lower().api.expect("lowering is deterministic");
    let changes = faultline_diff::diff(&api, &second);
    assert!(changes.is_empty(), "a description differs from itself: {:?}", changes.changes);
});

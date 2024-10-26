//! Lowers arbitrary text as a RAML API, then diffs the result against
//! itself: loading, expanding templates and lowering must never panic, an
//! error-free result must satisfy the IR invariants, and an API never
//! differs from itself.
#![no_main]

use faultline_doc::UriRef;
use faultline_doc::loader::MemoryLoader;
use faultline_raml::lower_raml;
use faultline_source::{Limits, SourceMap};
use libfuzzer_sys::fuzz_target;

/// A library and a fragment to use and include exercise those paths.
const LIBRARY: &str = "#%RAML 1.0 Library
types:
  Id: integer
  Pet:
    properties:
      id: Id
      name?: string
traits:
  paged:
    queryParameters:
      limit?: integer
resourceTypes:
  collection:
    get?:
      is: [paged]
      responses:
        200:
          body: <<item>>[]
";

fuzz_target!(|text: &str| {
    let loader = MemoryLoader::new()
        .with("file:///api.raml", text)
        .with("file:///lib.raml", LIBRARY)
        .with("file:///pet.json", r#"{"type": "object", "properties": {"id": {"type": "integer"}}}"#);
    let limits = Limits {
        max_depth: 64,
        max_document_nodes: 20_000,
        ..Limits::default()
    };
    let lower = || {
        let sources = SourceMap::new();
        lower_raml(
            &UriRef::parse("file:///api.raml").unwrap(),
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
    assert!(changes.is_empty(), "an API differs from itself: {:?}", changes.changes);
});

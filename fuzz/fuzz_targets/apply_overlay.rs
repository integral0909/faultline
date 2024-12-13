//! Reads arbitrary text as an overlay and applies it to a fixed OpenAPI
//! description: neither may panic, whatever the actions select.
#![no_main]

use faultline_doc::Format;
use faultline_source::{Diagnostics, Limits, SourceMap, SourceName};
use libfuzzer_sys::fuzz_target;

const API: &str = "openapi: 3.1.0
info: {title: Pets, version: 1.0.0}
tags: [{name: pets}, {name: admin}]
paths:
  /pets:
    get: {summary: List, tags: [pets], parameters: [{name: limit, in: query}]}
    post: {summary: Add, x-internal: true}
  /admin: {delete: {x-internal: true}}
";

fuzz_target!(|text: &str| {
    let sources = SourceMap::new();
    let limits = Limits {
        max_depth: 64,
        max_document_nodes: 20_000,
        ..Limits::default()
    };
    let parse = |name: &str, text: &str| {
        let file = sources
            .add(SourceName::Virtual(name.into()), text.to_owned())
            .ok()?;
        faultline_doc::parse(&file, Format::Yaml, &limits).document
    };
    let Some(overlay) = parse("overlay.yaml", text) else {
        return;
    };
    let mut api = parse("api.yaml", API).expect("the description parses").root;
    let mut diagnostics = Diagnostics::new();
    if let Some(overlay) = faultline_overlay::read(&overlay.root, &mut diagnostics) {
        faultline_overlay::apply(&overlay, &mut api, &mut diagnostics);
    }
});

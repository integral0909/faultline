#![allow(dead_code, unreachable_pub)] // shared by several test crates; each uses a subset

use std::fmt::Write as _;

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::Api;
use faultline_openapi::lower_openapi;
use faultline_source::{DiagnosticCode, Diagnostics, Limits, SourceMap, render};

pub struct Outcome {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
    pub map: SourceMap,
}

/// Lowers a workspace of `(path, text)` files; the first is the entry.
pub fn lower(files: &[(&str, &str)]) -> Outcome {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let map = SourceMap::new();
    let lowered = {
        let mut store = DocumentStore::new(&loader, &map, Limits::default());
        let entry = UriRef::parse(&format!("file:///{}", files[0].0)).unwrap();
        lower_openapi(&entry, &mut store, "api")
    };
    Outcome {
        api: lowered.api,
        diagnostics: lowered.diagnostics,
        map,
    }
}

impl Outcome {
    pub fn codes(&self) -> Vec<&'static str> {
        self.diagnostics
            .iter()
            .filter_map(|d| d.code.map(DiagnosticCode::as_str))
            .collect()
    }

    pub fn rendered(&self) -> String {
        self.diagnostics.iter().fold(String::new(), |mut out, d| {
            let _ = writeln!(out, "{}", render::render(d, &self.map));
            out
        })
    }

    pub fn clean(&self) -> &Api {
        assert!(
            self.diagnostics.is_empty(),
            "unexpected diagnostics:\n{}",
            self.rendered()
        );
        self.api.as_ref().expect("api")
    }

    pub fn dump(&self) -> String {
        faultline_ir::dump(self.api.as_ref().expect("api"))
    }
}

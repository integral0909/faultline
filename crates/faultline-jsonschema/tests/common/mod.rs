#![allow(dead_code, unreachable_pub)] // shared by several test crates; each uses a subset

use std::fmt::Write as _;

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Role, TypeId, TypeKind};
use faultline_jsonschema::{Lowered, Options, lower_document};
use faultline_source::{DiagnosticCode, Diagnostics, Limits, SourceMap, render};

pub struct Outcome {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
    pub map: SourceMap,
}

/// Lowers a workspace of `(path, text)` files; the first is the entry.
pub fn lower(files: &[(&str, &str)]) -> Outcome {
    lower_with(files, Options::default())
}

pub fn lower_with(files: &[(&str, &str)], options: Options) -> Outcome {
    let mut loader = MemoryLoader::new();
    for (path, text) in files {
        loader.insert(&format!("file:///{path}"), *text);
    }
    let map = SourceMap::new();
    let Lowered { api, diagnostics } = {
        let mut store = DocumentStore::new(&loader, &map, Limits::default());
        let entry = UriRef::parse(&format!("file:///{}", files[0].0)).unwrap();
        lower_document(&entry, &mut store, options, Role::Input)
    };
    Outcome {
        api,
        diagnostics,
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

    pub fn assert_clean(&self) -> &Api {
        assert!(
            self.diagnostics.is_empty(),
            "unexpected diagnostics:\n{}",
            self.rendered()
        );
        self.api.as_ref().expect("api")
    }

    pub fn api(&self) -> &Api {
        self.api.as_ref().expect("api")
    }

    pub fn dump(&self) -> String {
        faultline_ir::dump(self.api())
    }

    /// The node behind a named type, following references.
    pub fn named(&self, name: &str) -> &TypeKind {
        let api = self.api();
        let id = api
            .named_type(name)
            .unwrap_or_else(|| panic!("no type named {name}:\n{}", self.dump()));
        &api.types.get(api.types.resolve(id)).kind
    }

    pub fn kind(&self, id: TypeId) -> &TypeKind {
        let api = self.api();
        &api.types.get(api.types.resolve(id)).kind
    }

    /// The text of the primary span of the diagnostic with `code`.
    pub fn primary_text(&self, code: &str) -> String {
        let d = self
            .diagnostics
            .iter()
            .find(|d| d.code.map(DiagnosticCode::as_str) == Some(code))
            .unwrap_or_else(|| panic!("no {code}:\n{}", self.rendered()));
        let span = d.primary_span().expect("span");
        self.map.slice(span).expect("slice")
    }
}

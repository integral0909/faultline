#![allow(dead_code, unreachable_pub)] // shared by several test crates; each uses a subset

use faultline_doc::loader::MemoryLoader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, Literal, Role, TypeId};
use faultline_jsonschema::{Options, lower_document};
use faultline_source::{Limits, SourceMap};

/// A schema lowered into its own API, plus an independent validator for it.
pub struct Schema {
    pub api: Api,
    pub root: TypeId,
    oracle: jsonschema::Validator,
}

impl Schema {
    pub fn new(text: &str) -> Schema {
        let loader = MemoryLoader::new().with("file:///s.json", text);
        let sources = SourceMap::new();
        let mut store = DocumentStore::new(&loader, &sources, Limits::default());
        let lowered = lower_document(
            &UriRef::parse("file:///s.json").unwrap(),
            &mut store,
            Options::default(),
            Role::Input,
        );
        assert!(
            !lowered.diagnostics.has_errors(),
            "schema did not lower cleanly: {text}\n{:?}",
            lowered.diagnostics
        );
        let api = lowered.api.unwrap();
        let root = api.schemas[0].ty;
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        let oracle = jsonschema::draft202012::options()
            .should_validate_formats(false)
            .build(&json)
            .unwrap();
        Schema { api, root, oracle }
    }

    /// What the independent validator says about `value`.
    pub fn accepts(&self, value: &Literal) -> bool {
        let json: serde_json::Value = serde_json::from_str(&value.to_string()).unwrap();
        self.oracle.is_valid(&json)
    }
}

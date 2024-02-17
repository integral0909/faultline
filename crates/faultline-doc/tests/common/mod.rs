#![allow(dead_code, unreachable_pub)] // shared by several test crates; each uses a subset

use std::fmt::Write as _;

use faultline_doc::{Format, Node, Parsed, parse};
use faultline_source::{Diagnostic, DiagnosticCode, Limits, SourceMap, SourceName, Span, render};

pub struct Outcome {
    pub map: SourceMap,
    pub text: String,
    pub parsed: Parsed,
}

impl Outcome {
    pub fn root(&self) -> &Node {
        &self
            .parsed
            .document
            .as_ref()
            .unwrap_or_else(|| panic!("no document; diagnostics:\n{}", self.rendered()))
            .root
    }

    pub fn slice(&self, span: Span) -> &str {
        &self.text[span.range()]
    }

    pub fn codes(&self) -> Vec<&'static str> {
        self.parsed
            .diagnostics
            .iter()
            .filter_map(|d| d.code.map(DiagnosticCode::as_str))
            .collect()
    }

    pub fn only_diagnostic(&self) -> &Diagnostic {
        assert_eq!(
            self.parsed.diagnostics.len(),
            1,
            "expected exactly one diagnostic:\n{}",
            self.rendered()
        );
        self.parsed.diagnostics.iter().next().unwrap()
    }

    pub fn primary_text(&self, diagnostic: &Diagnostic) -> &str {
        self.slice(diagnostic.primary_span().expect("diagnostic has a span"))
    }

    pub fn rendered(&self) -> String {
        self.parsed
            .diagnostics
            .iter()
            .fold(String::new(), |mut out, d| {
                let _ = writeln!(out, "{}", render::render(d, &self.map));
                out
            })
    }

    pub fn assert_clean(&self) {
        assert!(
            self.parsed.diagnostics.is_empty(),
            "unexpected diagnostics:\n{}",
            self.rendered()
        );
    }
}

pub fn parse_with(format: Format, text: &str, limits: &Limits) -> Outcome {
    let map = SourceMap::new();
    let file = map
        .add(SourceName::Virtual("test".into()), text)
        .expect("small test input");
    let parsed = parse(&file, format, limits);
    Outcome {
        map,
        text: text.to_owned(),
        parsed,
    }
}

pub fn json(text: &str) -> Outcome {
    parse_with(Format::Json, text, &Limits::default())
}

pub fn yaml(text: &str) -> Outcome {
    parse_with(Format::Yaml, text, &Limits::default())
}

/// Serializes a node as compact JSON.
pub fn to_json(node: &Node) -> String {
    node.to_json()
}

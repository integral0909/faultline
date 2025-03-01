//! Overlays and extensions: `#%RAML 1.0 Overlay` and `#%RAML 1.0 Extension`
//! documents name the API they apply to in `extends`, and are merged into
//! it before anything else happens.
//!
//! Merging follows RAML's rules. A member only one side has is kept. Maps
//! present on both sides are merged member by member. Arrays get the
//! extension's values the master lacks. Any other value of the extension
//! replaces the master's. An extension may change anything; an overlay only
//! documentation (titles, descriptions, examples, annotations), and other
//! changes are reported. An extension or overlay may extend another in
//! turn.

use faultline_doc::{Node, Object, Value};
use faultline_source::{Diagnostic, Span};

use crate::codes;
use crate::load::{Document, RamlLoader};

/// Overlays and extensions extend each other at most this deep.
const MAX_DEPTH: usize = 16;

/// The members an overlay may add or change.
const DOCUMENTATION: [&str; 8] = [
    "title",
    "description",
    "displayName",
    "documentation",
    "usage",
    "example",
    "examples",
    "annotationTypes",
];

/// `document` with the API it extends merged in, if it is an overlay or
/// an extension; `document` itself otherwise. The result's kind is the
/// master's, and its URI the master's, against which the master's own
/// library paths resolve.
pub(crate) fn apply(raml: &mut RamlLoader<'_>, document: Document) -> Option<Document> {
    apply_at(raml, document, 0)
}

fn apply_at(raml: &mut RamlLoader<'_>, mut document: Document, depth: usize) -> Option<Document> {
    let overlay = match document.kind.as_str() {
        "Overlay" => true,
        "Extension" => false,
        _ => return Some(document),
    };
    let Some(extends) = document.root.get("extends") else {
        error(
            raml,
            format!(
                "a RAML {} names the API it applies to in `extends`",
                document.kind
            ),
            document.root.span,
        );
        return None;
    };
    let span = extends.span;
    let Some(path) = extends.as_str() else {
        error(raml, "`extends` is the path of an API".into(), span);
        return None;
    };
    if depth >= MAX_DEPTH {
        error(
            raml,
            format!("overlays and extensions extend each other more than {MAX_DEPTH} deep"),
            span,
        );
        return None;
    }
    let target = raml.resolve(path, &document.uri, span)?;
    let master = raml.document(&target, Some(span))?;
    let master = apply_at(raml, master, depth + 1)?;
    if !master.kind.is_empty() {
        error(
            raml,
            format!("{path} is a RAML {}, not an API", master.kind),
            span,
        );
        return None;
    }
    // Library paths are relative to the document that names them.
    absolute_uses(raml, &mut document);
    let mut root = master.root;
    let mut merger = Merger {
        overlay,
        diagnostics: Vec::new(),
    };
    if let Some(members) = document.root.as_object() {
        let Value::Object(into) = &mut root.value else {
            return None;
        };
        for (key, member) in members.members() {
            if key == "extends" {
                continue;
            }
            merger.member(into, key, member.key_span, &member.value, false);
        }
    }
    let failed = !merger.diagnostics.is_empty();
    for diagnostic in merger.diagnostics {
        raml.diagnostics.push(diagnostic);
    }
    (!failed).then_some(Document {
        kind: master.kind,
        root,
        uri: master.uri,
        version: master.version,
    })
}

/// Rewrites the library paths of `document`'s `uses` to absolute URIs.
fn absolute_uses(raml: &mut RamlLoader<'_>, document: &mut Document) {
    let Value::Object(root) = &mut document.root.value else {
        return;
    };
    let Some((_, uses)) = root.iter_mut().find(|(k, _)| *k == "uses") else {
        return;
    };
    let Value::Object(uses) = &mut uses.value else {
        return;
    };
    for (_, path) in uses.iter_mut() {
        if let Value::String(text) = &path.value
            && let Some(uri) = raml.resolve(text, &document.uri, path.span)
        {
            path.value = Value::String(uri.to_string());
        }
    }
}

struct Merger {
    overlay: bool,
    diagnostics: Vec<Diagnostic>,
}

impl Merger {
    /// Merges the member `key` of the extension into `into`. `documentation`
    /// is true inside a member an overlay may change freely.
    fn member(
        &mut self,
        into: &mut Object,
        key: &str,
        key_span: Span,
        value: &Node,
        documentation: bool,
    ) {
        let documentation = documentation || is_documentation(key);
        let existing = into.iter_mut().find(|(k, _)| *k == key).map(|(_, v)| v);
        match existing {
            None => {
                if self.overlay && !documentation {
                    self.refuse(key, key_span);
                    return;
                }
                into.insert(key.to_owned(), key_span, value.clone());
            }
            Some(target) => self.merge(target, key, value, documentation),
        }
    }

    fn merge(&mut self, target: &mut Node, key: &str, value: &Node, documentation: bool) {
        match (&mut target.value, &value.value) {
            (Value::Object(into), Value::Object(from)) => {
                for (key, member) in from.members() {
                    self.member(into, key, member.key_span, &member.value, documentation);
                }
            }
            (Value::Array(into), Value::Array(from)) => {
                let added: Vec<Node> = from
                    .iter()
                    .filter(|item| !into.iter().any(|i| i.same_value(item)))
                    .cloned()
                    .collect();
                if !added.is_empty() && self.overlay && !documentation {
                    self.refuse(key, value.span);
                    return;
                }
                into.extend(added);
            }
            _ => {
                if target.same_value(value) {
                    return;
                }
                if self.overlay && !documentation {
                    self.refuse(key, value.span);
                    return;
                }
                *target = value.clone();
            }
        }
    }

    fn refuse(&mut self, key: &str, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(format!(
                "an overlay can only change documentation, not `{key}`; use an extension"
            ))
            .with_code(codes::INVALID)
            .with_primary(span, ""),
        );
    }
}

/// Whether an overlay may change the member `key` and everything in it.
fn is_documentation(key: &str) -> bool {
    DOCUMENTATION.contains(&key) || (key.starts_with('(') && key.ends_with(')'))
}

fn error(raml: &mut RamlLoader<'_>, message: String, span: Span) {
    raml.diagnostics.push(
        Diagnostic::error(message)
            .with_code(codes::INVALID)
            .with_primary(span, ""),
    );
}

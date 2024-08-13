//! Navigating documents that embed JSON Schema (OpenAPI, AsyncAPI):
//! positions in loaded documents, and references between the objects
//! around schemas (`{"$ref": "#/components/messages/Placed"}`), which are
//! plain JSON references rather than schema references.

use std::sync::Arc;

use faultline_doc::{JsonPointer, Node, Object, StoreError, StoredDocument, UriRef};
use faultline_source::{Diagnostic, DiagnosticCode, Diagnostics, Span};

use crate::lower::Lowerer;
use crate::registry::Location;

/// A position in a loaded document.
#[derive(Clone)]
pub struct At {
    pub document: Arc<StoredDocument>,
    pub pointer: JsonPointer,
}

impl std::fmt::Debug for At {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}#{}", self.document.uri, self.pointer)
    }
}

impl At {
    pub fn root(document: Arc<StoredDocument>) -> Self {
        Self {
            document,
            pointer: JsonPointer::root(),
        }
    }

    /// The node at this position. Positions are only created for nodes that
    /// exist, so this cannot fail for a well-formed `At`.
    pub fn node(&self) -> &Node {
        self.document
            .document
            .root
            .pointer(&self.pointer)
            .expect("positions are created for existing nodes")
    }

    pub fn object(&self) -> Option<&Object> {
        self.node().as_object()
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.node().get(key)
    }

    /// The position of member `key`, if present.
    pub fn child(&self, key: &str) -> Option<At> {
        self.get(key)?;
        Some(self.join(key))
    }

    /// The position of member `key`, which the caller knows exists.
    #[must_use]
    pub fn join(&self, key: &str) -> At {
        At {
            document: Arc::clone(&self.document),
            pointer: self.pointer.join(key),
        }
    }

    pub fn location(&self) -> Location {
        Location::new(self.document.uri.clone(), self.pointer.clone())
    }

    pub fn span(&self) -> Span {
        self.node().span
    }

    /// The span of this member's key, when the position is an object member;
    /// otherwise the node's span.
    pub fn key_span(&self) -> Span {
        let (Some(parent), Some(key)) = (self.pointer.parent(), self.pointer.tokens().last())
        else {
            return self.span();
        };
        self.document
            .document
            .root
            .pointer(&parent)
            .and_then(Node::as_object)
            .and_then(|o| o.member(key))
            .map_or_else(|| self.span(), |m| m.key_span)
    }

    /// Members of an object-valued node, as positions, in document order.
    pub fn entries(&self) -> Vec<(String, At)> {
        self.object()
            .map(|obj| obj.keys().map(|k| (k.to_owned(), self.join(k))).collect())
            .unwrap_or_default()
    }

    /// Elements of an array-valued node, as positions.
    pub fn elements(&self) -> Vec<At> {
        let len = self.node().as_array().map_or(0, <[Node]>::len);
        (0..len).map(|i| self.join(&i.to_string())).collect()
    }
}

/// The diagnostic codes a front end reports reference problems with.
#[derive(Clone, Copy, Debug)]
pub struct ReferenceCodes {
    /// `$ref` is not a string.
    pub invalid: DiagnosticCode,
    /// The target cannot be loaded or does not exist.
    pub unresolved: DiagnosticCode,
    /// References lead back to themselves, or chain too long.
    pub cycle: DiagnosticCode,
}

const MAX_REFERENCE_HOPS: usize = 32;

/// Follows reference objects (`{"$ref": ...}`) from `at` until a
/// non-reference object is reached. Returns `None`, with a diagnostic, when a
/// reference cannot be resolved. Targets are loaded through `lowerer`, so
/// they share its sandbox and its registry of schema resources.
pub fn resolve(
    lowerer: &mut Lowerer<'_, '_>,
    at: At,
    codes: ReferenceCodes,
    diagnostics: &mut Diagnostics,
) -> Option<At> {
    let error = |diagnostics: &mut Diagnostics, code, message: &str, span| {
        diagnostics.push(
            Diagnostic::error(message.to_owned())
                .with_code(code)
                .with_primary(span, ""),
        );
    };
    let mut current = at;
    let mut seen: Vec<(UriRef, JsonPointer)> = Vec::new();
    for _ in 0..MAX_REFERENCE_HOPS {
        let Some(reference) = current.get("$ref") else {
            return Some(current);
        };
        let span = reference.span;
        let Some(text) = reference.as_str() else {
            error(diagnostics, codes.invalid, "$ref must be a string", span);
            return None;
        };
        let key = (current.document.uri.clone(), current.pointer.clone());
        if seen.contains(&key) {
            error(diagnostics, codes.cycle, "references form a cycle", span);
            return None;
        }
        seen.push(key);
        current = follow(lowerer, &current, text, span, codes, diagnostics)?;
    }
    let span = current.span();
    error(
        diagnostics,
        codes.cycle,
        "too many chained references",
        span,
    );
    None
}

fn follow(
    lowerer: &mut Lowerer<'_, '_>,
    from: &At,
    reference: &str,
    span: Span,
    codes: ReferenceCodes,
    diagnostics: &mut Diagnostics,
) -> Option<At> {
    let mut error = |message: &str| {
        diagnostics.push(
            Diagnostic::error(message.to_owned())
                .with_code(codes.unresolved)
                .with_primary(span, ""),
        );
    };
    let Ok(parsed) = UriRef::parse(reference) else {
        error("not a valid URI reference");
        return None;
    };
    let base = &from.document.uri;
    let is_file = base
        .scheme()
        .is_some_and(|s| s.eq_ignore_ascii_case("file"));
    let target = if is_file && !parsed.has_scheme() {
        let Some(target) = base.resolve_strict(&parsed) else {
            error("points outside the workspace root");
            return None;
        };
        target
    } else {
        base.resolve(&parsed)
    };
    let document = match lowerer.document(&target) {
        Ok(document) => document,
        Err(StoreError::Load(e)) => {
            error(&format!("cannot load the target: {e}"));
            return None;
        }
        Err(StoreError::Parse) => return None,
    };
    let pointer = match target.fragment() {
        None | Some("") => JsonPointer::root(),
        Some(fragment) => match JsonPointer::from_uri_fragment(fragment) {
            Ok(pointer) => pointer,
            Err(e) => {
                error(&e.to_string());
                return None;
            }
        },
    };
    if document.document.root.pointer(&pointer).is_none() {
        error("nothing exists at this location");
        return None;
    }
    Some(At { document, pointer })
}

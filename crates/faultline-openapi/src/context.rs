//! Navigation of OpenAPI documents: positions, reference resolution for
//! non-schema objects, and typed field access with diagnostics.

use std::sync::Arc;

use faultline_doc::{JsonPointer, Node, Object, StoreError, StoredDocument, UriRef, Value};
use faultline_ir::TypeId;
use faultline_jsonschema::{Dialect, Location, Lowerer};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;

/// A position in a loaded document.
#[derive(Clone)]
pub(crate) struct At {
    pub(crate) document: Arc<StoredDocument>,
    pub(crate) pointer: JsonPointer,
}

impl At {
    pub(crate) fn root(document: Arc<StoredDocument>) -> Self {
        Self {
            document,
            pointer: JsonPointer::root(),
        }
    }

    /// The node at this position. Positions are only created for nodes that
    /// exist, so this cannot fail for a well-formed `At`.
    pub(crate) fn node(&self) -> &Node {
        self.document
            .document
            .root
            .pointer(&self.pointer)
            .expect("positions are created for existing nodes")
    }

    pub(crate) fn object(&self) -> Option<&Object> {
        self.node().as_object()
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Node> {
        self.node().get(key)
    }

    /// The position of member `key`, if present.
    pub(crate) fn child(&self, key: &str) -> Option<At> {
        self.get(key)?;
        Some(self.join(key))
    }

    /// The position of member `key`, which the caller knows exists.
    pub(crate) fn join(&self, key: &str) -> At {
        At {
            document: Arc::clone(&self.document),
            pointer: self.pointer.join(key),
        }
    }

    pub(crate) fn location(&self) -> Location {
        Location::new(self.document.uri.clone(), self.pointer.clone())
    }

    pub(crate) fn span(&self) -> Span {
        self.node().span
    }

    /// Members of an object-valued node, as positions, in document order.
    pub(crate) fn entries(&self) -> Vec<(String, At)> {
        self.object()
            .map(|obj| obj.keys().map(|k| (k.to_owned(), self.join(k))).collect())
            .unwrap_or_default()
    }

    /// Elements of an array-valued node, as positions.
    pub(crate) fn elements(&self) -> Vec<At> {
        let len = self.node().as_array().map_or(0, <[Node]>::len);
        (0..len).map(|i| self.join(&i.to_string())).collect()
    }
}

/// Everything the lowering of one OpenAPI description needs.
pub(crate) struct Context<'x, 'a, 'l> {
    pub(crate) lowerer: &'x mut Lowerer<'a, 'l>,
    pub(crate) dialect: Dialect,
    pub(crate) diagnostics: Diagnostics,
}

const MAX_REFERENCE_HOPS: usize = 32;

impl Context<'_, '_, '_> {
    /// Follows Reference Objects (`{"$ref": ...}`) starting at `at` until a
    /// non-reference object is reached. Returns `None` (with a diagnostic)
    /// when a reference cannot be resolved.
    pub(crate) fn resolve(&mut self, at: At) -> Option<At> {
        let mut current = at;
        let mut seen: Vec<(UriRef, JsonPointer)> = Vec::new();
        for _ in 0..MAX_REFERENCE_HOPS {
            let Some(reference) = current.get("$ref") else {
                return Some(current);
            };
            let span = reference.span;
            let Some(text) = reference.as_str() else {
                self.error(codes::INVALID_FIELD, "$ref must be a string", span);
                return None;
            };
            let key = (current.document.uri.clone(), current.pointer.clone());
            if seen.contains(&key) {
                self.error(codes::REFERENCE_CYCLE, "references form a cycle", span);
                return None;
            }
            seen.push(key);
            current = self.follow(&current, text, span)?;
        }
        let span = current.span();
        self.error(codes::REFERENCE_CYCLE, "too many chained references", span);
        None
    }

    fn follow(&mut self, from: &At, reference: &str, span: Span) -> Option<At> {
        let Ok(parsed) = UriRef::parse(reference) else {
            self.error(
                codes::UNRESOLVED_REFERENCE,
                "not a valid URI reference",
                span,
            );
            return None;
        };
        let base = &from.document.uri;
        let is_file = base
            .scheme()
            .is_some_and(|s| s.eq_ignore_ascii_case("file"));
        let target = if is_file && !parsed.has_scheme() {
            let Some(target) = base.resolve_strict(&parsed) else {
                self.error(
                    codes::UNRESOLVED_REFERENCE,
                    "points outside the workspace root",
                    span,
                );
                return None;
            };
            target
        } else {
            base.resolve(&parsed)
        };
        let document = match self.lowerer.document(&target) {
            Ok(document) => document,
            Err(StoreError::Load(e)) => {
                self.error(
                    codes::UNRESOLVED_REFERENCE,
                    &format!("cannot load the target: {e}"),
                    span,
                );
                return None;
            }
            Err(StoreError::Parse) => return None,
        };
        let pointer = match target.fragment() {
            None | Some("") => JsonPointer::root(),
            Some(fragment) => match JsonPointer::from_uri_fragment(fragment) {
                Ok(pointer) => pointer,
                Err(e) => {
                    self.error(codes::UNRESOLVED_REFERENCE, &e.to_string(), span);
                    return None;
                }
            },
        };
        if document.document.root.pointer(&pointer).is_none() {
            self.error(
                codes::UNRESOLVED_REFERENCE,
                "nothing exists at this location",
                span,
            );
            return None;
        }
        Some(At { document, pointer })
    }

    /// The type of the schema at `at`.
    pub(crate) fn schema(&mut self, at: &At) -> TypeId {
        match self.lowerer.schema_at(&at.location(), self.dialect) {
            Some(id) => id,
            None => self
                .lowerer
                .api_mut()
                .types
                .alloc_kind(faultline_ir::TypeKind::Any),
        }
    }

    /// A string field, reporting a diagnostic if present with another type.
    pub(crate) fn string(&mut self, at: &At, key: &str) -> Option<String> {
        let node = at.get(key)?;
        if let Value::String(s) = &node.value {
            Some(s.clone())
        } else {
            let span = node.span;
            self.error(
                codes::INVALID_FIELD,
                &format!("\"{key}\" must be a string"),
                span,
            );
            None
        }
    }

    /// A boolean field, reporting a diagnostic if present with another type.
    pub(crate) fn boolean(&mut self, at: &At, key: &str) -> Option<bool> {
        let node = at.get(key)?;
        if let Value::Bool(b) = node.value {
            Some(b)
        } else {
            let span = node.span;
            self.error(
                codes::INVALID_FIELD,
                &format!("\"{key}\" must be a boolean"),
                span,
            );
            None
        }
    }

    /// Requires `at` to be an object.
    pub(crate) fn expect_object(&mut self, at: &At, what: &str) -> bool {
        if at.object().is_some() {
            return true;
        }
        let span = at.span();
        self.error(
            codes::INVALID_FIELD,
            &format!("{what} must be an object"),
            span,
        );
        false
    }

    pub(crate) fn error(
        &mut self,
        code: faultline_source::DiagnosticCode,
        message: &str,
        span: Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(message.to_owned())
                .with_code(code)
                .with_primary(span, ""),
        );
    }
}

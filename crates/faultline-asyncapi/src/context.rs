//! What the lowering of one AsyncAPI description carries along.

use faultline_doc::Value;
use faultline_ir::{TypeId, TypeKind};
use faultline_jsonschema::nav::{self, At, ReferenceCodes};
use faultline_jsonschema::{Dialect, Lowerer};
use faultline_source::{Diagnostic, DiagnosticCode, Diagnostics, Span};

use crate::codes;

pub(crate) struct Context<'x, 'a, 'l> {
    pub(crate) lowerer: &'x mut Lowerer<'a, 'l>,
    pub(crate) diagnostics: Diagnostics,
    /// `defaultContentType`, or `application/json`.
    pub(crate) default_content_type: String,
    pub(crate) v3: bool,
}

impl Context<'_, '_, '_> {
    /// Follows reference objects from `at` to the object they name.
    pub(crate) fn resolve(&mut self, at: At) -> Option<At> {
        let codes = ReferenceCodes {
            invalid: codes::INVALID_FIELD,
            unresolved: codes::UNRESOLVED_REFERENCE,
            cycle: codes::REFERENCE_CYCLE,
        };
        nav::resolve(self.lowerer, at, codes, &mut self.diagnostics)
    }

    /// The type of the JSON Schema at `at`, in `dialect`.
    pub(crate) fn schema(&mut self, at: &At, dialect: Dialect) -> TypeId {
        match self.lowerer.schema_at(&at.location(), dialect) {
            Some(id) => id,
            None => self.any(),
        }
    }

    pub(crate) fn any(&mut self) -> TypeId {
        self.lowerer.api_mut().types.alloc_kind(TypeKind::Any)
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

    pub(crate) fn error(&mut self, code: DiagnosticCode, message: &str, span: Span) {
        self.diagnostics.push(
            Diagnostic::error(message.to_owned())
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    pub(crate) fn warning(&mut self, code: DiagnosticCode, message: &str, span: Span) {
        self.diagnostics.push(
            Diagnostic::warning(message.to_owned())
                .with_code(code)
                .with_primary(span, ""),
        );
    }

    /// The traits of the message or operation at `at`, resolved, in the
    /// order they apply.
    pub(crate) fn trait_list(&mut self, at: &At) -> Vec<At> {
        let Some(list) = at.child("traits") else {
            return Vec::new();
        };
        if list.node().as_array().is_none() {
            let span = list.span();
            self.error(codes::INVALID_FIELD, "traits must be an array", span);
            return Vec::new();
        }
        list.elements()
            .into_iter()
            .filter_map(|element| self.resolve(element))
            .filter(|t| t.object().is_some())
            .collect()
    }

    /// A string field of `at` with its traits applied. In 2.x traits are
    /// merge patches applied in order, so the last trait that sets the
    /// field wins; in 3.0 the object's own value takes precedence, and
    /// traits only fill in what it leaves out.
    pub(crate) fn traited_string(&mut self, at: &At, traits: &[At], key: &str) -> Option<String> {
        let from_traits = traits.iter().rev().find(|t| t.get(key).is_some()).cloned();
        let source = if self.v3 {
            at.get(key).is_some().then(|| at.clone()).or(from_traits)
        } else {
            from_traits.or_else(|| at.get(key).is_some().then(|| at.clone()))
        }?;
        self.string(&source, key)
    }
}

/// The last segment of a reference's JSON Pointer fragment, which names the
/// object it points at: `#/components/messages/Placed` names `Placed`.
pub(crate) fn referenced_name(at: &At) -> Option<String> {
    let reference = at.get("$ref")?.as_str()?;
    let (_, fragment) = reference.split_once('#')?;
    let last = fragment.rsplit('/').next()?;
    let decoded = faultline_doc::uri::percent_decode(last)?;
    let name = decoded.replace("~1", "/").replace("~0", "~");
    (!name.is_empty()).then_some(name)
}

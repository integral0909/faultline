//! Navigation of OpenAPI documents: positions, reference resolution for
//! non-schema objects, and typed field access with diagnostics.

use faultline_doc::Value;
use faultline_ir::TypeId;
use faultline_jsonschema::nav::{self, ReferenceCodes};
use faultline_jsonschema::{Dialect, Lowerer};
use faultline_source::{Diagnostic, Diagnostics, Span};

use crate::codes;

pub(crate) use faultline_jsonschema::nav::At;

/// Everything the lowering of one OpenAPI description needs.
pub(crate) struct Context<'x, 'a, 'l> {
    pub(crate) lowerer: &'x mut Lowerer<'a, 'l>,
    pub(crate) dialect: Dialect,
    pub(crate) diagnostics: Diagnostics,
}

impl Context<'_, '_, '_> {
    /// Follows Reference Objects (`{"$ref": ...}`) starting at `at` until a
    /// non-reference object is reached. Returns `None` (with a diagnostic)
    /// when a reference cannot be resolved.
    pub(crate) fn resolve(&mut self, at: At) -> Option<At> {
        let codes = ReferenceCodes {
            invalid: codes::INVALID_FIELD,
            unresolved: codes::UNRESOLVED_REFERENCE,
            cycle: codes::REFERENCE_CYCLE,
        };
        nav::resolve(self.lowerer, at, codes, &mut self.diagnostics)
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

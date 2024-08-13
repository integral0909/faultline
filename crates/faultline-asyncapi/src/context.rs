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

    /// Warns that `traits` at `at` are not applied, when there are any.
    pub(crate) fn traits(&mut self, at: &At, what: &str) {
        if let Some(traits) = at.get("traits") {
            let span = traits.span;
            self.warning(
                codes::TRAITS_IGNORED,
                &format!("{what} traits are not applied; the comparison may miss what they add"),
                span,
            );
        }
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

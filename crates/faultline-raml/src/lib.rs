//! The RAML 1.0 front end.
//!
//! [`load`] reads RAML documents with their `!include`s expanded, and
//! [`expr`] parses type expressions, which [`types`] lowers into the IR
//! with the rest of RAML's type system. [`lower_raml`] lowers an API's
//! resources and methods into operations.

pub mod codes;
pub mod expr;
mod extend;
mod jsonschema;
mod libraries;
pub mod load;
mod lower;
mod raml08;
pub mod templates;
pub mod types;

pub use load::{Document, RamlLoader};
pub use lower::{Lowered, lower_raml};

use faultline_doc::Node;
use faultline_ir::{Api, ApiFormat, TypeId};
use faultline_source::{Diagnostic, Diagnostics};

/// A RAML type declared in another document, lowered on its own.
#[derive(Debug)]
pub struct Embedded {
    /// The types, and the one declared; `None` when the declaration has
    /// errors.
    pub lowered: Option<(Api, TypeId)>,
    pub diagnostics: Diagnostics,
}

/// Lowers the RAML type declaration `node` (an AsyncAPI payload, for
/// example): a type expression or a map of facets, naming only RAML's
/// built-in types. JSON Schemas inside it are compared as any value.
pub fn lower_embedded(node: &Node) -> Embedded {
    let mut api = Api::new("embedded", ApiFormat::Raml);
    let mut lowerer = types::TypeLowerer::new(&mut api);
    let id = lowerer.declaration(node, &types::Scope::default());
    let mut diagnostics = std::mem::take(&mut lowerer.diagnostics);
    for pending in std::mem::take(&mut lowerer.pending) {
        let span = match pending.source {
            types::SchemaSource::Text { span, .. } => span,
            types::SchemaSource::File(_) => node.span,
        };
        diagnostics.push(
            Diagnostic::warning(
                "a JSON Schema inside an embedded RAML type is compared as accepting any value"
                    .to_owned(),
            )
            .with_code(codes::INVALID)
            .with_primary(span, ""),
        );
    }
    diagnostics.sort();
    let lowered = (!diagnostics.has_errors()).then_some((api, id));
    Embedded {
        lowered,
        diagnostics,
    }
}

//! The JSON Schema front end.
//!
//! Lowers JSON Schema documents (draft-04 through 2020-12) into the Faultline
//! IR. Reference resolution follows the specifications: identifiers (`$id`)
//! establish base URIs, fragments are JSON Pointers or anchors, and dynamic
//! references are resolved against the dynamic scope. Referenced documents are
//! loaded through a [`DocumentStore`], whose loader enforces the workspace
//! sandbox.
//!
//! [`lower_document`] handles standalone schema documents. Formats that embed
//! JSON Schema, such as OpenAPI, drive a [`Lowerer`] directly.

mod applicators;
mod assertions;
pub mod codes;
mod dialect;
mod keywords;
mod lower;
mod registry;

use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, ApiFormat, Role, SchemaRoot};
use faultline_source::Diagnostics;

pub use dialect::Dialect;
pub use lower::{Lowerer, Options};
pub use registry::{Location, SchemaInfo};

/// The result of lowering: the API (present once the entry document could be
/// loaded) and every diagnostic. Callers must check
/// [`Diagnostics::has_errors`] before comparing the API.
#[derive(Debug)]
pub struct Lowered {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Lowers the schema document at `entry` into an API with one schema root,
/// named after the document, used in direction `role`. Every definition in
/// the entry document becomes a named type.
pub fn lower_document(
    entry: &UriRef,
    store: &mut DocumentStore<'_>,
    options: Options,
    role: Role,
) -> Lowered {
    let mut api = Api::new(document_name(entry), ApiFormat::JsonSchema);
    let mut lowerer = Lowerer::new(&mut api, store, options);
    let Some(root) = lowerer.load_schema_document(entry) else {
        return Lowered {
            api: None,
            diagnostics: lowerer.finish(),
        };
    };
    // Definitions first, so the entry document's own definitions get the
    // plain names when another document defines the same names.
    lowerer.lower_definitions(&root.document);
    let ty = lowerer.lower(&root);
    let diagnostics = lowerer.finish();
    let origin = api.types.get(ty).origin;
    api.schemas.push(SchemaRoot {
        name: api.name.clone(),
        ty,
        role,
        origin,
    });
    Lowered {
        api: Some(api),
        diagnostics,
    }
}

fn document_name(uri: &UriRef) -> String {
    let segment = uri
        .path()
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("schema");
    let decoded = faultline_doc::uri::percent_decode(segment).unwrap_or_else(|| segment.to_owned());
    match decoded.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem.to_owned(),
        _ => decoded,
    }
}

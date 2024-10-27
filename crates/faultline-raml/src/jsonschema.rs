//! JSON Schemas used as RAML types.
//!
//! RAML lets a type be a JSON Schema, included from a `.json` file or
//! written inline as JSON text. Their meaning is JSON Schema's, not
//! RAML's (properties are optional unless listed in `required`, for one),
//! so they are lowered by the JSON Schema front end into a separate type
//! graph that is then grafted onto the API's, with each placeholder the
//! RAML lowering left becoming a reference to its schema's type.
//! Schemas without `$schema` are read as draft-04, the draft RAML 1.0
//! names.

use faultline_doc::loader::Loader;
use faultline_doc::{DocumentStore, UriRef};
use faultline_ir::{Api, ApiFormat, TypeKind, TypeNode};
use faultline_jsonschema::{Dialect, Lowerer, Options};
use faultline_source::{Diagnostics, Limits, SourceMap, SourceName, Span};

use crate::types::{PendingSchema, SchemaSource};

/// Lowers `pending` schemas and points their placeholders in `api` at
/// them. References between schemas are resolved relative to the file each
/// is in, and inline schemas relative to `entry`.
pub(crate) fn lower_schemas(
    api: &mut Api,
    pending: &[PendingSchema],
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    limits: Limits,
) -> Diagnostics {
    if pending.is_empty() {
        return Diagnostics::new();
    }
    let mut store = DocumentStore::new(loader, sources, limits);
    // Where each schema is: its file, or a document registered for inline
    // text.
    let mut documents = Vec::with_capacity(pending.len());
    for (n, schema) in pending.iter().enumerate() {
        match &schema.source {
            SchemaSource::File(uri) => documents.push((schema.id, uri.clone())),
            SchemaSource::Text { text, span } => {
                let Some(uri) = inline_uri(entry, n) else {
                    continue;
                };
                let name =
                    SourceName::Virtual(format!("inline schema at {}", place(sources, *span)));
                if store.insert_text(&uri, &name, text.clone()).is_ok() {
                    documents.push((schema.id, uri));
                }
            }
        }
    }
    let mut schemas = Api::new(api.name.clone(), ApiFormat::JsonSchema);
    let mut lowerer = Lowerer::new(
        &mut schemas,
        &mut store,
        Options {
            default_dialect: Dialect::Draft4,
        },
    );
    let mut lowered = Vec::with_capacity(documents.len());
    for (placeholder, uri) in documents {
        if let Some(root) = lowerer.load_schema_document(&uri) {
            lowered.push((placeholder, lowerer.lower(&root)));
        }
    }
    let mut diagnostics = lowerer.finish();
    diagnostics.extend(store.take_diagnostics());
    let graft = api.types.graft(schemas.types);
    for (placeholder, ty) in lowered {
        // The placeholder was defined as `any`; it becomes the schema.
        let node = api.types.get_mut(placeholder);
        let origin = node.origin;
        *node = TypeNode::new(TypeKind::Ref(graft.map(ty)));
        node.origin = origin;
    }
    diagnostics
}

/// `file:line` of a span, for naming the inline schema there.
fn place(sources: &SourceMap, span: Span) -> String {
    sources.get(span.source()).map_or_else(String::new, |file| {
        let line = file.lines().line_of(span.start()) + 1;
        format!("{}:{line}", file.name())
    })
}

/// A URI for the `n`th inline schema, next to the entry document.
fn inline_uri(entry: &UriRef, n: usize) -> Option<UriRef> {
    let base = entry.without_fragment();
    UriRef::parse(&format!("{base}~schema-{n}.json")).ok()
}

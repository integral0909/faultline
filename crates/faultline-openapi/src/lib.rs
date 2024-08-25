//! The OpenAPI front end.
//!
//! Lowers Swagger 2.0, OpenAPI 3.0 and OpenAPI 3.1
//! descriptions into the Faultline IR: every
//! operation with its parameters (including their serialization), request
//! body, responses, headers, callbacks and security requirements; 3.1
//! webhooks; and every schema, lowered by the JSON Schema front end in the
//! dialect the description uses (the OpenAPI 3.0 Schema Object, or JSON Schema
//! 2020-12 / `jsonSchemaDialect` for 3.1). Every entry of
//! `components/schemas` becomes a named type, whether or not an operation
//! uses it.
//!
//! References between non-schema objects (`#/components/parameters/Limit`,
//! `common.yaml#/responses/NotFound`) are followed through the same sandboxed
//! document store as schema references.

pub mod codes;
mod context;
mod operations;
mod swagger;

use std::collections::HashMap;

use faultline_doc::{DocumentStore, StoreError, UriRef};
use faultline_ir::{Api, ApiFormat, Callback, SecurityRequirement};
use faultline_jsonschema::{Dialect, Lowered, Lowerer, Options};
use faultline_source::{Diagnostic, Diagnostics};

use context::{At, Context};
use operations::ItemKey;

/// Lowers the OpenAPI description at `entry` into an API named `name`.
pub fn lower_openapi(entry: &UriRef, store: &mut DocumentStore<'_>, name: &str) -> Lowered {
    let mut diagnostics = Diagnostics::new();
    let document = match store.get(entry) {
        Ok(document) => document,
        Err(StoreError::Load(e)) => {
            diagnostics.push(
                Diagnostic::error(format!("cannot load {entry}: {e}"))
                    .with_code(codes::NOT_OPENAPI),
            );
            diagnostics.extend(store.take_diagnostics());
            return Lowered {
                api: None,
                diagnostics,
            };
        }
        Err(StoreError::Parse) => {
            return Lowered {
                api: None,
                diagnostics: store.take_diagnostics(),
            };
        }
    };
    let root = At::root(document);
    let Some((format, dialect)) = detect_version(&root, &mut diagnostics) else {
        diagnostics.extend(store.take_diagnostics());
        return Lowered {
            api: None,
            diagnostics,
        };
    };

    let mut api = Api::new(name, format);
    api.version = root
        .get("info")
        .and_then(|info| info.get("version"))
        .and_then(|v| v.as_str())
        .map(str::to_owned);

    let mut lowerer = Lowerer::new(
        &mut api,
        store,
        Options {
            default_dialect: dialect,
        },
    );
    // Registers the entry document with the lowerer's reference registry.
    if lowerer.document(entry).is_err() {
        return Lowered {
            api: None,
            diagnostics: lowerer.finish(),
        };
    }
    let mut context = Context {
        lowerer: &mut lowerer,
        dialect,
        diagnostics,
    };
    if format == ApiFormat::Swagger2 {
        swagger::lower_description(&mut context, &root);
    } else {
        lower_description(&mut context, &root);
    }
    let own = context.diagnostics;
    let mut all = lowerer.finish();
    all.extend(own);
    all.sort();
    Lowered {
        api: Some(api),
        diagnostics: all,
    }
}

/// Reads `openapi`/`swagger` and chooses the schema dialect.
fn detect_version(root: &At, diagnostics: &mut Diagnostics) -> Option<(ApiFormat, Dialect)> {
    let span = root.span();
    if let Some(swagger) = root.get("swagger") {
        // `swagger: 2.0` unquoted is a YAML number.
        let two = match &swagger.value {
            faultline_doc::Value::String(text) => text == "2.0",
            faultline_doc::Value::Number(number) => {
                matches!(number.to_string().as_str(), "2" | "2.0")
            }
            _ => false,
        };
        if two {
            return Some((ApiFormat::Swagger2, Dialect::Swagger2));
        }
        diagnostics.push(
            Diagnostic::error("only Swagger 2.0 is supported among Swagger versions")
                .with_code(codes::SWAGGER_2)
                .with_primary(swagger.span, ""),
        );
        return None;
    }
    let version = root.get("openapi").and_then(|v| v.as_str());
    match version {
        Some(v) if v.starts_with("3.0.") || v == "3.0" => {
            Some((ApiFormat::OpenApi30, Dialect::OpenApi30))
        }
        Some(v) if v.starts_with("3.1.") || v == "3.1" => {
            let dialect = root
                .get("jsonSchemaDialect")
                .and_then(|d| d.as_str())
                .and_then(Dialect::from_meta_schema)
                .unwrap_or(Dialect::Draft2020_12);
            Some((ApiFormat::OpenApi31, dialect))
        }
        _ => {
            let (message, at) = match version {
                Some(v) => (
                    format!("unsupported OpenAPI version {v}"),
                    root.get("openapi").map_or(span, |n| n.span),
                ),
                None => (
                    "not an OpenAPI description: missing \"openapi\"".to_owned(),
                    span,
                ),
            };
            diagnostics.push(
                Diagnostic::error(message)
                    .with_code(codes::NOT_OPENAPI)
                    .with_primary(at, "")
                    .with_note("OpenAPI 3.0 and 3.1 are supported"),
            );
            None
        }
    }
}

fn lower_description(context: &mut Context<'_, '_, '_>, root: &At) {
    let security = root
        .child("security")
        .map(|s| context.security(&s))
        .unwrap_or_default();
    let security: Vec<SecurityRequirement> = security;

    let mut seen: HashMap<String, faultline_source::Span> = HashMap::new();
    if let Some(paths) = root.child("paths") {
        for (template, item) in paths.entries() {
            if template.starts_with("x-") {
                continue;
            }
            let key_span = paths
                .object()
                .and_then(|o| o.member(&template))
                .map_or_else(|| item.span(), |m| m.key_span);
            if !template.starts_with('/') {
                context.error(
                    codes::INVALID_PATH,
                    "path templates must start with '/'",
                    key_span,
                );
            }
            for op in context.path_item(&template, ItemKey::Template, item, &security) {
                let identity = op.key.identity();
                if let Some(first) = seen.get(&identity) {
                    context.diagnostics.push(
                        Diagnostic::error(format!("operation {identity} is declared twice"))
                            .with_code(codes::DUPLICATE_OPERATION)
                            .with_primary(
                                key_span,
                                "paths differing only in parameter names are the same endpoint",
                            )
                            .with_secondary(*first, "first declared here"),
                    );
                    continue;
                }
                seen.insert(identity, key_span);
                context.lowerer.api_mut().operations.push(op);
            }
        }
    }

    if let Some(webhooks) = root.child("webhooks") {
        for (name, item) in webhooks.entries() {
            let origin = Some(item.span());
            let operations = context.path_item(&name, ItemKey::Expression, item, &security);
            context.lowerer.api_mut().webhooks.push(Callback {
                name,
                operations,
                origin,
            });
        }
    }

    if let Some(schemas) = root.child("components").and_then(|c| c.child("schemas")) {
        for (_, schema) in schemas.entries() {
            context.schema(&schema);
        }
    }
}

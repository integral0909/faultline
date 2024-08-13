//! The AsyncAPI front end.
//!
//! Lowers AsyncAPI 2.x and 3.0 descriptions into the Faultline IR. Both
//! versions are lowered from the application's point of view, so that a
//! description migrated from 2.6 to 3.0 compares cleanly with its old self:
//!
//! * An operation is *receive* (2.x `publish`: others publish to the
//!   application) or *send* (2.x `subscribe`: others subscribe to what the
//!   application publishes), identified by its action and channel address.
//! * Received messages flow in, like request bodies; sent messages flow
//!   out. Send operations are lowered as the API's own requests (webhooks
//!   in OpenAPI terms), so their messages and channel parameters get the
//!   outward direction. A 3.0 `reply` is the operation's response.
//! * Each message becomes an object with its `contentType`, its `headers`
//!   (when declared) and its `payload`, keyed by message name
//!   (`message[OrderPlaced]` in paths).
//!
//! Payloads are JSON Schema (the AsyncAPI Schema Object, a superset of
//! draft-07), OpenAPI 3.0 Schema Objects, or Avro schemas; Avro payloads are
//! lowered by the Avro front end into the same type graph, so an AsyncAPI
//! description can carry Avro and JSON messages side by side.

pub mod codes;
mod context;
mod lower;
mod message;

use faultline_doc::{DocumentStore, StoreError, UriRef};
use faultline_ir::{Api, ApiFormat};
use faultline_jsonschema::nav::At;
use faultline_jsonschema::{Dialect, Lowered, Lowerer, Options};
use faultline_source::{Diagnostic, Diagnostics};

use context::Context;

/// The name of the webhook that holds an API's send operations.
pub const SENT: &str = "sent";

/// Lowers the AsyncAPI description at `entry` into an API named `name`.
pub fn lower_asyncapi(entry: &UriRef, store: &mut DocumentStore<'_>, name: &str) -> Lowered {
    let mut diagnostics = Diagnostics::new();
    let document = match store.get(entry) {
        Ok(document) => document,
        Err(StoreError::Load(e)) => {
            diagnostics.push(
                Diagnostic::error(format!("cannot load {entry}: {e}"))
                    .with_code(codes::NOT_ASYNCAPI),
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
    let Some(format) = detect_version(&root, &mut diagnostics) else {
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
            default_dialect: Dialect::Draft7,
        },
    );
    if lowerer.document(entry).is_err() {
        return Lowered {
            api: None,
            diagnostics: lowerer.finish(),
        };
    }
    let default_content_type = root
        .get("defaultContentType")
        .and_then(|v| v.as_str())
        .unwrap_or("application/json")
        .to_owned();
    let mut context = Context {
        lowerer: &mut lowerer,
        diagnostics,
        default_content_type,
        v3: format == ApiFormat::AsyncApi3,
    };
    if context.v3 {
        lower::version_3(&mut context, &root);
    } else {
        lower::version_2(&mut context, &root);
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

/// Reads `asyncapi`: 2.0 to 2.6, or 3.0.
fn detect_version(root: &At, diagnostics: &mut Diagnostics) -> Option<ApiFormat> {
    let version = root.get("asyncapi").and_then(|v| v.as_str());
    let supported = match version {
        Some(v) if v.starts_with("2.") => Some(ApiFormat::AsyncApi2),
        Some(v) if v == "3.0" || v.starts_with("3.0.") => Some(ApiFormat::AsyncApi3),
        _ => None,
    };
    if supported.is_none() {
        let (message, at) = match version {
            Some(v) => (
                format!("unsupported AsyncAPI version {v}"),
                root.get("asyncapi").map_or(root.span(), |n| n.span),
            ),
            None => (
                "not an AsyncAPI description: missing \"asyncapi\"".to_owned(),
                root.span(),
            ),
        };
        diagnostics.push(
            Diagnostic::error(message)
                .with_code(codes::NOT_ASYNCAPI)
                .with_primary(at, "")
                .with_note("AsyncAPI 2.0 to 2.6 and 3.0 are supported"),
        );
    }
    supported
}

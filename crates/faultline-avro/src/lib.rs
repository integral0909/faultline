//! Apache Avro schema front end.
//!
//! [`lower_avro`] reads an Avro schema (`.avsc`) or protocol (`.avpr`),
//! checks it the way Avro parsers do, and lowers it into the IR as a reader sees data, so that the
//! comparison roles are Avro's compatibility modes: `input` is BACKWARD,
//! `output` FORWARD and `both` FULL.

pub mod codes;
pub mod idl;
mod lower;
mod protocol;

pub use protocol::AVRO_MEDIA_TYPE;

use faultline_doc::{DocumentStore, Node, StoreError, UriRef};
use faultline_ir::{Api, Role, TypeId};
use faultline_source::{Diagnostic, Diagnostics};

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the schema has errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// An Avro schema embedded in another format's document (an AsyncAPI
/// message payload): its types, in an API of their own, and the type of the
/// schema itself. The caller grafts the types into its own arena.
#[derive(Debug)]
pub struct Embedded {
    /// `None` when the schema has errors.
    pub lowered: Option<(Api, TypeId)>,
    pub diagnostics: Diagnostics,
}

/// Lowers the Avro schema `node`, checking it as [`lower_avro`] does.
pub fn lower_embedded(node: &Node) -> Embedded {
    let mut lowerer = lower::Lowerer::new("embedded");
    let id = lowerer.embedded(node);
    let diagnostics = lowerer.diagnostics;
    let lowered = match id {
        Some(id) if !diagnostics.has_errors() => Some((lowerer.api, id)),
        _ => None,
    };
    Embedded {
        lowered,
        diagnostics,
    }
}

/// Reads the Avro IDL file at `entry` (`.avdl`), with the files it
/// imports, and lowers the protocol or schemas it declares into an [`Api`]
/// named `name`.
pub fn lower_avro_idl(
    entry: &UriRef,
    loader: &dyn faultline_doc::loader::Loader,
    sources: &faultline_source::SourceMap,
    name: &str,
    role: Role,
) -> Lowered {
    let mut diagnostics = Diagnostics::new();
    let Some(protocol) = idl::read(entry, loader, sources, &mut diagnostics) else {
        return Lowered {
            api: None,
            diagnostics,
        };
    };
    if diagnostics.has_errors() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut lowerer = lower::Lowerer::new(name);
    lowerer.protocol(&protocol, role);
    diagnostics.extend(lowerer.diagnostics);
    diagnostics.sort();
    let api = (!diagnostics.has_errors()).then_some(lowerer.api);
    Lowered { api, diagnostics }
}

/// Reads the schema at `entry` and lowers it into an [`Api`] named `name`,
/// whose single schema root is used in `role`.
pub fn lower_avro(
    entry: &UriRef,
    store: &mut DocumentStore<'_>,
    name: &str,
    role: Role,
) -> Lowered {
    let document = store.get(entry);
    let mut diagnostics = store.take_diagnostics();
    let document = match document {
        Ok(document) => document,
        Err(StoreError::Load(e)) => {
            diagnostics.push(
                Diagnostic::error(format!("cannot read {entry}: {e}"))
                    .with_code(codes::INVALID_SCHEMA),
            );
            return Lowered {
                api: None,
                diagnostics,
            };
        }
        Err(StoreError::Parse) => {
            return Lowered {
                api: None,
                diagnostics,
            };
        }
    };
    let mut lowerer = lower::Lowerer::new(name);
    let root = &document.document.root;
    if root.get("protocol").is_some() {
        lowerer.protocol(root, role);
    } else {
        lowerer.root(root, role);
    }
    diagnostics.extend(lowerer.diagnostics);
    diagnostics.sort();
    let api = (!diagnostics.has_errors()).then_some(lowerer.api);
    Lowered { api, diagnostics }
}

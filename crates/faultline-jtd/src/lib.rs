//! JSON Type Definition (RFC 8927) front end.
//!
//! [`lower_jtd`] reads a schema, checks it as the RFC requires, and lowers
//! it into the IR: one schema root, and every definition as a named type.

pub mod codes;
mod lower;

use faultline_doc::{DocumentStore, StoreError, UriRef};
use faultline_ir::{Api, Role};
use faultline_source::{Diagnostic, Diagnostics};

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the schema has errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Reads the schema at `entry` and lowers it into an [`Api`] named `name`,
/// whose single schema root is used in `role`.
pub fn lower_jtd(entry: &UriRef, store: &mut DocumentStore<'_>, name: &str, role: Role) -> Lowered {
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
    lowerer.root(&document.document.root, role);
    diagnostics.extend(lowerer.diagnostics);
    diagnostics.sort();
    let api = (!diagnostics.has_errors()).then_some(lowerer.api);
    Lowered { api, diagnostics }
}

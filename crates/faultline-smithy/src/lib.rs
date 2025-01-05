//! The Smithy IDL front end.
//!
//! [`load`] reads a model's files, [`lexer`] and [`parser`] turn each into
//! an [`ast::Model`], [`resolve`] resolves their names into one model of
//! absolute shape ids, and [`lower_smithy`] lowers it into the IR.

pub mod ast;
pub mod codes;
pub mod lexer;
pub mod load;
mod lower;
pub mod parser;
pub mod resolve;

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_ir::{Api, Role, SchemaRoot};
use faultline_source::{Diagnostics, SourceMap};

use crate::ast::ShapeKind;

/// The result of lowering: the API, present when the model had no errors,
/// and every diagnostic.
#[derive(Debug)]
pub struct Lowered {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Lowers the Smithy model whose entry file is `entry` into an API named
/// `name`. The structures and unions of the entry file's namespace are
/// schema roots used in `role`.
pub fn lower_smithy(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
    role: Role,
) -> Lowered {
    let files = load::load(entry, loader, sources);
    let mut diagnostics = files.diagnostics;
    let Some(first) = files.files.first() else {
        return Lowered {
            api: None,
            diagnostics,
        };
    };
    let namespace = first
        .model
        .namespace
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default();
    let mut model = resolve::resolve(&files.files);
    diagnostics.extend(std::mem::take(&mut model.diagnostics));
    diagnostics.sort();
    if diagnostics.has_errors() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut lowerer = lower::Lowerer::new(&model, name, &namespace);
    let prefix = format!("{namespace}#");
    for shape in &model.shapes {
        let root = matches!(shape.kind, ShapeKind::Structure | ShapeKind::Union)
            && shape.id.starts_with(&prefix)
            && !shape.has_trait("mixin");
        if root {
            let ty = lowerer.target(&shape.id, None);
            lowerer.api.schemas.push(SchemaRoot {
                name: shape.name.clone(),
                ty,
                role,
                origin: Some(shape.span),
            });
        }
    }
    lowerer.drain();
    Lowered {
        api: Some(lowerer.api),
        diagnostics,
    }
}

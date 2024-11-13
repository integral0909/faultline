//! The Apache Thrift front end.
//!
//! [`load`] reads a `.thrift` file and the files it includes, which
//! [`parser`] turns into syntax trees; [`linker`] resolves names across
//! them, and [`lower_thrift`] lowers the result into the IR.

pub mod ast;
pub mod codes;
pub mod lexer;
pub mod linker;
pub mod load;
mod lower;
pub mod parser;

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_ir::{Api, Role};
use faultline_source::{Diagnostics, SourceMap};

/// The result of lowering: the API, present when the files had no errors,
/// and every diagnostic.
#[derive(Debug)]
pub struct Lowered {
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Lowers the Thrift file at `entry`, with the files it includes, into an
/// API named `name`. Without services, each struct of the entry file is a
/// schema root used in direction `role`.
pub fn lower_thrift(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
    role: Role,
) -> Lowered {
    let files = load::load(entry, loader, sources);
    let mut diagnostics = files.diagnostics;
    if files.files.is_empty() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let mut linked = linker::link(&files.files);
    diagnostics.extend(std::mem::take(&mut linked.diagnostics));
    diagnostics.sort();
    if diagnostics.has_errors() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let api = lower::Lowerer::new(&files.files, &linked, name).lower(role);
    Lowered {
        api: Some(api),
        diagnostics,
    }
}

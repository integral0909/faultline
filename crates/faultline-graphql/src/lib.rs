//! GraphQL schema front end.
//!
//! [`lower_graphql`] reads a schema document (SDL), merges `extend`
//! definitions, checks the type-system rules, and lowers the schema into the
//! IR: each root field is an operation, and field arguments flow opposite to
//! the values they select.

pub mod ast;
pub mod codes;
pub mod lexer;
mod lower;
pub mod parser;
pub mod schema;

use faultline_doc::UriRef;
use faultline_doc::loader::Loader;
use faultline_ir::Api;
use faultline_source::{Diagnostic, Diagnostics, SourceMap};

pub use lower::{GRAPHQL_MEDIA_TYPE, TYPENAME};

/// The result of lowering one version.
#[derive(Debug)]
pub struct Lowered {
    /// `None` when the document has errors.
    pub api: Option<Api>,
    pub diagnostics: Diagnostics,
}

/// Reads the schema at `entry` through `loader`, registering it in
/// `sources`, and lowers it into an [`Api`] named `name`.
pub fn lower_graphql(
    entry: &UriRef,
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
) -> Lowered {
    let mut diagnostics = Diagnostics::new();
    let loaded = match loader.load(entry) {
        Ok(loaded) => loaded,
        Err(e) => {
            diagnostics.push(
                Diagnostic::error(format!("cannot read {entry}: {e}")).with_code(codes::SYNTAX),
            );
            return Lowered {
                api: None,
                diagnostics,
            };
        }
    };
    let file = match sources.add(loaded.name, loaded.text) {
        Ok(file) => file,
        Err(e) => {
            diagnostics.push(Diagnostic::error(e.to_string()).with_code(codes::SYNTAX));
            return Lowered {
                api: None,
                diagnostics,
            };
        }
    };
    let (document, parse_diagnostics) = parser::parse(file.id(), file.text());
    diagnostics.extend(parse_diagnostics);
    let (schema, schema_diagnostics) = schema::build(&document);
    diagnostics.extend(schema_diagnostics);
    diagnostics.sort();
    if diagnostics.has_errors() {
        return Lowered {
            api: None,
            diagnostics,
        };
    }
    let api = lower::Lowerer::new(&schema, name).lower();
    Lowered {
        api: Some(api),
        diagnostics,
    }
}

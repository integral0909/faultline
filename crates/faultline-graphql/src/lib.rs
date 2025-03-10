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
    lower_graphql_files(std::slice::from_ref(entry), loader, sources, name)
}

/// Reads a schema split over the files `entries`, as one document: types
/// may be defined in one file and extended or used in another.
pub fn lower_graphql_files(
    entries: &[UriRef],
    loader: &dyn Loader,
    sources: &SourceMap,
    name: &str,
) -> Lowered {
    let mut diagnostics = Diagnostics::new();
    let mut document = ast::Document::default();
    for entry in entries {
        let loaded = match loader.load(entry) {
            Ok(loaded) => loaded,
            Err(e) => {
                diagnostics.push(
                    Diagnostic::error(format!("cannot read {entry}: {e}")).with_code(codes::SYNTAX),
                );
                continue;
            }
        };
        let file = match sources.add(loaded.name, loaded.text) {
            Ok(file) => file,
            Err(e) => {
                diagnostics.push(Diagnostic::error(e.to_string()).with_code(codes::SYNTAX));
                continue;
            }
        };
        let (parsed, parse_diagnostics) = parser::parse(file.id(), file.text());
        diagnostics.extend(parse_diagnostics);
        document.schemas.extend(parsed.schemas);
        document.types.extend(parsed.types);
        document.directives.extend(parsed.directives);
    }
    if diagnostics.has_errors() {
        diagnostics.sort();
        return Lowered {
            api: None,
            diagnostics,
        };
    }
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
